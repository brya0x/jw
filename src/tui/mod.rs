//! The client: the sidebar of workspaces, the current one's panes in their
//! splits, and a one-shot leader key for every action (docs/specs/rust-tui.md).
//!
//! Two threads feed one channel: the daemon's messages and the terminal's
//! events. The main thread applies them and redraws.

mod diffview;
mod draw;
mod keys;
mod mdview;
mod modal;

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
};
use ratatui::crossterm::execute;

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::connectors::PullRequests;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::{Node, Rect};
use crate::proto::{ClientMsg, DaemonMsg, PaneId, PaneInfo};
use crate::stream::Stream;

use modal::{Modal, Outcome};

use keys::Leader;

/// Width of the sidebar, borders included.
const SIDEBAR: u16 = 28;

/// How long the leader waits before showing every key (REQ-31).
const WHICH_AFTER: Duration = Duration::from_millis(600);

enum Msg {
    Daemon(DaemonMsg),
    DaemonGone,
    Term(Event),
    /// A slow action finished on its worker thread. Boxed: output messages
    /// go through the same channel by the thousand and stay small.
    Job(Box<Job>),
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
    },
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

/// What the stage shows instead of the panes.
pub enum View {
    Diff(diffview::DiffView),
    Md(mdview::MdView),
}

/// One row of the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Space(String),
    Stream(Entry),
}

/// A pane of the active stream as this client sees it.
pub struct PaneView {
    pub role: String,
    pub parser: vt100::Parser,
    pub exited: Option<i32>,
}

/// A spawn sent and not yet confirmed. The daemon handles one client's
/// messages in order, so confirmations come back in the same order.
struct Pending {
    role: String,
    cols: u16,
    rows: u16,
}

pub struct App {
    tx: Client,
    pub leader: Leader,
    /// When the leader was pressed; the next key is an action (REQ-30).
    leader_at: Option<Instant>,
    /// The popup with every key (REQ-31).
    pub which: bool,
    pub rows: Vec<Row>,
    /// The workspace before the current one, for `^␣ tab`.
    prev: Option<Entry>,
    /// Every pane the daemon has, from the last `List`.
    pub daemon_panes: Vec<PaneInfo>,
    /// The stream on screen, by registry id.
    pub active: Option<Stream>,
    pub panes: BTreeMap<PaneId, PaneView>,
    pending: VecDeque<Pending>,
    pub focus: Option<String>,
    /// `f`: only the focused pane, across the whole terminal.
    pub full: bool,
    pub status: Option<String>,
    /// The terminal's size, for the layout.
    pub size: (u16, u16),
    quit: bool,
    pub modal: Option<Modal>,
    /// A viewer drawn in place of the stream's panes (diff, Markdown).
    pub view: Option<View>,
    /// The action running in the background, for the status bar.
    pub busy: Option<String>,
    /// For worker threads to report back.
    events: Sender<Msg>,
    /// Why the client left, when it wasn't the user's `q`.
    exit_reason: Option<String>,
}

