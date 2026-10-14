//! What a client's `cmd` does. Runs without the hub lock held for anything slow.

pub use contract::ErrorBody;
use contract::{Cmd, ErrorCode, Outcome};
use contract::{False, True};
use serde_json::Value;

use crate::agents;
use crate::changes;
use crate::panes;
use crate::rename;
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
        Cmd::RenameWorkspace { workspace, name } => rename::workspace(shared, &workspace, &name),
        Cmd::RenamePane { pane, name } => rename::pane(shared, &pane, &name),
        Cmd::OpenFolder { path } => workspaces::open_folder(shared, &path),
        Cmd::CreateWorktree {
            repo,
            branch,
            base,
            folder,
        } => {
            workspaces::create_worktree(shared, &repo, &branch, base.as_deref(), folder.as_deref())
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
        Cmd::OpenDiff {
            workspace,
            scope,
            commit,
        } => changes::open_diff(shared, &workspace, scope, commit.as_deref()),
        Cmd::OpenCommits { workspace } => changes::open_commits(shared, &workspace),
        Cmd::RefreshDiff { node } => changes::refresh(shared, &node),
        Cmd::SetViewed { node, path, viewed } => changes::set_viewed(shared, &node, &path, viewed),
        Cmd::AgentEvent {
            pane,
            event,
            notification_type,
            tool,
        } => agents::event(
            shared,
            &pane,
            event,
            notification_type.as_deref(),
            tool.as_deref(),
        ),
        Cmd::MobileStart {} => {
            let hub = shared.lock().mobile.clone();
            hub.start().map(Some).map_err(|why| err(ErrorCode::Io, why))
        }
        Cmd::MobileLink {} => {
            let hub = shared.lock().mobile.clone();
            hub.link().map(Some).map_err(|why| err(ErrorCode::Io, why))
        }
        Cmd::MobileWait { seconds } => {
            let hub = shared.lock().mobile.clone();
            Ok(hub
                .wait(seconds)
                .map(|ask| serde_json::json!({"name": ask.name, "code": ask.code})))
        }
        Cmd::MobileAnswer { yes } => {
            let hub = shared.lock().mobile.clone();
            if hub.answer(yes) {
                Ok(None)
            } else {
                Err(err(ErrorCode::NotFound, "no phone is asking to pair"))
            }
        }
        Cmd::MobileStop {} => {
            let hub = shared.lock().mobile.clone();
            hub.stop();
            Ok(None)
        }
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
