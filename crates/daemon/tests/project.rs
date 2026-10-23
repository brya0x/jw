//! N78: a repository's `.jw.toml` (REQ-329 to REQ-346), against a real daemon and real git.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, ErrorCode};
use serde_json::Value;
use wire::Client;

/// A repository with one commit, at `<sandbox>/work/app`, and `jw` as its `.jw.toml` when given.
fn repo(sb: &Sandbox, jw: Option<&str>) -> PathBuf {
    let dir = sb.dir.join("work/app");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    if let Some(jw) = jw {
        std::fs::write(dir.join(".jw.toml"), jw).unwrap();
    }
    std::fs::canonicalize(dir).unwrap()
}

fn ok(c: &mut Client, cmd: Cmd) -> Value {
    c.cmd(cmd).unwrap().unwrap().unwrap()
}

fn refused(c: &mut Client, cmd: Cmd) -> (ErrorCode, String) {
    let e = c.cmd(cmd).unwrap().unwrap_err();
    (e.code, e.message)
}

fn worktree(c: &mut Client, repo: &Path, name: &str) -> Result<Value, (ErrorCode, String)> {
    c.cmd(Cmd::CreateWorktree {
        repo: text(repo),
        branch: format!("feat/{name}"),
        base: None,
        folder: Some(name.to_owned()),
    })
    .unwrap()
    .map(|r| r.unwrap())
    .map_err(|e| (e.code, e.message))
}

fn open(c: &mut Client, node: &str) {
    c.open(node).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(node)).unwrap());
}

fn screen(c: &Client, pane: &str) -> String {
    c.nodes.get(pane).map_or_else(String::new, |n| {
        n["screen"]["lines"]
            .as_array()
            .map_or_else(String::new, |l| {
                l.iter()
                    .map(|l| l.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    })
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("timed out waiting for {what}");
}

const FULL: &str = r#"
root = "../app-trees"
# The template of branches that N78 read and N96 ignores.
branch = "bryan/{name}"
setup = ["echo installing {name}", "echo done-{branch}"]
ports = { web = 1 }
open = "claude"

# What N78 read and N95 ignores, even when it is no longer right.
[agent]
default = "claude"

[layout]
split = "right"
ratio = 0.6
a = { run = "cat" }
b = { split = "down", a = { run = "shell" }, b = { run = "editor" } }
editor = "cat -n"
"#;

/// A daemon whose panes find a `claude` that says it ran and stays: what the one panel starts
/// when the settings say `claude` (the conversation's swap is off in these tests, so the
/// program is the original).
fn start_with_claude(sb: &Sandbox) -> daemon::Daemon {
    use std::os::unix::fs::PermissionsExt;
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\necho \"claude-ran $*\"\nsleep 30\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
    config.env = vec![(
        "PATH".into(),
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    )];
    daemon::Daemon::start(config).expect("daemon starts")
}

/// The screen of the one panel of `ws`, once `text` is on it.
fn panel_shows(c: &mut Client, ws: &str, pane: &str, text: &str) -> String {
    open(c, ws);
    open(c, pane);
    wait_for(&format!("{text:?} on the screen"), || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(c, pane).contains(text)
    });
    screen(c, pane)
}

#[test]
fn project_config_tells_the_defaults_and_what_the_file_says() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, None);
    let mut c = sb.client();

    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert!(r.get("source").is_none());
    assert_eq!(r["root"], text(&top.parent().unwrap().join("app-wt")));
    assert!(
        r.get("branch").is_none(),
        "REQ-512: no template of branches"
    );
    assert_eq!(r["setup"], serde_json::json!([]));
    assert_eq!(r["open"], "shell", "REQ-509");
    assert!(r.get("panes").is_none());

    std::fs::write(top.join(".jw.toml"), FULL).unwrap();
    // Any path inside the repository will do, and the file is read afresh.
    let inside = top.join("sub");
    std::fs::create_dir_all(&inside).unwrap();
    let r = ok(
        &mut c,
        Cmd::ProjectConfig {
            repo: text(&inside),
        },
    );
    assert_eq!(r["source"], text(&top.join(".jw.toml")));
    assert_eq!(r["root"], text(&top.parent().unwrap().join("app-trees")));
    assert_eq!(r["setup"].as_array().unwrap().len(), 2);
    assert_eq!(r["open"], "claude");
    d.stop();
}

#[test]
fn a_file_that_cannot_be_used_is_refused_by_the_three_commands_and_nothing_is_made() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("open = \"shell\"\nsetup = [\"cd {nme}\"]\n"));
    let mut c = sb.client();

    let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(
        why.starts_with(".jw.toml, line 2: «{nme}» is no marker"),
        "{why}"
    );

    let (code, why) = worktree(&mut c, &top, "one").unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains(".jw.toml"), "{why}");
    assert!(!top.parent().unwrap().join("app-wt/one").exists());
    assert!(!git_has_worktree(&top, "one"));

    // A workspace that was made while the file was good, and whose file went bad.
    std::fs::write(top.join(".jw.toml"), "").unwrap();
    let ws = worktree(&mut c, &top, "two").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(top.join(".jw.toml"), "setup = [").unwrap();
    let (code, why) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains(".jw.toml"), "{why}");
    open(&mut c, &ws);
    assert!(c.nodes[&ws]["panes"].as_array().unwrap().is_empty());
    d.stop();
}

fn git_has_worktree(top: &Path, name: &str) -> bool {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(name)
}

