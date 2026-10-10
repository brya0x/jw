//! The client: the sidebar of workspaces, the current one's panes in their
//! splits, and a one-shot leader key for every action (docs/specs/rust-tui.md).
//!
//! Two threads feed one channel: the daemon's messages and the terminal's
//! events. The main thread applies them and redraws.

mod diffview;
mod draw;
mod finder;
mod keys;
mod mdview;
mod modal;
mod settings_view;

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::connectors::PullRequests;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::{Dir, Rect, Tree};
use crate::proto::{
    AgentState, ClientMsg, DaemonMsg, NewPane, PROTOCOL, PaneId, PaneInfo, PaneLeaf,
};
use crate::stream::Stream;

use modal::{Modal, Outcome};

use keys::Leader;

/// Width of the sidebar, borders included.
const SIDEBAR: u16 = 28;

/// How long the leader waits before showing every key (REQ-31).
fn which_after() -> Duration {
    crate::settings::get().which_delay()
}

enum Msg {
    Daemon(DaemonMsg),
    DaemonGone,
    Term(Event),
    /// A slow action finished on its worker thread. Boxed: output messages
    /// go through the same channel by the thousand and stay small.
    Job(Box<Job>),
    /// The system switched between light and dark.
    Theme,
    /// Twice a second: old status messages go, the panes' state and the
    /// PRs are asked for again.
    Tick,
    /// The pull requests of the worktrees, by workspace id.
    Prs(Vec<(String, Option<crate::connectors::Pr>)>),
}

// One job at a time crosses the channel, boxed; its size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Job {
    Created {
        entry: Entry,
        setup: bool,
    },
    /// done's checks passed: ask before deleting.
    DoneReady {
        entry: Entry,
        project: Project,
        plan: actions::RmPlan,
        pr: crate::connectors::Pr,
    },
    /// A finished action with nothing else to do but say so.
    Said(String),
    Diff {
        title: String,
        dir: String,
        files: Vec<crate::diff::File>,
        /// The viewer pane to fill, when it is already in the tree (a
        /// client attaching to a workspace that shows one).
        into: Option<PaneId>,
    },
    /// A worktree has its new name; open it again.
    Renamed(Entry),
    /// `X` on a worktree whose PR isn't merged: the rm checks, and why.
    RmReady {
        entry: Entry,
        project: Project,
        plan: actions::RmPlan,
        note: Option<String>,
    },
    Removed {
        name: String,
        note: Option<String>,
    },
    Failed(String),
}

/// What a viewer pane shows (REQ-57).
pub enum View {
    Diff(diffview::DiffView),
    Md(mdview::MdView),
}

/// One row of the sidebar: a workspace, and whether it is a worktree
/// listed under its project (REQ-50).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub entry: Entry,
    pub child: bool,
}

/// How a status message reads: green when something was done, red when it
/// failed or was refused (REQ-54).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Done,
    Error,
}

pub struct Status {
    pub text: String,
    pub tone: Tone,
    at: Instant,
}

/// Lines of history a pane keeps here, for the wheel: what arrived since
/// this client attached.
const SCROLLBACK: usize = 5_000;

/// Rows the wheel moves at a time.
const WHEEL: usize = 3;

/// How long a status message stays.
const STATUS_FOR: Duration = Duration::from_secs(4);

/// A pane of the current workspace as this client sees it.
pub struct PaneView {
    pub role: String,
    pub parser: vt100::Parser,
    pub exited: Option<i32>,
    /// The window title its program set (OSC 0/2).
    pub title: Option<String>,
}

pub struct App {
    tx: Client,
    pub leader: Leader,
    /// When the leader was pressed; the next key is an action (REQ-30).
    leader_at: Option<Instant>,
    /// The popup with every key (REQ-31).
    pub which: bool,
    pub rows: Vec<Row>,
    /// Workspace ids by last use, most recent first (`recent.json`).
    recent: Vec<String>,
    /// Every pane the daemon has, from the last `List`.
    pub daemon_panes: Vec<PaneInfo>,
    /// The stream on screen, by registry id.
    pub active: Option<Stream>,
    pub panes: BTreeMap<PaneId, PaneView>,
    /// How the current workspace's panes are laid out, as the daemon keeps it.
    pub tree: Option<Tree<PaneLeaf>>,
    pub focus: Option<PaneId>,
    /// `t` was pressed: focus the pane the next tree brings.
    focus_new: bool,
    /// A border being dragged with the mouse.
    dragging: Option<crate::layout::Border>,
    /// `f`: only the focused pane, across the whole terminal.
    pub full: bool,
    pub status: Option<Status>,
    /// The terminal's size, for the layout.
    pub size: (u16, u16),
    quit: bool,
    pub modal: Option<Modal>,
    /// A picker over everything else (`^␣ o`).
    pub finder: Option<finder::Finder>,
    /// Where each pane of the workspace is: its directory and that
    /// directory's git checkout, for the header.
    pub here: BTreeMap<PaneId, Here>,
    /// The settings screen, while it is open (REQ-67).
    pub settings: Option<settings_view::SettingsView>,
    /// Each worktree's pull request, from the last look (REQ-51).
    pub prs: BTreeMap<String, crate::connectors::Pr>,
    ticks: u64,
    /// The theme the daemon was last told (REQ-85).
    told: Option<crate::proto::Theme>,
    /// The viewer panes' contents, by pane id.
    pub views: BTreeMap<PaneId, View>,
    /// A viewer waiting for its pane: it fills the next viewer leaf the
    /// tree brings.
    pending_view: Option<View>,
    /// The action running in the background, for the status bar.
    pub busy: Option<String>,
    /// For worker threads to report back.
    events: Sender<Msg>,
    /// Why the client left, when it wasn't the user's `q`.
    exit_reason: Option<String>,
    /// The settings and theme files, to apply their changes.
    watch: crate::settings::Watch,
    /// Just started, or just switched session: with nothing running, the
    /// next List opens the session's last workspace.
    fresh: bool,
}

/// Runs the TUI until the user leaves it. The daemon keeps every pane.
pub fn run() -> Result<()> {
    let socket = crate::proto::socket_path();
    let exe = std::env::current_exe()?;
    let client = Client::connect_or_start(&socket, &exe)?;
    hello(&client, &socket)?;
    let mut reader = client.try_clone()?;

    let (tx, rx) = mpsc::channel();
    let to_main = tx.clone();
    thread::spawn(move || {
        loop {
            match reader.recv() {
                Ok(Some(m)) => {
                    if to_main.send(Msg::Daemon(m)).is_err() {
                        return;
                    }
                }
                _ => {
                    let _ = to_main.send(Msg::DaemonGone);
                    return;
                }
            }
        }
    });
    let events = tx.clone();
    spawn_term_reader(tx);

    follow_theme(events.clone());
    let ticks = events.clone();
    thread::spawn(move || {
        while ticks.send(Msg::Tick).is_ok() {
            thread::sleep(Duration::from_millis(500));
        }
    });

    let leader = leader();
    let theme_error = crate::theme::load(&crate::settings::get()).err();

    log_panics();
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let size = terminal.size()?;
    let mut app = App::new(client, events, leader, (size.width, size.height))?;
    if let Some(e) = theme_error {
        app.fail(format!("{e:#}"));
    }
    let result = app.event_loop(&mut terminal, rx);
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
    match (result, app.exit_reason) {
        (Err(e), _) => Err(e),
        (Ok(()), Some(why)) => anyhow::bail!(why),
        (Ok(()), None) => Ok(()),
    }
}

/// The leader: `$JW_LEADER`, else the settings', else Ctrl-Space.
fn leader() -> Leader {
    std::env::var("JW_LEADER")
        .ok()
        .or_else(|| crate::settings::get().leader.clone())
        .and_then(|l| Leader::parse(&l))
        .unwrap_or(Leader::DEFAULT)
}

/// Dark or light (REQ-43): `$JW_THEME` or the settings pin one; otherwise
/// the system's appearance, checked again every few seconds.
fn dark_now() -> bool {
    crate::theme::pinned().unwrap_or_else(crate::theme::system_dark)
}

fn follow_theme(tx: Sender<Msg>) {
    crate::theme::set_dark(dark_now());
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(3));
            if crate::theme::set_dark(dark_now()) && tx.send(Msg::Theme).is_err() {
                return;
            }
        }
    });
}

