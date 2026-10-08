//! The client: sidebar of spaces → streams, the active stream's panes in
//! their splits, terminal and navigation modes (docs/specs/rust-tui.md, P5).
//!
//! Two threads feed one channel: the daemon's messages and the terminal's
//! events. The main thread applies them and redraws.

mod diffview;
mod draw;
mod keys;
mod modal;

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::{Node, Rect};
use crate::proto::{ClientMsg, DaemonMsg, PaneId, PaneInfo};
use crate::stream::Stream;

use modal::{Modal, NewForm, Outcome};

use keys::Leader;

/// Width of the sidebar, borders included.
const SIDEBAR: u16 = 28;

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
    /// Type this line into the stream's shell pane.
    RunInShell {
        entry: Entry,
        line: String,
    },
    Info {
        title: String,
        rows: Vec<(String, String)>,
    },
    Diff {
        title: String,
        dir: String,
        files: Vec<crate::diff::File>,
    },
    /// A project was added (and maybe given a drafted config): create its
    /// first stream.
    Added {
        project: Project,
        wrote: Option<String>,
    },
    Removed {
        name: String,
        note: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every key but the leader goes to the focused pane.
    Term,
    /// Keys move around and run actions.
    Nav,
}

/// What the stage shows instead of the panes.
pub enum View {
    Diff(diffview::DiffView),
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
    pub mode: Mode,
    pub rows: Vec<Row>,
    pub cursor: usize,
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
            mode: Mode::Nav,
            rows: Vec::new(),
            cursor: 0,
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
            let first = rx.recv().context("event channels closed")?;
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
        let selected = self.selected().map(|e| e.id.clone());
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
        self.cursor = selected
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|r| matches!(r, Row::Stream(e) if e.id == id))
            })
            .or_else(|| self.rows.iter().position(|r| matches!(r, Row::Stream(_))))
            .unwrap_or(0);
        Ok(())
    }

    pub fn selected(&self) -> Option<&Entry> {
        match self.rows.get(self.cursor) {
            Some(Row::Stream(e)) => Some(e),
            _ => None,
        }
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
            DaemonMsg::Panes { panes } => self.daemon_panes = panes,
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
        if let Some(View::Diff(d)) = &mut self.view {
            match d.key(k) {
                diffview::Action::None => {}
                diffview::Action::Close => self.view = None,
                diffview::Action::Read(path) => {
                    self.status = Some(format!("the reader for {path} comes with M"));
                }
            }
            return;
        }
        if self.leader.matches(&k) {
            self.mode = match self.mode {
                Mode::Term => Mode::Nav,
                Mode::Nav if self.active.is_some() => Mode::Term,
                Mode::Nav => Mode::Nav,
            };
            return;
        }
        match self.mode {
            Mode::Term => self.send_key(&k),
            Mode::Nav => self.nav_key(k),
        }
    }

    fn nav_key(&mut self, k: KeyEvent) {
        self.status = None;
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('j') | KeyCode::Down if !shift => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up if !shift => self.move_cursor(-1),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Esc => {
                if self.active.is_some() {
                    self.mode = Mode::Term;
                }
            }
            KeyCode::Char('H') | KeyCode::Left => self.focus_towards(-1, 0),
            KeyCode::Char('L') | KeyCode::Right => self.focus_towards(1, 0),
            KeyCode::Char('K') | KeyCode::Up => self.focus_towards(0, -1),
            KeyCode::Char('J') | KeyCode::Down => self.focus_towards(0, 1),
            KeyCode::Tab => self.next_pane(),
            KeyCode::Char('f') => {
                self.full = !self.full;
                self.fit();
            }
            KeyCode::Char('r') => self.ask_dev(),
            KeyCode::Char('S') => self.run_setup(),
            KeyCode::Char('i') => self.show_info(),
            KeyCode::Char('I') => {
                let here = std::env::current_dir()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                self.modal = Some(Modal::AddProject {
                    path: here,
                    in_repo: false,
                    error: None,
                });
            }
            KeyCode::Char('R') => {
                if let Err(e) = self.reload() {
                    self.status = Some(format!("{e:#}"));
                }
                self.send(ClientMsg::List);
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => self.ask_new(),
            KeyCode::Char('c') => self.ask_close(),
            KeyCode::Char('x') => self.ask_rm(),
            KeyCode::Char('s') => self.run_sync(),
            KeyCode::Char('D') => self.open_diff(),
            KeyCode::Char('d') => self.ask_done(),
            KeyCode::Char('p') => self.ask_prompt(),
            _ => {}
        }
    }

    fn move_cursor(&mut self, by: isize) {
        // Space rows can be selected too: `n` on one creates a stream there,
        // even when it has none yet.
        let i = self.cursor as isize + by;
        if i >= 0 && (i as usize) < self.rows.len() {
            self.cursor = i as usize;
        }
    }

    /// Shows the selected stream: attaches to its panes when the daemon has
    /// them, otherwise starts them from the layout.
    fn open_selected(&mut self) {
        if let Some(entry) = self.selected().cloned() {
            self.open_entry(entry, false);
        }
    }

    /// `setup`: run the config's setup in the shell pane first (new streams).
    fn open_entry(&mut self, entry: Entry, setup: bool) {
        if self.active.as_ref().is_some_and(|s| s.entry.id == entry.id) {
            self.mode = Mode::Term;
            return;
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
        self.mode = Mode::Term;
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
        if self.mode != Mode::Term {
            return;
        }
        let bytes = if self.panes[&id].parser.screen().bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        self.send(ClientMsg::Input { pane: id, bytes });
    }

    fn next_pane(&mut self) {
        let roles = self.roles();
        if roles.is_empty() {
            return;
        }
        let i = self
            .focus
            .as_ref()
            .and_then(|f| roles.iter().position(|r| r == f))
            .map_or(0, |i| (i + 1) % roles.len());
        self.focus = Some(roles[i].clone());
        self.fit();
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
    /// The project of the space under the cursor, from any of its worktrees.
    /// The selected stream for an action that needs a repository: a free
    /// session gets a word instead (REQ-24).
    fn selected_repo_stream(&mut self, action: &str) -> Option<Entry> {
        let entry = self.selected().cloned()?;
        if crate::free::is_free(&entry) {
            self.status = Some(format!(
                "{} is a free session: {action} needs a repository",
                entry.name
            ));
            return None;
        }
        Some(entry)
    }

    /// The space the cursor is in: its own row or the one above its stream.
    fn cursor_space(&self) -> Option<String> {
        self.rows[..=self.cursor.min(self.rows.len().saturating_sub(1))]
            .iter()
            .rev()
            .find_map(|r| match r {
                Row::Space(s) => Some(s.clone()),
                _ => None,
            })
    }

    fn cursor_project(&self) -> Result<Project> {
        let space = self.cursor_space().context("no space selected")?;
        let entry = self
            .rows
            .iter()
            .find_map(|r| match r {
                Row::Stream(e) if e.project == space => Some(e),
                _ => None,
            })
            .context("no stream to find the repository from")?;
        Project::open(std::path::Path::new(&entry.path))
    }

    fn ask_new(&mut self) {
        if self.cursor_space().as_deref() == Some(crate::free::PROJECT) {
            self.modal = Some(Modal::NewFree {
                fields: [String::new(), "~".into()],
                field: 0,
                layout: 0,
                error: None,
            });
            return;
        }
        match self.cursor_project() {
            Ok(p) => self.modal = Some(Modal::New(NewForm::new(p))),
            Err(e) => self.status = Some(format!("new: {e:#}")),
        }
    }

    fn ask_close(&mut self) {
        let Some(entry) = self.selected().cloned() else {
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

    fn ask_rm(&mut self) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        if crate::free::is_free(&entry) {
            self.modal = Some(Modal::RmFree { entry });
            return;
        }
        let plan = Project::open(std::path::Path::new(&entry.path))
            .or_else(|_| self.cursor_project())
            .and_then(|p| Ok((actions::rm_plan(&p, &entry)?, p)));
        match plan {
            Ok((plan, project)) => {
                self.modal = Some(Modal::Rm {
                    entry,
                    project,
                    plan,
                    typed: String::new(),
                    error: None,
                })
            }
            Err(e) => self.status = Some(format!("rm {}: {e:#}", entry.name)),
        }
    }

    /// `r`: start a dev service in the stream's shell pane; with several
    /// services, pick one first.
    fn ask_dev(&mut self) {
        let Some(entry) = self.selected_repo_stream("dev") else {
            return;
        };
        let project = match Project::open(std::path::Path::new(&entry.path)) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("{e:#}"));
                return;
            }
        };
        let services: Vec<String> = project.cfg.dev.keys().cloned().collect();
        match services.as_slice() {
            [] => self.status = Some("no [dev] services in the config".into()),
            [one] => self.start_dev(entry, one.clone()),
            _ => {
                self.modal = Some(Modal::Pick {
                    title: format!("Dev · {}", entry.name),
                    items: services,
                    cursor: 0,
                    entry,
                })
            }
        }
    }

    fn start_dev(&mut self, entry: Entry, service: String) {
        if let Err(e) = self.shell_pane(&entry) {
            self.status = Some(e);
            return;
        }
        self.background(format!("checking ports for {service}"), move || {
            let p = Project::open(std::path::Path::new(&entry.path))?;
            let stream = Stream::resolve(&entry)?;
            let vars = stream.vars();
            let reg = Registry::load(&registry::default_path()?)?;
            actions::check_ports(
                &p,
                &entry,
                &service,
                &vars,
                &crate::connectors::system::System,
                &reg,
            )?;
            let cmds = actions::dev_commands(&p.cfg, &service, &vars)?;
            Ok(Job::RunInShell {
                entry,
                line: actions::dev_line(&cmds),
            })
        });
    }

    /// `S`: re-run the config's setup in the stream's shell pane.
    fn run_setup(&mut self) {
        let Some(entry) = self.selected_repo_stream("setup") else {
            return;
        };
        let line = Stream::resolve(&entry).and_then(|s| actions::setup_line(&s.cfg, &s.vars()));
        match line {
            Ok(Some(line)) => self.run_in_shell(&entry, &line),
            Ok(None) => self.status = Some("no setup in the config".into()),
            Err(e) => self.status = Some(format!("{e:#}")),
        }
    }

    fn show_info(&mut self) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        let open = self.is_open(&entry.id);
        if crate::free::is_free(&entry) {
            let layout =
                crate::free::Sessions::load(&crate::free::default_path().unwrap_or_default())
                    .ok()
                    .and_then(|all| all.get(&entry.id).map(|s| s.layout.label()))
                    .unwrap_or("?");
            let rows = vec![
                ("id".to_string(), entry.id.clone()),
                ("dir".to_string(), entry.path.clone()),
                ("layout".to_string(), layout.to_string()),
                (
                    "session".to_string(),
                    if open { "open" } else { "closed" }.to_string(),
                ),
            ];
            self.modal = Some(Modal::Info {
                title: format!("free/{}", entry.name),
                rows,
            });
            return;
        }
        self.background(format!("reading {}", entry.name), move || {
            let p = Project::open(std::path::Path::new(&entry.path))?;
            let prs = crate::connectors::github::Client::default();
            let mut rows = actions::info(&p, &entry, &prs, &crate::connectors::system::System);
            let state = if open { "open" } else { "closed" };
            rows.insert(5, ("stream".into(), state.into()));
            Ok(Job::Info {
                title: entry.name,
                rows,
            })
        });
    }

    /// The stream's shell pane, when it is open and idle at its prompt.
    fn shell_pane(&self, entry: &Entry) -> Result<PaneId, String> {
        let pane = self
            .daemon_panes
            .iter()
            .find(|p| p.stream == entry.id && p.role == "shell")
            .ok_or_else(|| format!("{} has no shell pane open: ↵ opens it", entry.name))?;
        if let Some(fg) = pane.fg.as_deref()
            && !SHELLS.contains(&fg)
        {
            return Err(format!(
                "the shell pane of {} is busy running {fg}",
                entry.name
            ));
        }
        Ok(pane.pane)
    }

    /// Types a command line into the stream's shell pane and shows it.
    fn run_in_shell(&mut self, entry: &Entry, line: &str) {
        let pane = match self.shell_pane(entry) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(e);
                return;
            }
        };
        self.send(ClientMsg::Input {
            pane,
            bytes: format!("{line}\r").into_bytes(),
        });
        if self.active.as_ref().is_some_and(|s| s.entry.id == entry.id) {
            self.focus = Some("shell".into());
        } else {
            self.open_entry(entry.clone(), false);
            self.focus = Some("shell".into());
        }
        self.status = Some(format!("$ {line}"));
        self.send(ClientMsg::List);
    }

    /// `D`: the stream's changes against its base, in the diff viewer.
    fn open_diff(&mut self) {
        let Some(entry) = self.selected_repo_stream("diff") else {
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
        let Some(entry) = self.selected_repo_stream("sync") else {
            return;
        };
        self.background(format!("syncing {}", entry.name), move || {
            let p = Project::open(std::path::Path::new(&entry.path))?;
            let done = actions::sync(&p, &entry)?;
            Ok(Job::Said(format!("{}: {}", entry.name, done.message())))
        });
    }

    fn ask_done(&mut self) {
        let Some(entry) = self.selected_repo_stream("done") else {
            return;
        };
        self.background(format!("checking {}'s PR", entry.name), move || {
            let project = Project::open(std::path::Path::new(&entry.path))?;
            let prs = crate::connectors::github::Client::default();
            let (plan, pr) = actions::done_plan(&project, &entry, &prs)?;
            Ok(Job::DoneReady {
                entry,
                project,
                plan,
                pr,
            })
        });
    }

    fn ask_prompt(&mut self) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        if !self.is_open(&entry.id) {
            self.status = Some(format!("{} is not open: ↵ opens it", entry.name));
            return;
        }
        self.modal = Some(Modal::Prompt {
            entry,
            text: String::new(),
        });
    }

    fn submit_modal(&mut self) {
        let Some(m) = self.modal.take() else {
            return;
        };
        match m {
            Modal::New(mut form) => {
                let o = NewOptions {
                    name: form.fields[0].trim().to_string(),
                    branch: form.fields[1].trim().to_string(),
                    from: form.fields[2].trim().to_string(),
                };
                if !actions::valid_name(&o.name) {
                    form.error = Some("name: lowercase letters, digits and dashes".into());
                    self.modal = Some(Modal::New(form));
                    return;
                }
                let (project, setup) = (form.project, form.setup);
                self.background(format!("creating {}", o.name), move || {
                    let reg = registry::default_path()?;
                    let entry = actions::new_stream(&project, &o, &reg)?;
                    Ok(Job::Created { entry, setup })
                });
            }
            Modal::Close { entry, .. } => self.close(&entry),
            Modal::Pick {
                items,
                cursor,
                entry,
                ..
            } => {
                if let Some(service) = items.get(cursor).cloned() {
                    self.start_dev(entry, service);
                }
            }
            Modal::Info { .. } => {}
            Modal::NewFree {
                fields,
                field,
                layout,
                ..
            } => {
                let path = match crate::free::default_path() {
                    Ok(p) => p,
                    Err(e) => {
                        self.status = Some(format!("{e:#}"));
                        return;
                    }
                };
                let chosen = crate::free::Layout::ALL[layout];
                match crate::free::create(&path, fields[0].trim(), &fields[1], chosen) {
                    Ok(session) => {
                        let _ = self.reload();
                        let entry = session.entry();
                        if let Some(i) = self
                            .rows
                            .iter()
                            .position(|r| matches!(r, Row::Stream(e) if e.id == entry.id))
                        {
                            self.cursor = i;
                        }
                        self.open_entry(entry, false);
                    }
                    Err(e) => {
                        self.modal = Some(Modal::NewFree {
                            error: Some(format!("{e:#}")),
                            fields,
                            field,
                            layout,
                        })
                    }
                }
            }
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
            Modal::AddProject { path, in_repo, .. } => {
                self.background("adding the project".into(), move || {
                    let dir = std::path::PathBuf::from(path.trim());
                    let repo = crate::connectors::git::Repo::open(&dir)?;
                    let name = crate::connectors::git::project_name(&repo.remote);
                    // A config already there (repo or personal) is kept as is.
                    let has_config = crate::core::config::load(&repo.root, &repo.remote, &name)?
                        .source
                        .is_some();
                    let wrote = if has_config {
                        None
                    } else {
                        let done = crate::init::init(&repo, in_repo, false)?;
                        Some(done.path.display().to_string())
                    };
                    Ok(Job::Added {
                        project: Project::open(&repo.root)?,
                        wrote,
                    })
                });
            }
            Modal::Prompt { entry, text } => {
                self.status = Some(format!("waiting for {}'s agent to settle…", entry.name));
                self.send(ClientMsg::Prompt {
                    stream: entry.id,
                    text: text.trim().to_string(),
                });
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
            self.mode = Mode::Nav;
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
                if let Some(i) = self
                    .rows
                    .iter()
                    .position(|r| matches!(r, Row::Stream(e) if e.id == entry.id))
                {
                    self.cursor = i;
                }
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
            Job::RunInShell { entry, line } => self.run_in_shell(&entry, &line),
            Job::Info { title, rows } => self.modal = Some(Modal::Info { title, rows }),
            Job::Diff { title, dir, files } => {
                self.view = Some(View::Diff(diffview::DiffView::new(title, dir, files)));
                self.mode = Mode::Nav;
            }
            Job::Added { project, wrote } => {
                self.status = Some(match wrote {
                    Some(path) => format!("wrote {path}: fill in [ports] and [dev] there"),
                    None => format!("{} already has a config", project.name),
                });
                self.modal = Some(Modal::New(NewForm::new(project)));
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