#[test]
fn a_worktree_goes_in_the_root_of_the_file_and_without_one_in_the_default() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("root = \"../app-trees\"\n"));
    let mut c = sb.client();

    let r = worktree(&mut c, &top, "one").unwrap();
    assert_eq!(
        r["path"],
        text(&top.parent().unwrap().join("app-trees/one"))
    );
    assert!(top.parent().unwrap().join("app-trees/one/f.txt").exists());

    std::fs::remove_file(top.join(".jw.toml")).unwrap();
    let r = worktree(&mut c, &top, "two").unwrap();
    assert_eq!(r["path"], text(&top.parent().unwrap().join("app-wt/two")));

    std::fs::write(
        top.join(".jw.toml"),
        "root = \"~/jwn-trees-that-do-not-exist\"\n",
    )
    .unwrap();
    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert!(
        r["root"]
            .as_str()
            .unwrap()
            .ends_with("/jwn-trees-that-do-not-exist")
    );
    assert!(!r["root"].as_str().unwrap().contains('~'));
    d.stop();
}

#[test]
fn start_workspace_opens_one_panel_a_shell_or_claude_as_the_settings_or_the_command_say() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let top = repo(&sb, Some("open = \"claude\"\n"));
    let mut c = sb.client();
    let made = |c: &mut Client, name: &str| {
        worktree(c, &top, name).unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let start = |c: &mut Client, ws: &str, open: Option<contract::OpenKind>| {
        let r = ok(
            c,
            Cmd::StartWorkspace {
                workspace: ws.to_owned(),
                open,
            },
        );
        let ids = r["panes"].as_array().unwrap();
        assert_eq!(ids.len(), 1, "REQ-506: one panel");
        ids[0].as_str().unwrap().to_owned()
    };

    // the settings say claude: one panel, and `claude` runs in it with its own conversation id
    let ws = made(&mut c, "ws-1");
    let pane = start(&mut c, &ws, None);
    let s = panel_shows(&mut c, &ws, &pane, "claude-ran");
    assert!(s.contains("--session-id"), "{s}");
    assert_eq!(c.nodes[&ws]["panes"].as_array().unwrap().len(), 1);

    // the command's choice wins over the settings', both ways
    let ws = made(&mut c, "ws-2");
    let pane = start(&mut c, &ws, Some(contract::OpenKind::Shell));
    open(&mut c, &ws);
    open(&mut c, &pane);
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo plain-shell\r".into(),
    })
    .unwrap()
    .unwrap();
    let s = panel_shows(&mut c, &ws, &pane, "plain-shell");
    assert!(!s.contains("claude-ran"), "{s}");
    std::fs::write(top.join(".jw.toml"), "open = \"shell\"\n").unwrap();
    let ws = made(&mut c, "ws-3");
    let pane = start(&mut c, &ws, Some(contract::OpenKind::Claude));
    panel_shows(&mut c, &ws, &pane, "claude-ran");
    d.stop();
}

#[test]
fn the_setup_runs_in_the_one_panel_and_then_the_shell_stays_or_claude_starts() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let top = repo(&sb, Some(FULL));
    let mut c = sb.client();
    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    let ids = r["panes"].as_array().unwrap();
    assert_eq!(
        ids.len(),
        1,
        "the setup does not make a panel of its own (REQ-507)"
    );
    let pane = ids[0].as_str().unwrap();
    // each line after `❯ ` and then run with its markers quoted, and then claude
    let s = panel_shows(&mut c, &ws, pane, "claude-ran");
    assert!(
        s.contains("❯ echo installing {name}") && s.contains("installing ws-1"),
        "{s}"
    );
    assert!(
        s.contains("❯ echo done-{branch}") && s.contains("done-feat/ws-1"),
        "{s}"
    );
    let (setup_at, claude_at) = (
        s.find("installing ws-1").unwrap(),
        s.find("claude-ran").unwrap(),
    );
    assert!(setup_at < claude_at, "the setup first, then claude: {s}");
    // the end of the setup is reported as a shell would, with its exit status 0
    let root = c.root.clone();
    open(&mut c, &root);
    assert!(
        c.wait_until(WAIT, |c| {
            c.nodes[&root]["attention"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["kind"] == "shell_done" && a["node"] == pane && a["exit_code"] == 0)
        })
        .unwrap(),
        "REQ-508: {}",
        c.nodes[&root]["attention"]
    );
    d.stop();
}

/// REQ-536: a worktree that was there before jwn is started with its panel and no setup, while the
/// one jwn makes runs it.
#[test]
fn a_worktree_that_existed_is_started_without_its_setup_and_one_that_was_made_runs_it() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("setup = [\"touch ../setup-ran-{name}\"]\n"));
    let existing = top.parent().unwrap().join("old-wt/had");
    git(
        &top,
        &["worktree", "add", "-q", "-b", "had", &text(&existing)],
    );
    let mut c = sb.client();
    let start = |c: &mut Client, ws: &str| {
        ok(
            c,
            Cmd::StartWorkspace {
                workspace: ws.to_owned(),
                open: None,
            },
        )
    };
    let adopted = ok(
        &mut c,
        Cmd::OpenWorktree {
            repo: text(&top),
            path: text(&existing),
        },
    )["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    start(&mut c, &adopted);

    let made = worktree(&mut c, &top, "made").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    start(&mut c, &made);
    let ran = top.parent().unwrap().join("app-wt/setup-ran-made");
    wait_for("the setup of the worktree jwn made", || ran.exists());
    assert!(
        !top.parent().unwrap().join("old-wt/setup-ran-had").exists(),
        "the setup does not run in a worktree that existed"
    );
    d.stop();
}

