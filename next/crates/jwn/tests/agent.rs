//! N80: the commands an agent uses, through the real binary against a daemon of its own and
//! real git (REQ-353 to REQ-370). Only the pid the daemon was started with is ever signalled.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use common::{Env, git};
use serde_json::Value;

struct World {
    env: Env,
    daemon: Option<std::process::Child>,
    repo: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let env = Env::new(name);
        let repo = std::fs::canonicalize(&env.dir).unwrap().join("work/app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let daemon = Some(env.start());
        World { env, daemon, repo }
    }

    /// `jwn <args>` as a command running in the pane `pane`, when it names one.
    fn run(&self, pane: Option<&str>, args: &[&str]) -> Output {
        let mut c = self.env.jwn();
        c.args(args)
            .env(identity::env_var("AGENT_WAIT_MS"), "3000")
            .current_dir(&self.repo);
        match pane {
            Some(p) => c.env(identity::env_var("PANE_ID"), p),
            None => c.env_remove(identity::env_var("PANE_ID")),
        };
        c.output().unwrap()
    }

    fn ok(&self, pane: Option<&str>, args: &[&str]) -> String {
        let out = self.run(pane, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fails(&self, pane: Option<&str>, args: &[&str], code: i32) -> String {
        let out = self.run(pane, args);
        assert_eq!(out.status.code(), Some(code), "{args:?}");
        String::from_utf8(out.stderr).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(None, args)).unwrap()
    }

    /// The folder of the repository as a workspace with one shell: its id and its pane's.
    fn open_repo(&self) -> (String, String) {
        let ws = self.env.cmd(&format!(
            r#"{{"cmd":"open_folder","path":"{}"}}"#,
            self.repo.display()
        ))["result"]["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        let pane = self
            .env
            .cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#))["result"]["pane"]
            .as_str()
            .unwrap()
            .to_owned();
        (ws, pane)
    }

    /// A program that stands in for an agent: it prints that it is ready, takes one line and
    /// records what it got in `got`. It is called `claude`, so it is an agent to the daemon.
    fn agent(&self, name: &str) -> (PathBuf, PathBuf) {
        let dir = self.env.dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("claude");
        let got = dir.join("got.txt");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf 'agent ready\\n'\nIFS= read -r line\nprintf '%s' \"$line\" > \"$1\"\nsleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (program, got)
    }

    fn stop(mut self) {
        if let Some(d) = self.daemon.take() {
            self.env.stop(d);
        }
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn ls_and_pane_ls_say_what_there_is_in_text_and_in_json_and_bad_uses_exit_2() {
    let w = World::new("ag-ls");
    let (_, pane) = w.open_repo();

    let rows = w.ok(None, &["ls"]);
    assert_eq!(rows, "app\tfolder\t-\t1 panes\t-\n");
    let json = w.json(&["ls", "--json"]);
    assert_eq!(json[0]["name"], "app");
    assert_eq!(
        (json[0]["kind"].as_str(), json[0]["panes"].as_u64()),
        (Some("folder"), Some(1))
    );
    assert_eq!(json[0]["path"], text(&w.repo));
    assert_eq!(json[0]["attention"], serde_json::json!([]));

    let panes = w.ok(None, &["pane", "ls", "app"]);
    assert_eq!(panes, format!("{pane}\tsh\t-\t\n"));
    let json = w.json(&["pane", "ls", "app", "--json"]);
    assert_eq!(
        (json[0]["id"].as_str(), json[0]["title"].as_str()),
        (Some(pane.as_str()), Some("sh"))
    );
    assert!(json[0].get("agent").is_none());

    assert!(
        w.fails(None, &["pane", "ls", "nope"], 1)
            .contains("no workspace is named «nope»")
    );
    let usage = w.fails(None, &["ls", "--bogus"], 2);
    assert!(
        usage.contains("unknown flag --bogus") && usage.contains("usage: jwn ls [--json]"),
        "{usage}"
    );
    assert!(w.fails(None, &["pane"], 2).contains("usage: jwn pane ls"));
    w.stop();
}

#[test]
fn without_a_daemon_every_command_but_skill_says_so_and_exits_1() {
    let env = Env::new("ag-none");
    let run = |args: &[&str]| env.jwn().args(args).output().unwrap();
    for args in [
        &["ls"][..],
        &["read", "x"],
        &["prompt", "x", "hi"],
        &["worktree", "x"],
        &["pane", "ls", "x"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("no daemon answers"),
            "{args:?}"
        );
    }
    assert!(run(&["skill"]).status.success());
}

#[test]
fn read_prints_the_last_lines_of_the_screen_and_says_when_there_are_no_commands_to_read() {
    let w = World::new("ag-read");
    let (_, pane) = w.open_repo();
    w.env.cmd(&format!(
        r#"{{"cmd":"write","pane":"{pane}","data":"echo marker-one; echo marker-two\r"}}"#
    ));
    wait_for("the output", || {
        w.ok(None, &["read", "app"]).contains("marker-two")
    });
    let screen = w.ok(None, &["read", "app"]);
    assert!(
        screen.contains("marker-one") && !screen.ends_with("\n\n"),
        "{screen:?}"
    );
    let last = w.ok(None, &["read", "app", "--pane", "sh", "--lines", "1"]);
    assert_eq!(last.trim_end().lines().count(), 1, "{last:?}");
    assert!(
        w.fails(None, &["read", "app", "--blocks"], 1)
            .contains("no commands to read")
    );
    assert!(
        w.fails(None, &["read", "app", "--lines", "0"], 2)
            .contains("--lines")
    );
    assert!(
        w.fails(None, &["read", "app", "--pane", "zzz"], 1)
            .contains("no pane")
    );
    w.stop();
}

#[test]
fn prompt_pastes_into_the_agent_with_enter_after_and_refuses_what_it_should() {
    let w = World::new("ag-prompt");
    let (_, shell) = w.open_repo();
    // No agent yet.
    assert!(
        w.fails(None, &["prompt", "app", "hello"], 1)
            .contains("no agent runs in app")
    );

    let (program, got) = w.agent("a1");
    let agent = w
        .ok(
            None,
            &["pane", "open", "app", "--", &text(&program), &text(&got)],
        )
        .trim()
        .to_owned();
    wait_for("the agent to be seen", || {
        w.ok(None, &["pane", "ls", "app"]).contains("claude")
    });
    let listed = w.json(&["pane", "ls", "app", "--json"]);
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == agent.as_str())
        .unwrap();
    assert_eq!(row["agent"], "claude");

    // It refuses to write into the pane it runs in, and to tag when it runs in none.
    assert!(
        w.fails(Some(&agent), &["prompt", "app", "hi"], 1)
            .contains("the pane this command runs in")
    );
    assert!(
        w.fails(None, &["prompt", "app", "--tag", "hi"], 1)
            .contains("--tag needs")
    );

    // From the shell's pane, tagged: it waits for the agent to settle, pastes, then Enter.
    let said = w.ok(
        Some(&shell),
        &["prompt", "app", "--tag", "--", "-do", "the", "thing"],
    );
    assert!(
        said.contains(&format!("sent to pane {agent} of app")),
        "{said}"
    );
    wait_for("what the agent got", || {
        std::fs::read_to_string(&got).is_ok_and(|s| !s.is_empty())
    });
    let sent = std::fs::read_to_string(&got).unwrap();
    assert_eq!(
        sent,
        format!("\x1b[200~[jwn: app, pane {shell}] -do the thing\x1b[201~")
    );
    w.stop();
}

#[test]
fn a_screen_that_never_settles_fails_without_writing_and_now_writes_at_once() {
    let w = World::new("ag-busy");
    w.open_repo();
    let dir = w.env.dir.join("busy");
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("claude");
    let got = dir.join("got.txt");
    std::fs::write(
        &program,
        "#!/bin/sh\n(IFS= read -r line < /dev/tty; printf '%s' \"$line\" > \"$1\") &\nwhile :; do date +%N; sleep 0.1; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    w.ok(
        None,
        &["pane", "open", "app", "--", &text(&program), &text(&got)],
    );
    wait_for("the agent to be seen", || {
        w.ok(None, &["pane", "ls", "app"]).contains("claude")
    });

    let why = w.fails(None, &["prompt", "app", "later"], 1);
    assert!(
        why.contains("never settled") && why.contains("--now"),
        "{why}"
    );
    assert!(!got.exists(), "nothing was written");

    w.ok(None, &["prompt", "app", "--now", "now"]);
    wait_for("what the agent got", || {
        std::fs::read_to_string(&got).is_ok_and(|s| s.contains("now"))
    });
    w.stop();
}

#[test]
fn worktree_makes_it_with_the_repositorys_settings_and_hands_its_agent_a_task() {
    let w = World::new("ag-wt");
    let (_, shell) = w.open_repo();
    let (program, got) = w.agent("a2");
    std::fs::write(
        w.repo.join(".jw.toml"),
        format!(
            "branch = \"bryan/{{name}}\"\nsetup = [\"true\"]\n[layout]\nrun = \"{} {}\"\n",
            program.display(),
            got.display()
        ),
    )
    .unwrap();

    // From the pane of the repository's own workspace: no `--in`.
    let made = w.ok(Some(&shell), &["worktree", "ws-1", "--task", "build it"]);
    let (id, path) = made.trim().split_once(' ').unwrap();
    assert_eq!(path, text(&w.repo.parent().unwrap().join("app-wt/ws-1")));
    assert!(w.repo.parent().unwrap().join("app-wt/ws-1/f").exists());
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&w.repo)
        .args(["branch", "--list", "bryan/ws-1"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("bryan/ws-1"));
    // Its panes: the layout's agent and the setup.
    let panes = w.json(&["pane", "ls", id, "--json"]);
    assert_eq!(panes.as_array().unwrap().len(), 2);
    wait_for("the task", || {
        std::fs::read_to_string(&got).is_ok_and(|s| s.contains("build it"))
    });
    assert_eq!(
        std::fs::read_to_string(&got).unwrap(),
        "\x1b[200~build it\x1b[201~"
    );

    // Named by `--in` (a workspace or a folder), with a branch and a base of its own.
    let made = w.ok(
        None,
        &[
            "worktree", "ws-2", "--in", "app", "--branch", "feat/own", "--from", "main",
        ],
    );
    assert!(made.contains("app-wt/ws-2"), "{made}");
    let made = w.ok(None, &["worktree", "ws-3", "--in", &text(&w.repo)]);
    assert!(made.contains("app-wt/ws-3"), "{made}");
    // What the daemon refuses is said, and a worktree that already exists is not made twice.
    assert!(
        w.fails(None, &["worktree", "ws-3", "--in", &text(&w.repo)], 1)
            .contains("worktree")
    );
    w.stop();
}

#[test]
fn a_task_for_a_worktree_with_no_agent_says_so_after_making_it() {
    let w = World::new("ag-noagent");
    w.open_repo();
    let err = w.fails(
        None,
        &["worktree", "ws-1", "--in", "app", "--task", "go"],
        1,
    );
    assert!(
        err.contains("no agent took the task") && err.contains("app-wt/ws-1"),
        "{err}"
    );
    assert!(
        w.repo.parent().unwrap().join("app-wt/ws-1").exists(),
        "it was made"
    );
    w.stop();
}

#[test]
fn a_settings_file_the_daemon_refuses_makes_nothing_and_the_message_names_it() {
    let w = World::new("ag-badstart");
    w.open_repo();
    std::fs::write(w.repo.join(".jw.toml"), "setup = [").unwrap();
    let err = w.fails(None, &["worktree", "ws-1", "--in", "app"], 1);
    assert!(err.contains(".jw.toml"), "{err}");
    assert!(
        !w.repo.parent().unwrap().join("app-wt/ws-1").exists(),
        "nothing was made"
    );
    w.stop();
}

#[test]
fn pane_open_splits_and_close_refuses_its_own_pane_and_the_last_one() {
    let w = World::new("ag-pane");
    let (_, first) = w.open_repo();
    let second = w
        .ok(None, &["pane", "open", "app", "--split", "down"])
        .trim()
        .to_owned();
    assert_ne!(first, second);
    assert_eq!(
        w.json(&["pane", "ls", "app", "--json"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        w.fails(None, &["pane", "open", "app", "--split", "sideways"], 2)
            .contains("--split")
    );

    // Refuses its own pane without --force.
    assert!(
        w.fails(Some(&second), &["pane", "close", "app", &second], 1)
            .contains("the pane this command runs in")
    );
    w.ok(Some(&first), &["pane", "close", "app", &second]);
    wait_for("the pane to close", || {
        w.json(&["pane", "ls", "app", "--json"])
            .as_array()
            .unwrap()
            .len()
            == 1
    });
    // The last pane, too.
    assert!(
        w.fails(None, &["pane", "close", "app", &first], 1)
            .contains("the last pane")
    );
    w.ok(None, &["pane", "close", "app", &first, "--force"]);
    wait_for("the last pane to close", || {
        w.json(&["pane", "ls", "app", "--json"])
            .as_array()
            .unwrap()
            .is_empty()
    });
    w.stop();
}

#[test]
fn skill_prints_the_skill_and_installs_it_under_the_claude_configuration() {
    let env = Env::new("ag-skill");
    let out = env.jwn().arg("skill").output().unwrap();
    let text_out = String::from_utf8(out.stdout).unwrap();
    assert!(text_out.starts_with("---\nname: jwn\n"), "{text_out}");
    assert!(text_out.contains(&identity::env_var("PANE_ID")) && !text_out.contains("{bin}"));

    let config = env.dir.join("claude-config");
    let out = env
        .jwn()
        .args(["skill", "install"])
        .env("CLAUDE_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let path = config.join("skills/jwn/SKILL.md");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("installed {}\n", path.display())
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text_out);
    // Again: it is written over, whole.
    assert!(
        env.jwn()
            .args(["skill", "install"])
            .env("CLAUDE_CONFIG_DIR", &config)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        env.jwn()
            .args(["skill", "nope"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn worktree_says_on_its_error_output_what_it_copied_and_what_it_did_not_and_prints_as_before() {
    let w = World::new("ag-copy");
    w.open_repo();
    std::fs::write(
        w.repo.join(".jw.toml"),
        "copy = [\".env\", \"nada\"]\nports = { web = 0, api = 3 }\n",
    )
    .unwrap();
    std::fs::write(w.repo.join(".env"), "A=1\n").unwrap();
    let out = w.run(None, &["worktree", "ws-1", "--in", "app"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout.trim().split(' ').count(),
        2,
        "an id and a path, as before: {stdout}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("worktree: copied .env"), "{stderr}");
    assert!(
        stderr.contains("worktree: ports api=20003 web=20000"),
        "{stderr}"
    );
    assert!(
        stderr.contains("did not copy nada: it is not in the checkout"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(w.repo.parent().unwrap().join("app-wt/ws-1/.env")).unwrap(),
        "A=1\n"
    );
    w.stop();
}
