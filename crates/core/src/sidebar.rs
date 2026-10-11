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
            // Two projects can resolve to one folder (the same checkout under
            // two names): both lists are kept, not the last.
            Some(i) => {
                groups[i].1.extend(worktrees);
                groups[i].1.sort_by(|a, b| a.name.cmp(&b.name));
            }
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

/// Where `session`'s `folders.json` is, looked up without making or moving
/// anything: `session::dir` migrates the old files into `sessions/main/`,
/// which a read for a phone must not do. Before that migration has happened,
/// `main`'s folders are still where the old jw kept them.
fn folders_path(state: &Path, session: &str) -> std::path::PathBuf {
    let root = state.join("sessions");
    if session == session::MAIN && !root.exists() {
        return state.join("folders.json");
    }
    root.join(session).join("folders.json")
}

/// Every session's name, by name with `main` first, read without side
/// effects. `main` is there even before its directory is.
pub fn names(state: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(state.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != session::MAIN && session::valid(n))
        .collect();
    out.sort();
    out.insert(0, session::MAIN.to_string());
    out
}

/// The rows of `session` from the files under `state` and the registry at
/// `reg_path`.
pub fn load(state: &Path, reg_path: &Path, session: &str) -> Result<Vec<Row>> {
    let folders = folders::Folders::load(&folders_path(state, session))?;
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

/// The most workspaces, and sessions, a phone is told of in one answer.
pub const MAX_ITEMS: usize = 1000;
pub const MAX_SESSIONS: usize = 200;

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
    /// The rows `workspaces` lists for it: its folders, its worktrees and the
    /// closed roots shown above worktrees whose folder is not open.
    pub workspaces: usize,
}

/// Every session, `main` first and the rest by name, with the count of the
/// workspaces it lists. A session whose files cannot be read is left out, so
/// one corrupt `folders.json` does not fail the rest.
pub fn sessions(state: &Path, reg_path: &Path) -> Result<Vec<SessionItem>> {
    let reg = Registry::load(reg_path)?;
    Ok(names(state)
        .into_iter()
        .filter_map(|name| {
            let folders = folders::Folders::load(&folders_path(state, &name)).ok()?;
            let workspaces = rows(
                &folders,
                Registry {
                    entries: reg.entries.clone(),
                },
                &name,
            )
            .len();
            Some(SessionItem { name, workspaces })
        })
        .take(MAX_SESSIONS)
        .collect())
}

