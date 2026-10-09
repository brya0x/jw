//! The subcommands besides the TUI: sessions (`jw new <session>`, `jw
//! sessions`), and the ones an agent uses to see and drive the other
//! workspaces of its session (OPEN-3, REQ-66): `jw ls`, `jw read`, `jw
//! worktree`, `jw prompt`, plus `jw hook <state>` for claude's hooks. They
//! act on `$JW_SESSION` (every pane has it) and talk to the daemon like the
//! TUI does; no exit code 3.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::Rect;
use crate::proto::{AgentState, ClientMsg, DaemonMsg, PaneInfo, socket_path};
use crate::stream::Stream;

/// Panes started without a TUI get this size; the TUI resizes them when it
/// attaches.
const COLS: u16 = 160;
const ROWS: u16 = 48;

/// How long `prompt` waits for the agent to settle and take the text.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(40);

pub fn prompt(args: &[String]) -> Result<()> {
    let [name, words @ ..] = args else {
        bail!("usage: jw prompt <workspace> <text>");
    };
    let text = words.join(" ");
    if text.trim().is_empty() {
        bail!("usage: jw prompt <workspace> <text>");
    }
    let entry = find(name)?.entry;
    let mut c = Client::connect(&socket_path())
        .with_context(|| format!("{} is not open: no jw daemon is running", entry.name))?;
    send_prompt(&mut c, &entry, text.trim())?;
    println!("sent to {}'s agent", entry.name);
    Ok(())
}

/// `jw hook <state>`: what claude's hooks run in a jw pane (REQ-73). It
/// tells the daemon the agent's state and stays quiet: outside jw, or with
/// no daemon, it does nothing, so it never gets in the agent's way.
pub fn hook(args: &[String]) -> Result<()> {
    let state: AgentState = args
        .first()
        .context("usage: jw hook working|waiting|idle")?
        .parse()
        .map_err(anyhow::Error::msg)?;
    // claude writes the event as JSON on stdin; take it so it never blocks.
    // SAFETY: isatty only looks at the descriptor.
    if unsafe { libc::isatty(0) } == 0 {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    }
    let Some(pane) = std::env::var("JW_PANE_ID")
        .ok()
        .and_then(|p| p.parse().ok())
    else {
        return Ok(());
    };
    if let Ok(mut c) = Client::connect(&socket_path()) {
        let _ = c.send(&ClientMsg::Agent { pane, state });
    }
    Ok(())
}

