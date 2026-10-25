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
        // What a panel that opens with `claude` finds: a `bin` the tests put an agent in.
        let bin = env.dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Only the system's folders besides it, so that the person's own `claude` is never the one
        // a test starts.
        let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.display());
        // Where Claude's transcripts are looked for: a folder of the test's own.
        let config = env.dir.join("claude-config").display().to_string();
        let daemon = Some(env.start_with(&[("PATH", path), ("CLAUDE_CONFIG_DIR", config)]));
        World { env, daemon, repo }
    }

    /// `jw <args>` as a command running in the pane `pane`, when it names one.
    fn run(&self, pane: Option<&str>, args: &[&str]) -> Output {
        let mut c = self.env.jw();
        c.args(args)
            .env(identity::env_var("AGENT_WAIT_MS"), "3000")
            .current_dir(&self.repo);
        match pane {
            Some(p) => c.env(identity::env_var("PANE_ID"), p),
            None => c.env_remove(identity::env_var("PANE_ID")),
        };
        c.output().unwrap()
    }

    /// `ok`, for a command that waits for an agent that will not come (it gives up after 200 ms).
    fn ok_quick(&self, args: &[&str]) -> String {
        let mut c = self.env.jw();
        c.args(args)
            .env(identity::env_var("AGENT_WAIT_MS"), "200")
            .env_remove(identity::env_var("PANE_ID"))
            .current_dir(&self.repo);
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
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

    /// The same stand-in, as the `claude` the daemon's panes find on their `PATH`, writing what it
    /// got to the file that is returned.
    fn agent_on_path(&self) -> PathBuf {
        let got = self.env.dir.join("bin/got.txt");
        let program = self.env.dir.join("bin/claude");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf 'agent ready\\n'\nIFS= read -r line\nprintf '%s' \"$line\" > {}\nsleep 30\n",
                got.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        got
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
    assert_eq!(
        rows, "app\tfolder\t-\t1 panes\t-\t-\n",
        "REQ-701: the group is the last column"
    );
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
        usage.contains("unknown flag --bogus") && usage.contains("usage: jw ls [--json]"),
        "{usage}"
    );
    assert!(w.fails(None, &["pane"], 2).contains("usage: jw pane ls"));
    w.stop();
}

#[test]
fn group_makes_lists_renames_moves_and_deletes_groups_and_ls_says_the_group_of_each_workspace() {
    let w = World::new("ag-group");
    w.open_repo();
    assert_eq!(w.ok(None, &["group", "ls"]), "");
    let id = w.ok(None, &["group", "new", "Product"]).trim().to_owned();
    assert_eq!(id.len(), 36, "it prints the id of the group");
    assert_eq!(w.ok(None, &["group", "ls"]), "Product\t0 workspaces\n");
    // a group is named without regard to case, or by its id
    w.ok(None, &["group", "move", "app", "product"]);
    assert_eq!(w.ok(None, &["ls"]), "app\tfolder\t-\t1 panes\t-\tProduct\n");
    assert_eq!(w.json(&["ls", "--json"])[0]["group"], "Product");
    let listed = w.json(&["group", "ls", "--json"]);
    assert_eq!(
        (
            listed[0]["id"].as_str(),
            listed[0]["name"].as_str(),
            listed[0]["workspaces"].as_u64()
        ),
        (Some(id.as_str()), Some("Product"), Some(1))
    );
    w.ok(None, &["group", "rename", &id, "Sales assist"]);
    assert_eq!(w.ok(None, &["group", "ls"]), "Sales assist\t1 workspaces\n");
    // what the daemon refuses is said, and a bad use is exit 2
    assert!(
        w.fails(None, &["group", "new", "SALES ASSIST"], 1)
            .contains("already")
    );
    assert!(
        w.fails(None, &["group", "rm", "nope"], 1)
            .contains("no group is named «nope»")
    );
    assert!(
        w.fails(None, &["group", "move", "app", "nope"], 1)
            .contains("no group is named «nope»")
    );
    assert!(
        w.fails(None, &["group", "move"], 2)
            .contains("usage: jw group")
    );
    assert!(w.fails(None, &["group"], 2).contains("usage: jw group ls"));
    // out of every group, and deleted: the workspace stays
    w.ok(None, &["group", "move", "app"]);
    assert_eq!(w.ok(None, &["ls"]), "app\tfolder\t-\t1 panes\t-\t-\n");
    w.ok(None, &["group", "move", "app", "sales assist"]);
    w.ok(None, &["group", "rm", "Sales Assist"]);
    assert_eq!(w.ok(None, &["group", "ls"]), "");
    assert_eq!(w.ok(None, &["ls"]), "app\tfolder\t-\t1 panes\t-\t-\n");
    assert!(w.json(&["ls", "--json"])[0].get("group").is_none());
    w.stop();
}