/// The workspace with id `id`, in any session: the ids carry their session
/// (`folders::id_in`), so one is the same workspace wherever it is asked.
pub fn find(state: &Path, reg_path: &Path, id: &str) -> Result<Option<Entry>> {
    let reg = Registry::load(reg_path)?;
    // A worktree is in the registry as it is: no git, no folders to read.
    if let Some(e) = reg.entries.iter().find(|e| e.id == id) {
        return Ok(Some(e.clone()));
    }
    // A folder's id says its directory and its session, so only that one
    // folder is looked at (one git probe), not every row of every session.
    if let Some(rest) = id.strip_prefix("dir:") {
        let (dir, session) = rest.rsplit_once('#').unwrap_or((rest, session::MAIN));
        if let Ok(folders) = folders::Folders::load(&folders_path(state, session))
            && let Some(f) = folders.folders.iter().find(|f| f.dir == dir)
        {
            return Ok(Some(folders::entry_for(f, session)));
        }
    }
    for name in names(state) {
        let Ok(folders) = folders::Folders::load(&folders_path(state, &name)) else {
            continue;
        };
        let reg = Registry {
            entries: reg.entries.clone(),
        };
        if let Some(row) = rows(&folders, reg, &name)
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
                    workspaces: 4
                },
                SessionItem {
                    name: "work".into(),
                    workspaces: 4
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
    fn two_projects_that_resolve_to_one_folder_keep_all_their_worktrees() {
        // A real checkout with an origin, so the folder is a root.
        let dir = std::env::temp_dir().join(format!("jw-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("jw");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["remote", "add", "origin", "https://example.com/x/jw.git"],
        ] {
            let ok = std::process::Command::new("git")
                .args(&args)
                .current_dir(&repo)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &dir)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        let path = repo.display().to_string();
        let fs = folders::Folders {
            folders: vec![folder(&path, None)],
        };
        // One worktree by its project's name, one by the checkout's path under
        // another project name: both land in the one group.
        let mut by_name = wt("a1", "one", "jw", "", 0);
        by_name.root = String::new();
        let mut by_path = wt("b1", "two", "alias", "", 0);
        by_path.root = path.clone();
        let reg = Registry {
            entries: vec![by_name, by_path],
        };
        let all = rows(&fs, reg, "main");
        let _ = std::fs::remove_dir_all(&dir);
        let shape: Vec<(&str, bool)> = all
            .iter()
            .map(|r| (r.entry.name.as_str(), r.child))
            .collect();
        assert_eq!(
            shape,
            [("jw", false), ("one", true), ("two", true)],
            "{shape:?}"
        );
    }

    #[test]
    fn reading_never_makes_or_moves_anything_and_main_is_found_before_the_migration() {
        let dir = std::env::temp_dir().join(format!("jw-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The old layout: folders.json at the top of the state dir.
        folders::Folders {
            folders: vec![folder("/a", None)],
        }
        .save(&dir.join("folders.json"))
        .unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry::default().save(&reg_path).unwrap();
        let got = sessions(&dir, &reg_path).unwrap();
        assert_eq!(
            got,
            [SessionItem {
                name: "main".into(),
                workspaces: 1
            }]
        );
        assert_eq!(load(&dir, &reg_path, "main").unwrap().len(), 1);
        assert!(find(&dir, &reg_path, "dir:/a").unwrap().is_some());
        // Nothing was made, and the old file was not moved.
        assert!(!dir.join("sessions").exists(), "a read made sessions/");
        assert!(
            dir.join("folders.json").exists(),
            "a read moved folders.json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_corrupt_session_is_skipped_not_fatal() {
        let dir = std::env::temp_dir().join(format!("jw-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("sessions").join("main");
        let bad = dir.join("sessions").join("bad");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        folders::Folders {
            folders: vec![folder("/a", None)],
        }
        .save(&main.join("folders.json"))
        .unwrap();
        std::fs::write(bad.join("folders.json"), "{ not json").unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry::default().save(&reg_path).unwrap();
        let got = sessions(&dir, &reg_path).unwrap();
        assert_eq!(
            got,
            [SessionItem {
                name: "main".into(),
                workspaces: 1
            }]
        );
        // Asking about it is an error for that session only; `find` goes on.
        assert!(load(&dir, &reg_path, "bad").is_err());
        assert!(find(&dir, &reg_path, "dir:/a").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_of_only_closed_roots_counts_what_it_lists() {
        let dir = std::env::temp_dir().join(format!("jw-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions").join("main")).unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry {
            entries: vec![wt("w", "only", "jw", "", 0)],
        }
        .save(&reg_path)
        .unwrap();
        let listed = load(&dir, &reg_path, "main").unwrap().len();
        assert_eq!(listed, 2, "a closed root and its worktree");
        assert_eq!(sessions(&dir, &reg_path).unwrap()[0].workspaces, listed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_is_found_by_its_id_without_reading_every_row() {
        let dir = std::env::temp_dir().join(format!("jw-fast-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join("sessions").join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(dir.join("sessions").join("main")).unwrap();
        folders::Folders {
            folders: vec![folder("/nowhere/a", Some("A"))],
        }
        .save(&work.join("folders.json"))
        .unwrap();
        // The folder of session `work` by its id, though `main` has none; the
        // other sessions' files are never needed (a corrupt one is no matter).
        std::fs::write(
            dir.join("sessions").join("main").join("folders.json"),
            "{ bad",
        )
        .unwrap();
        let reg_path = dir.join("workspaces.json");
        Registry {
            entries: vec![wt("w1", "w", "p", "work", 0)],
        }
        .save(&reg_path)
        .unwrap();
        let got = find(&dir, &reg_path, "dir:/nowhere/a#work")
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.name.as_str(), got.id.as_str()),
            ("A", "dir:/nowhere/a#work")
        );
        assert_eq!(find(&dir, &reg_path, "w1").unwrap().unwrap().name, "w");
        assert!(
            find(&dir, &reg_path, "dir:/nowhere/zzz#work")
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_valid_session_name_is_known() {
        assert!(known("main") && known("work-2"));
        assert!(!known("../etc") && !known("") && !known("New"));
    }
}
