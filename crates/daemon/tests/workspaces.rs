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

/// The jobs of the session as `c` holds them, once `done` holds of them (the session is open).
fn wait_jobs(c: &mut wire::Client, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let root = c.root.clone();
    let jobs = |c: &wire::Client| {
        c.nodes[&root]["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let ok = c.wait_until(WAIT, |c| done(&jobs(c))).unwrap();
    assert!(
        ok,
        "the jobs never were what the test waits for: {:?}",
        jobs(c)
    );
    jobs(c)
}

fn job_of(reply: Option<Value>) -> String {
    reply.unwrap()["job"].as_str().unwrap().to_owned()
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
            force: false,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    // A dirty worktree is kept, with its workspace: the request is answered at once with the job,
    // and the job is what fails, with what git said (REQ-648, REQ-649).
    std::fs::write(wt_dir.join("dirty"), "x").unwrap();
    let mut seen = sb.client();
    session(&mut seen);
    let job = job_of(
        c.cmd(Cmd::RemoveWorkspace {
            workspace: wt.clone(),
            remove_worktree: true,
            force: false,
        })
        .unwrap()
        .unwrap(),
    );
    let jobs = wait_jobs(&mut seen, |j| {
        j.iter()
            .any(|j| j["id"] == job.as_str() && j["state"] != "running")
    });
    assert_eq!(jobs[0]["state"], "failed");
    assert_eq!(jobs[0]["kind"], "remove_workspace");
    assert_eq!(jobs[0]["name"], "app: b1");
    assert!(
        jobs[0]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("--force")),
        "git's own message: {}",
        jobs[0]["detail"]
    );
    assert!(jobs[0]["duration_ms"].is_u64());
    assert!(wt_dir.exists());
    let root = seen.root.clone();
    let row = |c: &wire::Client, id: &str| {
        c.nodes[&root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .cloned()
    };
    let kept = row(&seen, &wt).expect("the workspace stays");
    assert!(
        kept.get("removing").is_none(),
        "a failed removal leaves the workspace as it was"
    );
    std::fs::remove_file(wt_dir.join("dirty")).unwrap();

    // The clean one goes: the workspace with it, in the same patch that ends the job.
    let job = job_of(
        c.cmd(Cmd::RemoveWorkspace {
            workspace: wt.clone(),
            remove_worktree: true,
            force: false,
        })
        .unwrap()
        .unwrap(),
    );
    let jobs = wait_jobs(&mut seen, |j| {
        j.iter()
            .any(|j| j["id"] == job.as_str() && j["state"] == "done")
    });
    assert_eq!(jobs[0]["id"], job.as_str(), "the newest job is first");
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
    assert!(row(&seen, &wt).is_none());
    assert!(
        seen.wait_until(WAIT, |s| !s.nodes.contains_key(&wt))
            .unwrap()
    );
    assert!(c.wait_until(WAIT, |c| !c.nodes.contains_key(&wt)).unwrap());

    // A folder workspace goes without touching the directory.
    c.cmd(Cmd::RemoveWorkspace {
        workspace: folder.clone(),
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();
    assert!(app.exists());
    let e = c
        .cmd(Cmd::RemoveWorkspace {
            workspace: folder,
            remove_worktree: false,
            force: false,
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
fn a_worktree_is_renamed_a_folder_is_not_and_an_empty_name_brings_the_old_back() {
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
    c.cmd(Cmd::RenameWorkspace {
        workspace: wt.clone(),
        name: " login fix ".into(),
    })
    .unwrap()
    .unwrap();
    assert_eq!(names(&mut c), [before[0].as_str(), "login fix"]);
    assert!(app.exists(), "the folder is untouched");
    assert!(app.parent().unwrap().join("app-wt/feat-login").exists());
    // It is kept across a restart.
    drop(c);
    d.stop();
    let d = sb.start();
    let mut c = sb.client();
    assert_eq!(names(&mut c), [before[0].as_str(), "login fix"]);
    // A folder is a directory on disk: it is not renamed.
    let e = c
        .cmd(Cmd::RenameWorkspace {
            workspace: folder.clone(),
            name: "my app".into(),
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert_eq!(names(&mut c)[0], before[0]);
    // An empty name brings back the one the worktree had.
    c.cmd(Cmd::RenameWorkspace {
        workspace: wt.clone(),
        name: "".into(),
    })
    .unwrap()
    .unwrap();
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

#[test]
fn list_dir_sends_the_first_500_folders_and_counts_them_all() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("many");
    for n in 0..505 {
        std::fs::create_dir_all(dir.join(format!("d{n:03}"))).unwrap();
    }
    let seen = c
        .cmd(Cmd::ListDir {
            path: Some(text(&dir)),
            hidden: false,
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(seen["total"], 505);
    assert_eq!(seen["dirs"].as_array().unwrap().len(), 500);
    assert_eq!(seen["dirs"][499]["name"], "d499");
    d.stop();
}

#[test]
fn a_repositorys_root_is_its_own_project_and_the_session_lists_repositories_before_other_folders() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let plain = std::fs::canonicalize(&sb.dir).unwrap().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut c = sb.client();
    let open = |c: &mut wire::Client, path: &Path| {
        c.cmd(Cmd::OpenFolder { path: text(path) })
            .unwrap()
            .unwrap()
            .unwrap();
    };
    open(&mut c, &plain);
    open(&mut c, &app);
    c.cmd(Cmd::CreateWorktree {
        repo: text(&app),
        branch: "feat/x".into(),
        base: None,
        folder: None,
    })
    .unwrap()
    .unwrap();
    let list = &session(&mut c)["workspaces"];
    let seen: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["kind"].as_str().unwrap(), w["project"].as_str()))
        .collect();
    let app_text = text(&app);
    assert_eq!(
        seen,
        [
            ("folder", Some(app_text.as_str())),
            ("worktree", Some(app_text.as_str())),
            ("folder", None),
        ]
    );
    assert_eq!(list[0]["path"], app_text);
    d.stop();
}

/// REQ-274 to REQ-277: `sync_workspace` pushes a worktree's branch, never forced.
#[test]
fn sync_pushes_the_worktrees_branch_and_says_how_many_commits_went_up() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let origin = sb.dir.join("origin.git");
    git(&sb.dir, &["init", "-q", "--bare", &text(&origin)]);
    git(&app, &["remote", "add", "origin", &text(&origin)]);
    git(&app, &["push", "-q", "origin", "main"]);
    let mut c = sb.client();
    session(&mut c);
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/login".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt = app.parent().unwrap().join("app-wt/feat-login");
    let sync = |c: &mut wire::Client, ws: &str| {
        c.cmd(Cmd::SyncWorkspace {
            workspace: ws.into(),
        })
    };
    for n in ["a", "b"] {
        std::fs::write(wt.join(n), n).unwrap();
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-q", "-m", n]);
    }
    let ahead = |c: &mut wire::Client| {
        c.cmd(Cmd::CheckWorkspace {
            workspace: ws.clone(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["ahead"]
            .as_u64()
    };
    assert_eq!(ahead(&mut c), Some(2), "both commits are in no remote");
    // A branch the remote does not have yet: it is made there, with the commits no remote has.
    let r = sync(&mut c, &ws).unwrap().unwrap().unwrap();
    assert_eq!(
        (
            r["branch"].as_str(),
            r["pushed"].as_u64(),
            r["new_branch"].as_bool()
        ),
        (Some("feat/login"), Some(2), Some(true))
    );
    let on_origin = |rev: &str| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&origin)
            .args(["rev-parse", rev])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let head = |dir: &Path| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    assert_eq!(on_origin("feat/login"), head(&wt));

    assert_eq!(ahead(&mut c), Some(0), "all went up");
    // Nothing new: nothing is pushed.
    let r = sync(&mut c, &ws).unwrap().unwrap().unwrap();
    assert_eq!(
        (r["pushed"].as_u64(), r["new_branch"].as_bool()),
        (Some(0), Some(false))
    );

    // One commit more goes up to the branch that is there.
    std::fs::write(wt.join("c"), "c").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "c"]);
    let r = sync(&mut c, &ws).unwrap().unwrap().unwrap();
    assert_eq!(
        (r["pushed"].as_u64(), r["new_branch"].as_bool()),
        (Some(1), Some(false))
    );
    assert_eq!(on_origin("feat/login"), head(&wt));

    // The remote has a commit the worktree has not: the push is refused, never forced.
    let other = sb.dir.join("other");
    git(&sb.dir, &["clone", "-q", &text(&origin), &text(&other)]);
    git(&other, &["checkout", "-q", "feat/login"]);
    std::fs::write(other.join("z"), "z").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "z"]);
    git(&other, &["push", "-q", "origin", "feat/login"]);
    std::fs::write(wt.join("d"), "d").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "d"]);
    let e = sync(&mut c, &ws).unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
    assert!(e.message.contains("rejected"), "{}", e.message);
    assert_eq!(
        on_origin("feat/login"),
        head(&other),
        "the remote kept its own"
    );

    // A folder has no branch of its own to push, and an unknown workspace is not found.
    let folder = id_of(
        c.cmd(Cmd::OpenFolder { path: text(&app) })
            .unwrap()
            .unwrap(),
    );
    let e = sync(&mut c, &folder).unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let e = sync(&mut c, "nope").unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn sync_without_a_remote_says_what_git_says() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    session(&mut c);
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/x".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt = app.parent().unwrap().join("app-wt/feat-x");
    std::fs::write(wt.join("a"), "a").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "a"]);
    let e = c
        .cmd(Cmd::SyncWorkspace { workspace: ws })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
    assert!(e.message.contains("origin"), "{}", e.message);
    d.stop();
}

/// REQ-604: a push that waits (here a `pre-push` hook that sleeps) is stopped by `cancel` from another
/// connection: the sync answers `cancelado` at once and the daemon does not keep the operation.
#[test]
fn a_push_that_waits_is_stopped_by_cancel_and_can_be_tried_again() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let origin = sb.dir.join("origin.git");
    git(&sb.dir, &["init", "-q", "--bare", &text(&origin)]);
    git(&app, &["remote", "add", "origin", &text(&origin)]);
    git(&app, &["push", "-q", "origin", "main"]);
    let mut c = sb.client();
    session(&mut c);
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/slow".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt = app.parent().unwrap().join("app-wt/feat-slow");
    std::fs::write(wt.join("a"), "a").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "a"]);
    // the hook of the repository the worktree belongs to runs on every push, and waits
    let hooks = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .unwrap();
    let hooks = Path::new(String::from_utf8_lossy(&hooks.stdout).trim()).join("hooks");
    let hooks = if hooks.is_absolute() {
        hooks
    } else {
        wt.join(hooks)
    };
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-push");
    std::fs::write(&hook, "#!/bin/sh\nsleep 60\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let began = std::time::Instant::now();
    let socket = sb.client();
    let waiting = ws.clone();
    let push = std::thread::spawn(move || {
        let mut c = socket;
        c.cmd(Cmd::SyncWorkspace { workspace: waiting })
    });
    // asked twice while it runs: the second is refused, and cancel reaches the first
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let mut other = sb.client();
    let again = other
        .cmd(Cmd::SyncWorkspace {
            workspace: ws.clone(),
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::Conflict, "{}", again.message);
    // everybody sees the push as a job that can be stopped (REQ-652)
    session(&mut other);
    let jobs = wait_jobs(&mut other, |j| !j.is_empty());
    assert_eq!(
        (
            jobs[0]["kind"].as_str(),
            jobs[0]["state"].as_str(),
            jobs[0]["name"].as_str(),
            jobs[0]["cancellable"].as_bool()
        ),
        (
            Some("sync_workspace"),
            Some("running"),
            Some("feat/slow"),
            Some(true)
        )
    );
    other
        .cmd(Cmd::Cancel {
            workspace: ws.clone(),
        })
        .unwrap()
        .unwrap();
    let stopped = push.join().unwrap().unwrap().unwrap_err();
    assert_eq!(stopped.code, ErrorCode::GitFailed);
    assert_eq!(stopped.message, "cancelado");
    let jobs = wait_jobs(&mut other, |j| j.iter().all(|j| j["state"] != "running"));
    assert_eq!(
        (jobs[0]["state"].as_str(), jobs[0]["detail"].as_str()),
        (Some("failed"), Some("cancelado"))
    );
    assert!(
        began.elapsed() < std::time::Duration::from_secs(30),
        "the hook was not waited for"
    );
    // nothing runs now: cancel answers the same, and a new push is not refused
    other
        .cmd(Cmd::Cancel {
            workspace: ws.clone(),
        })
        .unwrap()
        .unwrap();
    std::fs::remove_file(&hook).unwrap();
    let pushed = other
        .cmd(Cmd::SyncWorkspace { workspace: ws })
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(pushed["pushed"].as_u64(), Some(1), "{pushed}");
    d.stop();
}

/// REQ-281 to REQ-285: a worktree with changes is removed only by force, and its branch stays.
#[test]
fn a_worktree_with_changes_goes_only_by_force_and_its_branch_stays() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    session(&mut c);
    let ws = id_of(
        c.cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/x".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap(),
    );
    let wt = app.parent().unwrap().join("app-wt/feat-x");
    std::fs::write(wt.join("unsaved"), "x").unwrap();
    let remove = |c: &mut wire::Client, remove_worktree: bool, force: bool| {
        c.cmd(Cmd::RemoveWorkspace {
            workspace: ws.clone(),
            remove_worktree,
            force,
        })
        .unwrap()
    };
    // Without force git refuses, as a job that fails, and the workspace stays.
    let job = job_of(remove(&mut c, true, false).unwrap());
    wait_jobs(&mut c, |j| {
        j.iter()
            .any(|j| j["id"] == job.as_str() && j["state"] == "failed")
    });
    assert!(wt.exists());
    // A job that ended is forgotten by the person: one, then all (REQ-653).
    c.cmd(Cmd::DismissJob { job: job.clone() })
        .unwrap()
        .unwrap();
    wait_jobs(&mut c, |j| j.is_empty());
    assert_eq!(
        c.cmd(Cmd::DismissJob { job }).unwrap().unwrap_err().code,
        ErrorCode::NotFound
    );
    c.cmd(Cmd::DismissJobs {}).unwrap().unwrap();
    // Force is for removing a worktree: on its own it is a bad request and nothing happens.
    assert_eq!(
        remove(&mut c, false, true).unwrap_err().code,
        ErrorCode::BadRequest
    );
    assert!(wt.exists());
    // By force the folder goes with its changes, the workspace goes, the branch stays.
    let job = job_of(remove(&mut c, true, true).unwrap());
    wait_jobs(&mut c, |j| {
        j.iter()
            .any(|j| j["id"] == job.as_str() && j["state"] == "done")
    });
    assert!(!wt.exists());
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&app)
        .args(["branch", "--list", "feat/x"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("feat/x"));
    assert_eq!(
        remove(&mut c, true, false).unwrap_err().code,
        ErrorCode::NotFound
    );
    d.stop();
}

