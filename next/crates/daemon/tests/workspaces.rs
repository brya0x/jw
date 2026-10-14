//! N3: folders and git worktrees as workspaces, against real git in a temp dir.

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, ErrorCode};
use serde_json::Value;

/// A repository with one commit, at `<sandbox>/work/app`.
fn repo(sb: &Sandbox) -> PathBuf {
    let dir = sb.dir.join("work/app");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap()
}

fn session(c: &mut wire::Client) -> Value {
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    c.nodes[&root].clone()
}

fn id_of(reply: Option<Value>) -> String {
    reply.unwrap()["workspace"].as_str().unwrap().to_owned()
}

#[test]
fn a_folder_becomes_a_workspace_everyone_sees() {
    let sb = Sandbox::new();
    let d = sb.start();
    let folder = std::fs::canonicalize(&sb.dir).unwrap().join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let mut watcher = sb.client();
    session(&mut watcher);
    let root = watcher.root.clone();

    let mut c = sb.client();
    let ws = id_of(
        c.cmd(Cmd::OpenFolder {
            path: text(&folder),
        })
        .unwrap()
        .unwrap(),
    );

    assert!(
        watcher
            .wait_until(WAIT, |w| w.nodes[&root]["workspaces"]
                .as_array()
                .is_some_and(|a| a.len() == 1))
            .unwrap()
    );
    let summary = &watcher.nodes[&root]["workspaces"][0];
    assert_eq!(
        (
            summary["id"].as_str(),
            summary["name"].as_str(),
            summary["kind"].as_str()
        ),
        (Some(ws.as_str()), Some("notes"), Some("folder"))
    );
    assert_eq!(summary["pane_count"], 0);

    c.open(&ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&ws)).unwrap());
    assert_eq!(c.nodes[&ws]["type"], "workspace");
    assert_eq!(c.nodes[&ws]["path"], text(&folder));
    d.stop();
}

#[test]
fn open_folder_refuses_what_is_not_an_open_directory() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let dir = std::fs::canonicalize(&sb.dir).unwrap();
    let file = dir.join("a-file");
    std::fs::write(&file, "x").unwrap();
    let code = |c: &mut wire::Client, path: String| {
        c.cmd(Cmd::OpenFolder { path }).unwrap().unwrap_err().code
    };
    assert_eq!(
        code(&mut c, text(&dir.join("missing"))),
        ErrorCode::NotFound
    );
    assert_eq!(code(&mut c, text(&file)), ErrorCode::BadRequest);
    assert_eq!(code(&mut c, "relative/dir".into()), ErrorCode::BadRequest);
    c.cmd(Cmd::OpenFolder { path: text(&dir) })
        .unwrap()
        .unwrap();
    assert_eq!(code(&mut c, text(&dir)), ErrorCode::Conflict);
    d.stop();
}

#[test]
fn a_worktree_is_made_beside_the_repo_and_opened() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/login".into(),
            base: Some("main".into()),
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt = app.parent().unwrap().join("app-wt/feat-login");
    assert!(
        wt.join("f.txt").exists(),
        "the checkout exists at {}",
        wt.display()
    );

    c.open(&ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&ws)).unwrap());
    let n = &c.nodes[&ws];
    assert_eq!(
        (n["kind"].as_str(), n["branch"].as_str(), n["name"].as_str()),
        (
            Some("worktree"),
            Some("feat/login"),
            Some("app: feat/login")
        )
    );
    assert_eq!(n["project"], text(&app));
    assert_eq!(n["path"], text(&std::fs::canonicalize(&wt).unwrap()));
    let summary = &session(&mut c)["workspaces"][0];
    assert_eq!(summary["project"], text(&app), "the summary says it too");
    d.stop();
}

