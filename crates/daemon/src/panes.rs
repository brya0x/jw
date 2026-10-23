//! Panes: terminals inside a workspace, and the engine that turns shell events into
//! model changes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use contract::{AttentionKind, ErrorCode, Id, SplitDir};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::model::{Own, Pane, new_id};
use crate::pty::{self, Event, Mark};
use crate::server::Shared;

/// Output within this window is folded into one revision.
const COALESCE: Duration = Duration::from_millis(20);

fn no_pane(pane: &str) -> crate::commands::ErrorBody {
    err(ErrorCode::NotFound, format!("no pane {pane}"))
}

pub(crate) fn basename(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .map_or_else(|| shell.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Where a shell starts when it is made from a pane that is in `from`: there while that is still
/// a folder, in the workspace's own when it is not (REQ-628, REQ-629).
fn start_folder(from: &str, workspace: &str) -> String {
    if Path::new(from).is_dir() {
        from.to_owned()
    } else {
        workspace.to_owned()
    }
}

/// Open a pane; with `program`, the shell starts it and stays when it ends. An agent gets
/// its conversation id here, so a restart can resume it.
pub fn new_pane(
    shared: &Shared,
    workspace: &Id,
    program: Option<Vec<String>>,
    beside: Option<Id>,
    dir: Option<SplitDir>,
) -> Done {
    new_pane_then(shared, workspace, program, None, beside, dir)
}

/// `new_pane`, where `then` is started when `program` ends well (a worktree's setup, and then
/// `claude`, in one panel: REQ-507).
pub fn new_pane_then(
    shared: &Shared,
    workspace: &Id,
    program: Option<Vec<String>>,
    then: Option<Vec<String>>,
    beside: Option<Id>,
    dir: Option<SplitDir>,
) -> Done {
    if program.as_ref().is_some_and(|p| p.is_empty()) || then.as_ref().is_some_and(|p| p.is_empty())
    {
        return Err(err(ErrorCode::BadRequest, "program is empty"));
    }
    let argv = program.map(|p| crate::launch::with_session(&p, new_id));
    let then = then.map(|p| crate::launch::with_session(&p, new_id));
    let (path, events, shell, env, setup, hooks) = {
        let core = shared.lock();
        let ws = core.workspace(workspace)?;
        // Beside another pane, in the folder that pane is in now (REQ-629).
        let from = beside
            .as_ref()
            .and_then(|b| ws.panes.iter().find(|p| p.id == *b))
            .map_or_else(|| ws.path.clone(), |p| start_folder(&p.cwd, &ws.path));
        if beside
            .as_ref()
            .is_some_and(|b| !ws.panes.iter().any(|p| p.id == *b))
        {
            return Err(err(
                ErrorCode::BadRequest,
                "beside is not a pane of this workspace",
            ));
        }
        (
            from,
            core.events.clone(),
            core.shell.clone(),
            // What the pane's workspace was given besides what every pane has (REQ-496).
            core.env
                .iter()
                .cloned()
                .chain(ws.ports.iter().flat_map(crate::model::PortBlock::env))
                .chain(workspace_dir(&ws.path))
                .collect::<Vec<_>>(),
            core.integration_for(&core.shell),
            core.agent_hooks(),
        )
    };
    let launched = with_agent_hooks(argv.as_deref(), hooks.as_deref());
    let then_launched = with_agent_hooks(then.as_deref(), hooks.as_deref());
    let id = new_id();
    let term = pty::spawn(
        &id,
        &shell,
        Path::new(&path),
        pty::Program {
            argv: launched.as_deref(),
            then: then_launched.as_deref(),
        },
        &env,
        &setup,
        events,
    )
    .map_err(|m| err(ErrorCode::Io, m))?;
    let mut core = shared.lock();
    let Some(ws) = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.id == *workspace)
    else {
        drop(core);
        term.kill();
        return Err(err(
            ErrorCode::NotFound,
            format!("workspace {workspace} was removed"),
        ));
    };
    let ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
    if let Some(beside) = beside.filter(|b| ids.contains(b)) {
        let views: Vec<Id> = ws.views.iter().map(|v| v.id.clone()).collect();
        let tree = crate::layout::reconcile_views(ws.layout.take(), &ids, &views);
        ws.layout =
            tree.map(|t| crate::layout::split(t, &beside, &id, dir.unwrap_or(SplitDir::Row)));
    }
    ws.panes.push(Pane {
        id: id.clone(),
        title: basename(&shell),
        name: None,
        shell,
        cwd: path,
        attention: Vec::new(),
        running: None,
        last_program: None,
        resume: then
            .as_deref()
            .or(argv.as_deref())
            .and_then(crate::launch::agent_resume),
    });
    core.attach_term(&id, term);
    core.refresh();
    Ok(Some(json!({ "pane": id })))
}

/// `argv` with the hooks of an agent, when there are hooks to give.
fn with_agent_hooks(argv: Option<&[String]>, hooks: Option<&Path>) -> Option<Vec<String>> {
    match (argv, hooks) {
        (Some(argv), Some(file)) => Some(crate::launch::with_hooks(argv, file)),
        (argv, _) => argv.map(<[String]>::to_vec),
    }
}

/// The workspace that has the pane or the view, with its layout made to agree with both.
fn workspace_of<'a>(
    core: &'a mut crate::core::Core,
    leaf: &Id,
) -> Result<&'a mut crate::model::Workspace, ErrorBody> {
    let ws = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.panes.iter().any(|p| p.id == *leaf) || w.views.iter().any(|v| v.id == *leaf))
        .ok_or_else(|| no_pane(leaf))?;
    let ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
    let views: Vec<Id> = ws.views.iter().map(|v| v.id.clone()).collect();
    ws.layout = crate::layout::reconcile_views(ws.layout.take(), &ids, &views);
    Ok(ws)
}

