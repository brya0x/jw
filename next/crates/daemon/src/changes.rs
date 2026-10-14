//! The commands that read a workspace's changes: `open_diff`, `open_commits`, `refresh_diff`
//! and `set_viewed`. Git runs without the hub lock held.

use std::path::PathBuf;

use contract::{DiffScope, ErrorCode, Id};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::core::Reread;
use crate::diff::{self, Fail, Scope};
use crate::server::Shared;

fn failed(fail: Fail) -> ErrorBody {
    match fail {
        Fail::NotRepo => err(ErrorCode::BadRequest, "not inside a git repository"),
        Fail::Bad(why) => err(ErrorCode::BadRequest, why),
        Fail::Git(why) => err(ErrorCode::GitFailed, why),
    }
}

fn dir_of(shared: &Shared, workspace: &Id) -> Result<PathBuf, ErrorBody> {
    shared
        .lock()
        .workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))
}

pub fn open_diff(shared: &Shared, workspace: &Id, scope: DiffScope, commit: Option<&str>) -> Done {
    let dir = dir_of(shared, workspace)?;
    let scope = match (scope, commit) {
        (DiffScope::Uncommitted, _) => Scope::Uncommitted,
        (DiffScope::General, _) => Scope::General,
        (DiffScope::Commit, Some(sha)) => {
            Scope::Commit(diff::resolve_commit(&dir, sha).map_err(failed)?)
        }
        (DiffScope::Commit, None) => {
            return Err(err(
                ErrorCode::BadRequest,
                "the commit scope needs a commit",
            ));
        }
    };
    if let Some(node) = shared.lock().find_diff(workspace, &scope) {
        return Ok(Some(json!({ "node": node })));
    }
    let read = diff::read(&dir, &scope).map_err(failed)?;
    let node = shared.lock().add_diff(workspace, scope, read);
    Ok(Some(json!({ "node": node })))
}

pub fn open_commits(shared: &Shared, workspace: &Id) -> Done {
    let dir = dir_of(shared, workspace)?;
    if let Some(node) = shared.lock().find_commits(workspace) {
        return Ok(Some(json!({ "node": node })));
    }
    let list = diff::read_commits(&dir).map_err(failed)?;
    let node = shared.lock().add_commits(workspace, list);
    Ok(Some(json!({ "node": node })))
}

/// Read a `diff` or `commits` node again now.
pub fn refresh(shared: &Shared, node: &Id) -> Done {
    let (dir, what) = shared.lock().reread_target(node).ok_or_else(|| {
        err(
            ErrorCode::NotFound,
            format!("no diff or commits node {node}"),
        )
    })?;
    match what {
        Reread::Diff(scope) => {
            let read = diff::read(&dir, &scope).map_err(failed)?;
            shared.lock().diff_update(node, read);
        }
        Reread::Commits => {
            let list = diff::read_commits(&dir).map_err(failed)?;
            shared.lock().commits_update(node, list);
        }
    }
    Ok(None)
}

pub fn set_viewed(shared: &Shared, node: &Id, path: &str, viewed: bool) -> Done {
    shared
        .lock()
        .set_viewed(node, path, viewed)
        .map(|()| None)
        .map_err(|(code, why)| err(code, why))
}