#[test]
fn an_existing_branch_is_checked_out_and_a_bad_request_is_refused() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    git(&app, &["branch", "old"]);
    let mut c = sb.client();
    let make = |c: &mut wire::Client, repo: &Path, branch: &str, base: Option<&str>| {
        c.cmd(Cmd::CreateWorktree {
            repo: text(repo),
            branch: branch.into(),
            base: base.map(Into::into),
            folder: None,
        })
        .unwrap()
    };
    make(&mut c, &app, "old", None).unwrap();
    assert!(app.parent().unwrap().join("app-wt/old/f.txt").exists());
    // A base only means something for a new branch.
    git(&app, &["branch", "other"]);
    assert_eq!(
        make(&mut c, &app, "other", Some("main")).unwrap_err().code,
        ErrorCode::BadRequest
    );
    // Names that are options or not branch names.
    assert_eq!(
        make(&mut c, &app, "-evil", None).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        make(&mut c, &app, "has space", None).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        make(&mut c, &app, "new", Some("--detach"))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    // Opened already.
    assert_eq!(
        make(&mut c, &app, "old", None).unwrap_err().code,
        ErrorCode::Conflict
    );
    // Not a repository.
    let plain = std::fs::canonicalize(&sb.dir).unwrap();
    assert_eq!(
        make(&mut c, &plain, "x", None).unwrap_err().code,
        ErrorCode::GitFailed
    );
    // An unknown base is git's error, passed on.
    let e = make(&mut c, &app, "fresh", Some("nope")).unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
    assert!(!e.message.is_empty());
    d.stop();
}

