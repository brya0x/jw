//! What a client's `cmd` does. Runs without the hub lock held for anything slow.

pub use contract::ErrorBody;
use contract::{AttentionKind, Cmd, ErrorCode, Outcome};
use contract::{False, True};
use serde_json::Value;

use crate::agents;
use crate::artifacts;
use crate::browse;
use crate::changes;
use crate::engine;
use crate::files;
use crate::groups;
use crate::markdown;
use crate::panes;
use crate::rename;
use crate::review;
use crate::saved;
use crate::sent;
use crate::server::Shared;
use crate::start;
use crate::sync;
use crate::textfile;
use crate::views;
use crate::workspaces;
use crate::worktrees;

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
    crate::chats::refuse(shared, &cmd)?;
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
        Cmd::ChatNew {} => crate::chats::new(shared),
        Cmd::ArchiveChat { workspace } => crate::chats::archive(shared, &workspace),
        Cmd::ResumeChat { workspace } => crate::chats::resume(shared, &workspace),
        Cmd::DeleteChat { workspace } => crate::chats::delete(shared, &workspace),
        Cmd::PromoteChat {
            workspace,
            to,
            summary,
        } => crate::chats::promote(shared, &workspace, &to, summary),
        Cmd::CreateGroup { name } => groups::create(shared, &name),
        Cmd::RenameGroup { group, name } => groups::rename(shared, &group, &name),
        Cmd::DeleteGroup { group } => groups::delete(shared, &group),
        Cmd::MoveToGroup { workspace, group } => {
            groups::move_to(shared, &workspace, group.as_deref())
        }
        Cmd::MoveGroup { group, delta } => groups::move_group(shared, &group, delta),
        Cmd::MoveRepo { workspace, delta } => groups::move_repo(shared, &workspace, delta),
        Cmd::SyncWorkspace { workspace } => sync::workspace(shared, &workspace),
        Cmd::DismissJob { job } => {
            shared.lock().dismiss_job(&job)?;
            Ok(None)
        }
        Cmd::DismissJobs {} => {
            shared.lock().dismiss_finished_jobs();
            Ok(None)
        }
        Cmd::Cancel { workspace } => crate::cancel::cancel(shared, &workspace),
        Cmd::CheckWorkspace { workspace } => sync::check(shared, &workspace),
        Cmd::OpenFolder { path } => workspaces::open_folder(shared, &path),
        Cmd::ListDir { path, hidden } => browse::list_dir(path.as_deref(), hidden),
        Cmd::ListFiles { workspace, path } => files::list(shared, &workspace, path.as_deref()),
        Cmd::FindFiles { workspace, query } => files::find(shared, &workspace, &query),
        Cmd::ListConversations { workspace } => saved::list(shared, &workspace),
        Cmd::ReadMarkdown { workspace, path } => markdown::read(shared, &workspace, &path),
        Cmd::ReadFile { workspace, path } => textfile::read(shared, &workspace, &path),
        Cmd::ReadOlder {
            node,
            before,
            count,
        } => engine::read_older(shared, &node, before, count),
        Cmd::ReadPlan { node, ask } => markdown::read_plan(shared, &node, &ask),
        Cmd::CreateWorktree {
            repo,
            branch,
            base,
            folder,
        } => {
            workspaces::create_worktree(shared, &repo, &branch, base.as_deref(), folder.as_deref())
        }
        Cmd::ListWorktrees { repo } => worktrees::list(shared, &repo),
        Cmd::ListPrs {
            workspace,
            requested,
        } => review::list(shared, &workspace, requested),
        Cmd::ReviewPr { workspace, pr } => review::review(shared, &workspace, pr),
        Cmd::OpenWorktree { repo, path } => worktrees::open(shared, &repo, &path),
        Cmd::ProjectConfig { repo } => start::project_config(shared, &repo),
        Cmd::WriteProjectConfig { repo, to, text } => {
            start::write_project_config(shared, &repo, to, &text)
        }
        Cmd::EditProjectConfig { repo, to, set } => {
            start::edit_project_config(shared, &repo, to, &set)
        }
        Cmd::StartWorkspace { workspace, open } => start::start_workspace(shared, &workspace, open),
        Cmd::RemoveWorkspace {
            workspace,
            remove_worktree,
            force,
        } => workspaces::remove_workspace(shared, &workspace, remove_worktree, force),
        Cmd::MarkSeen { node, rev } => mark_seen(shared, &node, rev),
        Cmd::NewPane {
            workspace,
            program,
            beside,
            dir,
        } => panes::new_pane(shared, &workspace, program, beside, dir),
        Cmd::ClosePane { pane } => panes::close_pane(shared, &pane),
        Cmd::OpenView {
            workspace,
            kind,
            path,
            beside,
            dir,
        } => views::open_view(shared, &workspace, kind, path, beside, dir),
        Cmd::CloseView { view, path } => views::close_view(shared, &view, path),
        Cmd::SwapPanes { a, b } => panes::swap_panes(shared, &a, &b),
        Cmd::GrowPane { pane, dir, by } => panes::grow_pane(shared, &pane, dir, by),
        Cmd::Write { pane, data } => panes::write(shared, &pane, &data),
        Cmd::Resize { pane, rows, cols } => panes::resize(shared, &pane, rows, cols),
        Cmd::OpenDiff {
            workspace,
            scope,
            commit,
        } => changes::open_diff(shared, &workspace, scope, commit.as_deref()),
        Cmd::OpenCommits { workspace } => changes::open_commits(shared, &workspace),
        Cmd::OpenTasks { workspace } => crate::tasklists::open_tasks(shared, &workspace),
        Cmd::TasksEdit {
            workspace,
            op,
            id,
            text,
        } => crate::tasklists::edit(shared, &workspace, op, &id, text.as_deref()),
        Cmd::ArtifactPublish {
            workspace,
            name,
            title,
            kind,
            format,
            text,
            pane,
        } => artifacts::publish(
            shared,
            artifacts::Publish {
                workspace,
                name,
                title,
                kind,
                format,
                text,
                pane,
            },
        ),
        Cmd::OpenArtifacts { workspace } => artifacts::open(shared, &workspace),
        Cmd::ArtifactRead { id, n } => artifacts::read(shared, &id, n),
        Cmd::ArtifactDecide {
            id,
            n,
            hash,
            verdict,
            text,
        } => artifacts::decide(shared, &id, n, &hash, verdict, text),
        Cmd::ArtifactRm { id } => artifacts::rm(shared, &id),
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
        Cmd::OpenConversation {
            pane,
            resume,
            args,
            env,
        } => engine::open_conversation(shared, &pane, resume, args, env),
        Cmd::ConversationSend {
            node,
            text,
            images,
            folds,
            private,
        } => {
            // What the person sent is kept for ↑ before the daemon answers any of it itself.
            sent::record(shared, &node, &text, &folds, private);
            engine::send(shared, &node, &text, images)
        }
        Cmd::ConversationHistory { node } => sent::history(shared, &node),
        Cmd::ConversationAnswer {
            node,
            ask,
            key,
            text,
            by,
        } => engine::answer(shared, &node, &ask, &key, text.as_deref(), by.as_deref()),
        Cmd::ConversationInterrupt { node } => engine::interrupt(shared, &node),
        Cmd::ConversationStopTask { node, task } => engine::stop_task(shared, &node, &task),
        Cmd::ConversationSet {
            node,
            name,
            accent,
            model,
            effort,
            mode,
        } => engine::set(shared, &node, name, accent, model, effort, mode),
        Cmd::ConversationShell { node, command } => engine::shell(shared, &node, &command),
        Cmd::ConversationClose { node } => engine::close(shared, &node),
    }
}

/// End the attentions a node raised up to `rev`, so every client stops showing them.
fn mark_seen(shared: &Shared, node: &str, rev: u64) -> Done {
    let mut core = shared.lock();
    let ended = |own: &mut Vec<crate::model::Own>| {
        let before = own.len();
        // An artifact waits for a verdict, not for a look (REQ-791).
        own.retain(|o| o.kind == AttentionKind::Artifact || !o.rev.is_some_and(|r| r <= rev));
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