/// Checks that the daemon speaks this build's protocol (RISK-14). An older
/// daemon can't read `Hello` at all and answers with an error.
fn hello(client: &Client, socket: &std::path::Path) -> Result<()> {
    let mut c = client.try_clone()?;
    c.set_read_timeout(Some(Duration::from_secs(3)))?;
    c.send(&ClientMsg::Hello { protocol: PROTOCOL })?;
    let answer = c.recv();
    c.set_read_timeout(None)?;
    match answer {
        Ok(Some(DaemonMsg::Hello { protocol })) if protocol == PROTOCOL => Ok(()),
        _ => anyhow::bail!(
            "the jw daemon running on {} is from another build of jw. \
             Stop it (jw server stop) and start jw again; its panes close with it.",
            socket.display()
        ),
    }
}

/// Appends panics to client.log in the state dir: the terminal that shows
/// them is often gone by the time anyone looks (a window that closes on exit).
fn log_panics() {
    let Ok(dir) = registry::state_dir() else {
        return;
    };
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("client.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "panic: {info}\n{bt}");
        }
        prev(info);
    }));
}

fn spawn_term_reader(tx: Sender<Msg>) {
    thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Term(ev)).is_err() {
                return;
            }
        }
    });
}

impl App {
    fn new(tx: Client, events: Sender<Msg>, leader: Leader, size: (u16, u16)) -> Result<Self> {
        let mut app = Self {
            tx,
            leader,
            leader_at: None,
            which: false,
            rows: Vec::new(),
            recent: load_recent(),
            daemon_panes: Vec::new(),
            active: None,
            panes: BTreeMap::new(),
            tree: None,
            focus: None,
            focus_new: false,
            dragging: None,
            full: false,
            status: None,
            size,
            quit: false,
            modal: None,
            finder: None,
            settings: None,
            here: BTreeMap::new(),
            prs: BTreeMap::new(),
            ticks: 0,
            told: None,
            views: BTreeMap::new(),
            pending_view: None,
            busy: None,
            events,
            exit_reason: None,
            watch: crate::settings::Watch::new(),
            fresh: true,
        };
        app.reload()?;
        app.send(ClientMsg::List);
        Ok(app)
    }

    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        rx: Receiver<Msg>,
    ) -> Result<()> {
        terminal.draw(|f| draw::draw(f, self))?;
        while !self.quit {
            let first = match self.leader_at {
                Some(at) if !self.which => {
                    match rx.recv_timeout(which_after().saturating_sub(at.elapsed())) {
                        Ok(m) => m,
                        Err(RecvTimeoutError::Timeout) => {
                            self.which = true;
                            terminal.draw(|f| draw::draw(f, self))?;
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            anyhow::bail!("event channels closed")
                        }
                    }
                }
                _ => rx.recv().context("event channels closed")?,
            };
            self.handle(first);
            // Apply everything already queued before drawing once: a burst
            // of output costs one frame, not one per chunk.
            while let Ok(m) = rx.recv_timeout(Duration::from_millis(2)) {
                self.handle(m);
                if self.quit {
                    break;
                }
            }
            terminal.draw(|f| draw::draw(f, self))?;
        }
        Ok(())
    }

    /// REQ-85: the daemon answers the programs in panes with the colours
    /// they sit on, so it hears of every switch (by the system, the
    /// settings or a theme file).
    fn tell_theme(&mut self) {
        let theme = crate::theme::wire();
        if self.told != Some(theme) {
            self.told = Some(theme);
            self.send(ClientMsg::Theme(theme));
        }
    }

    fn send(&mut self, msg: ClientMsg) {
        if let Err(e) = self.tx.send(&msg) {
            self.fail(format!("daemon: {e}"));
        }
    }

    /// Rebuilds the sidebar from the registry: one space per project.
    fn reload(&mut self) -> Result<()> {
        let state = registry::state_dir()?;
        let folders = crate::folders::load(&state)?;
        let reg_path = registry::default_path()?;
        let mut reg = Registry::load(&reg_path)?;
        if actions::fill_roots(&mut reg) {
            reg.save(&reg_path)?;
        }
        let mut groups: Vec<(Entry, Vec<Entry>)> = folders
            .folders
            .iter()
            .map(|f| {
                (
                    crate::folders::entry_for(f, &crate::session::current()),
                    Vec::new(),
                )
            })
            .collect();
        let mut by_project: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        let session = crate::session::current();
        for e in reg
            .entries
            .into_iter()
            .filter(|e| crate::session::owns(&session, &e.session))
        {
            by_project.entry(e.project.clone()).or_default().push(e);
        }
        for (project, mut worktrees) in by_project {
            worktrees.sort_by(|a, b| a.name.cmp(&b.name));
            let root = worktrees
                .iter()
                .find(|w| !w.root.is_empty())
                .map(|w| w.root.clone());
            let at = groups.iter().position(|(r, _)| {
                !r.branch.is_empty() && (r.project == project || Some(&r.path) == root.as_ref())
            });
            match at {
                Some(i) => groups[i].1 = worktrees,
                None => {
                    // Worktrees whose project folder isn't open: the folder
                    // shows closed, above them.
                    let mut e = match &root {
                        Some(dir) => crate::folders::entry(dir, false),
                        None => Entry {
                            id: crate::folders::id(&project),
                            name: project.clone(),
                            ..Entry::default()
                        },
                    };
                    e.project = project;
                    groups.push((e, worktrees));
                }
            }
        }
        self.rows = groups
            .into_iter()
            .flat_map(|(root, kids)| {
                std::iter::once(Row {
                    entry: root,
                    child: false,
                })
                .chain(kids.into_iter().map(|entry| Row { entry, child: true }))
            })
            .collect();
        // The workspace on screen keeps its latest name and branch.
        if let Some(s) = &mut self.active
            && let Some(row) = self.rows.iter().find(|r| r.entry.id == s.entry.id)
        {
            s.entry = row.entry.clone();
        }
        Ok(())
    }

    /// Looks up the PR of every worktree that is open, off the main thread
    /// and without a word in the status bar (RISK-19).
    fn poll_prs(&mut self) {
        let worktrees: Vec<(String, String, String)> = self
            .rows
            .iter()
            .filter(|r| r.child && self.is_open(&r.entry.id))
            .map(|r| {
                (
                    r.entry.id.clone(),
                    r.entry.path.clone(),
                    r.entry.branch.clone(),
                )
            })
            .collect();
        if worktrees.is_empty() {
            return;
        }
        let tx = self.events.clone();
        thread::spawn(move || {
            use crate::connectors::PullRequests;
            let gh = crate::connectors::github::Client::default();
            let prs = worktrees
                .into_iter()
                .filter_map(|(id, path, branch)| {
                    let pr = gh.for_branch(std::path::Path::new(&path), &branch).ok()?;
                    Some((id, pr))
                })
                .collect();
            let _ = tx.send(Msg::Prs(prs));
        });
    }

    /// The marks a workspace's row carries: `✻` its agent prints, `?` its
    /// agent rang, `⚡` a dev server runs, `⚑` its PR is merged (REQ-51).
    pub fn marks(&self, id: &str) -> Vec<char> {
        let panes = || self.daemon_panes.iter().filter(|p| p.stream == id);
        // The agent's hooks say what it does (REQ-73); without them, output
        // and the bell are the guess (REQ-51).
        let guess = |p: &PaneInfo| p.agent.is_none() && p.role == "agent";
        let mut out = Vec::new();
        if panes().any(|p| p.agent == Some(AgentState::Working) || (guess(p) && p.busy)) {
            out.push('✻');
        }
        if panes().any(|p| p.agent == Some(AgentState::Waiting) || (guess(p) && p.bell && !p.busy))
        {
            out.push('?');
        }
        if panes().any(|p| p.role.starts_with("dev:") && p.exited.is_none()) {
            out.push('⚡');
        }
        if self.prs.get(id).is_some_and(|pr| pr.state == "MERGED") {
            out.push('⚑');
        }
        out
    }

    fn say(&mut self, text: String) {
        self.tell(text, Tone::Info);
    }

    fn done(&mut self, text: String) {
        self.tell(text, Tone::Done);
    }

    fn fail(&mut self, text: String) {
        self.tell(text, Tone::Error);
    }

    fn tell(&mut self, text: String, tone: Tone) {
        self.status = Some(Status {
            text,
            tone,
            at: Instant::now(),
        });
    }

    /// The viewer in the focused pane, if it is one.
    pub fn focused_view(&self) -> Option<&View> {
        self.focus.and_then(|f| self.views.get(&f))
    }

    /// Whether the leader was pressed and the next key is an action.
    pub fn leading(&self) -> bool {
        self.leader_at.is_some()
    }

    /// The workspace on screen: every action applies to it.
    pub fn current(&self) -> Option<&Entry> {
        self.active.as_ref().map(|s| &s.entry)
    }

    /// The sidebar's workspaces in order; `^␣ 1–9` picks from these.
    pub fn workspaces(&self) -> impl Iterator<Item = &Entry> {
        self.rows.iter().map(|r| &r.entry)
    }

    /// Whether the daemon runs panes for this stream.
    pub fn is_open(&self, id: &str) -> bool {
        self.daemon_panes.iter().any(|p| p.stream == id)
    }

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Daemon(d) => self.on_daemon(d),
            Msg::DaemonGone => {
                self.fail("the daemon closed the connection".into());
                self.exit_reason = Some("the daemon closed the connection".into());
                self.quit = true;
            }
            Msg::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            Msg::Term(Event::Paste(text)) => self.paste(&text),
            Msg::Term(Event::Mouse(m)) => self.on_mouse(m),
            Msg::Term(Event::Resize(w, h)) => {
                self.size = (w, h);
                self.fit();
            }
            Msg::Term(_) => {}
            Msg::Tick => {
                self.tell_theme();
                if self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.at.elapsed() > STATUS_FOR)
                {
                    self.status = None;
                }
                // Every 2 s the panes' state (✻ ? ⚡), every minute the PRs.
                if self.ticks.is_multiple_of(4) {
                    self.send(ClientMsg::List);
                    // REQ-69: settings and themes edited anywhere apply.
                    match self.watch.check() {
                        Ok(true) => {
                            self.leader = leader();
                            crate::theme::set_dark(dark_now());
                        }
                        Ok(false) => {}
                        Err(e) => self.fail(format!("settings: {e:#}")),
                    }
                }
                if self.ticks.is_multiple_of(120) {
                    self.poll_prs();
                }
                self.ticks += 1;
            }
            Msg::Prs(prs) => {
                for (id, pr) in prs {
                    match pr {
                        Some(pr) => {
                            self.prs.insert(id, pr);
                        }
                        None => {
                            self.prs.remove(&id);
                        }
                    }
                }
            }
            Msg::Theme => {
                for v in self.views.values_mut() {
                    if let View::Md(m) = v {
                        m.restyle();
                    }
                }
            }
            Msg::Job(j) => self.on_job(*j),
        }
    }

    fn on_daemon(&mut self, d: DaemonMsg) {
        match d {
            DaemonMsg::Snapshot {
                pane,
                role,
                cols,
                rows,
                bytes,
            } => {
                let mut parser = vt100::Parser::new(rows, cols, SCROLLBACK);
                parser.process(&bytes);
                self.panes.insert(
                    pane,
                    PaneView {
                        role,
                        parser,
                        exited: None,
                        title: None,
                    },
                );
                self.fit();
            }
            DaemonMsg::Output { pane, bytes } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.parser.process(&bytes);
                }
            }
            DaemonMsg::Tree { stream, tree } => {
                if self.current().is_some_and(|e| e.id == stream) {
                    self.set_tree(tree);
                }
                self.send(ClientMsg::List);
            }
            DaemonMsg::Title { pane, title } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.title = Some(title);
                }
            }
            DaemonMsg::Hello { .. } | DaemonMsg::Spawned { .. } => {}
            DaemonMsg::Exited { pane, status } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.exited = Some(status);
                }
                self.send(ClientMsg::List);
            }
            DaemonMsg::Panes { panes } => {
                self.daemon_panes = panes;
                self.find_here();
                // Nothing on screen (just started, or the current one was
                // closed): show the last used workspace that is running,
                // else any running one; on a fresh start with none, the
                // last used one (a new session's folder).
                let fresh = std::mem::take(&mut self.fresh);
                if self.active.is_none() && self.modal.is_none() {
                    let recent: Vec<Entry> = self
                        .recent
                        .iter()
                        .filter_map(|id| self.workspaces().find(|e| e.id == *id).cloned())
                        .collect();
                    let pick = recent
                        .iter()
                        .find(|e| self.is_open(&e.id))
                        .cloned()
                        .or_else(|| self.workspaces().find(|e| self.is_open(&e.id)).cloned())
                        .or_else(|| recent.first().cloned().filter(|_| fresh));
                    if let Some(e) = pick {
                        self.open_entry(e, false);
                    }
                }
            }
            DaemonMsg::Prompted { .. } => self.done("sent to the agent".into()),
            DaemonMsg::Error { msg } => self.fail(msg),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if let Some(v) = &mut self.settings {
            match v.key(k) {
                settings_view::Outcome::Stay => {}
                settings_view::Outcome::Close => self.settings = None,
                settings_view::Outcome::Changed => self.apply_settings(),
                settings_view::Outcome::Edit(path) => {
                    self.settings = None;
                    if self.active.is_none() {
                        self.fail("open a workspace first: the theme opens in its nvim".into());
                    } else {
                        self.open_file(&path.display().to_string());
                    }
                }
            }
            return;
        }
        if let Some(f) = &mut self.finder {
            match f.key(k) {
                finder::Outcome::Stay => {}
                finder::Outcome::Cancel => self.finder = None,
                finder::Outcome::Pick(pick) => {
                    self.finder = None;
                    self.picked(pick);
                }
            }
            return;
        }
        if let Some(m) = &mut self.modal {
            match m.key(k) {
                Outcome::Stay => {}
                Outcome::Cancel => self.modal = None,
                Outcome::Submit => self.submit_modal(),
            }
            return;
        }
        if self.leader_at.take().is_some() {
            self.which = false;
            self.leader_key(k);
            return;
        }
        if self.leader.matches(&k) {
            self.leader_at = Some(Instant::now());
            return;
        }
        let Some(f) = self.focus else { return };
        let action = match self.views.get_mut(&f) {
            Some(View::Diff(d)) => d.key(k),
            Some(View::Md(m)) => {
                if m.key(k) {
                    diffview::Action::Close
                } else {
                    diffview::Action::None
                }
            }
            None => return self.send_key(&k),
        };
        match action {
            diffview::Action::None => {}
            diffview::Action::Close => self.send(ClientMsg::Kill { pane: f }),
            diffview::Action::Open(path) => self.open_file(&path),
        }
    }

    /// The key after the leader (the keymap in docs/specs/rust-tui.md).
    fn leader_key(&mut self, k: KeyEvent) {
        self.status = None;
        match k.code {
            KeyCode::Esc => {}
            _ if self.leader.matches(&k) => self.ask_switch(),
            KeyCode::Char('?') => {
                // The popup stays and the next key still counts as an action.
                self.which = true;
                self.leader_at = Some(Instant::now());
            }
            KeyCode::Char(c @ '1'..='9') => self.jump(c as usize - '1' as usize),
            KeyCode::Char('h') => self.focus_towards(-1, 0),
            KeyCode::Char('l') => self.focus_towards(1, 0),
            KeyCode::Char('k') => self.focus_towards(0, -1),
            KeyCode::Char('j') => self.focus_towards(0, 1),
            KeyCode::Char('H') => self.move_pane(-1, 0),
            KeyCode::Char('L') => self.move_pane(1, 0),
            KeyCode::Char('K') => self.move_pane(0, -1),
            KeyCode::Char('J') => self.move_pane(0, 1),
            KeyCode::Char('q') => self.quit = true,
            code if self.action(code).is_some() => match self.action(code).unwrap_or("") {
                "switch" => self.ask_switch(),
                "previous" => self.go_back(),
                "open" => self.ask_folder(),
                "file" => self.ask_file(),
                "sessions" => self.ask_session(),
                "settings" => self.ask_settings(),
                "new" => self.new_worktree(),
                "rename" => self.ask_rename(),
                "sync" => self.run_sync(),
                "diff" => self.open_diff(),
                "remove" => self.ask_remove(),
                "pane" => self.new_pane(),
                "close" => self.close_pane(),
                "name" => self.ask_name(),
                "full" => {
                    self.full = !self.full;
                    self.fit();
                }
                _ => {}
            },
            code => {
                let key = match code {
                    KeyCode::Char(' ') => "␣".to_string(),
                    KeyCode::Char(c) => c.to_string(),
                    other => format!("{other:?}").to_lowercase(),
                };
                let l = self.leader.label();
                self.fail(format!("{l} {key} does nothing · {l} ? shows the keys"));
            }
        }
    }

    /// Reads where the workspace's panes are, from the daemon's last list.
    fn find_here(&mut self) {
        let Some(stream) = self.active.as_ref().map(|s| s.entry.clone()) else {
            self.here.clear();
            return;
        };
        let own = std::path::Path::new(&stream.path).canonicalize().ok();
        self.here = self
            .daemon_panes
            .iter()
            .filter(|p| p.stream == stream.id)
            .filter_map(|p| {
                let cwd = std::path::PathBuf::from(p.cwd.as_ref()?);
                let git = crate::connectors::git::head_of(&cwd);
                let home = git
                    .as_ref()
                    .is_some_and(|(root, _)| root.canonicalize().ok() == own);
                Some((p.pane, Here { cwd, git, home }))
            })
            .collect();
    }

    /// The action a key after the leader runs, from the settings.
    fn action(&self, code: KeyCode) -> Option<&'static str> {
        let key = match code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::Char(c) => c.to_string(),
            _ => return None,
        };
        crate::settings::get().action(&key)
    }

    /// `^␣ 1–9`: the workspace with that number in the sidebar.
    fn jump(&mut self, i: usize) {
        let target = self.workspaces().nth(i).cloned();
        match target {
            Some(e) => self.open_entry(e, false),
            None => self.fail(format!("no workspace {}", i + 1)),
        }
    }

    /// `^␣ tab`: the workspace used before this one.
    fn go_back(&mut self) {
        let here = self.current().map(|e| e.id.clone());
        let prev = self
            .recent
            .iter()
            .filter(|id| Some(*id) != here.as_ref())
            .find_map(|id| self.workspaces().find(|e| e.id == *id).cloned());
        match prev {
            Some(e) => self.open_entry(e, false),
            None => self.fail("no previous workspace yet".into()),
        }
    }

    /// `setup`: run the config's setup in the shell pane first (new streams).
    fn open_entry(&mut self, entry: Entry, setup: bool) {
        if self.current().is_some_and(|c| c.id == entry.id) {
            return;
        }
        self.recent.retain(|id| *id != entry.id);
        self.recent.insert(0, entry.id.clone());
        self.recent.truncate(50);
        save_recent(&self.recent);
        if crate::folders::is_folder(&entry) && !entry.path.is_empty() {
            let dir = entry.path.clone();
            if let Err(e) = crate::folders::edit(|all| {
                all.add(&dir);
            }) {
                self.fail(format!("{e:#}"));
            }
        }
        let stream = match Stream::resolve(&entry) {
            Ok(s) => s,
            Err(e) => {
                self.fail(format!("{}: {e:#}", entry.name));
                return;
            }
        };
        self.send(ClientMsg::Detach);
        self.panes.clear();
        self.views.clear();
        self.pending_view = None;
        self.tree = None;
        self.focus = None;
        let already = self.is_open(&entry.id);
        self.active = Some(stream);
        self.full = false;

        if already {
            self.send(ClientMsg::Attach {
                stream: entry.id.clone(),
            });
        } else if let Err(e) = self.start_panes(setup) {
            self.fail(format!("{}: {e:#}", entry.name));
        }
    }

    /// Asks the daemon to start the workspace's panes in its layout's shape.
    fn start_panes(&mut self, setup: bool) -> Result<()> {
        let stream = self.active.clone().context("no workspace")?;
        let (specs, note) = stream.open_specs(setup)?;
        if let Some(n) = note {
            self.say(n);
        }
        let sizes = stream.tree.rects(self.stage());
        let mut panes = specs.into_iter().zip(sizes).map(|(spec, rect)| {
            let (cols, rows) = inner(rect);
            NewPane {
                role: spec.role,
                cmd: spec.cmd,
                resume: spec.resume,
                cwd: spec.cwd,
                env: spec.env,
                cols,
                rows,
            }
        });
        let tree = Tree::from_node(&stream.tree, &mut panes).context("the layout has no panes")?;
        self.send(ClientMsg::Open {
            stream: stream.entry.id.clone(),
            tree,
        });
        stream.mark_opened()
    }

    /// The daemon's tree for the current workspace: forget panes that left,
    /// keep the focus on a pane that is still there.
    fn set_tree(&mut self, tree: Tree<PaneLeaf>) {
        let old: Vec<PaneId> = self
            .tree
            .as_ref()
            .map(|t| t.leaves().into_iter().map(|l| l.id).collect())
            .unwrap_or_default();
        let ids: Vec<PaneId> = tree.leaves().into_iter().map(|l| l.id).collect();
        if std::mem::take(&mut self.focus_new)
            && let Some(new) = ids.iter().find(|id| !old.contains(id))
        {
            self.focus = Some(*new);
        }
        if self.focus.is_none_or(|f| !ids.contains(&f)) {
            // The agent or the editor first; else the first pane.
            let pick = |role: &str| {
                tree.leaves()
                    .into_iter()
                    .find(|l| l.role == role)
                    .map(|l| l.id)
            };
            self.focus = pick("agent")
                .or_else(|| pick("editor"))
                .or_else(|| ids.first().copied());
        }
        self.panes.retain(|id, _| ids.contains(id));
        self.views.retain(|id, _| ids.contains(id));
        // Viewer leaves: the one just asked for gets its content; any other
        // without content (a client attaching) is filled from its role.
        let empty: Vec<(PaneId, String)> = tree
            .leaves()
            .into_iter()
            .filter(|l| crate::proto::is_view(&l.role) && !self.views.contains_key(&l.id))
            .map(|l| (l.id, l.role.clone()))
            .collect();
        for (id, role) in empty {
            if let Some(v) = self.pending_view.take() {
                self.views.insert(id, v);
                self.focus = Some(id);
            } else if role == "view:diff" {
                self.load_diff(Some(id));
            } else if let Some(file) = role.strip_prefix("view:md:")
                && let Some(s) = &self.active
                && let Ok(src) =
                    std::fs::read_to_string(std::path::Path::new(&s.entry.path).join(file))
            {
                self.views
                    .insert(id, View::Md(mdview::MdView::new(file.to_string(), src)));
            }
        }
        self.full &= ids.len() > 1;
        self.tree = Some(tree);
        self.fit();
    }

    /// Shows a viewer: in the workspace's viewer pane of that kind when it
    /// has one, else in a new pane along the right edge (REQ-57).
    fn show_view(&mut self, role: String, view: View) {
        let kind = |r: &str| r.split(':').take(2).collect::<Vec<_>>().join(":");
        let existing = self.tree.as_ref().and_then(|t| {
            t.leaves()
                .into_iter()
                .find(|l| kind(&l.role) == kind(&role))
                .map(|l| l.id)
        });
        if let Some(id) = existing {
            if self.tree.as_ref().and_then(|t| t.find(id)).map(|l| &l.role) != Some(&role) {
                self.send(ClientMsg::Role { pane: id, role });
            }
            self.views.insert(id, view);
            self.focus = Some(id);
            self.full = false;
            self.fit();
            return;
        }
        let Some(stream) = self.current().map(|e| e.id.clone()) else {
            return;
        };
        self.pending_view = Some(view);
        self.full = false;
        self.send(ClientMsg::Dock {
            stream,
            share: 0.36,
            new: NewPane {
                role,
                cmd: None,
                resume: None,
                cwd: std::path::PathBuf::new(),
                env: BTreeMap::new(),
                cols: 1,
                rows: 1,
            },
        });
    }

    /// Where the panes go: right of the sidebar and below the header. Full
    /// mode gives all of it to the focused pane; the sidebar stays.
    pub fn stage(&self) -> Rect {
        let (w, h) = self.size;
        Rect {
            x: SIDEBAR,
            y: 1,
            w: w.saturating_sub(SIDEBAR),
            h: h.saturating_sub(2),
        }
    }

    /// Each pane of the current workspace with its rectangle, borders
    /// included; in full mode, only the focused one.
    pub fn pane_rects(&self) -> Vec<(PaneId, Rect)> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        if self.full
            && let Some(f) = self.focus
        {
            return vec![(f, self.stage())];
        }
        tree.placed(self.stage())
    }

    /// What a pane's title says: its name; else its role, then what runs
    /// in it, dimmer: the title its program set, else its foreground
    /// process (REQ-53).
    pub fn pane_title(&self, id: PaneId) -> (String, Option<String>) {
        let leaf = self.tree.as_ref().and_then(|t| t.find(id));
        if let Some(name) = leaf.and_then(|l| l.name.clone()) {
            return (name, None);
        }
        let role = leaf.map(|l| l.role.clone()).unwrap_or_default();
        let title = self
            .panes
            .get(&id)
            .and_then(|v| v.title.clone())
            .filter(|t| !t.trim().is_empty());
        let fg = || {
            self.daemon_panes
                .iter()
                .find(|p| p.pane == id)
                .and_then(|p| p.fg.clone())
        };
        let runs = title.or_else(fg).filter(|r| *r != role);
        (role, runs)
    }

    /// Resizes every visible pane to its rectangle, here and in the daemon.
    fn fit(&mut self) {
        let rects = self.pane_rects();
        let mut resizes = Vec::new();
        for (id, rect) in rects {
            let Some(view) = self.panes.get_mut(&id) else {
                continue;
            };
            let (cols, rows) = inner(rect);
            if view.parser.screen().size() != (rows, cols) {
                view.parser.screen_mut().set_size(rows, cols);
                resizes.push(ClientMsg::Resize {
                    pane: id,
                    cols,
                    rows,
                });
            }
        }
        for r in resizes {
            self.send(r);
        }
    }

    fn focused_pane(&self) -> Option<PaneId> {
        self.focus.filter(|f| self.panes.contains_key(f))
    }

    /// The sidebar row under `y`, as the sidebar draws them (REQ-50).
    fn sidebar_row_at(&self, y: u16) -> Option<usize> {
        let rows = self.size.1.saturating_sub(1).saturating_sub(4) as usize;
        let y = usize::from(y.checked_sub(2)?);
        if y >= rows {
            return None;
        }
        let current = self
            .rows
            .iter()
            .position(|r| self.current().is_some_and(|c| c.id == r.entry.id))
            .unwrap_or(0);
        let i = current.saturating_sub(rows.saturating_sub(1)) + y;
        (i < self.rows.len()).then_some(i)
    }

    /// The mouse (Shift + drag still selects text in the terminal): click a
    /// workspace or a pane, roll the wheel over a pane or a picker, and pass
    /// everything to a program that asked for the mouse.
    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let wheel = match m.kind {
            MouseEventKind::ScrollUp => -(WHEEL as isize),
            MouseEventKind::ScrollDown => WHEEL as isize,
            _ => 0,
        };
        let click = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        if self.settings.is_some() {
            return;
        }
        if let Some(f) = &mut self.finder {
            if wheel != 0 {
                f.move_by(wheel.signum());
            } else if click {
                let area = ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1);
                if let Some(pick) = f.click(x, y, area) {
                    self.finder = None;
                    self.picked(pick);
                }
            }
            return;
        }
        if self.modal.is_some() {
            return;
        }
        if click && self.leader_at.take().is_some() {
            self.which = false;
        }
        if x < SIDEBAR {
            if click && let Some(i) = self.sidebar_row_at(y) {
                let e = self.rows[i].entry.clone();
                self.open_entry(e, false);
            }
            return;
        }
        if self.drag(&m) {
            return;
        }
        let Some((id, r)) = self
            .pane_rects()
            .into_iter()
            .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        else {
            return;
        };
        if matches!(m.kind, MouseEventKind::Down(_)) && self.focus != Some(id) {
            self.focus = Some(id);
            self.fit();
        }
        if let Some(v) = self.views.get_mut(&id) {
            match v {
                View::Diff(d) if wheel != 0 => d.scroll_by(wheel),
                View::Diff(d) if click => d.click(x, y),
                View::Md(md) if wheel != 0 => md.scroll_by(wheel),
                _ => {}
            }
            return;
        }
        let inside = x > r.x && x + 1 < r.x + r.w && y > r.y && y + 1 < r.y + r.h;
        let Some(view) = self.panes.get_mut(&id) else {
            return;
        };
        let screen = view.parser.screen();
        let mode = screen.mouse_protocol_mode();
        if inside && mode != vt100::MouseProtocolMode::None {
            let at = (x - r.x, y - r.y);
            if let Some(bytes) = keys::encode_mouse(&m, at, mode, screen.mouse_protocol_encoding())
            {
                self.send(ClientMsg::Input { pane: id, bytes });
            }
            return;
        }
        if wheel == 0 {
            return;
        }
        if screen.alternate_screen() {
            // A full-screen program without the mouse: arrows, like the
            // terminals that scroll it.
            let key = if wheel < 0 { 'A' } else { 'B' };
            let one = if screen.application_cursor() {
                format!("\x1bO{key}")
            } else {
                format!("\x1b[{key}")
            };
            let bytes = one.repeat(WHEEL).into_bytes();
            self.send(ClientMsg::Input { pane: id, bytes });
        } else {
            let back = screen.scrollback().saturating_add_signed(-wheel);
            view.parser.screen_mut().set_scrollback(back);
        }
    }

    /// Dragging the line between two panes resizes them; the daemon keeps
    /// the new share when the button goes up. True when the event was that.
    fn drag(&mut self, m: &MouseEvent) -> bool {
        let stage = self.stage();
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if !self.full => {
                self.dragging = self
                    .tree
                    .as_ref()
                    .and_then(|t| t.border_at(stage, m.column, m.row));
                self.dragging.is_some()
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(b) = &self.dragging else {
                    return false;
                };
                let (pos, start, len) = match b.dir {
                    Dir::Right => (m.column, b.area.x, b.area.w),
                    Dir::Down => (m.row, b.area.y, b.area.h),
                };
                let ratio = f32::from(pos.saturating_sub(start) + 1) / f32::from(len.max(1));
                let path = b.path.clone();
                if let Some(t) = &mut self.tree {
                    t.set_ratio(&path, ratio);
                }
                self.fit();
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(b) = self.dragging.take() else {
                    return false;
                };
                let ratio = self.tree.as_ref().and_then(|t| t.ratio_at(&b.path));
                if let (Some(stream), Some(ratio)) = (self.current().map(|e| e.id.clone()), ratio) {
                    self.send(ClientMsg::Ratio {
                        stream,
                        path: b.path,
                        ratio,
                    });
                }
                true
            }
            _ => false,
        }
    }

    fn send_key(&mut self, k: &KeyEvent) {
        let Some(id) = self.focused_pane() else {
            return;
        };
        // Typing goes back to the bottom of the history, as terminals do.
        if let Some(v) = self.panes.get_mut(&id) {
            v.parser.screen_mut().set_scrollback(0);
        }
        let app_cursor = self.panes[&id].parser.screen().application_cursor();
        let bytes = keys::encode(k, app_cursor);
        if !bytes.is_empty() {
            self.send(ClientMsg::Input { pane: id, bytes });
        }
    }

    fn paste(&mut self, text: &str) {
        let Some(id) = self.focused_pane() else {
            return;
        };
        let bytes = if self.panes[&id].parser.screen().bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        self.send(ClientMsg::Input { pane: id, bytes });
    }

    /// `^␣ hjkl`: focus the pane on that side.
    fn focus_towards(&mut self, dx: i32, dy: i32) {
        let (Some(tree), Some(f)) = (&self.tree, self.focus) else {
            return;
        };
        match tree.neighbour(f, dx, dy, self.stage()) {
            Some(n) => {
                self.focus = Some(n);
                self.full = false;
                self.fit();
            }
            None => self.fail("no pane that way".into()),
        }
    }

    /// `^␣ HJKL`: trade places with the pane on that side.
    fn move_pane(&mut self, dx: i32, dy: i32) {
        let (Some(tree), Some(f)) = (&self.tree, self.focus) else {
            return;
        };
        match tree.neighbour(f, dx, dy, self.stage()) {
            Some(n) => self.send(ClientMsg::Swap { a: f, b: n }),
            None => self.fail("no pane that way".into()),
        }
    }

    /// `^␣ t`: a shell beside the focused pane, in the workspace's folder;
    /// side by side when the pane is wide, one over the other when not.
    fn new_pane(&mut self) {
        let (Some(stream), Some(f)) = (&self.active, self.focus) else {
            return;
        };
        let Some((_, r)) = self.pane_rects().into_iter().find(|(id, _)| *id == f) else {
            return;
        };
        // Cells are about twice as tall as wide.
        let dir = if r.w >= r.h * 2 {
            Dir::Right
        } else {
            Dir::Down
        };
        let (cols, rows) = match dir {
            Dir::Right => inner(Rect { w: r.w / 2, ..r }),
            Dir::Down => inner(Rect { h: r.h / 2, ..r }),
        };
        let new = NewPane {
            role: "shell".into(),
            cmd: None,
            resume: None,
            cwd: std::path::PathBuf::from(&stream.entry.path),
            env: stream.env(),
            cols,
            rows,
        };
        self.focus_new = true;
        self.full = false;
        self.send(ClientMsg::Split { pane: f, dir, new });
    }

    /// `^␣ x`: close the focused pane, asking when a program other than a
    /// shell runs in it; the last pane closes the workspace (REQ-35).
    fn close_pane(&mut self) {
        let (Some(entry), Some(f)) = (self.current().cloned(), self.focus) else {
            return;
        };
        if self.views.contains_key(&f) {
            self.send(ClientMsg::Kill { pane: f });
            return;
        }
        let count = self.tree.as_ref().map_or(0, |t| {
            t.leaves()
                .into_iter()
                .filter(|l| !crate::proto::is_view(&l.role))
                .count()
        });
        if count <= 1 {
            let running = self.running(&entry.id);
            self.modal = Some(Modal::Close {
                entry,
                running,
                last: true,
            });
            return;
        }
        let fg = self
            .daemon_panes
            .iter()
            .find(|p| p.pane == f)
            .and_then(|p| p.fg.clone())
            .filter(|fg| !SHELLS.contains(&fg.as_str()));
        match fg {
            Some(fg) => {
                self.modal = Some(Modal::ClosePane {
                    pane: f,
                    title: self.pane_title(f).0,
                    running: fg,
                })
            }
            None => self.send(ClientMsg::Kill { pane: f }),
        }
    }

    /// `^␣ n`: name the focused pane; an empty name goes back to automatic.
    fn ask_name(&mut self) {
        let Some(f) = self.focus else { return };
        let text = self
            .tree
            .as_ref()
            .and_then(|t| t.find(f))
            .and_then(|l| l.name.clone())
            .unwrap_or_default();
        self.modal = Some(Modal::Name { pane: f, text });
    }

    /// The programs other than shells that run in a workspace's panes.
    fn running(&self, id: &str) -> Vec<String> {
        self.daemon_panes
            .iter()
            .filter(|p| p.stream == id)
            .filter_map(|p| {
                let fg = p.fg.as_deref()?;
                (!SHELLS.contains(&fg)).then(|| format!("{:<10} {fg}", p.role))
            })
            .collect()
    }
}