#[test]
fn a_setup_that_fails_leaves_the_shell_with_the_error_and_claude_does_not_start() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let top = repo(
        &sb,
        Some("open = \"claude\"\nsetup = [\"echo first\", \"false\", \"echo never\"]\n"),
    );
    let mut c = sb.client();
    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    let pane = r["panes"][0].as_str().unwrap().to_owned();
    panel_shows(&mut c, &ws, &pane, "❯ false");
    // Its end is reported as a shell would, with the exit status of the line that failed.
    let root = c.root.clone();
    open(&mut c, &root);
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&root]["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| {
                a["kind"] == "shell_done" && a["node"] == pane.as_str() && a["exit_code"] == 1
            }))
        .unwrap(),
        "{}",
        c.nodes[&root]["attention"]
    );
    let s = screen(&c, &pane);
    assert!(s.contains("❯ echo first") && s.contains("first"), "{s}");
    assert!(!s.contains("never"), "{s}");
    // Still a shell: it takes input, and claude never ran.
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo still-here\r".into(),
    })
    .unwrap()
    .unwrap();
    wait_for("the shell", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, &pane).contains("still-here")
    });
    assert!(!screen(&c, &pane).contains("claude-ran"), "REQ-507");
    d.stop();
}

#[test]
fn the_layout_the_agent_and_the_editor_of_an_old_file_change_nothing() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(
        &sb,
        Some(
            "setup = [\"true\"]\n[agent]\ndefault = \"codex\"\n[layout]\neditor = \"nvim\"\nrun = \"agent\"\n",
        ),
    );
    let mut c = sb.client();
    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(
        r["open"], "shell",
        "REQ-505: the design-studio file of N78 opens a shell now"
    );
    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws,
            open: None,
        },
    );
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    // and an `open` that is neither is refused naming the line
    std::fs::write(top.join(".jw.toml"), "setup = []\nopen = \"nvim\"\n").unwrap();
    let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(
        why.starts_with(".jw.toml, line 2:") && why.contains("\"shell\" or \"claude\""),
        "{why}"
    );
    d.stop();
}

#[test]
fn without_a_file_start_workspace_opens_one_shell_and_a_workspace_with_panes_conflicts() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let top = repo(&sb, None);
    let mut c = sb.client();

    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    assert_eq!(code, ErrorCode::Conflict);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: "nope".into(),
            open: None,
        },
    );
    assert_eq!(code, ErrorCode::NotFound);
    open(&mut c, &ws);
    assert_eq!(c.nodes[&ws]["panes"].as_array().unwrap().len(), 1);

    // The top level of the repository, opened as a folder, has one panel and not the setup
    // (REQ-500): FULL's setup lines are for worktrees.
    std::fs::write(top.join(".jw.toml"), FULL).unwrap();
    let folder = ok(&mut c, Cmd::OpenFolder { path: text(&top) })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: folder,
            open: None,
        },
    );
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    d.stop();
}

fn config(c: &mut Client, repo: &Path) -> Value {
    ok(c, Cmd::ProjectConfig { repo: text(repo) })
}

fn write(
    c: &mut Client,
    repo: &Path,
    to: contract::ConfigFrom,
    body: &str,
) -> Result<Value, (ErrorCode, String)> {
    c.cmd(Cmd::WriteProjectConfig {
        repo: text(repo),
        to,
        text: body.to_owned(),
    })
    .unwrap()
    .map(|r| r.unwrap())
    .map_err(|e| (e.code, e.message))
}

/// N91: where the settings are (REQ-482 to REQ-487).
#[test]
fn the_settings_come_from_the_repository_then_from_the_personal_file_then_from_the_defaults() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();

    // neither file: the defaults, and where the personal one would be
    let none = config(&mut c, &app);
    assert!(none.get("source").is_none() && none.get("from").is_none());
    let personal = PathBuf::from(none["personal"].as_str().unwrap());
    assert!(
        personal.starts_with(sb.dir.join("projects")),
        "{personal:?}"
    );
    assert!(
        personal
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("local_"),
        "a repository with no origin is named by its path: {personal:?}"
    );
    assert!(!personal.exists(), "asking writes nothing");

    // the personal file, read as a `.jw.toml` is
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, "copy = [\"mio\"]\nsetup = [\"true\"]\n").unwrap();
    let mine = config(&mut c, &app);
    assert_eq!(mine["copy"], serde_json::json!(["mio"]));
    assert_eq!(
        (mine["from"].as_str(), mine["source"].as_str()),
        (Some("personal"), personal.to_str())
    );
    assert_eq!(mine["setup"], serde_json::json!(["true"]));

    // the repository's own file wins, and the personal one is not read
    std::fs::write(app.join(".jw.toml"), "copy = [\"equipo\"]\n").unwrap();
    let theirs = config(&mut c, &app);
    assert_eq!(theirs["copy"], serde_json::json!(["equipo"]));
    assert_eq!(theirs["from"], "repo");
    assert_eq!(theirs["setup"], serde_json::json!([]));
    assert_eq!(
        theirs["personal"], mine["personal"],
        "it still says where the personal one is"
    );

    // a worktree of the repository has the same personal file
    std::fs::remove_file(app.join(".jw.toml")).unwrap();
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let wt = PathBuf::from(made["path"].as_str().unwrap());
    let from_worktree = config(&mut c, &wt);
    assert_eq!(from_worktree["personal"], mine["personal"], "REQ-483");
    assert_eq!(from_worktree["from"], "personal");

    // an error in the personal file names it and its line
    std::fs::write(&personal, "open = \"shell\"\nsetup = nope\n").unwrap();
    let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&app) });
    assert_eq!(code, ErrorCode::BadRequest);
    let name = personal.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        why.starts_with(&format!("{name}, line 2")),
        "REQ-484: {why}"
    );
    d.stop();
}

