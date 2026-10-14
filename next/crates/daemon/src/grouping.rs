//! The order of the session's workspaces: each repository's root and then its worktrees, then the
//! folders that are not repositories. Every client draws the list as it comes, so they agree.

use contract::{WorkspaceKind, WorkspaceSummary};

/// `list` with each repository's workspaces together (its root first, then its worktrees in the
/// order they were opened), the repositories in the order their first workspace was opened, and
/// the folders with no `project` last. A repository whose root is not open has only worktrees.
pub fn group(list: Vec<WorkspaceSummary>) -> Vec<WorkspaceSummary> {
    let mut repos: Vec<(String, Vec<WorkspaceSummary>)> = Vec::new();
    let mut loose = Vec::new();
    for w in list {
        let Some(project) = w.project.clone() else {
            loose.push(w);
            continue;
        };
        let at = match repos.iter().position(|(p, _)| *p == project) {
            Some(at) => at,
            None => {
                repos.push((project, Vec::new()));
                repos.len() - 1
            }
        };
        let members = &mut repos[at].1;
        if w.kind == WorkspaceKind::Folder {
            members.insert(0, w);
        } else {
            members.push(w);
        }
    }
    repos
        .into_iter()
        .flat_map(|(_, members)| members)
        .chain(loose)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str, kind: WorkspaceKind, project: Option<&str>) -> WorkspaceSummary {
        WorkspaceSummary {
            id: name.into(),
            name: name.into(),
            kind,
            path: format!("/w/{name}"),
            project: project.map(Into::into),
            branch: None,
            pane_count: 0,
            attention: Vec::new(),
        }
    }

    fn names(list: Vec<WorkspaceSummary>) -> Vec<String> {
        list.into_iter().map(|w| w.name).collect()
    }

    #[test]
    fn a_root_comes_before_its_worktrees_and_the_folders_that_are_not_repositories_come_last() {
        use WorkspaceKind::{Folder, Worktree};
        let list = vec![
            ws("notes", Folder, None),
            ws("api-login", Worktree, Some("/r/api")),
            ws("tools-cli", Worktree, Some("/r/tools")),
            ws("api", Folder, Some("/r/api")),
            ws("api-fix", Worktree, Some("/r/api")),
            ws("infra", Folder, None),
        ];
        // `tools` has no root open: only its worktree. `api` was first opened by a worktree, so it leads.
        assert_eq!(
            names(group(list)),
            ["api", "api-login", "api-fix", "tools-cli", "notes", "infra"]
        );
    }

    #[test]
    fn a_list_already_in_order_stays_as_it_is_and_an_empty_one_is_empty() {
        use WorkspaceKind::Folder;
        let list = vec![ws("a", Folder, None), ws("b", Folder, None)];
        assert_eq!(names(group(list)), ["a", "b"]);
        assert!(group(Vec::new()).is_empty());
    }
}
