//! N76: the files of a workspace, folder by folder, with what git says of them, and a search by name.

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, git, text};
use contract::{Cmd, ErrorCode, FileKind, FileListing, FoundFiles, GitMark};

/// A repository with `src/auth/{login,token}.rs`, `src/main.rs`, `docs/README.md`, `.gitignore`
/// (`target/`), committed; then `login.rs` changed, `docs/new.md` not tracked and `target/` made.
fn repo(sb: &Sandbox) -> PathBuf {
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("app");
    std::fs::create_dir_all(dir.join("src/auth")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    for (path, body) in [
        ("src/auth/login.rs", "a"),
        ("src/auth/token.rs", "b"),
        ("src/main.rs", "c"),
        ("docs/README.md", "d"),
        (".gitignore", "target/\n"),
    ] {
        std::fs::write(dir.join(path), body).unwrap();
    }
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("src/auth/login.rs"), "changed").unwrap();
    std::fs::write(dir.join("docs/new.md"), "n").unwrap();
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::write(dir.join("target/debug/app"), "bin").unwrap();
    dir
}

fn open(c: &mut wire::Client, dir: &Path) -> String {
    c.cmd(Cmd::OpenFolder { path: text(dir) })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn list(c: &mut wire::Client, ws: &str, path: Option<&str>) -> Result<FileListing, ErrorCode> {
    match c
        .cmd(Cmd::ListFiles {
            workspace: ws.into(),
            path: path.map(Into::into),
        })
        .unwrap()
    {
        Ok(v) => Ok(serde_json::from_value(v.unwrap()).unwrap()),
        Err(e) => Err(e.code),
    }
}

fn names(l: &FileListing) -> Vec<&str> {
    l.entries.iter().map(|e| e.name.as_str()).collect()
}

fn entry<'a>(l: &'a FileListing, name: &str) -> &'a contract::FileEntry {
    l.entries.iter().find(|e| e.name == name).unwrap()
}

/// REQ-309 to REQ-312.
#[test]
fn a_folder_is_listed_with_folders_first_what_git_says_of_each_and_no_dot_git() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open(&mut c, &app);

    let top = list(&mut c, &ws, None).unwrap();
    assert_eq!(top.path, "");
    assert_eq!(
        names(&top),
        ["docs", "src", "target", ".gitignore"],
        "folders first, each by name, `.git` left out"
    );
    assert_eq!(top.total, 4);
    assert_eq!(
        entry(&top, "src").git,
        Some(GitMark::Modified),
        "something under it changed"
    );
    assert_eq!(entry(&top, "docs").git, Some(GitMark::Untracked));
    assert!(entry(&top, "target").ignored && entry(&top, "target").git.is_none());
    assert_eq!(entry(&top, ".gitignore").kind, FileKind::File);
    assert_eq!(entry(&top, ".gitignore").size, Some(8));
    assert!(!entry(&top, "src").ignored);

    let auth = list(&mut c, &ws, Some("src/auth")).unwrap();
    assert_eq!(auth.path, "src/auth");
    assert_eq!(names(&auth), ["login.rs", "token.rs"]);
    assert_eq!(entry(&auth, "login.rs").git, Some(GitMark::Modified));
    assert_eq!(entry(&auth, "token.rs").git, None);
    assert_eq!(entry(&auth, "login.rs").size, Some(7));

    let docs = list(&mut c, &ws, Some("./docs/")).unwrap();
    assert_eq!(
        names(&docs),
        ["new.md", "README.md"],
        "by name whatever the case"
    );
    assert_eq!(entry(&docs, "new.md").git, Some(GitMark::Untracked));
    assert_eq!(entry(&docs, "README.md").git, None);
    d.stop();
}

