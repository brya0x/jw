//! A session's workspaces as its sidebar groups them, and what the phone is
//! told of them (docs/specs/mobile.md, REQ-12): the TUI and the bridge's
//! `workspaces` answer start from the same rows, so they never disagree on
//! what is a root, a worktree or a folder.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::core::registry::{Entry, Registry};
use crate::{folders, session};

/// One row of the sidebar: a workspace, and whether it is a worktree
/// listed under its project (REQ-50).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub entry: Entry,
    pub child: bool,
}

/// The rows of `session`: each folder, with the worktrees of its project
/// under it; the worktrees whose project folder is not open come under that
/// folder shown closed.
pub fn rows(folders: &folders::Folders, reg: Registry, session: &str) -> Vec<Row> {
    let mut groups: Vec<(Entry, Vec<Entry>)> = folders
        .folders
        .iter()
        .map(|f| (folders::entry_for(f, session), Vec::new()))
        .collect();
    let mut by_project: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in reg
        .entries
        .into_iter()
        .filter(|e| session::owns(session, &e.session))
    {
        by_project.entry(e.project.clone()).or_default().push(e);
    }
    for (project, mut worktrees) in by_project {
        worktrees.sort_by(|a, b| a.name.cmp(&b.name));
        let root = worktrees
            .iter()
            .find(|w| !w.root.is_empty())
            .map(|w| w.root.clone());
        let at = groups.iter().position(|(r, _)| {
            !r.branch.is_empty() && (r.project == project || Some(&r.path) == root.as_ref())
        });
        match at {
            Some(i) => groups[i].1 = worktrees,
            None => {
                // Worktrees whose project folder isn't open: the folder
                // shows closed, above them.
                let mut e = match &root {
                    Some(dir) => folders::entry(dir, false),
                    None => Entry {
                        name: project.clone(),
                        ..Entry::default()
                    },
                };
                // The id of the session asked about, not of the process's own
                // (`folders::entry` and `id` use that one): a closed root of
                // `work` must not be the id of the same folder in `main`.
                e.id = folders::id_in(session, root.as_deref().unwrap_or(&project));
                e.project = project;
                groups.push((e, worktrees));
            }
        }
    }
    groups
        .into_iter()
        .flat_map(|(root, kids)| {
            std::iter::once(Row {
                entry: root,
                child: false,
            })
            .chain(kids.into_iter().map(|entry| Row { entry, child: true }))
        })
        .collect()
}

/// The rows of `session` from the files under `state` and the registry at
/// `reg_path`.
pub fn load(state: &Path, reg_path: &Path, session: &str) -> Result<Vec<Row>> {
    let folders = folders::Folders::load(&session::dir(state, session)?.join("folders.json"))?;
    Ok(rows(&folders, Registry::load(reg_path)?, session))
}

/// A workspace as the phone gets it: the item of the `workspaces` answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// The id the daemon's `PaneInfo.stream` carries.
    pub id: String,
    /// `root` (a git checkout, or a project whose folder is not open),
    /// `worktree` (under its root) or `folder` (a plain one).
    pub kind: &'static str,
    pub name: String,
    pub path: String,
    pub project: String,
    /// Empty for a plain folder.
    pub branch: String,
    /// The pull request number jw recorded; no live lookup.
    pub pr: Option<u64>,
    pub session: String,
    /// A worktree's root.
    pub parent: Option<String>,
}

/// The items of `rows`, in their order.
pub fn items(rows: &[Row], session: &str) -> Vec<Item> {
    let mut parent: Option<String> = None;
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let e = &row.entry;
        let kind = if row.child {
            "worktree"
        } else {
            parent = Some(e.id.clone());
            let has_children = rows.get(i + 1).is_some_and(|r| r.child);
            if has_children || !e.branch.is_empty() {
                "root"
            } else {
                "folder"
            }
        };
        out.push(Item {
            id: e.id.clone(),
            kind,
            name: e.name.clone(),
            path: e.path.clone(),
            project: e.project.clone(),
            branch: e.branch.clone(),
            pr: (e.pr > 0).then_some(e.pr),
            session: session.to_string(),
            parent: if row.child { parent.clone() } else { None },
        });
    }
    out
}

