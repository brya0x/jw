//! Sync: pushing a worktree's branch to its remote (spec REQ-274 to REQ-277). It is `git push`,
//! never forced and with nothing merged: what the remote has that the worktree has not is for the
//! person to bring, with git, in a pane.

use std::path::Path;

use contract::{ErrorCode, JobKind, WorkspaceKind};
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
        // The head of a pull request that is being reviewed was fetched, not made here: it is no work to lose.
        None => git::run(
            dir,
            &[
                "rev-list",
                "--count",
                "HEAD",
                "--not",
                "--remotes",
                &format!("--glob=refs/{}/*", identity::get().name),
            ],
        ),
    }
    .map_err(|m| err(ErrorCode::GitFailed, m))?;
    Ok((upstream.is_some(), count.parse().unwrap_or(0)))
}

/// `check_workspace`: the commits of the worktree's branch that no remote has (REQ-283), and
/// `orphan: true` for a worktree that git no longer knows (REQ-1074).
pub fn check(shared: &Shared, workspace: &str) -> Done {
    let (path, _) = worktree_of(shared, workspace)?;
    // A worktree that git no longer lists has nothing to count, and is not a `git_failed` (REQ-1074).
    let project = shared.lock().workspace(workspace)?.project.clone();
    if let Some(project) = project
        && git::knows(Path::new(&project), Path::new(&path)) == Ok(false)
    {
        return Ok(Some(json!({ "ahead": 0, "orphan": true })));
    }
    let (_, ahead) = ahead(Path::new(&path))?;
    Ok(Some(json!({ "ahead": ahead })))
}

pub fn workspace(shared: &Shared, workspace: &str) -> Done {
    // The lock is let go before git runs: a push may take as long as the network does, and the
    // person can stop it (REQ-604).
    let (path, branch) = worktree_of(shared, workspace)?;
    let (cancel, _op) = crate::cancel::begin(shared, workspace)?;
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
    // From here the push is a job that every client sees, and that `cancel` can stop (REQ-652).
    let job = shared
        .lock()
        .begin_job(JobKind::SyncWorkspace, workspace, branch.clone(), true);
    let pushed = git::run_cancel(dir, &args, &cancel);
    shared.lock().end_job(&job, pushed.clone().map(|_| ()));
    pushed.map_err(|m| err(ErrorCode::GitFailed, m))?;
    Ok(Some(
        json!({"branch": branch, "pushed": ahead, "new_branch": new_branch}),
    ))
}