#[test]
fn a_worktree_starts_with_the_personal_settings_and_a_project_that_needs_nothing_needs_no_file() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();
    // no file anywhere: one shell and no setup pane, and nothing is written
    let plain = worktree(&mut c, &app, "ws-plain").unwrap();
    let ws = plain["workspace"].as_str().unwrap().to_owned();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws,
            open: None,
        },
    );
    assert_eq!(started["panes"].as_array().unwrap().len(), 1);
    assert!(!app.join(".jw.toml").exists());
    assert!(
        !sb.dir.join("projects").exists(),
        "REQ-488: nothing is asked or made"
    );
    // a personal file with a setup gives the next worktree its setup pane
    let personal = PathBuf::from(config(&mut c, &app)["personal"].as_str().unwrap());
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, "setup = [\"true\"]\n").unwrap();
    let with = worktree(&mut c, &app, "ws-setup").unwrap();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: with["workspace"].as_str().unwrap().to_owned(),
            open: None,
        },
    );
    assert_eq!(
        started["panes"].as_array().unwrap().len(),
        1,
        "the setup runs in the one panel (REQ-507)"
    );
    d.stop();
}

#[test]
fn writing_the_settings_checks_them_first_keeps_the_personal_file_private_and_never_touches_the_repository_unasked()
 {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();
    let personal = PathBuf::from(config(&mut c, &app)["personal"].as_str().unwrap());

    // REQ-486: it is written whole, to the personal file, readable by its owner only
    let wrote = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "copy = [\"x\"]\n",
    )
    .unwrap();
    assert_eq!(wrote["path"], text(&personal));
    assert_eq!(wrote["shadowed"], false);
    assert_eq!(
        std::fs::read_to_string(&personal).unwrap(),
        "copy = [\"x\"]\n"
    );
    assert_eq!(
        std::fs::metadata(&personal).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !app.join(".jw.toml").exists(),
        "REQ-487: the repository is not touched"
    );
    assert_eq!(
        config(&mut c, &app)["copy"],
        serde_json::json!(["x"]),
        "and the next read has it"
    );
    // no temporary file is left behind
    let left: Vec<_> = std::fs::read_dir(personal.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");

    // a text that does not read is refused, says where, and changes nothing
    let (code, why) = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "setup = [\"cd {nombre}\"]\n",
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains("line 1"), "{why}");
    assert_eq!(
        std::fs::read_to_string(&personal).unwrap(),
        "copy = [\"x\"]\n"
    );

    // only when asked does it write into the repository, and then the personal one is hidden by it
    let repo_file = write(
        &mut c,
        &app,
        contract::ConfigFrom::Repo,
        "copy = [\"equipo\"]\n",
    )
    .unwrap();
    assert_eq!(repo_file["path"], text(&app.join(".jw.toml")));
    assert_eq!(
        std::fs::read_to_string(app.join(".jw.toml")).unwrap(),
        "copy = [\"equipo\"]\n"
    );
    let shadowed = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "copy = [\"y\"]\n",
    )
    .unwrap();
    assert_eq!(shadowed["shadowed"], true, "REQ-487");
    assert_eq!(config(&mut c, &app)["copy"], serde_json::json!(["equipo"]));
    // a folder that is not a repository has nothing to write for
    let plain = sb.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(
        write(&mut c, &plain, contract::ConfigFrom::Personal, "")
            .unwrap_err()
            .0,
        ErrorCode::GitFailed
    );
    d.stop();
}

