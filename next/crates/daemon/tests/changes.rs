//! N32: the changes of a workspace, read from real git by a real daemon (REQ-91 to REQ-103).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, DiffScope, ErrorCode};
use serde_json::Value;

/// A repository with one commit (`f.txt`, `g.txt`) at `<sandbox>/work/app`.
fn repo(sb: &Sandbox) -> PathBuf {
    let dir = sb.dir.join("work/app");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("g.txt"), "gone soon\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap()
}

/// Say that origin's default branch is `main` as of now.
fn with_origin(dir: &Path) {
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
}

fn open_workspace(c: &mut wire::Client, dir: &Path) -> String {
    c.cmd(Cmd::OpenFolder { path: text(dir) })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn node_of(reply: Option<Value>) -> String {
    reply.unwrap()["node"].as_str().unwrap().to_owned()
}

fn diff(c: &mut wire::Client, ws: &str, scope: DiffScope) -> String {
    node_of(
        c.cmd(Cmd::OpenDiff {
            workspace: ws.into(),
            scope,
            commit: None,
        })
        .unwrap()
        .unwrap(),
    )
}

fn watch(c: &mut wire::Client, node: &str) -> Value {
    c.open(node).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(node)).unwrap());
    c.nodes[node].clone()
}

fn watch_diff(c: &mut wire::Client, ws: &str, scope: DiffScope) -> Value {
    let node = diff(c, ws, scope);
    watch(c, &node)
}

fn watch_commits(c: &mut wire::Client, ws: &str) -> Value {
    let node = node_of(
        c.cmd(Cmd::OpenCommits {
            workspace: ws.into(),
        })
        .unwrap()
        .unwrap(),
    );
    watch(c, &node)
}

fn file<'a>(node: &'a Value, path: &str) -> &'a Value {
    node["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("no {path} in {node}"))
}

#[test]
fn uncommitted_lists_what_changed_and_what_is_new() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    std::fs::write(app.join("f.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::remove_file(app.join("g.txt")).unwrap();
    std::fs::write(app.join("new.txt"), "a\nb\n").unwrap();
    std::fs::write(app.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    std::os::unix::fs::symlink("f.txt", app.join("link")).unwrap();
    std::fs::write(app.join(".gitignore"), "ignored.log\n").unwrap();
    std::fs::write(app.join("ignored.log"), "noise").unwrap();

    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    assert_eq!(diff(&mut c, &ws, DiffScope::Uncommitted), node, "one node");
    let n = watch(&mut c, &node);

    assert_eq!(
        (n["type"].as_str(), n["scope"].as_str(), n["state"].as_str()),
        (Some("diff"), Some("uncommitted"), Some("ok"))
    );
    let f = file(&n, "f.txt");
    assert_eq!(
        (
            f["status"].as_str(),
            f["added"].as_u64(),
            f["deleted"].as_u64()
        ),
        (Some("modified"), Some(2), Some(1))
    );
    let h = &f["hunks"][0];
    assert_eq!(h["lines"][0], " one");
    assert!(h["lines"].as_array().unwrap().iter().any(|l| l == "-two"));
    assert!(h["lines"].as_array().unwrap().iter().any(|l| l == "+four"));
    assert_eq!(file(&n, "g.txt")["status"], "deleted");
    let new = file(&n, "new.txt");
    assert_eq!(
        (new["status"].as_str(), new["added"].as_u64()),
        (Some("untracked"), Some(2))
    );
    assert_eq!(new["hunks"][0]["lines"], serde_json::json!(["+a", "+b"]));
    assert_eq!(new["hunks"][0]["new_start"], 1);
    assert_eq!(file(&n, "bin.dat")["binary"], true);
    assert!(file(&n, "bin.dat")["hunks"].as_array().unwrap().is_empty());
    assert_eq!(file(&n, "link")["unreadable"], "symlink");
    assert!(
        n["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["path"] != "ignored.log"),
        "what git ignores is not listed"
    );
    d.stop();
}

#[test]
fn a_repository_with_no_commit_compares_with_nothing() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("fresh");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    std::fs::write(dir.join("b.txt"), "y\n").unwrap();
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &dir);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    let n = watch(&mut c, &node);
    assert_eq!(file(&n, "a.txt")["status"], "added");
    assert_eq!(file(&n, "b.txt")["status"], "untracked");
    assert!(
        watch_commits(&mut c, &ws)["commits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    d.stop();
}

#[test]
fn a_folder_inside_a_repository_shows_the_repositorys_changes() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    std::fs::create_dir_all(app.join("sub")).unwrap();
    std::fs::write(app.join("f.txt"), "changed\n").unwrap();
    std::fs::write(app.join("top.txt"), "new\n").unwrap();
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app.join("sub"));
    let n = watch_diff(&mut c, &ws, DiffScope::Uncommitted);
    assert_eq!(file(&n, "f.txt")["status"], "modified");
    assert_eq!(file(&n, "top.txt")["status"], "untracked");
    d.stop();
}

#[test]
fn an_edit_arrives_by_itself_while_the_node_is_open() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    let n = watch(&mut c, &node);
    assert!(n["files"].as_array().unwrap().is_empty());
    let rev = n["rev"].as_u64().unwrap();

    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    assert!(
        c.wait_until(Duration::from_secs(8), |c| {
            c.nodes[&node]["files"]
                .as_array()
                .is_some_and(|f| f.len() == 1)
        })
        .unwrap(),
        "the poll found the edit"
    );
    assert!(c.nodes[&node]["rev"].as_u64().unwrap() > rev);
    d.stop();
}

