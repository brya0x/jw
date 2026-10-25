//! N81: `claude` typed in a pane (REQ-371 to REQ-373, REQ-395), through the real binary, a real
//! shell in a real pty and the fake engine the daemon's tests use.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{Env, JW};
use wire::Client;

const ENGINE: &str = include_str!("../../daemon/tests/fake-claude.py");

struct World {
    env: Env,
    daemon: Option<Child>,
    log: PathBuf,
    ws: String,
    pane: String,
}

fn shell_path(name: &str) -> Option<String> {
    let out = Command::new("which").arg(name).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

impl World {
    /// A daemon of its own whose panes run `shell`; without `swap` it has `JWN_CONVERSATION=0`.
    fn new(name: &str, shell: &str, swap: bool) -> World {
        let env = Env::new(name);
        let bin = env.dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let engine = bin.join("claude");
        std::fs::write(&engine, ENGINE).unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = env.dir.join("engine.log");
        let home = env.dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let mut cmd = env.jw();
        // ZDOTDIR too: the pane's zsh reads the dotfiles of ZDOTDIR, and the user's own (which
        // put ~/.local/bin, where the real claude is, first) must not reach this test.
        cmd.env("SHELL", shell)
            .env("HOME", &home)
            .env("ZDOTDIR", &home)
            // Ubuntu's /etc/zsh/zshrc runs compinit, which on the CI runner asks about insecure
            // directories at the prompt and the shell never reaches it.
            .env("skip_global_compinit", "1")
            .env("PATH", &path)
            .env("FAKE_LOG", &log)
            .env_remove(identity::env_var("PANE_ID"))
            .arg("daemon");
        if !swap {
            cmd.env(identity::env_var("CONVERSATION"), "0");
        }
        let child = cmd.spawn().unwrap();
        wait_for("the daemon", || {
            wire::Conn::connect(&env.dir.join("s.sock")).is_ok()
        });
        let proj = env.dir.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let ws = env.cmd(&format!(
            r#"{{"cmd":"open_folder","path":"{}"}}"#,
            proj.display()
        ))["result"]["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        let pane =
            env.cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#))["result"]["pane"]
                .as_str()
                .unwrap()
                .to_owned();
        World {
            env,
            daemon: Some(child),
            log,
            ws,
            pane,
        }
    }

    fn type_line(&self, line: &str) {
        let data = serde_json::to_string(&format!("{line}\r")).unwrap();
        self.env.cmd(&format!(
            r#"{{"cmd":"write","pane":"{}","data":{data}}}"#,
            self.pane
        ));
    }

    /// What the pane's own terminal shows (`read` prints the conversation's items instead while
    /// the pane has one, REQ-455).
    fn screen(&self) -> String {
        let mut c = self.client();
        c.open(&self.pane).unwrap();
        c.wait_until(Duration::from_secs(3), |c| c.nodes.contains_key(&self.pane))
            .unwrap();
        c.nodes[&self.pane]["screen"]["lines"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    fn wait_screen(&self, what: &str, text: &str) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if self.screen().contains(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "timed out waiting for {what} ({text:?}); the pane shows:\n{}",
            self.screen()
        );
    }

    fn client(&self) -> Client {
        Client::connect(&self.env.dir.join("s.sock"), "test").unwrap()
    }

    /// The pane node's `conversation`, if it names one.
    fn conversation(&self) -> Option<String> {
        let mut c = self.client();
        c.open(&self.pane).unwrap();
        c.wait_until(Duration::from_secs(3), |c| c.nodes.contains_key(&self.pane))
            .unwrap();
        c.nodes[&self.pane]["conversation"]
            .as_str()
            .map(str::to_owned)
    }

    fn engine_log(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .map(|s| s.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn stop(mut self) {
        if let Some(d) = self.daemon.take() {
            self.env.stop(d);
        }
    }
}

/// A test that fails must not leave its daemon behind: only the pid it started is ended.
impl Drop for World {
    fn drop(&mut self) {
        if let Some(mut d) = self.daemon.take() {
            let _ = d.kill();
            let _ = d.wait();
        }
    }
}

fn shells() -> Vec<String> {
    ["bash", "zsh"]
        .iter()
        .filter_map(|s| shell_path(s))
        .collect()
}

#[test]
fn claude_typed_in_a_pane_is_a_conversation_and_command_claude_is_the_original() {
    for shell in shells() {
        let w = World::new("claude-pane", &shell, true);
        let kind = Path::new(&shell)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // the function exists in the pane's shell (REQ-371)
        w.type_line(if kind == "zsh" {
            "whence -w claude"
        } else {
            "type -t claude"
        });
        w.wait_screen("the shell to say what claude is", "function");
        // typed, it is a conversation: the pane names it and the pane's terminal shows it
        w.type_line("claude");
        w.wait_screen("the conversation to open", "conversación de este panel");
        wait_for("the pane to name the conversation", || {
            w.conversation().is_some()
        });
        w.type_line("hola desde el panel");
        w.wait_screen("the answer", "● Voy a mirar.");
        w.wait_screen("the end of the turn", "✻ Listo");
        // the engine ran with the environment of the pane's shell and in its folder
        let start = w
            .engine_log()
            .into_iter()
            .find(|l| l.starts_with("start"))
            .unwrap();
        assert!(start.contains("--permission-prompt-tool"), "{start}");
        assert!(start.contains("cwd=") && start.contains("proj"), "{start}");
        // /exit ends it and the pane is the shell again
        w.type_line("/exit");
        w.wait_screen("the conversation to end", "conversación terminada");
        wait_for("the pane to let go", || w.conversation().is_none());
        // `command claude` runs the original, and so does what the engine cannot serve
        w.type_line("command claude --version");
        w.wait_screen("the original to answer", "fake-claude 1.0");
        w.type_line("claude --version");
        wait_for("the unserved flag to reach the original", || {
            w.screen().matches("fake-claude 1.0").count() >= 2
        });
        w.stop();
    }
}

#[test]
fn without_the_swap_claude_in_a_pane_is_the_original() {
    for shell in shells() {
        let w = World::new("claude-off", &shell, false);
        let kind = Path::new(&shell)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        w.type_line(if kind == "zsh" {
            "whence -w claude"
        } else {
            "type -t claude"
        });
        w.wait_screen(
            "the shell to say what claude is",
            if kind == "zsh" {
                "claude: command"
            } else {
                "file"
            },
        );
        assert!(!w.screen().contains("function"), "{}", w.screen());
        w.stop();
    }
}

#[test]
fn outside_a_pane_or_a_terminal_the_binary_runs_the_original() {
    let env = Env::new("claude-plain");
    let bin = env.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("claude");
    std::fs::write(&engine, ENGINE).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    // no pane, no terminal: not a conversation
    let out = Command::new(JW)
        .args(["claude", "--version"])
        .env("PATH", &path)
        .env_remove(identity::env_var("PANE_ID"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "fake-claude 1.0"
    );
    // in a pane's environment but with its output piped (`claude … | tee`): still the original
    let out = Command::new(JW)
        .args(["claude", "--version"])
        .env("PATH", &path)
        .env(
            identity::env_var("PANE_ID"),
            "00000000-0000-4000-8000-0000000000c1",
        )
        .env(identity::env_var("SOCKET"), env.dir.join("none.sock"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "fake-claude 1.0"
    );
    // and when there is no claude at all it says so as a shell does
    let out = Command::new(JW)
        .args(["claude"])
        .env("PATH", "/nonexistent")
        .env_remove(identity::env_var("PANE_ID"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127));
}

impl World {
    /// `claude` typed in the pane and its conversation open.
    fn open_conversation(&self) {
        self.type_line("claude");
        wait_for("the conversation", || self.conversation().is_some());
    }

    /// `jwn <args>` the way an agent runs it: from outside a pane, with a short wait for agents.
    fn agent_cmd(&self, args: &[&str]) -> std::process::Output {
        self.env
            .jw()
            .args(args)
            .env(identity::env_var("AGENT_WAIT_MS"), "6000")
            .output()
            .unwrap()
    }

    fn user_lines(&self) -> Vec<String> {
        self.engine_log()
            .into_iter()
            .filter(|l| l.starts_with("user "))
            .collect()
    }
}

fn out_of(o: &std::process::Output) -> (Option<i32>, String, String) {
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn prompt_with_a_conversation_is_a_message_and_wait_returns_the_answer_and_read_prints_the_items() {
    let Some(shell) = shells().into_iter().next() else {
        return;
    };
    let w = World::new("claude-prompt", &shell, true);
    w.open_conversation();

    // REQ-453: a message, with no wait for the screen and no paste
    let (code, stdout, _) = out_of(&w.agent_cmd(&["prompt", &w.ws, "--pane", &w.pane, "hola uno"]));
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.starts_with("sent to pane "), "{stdout}");
    wait_for("the engine to get the message", || {
        w.user_lines().iter().any(|l| l == "user \"hola uno\"")
    });
    w.wait_screen("the end of the turn", "✻ Listo");

    // REQ-454: --wait prints the final answer and nothing else on stdout
    let (code, stdout, stderr) =
        out_of(&w.agent_cmd(&["prompt", &w.ws, "--pane", &w.pane, "--wait", "hola dos"]));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "Voy a mirar.\n");
    assert!(stderr.starts_with("sent to pane "), "{stderr}");
    assert!(w.user_lines().iter().any(|l| l == "user \"hola dos\""));

    // REQ-455: read prints the last items as the plain terminal does
    let (code, stdout, _) =
        out_of(&w.agent_cmd(&["read", &w.ws, "--pane", &w.pane, "--lines", "4"]));
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("❯ hola dos") && stdout.contains("● Voy a mirar."),
        "{stdout}"
    );
    assert!(
        !stdout.contains("hola uno"),
        "only the last 4 items: {stdout}"
    );

    // the usage: --timeout goes with --wait and wants seconds
    for bad in [
        vec!["prompt", &w.ws, "--timeout", "5", "x"],
        vec!["prompt", &w.ws, "--wait", "--timeout", "0", "x"],
    ] {
        assert_eq!(w.agent_cmd(&bad).status.code(), Some(2), "{bad:?}");
    }
    w.stop();
}

#[test]
fn wait_on_a_turn_that_ends_asking_prints_the_question_and_exits_3_and_a_turn_that_lasts_times_out()
{
    let Some(shell) = shells().into_iter().next() else {
        return;
    };
    let w = World::new("claude-wait3", &shell, true);
    w.open_conversation();

    // REQ-454: a question waiting is printed and the exit is 3
    let (code, stdout, stderr) =
        out_of(&w.agent_cmd(&["prompt", &w.ws, "--pane", &w.pane, "--wait", "una pregunta"]));
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stdout.contains("? pregunta:"), "{stdout}");
    assert!(stdout.contains("1) "), "the options are numbered: {stdout}");
    // read says the same while it waits
    let (_, read, _) = out_of(&w.agent_cmd(&["read", &w.ws, "--pane", &w.pane]));
    assert!(read.contains("? pregunta:"), "{read}");
    w.stop();

    // a turn that does not end in time is an error that says so
    let w = World::new("claude-slow", &shell, true);
    w.open_conversation();
    let (code, stdout, stderr) = out_of(&w.agent_cmd(&[
        "prompt",
        &w.ws,
        "--pane",
        &w.pane,
        "--wait",
        "--timeout",
        "1",
        "algo slow",
    ]));
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stderr.contains("did not end in 1 s"), "{stderr}");
    w.stop();
}

#[test]
fn wait_needs_a_conversation() {
    let Some(shell) = shells().into_iter().next() else {
        return;
    };
    let w = World::new("claude-nowait", &shell, true);
    // the pane is a shell with no conversation
    let (code, _, stderr) =
        out_of(&w.agent_cmd(&["prompt", &w.ws, "--pane", &w.pane, "--wait", "hola"]));
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no conversation to wait on"), "{stderr}");
    w.stop();
}

#[test]
fn worktree_task_goes_to_the_new_agents_conversation_as_a_message() {
    let Some(shell) = shells().into_iter().next() else {
        return;
    };
    let w = World::new("claude-wt", &shell, true);
    let proj = w.env.dir.join("proj");
    let proj = std::fs::canonicalize(&proj).unwrap();
    common::git(&proj, &["init", "-q", "-b", "main"]);
    std::fs::write(proj.join("f"), "x").unwrap();
    common::git(&proj, &["add", "."]);
    common::git(&proj, &["commit", "-q", "-m", "init"]);
    std::fs::write(proj.join(".jw.toml"), "open = \"claude\"\n").unwrap();

    let out = w.agent_cmd(&[
        "worktree",
        "ws-1",
        "--in",
        &proj.to_string_lossy(),
        "--task",
        "haz esto",
    ]);
    let (code, stdout, stderr) = out_of(&out);
    assert_eq!(code, Some(0), "{stdout} {stderr}");
    wait_for("the task to reach the engine", || {
        w.user_lines().iter().any(|l| l == "user \"haz esto\"")
    });
    w.stop();
}

/// REQ-862, REQ-856: `jw stop` asks the daemon to stop (it does not signal it), so an engine with an ask waiting
/// is ended at once with its input open (Claude would reject the ask itself) and an engine with none has its
/// input closed and ends by itself. Through the real binary: the daemon's own tests call `Daemon::stop` and
/// never saw that a signal does not run it.
#[test]
fn jw_stop_asks_the_daemon_to_stop_so_an_ask_is_kept_and_the_other_engines_are_closed() {
    let Some(shell) = shells().into_iter().next() else {
        return;
    };
    for (said, asks) in [("please edit auth", true), ("hola", false)] {
        let mut w = World::new("claude-stop-ask", &shell, true);
        w.open_conversation();
        let node = w.conversation().unwrap();
        w.env.cmd(&format!(
            r#"{{"cmd":"conversation_send","node":"{node}","text":"{said}"}}"#
        ));
        let mut c = w.client();
        c.open(&node).unwrap();
        let want = if asks { "asking" } else { "idle" };
        c.wait_until(Duration::from_secs(10), |c| {
            c.nodes.get(&node).is_some_and(|n| {
                n["status"] == want
                    && (asks
                        || n["items"]
                            .as_array()
                            .is_some_and(|i| i.iter().any(|i| i["t"] == "result")))
            })
        })
        .unwrap();
        let child = w.daemon.take().unwrap();
        w.env.stop(child);
        if !asks {
            wait_for("the engine's input to close", || {
                w.engine_log().iter().any(|l| l == "eof")
            });
        }
        let eof = w.engine_log().iter().any(|l| l == "eof");
        assert_eq!(
            eof,
            !asks,
            "ask waiting: {asks}; engine log: {:?}",
            w.engine_log()
        );
    }
}