/// `jw new <session> [--dir <folder>]`: makes the session, starting in the
/// folder (the current one by default), and returns its name for the TUI
/// to open (REQ-61).
pub fn new_session(args: &[String]) -> Result<String> {
    let mut name = None;
    let mut dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => dir = Some(it.next().context("--dir needs a folder")?.clone()),
            // RISK-24: `jw new` used to make a worktree.
            "--task" | "--branch" | "--from" => bail!(
                "jw new now starts a session; a worktree is \
                 jw worktree <name> [--branch …] [--from …] [--task …]"
            ),
            s if s.starts_with('-') => bail!("unknown flag {s}"),
            s if name.is_none() => name = Some(s.to_string()),
            s => bail!("unexpected argument {s:?}"),
        }
    }
    let name = name.context("usage: jw new <session> [--dir <folder>]")?;
    let dir = match dir {
        Some(d) => PathBuf::from(d),
        None => std::env::current_dir()?,
    };
    let dir = dir
        .canonicalize()
        .with_context(|| format!("{} is not a folder", dir.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    crate::session::create(&registry::state_dir()?, &name, &dir)?;
    Ok(name)
}

/// `jw sessions`: each session, how many workspaces it has and how many run,
/// and what its agents do.
pub fn sessions() -> Result<()> {
    let state = registry::state_dir()?;
    let last = crate::session::last(&state);
    let reg = Registry::load(&registry::default_path()?)?;
    let panes = daemon_panes();
    for name in crate::session::list(&state)? {
        let folders = crate::folders::Folders::load(
            &crate::session::dir(&state, &name)?.join("folders.json"),
        )?;
        let ids: Vec<String> = folders
            .folders
            .iter()
            .map(|f| crate::folders::id_in(&name, &f.dir))
            .chain(
                reg.entries
                    .iter()
                    .filter(|e| crate::session::owns(&name, &e.session))
                    .map(|e| e.id.clone()),
            )
            .collect();
        let mine = || panes.iter().filter(|p| ids.contains(&p.stream));
        let open: std::collections::BTreeSet<&str> = mine().map(|p| p.stream.as_str()).collect();
        let count = |s: AgentState| mine().filter(|p| p.agent == Some(s)).count();
        let mut line = format!(
            "{} {name:<24} {}, {} open",
            if name == last { "*" } else { " " },
            workspaces(ids.len()),
            open.len()
        );
        for (n, what) in [
            (count(AgentState::Working), "working"),
            (count(AgentState::Waiting), "waiting"),
        ] {
            if n > 0 {
                line.push_str(&format!(", {n} {what}"));
            }
        }
        println!("{line}");
    }
    Ok(())
}

/// "1 workspace", "3 workspaces".
pub fn workspaces(n: usize) -> String {
    if n == 1 {
        "1 workspace".into()
    } else {
        format!("{n} workspaces")
    }
}

/// `jw worktree <name>`: a worktree of the project in the current folder,
/// in the current session, opened, with a task for its agent.
pub fn worktree(args: &[String]) -> Result<()> {
    let mut o = NewOptions::default();
    let mut task = None;
    let mut into = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .with_context(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--task" => task = Some(value("--task")?),
            "--in" => into = Some(value("--in")?),
            "--branch" => o.branch = value("--branch")?,
            "--from" => o.from = value("--from")?,
            s if s.starts_with('-') => bail!("unknown flag {s}"),
            s if o.name.is_empty() => o.name = s.to_string(),
            s => bail!("unexpected argument {s:?}"),
        }
    }
    if o.name.is_empty() {
        bail!(
            "usage: jw worktree <name> [--in <project>] [--branch <branch>] [--from <ref>] \
             [--task <text>]"
        );
    }

    let project = match into {
        Some(p) => project_named(&p)?,
        None => Project::open(&std::env::current_dir()?)?,
    };
    eprintln!("creating {}/{}…", project.name, o.name);
    let entry = actions::new_stream(&project, &o, &registry::default_path()?)?;
    eprintln!(
        "created {}: branch {}, slot {}, {}",
        entry.name, entry.branch, entry.slot, entry.path
    );

    let stream = Stream::resolve(&entry)?;
    let mut c = Client::connect_or_start(&socket_path(), &std::env::current_exe()?)?;
    open(&mut c, &stream)?;
    eprintln!("opened {}: setup runs in its shell pane", entry.name);
    if let Some(task) = task {
        send_prompt(&mut c, &entry, &task)?;
        eprintln!("task sent to {}'s agent", entry.name);
    }
    Ok(())
}

/// A project of the session by name: an open folder that is its checkout,
/// or the main checkout its worktrees came from.
fn project_named(name: &str) -> Result<Project> {
    let all = session_workspaces()?;
    let dir = all
        .iter()
        .find(|w| !w.worktree && w.entry.project == name && !w.entry.branch.is_empty())
        .map(|w| w.entry.path.clone())
        .or_else(|| {
            all.iter()
                .find(|w| w.worktree && w.entry.project == name && !w.entry.root.is_empty())
                .map(|w| w.entry.root.clone())
        })
        .with_context(|| format!("no project {name} in session {}", crate::session::current()))?;
    Project::open(Path::new(&dir))
}

/// Starts the stream's panes in the daemon, sized for the default layout.
fn open(c: &mut Client, stream: &Stream) -> Result<()> {
    let (specs, note) = stream.open_specs(true)?;
    if let Some(n) = note {
        eprintln!("{n}");
    }
    let rects = stream.tree.rects(Rect {
        x: 0,
        y: 0,
        w: COLS,
        h: ROWS,
    });
    let roles: Vec<String> = stream.tree.leaves().into_iter().map(|l| l.role).collect();
    let mut want = specs.len();
    for spec in specs {
        let size = roles
            .iter()
            .position(|r| *r == spec.role)
            .map(|i| (rects[i].w.saturating_sub(2), rects[i].h.saturating_sub(2)))
            .unwrap_or((80, 24));
        c.send(&spec.spawn(&stream.entry.id, size))?;
    }
    c.set_read_timeout(Some(Duration::from_secs(10)))?;
    while want > 0 {
        match c.recv()?.context("the daemon hung up")? {
            DaemonMsg::Spawned { .. } => want -= 1,
            DaemonMsg::Error { msg } => bail!("{msg}"),
            _ => {}
        }
    }
    stream.mark_opened()
}