#[test]
fn general_is_the_branch_against_its_merge_base_and_commits_list_it() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    with_origin(&app);
    git(&app, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    git(&app, &["commit", "-q", "-am", "add four"]);
    std::fs::write(app.join("h.txt"), "h\n").unwrap();
    git(&app, &["add", "h.txt"]);
    git(&app, &["commit", "-q", "-m", "add h"]);
    // A change on main the branch does not have must not show up as a deletion.
    git(&app, &["checkout", "-q", "main"]);
    std::fs::write(app.join("main-only.txt"), "m\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-q", "-m", "main moves"]);
    git(&app, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&app, &["checkout", "-q", "feat"]);

    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let n = watch_diff(&mut c, &ws, DiffScope::General);
    assert_eq!(n["base"], "origin/main");
    assert_eq!(file(&n, "f.txt")["status"], "modified");
    assert_eq!(file(&n, "h.txt")["status"], "added");
    assert!(
        n["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["path"] != "main-only.txt"),
        "compared with the merge base, not with the tip of main"
    );

    let commits = watch_commits(&mut c, &ws);
    let subjects: Vec<_> = commits["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["subject"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        subjects,
        ["add h", "add four"],
        "newest first, only the branch's"
    );
    let first = &commits["commits"][0];
    assert_eq!(first["sha"].as_str().unwrap().len(), 40);
    assert!(
        first["sha"]
            .as_str()
            .unwrap()
            .starts_with(first["short"].as_str().unwrap())
    );
    assert!(first["time"].as_u64().unwrap() > 1_600_000_000);
    d.stop();
}

#[test]
fn without_a_default_branch_general_has_nothing_to_compare_with() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let n = watch_diff(&mut c, &ws, DiffScope::General);
    assert_eq!(n["state"], "no_base");
    assert!(n["files"].as_array().unwrap().is_empty());
    assert!(n.get("base").is_none());
    // The commits are the branch's whole history then.
    let commits = watch_commits(&mut c, &ws);
    assert_eq!(commits["commits"].as_array().unwrap().len(), 1);
    assert!(commits.get("base").is_none());
    d.stop();
}

