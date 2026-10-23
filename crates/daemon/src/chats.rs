//! Conversations of their own (N172, REQ-792 to REQ-804): a workspace with `chat`, in an empty
//! folder under the state directory, with one conversation panel. Closing keeps it (archived,
//! with the id of its session) and resuming opens a panel on that session; deleting removes the
//! folder and never touches Claude's own folders. No git is read for it and no worktree is made.

use std::path::{Path, PathBuf};

use contract::conversation::Item;
use contract::{Cmd, ErrorCode, Id, WorkspaceKind, WorkspaceSummary};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::core::Core;
use crate::model::{Chat, Workspace, new_id};
use crate::server::Shared;

/// What a chat is called until its first message names it (REQ-794).
pub const NEW_NAME: &str = "conversación nueva";
/// The most characters of a chat's name and of its last line (REQ-795, REQ-792).
const NAME: usize = 40;
const LAST: usize = 80;
/// What the summary of `promote_chat` is made of (REQ-803).
const SUMMARY_ITEMS: usize = 6;
const SUMMARY_CLIP: usize = 300;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `text` on one line, its whitespace collapsed, cut to `max` characters with `…`.
fn one_line(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// The session's workspaces in the order Home draws them: the groups' and the repositories' as
/// `grouping::group` says, and the chats after all of them, the live ones by activity (the most
/// recent first) and then the archived ones the same way (REQ-797).
pub fn arrange(list: Vec<WorkspaceSummary>, groups: &[Id]) -> Vec<WorkspaceSummary> {
    let (mut chats, rest): (Vec<_>, Vec<_>) = list.into_iter().partition(|w| w.chat.is_some());
    chats.sort_by_key(|w| {
        let c = w.chat.as_ref().expect("partitioned by chat");
        (c.archived, std::cmp::Reverse(c.active))
    });
    let mut out = crate::grouping::group(rest, groups);
    out.extend(chats);
    out
}

fn bad(why: &str) -> ErrorBody {
    err(ErrorCode::BadRequest, why)
}

/// The commands that make no sense for a chat answer `bad_request` (REQ-796): a chat is closed
/// with `archive_chat` and removed with `delete_chat`, so no folder or session id is left behind.
pub fn refuse(shared: &Shared, cmd: &Cmd) -> Result<(), ErrorBody> {
    let core = shared.lock();
    let chat_id = |id: &str| {
        core.model
            .workspaces
            .iter()
            .any(|w| w.id == id && w.chat.is_some())
    };
    let chat_path = |p: &str| {
        core.model
            .workspaces
            .iter()
            .any(|w| w.path == p && w.chat.is_some())
    };
    let hit = match cmd {
        Cmd::OpenDiff { workspace, .. }
        | Cmd::OpenCommits { workspace, .. }
        | Cmd::SyncWorkspace { workspace }
        | Cmd::StartWorkspace { workspace, .. }
        | Cmd::MoveToGroup { workspace, .. }
        | Cmd::MoveRepo { workspace, .. }
        | Cmd::RemoveWorkspace { workspace, .. } => chat_id(workspace),
        Cmd::CreateWorktree { repo, .. } | Cmd::OpenWorktree { repo, .. } => chat_path(repo),
        _ => false,
    };
    if hit {
        return Err(bad(
            "a conversation of its own has no git, group or worktree: archive_chat closes it and delete_chat removes it",
        ));
    }
    Ok(())
}

/// The workspace `id`, which has to be a chat.
fn chat_of<'a>(core: &'a Core, id: &str) -> Result<&'a Workspace, ErrorBody> {
    let ws = core.workspace(id)?;
    if ws.chat.is_none() {
        return Err(bad("that workspace is not a conversation of its own"));
    }
    Ok(ws)
}

fn state_dir(shared: &Shared) -> Result<PathBuf, ErrorBody> {
    shared
        .lock()
        .state_dir
        .clone()
        .ok_or_else(|| err(ErrorCode::Io, "the daemon has no state directory"))
}