#[test]
fn writing_into_the_repository_never_goes_through_a_link_planted_where_the_temporary_file_goes() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let victim = sb.dir.join("victim.txt");
    std::fs::write(&victim, "intacto").unwrap();
    std::os::unix::fs::symlink(&victim, app.join(".jw.toml.tmp")).unwrap();
    let mut c = sb.client();
    write(&mut c, &app, contract::ConfigFrom::Repo, "copy = [\"a\"]\n").unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "intacto");
    assert_eq!(
        std::fs::read_to_string(app.join(".jw.toml")).unwrap(),
        "copy = [\"a\"]\n"
    );
    assert!(!app.join(".jw.toml.tmp").exists());
    // a text that is too big is refused whole
    let big = "# ".repeat(40 * 1024);
    let (code, why) = write(&mut c, &app, contract::ConfigFrom::Personal, &big).unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains("KiB"), "{why}");
    // the personal folder is made for its owner only
    use std::os::unix::fs::PermissionsExt;
    write(&mut c, &app, contract::ConfigFrom::Personal, "").unwrap();
    let mode = std::fs::metadata(sb.dir.join("projects"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
    d.stop();
}

/// N92: `copy` (REQ-489 to REQ-492).
#[test]
fn copy_puts_the_files_of_the_checkout_in_a_new_worktree_with_their_owners_bits_and_says_what_it_skipped()
 {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(
        &sb,
        Some("copy = [\".env\", \"apps/.env.local\", \"nada\", \"f.txt\"]\n"),
    );
    std::fs::write(app.join(".env"), "SECRETO=1\n").unwrap();
    std::fs::set_permissions(app.join(".env"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::create_dir_all(app.join("apps")).unwrap();
    std::fs::write(app.join("apps/.env.local"), "LOCAL=2\n").unwrap();
    let mut c = sb.client();

    assert_eq!(
        config(&mut c, &app)["copy"],
        serde_json::json!([".env", "apps/.env.local", "nada", "f.txt"]),
        "REQ-489: the settings list them"
    );
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let wt = PathBuf::from(made["path"].as_str().unwrap());
    assert_eq!(
        made["copied"],
        serde_json::json!([".env", "apps/.env.local"])
    );
    assert_eq!(
        made["skipped"],
        serde_json::json!([
            { "path": "nada", "why": "it is not in the checkout" },
            { "path": "f.txt", "why": "it is already in the worktree" },
        ])
    );
    assert_eq!(
        std::fs::read_to_string(wt.join(".env")).unwrap(),
        "SECRETO=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("apps/.env.local")).unwrap(),
        "LOCAL=2\n"
    );
    let mode = std::fs::metadata(wt.join(".env"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "REQ-490");
    assert_eq!(
        std::fs::read_to_string(app.join(".env")).unwrap(),
        "SECRETO=1\n",
        "the checkout is as it was"
    );
    // a worktree of a repository with nothing to copy says nothing of it
    std::fs::write(app.join(".jw.toml"), "").unwrap();
    let plain = worktree(&mut c, &app, "ws-2").unwrap();
    assert!(
        plain.get("copied").is_none() && plain.get("skipped").is_none(),
        "{plain}"
    );
    d.stop();
}

#[test]
fn a_copy_that_leaves_the_repository_is_refused_naming_the_path_and_the_line() {
    let sb = Sandbox::new();
    let d = sb.start();
    for bad in ["../fuera", "/etc/passwd", ""] {
        let app = repo(
            &sb,
            Some(&format!("branch = \"a/{{name}}\"\ncopy = [{bad:?}]\n")),
        );
        let mut c = sb.client();
        let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&app) });
        assert_eq!(code, ErrorCode::BadRequest);
        assert!(why.starts_with(".jw.toml, line 2"), "{bad:?}: {why}");
        assert!(why.contains("not a path inside the repository"), "{why}");
        std::fs::remove_dir_all(&app).unwrap();
    }
    d.stop();
}

/// N93: ports per worktree (REQ-494 to REQ-498).
#[test]
fn a_worktree_gets_a_block_of_ports_its_panes_see_them_and_a_removed_one_gives_its_block_back() {
    let sb = Sandbox::new();
    let d = sb.start();
    let base = identity::env_var("PORT_BASE");
    let web = identity::env_var("PORT_WEB");
    let app = repo(
        &sb,
        Some(&format!(
            "ports = {{ web = 0, api = 5 }}\nsetup = [\"echo seen-${base}-${web}-end\"]\n"
        )),
    );
    let mut c = sb.client();
    assert_eq!(
        config(&mut c, &app)["ports"],
        serde_json::json!({ "api": 5, "web": 0 }),
        "REQ-494"
    );

    let one = worktree(&mut c, &app, "ws-1").unwrap();
    assert_eq!(
        one["ports"],
        serde_json::json!({ "api": 20005, "web": 20000 }),
        "REQ-497"
    );
    let two = worktree(&mut c, &app, "ws-2").unwrap();
    assert_eq!(
        two["ports"],
        serde_json::json!({ "api": 20105, "web": 20100 }),
        "the next slot"
    );

    // the panes of the worktree start with its ports, the setup pane among them (REQ-496)
    let ws = two["workspace"].as_str().unwrap().to_owned();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws,
            open: None,
        },
    );
    let setup = started["panes"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    open(&mut c, &setup);
    wait_for("the setup to say its ports", || {
        let _ = c.poll(Duration::from_millis(20));
        screen(&c, &setup).contains("seen-20100-20100-end")
    });

    // a change of the settings is for the next worktree, not for this one (REQ-498)
    std::fs::write(app.join(".jw.toml"), "ports = { web = 9 }\n").unwrap();
    let three = worktree(&mut c, &app, "ws-3").unwrap();
    assert_eq!(three["ports"], serde_json::json!({ "web": 20209 }));

    // a removed worktree gives its block back, and the lowest free one is the next to be given (REQ-495)
    c.cmd(Cmd::RemoveWorkspace {
        workspace: one["workspace"].as_str().unwrap().to_owned(),
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();
    let four = worktree(&mut c, &app, "ws-4").unwrap();
    assert_eq!(
        four["ports"],
        serde_json::json!({ "web": 20009 }),
        "the first block again"
    );
    d.stop();
}

#[test]
fn a_project_without_ports_gives_none_and_the_blocks_survive_a_restart_of_the_daemon() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, Some("ports = { web = 0 }\n"));
    let mut c = sb.client();
    let plain = repo_two(&sb);
    let none = worktree(&mut c, &plain, "ws-n").unwrap();
    assert!(none.get("ports").is_none(), "{none}");
    let one = worktree(&mut c, &app, "ws-1").unwrap();
    assert_eq!(one["ports"], serde_json::json!({ "web": 20000 }));
    d.stop();
    let d = sb.start();
    let mut c = sb.client();
    let two = worktree(&mut c, &app, "ws-2").unwrap();
    assert_eq!(
        two["ports"],
        serde_json::json!({ "web": 20100 }),
        "the first block is still held"
    );
    d.stop();
}