#[test]
fn a_folder_that_is_not_a_repository_has_no_marks_and_a_link_is_not_followed() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("notes");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    std::os::unix::fs::symlink("/etc", dir.join("etc-link")).unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);
    let l = list(&mut c, &ws, None).unwrap();
    assert_eq!(names(&l), ["sub", "a.txt", "etc-link"]);
    assert!(l.entries.iter().all(|e| e.git.is_none() && !e.ignored));
    assert_eq!(entry(&l, "etc-link").kind, FileKind::Link);
    // Listing through the link would leave the workspace.
    assert_eq!(
        list(&mut c, &ws, Some("etc-link")).unwrap_err(),
        ErrorCode::BadRequest
    );
    d.stop();
}

#[test]
fn what_cannot_be_listed_is_refused_with_why() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open(&mut c, &app);
    assert_eq!(
        list(&mut c, &ws, Some("..")).unwrap_err(),
        ErrorCode::BadRequest
    );
    assert_eq!(
        list(&mut c, &ws, Some("src/../../x")).unwrap_err(),
        ErrorCode::BadRequest
    );
    assert_eq!(
        list(&mut c, &ws, Some("/etc")).unwrap_err(),
        ErrorCode::BadRequest
    );
    assert_eq!(
        list(&mut c, &ws, Some("nowhere")).unwrap_err(),
        ErrorCode::NotFound
    );
    assert_eq!(
        list(&mut c, &ws, Some("src/main.rs")).unwrap_err(),
        ErrorCode::BadRequest,
        "a file is not a folder"
    );
    assert_eq!(list(&mut c, "nope", None).unwrap_err(), ErrorCode::NotFound);
    d.stop();
}

#[test]
fn a_folder_with_more_than_two_thousand_entries_is_cut_and_says_how_many_there_are() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("big");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..2005 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), "").unwrap();
    }
    let mut c = sb.client();
    let ws = open(&mut c, &dir);
    let l = list(&mut c, &ws, None).unwrap();
    assert_eq!((l.entries.len(), l.total), (2000, 2005));
    assert_eq!(l.entries[0].name, "f0000.txt");
    d.stop();
}

fn found(c: &mut wire::Client, ws: &str, q: &str) -> FoundFiles {
    serde_json::from_value(
        c.cmd(Cmd::FindFiles {
            workspace: ws.into(),
            query: q.into(),
        })
        .unwrap()
        .unwrap()
        .unwrap(),
    )
    .unwrap()
}

/// REQ-313: a search over the whole workspace, what git ignores left out.
#[test]
fn a_search_finds_the_tracked_and_the_untracked_files_and_not_the_ignored_ones() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb);
    let mut c = sb.client();
    let ws = open(&mut c, &app);
    assert_eq!(found(&mut c, &ws, "tok").paths, ["src/auth/token.rs"]);
    assert_eq!(
        found(&mut c, &ws, "NEW").paths,
        ["docs/new.md"],
        "any case, not tracked yet"
    );
    assert!(
        found(&mut c, &ws, "app").paths.is_empty(),
        "target/debug/app is ignored"
    );
    let md = found(&mut c, &ws, "md").paths;
    assert_eq!(md, ["docs/new.md", "docs/README.md"]);
    assert!(found(&mut c, &ws, "  ").paths.is_empty());
    let e = c
        .cmd(Cmd::FindFiles {
            workspace: ws.clone(),
            query: "x".repeat(201),
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    d.stop();
}

#[test]
fn a_search_in_a_folder_that_is_not_a_repository_walks_it_and_skips_what_is_not_source() {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("plain");
    std::fs::create_dir_all(dir.join("a/node_modules/x")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join("a/found.txt"), "").unwrap();
    std::fs::write(dir.join("a/node_modules/x/found.js"), "").unwrap();
    std::fs::write(dir.join(".hidden/found.cfg"), "").unwrap();
    let mut c = sb.client();
    let ws = open(&mut c, &dir);
    assert_eq!(found(&mut c, &ws, "found").paths, ["a/found.txt"]);
    d.stop();
}
