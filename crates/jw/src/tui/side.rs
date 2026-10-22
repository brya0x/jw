//! The rows of the sidebar (prototypes: `docs/prototypes/tui-levels.html`, and `groups.html` for
//! the groups). The daemon sends the workspaces with each group's together, in each group each
//! repository's root and worktrees together, and the folders that are not repositories last
//! (REQ-219, REQ-668); the sidebar adds the headings and the rows of the groups.

use std::collections::HashSet;

use contract::{Group, WorkspaceKind, WorkspaceSummary};

use super::base;

/// A row of the sidebar: a heading, which is never highlighted, the row of a group, `vacío`
/// under a group that holds nothing, or the workspace at an index.
#[derive(Debug, PartialEq, Eq)]
pub enum Side {
    Head(String),
    /// The heading of the workspaces that are in no group, WHERE a group exists (REQ-669).
    Ungrouped,
    /// The row of the group at an index into the session's groups.
    Group(usize),
    /// What an open group with no workspace says.
    Empty,
    Workspace(usize),
}

/// `list` as rows. With no group it is what REQ-219 to REQ-223 say. With groups, the row of each
/// group in its order and, unless it is in `folded`, under it its workspaces as without groups
/// (but with no `carpetas`) or `vacío`; last, under `sin grupo`, the workspaces of no group (or of
/// a group that is not listed).
pub fn rows(list: &[WorkspaceSummary], groups: &[Group], folded: &HashSet<String>) -> Vec<Side> {
    let all: Vec<usize> = (0..list.len()).collect();
    if groups.is_empty() {
        return rows_of(list, &all, true);
    }
    let mut rows = Vec::new();
    for (at, g) in groups.iter().enumerate() {
        rows.push(Side::Group(at));
        if folded.contains(&g.id) {
            continue;
        }
        let mine: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&i| list[i].group.as_deref() == Some(g.id.as_str()))
            .collect();
        if mine.is_empty() {
            rows.push(Side::Empty);
        } else {
            rows.extend(rows_of(list, &mine, false));
        }
    }
    let rest: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&i| {
            list[i]
                .group
                .as_ref()
                .is_none_or(|g| !groups.iter().any(|x| x.id == *g))
        })
        .collect();
    if !rest.is_empty() {
        rows.push(Side::Ungrouped);
        rows.extend(rows_of(list, &rest, false));
    }
    rows
}

