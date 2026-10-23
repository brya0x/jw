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
        w.chat
            .as_ref()
            .map(|c| (c.archived, std::cmp::Reverse(c.active)))
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
    // An archived chat has no panel and `resume_chat` is what opens one: a shell put in it would
    // make it a live chat with a terminal that is no conversation.
    if let Cmd::NewPane { workspace, .. } = cmd {
        let archived = shared
            .lock()
            .model
            .workspaces
            .iter()
            .any(|w| w.id == *workspace && w.chat.as_ref().is_some_and(|c| c.archived));
        if archived {
            return Err(bad(
                "an archived conversation has no panel: resume_chat opens one",
            ));
        }
        return Ok(());
    }
    // Only these commands are looked at, and only they lock: every keystroke is a command too.
    let (id, path) = match cmd {
        Cmd::OpenDiff { workspace, .. }
        | Cmd::OpenCommits { workspace, .. }
        | Cmd::SyncWorkspace { workspace }
        | Cmd::StartWorkspace { workspace, .. }
        | Cmd::MoveToGroup { workspace, .. }
        | Cmd::MoveRepo { workspace, .. }
        | Cmd::RemoveWorkspace { workspace, .. } => (Some(workspace.as_str()), None),
        Cmd::CreateWorktree { repo, .. } | Cmd::OpenWorktree { repo, .. } => {
            (None, Some(repo.as_str()))
        }
        _ => return Ok(()),
    };
    let core = shared.lock();
    let hit = core.model.workspaces.iter().any(|w| {
        w.chat.is_some() && (id.is_some_and(|i| w.id == i) || path.is_some_and(|p| w.path == p))
    });
    if hit {
        return Err(bad(
            "a conversation of its own has no git, group or worktree: archive_chat closes it and delete_chat removes it",
        ));
    }
    Ok(())
}

/// The workspace `id` and its `chat`: it has to be a chat.
fn chat_of<'a>(core: &'a Core, id: &str) -> Result<(&'a Workspace, &'a Chat), ErrorBody> {
    let ws = core.workspace(id)?;
    match &ws.chat {
        Some(chat) => Ok((ws, chat)),
        None => Err(bad("that workspace is not a conversation of its own")),
    }
}

/// The workspace that has the panel `pane`.
fn workspace_of_pane<'a>(core: &'a mut Core, pane: &str) -> Option<&'a mut Workspace> {
    core.model
        .workspaces
        .iter_mut()
        .find(|w| w.panes.iter().any(|p| p.id == pane))
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
    let dir = PathBuf::from(&chat_of(&shared.lock(), id)?.0.path);
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
    let (session, env, said) = {
        let core = shared.lock();
        let (ws, chat) = chat_of(&core, id)?;
        if chat.archived {
            return Ok(None);
        }
        // The id the panel's conversation has, before the panel goes.
        let session = ws
            .panes
            .iter()
            .find_map(|p| p.resume.as_ref().and_then(|r| r.get(2).cloned()))
            .or_else(|| chat.session.clone());
        (session, core.env.clone(), chat.last.is_some())
    };
    // A conversation that did not go through the daemon's `send` still has its transcript: it is
    // kept and not discarded as an empty one (the look is made with no lock held).
    let spoke = said
        || session
            .as_deref()
            .is_some_and(|s| crate::launch::transcript_exists(&env, s));
    if !spoke {
        remove(shared, id)?;
        return Ok(Some(json!({ "discarded": true })));
    }
    let mut core = shared.lock();
    let ws = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.id == *id)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {id}")))?;
    ws.panes.clear();
    ws.layout = None;
    if let Some(chat) = ws.chat.as_mut() {
        chat.session = session.or(chat.session.take());
        chat.archived = true;
    }
    core.refresh();
    Ok(None)
}

/// `resume_chat` (REQ-799): a panel on the session the chat kept; the engine starts it with
/// `--resume` when the transcript is on disk and says so when it is not (REQ-754, REQ-755).
pub fn resume(shared: &Shared, id: &Id) -> Done {
    let session = {
        let core = shared.lock();
        let (_, chat) = chat_of(&core, id)?;
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
        let (ws, chat) = chat_of(&core, id)?;
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
    let Some(ws) = workspace_of_pane(core, pane) else {
        return;
    };
    let Some(chat) = ws.chat.as_mut() else { return };
    // `/compact` and the like are the engine's, not something said.
    if text.starts_with('/') {
        return;
    }
    chat.active = now();
    chat.last = Some(one_line(text, LAST));
    if ws.name == NEW_NAME {
        ws.name = one_line(text, NAME);
    }
}

/// A turn of the conversation of `pane` ended: its last line is what Claude wrote last.
pub fn answered(core: &mut Core, pane: &str) {
    let said = core.convs.get(pane).and_then(|c| {
        c.st.items
            .iter()
            .rev()
            .take_while(|i| !matches!(i, Item::User { .. }))
            .find_map(|i| match i {
                Item::Text { text, .. } => Some(one_line(text, LAST)),
                _ => None,
            })
    });
    let Some(ws) = workspace_of_pane(core, pane) else {
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
        let _ = std::fs::remove_dir_all(&tmp);
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