/// Runs the TUI until the user leaves it. The daemon keeps every pane.
pub fn run() -> Result<()> {
    let socket = crate::proto::socket_path();
    let exe = std::env::current_exe()?;
    let client = Client::connect_or_start(&socket, &exe)?;
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

    let leader = std::env::var("JW_LEADER")
        .ok()
        .and_then(|l| Leader::parse(&l))
        .unwrap_or(Leader::DEFAULT);

    log_panics();
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    let size = terminal.size()?;
    let mut app = App::new(client, events, leader, (size.width, size.height))?;
    let result = app.event_loop(&mut terminal, rx);
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    match (result, app.exit_reason) {
        (Err(e), _) => Err(e),
        (Ok(()), Some(why)) => anyhow::bail!(why),
        (Ok(()), None) => Ok(()),
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
            prev: None,
            daemon_panes: Vec::new(),
            active: None,
            panes: BTreeMap::new(),
            pending: VecDeque::new(),
            focus: None,
            full: false,
            status: None,
            size,
            quit: false,
            modal: None,
            view: None,
            busy: None,
            events,
            exit_reason: None,
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
                    match rx.recv_timeout(WHICH_AFTER.saturating_sub(at.elapsed())) {
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

    fn send(&mut self, msg: ClientMsg) {
        if let Err(e) = self.tx.send(&msg) {
            self.status = Some(format!("daemon: {e}"));
        }
    }

    /// Rebuilds the sidebar from the registry: one space per project.
    fn reload(&mut self) -> Result<()> {
        let reg = Registry::load(&registry::default_path()?)?;
        let mut by_project: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for e in reg.entries {
            by_project.entry(e.project.clone()).or_default().push(e);
        }
        self.rows.clear();
        // The free space is always there and always first (REQ-23).
        let mut free = crate::free::Sessions::load(&crate::free::default_path()?)?.sessions;
        free.sort_by(|a, b| a.name.cmp(&b.name));
        self.rows.push(Row::Space(crate::free::PROJECT.into()));
        self.rows
            .extend(free.iter().map(|s| Row::Stream(s.entry())));
        for (project, mut entries) in by_project {
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            self.rows.push(Row::Space(project));
            self.rows.extend(entries.into_iter().map(Row::Stream));
        }
        Ok(())
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
        self.rows.iter().filter_map(|r| match r {
            Row::Stream(e) => Some(e),
            Row::Space(_) => None,
        })
    }

    /// Whether the daemon runs panes for this stream.
    pub fn is_open(&self, id: &str) -> bool {
        self.daemon_panes.iter().any(|p| p.stream == id)
    }

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Daemon(d) => self.on_daemon(d),
            Msg::DaemonGone => {
                self.status = Some("the daemon closed the connection".into());
                self.exit_reason = Some("the daemon closed the connection".into());
                self.quit = true;
            }
            Msg::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            Msg::Term(Event::Paste(text)) => self.paste(&text),
            Msg::Term(Event::Resize(w, h)) => {
                self.size = (w, h);
                self.fit();
            }
            Msg::Term(_) => {}
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
                let mut parser = vt100::Parser::new(rows, cols, 0);
                parser.process(&bytes);
                self.panes.insert(
                    pane,
                    PaneView {
                        role,
                        parser,
                        exited: None,
                    },
                );
                self.fit();
            }
            DaemonMsg::Output { pane, bytes } => {
                if !self.panes.contains_key(&pane) {
                    // Output of a pane we're spawning can beat its Spawned.
                    let Some(p) = self.pending.front() else {
                        return;
                    };
                    let view = PaneView {
                        role: p.role.clone(),
                        parser: vt100::Parser::new(p.rows, p.cols, 0),
                        exited: None,
                    };
                    self.panes.insert(pane, view);
                }
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.parser.process(&bytes);
                }
            }
            DaemonMsg::Spawned { pane } => {
                if let Some(p) = self.pending.pop_front() {
                    self.panes.entry(pane).or_insert_with(|| PaneView {
                        role: p.role,
                        parser: vt100::Parser::new(p.rows, p.cols, 0),
                        exited: None,
                    });
                }
                if self.pending.is_empty() {
                    self.send(ClientMsg::List);
                }
            }
            DaemonMsg::Exited { pane, status } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.exited = Some(status);
                }
                self.send(ClientMsg::List);
            }
            DaemonMsg::Panes { panes } => {
                self.daemon_panes = panes;
                // Nothing on screen (just started, or the current one was
                // closed): show the first workspace that is running.
                if self.active.is_none() && self.modal.is_none() {
                    let running = self.workspaces().find(|e| self.is_open(&e.id)).cloned();
                    if let Some(e) = running {
                        self.open_entry(e, false);
                    }
                }
            }
            DaemonMsg::Prompted { .. } => self.status = Some("sent to the agent".into()),
            DaemonMsg::Error { msg } => self.status = Some(msg),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
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
        match &mut self.view {
            Some(View::Diff(d)) => match d.key(k) {
                diffview::Action::None => {}
                diffview::Action::Close => self.view = None,
                diffview::Action::Read(path) => {
                    if let Some(View::Diff(d)) = self.view.take() {
                        let dir = d.dir.clone();
                        self.read_file(&dir, &path, Some(Box::new(d)));
                    }
                }
            },
            Some(View::Md(m)) => {
                if m.key(k)
                    && let Some(View::Md(m)) = self.view.take()
                {
                    // Back to the diff it was opened from, if any.
                    self.view = m.back.map(|d| View::Diff(*d));
                }
            }
            None => self.send_key(&k),
        }
    }

    /// The key after the leader (the keymap in docs/specs/rust-tui.md).
    fn leader_key(&mut self, k: KeyEvent) {
        self.status = None;
        match k.code {
            KeyCode::Esc => {}
            KeyCode::Char('?') => {
                // The popup stays and the next key still counts as an action.
                self.which = true;
                self.leader_at = Some(Instant::now());
            }
            KeyCode::Char(c @ '1'..='9') => self.jump(c as usize - '1' as usize),
            KeyCode::Tab => self.go_back(),
            KeyCode::Char('h') => self.focus_towards(-1, 0),
            KeyCode::Char('l') => self.focus_towards(1, 0),
            KeyCode::Char('k') => self.focus_towards(0, -1),
            KeyCode::Char('j') => self.focus_towards(0, 1),
            KeyCode::Char('f') => {
                self.full = !self.full;
                self.fit();
            }
            KeyCode::Char('w') => self.new_worktree(),
            KeyCode::Char('s') => self.run_sync(),
            KeyCode::Char('d') => self.open_diff(),
            KeyCode::Char('x') => self.ask_close(),
            KeyCode::Char('X') => self.ask_remove(),
            KeyCode::Char('q') => self.quit = true,
            code => {
                let key = match code {
                    KeyCode::Char(' ') => "␣".to_string(),
                    KeyCode::Char(c) => c.to_string(),
                    other => format!("{other:?}").to_lowercase(),
                };
                let l = self.leader.label();
                self.status = Some(format!("{l} {key} does nothing · {l} ? shows the keys"));
            }
        }
    }

    /// `^␣ 1–9`: the workspace with that number in the sidebar.
    fn jump(&mut self, i: usize) {
        let target = self.workspaces().nth(i).cloned();
        match target {
            Some(e) => self.open_entry(e, false),
            None => self.status = Some(format!("no workspace {}", i + 1)),
        }
    }

    /// `^␣ tab`: the workspace before this one.
    fn go_back(&mut self) {
        let prev = self
            .prev
            .clone()
            .and_then(|p| self.workspaces().find(|e| e.id == p.id).cloned());
        match prev {
            Some(e) => self.open_entry(e, false),
            None => self.status = Some("no previous workspace yet".into()),
        }
    }

    /// `setup`: run the config's setup in the shell pane first (new streams).
    fn open_entry(&mut self, entry: Entry, setup: bool) {
        if let Some(cur) = self.current() {
            if cur.id == entry.id {
                return;
            }
            self.prev = Some(cur.clone());
        }
        let stream = match Stream::resolve(&entry) {
            Ok(s) => s,
            Err(e) => {
                self.status = Some(format!("{}: {e:#}", entry.name));
                return;
            }
        };
        self.send(ClientMsg::Detach);
        self.panes.clear();
        self.pending.clear();
        let already = self.is_open(&entry.id);
        self.active = Some(stream);
        self.full = false;

        if already {
            self.send(ClientMsg::Attach {
                stream: entry.id.clone(),
            });
        } else if let Err(e) = self.spawn_panes(setup) {
            self.status = Some(format!("{}: {e:#}", entry.name));
            return;
        }
        let roles = self.roles();
        self.focus = ["agent", "editor"]
            .into_iter()
            .find(|r| roles.iter().any(|x| x == r))
            .map(str::to_string)
            .or_else(|| roles.first().cloned());
    }

    fn spawn_panes(&mut self, setup: bool) -> Result<()> {
        let stream = self.active.clone().context("no stream")?;
        let (specs, note) = stream.open_specs(setup)?;
        if note.is_some() {
            self.status = note;
        }
        let rects = self.pane_rects();
        for spec in specs {
            let inner = rects
                .iter()
                .find(|(r, _)| *r == spec.role)
                .map(|(_, rect)| inner(*rect))
                .unwrap_or((80, 24));
            self.pending.push_back(Pending {
                role: spec.role.clone(),
                cols: inner.0,
                rows: inner.1,
            });
            self.send(spec.spawn(&stream.entry.id, inner));
        }
        stream.mark_opened()
    }

    /// The roles of the active stream, in layout order.
    pub fn roles(&self) -> Vec<String> {
        match &self.active {
            Some(s) => s.tree.leaves().into_iter().map(|l| l.role).collect(),
            None => Vec::new(),
        }
    }

    /// Where the panes go on screen: the area right of the sidebar and below
    /// the header, or the whole terminal in full mode.
    /// Where a viewer goes: right of the sidebar, whatever the panes do.
    pub fn view_area(&self) -> Rect {
        let (w, h) = self.size;
        Rect {
            x: SIDEBAR,
            y: 1,
            w: w.saturating_sub(SIDEBAR),
            h: h.saturating_sub(2),
        }
    }

    pub fn stage(&self) -> Rect {
        let (w, h) = self.size;
        if self.full {
            return Rect { x: 0, y: 0, w, h };
        }
        Rect {
            x: SIDEBAR,
            y: 1,
            w: w.saturating_sub(SIDEBAR),
            h: h.saturating_sub(2),
        }
    }

    /// Each role of the active stream with its rectangle, borders included.
    pub fn pane_rects(&self) -> Vec<(String, Rect)> {
        let Some(stream) = &self.active else {
            return Vec::new();
        };
        if self.full
            && let Some(f) = &self.focus
        {
            return vec![(f.clone(), self.stage())];
        }
        let tree: &Node = &stream.tree;
        tree.leaves()
            .into_iter()
            .map(|l| l.role)
            .zip(tree.rects(self.stage()))
            .collect()
    }

    /// Resizes every visible pane to its rectangle, here and in the daemon.
    fn fit(&mut self) {
        let rects = self.pane_rects();
        let mut resizes = Vec::new();
        for (id, view) in self.panes.iter_mut() {
            let Some((_, rect)) = rects.iter().find(|(r, _)| *r == view.role) else {
                continue;
            };
            let (cols, rows) = inner(*rect);
            if view.parser.screen().size() != (rows, cols) {
                view.parser.screen_mut().set_size(rows, cols);
                resizes.push(ClientMsg::Resize {
                    pane: *id,
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
        let f = self.focus.as_ref()?;
        self.panes
            .iter()
            .find(|(_, v)| &v.role == f)
            .map(|(id, _)| *id)
    }

    fn send_key(&mut self, k: &KeyEvent) {
        let Some(id) = self.focused_pane() else {
            return;
        };
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

    /// Moves focus to the nearest pane in a direction, by rectangle centres.
    fn focus_towards(&mut self, dx: i32, dy: i32) {
        let rects = self.pane_rects();
        let Some((_, from)) = rects.iter().find(|(r, _)| Some(r) == self.focus.as_ref()) else {
            return;
        };
        let centre = |r: &Rect| {
            (
                i32::from(r.x) * 2 + i32::from(r.w),
                i32::from(r.y) * 2 + i32::from(r.h),
            )
        };
        let (fx, fy) = centre(from);
        let best = rects
            .iter()
            .filter(|(r, _)| Some(r) != self.focus.as_ref())
            .filter_map(|(role, r)| {
                let (x, y) = centre(r);
                let (ex, ey) = (x - fx, y - fy);
                let along = ex * dx + ey * dy;
                (along > 0).then(|| (along + (ex * dy - ey * dx).abs() * 2, role))
            })
            .min_by_key(|(d, _)| *d);
        if let Some((_, role)) = best {
            self.focus = Some(role.clone());
        }
    }
}

impl App {
    /// The current workspace, for an action that needs a repository: a free
    /// session gets a word instead.
    fn current_repo_stream(&mut self, action: &str) -> Option<Entry> {
        let Some(entry) = self.current().cloned() else {
            let l = self.leader.label();
            self.status = Some(format!("no workspace open: {l} 1–9 opens one"));
            return None;
        };
        if crate::free::is_free(&entry) {
            self.status = Some(format!(
                "{} is not a git repository: {action} needs one",
                entry.name
            ));
            return None;
        }
        Some(entry)
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

    fn ask_close(&mut self) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        if !self.is_open(&entry.id) {
            self.status = Some(format!("{} is already closed", entry.name));
            return;
        }
        // REQ-12: anything but a shell in the foreground would be killed.
        let running: Vec<String> = self
            .daemon_panes
            .iter()
            .filter(|p| p.stream == entry.id)
            .filter_map(|p| {
                let fg = p.fg.as_deref()?;
                (!SHELLS.contains(&fg)).then(|| format!("{:<10} {fg}", p.role))
            })
            .collect();
        if running.is_empty() {
            self.close(&entry);
        } else {
            self.modal = Some(Modal::Close { entry, running });
        }
    }

    /// `X`: the done checks when the PR is merged, the rm checks otherwise
    /// (REQ-39).
    fn ask_remove(&mut self) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        if crate::free::is_free(&entry) {
            self.modal = Some(Modal::RmFree { entry });
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

    /// Opens `file` (relative to `dir`) in the reader; `back` is the diff
    /// to return to.
    fn read_file(&mut self, dir: &str, file: &str, back: Option<Box<diffview::DiffView>>) {
        match std::fs::read_to_string(std::path::Path::new(dir).join(file)) {
            Ok(src) => {
                self.view = Some(View::Md(mdview::MdView::new(file.to_string(), src, back)));
            }
            Err(e) => {
                self.status = Some(format!("{file}: {e}"));
                self.view = back.map(|d| View::Diff(*d));
            }
        }
    }

    /// `d`: the workspace's changes against its base, in the diff viewer.
    fn open_diff(&mut self) {
        let Some(entry) = self.current_repo_stream("diff") else {
            return;
        };
        self.background(format!("diffing {}", entry.name), move || {
            let stream = Stream::resolve(&entry)?;
            let files = crate::diff::load(std::path::Path::new(&entry.path), &stream.base)?;
            Ok(Job::Diff {
                title: entry.name.clone(),
                dir: entry.path,
                files,
            })
        });
    }

    fn run_sync(&mut self) {
        let Some(entry) = self.current_repo_stream("sync") else {
            return;
        };
        self.background(format!("syncing {}", entry.name), move || {
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
            Modal::RmFree { entry } => {
                self.close(&entry);
                let removed =
                    crate::free::default_path().and_then(|p| crate::free::remove(&p, &entry.id));
                self.status = Some(match removed {
                    Ok(()) => format!("removed free/{}; {} is untouched", entry.name, entry.path),
                    Err(e) => format!("{e:#}"),
                });
                let _ = self.reload();
            }
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
        let ids: Vec<PaneId> = self
            .daemon_panes
            .iter()
            .filter(|p| p.stream == entry.id)
            .map(|p| p.pane)
            .collect();
        for pane in ids {
            self.send(ClientMsg::Kill { pane });
        }
        self.daemon_panes.retain(|p| p.stream != entry.id);
        if self.active.as_ref().is_some_and(|s| s.entry.id == entry.id) {
            self.active = None;
            self.panes.clear();
            self.focus = None;
        }
        self.status = Some(format!("closed {}, worktree kept", entry.name));
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
                self.status = Some(format!("created {} on {}", entry.name, entry.branch));
                self.open_entry(entry, setup);
            }
            Job::Removed { name, note } => {
                self.status = Some(match note {
                    Some(n) => format!("removed {name}; {n}"),
                    None => format!("removed {name}"),
                });
            }
            Job::Failed(e) => self.status = Some(e),
            Job::Said(msg) => self.status = Some(msg),
            Job::Diff { title, dir, files } => {
                self.view = Some(View::Diff(diffview::DiffView::new(title, dir, files)));
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
                self.status = note;
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
            self.status = Some(format!("{e:#}"));
        }
    }
}

/// Shells don't count as "running something" when closing.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "nu"];

/// The terminal size inside a pane's border.
fn inner(r: Rect) -> (u16, u16) {
    (r.w.saturating_sub(2).max(1), r.h.saturating_sub(2).max(1))
}
