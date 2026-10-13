//! What a client's `cmd` does. Runs without the hub lock held for anything slow.

pub use contract::ErrorBody;
use contract::{Cmd, ErrorCode, Outcome};
use contract::{False, True};
use serde_json::Value;

use crate::panes;
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
        Cmd::MarkSeen { node, rev } => mark_seen(shared, &node, rev),
        Cmd::NewPane { workspace, program } => panes::new_pane(shared, &workspace, program),
        Cmd::ClosePane { pane } => panes::close_pane(shared, &pane),
        Cmd::Write { pane, data } => panes::write(shared, &pane, &data),
        Cmd::Resize { pane, rows, cols } => panes::resize(shared, &pane, rows, cols),
    }
}

/// End the attentions a node raised up to `rev`, so every client stops showing them.
fn mark_seen(shared: &Shared, node: &str, rev: u64) -> Done {
    let mut core = shared.lock();
    let ended = |own: &mut Vec<crate::model::Own>| {
        let before = own.len();
        own.retain(|o| !o.rev.is_some_and(|r| r <= rev));
        before != own.len()
    };
    let mut found = false;
    for ws in &mut core.model.workspaces {
        if ws.id == node {
            found = true;
            ended(&mut ws.attention);
        }
        for p in ws.panes.iter_mut().filter(|p| p.id == node) {
            found = true;
            ended(&mut p.attention);
        }
    }
    if !found {
        return Err(err(ErrorCode::NotFound, format!("no node {node}")));
    }
    core.refresh();
    Ok(None)
}
