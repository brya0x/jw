//! Naming: what a worktree and a pane are called by. Only the daemon's own record changes;
//! a worktree's folder and branch are git's and are left as they are. A folder, or the root of a
//! repository, is a directory on disk and is not renamed.

use std::path::Path;

use contract::{ErrorCode, WorkspaceKind};

use crate::commands::{Done, err};
use crate::server::Shared;

/// The most characters a name may have.
const MAX: usize = 80;

/// The name as kept: trimmed, with no control characters and not too long; `None` for none.
fn clean(name: &str) -> Result<Option<String>, crate::commands::ErrorBody> {
    let name = name.trim();
    if name.chars().any(char::is_control) {
        return Err(err(
            ErrorCode::BadRequest,
            "a name has no control characters",
        ));
    }
    if name.chars().count() > MAX {
        return Err(err(
            ErrorCode::BadRequest,
            format!("a name has at most {MAX} characters"),
        ));
    }
    Ok((!name.is_empty()).then(|| name.to_owned()))
}

/// The name a worktree has until it is given another: `repo: branch`.
fn automatic(path: &str, project: Option<&str>, branch: Option<&str>) -> String {
    let last = |p: &str| {
        Path::new(p)
            .file_name()
            .map_or_else(|| p.to_owned(), |n| n.to_string_lossy().into_owned())
    };
    match (project, branch) {
        (Some(project), Some(branch)) => format!("{}: {branch}", last(project)),
        _ => last(path),
    }
}

pub fn workspace(shared: &Shared, workspace: &str, name: &str) -> Done {
    let name = clean(name)?;
    let mut core = shared.lock();
    let Some(ws) = core.model.workspaces.iter_mut().find(|w| w.id == workspace) else {
        return Err(err(
            ErrorCode::NotFound,
            format!("no workspace {workspace}"),
        ));
    };
    // A chat's folder is the daemon's, not the person's (REQ-802).
    if ws.kind != WorkspaceKind::Worktree && ws.chat.is_none() {
        return Err(err(
            ErrorCode::BadRequest,
            "only a worktree is renamed: a folder is a directory on disk",
        ));
    }
    ws.name = name.unwrap_or_else(|| {
        if ws.chat.is_some() {
            crate::chats::NEW_NAME.to_owned()
        } else {
            automatic(&ws.path, ws.project.as_deref(), ws.branch.as_deref())
        }
    });
    core.refresh();
    Ok(None)
}

pub fn pane(shared: &Shared, pane: &str, name: &str) -> Done {
    let name = clean(name)?;
    let mut core = shared.lock();
    let Some(p) = core
        .model
        .workspaces
        .iter_mut()
        .flat_map(|w| w.panes.iter_mut())
        .find(|p| p.id == pane)
    else {
        return Err(err(ErrorCode::NotFound, format!("no pane {pane}")));
    };
    p.title = name.clone().unwrap_or_else(|| {
        p.running
            .as_deref()
            .and_then(crate::procs::title_for)
            .unwrap_or_else(|| crate::panes::basename(&p.shell))
    });
    p.name = name;
    core.refresh();
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_and_may_be_empty_but_not_long_or_with_control_characters() {
        assert_eq!(clean("  login fix ").unwrap().as_deref(), Some("login fix"));
        assert_eq!(clean("   ").unwrap(), None);
        assert!(clean(&"x".repeat(81)).is_err());
        assert_eq!(clean(&"x".repeat(80)).unwrap().map(|n| n.len()), Some(80));
        assert!(clean("a\nb").is_err());
        assert_eq!(clean("é ñ").unwrap().as_deref(), Some("é ñ"));
    }

    #[test]
    fn the_automatic_name_of_a_worktree_is_its_repo_and_branch() {
        let wt = automatic(
            "/home/u/app-wt/login",
            Some("/home/u/app"),
            Some("feat/login"),
        );
        assert_eq!(wt, "app: feat/login");
        assert_eq!(automatic("/home/u/app-wt/login", None, None), "login");
    }
}
