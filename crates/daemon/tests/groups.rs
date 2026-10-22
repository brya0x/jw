//! N161a: groups of workspaces (REQ-662 to REQ-668), against real git in a temp dir.

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, ErrorCode};
use serde_json::Value;

/// A repository with one commit, at `<sandbox>/work/<name>`.
fn repo(sb: &Sandbox, name: &str) -> PathBuf {
    let dir = sb.dir.join("work").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap()
}

fn plain(sb: &Sandbox, name: &str) -> PathBuf {
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn session(c: &mut wire::Client) -> Value {
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    c.nodes[&root].clone()
}

fn open(c: &mut wire::Client, path: &Path) -> String {
    c.cmd(Cmd::OpenFolder { path: text(path) })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn make_group(c: &mut wire::Client, name: &str) -> String {
    c.cmd(Cmd::CreateGroup { name: name.into() })
        .unwrap()
        .unwrap()
        .unwrap()["group"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What the session says: the names of its groups, and for each workspace (in the order listed)
/// its path's last part and the name of its group or `-`.
fn shape(c: &mut wire::Client) -> (Vec<String>, Vec<(String, String)>) {
    let s = session(c);
    let groups: Vec<(String, String)> = s["groups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|g| {
            (
                g["id"].as_str().unwrap().to_owned(),
                g["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let name_of = |id: Option<&str>| {
        id.map_or("-".to_owned(), |id| {
            groups.iter().find(|(g, _)| g == id).unwrap().1.clone()
        })
    };
    let ws = s["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                Path::new(w["path"].as_str().unwrap())
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                name_of(w["group"].as_str()),
            )
        })
        .collect();
    (groups.into_iter().map(|(_, n)| n).collect(), ws)
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

#[test]
fn a_session_with_no_group_says_none_and_a_group_is_made_renamed_ordered_and_deleted_with_its_workspaces_open()
 {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let (a, b) = (plain(&sb, "a"), plain(&sb, "b"));
    let (wa, wb) = (open(&mut c, &a), open(&mut c, &b));
    let s = session(&mut c);
    assert!(s.get("groups").is_none(), "absent when empty (REQ-662)");
    assert!(s["workspaces"][0].get("group").is_none());

    let sales = make_group(&mut c, "  salesassist ");
    let ideas = make_group(&mut c, "ideas");
    let (names, _) = shape(&mut c);
    assert_eq!(
        names,
        ["salesassist", "ideas"],
        "trimmed, in the order made"
    );

    c.cmd(Cmd::MoveToGroup {
        workspace: wb.clone(),
        group: Some(ideas.clone()),
    })
    .unwrap()
    .unwrap();
    c.cmd(Cmd::MoveToGroup {
        workspace: wa.clone(),
        group: Some(sales.clone()),
    })
    .unwrap()
    .unwrap();
    // the groups come in their order, whatever order the workspaces were opened in (REQ-668)
    assert_eq!(
        shape(&mut c).1,
        [pair("a", "salesassist"), pair("b", "ideas")]
    );
    c.cmd(Cmd::MoveGroup {
        group: ideas.clone(),
        delta: -1,
    })
    .unwrap()
    .unwrap();
    let (names, ws) = shape(&mut c);
    assert_eq!(names, ["ideas", "salesassist"]);
    assert_eq!(ws, [pair("b", "ideas"), pair("a", "salesassist")]);
    // at the ends nothing changes and it is ok (REQ-667)
    for (group, delta) in [(&ideas, -1), (&sales, 1)] {
        c.cmd(Cmd::MoveGroup {
            group: group.clone(),
            delta,
        })
        .unwrap()
        .unwrap();
    }
    assert_eq!(shape(&mut c).0, ["ideas", "salesassist"]);

    c.cmd(Cmd::RenameGroup {
        group: sales.clone(),
        name: "Sales Assist".into(),
    })
    .unwrap()
    .unwrap();
    assert_eq!(shape(&mut c).0, ["ideas", "Sales Assist"]);

    // deleting releases the workspaces and closes nothing (REQ-665)
    c.cmd(Cmd::DeleteGroup {
        group: sales.clone(),
    })
    .unwrap()
    .unwrap();
    let (names, ws) = shape(&mut c);
    assert_eq!(names, ["ideas"]);
    assert_eq!(ws, [pair("b", "ideas"), pair("a", "-")]);
    // moving to none leaves the last group too
    c.cmd(Cmd::MoveToGroup {
        workspace: wb,
        group: None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(shape(&mut c).1, [pair("a", "-"), pair("b", "-")]);
    drop(c);
    d.stop();
}

#[test]
fn a_repositorys_workspaces_move_together_and_a_workspace_opened_again_and_a_restart_find_the_group()
 {
    let sb = Sandbox::new();
    let d = sb.start();
    let (app, tools) = (repo(&sb, "app"), repo(&sb, "tools"));
    let notes = plain(&sb, "notes");
    let mut c = sb.client();
    let root = open(&mut c, &app);
    open(&mut c, &tools);
    open(&mut c, &notes);
    let wt = c
        .cmd(Cmd::CreateWorktree {
            repo: text(&app),
            branch: "feat/x".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let g = make_group(&mut c, "product");
    // moving the worktree moves the repository: its root goes too, the others stay (REQ-666)
    c.cmd(Cmd::MoveToGroup {
        workspace: wt,
        group: Some(g.clone()),
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        shape(&mut c).1,
        [
            pair("app", "product"),
            pair("feat-x", "product"),
            pair("tools", "-"),
            pair("notes", "-"),
        ]
    );
    // closing the root and opening it again: the group was kept under the repository (REQ-663)
    c.cmd(Cmd::RemoveWorkspace {
        workspace: root,
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();
    open(&mut c, &app);
    assert_eq!(shape(&mut c).1[0], pair("app", "product"));
    // a folder that is no repository is its own key
    let s = session(&mut c);
    let loose = s["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["path"] == text(&notes).as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.cmd(Cmd::MoveToGroup {
        workspace: loose,
        group: Some(g),
    })
    .unwrap()
    .unwrap();
    drop(c);
    d.stop();

    let d = sb.start();
    let mut c = sb.client();
    let (names, ws) = shape(&mut c);
    assert_eq!(names, ["product"], "a restart keeps the groups");
    assert_eq!(
        ws.iter().map(|(_, g)| g.as_str()).collect::<Vec<_>>(),
        ["product", "product", "product", "-"],
        "the repository, its worktree and the folder, then the one in none"
    );
    // the file says it too, and an old daemon would have ignored it (RISK-82)
    let saved = std::fs::read_to_string(sb.dir.join("state/session.json")).unwrap();
    assert!(saved.contains("\"groups\"") && saved.contains("\"grouped\""));
    d.stop();
}

#[test]
fn a_name_that_is_empty_long_or_taken_and_an_unknown_group_or_workspace_change_nothing() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let w = open(&mut c, &plain(&sb, "a"));
    let g = make_group(&mut c, "Product");
    let code = |c: &mut wire::Client, cmd: Cmd| c.cmd(cmd).unwrap().unwrap_err().code;
    assert_eq!(
        code(&mut c, Cmd::CreateGroup { name: "  ".into() }),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::CreateGroup {
                name: "x".repeat(41)
            }
        ),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::CreateGroup {
                name: "pRODUCT".into()
            }
        ),
        ErrorCode::Conflict
    );
    let other = make_group(&mut c, "x".repeat(40).as_str());
    assert_eq!(
        code(
            &mut c,
            Cmd::RenameGroup {
                group: other,
                name: "product".into()
            }
        ),
        ErrorCode::Conflict
    );
    c.cmd(Cmd::RenameGroup {
        group: g.clone(),
        name: "PRODUCT".into(),
    })
    .unwrap()
    .unwrap();
    let nobody = "00000000-0000-4000-8000-0000000000ff".to_owned();
    assert_eq!(
        code(
            &mut c,
            Cmd::RenameGroup {
                group: nobody.clone(),
                name: "z".into()
            }
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::DeleteGroup {
                group: nobody.clone()
            }
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::MoveToGroup {
                workspace: w.clone(),
                group: Some(nobody.clone())
            }
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::MoveToGroup {
                workspace: nobody.clone(),
                group: Some(g.clone())
            }
        ),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::MoveGroup {
                group: g.clone(),
                delta: 2
            }
        ),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(
            &mut c,
            Cmd::MoveGroup {
                group: nobody,
                delta: 1
            }
        ),
        ErrorCode::NotFound
    );
    let (names, ws) = shape(&mut c);
    assert_eq!(names, ["PRODUCT", &"x".repeat(40)]);
    assert_eq!(ws, [pair("a", "-")]);
    d.stop();
}
