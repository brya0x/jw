//! Sync: pushing a worktree's branch to its remote (spec REQ-274 to REQ-277). It is `git push`,
//! never forced and with nothing merged: what the remote has that the worktree has not is for the
//! person to bring, with git, in a pane.

use std::path::Path;

use contract::{ErrorCode, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::git;
use crate::server::Shared;

/// The folder and the branch of a worktree that has one.
fn worktree_of(shared: &Shared, workspace: &str) -> Result<(String, String), ErrorBody> {
    let core = shared.lock();
    let ws = core.workspace(workspace)?;
    match (ws.kind, ws.branch.clone()) {
        (WorkspaceKind::Worktree, Some(branch)) if !branch.starts_with('-') => {
            Ok((ws.path.clone(), branch))
        }
        _ => Err(err(
            ErrorCode::BadRequest,
            "only a worktree with a branch has one to push",
        )),
    }
}

/// Whether the branch has an upstream, and how many commits it has that the remote has not: those
/// ahead of the upstream, or without one, those that no remote has.
fn ahead(dir: &Path) -> Result<(bool, u64), ErrorBody> {
    let upstream = git::run(
        dir,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok();
    let count = match &upstream {
        Some(up) => git::run(dir, &["rev-list", "--count", &format!("{up}..HEAD")]),
        None => git::run(dir, &["rev-list", "--count", "HEAD", "--not", "--remotes"]),
    }
    .map_err(|m| err(ErrorCode::GitFailed, m))?;
    Ok((upstream.is_some(), count.parse().unwrap_or(0)))
}

/// `check_workspace`: the commits of the worktree's branch that no remote has (REQ-285).
pub fn check(shared: &Shared, workspace: &str) -> Done {
    let (path, _) = worktree_of(shared, workspace)?;
    let (_, ahead) = ahead(Path::new(&path))?;
    Ok(Some(json!({ "ahead": ahead })))
}

pub fn workspace(shared: &Shared, workspace: &str) -> Done {
    // The lock is let go before git runs: a push may take as long as the network does.
    let (path, branch) = worktree_of(shared, workspace)?;
    let dir = Path::new(&path);
    let (has_upstream, ahead) = ahead(dir)?;
    let new_branch = !has_upstream;
    if has_upstream && ahead == 0 {
        return Ok(Some(
            json!({"branch": branch, "pushed": 0, "new_branch": false}),
        ));
    }
    let args: Vec<&str> = if new_branch {
        vec!["push", "--set-upstream", "origin", &branch]
    } else {
        vec!["push"]
    };
    git::run(dir, &args).map_err(|m| err(ErrorCode::GitFailed, m))?;
    Ok(Some(
        json!({"branch": branch, "pushed": ahead, "new_branch": new_branch}),
    ))
}
