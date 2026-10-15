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

fn leaf_ids(layout: &Value) -> Vec<String> {
    match layout["t"].as_str() {
        Some("pane") => vec![layout["pane"].as_str().unwrap().to_owned()],
        _ => {
            let mut v = leaf_ids(&layout["a"]);
            v.extend(leaf_ids(&layout["b"]));
            v
        }
    }
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
branch = "bryan/{name}"
setup = ["echo installing {name}", "echo done-{branch}"]
ports = { web = 1 }

[agent]
default = "claude"

[layout]
split = "right"
ratio = 0.6
a = { run = "cat" }
b = { split = "down", a = { run = "shell" }, b = { run = "editor" } }
editor = "cat -n"
"#;

#[test]
fn project_config_tells_the_defaults_and_what_the_file_says() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, None);
    let mut c = sb.client();

    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert!(r.get("source").is_none());
    assert_eq!(r["root"], text(&top.parent().unwrap().join("app-wt")));
    assert_eq!(r["branch"], "feat/{name}");
    assert_eq!(r["setup"], serde_json::json!([]));
    assert_eq!(r["panes"], serde_json::json!(["sh"]));

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
    assert_eq!(r["branch"], "bryan/{name}");
    assert_eq!(r["setup"].as_array().unwrap().len(), 2);
    assert_eq!(r["panes"], serde_json::json!(["cat", "sh", "cat"]));
    d.stop();
}

#[test]
fn a_file_that_cannot_be_used_is_refused_by_the_three_commands_and_nothing_is_made() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("branch = \"x\"\nsetup = [\"cd {nme}\"]\n"));
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
fn start_workspace_starts_the_layout_in_order_with_its_shares_and_the_setup_under_it() {
    let sb = Sandbox::new();
    let d = sb.start();
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
        },
    );
    let ids: Vec<String> = r["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 4, "cat, a shell, the editor and the setup");

    open(&mut c, &ws);
    wait_for("the layout", || {
        c.wait_until(Duration::from_millis(50), |c| {
            leaf_ids(&c.nodes[&ws]["layout"]).len() == 4
        })
        .unwrap_or(false)
    });
    let l = c.nodes[&ws]["layout"].clone();
    // Under everything, the setup keeps about a third.
    assert_eq!(
        (l["t"].as_str(), l["dir"].as_str(), l["ratio"].as_u64()),
        (Some("split"), Some("col"), Some(680))
    );
    assert_eq!(l["b"]["pane"], ids[3].as_str());
    // The layout above it: a on the left with 60%, b down.
    let above = &l["a"];
    assert_eq!(
        (above["dir"].as_str(), above["ratio"].as_u64()),
        (Some("row"), Some(600))
    );
    assert_eq!(above["a"]["pane"], ids[0].as_str());
    assert_eq!(
        (above["b"]["dir"].as_str(), above["b"]["ratio"].as_u64()),
        (Some("col"), Some(500))
    );
    assert_eq!(leaf_ids(&l), ids);

    let title = |c: &Client, id: &str| {
        c.nodes[&ws]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p["title"].as_str().unwrap().to_owned())
    };
    assert_eq!(title(&c, &ids[3]).as_deref(), Some("setup"), "named");
    assert!(
        ids[..3]
            .iter()
            .all(|i| title(&c, i).as_deref() != Some("setup"))
    );

    // The programs: the first leaf runs `cat`, the third is the editor, `cat -n`.
    for (i, argv) in [
        (0, serde_json::json!(["cat"])),
        (2, serde_json::json!(["cat", "-n"])),
    ] {
        let pane = &ids[i];
        open(&mut c, pane);
        assert!(
            c.wait_until(WAIT, |c| c.nodes[pane]["running"]["argv"] == argv)
                .unwrap(),
            "pane {i}"
        );
    }

    // The setup: each line after `❯ ` and then run, with the markers quoted.
    let setup = &ids[3];
    open(&mut c, setup);
    wait_for("the setup's output", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, setup).contains("done-feat/ws-1")
    });
    let s = screen(&c, setup);
    assert!(s.contains("❯ echo installing {name}"), "{s}");
    assert!(s.contains("installing ws-1"), "{s}");
    assert!(s.contains("❯ echo done-{branch}"), "{s}");
    d.stop();
}

#[test]
fn the_setup_stops_at_the_first_line_that_fails_and_the_pane_stays_a_shell() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(
        &sb,
        Some("setup = [\"echo first\", \"false\", \"echo never\"]\n"),
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
        },
    );
    let ids = r["panes"].as_array().unwrap();
    assert_eq!(ids.len(), 2, "the shell and the setup");
    let setup = ids[1].as_str().unwrap().to_owned();
    open(&mut c, &setup);
    wait_for("the failing line", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, &setup).contains("❯ false")
    });
    // Its end is reported as a shell would, with the exit status of the line that failed.
    let root = c.root.clone();
    open(&mut c, &root);
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&root]["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "shell_done"
                && a["node"] == setup.as_str()
                && a["exit_code"] == 1))
            .unwrap(),
        "{}",
        c.nodes[&root]["attention"]
    );
    let s = screen(&c, &setup);
    assert!(s.contains("❯ echo first") && s.contains("first"), "{s}");
    assert!(!s.contains("never"), "{s}");

    // Still a shell: it takes input.
    c.cmd(Cmd::Write {
        pane: setup.clone(),
        data: "echo still-here\r".into(),
    })
    .unwrap()
    .unwrap();
    wait_for("the shell", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, &setup).contains("still-here")
    });
    d.stop();
}

#[test]
fn design_studios_file_names_one_agent_pane() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(
        &sb,
        Some(
            "setup = [\"true\"]\n[agent]\ndefault = \"claude\"\n[layout]\neditor = \"nvim\"\nrun = \"agent\"\n",
        ),
    );
    let mut c = sb.client();
    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(r["panes"], serde_json::json!(["claude"]));
    d.stop();
}

#[test]
fn without_a_file_or_for_a_folder_start_workspace_opens_one_shell_and_a_workspace_with_panes_conflicts()
 {
    let sb = Sandbox::new();
    let d = sb.start();
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
        },
    );
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(code, ErrorCode::Conflict);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: "nope".into(),
        },
    );
    assert_eq!(code, ErrorCode::NotFound);
    open(&mut c, &ws);
    assert_eq!(c.nodes[&ws]["panes"].as_array().unwrap().len(), 1);

    // A folder gets one shell whatever its repository's file says.
    std::fs::write(top.join(".jw.toml"), FULL).unwrap();
    let folder = ok(&mut c, Cmd::OpenFolder { path: text(&top) })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(&mut c, Cmd::StartWorkspace { workspace: folder });
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    d.stop();
}