/// The worktrees git has that jwn did not make: a branch, a detached head, one with a change and
/// one whose folder is gone.
fn existing(app: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let wt = app.parent().unwrap().join("app-wt");
    std::fs::create_dir_all(&wt).unwrap();
    let (login, bisect, gone) = (wt.join("fix-login"), wt.join("bisect"), wt.join("old"));
    git(
        app,
        &["worktree", "add", "-q", "-b", "fix/login", &text(&login)],
    );
    git(app, &["worktree", "add", "-q", "--detach", &text(&bisect)]);
    git(app, &["worktree", "add", "-q", "-b", "old", &text(&gone)]);
    std::fs::remove_dir_all(&gone).unwrap();
    std::fs::write(login.join("new.txt"), "x").unwrap();
    std::fs::write(login.join("f.txt"), "changed").unwrap();
    (
        std::fs::canonicalize(login).unwrap(),
        std::fs::canonicalize(bisect).unwrap(),
        gone,
    )
}

fn listed(c: &mut wire::Client, repo: &Path) -> Vec<Value> {
    let r = c
        .cmd(Cmd::ListWorktrees { repo: text(repo) })
        .unwrap()
        .unwrap()
        .unwrap();
    r["worktrees"].as_array().unwrap().clone()
}

/// REQ-533: the worktrees git lists but the main one, from the repository or from one of them.
#[test]
fn the_worktrees_git_has_are_listed_with_their_branch_changes_and_whether_their_folder_is_there() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    assert!(
        listed(&mut c, &app).is_empty(),
        "the main worktree is the repository's own"
    );
    let (login, bisect, gone) = existing(&app);

    let list = listed(&mut c, &app);
    assert_eq!(list.len(), 3, "{list:?}");
    let by = |path: &Path| {
        list.iter()
            .find(|w| Path::new(w["path"].as_str().unwrap()).ends_with(path.file_name().unwrap()))
            .unwrap_or_else(|| panic!("{} is not listed: {list:?}", path.display()))
    };
    let (l, b, g) = (by(&login), by(&bisect), by(&gone));
    assert_eq!(l["branch"], "fix/login", "the prefix is kept");
    assert_eq!(l["changes"], 2, "a changed file and a new one");
    assert_eq!(l["head"].as_str().unwrap().len(), 7);
    assert!(l.get("missing").is_none() && l.get("workspace").is_none());
    assert!(b.get("branch").is_none(), "a detached head has no branch");
    assert_eq!(b["changes"], 0);
    assert_eq!(g["missing"], true);
    assert!(
        g.get("changes").is_none(),
        "nothing to read in a folder that is not there"
    );

    assert_eq!(
        listed(&mut c, &login).len(),
        3,
        "from a worktree it is the same list, and the main one is still left out"
    );
    let plain = std::fs::canonicalize(&sb.dir).unwrap();
    let e = c
        .cmd(Cmd::ListWorktrees { repo: text(&plain) })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed, "not a repository");
    d.stop();
}

