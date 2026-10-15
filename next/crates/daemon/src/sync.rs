//! Sync: pushing a worktree's branch to its remote (spec REQ-274 to REQ-277). It is `git push`,
//! never forced and with nothing merged: what the remote has that the worktree has not is for the
//! person to bring, with git, in a pane.

use std::path::Path;

use contract::{ErrorCode, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, err};
use crate::git;
use crate::server::Shared;

pub fn workspace(shared: &Shared, workspace: &str) -> Done {
    // The lock is let go before git runs: a push may take as long as the network does.
    let (path, branch) = {
        let core = shared.lock();
        let ws = core
            .model
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
        match (ws.kind, ws.branch.clone()) {
            (WorkspaceKind::Worktree, Some(branch)) if !branch.starts_with('-') => {
                (ws.path.clone(), branch)
            }
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "only a worktree with a branch has one to push",
                ));
            }
        }
    };
    let dir = Path::new(&path);
    let git_failed = |m: String| err(ErrorCode::GitFailed, m);
    let upstream = git::run(
        dir,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok();
    // Without an upstream the branch is new on the remote: what goes up is whatever no remote has.
    let ahead: u64 = match &upstream {
        Some(up) => git::run(dir, &["rev-list", "--count", &format!("{up}..HEAD")]),
        None => git::run(dir, &["rev-list", "--count", "HEAD", "--not", "--remotes"]),
    }
    .map_err(git_failed)?
    .parse()
    .unwrap_or(0);
    let new_branch = upstream.is_none();
    if !new_branch && ahead == 0 {
        return Ok(Some(
            json!({"branch": branch, "pushed": 0, "new_branch": false}),
        ));
    }
    let args: Vec<&str> = if new_branch {
        vec!["push", "--set-upstream", "origin", &branch]
    } else {
        vec!["push"]
    };
    git::run(dir, &args).map_err(git_failed)?;
    Ok(Some(
        json!({"branch": branch, "pushed": ahead, "new_branch": new_branch}),
    ))
}