pub fn swap_panes(shared: &Shared, a: &Id, b: &Id) -> Done {
    let mut core = shared.lock();
    let ws = workspace_of(&mut core, a)?;
    if a == b || !(ws.panes.iter().any(|p| p.id == *b) || ws.views.iter().any(|v| v.id == *b)) {
        return Err(err(
            ErrorCode::BadRequest,
            "swapping takes two different panes or views of one workspace",
        ));
    }
    ws.layout = ws.layout.take().map(|t| crate::layout::swap(t, a, b));
    core.refresh();
    Ok(None)
}

pub fn grow_pane(shared: &Shared, pane: &Id, dir: SplitDir, by: i16) -> Done {
    if !(-800..=800).contains(&by) {
        return Err(err(ErrorCode::BadRequest, "by is -800 to 800 thousandths"));
    }
    let mut core = shared.lock();
    let ws = workspace_of(&mut core, pane)?;
    let Some(tree) = ws.layout.take() else {
        return Err(err(ErrorCode::BadRequest, "the workspace has no layout"));
    };
    match crate::layout::grow(tree, pane, dir, by) {
        Ok(grown) => ws.layout = Some(grown),
        Err(unchanged) => {
            ws.layout = Some(unchanged);
            return Err(err(
                ErrorCode::BadRequest,
                "no split around the pane goes that way",
            ));
        }
    }
    core.refresh();
    Ok(None)
}

pub fn close_pane(shared: &Shared, pane: &Id) -> Done {
    let mut core = shared.lock();
    let ws = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.panes.iter().any(|p| p.id == *pane))
        .ok_or_else(|| no_pane(pane))?;
    ws.panes.retain(|p| p.id != *pane);
    core.refresh();
    Ok(None)
}

pub fn write(shared: &Shared, pane: &Id, data: &str) -> Done {
    let term = shared.lock().term(pane).ok_or_else(|| no_pane(pane))?;
    term.write(data.as_bytes())
        .map_err(|e| err(ErrorCode::Io, e.to_string()))?;
    Ok(None)
}

pub fn resize(shared: &Shared, pane: &Id, rows: u16, cols: u16) -> Done {
    if !(1..=pty::MAX_SIZE).contains(&rows) || !(1..=pty::MAX_SIZE).contains(&cols) {
        return Err(err(
            ErrorCode::BadRequest,
            format!("a pane is 1 to {} rows and columns", pty::MAX_SIZE),
        ));
    }
    let term = shared.lock().term(pane).ok_or_else(|| no_pane(pane))?;
    term.resize(rows, cols).map_err(|m| err(ErrorCode::Io, m))?;
    let mut core = shared.lock();
    core.read_screen(pane);
    core.refresh();
    Ok(None)
}