#[test]
fn one_commit_is_one_scope_however_it_is_named() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    git(&app, &["commit", "-q", "-am", "four"]);
    let full = {
        let out = Command::new("git")
            .arg("-C")
            .arg(&app)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let open = |c: &mut wire::Client, sha: &str| {
        c.cmd(Cmd::OpenDiff {
            workspace: ws.clone(),
            scope: DiffScope::Commit,
            commit: Some(sha.into()),
        })
        .unwrap()
    };
    let short = node_of(open(&mut c, &full[..7]).unwrap());
    assert_eq!(node_of(open(&mut c, &full).unwrap()), short);
    let n = watch(&mut c, &short);
    assert_eq!(n["commit"], full.as_str());
    assert_eq!(n["scope"], "commit");
    assert_eq!(file(&n, "f.txt")["added"], 1);

    for bad in ["--output=x", "main", "abc", ""] {
        assert_eq!(
            open(&mut c, bad).unwrap_err().code,
            ErrorCode::BadRequest,
            "{bad:?}"
        );
    }
    assert_eq!(
        open(&mut c, "deadbeef").unwrap_err().code,
        ErrorCode::GitFailed
    );
    assert_eq!(
        c.cmd(Cmd::OpenDiff {
            workspace: ws,
            scope: DiffScope::Commit,
            commit: None
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    d.stop();
}

#[test]
fn what_is_not_a_repository_or_a_workspace_is_refused_and_leaves_no_node() {
    let sb = Sandbox::new();
    let d = sb.start();
    let plain = std::fs::canonicalize(&sb.dir).unwrap().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &plain);
    for scope in [DiffScope::Uncommitted, DiffScope::General] {
        let e = c
            .cmd(Cmd::OpenDiff {
                workspace: ws.clone(),
                scope,
                commit: None,
            })
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
    }
    assert_eq!(
        c.cmd(Cmd::OpenCommits {
            workspace: ws.clone()
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    let missing = "00000000-0000-4000-8000-000000000999";
    assert_eq!(
        c.cmd(Cmd::OpenDiff {
            workspace: missing.into(),
            scope: DiffScope::Uncommitted,
            commit: None
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        c.cmd(Cmd::RefreshDiff {
            node: missing.into()
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    d.stop();
}

#[test]
fn a_file_looked_at_is_marked_for_everyone_until_it_changes() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let mut a = sb.client();
    let mut b = sb.client();
    let ws = open_workspace(&mut a, &app);
    let node = diff(&mut a, &ws, DiffScope::Uncommitted);
    watch(&mut a, &node);
    let n = watch(&mut b, &node);
    assert!(file(&n, "f.txt").get("seen").is_none());

    a.cmd(Cmd::SetViewed {
        node: node.clone(),
        path: "f.txt".into(),
        viewed: true,
    })
    .unwrap()
    .unwrap();
    assert!(
        b.wait_until(WAIT, |b| b.nodes[&node]["files"][0]["seen"] == "yes")
            .unwrap(),
        "the other client sees the mark"
    );

    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    assert!(
        b.wait_until(
            Duration::from_secs(8),
            |b| b.nodes[&node]["files"][0]["seen"] == "changed"
        )
        .unwrap(),
        "a file that differs from what was looked at says so"
    );

    a.cmd(Cmd::SetViewed {
        node: node.clone(),
        path: "f.txt".into(),
        viewed: false,
    })
    .unwrap()
    .unwrap();
    assert!(
        b.wait_until(WAIT, |b| b.nodes[&node]["files"][0].get("seen").is_none())
            .unwrap()
    );
    let e = a
        .cmd(Cmd::SetViewed {
            node: node.clone(),
            path: "nope.txt".into(),
            viewed: true,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn the_marks_survive_a_restart_and_go_when_the_file_leaves_the_diff() {
    let sb = Sandbox::new();
    let app = repo(&sb);
    std::fs::write(app.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let d = sb.start();
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    watch(&mut c, &node);
    c.cmd(Cmd::SetViewed {
        node,
        path: "f.txt".into(),
        viewed: true,
    })
    .unwrap()
    .unwrap();
    d.stop();

    let d = sb.start();
    let mut c = sb.client();
    let ws = {
        let root = c.root.clone();
        c.open(&root).unwrap();
        assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
        c.nodes[&root]["workspaces"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    let n = watch(&mut c, &node);
    assert_eq!(file(&n, "f.txt")["seen"], "yes", "kept across the restart");

    git(&app, &["commit", "-q", "-am", "four"]);
    c.cmd(Cmd::RefreshDiff { node: node.clone() })
        .unwrap()
        .unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&node]["files"]
            .as_array()
            .is_some_and(Vec::is_empty))
            .unwrap()
    );
    let saved = std::fs::read_to_string(sb.dir.join("state/session.json")).unwrap();
    assert!(
        !saved.contains("viewed"),
        "the mark of a file that left the diff is dropped: {saved}"
    );
    d.stop();
}

#[test]
fn a_diff_over_the_limit_lists_names_only() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let big = "0123456789abcdef\n".repeat(900);
    for i in 0..250 {
        std::fs::write(app.join(format!("m{i:03}.txt")), "base\n").unwrap();
    }
    git(&app, &["add", "."]);
    git(&app, &["commit", "-q", "-m", "many"]);
    for i in 0..250 {
        std::fs::write(app.join(format!("m{i:03}.txt")), format!("base\n{big}")).unwrap();
    }
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    let n = watch(&mut c, &node);
    assert_eq!(n["too_large"], true);
    assert_eq!(n["files"].as_array().unwrap().len(), 200);
    assert_eq!(n["omitted"], 50);
    assert!(
        n["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["hunks"].as_array().unwrap().is_empty() && f["status"] == "modified")
    );
    let e = c
        .cmd(Cmd::SetViewed {
            node,
            path: "m000.txt".into(),
            viewed: true,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    d.stop();
}

#[test]
fn removing_a_workspace_takes_its_nodes_with_it() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open_workspace(&mut c, &app);
    let node = diff(&mut c, &ws, DiffScope::Uncommitted);
    watch(&mut c, &node);
    c.cmd(Cmd::RemoveWorkspace {
        workspace: ws,
        remove_worktree: false,
    })
    .unwrap()
    .unwrap();
    assert!(
        c.wait_until(WAIT, |c| !c.nodes.contains_key(&node))
            .unwrap(),
        "gone was sent"
    );
    d.stop();
}