/// A session and how many workspaces it has.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionItem {
    pub name: String,
    pub workspaces: usize,
}

/// Every session, `main` first and the rest by name, with the count of its
/// folders and worktrees. `main` is there even before its directory is.
pub fn sessions(state: &Path, reg_path: &Path) -> Result<Vec<SessionItem>> {
    let reg = Registry::load(reg_path)?;
    let mut names = session::list(state).unwrap_or_default();
    names.retain(|n| n != session::MAIN);
    names.insert(0, session::MAIN.to_string());
    names
        .into_iter()
        .map(|name| {
            let dir = session::dir(state, &name)?.join("folders.json");
            let folders = folders::Folders::load(&dir)?.folders.len();
            let worktrees = reg
                .entries
                .iter()
                .filter(|e| session::owns(&name, &e.session))
                .count();
            Ok(SessionItem {
                name,
                workspaces: folders + worktrees,
            })
        })
        .collect()
}

/// The workspace with id `id`, in any session: the ids carry their session
/// (`folders::id_in`), so one is the same workspace wherever it is asked.
pub fn find(state: &Path, reg_path: &Path, id: &str) -> Result<Option<Entry>> {
    let mut names = session::list(state).unwrap_or_default();
    names.retain(|n| n != session::MAIN);
    names.insert(0, session::MAIN.to_string());
    for name in names {
        if let Some(row) = load(state, reg_path, &name)?
            .into_iter()
            .find(|r| r.entry.id == id)
        {
            return Ok(Some(row.entry));
        }
    }
    Ok(None)
}