/// The variable that tells a pane's shell the folder of its workspace, which `cd @` goes to (REQ-645).
fn workspace_dir(path: &str) -> Option<(String, String)> {
    (!path.is_empty()).then(|| (identity::env_var("WORKSPACE_DIR"), path.to_owned()))
}

/// A pane a restored session remembers: its shell, the folder to start it in, what it ran and the
/// folder of its workspace.
struct Wanted {
    id: Id,
    shell: String,
    cwd: String,
    resume: Option<Vec<String>>,
    root: String,
}

/// Start the shells of the panes a restored session remembers; a pane whose shell cannot
/// start is dropped (its workspace stays).
pub fn restore(shared: &Shared) {
    let (events, shell, env, hooks, history_dir, wanted) = {
        let core = shared.lock();
        let wanted: Vec<Wanted> = core
            .model
            .workspaces
            .iter()
            .flat_map(|w| {
                w.panes.iter().map(|p| {
                    let cwd = start_folder(&p.cwd, &w.path);
                    Wanted {
                        id: p.id.clone(),
                        shell: p.shell.clone(),
                        cwd,
                        resume: p.resume.clone(),
                        root: w.path.clone(),
                    }
                })
            })
            .collect();
        (
            core.events.clone(),
            core.shell.clone(),
            core.env.clone(),
            core.agent_hooks(),
            core.history_dir.clone(),
            wanted,
        )
    };
    let mut failed = Vec::new();
    let mut resumed = Vec::new();
    for Wanted {
        id,
        shell: saved_shell,
        cwd,
        resume,
        root,
    } in wanted
    {
        let shell = if saved_shell.is_empty() {
            shell.clone()
        } else {
            saved_shell
        };
        // An agent that was running comes back with its conversation, when it has one saved;
        // anything else is a shell.
        let resume = resume.filter(|argv| {
            argv.get(2)
                .is_some_and(|id| crate::launch::transcript_exists(&env, id))
        });
        if resume.is_some() {
            resumed.push(id.clone());
        }
        let setup = shared.lock().integration_for(&shell);
        let launched = with_agent_hooks(resume.as_deref(), hooks.as_deref());
        let pane_env: Vec<_> = env.iter().cloned().chain(workspace_dir(&root)).collect();
        match pty::spawn(
            &id,
            &shell,
            Path::new(&cwd),
            pty::Program {
                argv: launched.as_deref(),
                then: None,
            },
            &pane_env,
            &setup,
            events.clone(),
        ) {
            Ok(term) => {
                // The blocks it had come back with it, ahead of what the new shell runs.
                if let Some(saved) = history_dir
                    .as_deref()
                    .and_then(|dir| crate::history::load(dir, &id))
                {
                    term.restore_blocks(saved.blocks, saved.saved_at, crate::blocks::now_ms());
                }
                let mut core = shared.lock();
                core.attach_term(&id, term);
                core.read_blocks(&id);
            }
            Err(m) => {
                eprintln!("pane {id} not restored: {m}");
                failed.push(id);
            }
        }
    }
    let mut core = shared.lock();
    for ws in &mut core.model.workspaces {
        ws.panes.retain(|p| !failed.contains(&p.id));
        // The title says what runs now; the saved one names a program that may be gone.
        for p in &mut ws.panes {
            p.title = p.name.clone().unwrap_or_else(|| basename(&p.shell));
            // What was not brought back is not waiting to be.
            if !resumed.contains(&p.id) {
                p.resume = None;
            }
        }
    }
    core.refresh();
    if let Some(dir) = history_dir {
        let live: std::collections::HashSet<&str> = core
            .model
            .workspaces
            .iter()
            .flat_map(|w| w.panes.iter().map(|p| p.id.as_str()))
            .collect();
        crate::history::prune(&dir, &live);
    }
}

/// How long the engine waits for an event before it looks at what is due to be saved.
const SAVE_TICK: Duration = Duration::from_secs(1);

