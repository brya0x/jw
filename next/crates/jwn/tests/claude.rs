//! N81: `claude` typed in a pane (REQ-371 to REQ-373, REQ-395), through the real binary, a real
//! shell in a real pty and the fake engine the daemon's tests use.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{Env, JWN};
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
    /// A daemon of its own whose panes run `shell`; `swap` is `JWN_CONVERSATION=1` for it.
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
        let mut cmd = env.jwn();
        cmd.env("SHELL", shell)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("FAKE_LOG", &log)
            .env_remove("JWN_PANE_ID")
            .arg("daemon");
        if swap {
            cmd.env("JWN_CONVERSATION", "1");
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

    fn screen(&self) -> String {
        let out = self
            .env
            .jwn()
            .args(["read", &self.ws, "--pane", &self.pane, "--lines", "60"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
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
    let out = Command::new(JWN)
        .args(["claude", "--version"])
        .env("PATH", &path)
        .env_remove("JWN_PANE_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "fake-claude 1.0"
    );
    // in a pane's environment but with its output piped (`claude … | tee`): still the original
    let out = Command::new(JWN)
        .args(["claude", "--version"])
        .env("PATH", &path)
        .env("JWN_PANE_ID", "00000000-0000-4000-8000-0000000000c1")
        .env("JWN_SOCKET", env.dir.join("none.sock"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "fake-claude 1.0"
    );
    // and when there is no claude at all it says so as a shell does
    let out = Command::new(JWN)
        .args(["claude"])
        .env("PATH", "/nonexistent")
        .env_remove("JWN_PANE_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127));
}
