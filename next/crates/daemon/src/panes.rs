//! Panes: terminals inside a workspace, and the engine that turns shell events into
//! model changes.

use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use contract::{AttentionKind, ErrorCode, Id};
use serde_json::json;

use crate::commands::{Done, err};
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

/// Open a pane; with `program`, the shell starts it and stays when it ends. An agent gets
/// its conversation id here, so a restart can resume it.
pub fn new_pane(shared: &Shared, workspace: &Id, program: Option<Vec<String>>) -> Done {
    if program.as_ref().is_some_and(|p| p.is_empty()) {
        return Err(err(ErrorCode::BadRequest, "program is empty"));
    }
    let argv = program.map(|p| crate::launch::with_session(&p, new_id));
    let (path, events, shell, env, setup) = {
        let core = shared.lock();
        let ws = core
            .model
            .workspaces
            .iter()
            .find(|w| w.id == *workspace)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
        (
            ws.path.clone(),
            core.events.clone(),
            core.shell.clone(),
            core.env.clone(),
            core.integration_for(&core.shell),
        )
    };
    let id = new_id();
    let term = pty::spawn(
        &id,
        &shell,
        Path::new(&path),
        argv.as_deref(),
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
    ws.panes.push(Pane {
        id: id.clone(),
        title: basename(&shell),
        shell,
        cwd: path,
        attention: Vec::new(),
        running: None,
        last_program: None,
        resume: argv.as_deref().and_then(crate::launch::agent_resume),
    });
    core.attach_term(&id, term);
    core.refresh();
    Ok(Some(json!({ "pane": id })))
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

/// Start the shells of the panes a restored session remembers; a pane whose shell cannot
/// start is dropped (its workspace stays).
pub fn restore(shared: &Shared) {
    let (events, shell, env, wanted) = {
        let core = shared.lock();
        let wanted: Vec<(Id, String, String, Option<Vec<String>>)> = core
            .model
            .workspaces
            .iter()
            .flat_map(|w| {
                w.panes.iter().map(|p| {
                    (
                        p.id.clone(),
                        p.shell.clone(),
                        p.cwd.clone(),
                        p.resume.clone(),
                    )
                })
            })
            .collect();
        (
            core.events.clone(),
            core.shell.clone(),
            core.env.clone(),
            wanted,
        )
    };
    let mut failed = Vec::new();
    let mut resumed = Vec::new();
    for (id, saved_shell, cwd, resume) in wanted {
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
        match pty::spawn(
            &id,
            &shell,
            Path::new(&cwd),
            resume.as_deref(),
            &env,
            &setup,
            events.clone(),
        ) {
            Ok(term) => shared.lock().attach_term(&id, term),
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
            p.title = basename(&p.shell);
            // What was not brought back is not waiting to be.
            if !resumed.contains(&p.id) {
                p.resume = None;
            }
        }
    }
    core.refresh();
}

/// Apply what shells report: fold bursts of output into one refresh.
pub fn engine(shared: Shared, rx: Receiver<Event>) {
    while let Ok(first) = rx.recv() {
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
            return;
        }
        apply(&shared, batch);
    }
}

fn apply(shared: &Shared, batch: Vec<Event>) {
    let mut core = shared.lock();
    for event in batch {
        match event {
            Event::Output { pane, marks } => {
                core.read_screen(&pane);
                core.read_blocks(&pane);
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
            // A shell that ends takes its pane with it, like closing a terminal.
            Event::Exited { pane } => {
                for ws in &mut core.model.workspaces {
                    ws.panes.retain(|p| p.id != pane);
                }
            }
            Event::Stop => {}
        }
    }
    core.refresh();
}