#[test]
fn without_a_daemon_every_command_but_skill_says_so_and_exits_1() {
    let env = Env::new("ag-none");
    let run = |args: &[&str]| env.jw().args(args).output().unwrap();
    for args in [
        &["ls"][..],
        &["read", "x"],
        &["prompt", "x", "hi"],
        &["worktree", "x"],
        &["pane", "ls", "x"],
        &["group", "ls"],
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
        format!(
            "\x1b[200~[{}: app, pane {shell}] -do the thing\x1b[201~",
            identity::get().name
        )
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
    let got = w.agent_on_path();
    std::fs::write(w.repo.join(".jw.toml"), "setup = [\"true\"]\n").unwrap();

    // From the pane of the repository's own workspace: no `--in`.
    let made = w.ok(Some(&shell), &["worktree", "ws-1", "--task", "build it"]);
    let (id, path) = made.trim().split_once(' ').unwrap();
    assert_eq!(path, text(&w.repo.parent().unwrap().join("app-wt/ws-1")));
    assert!(w.repo.parent().unwrap().join("app-wt/ws-1/f").exists());
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&w.repo)
        .args(["branch", "--list", "ws-1"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains("ws-1"),
        "REQ-512"
    );
    // Its one panel: the setup ran in it and then the agent started (`--task` opens `claude`).
    let panes = w.json(&["pane", "ls", id, "--json"]);
    assert_eq!(panes.as_array().unwrap().len(), 1);
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
    // A prefix in the name is part of the branch and the folder is its last part (REQ-512).
    let made = w.ok(None, &["worktree", "fix/login", "--in", &text(&w.repo)]);
    assert!(made.contains("app-wt/login"), "{made}");
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&w.repo)
        .args(["branch", "--list", "fix/login"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("fix/login"));
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
    let out = env.jw().arg("skill").output().unwrap();
    let text_out = String::from_utf8(out.stdout).unwrap();
    let name = &identity::get().name;
    assert!(
        text_out.starts_with(&format!("---\nname: {name}\n")),
        "{text_out}"
    );
    assert!(text_out.contains(&identity::env_var("PANE_ID")) && !text_out.contains("{bin}"));

    let config = env.dir.join("claude-config");
    let out = env
        .jw()
        .args(["skill", "install"])
        .env("CLAUDE_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let path = config.join(format!("skills/{name}/SKILL.md"));
    let tasks = config.join(format!("skills/{name}-tasks/SKILL.md"));
    // both skills are written (REQ-915)
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!(
            "installed {}\ninstalled {}\n",
            path.display(),
            tasks.display()
        )
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text_out);
    let second = std::fs::read_to_string(&tasks).unwrap();
    assert!(
        second.starts_with(&format!("---\nname: {name}-tasks\n")),
        "{second}"
    );
    assert!(second.contains(&format!("{name} tasks loop on")) && !second.contains("{bin}"));
    // Again: it is written over, whole.
    assert!(
        env.jw()
            .args(["skill", "install"])
            .env("CLAUDE_CONFIG_DIR", &config)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        env.jw()
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

#[test]
fn chat_makes_archives_resumes_and_removes_conversations_and_ls_says_which_are_chats() {
    let w = World::new("ag-chat");
    w.open_repo();
    let made = w.ok_quick(&["chat", "new"]);
    let (id, name) = made.trim().split_once('\t').expect("id and name");
    assert_eq!((id.len(), name), (36, "conversación nueva"));
    let id = id.to_owned();
    let json = serde_json::from_str::<Value>(&w.ok_quick(&["chat", "new", "--json"])).unwrap();
    assert_eq!(json["name"], "conversación nueva");
    let other = json["id"].as_str().unwrap().to_owned();
    // `ls` says `chat` where it says the branch, and the chats come last
    let rows = w.ok(None, &["ls"]);
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(lines[0], "app\tfolder\t-\t1 panes\t-\t-", "{rows}");
    assert_eq!(lines.len(), 3, "{rows}");
    assert!(
        lines[1..]
            .iter()
            .all(|l| *l == "conversación nueva\tfolder\tchat\t1 panes\t-\t-"),
        "{rows}"
    );
    let listed = w.json(&["ls", "--json"]);
    assert!(listed[0].get("chat").is_none());
    assert!(listed[1]["chat"]["active"].as_u64().is_some());
    assert!(listed[1]["chat"].get("archived").is_none());
    // what is refused is said
    assert!(
        w.fails(None, &["chat", "archive", "app"], 1)
            .contains("is not a conversation of its own")
    );
    assert!(
        w.fails(None, &["chat", "resume", &id], 1)
            .contains("not archived")
    );
    assert!(
        w.fails(None, &["chat", "promote", &id, "app"], 1)
            .contains("empty")
    );
    assert!(
        w.fails(None, &["chat", "rm", "app"], 1)
            .contains("is not a conversation of its own")
    );
    assert!(w.fails(None, &["chat"], 2).contains("usage: jw chat new"));
    assert!(
        w.fails(None, &["chat", "new", "x"], 2)
            .contains("usage: jw chat new")
    );
    // a chat with nothing said is discarded when it is archived, and its folder goes
    let state = w.env.dir.join("state");
    let folder = |id: &str| state.join("chats").join(id);
    assert!(folder(&other).is_dir());
    assert!(
        w.ok(None, &["chat", "archive", &other])
            .contains("discarded")
    );
    assert!(!folder(&other).exists());
    assert_eq!(w.ok(None, &["ls"]).lines().count(), 2);
    // one that Claude has a transcript of is kept: archived, with no panel
    let session = {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let saved: Value = std::fs::read_to_string(state.join("session.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let found = saved["workspaces"].as_array().and_then(|all| {
                all.iter()
                    .find(|x| x["id"] == id.as_str())
                    .and_then(|x| x["panes"][0]["resume"][2].as_str().map(str::to_owned))
            });
            if let Some(s) = found {
                break s;
            }
            assert!(Instant::now() < end, "the panel has no session id");
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    let projects = w.env.dir.join("claude-config/projects/x");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join(format!("{session}.jsonl")), "{}\n").unwrap();
    assert_eq!(w.ok(None, &["chat", "archive", &id]), "");
    let rows = w.ok(None, &["ls"]);
    assert!(
        rows.contains("conversación nueva\tfolder\tchat archivada\t0 panes\t-\t-\n"),
        "{rows}"
    );
    assert_eq!(
        w.json(&["ls", "--json"])[1]["chat"]["archived"],
        serde_json::json!(true)
    );
    // an archived chat has no panel to read or to prompt
    for args in [&["read", &id][..], &["prompt", &id, "hola"]] {
        let said = w.fails(None, args, 1);
        assert!(
            said.contains(&format!("archivada: jw chat resume {id}")),
            "{said}"
        );
    }
    // resuming opens a panel again, and prints its id
    let pane = w.ok(None, &["chat", "resume", &id]);
    assert_eq!(pane.trim().len(), 36, "{pane}");
    assert!(
        w.ok(None, &["ls"])
            .contains("conversación nueva\tfolder\tchat\t1 panes\t-\t-\n")
    );
    // removing takes the folder and the chat
    assert_eq!(w.ok(None, &["chat", "rm", &id]), "");
    assert!(!folder(&id).exists());
    assert_eq!(w.ok(None, &["ls"]), "app\tfolder\t-\t1 panes\t-\t-\n");
    w.stop();
}

// ---- N171b: `jw artifact` (REQ-771 to REQ-773)

impl World {
    fn spawn(&self, pane: &str, args: &[&str]) -> std::process::Child {
        let mut c = self.env.jw();
        c.args(args)
            .env(identity::env_var("PANE_ID"), pane)
            .current_dir(&self.repo)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        c.spawn().unwrap()
    }

    fn hash_of(&self, id: &str) -> String {
        self.env
            .cmd(&format!(r#"{{"cmd":"artifact_read","id":"{id}"}}"#))["result"]["hash"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn decide(&self, id: &str, n: u32, verdict: &str, text: Option<&str>) {
        let hash = self.hash_of(id);
        let text = text.map_or(String::new(), |t| format!(r#","text":"{t}""#));
        let reply = self.env.cmd(&format!(
            r#"{{"cmd":"artifact_decide","id":"{id}","n":{n},"hash":"{hash}","type":"{verdict}"{text}}}"#
        ));
        assert_eq!(reply["ok"], true, "{reply}");
    }

    fn write(&self, name: &str, text: &str) -> String {
        let p = self.repo.join(name);
        std::fs::write(&p, text).unwrap();
        name.to_owned()
    }
}

fn published(out: &str) -> (String, String) {
    let words: Vec<&str> = out.split_whitespace().collect();
    assert_eq!(words.len(), 3, "{out}");
    (words[0].to_owned(), words[1].to_owned())
}

#[test]
fn artifact_publishes_lists_and_removes_and_bad_uses_exit_2() {
    let w = World::new("ag-art");
    let (_, pane) = w.open_repo();
    let p = Some(pane.as_str());
    let page = w.write(
        "Ajustes.html",
        "<title>\n Ajustes de  prueba </title><p>hola",
    );
    let out = w.ok(p, &["artifact", "publish", &page]);
    let (id, v) = published(&out);
    assert_eq!(v, "v1");
    // The same bytes are the same version; other bytes the next.
    assert_eq!(w.ok(p, &["artifact", "publish", &page]), out);
    let spec = w.write("n171.md", "# La spec\n\ntexto");
    w.ok(
        p,
        &[
            "artifact", "publish", &spec, "--name", "plan", "--kind", "spec",
        ],
    );
    assert_eq!(
        w.ok(p, &["artifact", "ls"]),
        format!("app\t{id}\tajustes\tdocument\tv1\tpending\tAjustes de prueba\n")
            + &format!(
                "app\t{}\tplan\tspec\tv1\tpending\tLa spec\n",
                w.json(&["artifact", "ls", "--json"])[1]["id"]
                    .as_str()
                    .unwrap()
            )
    );
    let rows = w.json(&["artifact", "ls", "app", "--json"]);
    assert_eq!(rows[0]["title"], "Ajustes de prueba");
    assert_eq!(
        (rows[0]["status"].as_str(), rows[0]["format"].as_str()),
        (Some("pending"), Some("html"))
    );
    assert!(
        w.fails(None, &["artifact", "ls", "nowhere"], 1)
            .contains("nowhere")
    );
    // What it is not given right.
    let text = w.write("a.txt", "x");
    assert!(
        w.fails(p, &["artifact", "publish", &text], 2)
            .contains(".html")
    );
    assert!(
        w.fails(p, &["artifact", "publish", &page, "--kind", "weird"], 2)
            .contains("--kind")
    );
    assert!(
        w.fails(p, &["artifact", "publish", &page, "--timeout", "5"], 2)
            .contains("--wait")
    );
    w.fails(p, &["artifact"], 2);
    w.fails(p, &["artifact", "nonsense"], 2);
    assert!(
        w.fails(None, &["artifact", "publish", &page], 1)
            .contains("pane")
    );
    let big = w.write("big.html", &"x".repeat(512 * 1024 + 1));
    assert!(
        w.fails(p, &["artifact", "publish", &big], 1)
            .contains("524288")
    );
    // Removed, then gone.
    w.ok(p, &["artifact", "rm", &id]);
    assert!(!w.ok(p, &["artifact", "ls"]).contains(&id));
    w.fails(p, &["artifact", "rm", &id], 1);
    w.stop();
}

#[test]
fn artifact_wait_returns_the_verdict_exit_0_for_approved_and_4_with_the_text_for_changes() {
    let w = World::new("ag-artw");
    let (_, pane) = w.open_repo();
    let p = Some(pane.as_str());
    let page = w.write("a.html", "<p>uno");
    let (id, _) = published(&w.ok(p, &["artifact", "publish", &page]));
    // An approval.
    let waiting = w.spawn(&pane, &["artifact", "wait", &id]);
    w.decide(&id, 1, "approved", None);
    let out = waiting.wait_with_output().unwrap();
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8(out.stdout).unwrap().as_str()
        ),
        (Some(0), "approved\n")
    );
    // A new version asks for changes: exit 4, and what to change.
    std::fs::write(w.repo.join("a.html"), "<p>dos").unwrap();
    w.ok(p, &["artifact", "publish", &page]);
    let waiting = w.spawn(&pane, &["artifact", "wait", &id]);
    w.decide(&id, 2, "changes", Some("más grande"));
    let out = waiting.wait_with_output().unwrap();
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8(out.stdout).unwrap().as_str()
        ),
        (Some(4), "changes\nmás grande\n")
    );
    // A verdict that is already there is answered at once; a version with none times out.
    let again = w.run(p, &["artifact", "wait", &id]);
    assert_eq!(again.status.code(), Some(4));
    std::fs::write(w.repo.join("a.html"), "<p>tres").unwrap();
    w.ok(p, &["artifact", "publish", &page]);
    let late = w.run(p, &["artifact", "wait", &id, "--timeout", "1"]);
    assert_eq!(late.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&late.stderr).contains("no verdict in 1 s"));
    w.fails(p, &["artifact", "wait", "nope", "--timeout", "1"], 1);
    w.stop();
}

#[test]
fn artifact_publish_wait_prints_what_it_made_and_then_the_verdict() {
    let w = World::new("ag-artpw");
    let (_, pane) = w.open_repo();
    let page = w.write("a.html", "<p>uno");
    let waiting = w.spawn(&pane, &["artifact", "publish", &page, "--wait"]);
    let mut id = String::new();
    wait_for("the artifact to be published", || {
        let rows = w.json(&["artifact", "ls", "--json"]);
        if let Some(r) = rows.as_array().and_then(|r| r.first()) {
            id = r["id"].as_str().unwrap().to_owned();
        }
        !id.is_empty()
    });
    w.decide(&id, 1, "approved", None);
    let out = waiting.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    let (line, verdict) = stdout.split_once('\n').unwrap();
    assert_eq!(published(line).0, id);
    assert_eq!(verdict, "approved\n");
    w.stop();
}