fn send_prompt(c: &mut Client, entry: &Entry, text: &str) -> Result<()> {
    c.send(&ClientMsg::Prompt {
        stream: entry.id.clone(),
        text: text.to_string(),
    })?;
    let deadline = Instant::now() + PROMPT_TIMEOUT;
    c.set_read_timeout(Some(PROMPT_TIMEOUT))?;
    loop {
        if Instant::now() > deadline {
            bail!("{}'s agent never took the prompt", entry.name);
        }
        match c.recv()?.context("the daemon hung up")? {
            DaemonMsg::Prompted { .. } => return Ok(()),
            DaemonMsg::Error { msg } => bail!("{}: {msg}", entry.name),
            _ => {}
        }
    }
}

/// One workspace of the session.
struct Ws {
    entry: Entry,
    /// `name` for a folder, `project/name` for a worktree.
    label: String,
    worktree: bool,
}

/// The session's workspaces, as its sidebar lists them: its folders, then
/// its worktrees.
fn session_workspaces() -> Result<Vec<Ws>> {
    let state = registry::state_dir()?;
    let session = crate::session::current();
    let mut out: Vec<Ws> = crate::folders::load(&state)?
        .folders
        .iter()
        .map(|f| {
            let entry = crate::folders::entry_for(f, &session);
            Ws {
                label: entry.name.clone(),
                entry,
                worktree: false,
            }
        })
        .collect();
    let reg = Registry::load(&registry::default_path()?)?;
    out.extend(
        reg.entries
            .into_iter()
            .filter(|e| crate::session::owns(&session, &e.session))
            .map(|e| Ws {
                label: format!("{}/{}", e.project, e.name),
                entry: e,
                worktree: true,
            }),
    );
    Ok(out)
}

/// A workspace of the session by `project/name`, name or id. When several
/// projects have a worktree with that name, the project of the current
/// directory (or `$JW_PROJECT`, set in every pane) decides.
fn find(name: &str) -> Result<Ws> {
    let all = session_workspaces()?;
    let mut matches: Vec<Ws> = all
        .into_iter()
        .filter(|w| w.label == name || w.entry.name == name || w.entry.id == name)
        .collect();
    if matches.len() > 1 {
        let here = std::env::var("JW_PROJECT").ok().or_else(|| {
            let dir: PathBuf = std::env::current_dir().ok()?;
            Project::open(Path::new(&dir)).ok().map(|p| p.name)
        });
        if let Some(i) = matches
            .iter()
            .position(|w| w.worktree && Some(&w.entry.project) == here.as_ref())
        {
            return Ok(matches.swap_remove(i));
        }
        let all: Vec<&str> = matches.iter().map(|w| w.label.as_str()).collect();
        bail!("{name} is ambiguous: {}", all.join(", "));
    }
    matches.pop().with_context(|| {
        format!(
            "no workspace {name} in session {} (jw ls lists them)",
            crate::session::current()
        )
    })
}

/// Every pane the daemon runs; none when no daemon is running.
fn daemon_panes() -> Vec<PaneInfo> {
    Client::connect(&socket_path())
        .ok()
        .and_then(|mut c| {
            c.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
            c.send(&ClientMsg::List).ok()?;
            loop {
                match c.recv().ok()?? {
                    DaemonMsg::Panes { panes } => return Some(panes),
                    DaemonMsg::Error { .. } => return None,
                    _ => {}
                }
            }
        })
        .unwrap_or_default()
}

/// What a workspace's panes say: its agent's state and the sidebar marks.
fn state_of(panes: &[&PaneInfo]) -> (&'static str, String) {
    let guess = |p: &&&PaneInfo| p.agent.is_none() && p.role == "agent";
    let working = panes
        .iter()
        .any(|p| p.agent == Some(AgentState::Working) || (guess(&p) && p.busy));
    let waiting = panes
        .iter()
        .any(|p| p.agent == Some(AgentState::Waiting) || (guess(&p) && p.bell && !p.busy));
    let mut marks = String::new();
    if working {
        marks.push('✻');
    }
    if waiting {
        marks.push('?');
    }
    if panes
        .iter()
        .any(|p| p.role.starts_with("dev:") && p.exited.is_none())
    {
        marks.push('⚡');
    }
    let state = if panes.is_empty() {
        "closed"
    } else if waiting {
        "waiting"
    } else if working {
        "working"
    } else {
        "idle"
    };
    (state, marks)
}