/// A second repository, at `<sandbox>/work/other`, with no settings.
fn repo_two(sb: &Sandbox) -> PathBuf {
    let dir = sb.dir.join("work/other");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap()
}

/// The personal file of a folder that is not a repository: named by its path (REQ-483).
fn folder_personal(sb: &Sandbox, folder: &Path) -> PathBuf {
    daemon::personal_file(&sb.dir.join("projects"), None, folder)
}

fn open_folder(c: &mut Client, folder: &Path) -> String {
    ok(c, Cmd::OpenFolder { path: text(folder) })["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

const TWO: &str = "open = \"claude\"\nsetup = [\"echo setup-ran\"]\nports = { web = 0 }\n";

/// N94 and N95: a folder's panel (REQ-500 to REQ-502, REQ-506).
#[test]
fn a_folder_opened_gets_the_panel_its_personal_file_says_and_never_its_setup_or_ports() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let folder = sb.dir.join("work/plain");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = std::fs::canonicalize(folder).unwrap();
    let mut c = sb.client();
    let start = |c: &mut Client, ws: &str| {
        ok(
            c,
            Cmd::StartWorkspace {
                workspace: ws.to_owned(),
                open: None,
            },
        )["panes"]
            .clone()
    };
    let close = |c: &mut Client, ws: String| {
        c.cmd(Cmd::RemoveWorkspace {
            workspace: ws,
            remove_worktree: false,
            force: false,
        })
        .unwrap()
        .unwrap();
    };

    // no settings: one shell, as always
    let ws = open_folder(&mut c, &folder);
    let pane = start(&mut c, &ws)[0].as_str().unwrap().to_owned();
    open(&mut c, &pane);
    assert!(!screen(&c, &pane).contains("claude-ran"));
    close(&mut c, ws);

    // the personal file named by its path: one panel with claude, and its setup is not run
    let personal = folder_personal(&sb, &folder);
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, TWO).unwrap();
    let ws = open_folder(&mut c, &folder);
    let started = start(&mut c, &ws);
    assert_eq!(
        started.as_array().unwrap().len(),
        1,
        "REQ-500: one panel and no setup"
    );
    let s = panel_shows(&mut c, &ws, started[0].as_str().unwrap(), "claude-ran");
    assert!(!s.contains("setup-ran"), "a folder has no setup: {s}");
    close(&mut c, ws);

    // its own `.jw.toml` is read before the personal file
    std::fs::write(folder.join(".jw.toml"), "open = \"shell\"\n").unwrap();
    let ws = open_folder(&mut c, &folder);
    let pane = start(&mut c, &ws)[0].as_str().unwrap().to_owned();
    open(&mut c, &ws);
    open(&mut c, &pane);
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo shell-here\r".into(),
    })
    .unwrap()
    .unwrap();
    let s = panel_shows(&mut c, &ws, &pane, "shell-here");
    assert!(!s.contains("claude-ran"), "{s}");
    close(&mut c, ws);

    // settings that cannot be used: refused naming the file and the line, and nothing started
    std::fs::write(folder.join(".jw.toml"), "open = \n").unwrap();
    let ws = open_folder(&mut c, &folder);
    let (code, why) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.starts_with(".jw.toml, line"), "REQ-502: {why}");
    open(&mut c, &ws);
    assert_eq!(c.nodes[&ws]["panes"].as_array().map_or(0, Vec::len), 0);
    d.stop();
}

#[test]
fn the_top_of_a_repository_has_its_settings_and_a_folder_inside_it_does_not() {
    let sb = Sandbox::new();
    let d = start_with_claude(&sb);
    let app = repo(&sb, Some(TWO));
    std::fs::create_dir_all(app.join("apps/web")).unwrap();
    let mut c = sb.client();
    // the top level: the repository's `claude`, but not its setup (REQ-501)
    let ws = open_folder(&mut c, &app);
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    let s = panel_shows(
        &mut c,
        &ws,
        started["panes"][0].as_str().unwrap(),
        "claude-ran",
    );
    assert!(!s.contains("setup-ran"), "{s}");
    // a folder inside it is not the repository: one shell, whatever the repository says
    let inner = open_folder(&mut c, &app.join("apps/web"));
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: inner.clone(),
            open: None,
        },
    );
    let pane = started["panes"][0].as_str().unwrap().to_owned();
    open(&mut c, &inner);
    open(&mut c, &pane);
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo inner-shell\r".into(),
    })
    .unwrap()
    .unwrap();
    let s = panel_shows(&mut c, &inner, &pane, "inner-shell");
    assert!(!s.contains("claude-ran"), "{s}");
    // while a worktree of the repository has the setup, and then claude, in its one panel
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let ws = made["workspace"].as_str().unwrap().to_owned();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    assert_eq!(started["panes"].as_array().unwrap().len(), 1);
    let s = panel_shows(
        &mut c,
        &ws,
        started["panes"][0].as_str().unwrap(),
        "claude-ran",
    );
    assert!(s.contains("setup-ran"), "{s}");
    d.stop();
}

fn edit(
    c: &mut Client,
    repo: &Path,
    to: contract::ConfigFrom,
    set: contract::ProjectEdit,
) -> Result<Value, (ErrorCode, String)> {
    c.cmd(Cmd::EditProjectConfig {
        repo: text(repo),
        to,
        set,
    })
    .unwrap()
    .map(|r| r.unwrap())
    .map_err(|e| (e.code, e.message))
}