impl App {
    /// The current workspace, for an action that needs a repository: a
    /// plain folder gets a word instead.
    fn current_repo_stream(&mut self, action: &str) -> Option<Entry> {
        let Some(entry) = self.current().cloned() else {
            let l = self.leader.label();
            self.fail(format!("no workspace open: {l} o opens a folder"));
            return None;
        };
        if crate::folders::is_folder(&entry) && entry.branch.is_empty() {
            self.fail(format!(
                "{} is not a git repository: {action} needs one",
                entry.name
            ));
            return None;
        }
        Some(entry)
    }

    /// The current workspace when it is a worktree; a project's own folder
    /// gets a word instead.
    fn current_worktree(&mut self, action: &str) -> Option<Entry> {
        let entry = self.current_repo_stream(action)?;
        if crate::folders::is_folder(&entry) {
            self.fail(format!(
                "{} is the project itself: {action} is for its worktrees",
                entry.name
            ));
            return None;
        }
        Some(entry)
    }

    /// `^␣ r`: rename the current worktree (REQ-40): name, branch and
    /// folder. A folder only changes the name jw shows.
    fn ask_rename(&mut self) {
        if let Some(e) = self
            .current()
            .filter(|e| crate::folders::is_folder(e))
            .cloned()
        {
            let own = std::path::Path::new(&e.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.path.clone());
            self.modal = Some(Modal::Alias {
                text: e.name.clone(),
                dir: e.path,
                own,
            });
            return;
        }
        let Some(entry) = self.current_worktree("rename") else {
            return;
        };
        match Project::open(std::path::Path::new(&entry.path)) {
            Ok(project) => {
                let plan = registry::default_path()
                    .and_then(|reg| actions::rename_plan(&project, &entry, &entry.name, &reg))
                    .map_err(|e| format!("{e:#}"));
                self.modal = Some(Modal::Rename {
                    text: entry.name.clone(),
                    plan,
                    entry,
                    project,
                })
            }
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    /// `^␣ ,`: the settings screen (REQ-67).
    fn ask_settings(&mut self) {
        self.settings = Some(settings_view::SettingsView::new());
    }

    /// What the settings screen changed, on screen now.
    fn apply_settings(&mut self) {
        if let Err(e) = crate::theme::load(&crate::settings::get()) {
            self.fail(format!("{e:#}"));
        }
        crate::theme::set_dark(dark_now());
        self.leader = leader();
    }

    /// `^␣ o`: the folder browser, starting beside the current project.
    fn ask_folder(&mut self) {
        let start = self
            .current()
            .map(|e| {
                let dir = if crate::folders::is_folder(e) {
                    std::path::PathBuf::from(&e.path)
                } else {
                    self.rows
                        .iter()
                        .find(|r| !r.child && r.entry.project == e.project)
                        .map(|r| std::path::PathBuf::from(&r.entry.path))
                        .unwrap_or_else(|| std::path::PathBuf::from(&e.path))
                };
                dir.parent().map(|d| d.to_path_buf()).unwrap_or(dir)
            })
            .filter(|d| d.is_dir())
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .unwrap_or_else(|| "/".into());
        let open = self
            .rows
            .iter()
            .filter(|r| !r.child && self.is_open(&r.entry.id))
            .map(|r| std::path::PathBuf::from(&r.entry.path))
            .collect();
        self.finder = Some(finder::Finder::folders(start, open));
    }

    /// `^␣ ␣`: every workspace by last use, with what each holds (REQ-41).
    fn ask_switch(&mut self) {
        let here = self.current().map(|e| e.id.clone());
        let rank = |id: &str| {
            if Some(id) == here.as_deref() {
                usize::MAX
            } else {
                self.recent
                    .iter()
                    .position(|r| r == id)
                    .unwrap_or(usize::MAX - 1)
            }
        };
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by_key(|i| rank(&self.rows[*i].entry.id));
        let items = order
            .into_iter()
            .map(|i| {
                let e = &self.rows[i].entry;
                let parent = self.rows[..i]
                    .iter()
                    .rev()
                    .find(|r| !r.child)
                    .filter(|_| self.rows[i].child)
                    .map(|r| r.entry.name.clone());
                let label = match parent {
                    Some(p) => format!("{p}/{}", e.name),
                    None => e.name.clone(),
                };
                finder::WsItem {
                    id: e.id.clone(),
                    open: self.is_open(&e.id),
                    hint: if Some(&e.id) == here.as_ref() {
                        "here".into()
                    } else {
                        e.branch.clone()
                    },
                    preview: self.ws_preview(e),
                    label,
                }
            })
            .collect();
        self.finder = Some(finder::Finder::workspaces(items));
    }

    /// `^␣ a`: every session, the others by name (REQ-63).
    fn ask_session(&mut self) {
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        let here = crate::session::current();
        let state = match registry::state_dir() {
            Ok(s) => s,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        let mut names = crate::session::list(&state).unwrap_or_default();
        names.sort_by_key(|n| *n == here);
        let reg = registry::default_path()
            .and_then(|p| Registry::load(&p))
            .unwrap_or_default();
        let dim = Style::default().fg(crate::theme::p().dim);
        let items = names
            .into_iter()
            .map(|name| {
                let folders = crate::folders::Folders::load(
                    &state.join("sessions").join(&name).join("folders.json"),
                )
                .unwrap_or_default();
                let worktrees: Vec<&Entry> = reg
                    .entries
                    .iter()
                    .filter(|e| crate::session::owns(&name, &e.session))
                    .collect();
                let ids: Vec<String> = folders
                    .folders
                    .iter()
                    .map(|f| crate::folders::id_in(&name, &f.dir))
                    .chain(worktrees.iter().map(|e| e.id.clone()))
                    .collect();
                let open = ids.iter().filter(|id| self.is_open(id)).count();
                let mut preview = vec![
                    Line::from(Span::styled(
                        name.clone(),
                        Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                    )),
                    Line::from(Span::styled(format!("{open} open of {}", ids.len()), dim)),
                    Line::from(""),
                ];
                for f in &folders.folders {
                    preview.push(Line::from(finder::tilde(std::path::Path::new(&f.dir))));
                }
                for e in &worktrees {
                    preview.push(Line::from(format!("  ↳ {}/{}", e.project, e.name)));
                }
                finder::WsItem {
                    hint: if name == here {
                        "here".into()
                    } else {
                        crate::session::workspaces(ids.len())
                    },
                    open: open > 0,
                    id: name.clone(),
                    label: name,
                    preview,
                }
            })
            .collect();
        self.finder = Some(finder::Finder::sessions(items));
    }

    /// Renames a session; its running workspaces follow their new ids.
    fn rename_session(&mut self, old: &str, new: &str) {
        let state = match registry::state_dir() {
            Ok(s) => s,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        let ids = match crate::session::rename(&state, old, new) {
            Ok(ids) => ids,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        for (from, to) in ids {
            self.send(ClientMsg::Rekey { from, to });
        }
        if crate::session::current() == old {
            crate::session::set(new);
            self.send(ClientMsg::Detach);
            self.active = None;
            self.panes.clear();
            self.views.clear();
            self.tree = None;
            self.focus = None;
            self.recent = load_recent();
            self.fresh = true;
            if let Err(e) = self.reload() {
                self.fail(format!("{e:#}"));
            }
            self.send(ClientMsg::List);
        }
        self.done(format!("session {old} is {new} now"));
    }

    /// Shows another session: the workspaces on screen keep running.
    fn switch_session(&mut self, name: &str) {
        if name == crate::session::current() {
            return;
        }
        crate::session::set(name);
        if let Ok(state) = registry::state_dir() {
            let _ = crate::session::set_last(&state, name);
        }
        self.send(ClientMsg::Detach);
        self.active = None;
        self.panes.clear();
        self.views.clear();
        self.pending_view = None;
        self.tree = None;
        self.focus = None;
        self.full = false;
        self.recent = load_recent();
        self.fresh = true;
        if let Err(e) = self.reload() {
            self.fail(format!("{e:#}"));
        }
        self.send(ClientMsg::List);
        self.done(format!("session {name}"));
    }

    /// `+ create` in the session picker: a session that starts in the
    /// current workspace's folder.
    fn new_session(&mut self, name: &str) {
        let dir = self
            .current()
            .map(|e| std::path::PathBuf::from(&e.path))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let made =
            registry::state_dir().and_then(|state| crate::session::create(&state, name, &dir));
        match made {
            Ok(()) => self.switch_session(name),
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    /// The switcher's right side for one workspace.
    fn ws_preview(&self, e: &Entry) -> Vec<ratatui::text::Line<'static>> {
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        let p = crate::theme::p();
        let dim = Style::default().fg(p.dim);
        let mut l = vec![Line::from(Span::styled(
            finder::tilde(std::path::Path::new(&e.path)),
            Style::default().add_modifier(ratatui::style::Modifier::BOLD),
        ))];
        l.push(if e.branch.is_empty() {
            Line::from(Span::styled("not a git repository", dim))
        } else {
            Line::from(Span::styled(
                e.branch.clone(),
                Style::default().fg(p.magenta),
            ))
        });
        l.push(Line::default());
        let panes: Vec<&PaneInfo> = self
            .daemon_panes
            .iter()
            .filter(|x| x.stream == e.id)
            .collect();
        if panes.is_empty() {
            l.push(Line::from(Span::styled("closed: ↵ opens it", dim)));
        } else {
            l.push(Line::from(match panes.len() {
                1 => "1 pane running".to_string(),
                n => format!("{n} panes running"),
            }));
            for x in panes {
                let runs = x.fg.clone().unwrap_or_default();
                l.push(Line::from(vec![
                    Span::raw(format!("  {:<10}", x.role)),
                    Span::styled(runs, dim),
                ]));
            }
        }
        l
    }

    /// `^␣ /`: the files of the current workspace (REQ-42).
    fn ask_file(&mut self) {
        let Some(e) = self.current().cloned() else {
            let l = self.leader.label();
            self.fail(format!("no workspace open: {l} o opens a folder"));
            return;
        };
        let root = std::path::PathBuf::from(&e.path);
        let files = finder::list_files(&root, 20_000);
        if files.is_empty() {
            self.fail(format!("{} has no files", e.name));
            return;
        }
        self.finder = Some(finder::Finder::files(
            format!("Open a file in {}", e.name),
            root,
            files,
        ));
    }

    /// A file of the current workspace: Markdown in the reader, anything
    /// else in its nvim pane, started on the right when there is none.
    fn open_file(&mut self, file: &str) {
        let Some(stream) = self.active.clone() else {
            return;
        };
        if file.ends_with(".md") {
            self.read_file(&stream.entry.path, file);
            return;
        }
        let editor = self.tree.as_ref().and_then(|t| {
            t.leaves()
                .into_iter()
                .find(|l| l.role == "editor")
                .map(|l| l.id)
        });
        match editor {
            Some(pane) => {
                // REQ-75: through the nvim's own socket when it listens on
                // one; else typed as keys, which needs normal mode (RISK-16).
                let path = std::path::Path::new(&stream.entry.path).join(file);
                let remote = crate::stream::nvim_socket(pane)
                    .filter(|s| s.exists())
                    .is_some_and(|s| {
                        std::process::Command::new("nvim")
                            .arg("--server")
                            .arg(&s)
                            .arg("--remote")
                            .arg(&path)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .is_ok_and(|st| st.success())
                    });
                if !remote {
                    let line = format!("\x1b:e {}\r", file.replace(' ', "\\ "));
                    self.send(ClientMsg::Input {
                        pane,
                        bytes: line.into_bytes(),
                    });
                }
                self.focus = Some(pane);
                self.full = false;
                self.fit();
            }
            None => {
                let Some(beside) = self.focus else { return };
                let stage = self.stage();
                let new = NewPane {
                    role: "editor".into(),
                    cmd: Some(crate::stream::listen(&format!(
                        "nvim {}",
                        crate::stream::shell_quote(file)
                    ))),
                    resume: None,
                    cwd: std::path::PathBuf::from(&stream.entry.path),
                    env: stream.env(),
                    cols: (stage.w / 2).saturating_sub(2).max(1),
                    rows: stage.h.saturating_sub(2).max(1),
                };
                self.focus_new = true;
                self.full = false;
                self.send(ClientMsg::Split {
                    pane: beside,
                    dir: Dir::Right,
                    new,
                });
            }
        }
        self.say(format!("nvim {file}"));
    }

    /// A folder picked in the browser: made first when new, then opened, or
    /// shown when it already is (REQ-37). A workspace or a file from the
    /// other pickers.
    fn picked(&mut self, pick: finder::Pick) {
        let dir = match pick {
            finder::Pick::Ws(id) => {
                let target = self.workspaces().find(|e| e.id == id).cloned();
                if let Some(e) = target {
                    self.open_entry(e, false);
                }
                return;
            }
            finder::Pick::File(f) => return self.open_file(&f),
            finder::Pick::Session(s) => return self.switch_session(&s),
            finder::Pick::NewSession(s) => return self.new_session(&s),
            finder::Pick::RenameSession(s) => {
                self.modal = Some(Modal::RenameSession {
                    text: s.clone(),
                    old: s,
                });
                return;
            }
            finder::Pick::Dir(d) => d,
            finder::Pick::Create(d) => {
                if let Err(e) = std::fs::create_dir(&d) {
                    self.fail(format!("{}: {e}", finder::tilde(&d)));
                    return;
                }
                d
            }
        };
        let dir = dir.display().to_string();
        let id = crate::folders::id(&dir);
        let entry = self
            .workspaces()
            .find(|e| e.id == id)
            .cloned()
            .unwrap_or_else(|| crate::folders::entry(&dir, false));
        self.open_entry(entry, false);
        if let Err(e) = self.reload() {
            self.fail(format!("{e:#}"));
        }
    }

    /// `w`: a worktree `ws-N` from the current workspace's branch, set up and
    /// shown at once (REQ-38).
    fn new_worktree(&mut self) {
        let Some(from) = self.current_repo_stream("a worktree") else {
            return;
        };
        let taken: Vec<&str> = self
            .workspaces()
            .filter(|e| e.project == from.project)
            .map(|e| e.name.as_str())
            .collect();
        let name = (1..)
            .map(|n| format!("ws-{n}"))
            .find(|n| !taken.contains(&n.as_str()))
            .expect("some ws-N is free");
        self.background(format!("creating {name}"), move || {
            let project = Project::open(std::path::Path::new(&from.path))?;
            let o = NewOptions {
                name,
                from: from.branch,
                branch: String::new(),
            };
            let entry = actions::new_stream(&project, &o, &registry::default_path()?)?;
            Ok(Job::Created {
                entry,
                setup: !project.cfg.setup.is_empty(),
            })
        });
    }

    /// `X`: the done checks when the PR is merged, the rm checks otherwise
    /// (REQ-39).
    fn ask_remove(&mut self) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        if crate::folders::is_folder(&entry) {
            let l = self.leader.label();
            self.fail(format!(
                "{} is a project folder: jw never deletes it. {l} x on its last pane closes it",
                entry.name
            ));
            return;
        }
        // A worktree already gone from disk finds its repository through a
        // sibling.
        let siblings: Vec<String> = self
            .workspaces()
            .filter(|e| e.project == entry.project && e.id != entry.id)
            .map(|e| e.path.clone())
            .collect();
        self.background(format!("checking {}", entry.name), move || {
            let project = std::iter::once(&entry.path)
                .chain(&siblings)
                .find_map(|p| Project::open(std::path::Path::new(p)).ok())
                .context("no checkout of this project left to find its repository")?;
            let prs = crate::connectors::github::Client::default();
            let pr = prs
                .for_branch(&project.repo.root, &entry.branch)
                .ok()
                .flatten();
            if pr.as_ref().is_some_and(|p| p.state == "MERGED") {
                let (plan, pr) = actions::done_plan(&project, &entry, &prs)?;
                return Ok(Job::DoneReady {
                    entry,
                    project,
                    plan,
                    pr,
                });
            }
            let plan = actions::rm_plan(&project, &entry)?;
            let note = pr.map(|p| format!("PR #{} is {}", p.number, p.status()));
            Ok(Job::RmReady {
                entry,
                project,
                plan,
                note,
            })
        });
    }

    /// Opens `file` (relative to `dir`) in the workspace's reader pane.
    fn read_file(&mut self, dir: &str, file: &str) {
        match std::fs::read_to_string(std::path::Path::new(dir).join(file)) {
            Ok(src) => self.show_view(
                format!("view:md:{file}"),
                View::Md(mdview::MdView::new(file.to_string(), src)),
            ),
            Err(e) => self.fail(format!("{file}: {e}")),
        }
    }

    /// `d`: the workspace's changes against its base, in a pane on the right.
    fn open_diff(&mut self) {
        if self.current_worktree("diff").is_some() {
            self.load_diff(None);
        }
    }

    fn load_diff(&mut self, into: Option<PaneId>) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.background(format!("diffing {}", entry.name), move || {
            let stream = Stream::resolve(&entry)?;
            let files = crate::diff::load(std::path::Path::new(&entry.path), &stream.base)?;
            Ok(Job::Diff {
                title: format!("{} vs {}", entry.name, stream.base),
                dir: entry.path,
                files,
                into,
            })
        });
    }

    fn run_sync(&mut self) {
        let Some(entry) = self.current_repo_stream("sync") else {
            return;
        };
        self.background(format!("syncing {}", entry.name), move || {
            if crate::folders::is_folder(&entry) {
                let dir = std::path::Path::new(&entry.path);
                crate::connectors::git::pull_ff(dir)?;
                return Ok(Job::Said(format!(
                    "{}: {} is up to date with origin",
                    entry.name, entry.branch
                )));
            }
            let p = Project::open(std::path::Path::new(&entry.path))?;
            let done = actions::sync(&p, &entry)?;
            Ok(Job::Said(format!("{}: {}", entry.name, done.message())))
        });
    }

    fn submit_modal(&mut self) {
        let Some(m) = self.modal.take() else {
            return;
        };
        match m {
            Modal::Close { entry, .. } => self.close(&entry),
            Modal::ClosePane { pane, .. } => self.send(ClientMsg::Kill { pane }),
            Modal::Rename {
                entry,
                project,
                text,
                ..
            } => {
                // The panes go first, so nothing runs in the folder it moves.
                self.close(&entry);
                self.background(format!("renaming {}", entry.name), move || {
                    let reg = registry::default_path()?;
                    let renamed = actions::rename(&project, &entry, &text, &reg)?;
                    Ok(Job::Renamed(renamed))
                });
            }
            Modal::Name { pane, text } => {
                let name = Some(text.trim().to_string()).filter(|n| !n.is_empty());
                self.send(ClientMsg::Name { pane, name });
            }
            Modal::Alias { dir, own, text } => {
                let name = Some(text.trim().to_string()).filter(|n| !n.is_empty() && *n != own);
                let shown = name.clone().unwrap_or_else(|| own.clone());
                match crate::folders::edit(|all| {
                    if let Some(f) = all.folders.iter_mut().find(|f| f.dir == dir) {
                        f.name = name;
                    }
                }) {
                    Ok(()) => {
                        if let Err(e) = self.reload() {
                            self.fail(format!("{e:#}"));
                        }
                        self.done(format!("{own} shows as {shown}"));
                    }
                    Err(e) => self.fail(format!("{e:#}")),
                }
            }
            Modal::RenameSession { old, text } => self.rename_session(&old, &text),
            Modal::Done {
                entry,
                project,
                plan,
                ..
            } => {
                self.close(&entry);
                self.background(format!("removing {}", entry.name), move || {
                    let reg = registry::default_path()?;
                    let note = actions::rm(&project, &entry, &plan, &reg)?;
                    Ok(Job::Removed {
                        name: entry.name,
                        note,
                    })
                });
            }
            Modal::Rm {
                entry,
                project,
                plan,
                typed,
                ..
            } => {
                if plan.loses_work() && typed != entry.name {
                    self.modal = Some(Modal::Rm {
                        error: Some(format!("type {} to confirm", entry.name)),
                        entry,
                        project,
                        plan,
                        typed,
                    });
                    return;
                }
                // Panes first, so no process holds the directory.
                self.close(&entry);
                self.background(format!("removing {}", entry.name), move || {
                    let reg = registry::default_path()?;
                    let note = actions::rm(&project, &entry, &plan, &reg)?;
                    Ok(Job::Removed {
                        name: entry.name,
                        note,
                    })
                });
            }
        }
    }

    /// Kills every pane of the stream; the worktree stays.
    fn close(&mut self, entry: &Entry) {
        self.send(ClientMsg::Close {
            stream: entry.id.clone(),
        });
        self.daemon_panes.retain(|p| p.stream != entry.id);
        if self.current().is_some_and(|c| c.id == entry.id) {
            self.active = None;
            self.panes.clear();
            self.tree = None;
            self.focus = None;
            self.full = false;
        }
        if crate::folders::is_folder(entry) {
            let dir = entry.path.clone();
            if let Err(e) = crate::folders::edit(|all| all.folders.retain(|f| f.dir != dir)) {
                self.fail(format!("{e:#}"));
                return;
            }
            let _ = self.reload();
        }
        self.done(format!("closed {}; its folder stays", entry.name));
        self.send(ClientMsg::List);
    }

    /// Runs a slow action off the main thread; its result comes back as a Job.
    fn background(&mut self, what: String, work: impl FnOnce() -> Result<Job> + Send + 'static) {
        self.busy = Some(what);
        let tx = self.events.clone();
        thread::spawn(move || {
            let job = work().unwrap_or_else(|e| Job::Failed(format!("{e:#}")));
            let _ = tx.send(Msg::Job(Box::new(job)));
        });
    }

    fn on_job(&mut self, job: Job) {
        self.busy = None;
        let reload = self.reload();
        match job {
            Job::Created { entry, setup } => {
                self.done(format!("created {} on {}", entry.name, entry.branch));
                self.open_entry(entry, setup);
            }
            Job::Removed { name, note } => {
                self.done(match note {
                    Some(n) => format!("removed {name}; {n}"),
                    None => format!("removed {name}"),
                });
            }
            Job::Failed(e) => self.fail(e),
            Job::Renamed(entry) => {
                self.done(format!("renamed to {} on {}", entry.name, entry.branch));
                self.open_entry(entry, false);
            }
            Job::Said(msg) => self.done(msg),
            Job::Diff {
                title,
                dir,
                files,
                into,
            } => {
                let view = View::Diff(diffview::DiffView::new(title, dir, files));
                match into {
                    Some(id) => {
                        self.views.insert(id, view);
                    }
                    None => self.show_view("view:diff".into(), view),
                }
            }
            Job::DoneReady {
                entry,
                project,
                plan,
                pr,
            } => {
                self.modal = Some(Modal::Done {
                    entry,
                    project,
                    plan,
                    pr,
                })
            }
            Job::RmReady {
                entry,
                project,
                plan,
                note,
            } => {
                if let Some(n) = note {
                    self.say(n);
                }
                self.modal = Some(Modal::Rm {
                    entry,
                    project,
                    plan,
                    typed: String::new(),
                    error: None,
                });
            }
        }
        if let Err(e) = reload {
            self.fail(format!("{e:#}"));
        }
    }
}

/// The current session's directory, where `recent.json` lives.
fn session_dir() -> Option<std::path::PathBuf> {
    let state = registry::state_dir().ok()?;
    crate::session::dir(&state, &crate::session::current()).ok()
}

/// Where a pane is (the header follows the focused one).
pub struct Here {
    pub cwd: std::path::PathBuf,
    /// The checkout it is in and its branch.
    pub git: Option<(std::path::PathBuf, String)>,
    /// That checkout is the workspace's own.
    pub home: bool,
}

/// The workspace ids in the session's `recent.json`, most recent first.
fn load_recent() -> Vec<String> {
    session_dir()
        .and_then(|d| std::fs::read(d.join("recent.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Best effort: a lost recency list only reorders the switcher.
fn save_recent(ids: &[String]) {
    if let Some(d) = session_dir()
        && std::fs::create_dir_all(&d).is_ok()
        && let Ok(json) = serde_json::to_vec(ids)
    {
        let tmp = d.join("recent.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, d.join("recent.json"));
        }
    }
}

/// Shells don't count as "running something" when closing.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "nu"];

/// The terminal size inside a pane's border.
fn inner(r: Rect) -> (u16, u16) {
    (r.w.saturating_sub(2).max(1), r.h.saturating_sub(2).max(1))
}