/// Whether `name` is a session that can be asked about.
pub fn known(name: &str) -> bool {
    name == session::MAIN || session::valid(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folders::Folder;

    fn folder(dir: &str, name: Option<&str>) -> Folder {
        Folder {
            dir: dir.into(),
            opened: false,
            created: "2026-10-11T00:00:00Z".into(),
            agent: None,
            plain: false,
            name: name.map(str::to_string),
        }
    }

    fn wt(id: &str, name: &str, project: &str, session: &str, pr: u64) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            project: project.into(),
            branch: format!("feat/{name}"),
            path: format!("/wt/{project}/{name}"),
            pr,
            session: session.into(),
            root: format!("/nowhere/{project}"),
            ..Entry::default()
        }
    }

    #[test]
    fn worktrees_nest_under_their_root_and_a_plain_folder_stands_alone() {
        // No git here: neither folder is a checkout, so a root is a folder
        // with worktrees under it, and the rest are plain.
        let fs = folders::Folders {
            folders: vec![
                folder("/nowhere/jw", Some("jw")),
                folder("/nowhere/notes", None),
            ],
        };
        let reg = Registry {
            entries: vec![
                wt("w2", "beta", "jw", "", 0),
                wt("w1", "alpha", "jw", "", 42),
                wt("x1", "other", "jw", "work", 0),
            ],
        };
        let rows = rows(&fs, reg, "main");
        let got: Vec<(&str, bool)> = rows
            .iter()
            .map(|r| (r.entry.name.as_str(), r.child))
            .collect();
        // The worktrees of `jw` are not under a folder that is not a checkout:
        // the project's folder shows closed above them.
        assert_eq!(
            got,
            [
                ("jw", false),
                ("notes", false),
                ("jw", false),
                ("alpha", true),
                ("beta", true)
            ]
        );
        let items = items(&rows, "main");
        let kinds: Vec<&str> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, ["folder", "folder", "root", "worktree", "worktree"]);
        // The closed root is the parent of both worktrees.
        assert_eq!(items[3].parent, Some(items[2].id.clone()));
        assert_eq!(items[4].parent, Some(items[2].id.clone()));
        assert_eq!(items[0].parent, None);
        assert_eq!(items[3].pr, Some(42));
        assert_eq!(items[4].pr, None);
        assert!(items.iter().all(|i| i.session == "main"));
        assert_eq!(items[3].branch, "feat/alpha");
        // The other session's worktree is not listed.
        assert!(!items.iter().any(|i| i.name == "other"));
    }

    #[test]
    fn a_closed_root_has_the_id_of_the_session_asked_about() {
        let reg = |session: &str| Registry {
            entries: vec![wt("x1", "other", "jw", session, 0)],
        };
        let none = folders::Folders::default();
        let work = rows(&none, reg("work"), "work");
        let items = items(&work, "work");
        assert_eq!(items[0].kind, "root");
        assert!(items[0].id.ends_with("#work"), "{}", items[0].id);
        assert_eq!(items[1].parent.as_deref(), Some(items[0].id.as_str()));
        // The same project in main has the id main's workspace has.
        let main = rows(&none, reg(""), "main");
        let main = super::items(&main, "main");
        assert!(!main[0].id.contains('#'), "{}", main[0].id);
        assert_ne!(main[0].id, items[0].id);
        // And without a known root folder, the project's name is the id's.
        let mut e = wt("x2", "n", "solo", "work", 0);
        e.root = String::new();
        let rows = rows(&none, Registry { entries: vec![e] }, "work");
        assert!(
            rows[0].entry.id.ends_with("solo#work"),
            "{}",
            rows[0].entry.id
        );
    }

    #[test]
    fn a_checkout_is_a_root_even_without_worktrees() {
        let mut e = Entry {
            id: "dir:/r".into(),
            name: "r".into(),
            branch: "main".into(),
            ..Entry::default()
        };
        let plain = Entry {
            id: "dir:/p".into(),
            name: "p".into(),
            ..Entry::default()
        };
        let rows = vec![
            Row {
                entry: e.clone(),
                child: false,
            },
            Row {
                entry: plain,
                child: false,
            },
        ];
        let items = items(&rows, "work");
        assert_eq!((items[0].kind, items[1].kind), ("root", "folder"));
        assert_eq!(items[0].session, "work");
        e.pr = 7;
        assert_eq!(
            super::items(
                &[Row {
                    entry: e,
                    child: false
                }],
                "main"
            )[0]
            .pr,
            Some(7)
        );
    }

    #[test]
    fn sessions_list_main_first_with_their_counts() {
        let dir = std::env::temp_dir().join(format!("jw-side-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("sessions").join("main");
        let work = dir.join("sessions").join("work");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        folders::Folders {
            folders: vec![folder("/a", None), folder("/b", None)],
        }
        .save(&main.join("folders.json"))
        .unwrap();
        folders::Folders {
            folders: vec![folder("/c", None)],
        }
        .save(&work.join("folders.json"))
        .unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry {
            entries: vec![
                wt("a", "a", "p", "", 0),
                wt("b", "b", "p", "work", 0),
                wt("c", "c", "p", "work", 0),
            ],
        }
        .save(&reg_path)
        .unwrap();
        let got = sessions(&dir, &reg_path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            got,
            [
                SessionItem {
                    name: "main".into(),
                    workspaces: 3
                },
                SessionItem {
                    name: "work".into(),
                    workspaces: 3
                },
            ]
        );
    }

    #[test]
    fn a_workspace_is_found_by_its_id_in_any_session() {
        let dir = std::env::temp_dir().join(format!("jw-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join("sessions").join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(dir.join("sessions").join("main")).unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry {
            entries: vec![wt("a", "a", "p", "", 0), wt("b", "b", "p", "work", 0)],
        }
        .save(&reg_path)
        .unwrap();
        assert_eq!(find(&dir, &reg_path, "a").unwrap().unwrap().name, "a");
        assert_eq!(find(&dir, &reg_path, "b").unwrap().unwrap().name, "b");
        assert!(find(&dir, &reg_path, "nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_valid_session_name_is_known() {
        assert!(known("main") && known("work-2"));
        assert!(!known("../etc") && !known("") && !known("New"));
    }
}