/// N97: a change touches only the keys it changes (REQ-516 to REQ-519).
#[test]
fn an_edit_changes_only_what_it_says_and_keeps_the_comments_and_the_order_of_the_file() {
    use contract::{ConfigFrom, OpenKind, ProjectEdit};
    let sb = Sandbox::new();
    let d = sb.start();
    let old = "# the team's file\nroot = \"../app-trees\" # beside\n\n# what runs first\nsetup = [\"make deps\"]\n\n[agent]\ndefault = \"claude\"\n";
    let app = repo(&sb, Some(old));
    let mut c = sb.client();

    // `root_text` is what the file wrote, and there is none when the file does not say
    assert_eq!(config(&mut c, &app)["root_text"], "../app-trees");

    // nothing different, nothing written: the file is the same byte for byte
    let same = ProjectEdit {
        root: Some("../app-trees".into()),
        setup: Some(vec!["make deps".into()]),
        open: Some(OpenKind::Shell),
        ..Default::default()
    };
    let r = edit(&mut c, &app, ConfigFrom::Repo, same).unwrap();
    assert_eq!(r["path"], text(&app.join(".jw.toml")));
    assert_eq!(std::fs::read_to_string(app.join(".jw.toml")).unwrap(), old);

    // a change: the comments, the old `[agent]` and the order stay; a new key is a top-level one
    let set = ProjectEdit {
        setup: Some(vec!["make deps".into(), "make seed".into()]),
        copy: Some(vec![".env".into()]),
        ports: Some([("web".to_owned(), 0), ("api".to_owned(), 1)].into()),
        open: Some(OpenKind::Claude),
        ..Default::default()
    };
    edit(&mut c, &app, ConfigFrom::Repo, set).unwrap();
    let after = std::fs::read_to_string(app.join(".jw.toml")).unwrap();
    for kept in [
        "# the team's file",
        "# beside",
        "# what runs first",
        "[agent]",
        "default = \"claude\"",
    ] {
        assert!(after.contains(kept), "{kept} lost:\n{after}");
    }
    assert!(
        after.find("root").unwrap() < after.find("[agent]").unwrap(),
        "{after}"
    );
    let r = config(&mut c, &app);
    assert_eq!(r["setup"], serde_json::json!(["make deps", "make seed"]));
    assert_eq!(r["copy"], serde_json::json!([".env"]));
    assert_eq!(r["ports"], serde_json::json!({"web": 0, "api": 1}));
    assert_eq!(r["open"], "claude");
    assert_eq!(r["root_text"], "../app-trees", "REQ-519: untouched");

    // empty takes the key out, and a root changed is the new text
    let set = ProjectEdit {
        root: Some("../otra".into()),
        setup: Some(vec![]),
        ports: Some(Default::default()),
        ..Default::default()
    };
    edit(&mut c, &app, ConfigFrom::Repo, set).unwrap();
    let r = config(&mut c, &app);
    assert_eq!(r["root_text"], "../otra");
    assert_eq!(r["setup"], serde_json::json!([]));
    assert!(r.get("ports").is_none());
    let after = std::fs::read_to_string(app.join(".jw.toml")).unwrap();
    assert!(!after.contains("setup ="), "{after}");
    assert!(
        after.contains("# beside") && after.contains("[agent]"),
        "{after}"
    );
    d.stop();
}

#[test]
fn an_edit_of_the_personal_file_makes_it_private_and_says_when_the_repositorys_hides_it() {
    use contract::{ConfigFrom, OpenKind, ProjectEdit};
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();
    let personal = PathBuf::from(config(&mut c, &app)["personal"].as_str().unwrap());
    assert!(!personal.exists());

    // no file yet: it is made with the changed keys only (REQ-517), readable by its owner only
    let set = ProjectEdit {
        open: Some(OpenKind::Claude),
        copy: Some(vec![".env".into()]),
        ..Default::default()
    };
    let r = edit(&mut c, &app, ConfigFrom::Personal, set).unwrap();
    assert_eq!(r["path"], text(&personal));
    assert_eq!(r["shadowed"], false);
    assert_eq!(
        std::fs::read_to_string(&personal).unwrap(),
        "copy = [\".env\"]\nopen = \"claude\"\n"
    );
    assert_eq!(
        std::fs::metadata(&personal).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !app.join(".jw.toml").exists(),
        "the repository is not touched"
    );

    // a refused value names the file and the line, and nothing changes
    let before = std::fs::read_to_string(&personal).unwrap();
    for set in [
        ProjectEdit {
            ports: Some([("base".to_owned(), 1)].into()),
            ..Default::default()
        },
        ProjectEdit {
            ports: Some([("web".to_owned(), 100)].into()),
            ..Default::default()
        },
        ProjectEdit {
            copy: Some(vec!["../fuera".into()]),
            ..Default::default()
        },
        ProjectEdit {
            setup: Some(vec!["cd {nme}".into()]),
            ..Default::default()
        },
    ] {
        let (code, why) = edit(&mut c, &app, ConfigFrom::Personal, set).unwrap_err();
        assert_eq!(code, ErrorCode::BadRequest);
        assert!(why.contains(".toml"), "{why}");
    }
    assert_eq!(std::fs::read_to_string(&personal).unwrap(), before);

    // the repository's own file hides the personal one, and the reply says so
    std::fs::write(app.join(".jw.toml"), "").unwrap();
    let set = ProjectEdit {
        open: Some(OpenKind::Shell),
        ..Default::default()
    };
    assert_eq!(
        edit(&mut c, &app, ConfigFrom::Personal, set).unwrap()["shadowed"],
        true
    );
    // a folder that is no repository has nothing to edit
    let plain = sb.dir.join("plain2");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(
        edit(&mut c, &plain, ConfigFrom::Personal, ProjectEdit::default())
            .unwrap_err()
            .0,
        ErrorCode::GitFailed
    );
    d.stop();
}