/// Write the blocks of the panes that are due (spec REQ-297), with the lock let go.
fn save_histories(shared: &Shared, force: bool) {
    let writes = shared.lock().history_writes(force);
    let mut written = Vec::new();
    for (dir, pane, text) in writes {
        match crate::history::save(&dir, &pane, &text) {
            Ok(()) => written.push(pane),
            Err(e) => eprintln!("cannot save the history of pane {pane}: {e}"),
        }
    }
    if !written.is_empty() {
        shared.lock().forget_written(&written);
    }
}

/// Apply what shells report: fold bursts of output into one refresh.
pub fn engine(shared: Shared, rx: Receiver<Event>) {
    loop {
        let first = match rx.recv_timeout(SAVE_TICK) {
            Ok(first) => first,
            Err(RecvTimeoutError::Timeout) => {
                save_histories(&shared, false);
                crate::engine::save_due(&shared, false);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut batch = vec![first];
        let deadline = Instant::now() + COALESCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(ev) => batch.push(ev),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if left.is_zero() {
                break;
            }
        }
        if batch.iter().any(|e| matches!(e, Event::Stop)) {
            save_histories(&shared, true);
            return;
        }
        apply(&shared, batch);
        save_histories(&shared, false);
        crate::engine::save_due(&shared, false);
    }
}

/// For each pane with output in `batch`, where its last output is: its screen and blocks are read
/// there once, whatever came before it in the batch (spec REQ-270).
fn last_outputs(batch: &[Event]) -> HashMap<String, usize> {
    batch
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::Output { pane, .. } => Some((pane.clone(), i)),
            _ => None,
        })
        .collect()
}

fn apply(shared: &Shared, batch: Vec<Event>) {
    let mut core = shared.lock();
    let last = last_outputs(&batch);
    for (at, event) in batch.into_iter().enumerate() {
        match event {
            Event::Output { pane, marks } => {
                if last.get(&pane) == Some(&at) {
                    core.read_screen(&pane);
                    core.read_blocks(&pane);
                }
                let Some(p) = core
                    .model
                    .workspaces
                    .iter_mut()
                    .flat_map(|w| w.panes.iter_mut())
                    .find(|p| p.id == pane)
                else {
                    continue;
                };
                for mark in marks {
                    p.attention.retain(|o| o.kind != AttentionKind::ShellDone);
                    if let Mark::CommandDone(exit_code) = mark {
                        p.attention.push(Own {
                            kind: AttentionKind::ShellDone,
                            exit_code,
                            rev: None,
                        });
                    }
                }
            }
            // A shell that ends takes its pane with it, like closing a terminal; not when it is the
            // daemon that ends them, which the restart brings back.
            Event::Exited { pane } if !core.stopping => {
                for ws in &mut core.model.workspaces {
                    ws.panes.retain(|p| p.id != pane);
                }
            }
            Event::Exited { .. } => {}
            Event::Stop => {}
        }
    }
    core.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(pane: &str) -> Event {
        Event::Output {
            pane: pane.into(),
            marks: Vec::new(),
        }
    }

    /// REQ-628, REQ-629: a shell starts in the folder the pane is in, and in the workspace's own
    /// when that folder is gone (or is not one).
    #[test]
    fn a_shell_starts_where_the_pane_is_and_in_the_workspace_when_that_is_gone() {
        let dir = std::env::temp_dir().join(format!("jwn-start-{}", std::process::id()));
        let here = dir.join("here");
        std::fs::create_dir_all(&here).unwrap();
        let here = here.to_string_lossy().into_owned();
        assert_eq!(start_folder(&here, "/ws"), here);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(start_folder(&here, "/ws"), "/ws");
        assert_eq!(start_folder("", "/ws"), "/ws");
    }

    #[test]
    fn a_batch_reads_each_pane_once_at_its_last_output() {
        let batch = [
            out("a"),
            out("b"),
            out("a"),
            Event::Stop,
            out("a"),
            out("b"),
        ];
        let last = last_outputs(&batch);
        assert_eq!(last.len(), 2);
        assert_eq!((last["a"], last["b"]), (4, 5));
        assert!(last_outputs(&[Event::Stop]).is_empty());
    }
}