#[test]
fn removing_a_workspace_keeps_the_folder_and_can_remove_a_clean_worktree() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    session(&mut c);
    let folder = c
        .cmd(Cmd::OpenFolder { path: text(&app) })
        .unwrap()
        .unwrap();
    let folder = id_of(folder);
    let wt = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "b1".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt_dir = app.parent().unwrap().join("app-wt/b1");

    c.open(&wt).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&wt)).unwrap());

    // remove_worktree on a folder makes no sense.
    let e = c
        .cmd(Cmd::RemoveWorkspace {
            workspace: folder.clone(),
            remove_worktree: true,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    // A dirty worktree is kept, with its workspace.
    std::fs::write(wt_dir.join("dirty"), "x").unwrap();
    let e = c
        .cmd(Cmd::RemoveWorkspace {
            workspace: wt.clone(),
            remove_worktree: true,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
    assert!(wt_dir.exists());
    std::fs::remove_file(wt_dir.join("dirty")).unwrap();

    c.cmd(Cmd::RemoveWorkspace {
        workspace: wt.clone(),
        remove_worktree: true,
    })
    .unwrap()
    .unwrap();
    assert!(!wt_dir.exists());
    // The folder goes and the branch stays, with whatever commits it held (REQ-147).
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&app)
        .args(["branch", "--list", "b1"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains("b1"),
        "the branch was deleted"
    );
    // The `gone` arrives before the reply, so the cache has already dropped the node.
    assert!(!c.nodes.contains_key(&wt));

    // A folder workspace goes without touching the directory.
    c.cmd(Cmd::RemoveWorkspace {
        workspace: folder.clone(),
        remove_worktree: false,
    })
    .unwrap()
    .unwrap();
    assert!(app.exists());
    let e = c
        .cmd(Cmd::RemoveWorkspace {
            workspace: folder,
            remove_worktree: false,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn workspaces_survive_a_restart() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "keep".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    drop(c);
    d.stop();

    let d = sb.start();
    let mut c = sb.client();
    let s = session(&mut c);
    assert_eq!(s["workspaces"][0]["id"], ws.as_str());
    assert_eq!(s["workspaces"][0]["branch"], "keep");
    d.stop();
}

#[test]
fn a_workspace_is_renamed_without_touching_its_folder_and_an_empty_name_brings_the_old_back() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    session(&mut c);
    let folder = id_of(
        c.cmd(Cmd::OpenFolder { path: text(&app) })
            .unwrap()
            .unwrap(),
    );
    let wt = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/login".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let names = |c: &mut wire::Client| -> Vec<String> {
        let s = session(c);
        s["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = names(&mut c);
    for (id, name) in [(&folder, "my app"), (&wt, "login fix")] {
        c.cmd(Cmd::RenameWorkspace {
            workspace: id.clone(),
            name: format!(" {name} "),
        })
        .unwrap()
        .unwrap();
    }
    assert_eq!(names(&mut c), ["my app", "login fix"]);
    assert!(app.exists(), "the folder is untouched");
    assert!(app.parent().unwrap().join("app-wt/feat-login").exists());
    // It is kept across a restart.
    drop(c);
    d.stop();
    let d = sb.start();
    let mut c = sb.client();
    assert_eq!(names(&mut c), ["my app", "login fix"]);
    // An empty name brings back the one each had.
    for id in [&folder, &wt] {
        c.cmd(Cmd::RenameWorkspace {
            workspace: id.clone(),
            name: "".into(),
        })
        .unwrap()
        .unwrap();
    }
    assert_eq!(names(&mut c), before);
    d.stop();
}

#[test]
fn a_worktree_folder_may_be_named_and_cannot_leave_its_parent() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    session(&mut c);
    let make = |c: &mut wire::Client, branch: &str, folder: Option<&str>| {
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: branch.into(),
            base: None,
            folder: folder.map(Into::into),
        })
        .unwrap()
    };
    // The folder is the name, the branch is its own: `<repo>-wt/login` on `feat/login`.
    let made = make(&mut c, "feat/login", Some("login")).unwrap().unwrap();
    let wt = app.parent().unwrap().join("app-wt");
    assert!(wt.join("login").exists());
    assert!(!wt.join("feat-login").exists());
    assert_eq!(made["path"], text(&wt.join("login")).as_str());
    // Without one it is still the branch with dashes.
    make(&mut c, "feat/other", None).unwrap().unwrap();
    assert!(wt.join("feat-other").exists());
    // A name that is not one path component is refused before git is run.
    for bad in ["", ".", "..", "a/b", "../escape", "x\ny"] {
        let e = make(&mut c, "feat/bad", Some(bad)).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest, "{bad:?}");
    }
    assert!(!wt.join("escape").exists() && !app.parent().unwrap().join("escape").exists());
    // A folder that is there is a conflict.
    let e = make(&mut c, "feat/again", Some("login")).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    d.stop();
}

#[test]
fn list_dir_names_the_folders_and_marks_repositories() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("browse");
    for name in ["Beta", "alpha", ".hidden", "repo/.git"] {
        std::fs::create_dir_all(dir.join(name)).unwrap();
    }
    std::fs::write(dir.join("a-file"), "x").unwrap();
    let list = |c: &mut wire::Client, hidden: bool| {
        c.cmd(Cmd::ListDir {
            path: Some(text(&dir)),
            hidden,
        })
        .unwrap()
        .unwrap()
        .unwrap()
    };
    let seen = list(&mut c, false);
    assert_eq!(seen["path"], text(&dir));
    assert_eq!(seen["parent"], text(dir.parent().unwrap()));
    assert_eq!(seen["total"], 3);
    let names: Vec<_> = seen["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_owned(),
                e["git"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("alpha".into(), false),
            ("Beta".into(), false),
            ("repo".into(), true)
        ]
    );
    assert_eq!(list(&mut c, true)["total"], 4);
    d.stop();
}

#[test]
fn list_dir_without_a_path_starts_at_home_and_refuses_what_is_not_a_folder() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let home = c
        .cmd(Cmd::ListDir {
            path: None,
            hidden: false,
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(home["path"], home["home"]);
    let dir = std::fs::canonicalize(&sb.dir).unwrap();
    std::fs::write(dir.join("a-file"), "x").unwrap();
    let code = |c: &mut wire::Client, path: String| {
        c.cmd(Cmd::ListDir {
            path: Some(path),
            hidden: false,
        })
        .unwrap()
        .unwrap_err()
        .code
    };
    assert_eq!(
        code(&mut c, text(&dir.join("missing"))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(&mut c, text(&dir.join("a-file"))),
        ErrorCode::BadRequest
    );
    assert_eq!(code(&mut c, "relative".into()), ErrorCode::BadRequest);
    d.stop();
}
