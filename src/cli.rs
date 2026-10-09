//! The subcommands besides the TUI: sessions (`jw new <session>`, `jw
//! sessions`), and the ones for agents running inside a workspace (OPEN-3):
//! `jw worktree <name> [--task <text>]`, `jw prompt <stream> <text>`, and
//! `jw hook <state>` for claude's hooks. They talk to the daemon like the
//! TUI does; no exit code 3.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::Rect;
use crate::proto::{AgentState, ClientMsg, DaemonMsg, socket_path};
use crate::stream::Stream;

/// Panes started without a TUI get this size; the TUI resizes them when it
/// attaches.
const COLS: u16 = 160;
const ROWS: u16 = 48;

/// How long `prompt` waits for the agent to settle and take the text.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(40);

pub fn prompt(args: &[String]) -> Result<()> {
    let [name, words @ ..] = args else {
        bail!("usage: jw prompt <stream> <text>");
    };
    let text = words.join(" ");
    if text.trim().is_empty() {
        bail!("usage: jw prompt <stream> <text>");
    }
    let entry = find(name)?;
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
    let panes = Client::connect(&socket_path())
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
        .unwrap_or_default();
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .with_context(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--task" => task = Some(value("--task")?),
            "--branch" => o.branch = value("--branch")?,
            "--from" => o.from = value("--from")?,
            s if s.starts_with('-') => bail!("unknown flag {s}"),
            s if o.name.is_empty() => o.name = s.to_string(),
            s => bail!("unexpected argument {s:?}"),
        }
    }
    if o.name.is_empty() {
        bail!("usage: jw worktree <name> [--branch <branch>] [--from <ref>] [--task <text>]");
    }

    let project = Project::open(&std::env::current_dir()?)?;
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

/// A stream by name. When several projects have one with that name, the
/// project of the current directory (or `$JW_PROJECT`, set in every pane)
/// decides.
fn find(name: &str) -> Result<Entry> {
    let reg = Registry::load(&registry::default_path()?)?;
    let matches: Vec<&Entry> = reg.entries.iter().filter(|e| e.name == name).collect();
    match matches.as_slice() {
        [] => bail!("no stream named {name}"),
        [one] => Ok((*one).clone()),
        many => {
            let here = std::env::var("JW_PROJECT").ok().or_else(|| {
                let dir: PathBuf = std::env::current_dir().ok()?;
                Project::open(Path::new(&dir)).ok().map(|p| p.name)
            });
            if let Some(e) = many.iter().find(|e| Some(&e.project) == here.as_ref()) {
                return Ok((*e).clone());
            }
            let projects: Vec<&str> = many.iter().map(|e| e.project.as_str()).collect();
            bail!(
                "{name} exists in {}: run this from inside one of those projects",
                projects.join(", ")
            )
        }
    }
}