/// `jw ls [--json]`: the session's workspaces, whether they run, what their
/// agents do (REQ-66).
pub fn ls(args: &[String]) -> Result<()> {
    let json = match args {
        [] => false,
        [f] if f == "--json" => true,
        _ => bail!("usage: jw ls [--json]"),
    };
    let panes = daemon_panes();
    let rows: Vec<serde_json::Value> = session_workspaces()?
        .into_iter()
        .map(|w| {
            let mine: Vec<&PaneInfo> = panes.iter().filter(|p| p.stream == w.entry.id).collect();
            let (state, marks) = state_of(&mine);
            serde_json::json!({
                "name": w.label,
                "id": w.entry.id,
                "kind": if w.worktree { "worktree" } else { "folder" },
                "path": w.entry.path,
                "branch": w.entry.branch,
                "pr": (w.entry.pr > 0).then_some(w.entry.pr),
                "state": state,
                "marks": marks,
                "panes": mine.iter().map(|p| p.role.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for r in &rows {
        let pr = r["pr"]
            .as_u64()
            .map(|n| format!("  PR #{n}"))
            .unwrap_or_default();
        println!(
            "{:<32} {:<8} {:<3} {}{pr}",
            r["name"].as_str().unwrap_or_default(),
            r["state"].as_str().unwrap_or_default(),
            r["marks"].as_str().unwrap_or_default(),
            r["branch"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

/// `jw read <workspace> [--pane <role>] [--lines N]`: the last lines of a
/// pane (the agent's by default), scrollback included, as text (REQ-66).
pub fn read(args: &[String]) -> Result<()> {
    let mut name = None;
    let mut role = None;
    let mut lines = 40usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pane" => role = Some(it.next().context("--pane needs a role")?.clone()),
            "--lines" => {
                lines = it
                    .next()
                    .context("--lines needs a number")?
                    .parse()
                    .context("--lines needs a number")?
            }
            s if s.starts_with('-') => bail!("unknown flag {s}"),
            s if name.is_none() => name = Some(s.to_string()),
            s => bail!("unexpected argument {s:?}"),
        }
    }
    let name = name.context("usage: jw read <workspace> [--pane <role>] [--lines N]")?;
    let ws = find(&name)?;
    let panes = daemon_panes();
    let mine: Vec<&PaneInfo> = panes.iter().filter(|p| p.stream == ws.entry.id).collect();
    if mine.is_empty() {
        bail!("{} is not open", ws.label);
    }
    let pane = match &role {
        Some(r) => mine.iter().find(|p| p.role == *r),
        None => mine
            .iter()
            .find(|p| p.role == "agent")
            .or_else(|| mine.first()),
    }
    .with_context(|| {
        let roles: Vec<&str> = mine.iter().map(|p| p.role.as_str()).collect();
        format!("{} has no pane {:?}: {}", ws.label, role, roles.join(", "))
    })?
    .pane;

    let mut c = Client::connect(&socket_path())?;
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    c.send(&ClientMsg::Attach {
        stream: ws.entry.id.clone(),
    })?;
    let (rows, cols, bytes) = loop {
        match c.recv()?.context("the daemon hung up")? {
            DaemonMsg::Snapshot {
                pane: p,
                rows,
                cols,
                bytes,
                ..
            } if p == pane => break (rows, cols, bytes),
            DaemonMsg::Error { msg } => bail!("{msg}"),
            _ => {}
        }
    };
    let _ = c.send(&ClientMsg::Detach);
    for line in text_of(rows, cols, &bytes, lines) {
        println!("{line}");
    }
    Ok(())
}

/// The last `n` lines a Snapshot shows, scrollback first, without the
/// empty lines at the end.
fn text_of(rows: u16, cols: u16, bytes: &[u8], n: usize) -> Vec<String> {
    let mut p = vt100::Parser::new(rows.max(1), cols.max(1), 10_000);
    p.process(bytes);
    p.screen_mut().set_scrollback(usize::MAX);
    let back = p.screen().scrollback();
    let mut all = vec![String::new(); back + rows as usize];
    let mut offset = back;
    loop {
        p.screen_mut().set_scrollback(offset);
        for (i, row) in p.screen().rows(0, cols).enumerate() {
            all[back - offset + i] = row.trim_end().to_string();
        }
        if offset == 0 {
            break;
        }
        offset = offset.saturating_sub(rows as usize);
    }
    while all.last().is_some_and(|l| l.is_empty()) {
        all.pop();
    }
    let skip = all.len().saturating_sub(n);
    all.split_off(skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_takes_the_last_lines_with_the_scrollback() {
        let out: String = (1..=200).map(|i| format!("{i}\r\n")).collect();
        assert_eq!(text_of(24, 80, out.as_bytes(), 3), ["198", "199", "200"]);
        let all = text_of(24, 80, out.as_bytes(), 1000);
        assert_eq!(all.len(), 200);
        assert_eq!(all[0], "1");
        assert_eq!(all[99], "100");
    }
}