/// The workspaces of `list` at `at`, in that order, as rows: the name of a repository above its
/// worktrees when its root is not what comes before them (REQ-220), and, when `carpetas`, that
/// word above the folders that are not repositories when something comes before them (REQ-221).
/// A folder is a root when it says so (`project` is its own path) or, for a daemon that does not
/// say it, when a worktree names it as its repository.
fn rows_of(list: &[WorkspaceSummary], at: &[usize], carpetas: bool) -> Vec<Side> {
    let repos: HashSet<&str> = list
        .iter()
        .filter(|w| w.kind == WorkspaceKind::Worktree)
        .filter_map(|w| w.project.as_deref())
        .collect();
    let root = |w: &WorkspaceSummary| {
        w.kind == WorkspaceKind::Folder
            && (w.project.as_deref() == Some(w.path.as_str()) || repos.contains(w.path.as_str()))
    };
    let mut rows = Vec::new();
    let mut loose = false;
    for (k, &i) in at.iter().enumerate() {
        let w = &list[i];
        let before = k.checked_sub(1).map(|b| &list[at[b]]);
        match (w.kind, &w.project) {
            (WorkspaceKind::Folder, None) if !loose && !root(w) => {
                loose = true;
                if carpetas && before.is_some() {
                    rows.push(Side::Head("carpetas".into()));
                }
            }
            (WorkspaceKind::Worktree, Some(project))
                if before.is_none_or(|b| {
                    b.project.as_ref() != Some(project) && !(root(b) && b.path == *project)
                }) =>
            {
                rows.push(Side::Head(base(project).into()));
            }
            _ => {}
        }
        rows.push(Side::Workspace(i));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use WorkspaceKind::{Folder, Worktree};

    fn ws(kind: WorkspaceKind, project: Option<&str>) -> WorkspaceSummary {
        WorkspaceSummary {
            id: String::new(),
            name: String::new(),
            kind,
            path: String::new(),
            project: project.map(Into::into),
            branch: None,
            pane_count: 0,
            attention: Vec::new(),
            review: None,
            working: false,
            removing: false,
            group: None,
        }
    }

    fn heads(rows: &[Side]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Side::Head(h) => format!("# {h}"),
                Side::Ungrouped => "sin grupo".into(),
                Side::Group(g) => format!("@{g}"),
                Side::Empty => "vacío".into(),
                Side::Workspace(i) => i.to_string(),
            })
            .collect()
    }

    #[test]
    fn a_root_and_its_worktrees_need_no_heading_a_rootless_repository_is_named_and_folders_come_under_carpetas()
     {
        let list = [
            ws(Folder, Some("/r/api")),
            ws(Worktree, Some("/r/api")),
            ws(Worktree, Some("/r/tools")),
            ws(Folder, None),
            ws(Folder, None),
        ];
        assert_eq!(
            heads(&rows(&list, &[], &HashSet::new())),
            ["0", "1", "# tools", "2", "# carpetas", "3", "4"]
        );
    }

    #[test]
    fn folders_alone_have_no_heading() {
        let list = [ws(Folder, None), ws(Folder, None)];
        assert_eq!(heads(&rows(&list, &[], &HashSet::new())), ["0", "1"]);
        assert!(rows(&[], &[], &HashSet::new()).is_empty());
    }

    #[test]
    fn two_repositories_without_roots_are_each_named() {
        let list = [
            ws(Worktree, Some("/r/a")),
            ws(Worktree, Some("/r/a")),
            ws(Worktree, Some("/r/b")),
        ];
        assert_eq!(
            heads(&rows(&list, &[], &HashSet::new())),
            ["# a", "0", "1", "# b", "2"]
        );
    }

    #[test]
    fn a_daemon_that_sends_no_project_for_a_root_is_understood_by_its_path() {
        let mut root = ws(Folder, None);
        root.path = "/r/loom".into();
        let list = [root, ws(Worktree, Some("/r/loom")), ws(Folder, None)];
        assert_eq!(
            heads(&rows(&list, &[], &HashSet::new())),
            ["0", "1", "# carpetas", "2"]
        );
    }

    fn group(id: &str) -> Group {
        Group {
            id: id.into(),
            name: id.into(),
        }
    }

    fn in_group(mut w: WorkspaceSummary, g: &str) -> WorkspaceSummary {
        w.group = Some(g.into());
        w
    }

    /// REQ-669: the rows of each group in order, `vacío` for one that holds nothing, `sin grupo`
    /// over what is in none, no `carpetas` among groups, and a repository's heading as ever.
    #[test]
    fn groups_come_in_order_with_their_workspaces_vacio_and_sin_grupo_last() {
        let list = [
            in_group(ws(Folder, Some("/r/api")), "g1"),
            in_group(ws(Worktree, Some("/r/api")), "g1"),
            in_group(ws(Worktree, Some("/r/tools")), "g2"),
            ws(Folder, None),
            ws(Folder, None),
        ];
        let groups = [group("g1"), group("g2"), group("g3")];
        assert_eq!(
            heads(&rows(&list, &groups, &HashSet::new())),
            [
                "@0",
                "0",
                "1",
                "@1",
                "# tools",
                "2",
                "@2",
                "vacío",
                "sin grupo",
                "3",
                "4"
            ]
        );
    }

    #[test]
    fn a_folded_group_shows_only_its_row_and_nothing_hides_the_workspaces_of_none() {
        let list = [
            in_group(ws(Folder, None), "g1"),
            in_group(ws(Folder, None), "g2"),
            ws(Folder, None),
        ];
        let groups = [group("g1"), group("g2")];
        let folded: HashSet<String> = ["g1".to_owned(), "g2".to_owned()].into();
        assert_eq!(
            heads(&rows(&list, &groups, &folded)),
            ["@0", "@1", "sin grupo", "2"]
        );
        // an empty group that is folded has no `vacío`
        assert_eq!(
            heads(&rows(&[], &[group("g1")], &["g1".to_owned()].into())),
            ["@0"]
        );
    }

    #[test]
    fn a_workspace_whose_group_is_not_listed_is_in_none_and_with_no_group_the_list_is_as_before() {
        let list = [in_group(ws(Folder, None), "gone"), ws(Folder, None)];
        assert_eq!(
            heads(&rows(&list, &[group("g1")], &HashSet::new())),
            ["@0", "vacío", "sin grupo", "0", "1"]
        );
        assert_eq!(
            heads(&rows(&list, &[], &HashSet::new())),
            ["0", "1"],
            "a workspace that names a group the session does not list is drawn as without groups"
        );
    }
}
