//! What a client's `cmd` does. Runs without the hub lock held for anything slow.

pub use contract::ErrorBody;
use contract::{Cmd, ErrorCode, Outcome};
use contract::{False, True};
use serde_json::Value;

use crate::server::Shared;
use crate::workspaces;

pub type Done = Result<Option<Value>, ErrorBody>;

pub fn err(code: ErrorCode, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code,
        message: message.into(),
    }
}

pub fn outcome(done: Done) -> Outcome {
    match done {
        Ok(result) => Outcome::Ok { ok: True, result },
        Err(error) => Outcome::Err { ok: False, error },
    }
}

pub fn run(shared: &Shared, cmd: Cmd) -> Done {
    match cmd {
        Cmd::RenameSession { name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(err(ErrorCode::BadRequest, "the session name is empty"));
            }
            let mut core = shared.lock();
            core.model.name = name.to_owned();
            core.refresh();
            Ok(None)
        }
        Cmd::OpenFolder { path } => workspaces::open_folder(shared, &path),
        Cmd::CreateWorktree { repo, branch, base } => {
            workspaces::create_worktree(shared, &repo, &branch, base.as_deref())
        }
        Cmd::RemoveWorkspace {
            workspace,
            remove_worktree,
        } => workspaces::remove_workspace(shared, &workspace, remove_worktree),
        Cmd::NewPane { .. } | Cmd::ClosePane { .. } | Cmd::Write { .. } | Cmd::Resize { .. } => {
            Err(err(ErrorCode::Unsupported, "panes are not implemented yet"))
        }
    }
}
