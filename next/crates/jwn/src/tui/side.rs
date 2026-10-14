//! The rows of the sidebar (prototype: `docs/prototypes/tui-levels.html`). The daemon sends the
//! workspaces with each repository's root and worktrees together and the folders that are not
//! repositories last (REQ-219); the sidebar adds the headings between them.

use contract::{WorkspaceKind, WorkspaceSummary};

/// A row of the sidebar: a heading, which is never highlighted, or the workspace at an index.
#[derive(Debug, PartialEq, Eq)]
pub enum Side {
    Head(String),
    Workspace(usize),
}

/// The last folder of a path.
fn base(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

/// `list` as rows: the name of a repository above its worktrees when its root is not what comes
/// before them (REQ-220), and `carpetas` above the folders that are not repositories when
/// something comes before them (REQ-221).
pub fn rows(list: &[WorkspaceSummary]) -> Vec<Side> {
    let mut rows = Vec::new();
    let mut loose = false;
    for (i, w) in list.iter().enumerate() {
        let before = i.checked_sub(1).map(|b| &list[b]);
        match (w.kind, &w.project) {
            (WorkspaceKind::Folder, None) if !loose => {
                loose = true;
                if before.is_some() {
                    rows.push(Side::Head("carpetas".into()));
                }
            }
            (WorkspaceKind::Worktree, Some(project))
                if before.is_none_or(|b| b.project.as_ref() != Some(project)) =>
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
        }
    }

    fn heads(rows: &[Side]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Side::Head(h) => format!("# {h}"),
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
            heads(&rows(&list)),
            ["0", "1", "# tools", "2", "# carpetas", "3", "4"]
        );
    }

    #[test]
    fn folders_alone_have_no_heading() {
        let list = [ws(Folder, None), ws(Folder, None)];
        assert_eq!(heads(&rows(&list)), ["0", "1"]);
        assert!(rows(&[]).is_empty());
    }

    #[test]
    fn two_repositories_without_roots_are_each_named() {
        let list = [
            ws(Worktree, Some("/r/a")),
            ws(Worktree, Some("/r/a")),
            ws(Worktree, Some("/r/b")),
        ];
        assert_eq!(heads(&rows(&list)), ["# a", "0", "1", "# b", "2"]);
    }
}