/// What a pane says of its environment: `N=<name> P=<project> B=<port base> W=<web port>`.
fn type_probe(c: &mut Client, pane: &str) {
    let var = |n: &str| format!("${}", identity::env_var(n));
    let line = format!(
        "echo N={} P={} B={} W={}\r",
        var("NAME"),
        var("PROJECT"),
        var("PORT_BASE"),
        var("PORT_WEB")
    );
    c.cmd(Cmd::Write {
        pane: pane.to_owned(),
        data: line,
    })
    .unwrap()
    .unwrap();
}

fn pane_says(c: &mut Client, pane: &str, said: &str) {
    open(c, pane);
    type_probe(c, pane);
    wait_for(said, || {
        let _ = c.poll(Duration::from_millis(20));
        screen(c, pane).lines().any(|l| l == said)
    });
}

/// N177 (REQ-870 to REQ-872): the panel of the layout, a pane opened by `new_pane` (which is what
/// `pane open` sends) and a pane restored with the daemon all start with the same names and ports.
#[test]
fn every_pane_starts_with_its_name_project_and_ports_whoever_starts_it_and_a_restored_one_too() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, Some("ports = { web = 0 }\n"));
    let mut c = sb.client();
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let ws = made["workspace"].as_str().unwrap().to_owned();
    let said = "N=ws-1 P=app B=20000 W=20000";

    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
            open: None,
        },
    );
    let layout = started["panes"][0].as_str().unwrap().to_owned();
    pane_says(&mut c, &layout, said);
    let opened = ok(
        &mut c,
        Cmd::NewPane {
            workspace: ws.clone(),
            program: None,
            beside: None,
            dir: None,
        },
    )["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    pane_says(&mut c, &opened, said);

    drop(c);
    d.stop();
    let d = sb.start();
    let mut c = sb.client();
    open(&mut c, &ws);
    pane_says(&mut c, &opened, said);
    d.stop();
}

/// REQ-870: the name is the folder's, whatever the branch, and a repository's own checkout and a
/// folder that is no repository say their own names as the project.
#[test]
fn the_name_is_the_folder_and_the_project_is_the_repository_or_the_folder() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let plain = sb.dir.join("work/plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut c = sb.client();
    for (folder, said) in [
        (&app, "N=app P=app B= W="),
        (
            &std::fs::canonicalize(plain).unwrap(),
            "N=plain P=plain B= W=",
        ),
    ] {
        let ws = open_folder(&mut c, folder);
        let pane = ok(
            &mut c,
            Cmd::NewPane {
                workspace: ws,
                program: None,
                beside: None,
                dir: None,
            },
        )["pane"]
            .as_str()
            .unwrap()
            .to_owned();
        pane_says(&mut c, &pane, said);
    }
    d.stop();
}

/// REQ-873: opening a worktree git made copies what `copy` lists and the worktree lacks, never
/// over a file that is there, says what it skipped, and the restore of a session copies nothing.
#[test]
fn opening_an_existing_worktree_copies_what_it_lacks_never_over_a_file_and_a_restart_copies_nothing()
 {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(
        &sb,
        Some("copy = [\".env\", \"apps/.env.local\", \"nada\", \"f.txt\"]\n"),
    );
    std::fs::write(app.join(".env"), "SECRETO=1\n").unwrap();
    std::fs::create_dir_all(app.join("apps")).unwrap();
    std::fs::write(app.join("apps/.env.local"), "LOCAL=2\n").unwrap();
    let wt = sb.dir.join("work/old");
    git(
        &app,
        &["worktree", "add", "-q", "-b", "fix/old", &text(&wt)],
    );
    std::fs::create_dir_all(wt.join("apps")).unwrap();
    std::fs::write(wt.join("apps/.env.local"), "MINE\n").unwrap();
    let mut c = sb.client();

    let reply = ok(
        &mut c,
        Cmd::OpenWorktree {
            repo: text(&app),
            path: text(&wt),
        },
    );
    assert_eq!(reply["copied"], serde_json::json!([".env"]));
    assert_eq!(
        reply["skipped"],
        serde_json::json!([
            { "path": "apps/.env.local", "why": "it is already in the worktree" },
            { "path": "nada", "why": "it is not in the checkout" },
            { "path": "f.txt", "why": "it is already in the worktree" },
        ]),
        "a failure is said, and the open went on"
    );
    assert!(reply["workspace"].is_string());
    assert_eq!(
        std::fs::read_to_string(wt.join(".env")).unwrap(),
        "SECRETO=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("apps/.env.local")).unwrap(),
        "MINE\n",
        "never over a file"
    );

    std::fs::remove_file(wt.join(".env")).unwrap();
    drop(c);
    d.stop();
    let d = sb.start();
    assert!(!wt.join(".env").exists(), "a restart does not copy");
    d.stop();
}