/// REQ-534 and REQ-535: opening makes a workspace with no panel, says it is open in the list,
/// and refuses what is not a worktree of the repository, is not there or is open already.
#[test]
fn an_existing_worktree_opens_as_a_workspace_with_no_panel_and_the_list_then_says_so() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let (login, bisect, gone) = existing(&app);
    let mut c = sb.client();

    let reply = c
        .cmd(Cmd::OpenWorktree {
            repo: text(&app),
            path: text(&login),
        })
        .unwrap()
        .unwrap()
        .unwrap();
    let ws = reply["workspace"].as_str().unwrap().to_owned();
    c.open(&ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&ws)).unwrap());
    let n = &c.nodes[&ws];
    assert_eq!(
        (n["kind"].as_str(), n["branch"].as_str(), n["name"].as_str()),
        (Some("worktree"), Some("fix/login"), Some("app: fix/login"))
    );
    assert_eq!(n["project"], text(&app));
    assert_eq!(n["path"], text(&login));
    assert_eq!(
        n["panes"].as_array().map(Vec::len),
        Some(0),
        "no panel until it is started"
    );
    assert!(
        listed(&mut c, &app)
            .iter()
            .any(|w| w["workspace"] == ws.as_str()),
        "the list says which workspace has it"
    );

    let refused = |c: &mut wire::Client, path: &Path| {
        c.cmd(Cmd::OpenWorktree {
            repo: text(&app),
            path: text(path),
        })
        .unwrap()
        .unwrap_err()
        .code
    };
    assert_eq!(refused(&mut c, &login), ErrorCode::Conflict, "open already");
    assert_eq!(
        refused(&mut c, &gone),
        ErrorCode::NotFound,
        "its folder is gone"
    );
    assert_eq!(
        refused(&mut c, &app),
        ErrorCode::NotFound,
        "the main worktree is the repository's workspace"
    );
    assert_eq!(
        refused(&mut c, &sb.dir.join("work")),
        ErrorCode::NotFound,
        "a folder git does not list"
    );
    // A detached head opens too, named by its commit.
    let reply = c
        .cmd(Cmd::OpenWorktree {
            repo: text(&login),
            path: text(&bisect),
        })
        .unwrap()
        .unwrap()
        .unwrap();
    let detached = reply["workspace"].as_str().unwrap().to_owned();
    c.open(&detached).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes.contains_key(&detached))
            .unwrap()
    );
    assert!(c.nodes[&detached]["branch"].is_null());
    assert!(
        c.nodes[&detached]["name"]
            .as_str()
            .unwrap()
            .starts_with("app: (")
    );
    d.stop();
}