/// A panel of a chat: `claude`, with the session to resume when there is one.
fn start(shared: &Shared, ws: &Id, session: Option<&str>) -> Result<Id, ErrorBody> {
    let mut argv = vec!["claude".to_owned()];
    if let Some(id) = session {
        argv.extend(["--resume".to_owned(), id.to_owned()]);
    }
    let reply = crate::panes::new_pane(shared, ws, Some(argv), None, None)?;
    reply
        .as_ref()
        .and_then(|r| r["pane"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| err(ErrorCode::Io, "the pane has no id"))
}

/// `chat_new` (REQ-794).
pub fn new(shared: &Shared) -> Done {
    let id = new_id();
    let dir = identity::chats_dir(&state_dir(shared)?).join(&id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| err(ErrorCode::Io, format!("{}: {e}", dir.display())))?;
    let ws = Workspace {
        id: id.clone(),
        name: NEW_NAME.to_owned(),
        kind: WorkspaceKind::Folder,
        path: dir.to_string_lossy().into_owned(),
        project: None,
        branch: None,
        panes: Vec::new(),
        layout: None,
        attention: Vec::new(),
        views: Vec::new(),
        places: Default::default(),
        adopted: false,
        ports: None,
        review: None,
        github: None,
        chat: Some(Chat {
            archived: false,
            session: None,
            active: now(),
            last: None,
        }),
    };
    if let Err(e) = crate::workspaces::added(shared, ws) {
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    if let Err(e) = start(shared, &id, None) {
        forget(shared, &id);
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    Ok(Some(json!({ "workspace": id })))
}

/// Take the workspace out of the model; its panels and engines end with the next refresh.
fn forget(shared: &Shared, id: &str) {
    let mut core = shared.lock();
    core.model.workspaces.retain(|w| w.id != id);
    core.refresh();
}

/// The folder is where it should be to be removed: inside `chats_dir`, and not a link out of it.
/// A folder that is already gone is fine: only the record is left to remove.
fn inside(root: &Path, dir: &Path) -> Result<(), ErrorBody> {
    if dir
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(bad("the folder of a chat is not a link"));
    }
    match (root.canonicalize(), dir.canonicalize()) {
        (Ok(root), Ok(dir)) if dir != root && dir.starts_with(&root) => Ok(()),
        (_, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(bad(
            "the folder is not inside the folder of the conversations",
        )),
    }
}

/// The chat's panels closed, its folder removed and the chat forgotten (REQ-800).
fn remove(shared: &Shared, id: &str) -> Result<(), ErrorBody> {
    let root = identity::chats_dir(&state_dir(shared)?);
    let dir = PathBuf::from(&chat_of(&shared.lock(), id)?.path);
    inside(&root, &dir)?;
    {
        let mut core = shared.lock();
        if let Some(ws) = core.model.workspaces.iter_mut().find(|w| w.id == id) {
            ws.panes.clear();
            ws.layout = None;
        }
        core.refresh();
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| err(ErrorCode::Io, format!("{}: {e}", dir.display())))?;
    }
    forget(shared, id);
    Ok(())
}

/// `archive_chat` (REQ-798).
pub fn archive(shared: &Shared, id: &Id) -> Done {
    let empty = {
        let mut core = shared.lock();
        let ws = chat_of(&core, id)?;
        let chat = ws.chat.as_ref().expect("a chat");
        if chat.archived {
            return Ok(None);
        }
        if chat.last.is_some() {
            // The id the panel's conversation has, before the panel goes.
            let session = ws
                .panes
                .iter()
                .find_map(|p| p.resume.as_ref().and_then(|r| r.get(2).cloned()));
            let ws = core
                .model
                .workspaces
                .iter_mut()
                .find(|w| w.id == *id)
                .expect("found above");
            ws.panes.clear();
            ws.layout = None;
            let chat = ws.chat.as_mut().expect("a chat");
            chat.session = session.or(chat.session.take());
            chat.archived = true;
            core.refresh();
            false
        } else {
            true
        }
    };
    if empty {
        remove(shared, id)?;
        return Ok(Some(json!({ "discarded": true })));
    }
    Ok(None)
}

/// `resume_chat` (REQ-799): a panel on the session the chat kept; the engine starts it with
/// `--resume` when the transcript is on disk and says so when it is not (REQ-754, REQ-755).
pub fn resume(shared: &Shared, id: &Id) -> Done {
    let session = {
        let core = shared.lock();
        let chat = chat_of(&core, id)?.chat.as_ref().expect("a chat");
        if !chat.archived {
            return Err(bad("the conversation is not archived"));
        }
        chat.session.clone()
    };
    let pane = start(shared, id, session.as_deref())?;
    let mut core = shared.lock();
    if let Some(chat) = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.id == *id)
        .and_then(|w| w.chat.as_mut())
    {
        chat.archived = false;
        chat.active = now();
    }
    core.refresh();
    Ok(Some(json!({ "pane": pane })))
}

/// `delete_chat` (REQ-800).
pub fn delete(shared: &Shared, id: &Id) -> Done {
    remove(shared, id)?;
    Ok(None)
}

/// What the chat said, to put in the field of the new conversation: the last items of the
/// person and of Claude, one line each (REQ-803).
fn summary(core: &Core, ws: &Workspace) -> String {
    let lines: Vec<String> = ws
        .panes
        .iter()
        .find_map(|p| core.convs.get(&p.id))
        .map(|c| {
            c.st.items
                .iter()
                .filter_map(|i| match i {
                    Item::User { text, .. } => {
                        Some(format!("tú: {}", one_line(text, SUMMARY_CLIP)))
                    }
                    Item::Text { text, .. } => {
                        Some(format!("claude: {}", one_line(text, SUMMARY_CLIP)))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let tail = &lines[lines.len().saturating_sub(SUMMARY_ITEMS)..];
    format!(
        "Contexto de la conversación «{}» (sus últimos mensajes):\n{}\n\n",
        ws.name,
        tail.join("\n")
    )
}

/// `promote_chat` (REQ-803): a new conversation in `to`; the chat stays as it was.
pub fn promote(shared: &Shared, id: &Id, to: &Id, summary_wanted: Option<bool>) -> Done {
    let text = {
        let core = shared.lock();
        let ws = chat_of(&core, id)?;
        let chat = ws.chat.as_ref().expect("a chat");
        if chat.archived {
            return Err(bad(
                "resume the conversation first: only a live one can be taken",
            ));
        }
        if chat.last.is_none() {
            return Err(bad("the conversation is empty: there is nothing to take"));
        }
        if core.workspace(to)?.chat.is_some() {
            return Err(bad("a conversation of its own cannot be taken to another"));
        }
        if summary_wanted == Some(false) {
            String::new()
        } else {
            summary(&core, ws)
        }
    };
    let pane = crate::panes::new_pane(shared, to, Some(vec!["claude".to_owned()]), None, None)?
        .and_then(|r| r["pane"].as_str().map(str::to_owned))
        .ok_or_else(|| err(ErrorCode::Io, "the pane has no id"))?;
    Ok(Some(json!({ "pane": pane, "text": text })))
}

/// The person sent `text` in the conversation of `pane`: if it is a chat's, that is its activity
/// and its last line, and its first message names it while nothing else has (REQ-795).
pub fn said(core: &mut Core, pane: &str, text: &str) {
    let Some(ws) = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.panes.iter().any(|p| p.id == pane))
    else {
        return;
    };
    let Some(chat) = ws.chat.as_mut() else { return };
    chat.active = now();
    chat.last = Some(one_line(text, LAST));
    if ws.name == NEW_NAME {
        ws.name = one_line(text, NAME);
    }
}

/// A turn of the conversation of `pane` ended: its last line is what Claude wrote last.
pub fn answered(core: &mut Core, pane: &str) {
    let said = core.convs.get(pane).and_then(|c| {
        c.st.items.iter().rev().find_map(|i| match i {
            Item::Text { text, .. } => Some(one_line(text, LAST)),
            _ => None,
        })
    });
    let Some(ws) = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.panes.iter().any(|p| p.id == pane))
    else {
        return;
    };
    if let Some(chat) = ws.chat.as_mut() {
        chat.active = now();
        if said.is_some() {
            chat.last = said;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::ChatInfo;

    fn summary(id: &str, chat: Option<(bool, u64)>) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: id.into(),
            kind: WorkspaceKind::Folder,
            path: format!("/p/{id}"),
            project: None,
            branch: None,
            pane_count: 0,
            attention: Vec::new(),
            review: None,
            working: false,
            removing: false,
            group: None,
            chat: chat.map(|(archived, active)| ChatInfo {
                archived,
                active,
                last: None,
            }),
        }
    }

    #[test]
    fn a_line_is_collapsed_and_cut_with_an_ellipsis() {
        assert_eq!(one_line("  a \n b\tc ", 40), "a b c");
        let long = "x".repeat(60);
        let cut = one_line(&long, NAME);
        assert_eq!(cut.chars().count(), NAME);
        assert!(cut.ends_with('…'));
        assert_eq!(one_line("áé", 2), "áé");
    }

    #[test]
    fn chats_come_after_everything_live_first_then_archived_by_activity() {
        let list = vec![
            summary("old", Some((false, 1))),
            summary("ws", None),
            summary("gone", Some((true, 9))),
            summary("new", Some((false, 5))),
            summary("other", None),
        ];
        let order: Vec<_> = arrange(list, &[]).into_iter().map(|w| w.id).collect();
        assert_eq!(order, ["ws", "other", "new", "old", "gone"]);
    }

    #[test]
    fn only_a_folder_inside_the_chats_is_removed() {
        let tmp = std::env::temp_dir().join(format!("jw-chats-{}", std::process::id()));
        let root = tmp.join("chats");
        let inner = root.join("a");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(tmp.join("outside")).unwrap();
        assert!(inside(&root, &inner).is_ok());
        assert!(inside(&root, &root).is_err());
        assert!(inside(&root, &tmp.join("outside")).is_err());
        assert!(inside(&root, &root.join("..").join("outside")).is_err());
        std::os::unix::fs::symlink(tmp.join("outside"), root.join("link")).unwrap();
        assert!(inside(&root, &root.join("link")).is_err());
        assert!(inside(&root, &root.join("gone")).is_ok());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
