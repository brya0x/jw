//! The client: the sidebar of workspaces, the current one's panes in their
//! splits, and a one-shot leader key for every action (docs/specs/rust-tui.md).
//!
//! Threads feed one channel: the daemon's messages (parsed into a `Parser`
//! when they are a pane's screen), the terminal's events, a tick, and what
//! workers resolve or replay off the UI thread. The main thread applies them
//! and redraws.

mod anim;
mod diffview;
mod draw;
mod finder;
mod keys;
mod mdview;
mod mobile;
mod modal;
mod prcache;
mod registry_watch;
mod select;
mod settings_view;
mod setup;
mod trace;

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};

use crate::actions::{self, NewOptions, Project};
use crate::client::Client;
use crate::connectors::PullRequests;
use crate::core::registry::{self, Entry, Registry};
use crate::layout::{Dir, Node, Rect, Tree};
use crate::program;
use crate::proto::{
    AgentState, ClientMsg, DaemonMsg, NewPane, PROTOCOL, PaneId, PaneInfo, PaneLeaf,
};
use crate::stream::Stream;

use modal::{Modal, Outcome};
use select::{Pos, Sel};

use keys::Leader;

/// Width of the sidebar, borders included.
const SIDEBAR: u16 = 28;

/// How long the leader waits before showing every key (REQ-31).
fn which_after() -> Duration {
    crate::settings::get().which_delay()
}

/// Why the loop wakes without a message.
enum Wake {
    Edge,
    Which,
    Frame,
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
    /// The sidebar's rows, read again off the UI thread because another
    /// process rewrote the registry (addendum 14).
    Rows(Result<Vec<Row>>),
    /// The pull requests of the worktrees, by workspace id.
    Prs(Vec<(String, Option<crate::connectors::Pr>)>),
    /// A sidebar workspace's stream, resolved on a worker thread.
    Stream(Box<Stream>),
    /// A pane's `Snapshot`, already replayed into its parser by the daemon's
    /// reader thread (REQ-186): the replay of up to 2 MiB stays off the UI thread.
    Snapshot {
        pane: PaneId,
        role: String,
        parser: Box<vt100::Parser<keys::Flags>>,
        /// Set for a `Screen`: its `History` is on the way, with this number.
        history: Option<u64>,
    },
    /// The pane's output and screen replayed in full, with the scrollback.
    History {
        pane: PaneId,
        seq: u64,
        parser: Box<vt100::Parser<keys::Flags>>,
    },
}

/// What the status bar says while gh is asked about a PR, before the name.
const PR_BUSY: &str = "checking the PR of ";

// One job at a time crosses the channel, boxed; its size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Job {
    Created {
        entry: Entry,
        setup: bool,
        /// The `.jw.toml` the setup step wrote.
        wrote: Option<String>,
    },
    /// What the setup step starts from, found off the UI thread.
    SetupReady(Box<setup::Setup>),
    /// What gh said about the PR of the branch a remove modal is open on
    /// (REQ-259). Only that modal's PR line takes it.
    RmPr {
        id: String,
        line: modal::PrLine,
    },
    /// What removing a session closes and forgets, read off the UI thread.
    SessionRm {
        name: String,
        plan: crate::session::RmPlan,
    },
    /// A finished action with nothing else to do but say so.
    Said(String),
    /// The open PRs gh listed for the picker (REQ-160); `requested` is the
    /// list asked for.
    Prs {
        prs: Vec<crate::connectors::github::PrSummary>,
        requested: bool,
    },
    /// The review workspace is ready to open (REQ-166).
    Reviewed(Box<crate::review::Reviewed>),
    Diff {
        name: String,
        base: String,
        dir: String,
        /// General, which a diff pane opens on (REQ-222); git's complaint
        /// when it failed (REQ-246).
        general: Result<crate::diff::Loaded, String>,
        /// The viewer pane to fill, when it is already in the tree (a
        /// client attaching to a workspace that shows one).
        into: Option<PaneId>,
    },
    /// What git work a diff pane asked for found (addendum 17).
    DiffPart {
        pane: PaneId,
        slot: diffview::Slot,
        seq: u64,
        result: Result<diffview::Part, String>,
    },
    /// A worktree has its new name and folder; `from` is the old folder.
    Renamed {
        entry: Entry,
        from: String,
    },
    /// The project of the remove modal open on `id`, which gh and the removal
    /// need (REQ-268).
    RmProject {
        id: String,
        project: Project,
    },
    /// What git found in the worktree of the remove modal that is open on
    /// `id`; only that modal's plan takes it (REQ-268). An error closes it.
    RmWork {
        id: String,
        plan: Result<actions::RmPlan, String>,
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
/// listed under its project (REQ-50). Grouped in `core::sidebar`, which the
/// phone's `workspaces` answer is made from too.
pub use crate::sidebar::Row;

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

/// How often a drag held above or below a pane scrolls it one line (REQ-127).
const EDGE_EVERY: Duration = Duration::from_millis(60);

/// Two clicks this close, on the same cell, are a double click (REQ-126).
const CLICK_WITHIN: Duration = Duration::from_millis(400);

/// The longest a burst of messages holds back the next frame.
const FRAME: Duration = Duration::from_millis(16);

/// Applies what is queued, waiting up to 2 ms for each next message, so a
/// burst of output costs one frame, not one per chunk; but for `budget` at
/// most, so output that never pauses still redraws. `apply` returns false
/// to stop.
fn drain<T>(rx: &Receiver<T>, budget: Duration, mut apply: impl FnMut(T) -> bool) {
    let until = Instant::now() + budget;
    while let Some(left) = until.checked_duration_since(Instant::now())
        && let Ok(m) = rx.recv_timeout(left.min(Duration::from_millis(2)))
    {
        if !apply(m) {
            return;
        }
    }
}

/// How long a status message stays.
const STATUS_FOR: Duration = Duration::from_secs(4);

/// How long the copied banner stays (it leaves on the next 500 ms tick).
const BANNER_FOR: Duration = Duration::from_millis(1500);

/// What happened to a pane's screen since its `Screen`, in order.
pub struct Pending {
    seq: u64,
    ops: Vec<Op>,
    bytes: usize,
}

enum Op {
    Out(Vec<u8>),
    Size(u16, u16),
}

/// Past this much output waiting for its history, the pane stays without.
const PENDING_MAX: usize = 8 << 20;

impl Pending {
    fn out(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len();
        match self.ops.last_mut() {
            Some(Op::Out(last)) => last.extend_from_slice(bytes),
            _ => self.ops.push(Op::Out(bytes.to_vec())),
        }
    }
}

/// A pane of the current workspace as this client sees it.
pub struct PaneView {
    pub role: String,
    pub parser: vt100::Parser<keys::Flags>,
    /// While its scrollback is being rebuilt (REQ-193): `parser` holds the screen
    /// only, and what happens to it meanwhile is kept to redo on the full one.
    pub history: Option<Pending>,
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
    /// An arrow followed the leader: more arrows keep moving through the
    /// sidebar until another key (REQ-112).
    pub walking: bool,
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
    /// The text selected in a pane (addendum 12).
    sel: Option<Sel>,
    /// The button is down and the selection follows the pointer.
    selecting: bool,
    /// The drag is held above (-1) or below (1) the pane: it scrolls (REQ-127).
    edge: Option<isize>,
    /// The last left click: when, where and how many in a row.
    clicks: Option<(Instant, u16, u16, u8)>,
    /// What the last copy sent, to show its banner on that pane (REQ-135).
    copied: Option<(PaneId, usize, Instant)>,
    /// `f`: only the focused pane, across the whole terminal.
    pub full: bool,
    pub status: Option<Status>,
    /// The terminal's size, for the layout.
    pub size: (u16, u16),
    quit: bool,
    pub modal: Option<Modal>,
    /// Mobile as the daemon last said, and the questions waiting (REQ-42).
    pub mobile: mobile::Mobile,
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
    /// What to tell the agent of a review once its workspace's panes are
    /// open: the workspace id and the text (REQ-171).
    pending_prompt: Option<(String, String)>,
    /// The action running in the background, for the status bar.
    pub busy: Option<String>,
    /// For worker threads to report back.
    events: Sender<Msg>,
    /// Why the client left, when it wasn't the user's `q`.
    exit_reason: Option<String>,
    /// The settings and theme files, to apply their changes.
    watch: crate::settings::Watch,
    /// `workspaces.json` and `folders.json`, to follow what others write.
    regwatch: registry_watch::RegistryWatch,
    /// Rows read after an outside change, held while a modal, picker or
    /// the settings screen is open (REQ-154).
    pending_rows: Option<Vec<Row>>,
    /// Just started, or just switched session: with nothing running, the
    /// next List opens the session's last workspace.
    fresh: bool,
    /// Whether a transition is running, for the loop's frame deadline.
    motion: anim::Motion,
    /// When the last frame began, or the deadline it kept (the next is due
    /// `FRAME` later, REQ-137).
    last_draw: Instant,
    /// The last frame, kept only under `JW_TRACE_CELLS`, to count the cells a draw changed.
    traced: Option<ratatui::buffer::Buffer>,
    /// Streams resolved off the UI thread, by workspace id (REQ-185): a switch to
    /// an open workspace reuses one instead of running `git` on the UI thread.
    streams: BTreeMap<String, Stream>,
    /// Ids already sent to be resolved, so `reload` asks once.
    warming: std::collections::BTreeSet<String>,
    /// When a held drag next scrolls (REQ-127).
    edge_due: Option<Instant>,
    /// The sidebar's selection band on its way to the current row (REQ-136).
    bar: Option<Bar>,
    /// The row the band is drawn on this frame.
    band: Option<usize>,
    /// A workspace switch under way, 0 to 1, and which way it went (REQ-136).
    switch: Option<(anim::Tween, i8)>,
    /// The agent's spinner glyph, while one is working (REQ-143).
    spinner: Option<char>,
    /// When the spinner steps next.
    spin_due: Option<Instant>,
    /// When the TUI started, the spinner's clock.
    born: Instant,
    /// The status bar's mode and the one before it (REQ-140).
    mode: Option<draw::ModeFade>,
    /// Which overlays were open at the last frame, one bit each: modal,
    /// picker, settings, which-key.
    overlays: u8,
    /// A popup fading in, 0 to 1 (REQ-144).
    popup: Option<anim::Tween>,
    /// The pane that was focused, and the one that is, mid-change (REQ-145).
    focus_fade: Option<(Option<PaneId>, Option<PaneId>, anim::Tween)>,
    /// The focus at the last frame.
    last_focus: Option<PaneId>,
    /// A turn of the wheel being spread over a few frames (REQ-145).
    scroll: Option<Scroll>,
    /// The first-run welcome, since when it has been showing (REQ-146).
    welcome: Option<Instant>,
}

/// Lines of history to move in a pane, and how many have moved.
struct Scroll {
    pane: PaneId,
    total: isize,
    applied: isize,
    since: Instant,
}

/// The selection band: the workspace it is on, its row, and how it moves.
struct Bar {
    id: String,
    idx: usize,
    tween: anim::Tween,
}

/// Runs the TUI until the user leaves it. The daemon keeps every pane.
pub fn run() -> Result<()> {
    let socket = crate::proto::socket_path();
    let exe = std::env::current_exe()?;
    let client = Client::connect_or_start(&socket, &exe)?;
    hello(&client, &socket)?;
    client.try_clone()?.send(&ClientMsg::SplitSnapshots)?;
    let mut reader = client.try_clone()?;

    let (tx, rx) = mpsc::channel();
    let to_main = tx.clone();
    thread::spawn(move || {
        // Which `Screen` a pane's `History` belongs to.
        let mut screens = 0u64;
        let mut seqs: BTreeMap<PaneId, u64> = BTreeMap::new();
        loop {
            match reader.recv() {
                Ok(Some(m)) => {
                    if trace::on() {
                        match &m {
                            DaemonMsg::Snapshot { bytes, .. } => {
                                trace::mark("rx_snapshot", bytes.len() as u64)
                            }
                            DaemonMsg::Tree { .. } => trace::mark("rx_tree", 0),
                            _ => {}
                        }
                    }
                    // Messages keep their order: this thread sends the
                    // parser where the Snapshot was, and the Output behind it.
                    let m = match m {
                        DaemonMsg::Snapshot {
                            pane,
                            role,
                            cols,
                            rows,
                            bytes,
                        } => Msg::Snapshot {
                            pane,
                            role,
                            parser: Box::new(replay("snap_parse", rows, cols, &bytes)),
                            history: None,
                        },
                        DaemonMsg::Screen {
                            pane,
                            role,
                            cols,
                            rows,
                            bytes,
                        } => {
                            screens += 1;
                            seqs.insert(pane, screens);
                            Msg::Snapshot {
                                pane,
                                role,
                                parser: Box::new(replay("snap_parse", rows, cols, &bytes)),
                                history: Some(screens),
                            }
                        }
                        // Its replay takes a while: on its own thread, so
                        // the Output behind it is not held up.
                        DaemonMsg::History {
                            pane,
                            cols,
                            rows,
                            bytes,
                        } => {
                            if let Some(&seq) = seqs.get(&pane) {
                                let to_main = to_main.clone();
                                thread::spawn(move || {
                                    let parser =
                                        Box::new(replay("history_parse", rows, cols, &bytes));
                                    let _ = to_main.send(Msg::History { pane, seq, parser });
                                });
                            }
                            continue;
                        }
                        m => Msg::Daemon(m),
                    };
                    if to_main.send(m).is_err() {
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
    // Asked before the reader starts: crossterm's probe blocks while
    // another thread is in `event::read`.
    let kitty = terminal::supports_keyboard_enhancement().unwrap_or(false);
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
    // REQ-90: Shift+Enter and the like, where the terminal can tell them
    // apart. After init, so it lands on the alternate screen's stack (RAT-18).
    if kitty {
        execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let size = terminal.size()?;
    let mut app = App::new(client, events, leader, (size.width, size.height))?;
    if let Some(e) = theme_error {
        app.fail(format!("{e:#}"));
    }
    let result = app.event_loop(&mut terminal, rx);
    let _ = execute!(std::io::stdout(), terminal::EndSynchronizedUpdate);
    if kitty {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
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
        // REQ-139: a frame left open would freeze the screen.
        let _ = execute!(std::io::stdout(), terminal::EndSynchronizedUpdate);
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
        let mut app = Self::bare(tx, events, leader, size);
        app.recent = load_recent();
        app.reload()?;
        let skip = std::env::var("JW_WELCOME").is_ok_and(|v| v == "0");
        if app.rows.is_empty() && !crate::settings::get().welcomed() && !skip {
            app.welcome = Some(Instant::now());
        }
        app.send(ClientMsg::List);
        // Right after `Hello` (in `run`): the indicator and the pair question
        // need it (REQ-42).
        app.send(ClientMsg::MobileWatch);
        Ok(app)
    }

    /// An app with nothing loaded: no registry read, no message sent.
    fn bare(tx: Client, events: Sender<Msg>, leader: Leader, size: (u16, u16)) -> Self {
        Self {
            tx,
            leader,
            leader_at: None,
            which: false,
            walking: false,
            rows: Vec::new(),
            recent: Vec::new(),
            daemon_panes: Vec::new(),
            active: None,
            panes: BTreeMap::new(),
            tree: None,
            focus: None,
            focus_new: false,
            dragging: None,
            sel: None,
            selecting: false,
            edge: None,
            clicks: None,
            copied: None,
            full: false,
            status: None,
            size,
            quit: false,
            modal: None,
            mobile: mobile::Mobile::default(),
            finder: None,
            settings: None,
            here: BTreeMap::new(),
            prs: BTreeMap::new(),
            ticks: 0,
            told: None,
            views: BTreeMap::new(),
            pending_view: None,
            pending_prompt: None,
            busy: None,
            events,
            exit_reason: None,
            watch: crate::settings::Watch::new(),
            regwatch: registry_watch::RegistryWatch::new(),
            pending_rows: None,
            fresh: true,
            motion: anim::Motion::default(),
            last_draw: Instant::now(),
            traced: None,
            streams: BTreeMap::new(),
            warming: Default::default(),
            edge_due: None,
            bar: None,
            band: None,
            switch: None,
            spinner: None,
            spin_due: None,
            born: Instant::now(),
            mode: None,
            overlays: 0,
            popup: None,
            focus_fade: None,
            last_focus: None,
            scroll: None,
            welcome: None,
        }
    }

    /// Draws one frame, wrapped in a synchronized update so the terminal
    /// paints it whole or not at all (REQ-139). The frame is closed on every
    /// path, a failed draw included.
    fn draw(&mut self, term: &mut ratatui::DefaultTerminal) -> Result<()> {
        let mut out = std::io::stdout();
        // The next frame is due FRAME after this one *began*: counted from the
        // end of the draw, the cadence was 16 ms plus the draw plus the wake's slop.
        let began = Instant::now();
        let due = self.last_draw + FRAME;
        self.last_draw = if began >= due && began < due + FRAME {
            due
        } else {
            began
        };
        let drawn = trace::span("draw_total", || {
            anim::synced(&mut out, || {
                trace::span("term_draw", || {
                    term.draw(|f| trace::span("draw_build", || draw::draw(f, self)))
                })
            })
        });
        if trace::on()
            && std::env::var_os("JW_TRACE_CELLS").is_some()
            && let Ok(frame) = &drawn
        {
            let cells = self
                .traced
                .as_ref()
                .map_or(frame.buffer.content.len(), |old| {
                    old.diff(frame.buffer).len()
                });
            trace::mark("diff_cells", cells as u64);
            self.traced = Some(frame.buffer.clone());
        }
        drawn?;
        Ok(())
    }

    /// The soonest thing the loop must wake for without a message: a held
    /// drag scrolling, the which-key popup, or the next frame of a transition
    /// (REQ-137). `None` blocks until a message comes, so an idle jw costs
    /// nothing (RAT-29).
    fn next_wake(&mut self, now: Instant) -> Option<(Instant, Wake)> {
        match (self.edge.is_some(), self.edge_due) {
            (false, _) => self.edge_due = None,
            (true, None) => self.edge_due = Some(now + EDGE_EVERY),
            (true, Some(_)) => {}
        }
        let mut wake: Option<(Instant, Wake)> = None;
        let mut soonest = |at: Instant, w: Wake| {
            if wake.as_ref().is_none_or(|(best, _)| at < *best) {
                wake = Some((at, w));
            }
        };
        if let Some(at) = self.edge_due {
            soonest(at, Wake::Edge);
        }
        if let Some(at) = self.leader_at
            && !self.which
        {
            soonest(at + which_after(), Wake::Which);
        }
        if self.motion.active(now) {
            soonest(self.last_draw + FRAME, Wake::Frame);
        }
        if let Some(at) = self.spin_due {
            soonest(at, Wake::Frame);
        }
        if let Some(st) = &self.status {
            let fade_at = st.at + STATUS_FOR.saturating_sub(anim::MESSAGE_EDGE);
            if fade_at > now {
                soonest(fade_at, Wake::Frame);
            }
        }
        wake
    }

    /// The focused pane's border eases to its colour (REQ-145). A switch of
    /// workspace brings its own motion, so it does not add this one.
    fn tick_focus(&mut self, now: Instant, on: bool) {
        if self.focus != self.last_focus {
            if on && anim::fades() && self.switch.is_none() {
                let ease = anim::Tween::new(0.0, 1.0, anim::FOCUS_EASE, now);
                self.focus_fade = Some((self.last_focus, self.focus, ease));
                self.motion.kick(anim::FOCUS_EASE, now);
            }
            self.last_focus = self.focus;
        }
        if self.focus_fade.is_some_and(|(_, _, t)| t.done(now)) {
            self.focus_fade = None;
        }
    }

    /// Starts spreading a turn of the wheel over a few frames; one that
    /// comes mid-way adds what is left to its own lines.
    fn glide_scroll(&mut self, pane: PaneId, lines: isize, now: Instant) {
        let left = match &self.scroll {
            Some(s) if s.pane == pane => s.total - s.applied,
            _ => 0,
        };
        self.scroll = Some(Scroll {
            pane,
            total: left + lines,
            applied: 0,
            since: now,
        });
        self.motion.kick(anim::SCROLL, now);
    }

    /// Moves the history the lines the wheel is due by now (REQ-145).
    fn tick_scroll(&mut self, now: Instant) {
        let Some(s) = &mut self.scroll else {
            return;
        };
        let elapsed = now.saturating_duration_since(s.since);
        let t = anim::ease_out(anim::progress(elapsed, anim::SCROLL));
        let step = (t * s.total as f32).round() as isize - s.applied;
        s.applied += step;
        let pane = s.pane;
        if t >= 1.0 {
            self.scroll = None;
        } else {
            self.motion.kick(anim::SCROLL.saturating_sub(elapsed), now);
        }
        match self.panes.get_mut(&pane) {
            Some(view) if step != 0 => {
                let screen = view.parser.screen_mut();
                let back = screen.scrollback().saturating_add_signed(step);
                screen.set_scrollback(back);
            }
            Some(_) => {}
            None => self.scroll = None,
        }
    }

    /// The welcome steps in one by one.
    fn tick_welcome(&mut self, now: Instant, on: bool) {
        let Some(since) = self.welcome else {
            return;
        };
        let shown = anim::WELCOME_STEP * 3 + anim::WELCOME_FADE;
        let age = now.saturating_duration_since(since);
        if on && age < shown {
            self.motion.kick(shown - age, now);
        }
    }

    /// Closes the welcome for good (REQ-146).
    fn dismiss_welcome(&mut self) {
        self.welcome = None;
        let mut s = (*crate::settings::get()).clone();
        s.welcomed = Some(true);
        if let Err(e) = crate::settings::save(s) {
            self.fail(format!(
                "could not remember the welcome ({e:#}); check that ~/.config/jw is writable"
            ));
        }
    }

    /// Popups: one that opens fades in (REQ-144), and a picker's highlight
    /// slides to its selection.
    fn tick_overlays(&mut self, now: Instant, on: bool) {
        let open = u8::from(self.modal.is_some())
            | u8::from(self.finder.is_some()) << 1
            | u8::from(self.settings.is_some()) << 2
            | u8::from(self.which) << 3;
        if open & !self.overlays != 0 && on && anim::fades() {
            self.popup = Some(anim::Tween::new(0.0, 1.0, anim::POPUP_FADE, now));
            self.motion.kick(anim::POPUP_FADE, now);
        }
        self.overlays = open;
        if self.popup.is_some_and(|t| t.done(now)) {
            self.popup = None;
        }
        if let Some(finder) = self.finder.as_mut()
            && finder.glide(now, on)
        {
            self.motion.kick(anim::PICK_SLIDE, now);
        }
    }

    /// The status bar's motion: a change of mode (REQ-140) and a message
    /// entering or leaving (REQ-142).
    fn tick_status(&mut self, now: Instant, on: bool) {
        let cur = draw::mode_of(self);
        match &mut self.mode {
            Some(m) if m.mode == cur => {}
            slot => {
                let prev = slot.as_ref().map_or(cur, |m| m.mode);
                let fade = on && anim::fades() && slot.is_some();
                *slot = Some(draw::ModeFade {
                    mode: cur,
                    prev: if fade { prev } else { cur },
                    since: now,
                });
                if fade {
                    self.motion.kick(anim::HINT_FADE.max(anim::CHIP_EASE), now);
                }
            }
        }
        if on
            && anim::fades()
            && let Some(st) = &self.status
        {
            let age = now.saturating_duration_since(st.at);
            if age < anim::MESSAGE_EDGE {
                self.motion.kick(anim::MESSAGE_EDGE - age, now);
            } else if age + anim::MESSAGE_EDGE >= STATUS_FOR {
                self.motion.kick(STATUS_FOR.saturating_sub(age), now);
            }
        }
    }

    /// Moves what moves, before a frame is drawn: the selection band toward
    /// the current row, the switch of workspaces, the agent's spinner.
    fn tick_motion(&mut self, now: Instant) {
        let on = anim::enabled();
        let here = self.active.as_ref().and_then(|s| {
            let id = &s.entry.id;
            let idx = self.rows.iter().position(|r| &r.entry.id == id)?;
            Some((id.clone(), idx))
        });
        match (here, &mut self.bar) {
            (None, bar) => *bar = None,
            (Some((id, idx)), bar @ None) => {
                *bar = Some(Bar {
                    id,
                    idx,
                    tween: anim::Tween::at(idx as f32, now),
                });
            }
            (Some((id, idx)), Some(bar)) => {
                if bar.idx != idx || bar.id != id {
                    let dir = if idx >= bar.idx { 1 } else { -1 };
                    if on {
                        bar.tween.retarget(idx as f32, anim::SWITCH, now);
                        self.motion.kick(anim::SWITCH, now);
                        if bar.id != id && anim::fades() {
                            let enter = anim::Tween::new(0.0, 1.0, anim::SWITCH, now);
                            self.switch = Some((enter, dir));
                        }
                    } else {
                        bar.tween = anim::Tween::at(idx as f32, now);
                    }
                    bar.id = id;
                    bar.idx = idx;
                }
            }
        }
        self.tick_status(now, on);
        self.tick_overlays(now, on);
        self.tick_focus(now, on);
        self.tick_scroll(now);
        self.tick_welcome(now, on);
        self.band = self
            .bar
            .as_ref()
            .map(|b| b.tween.value(now).round().max(0.0) as usize);
        if self.switch.is_some_and(|(t, _)| t.done(now)) {
            self.switch = None;
        }
        let shown = usize::from(self.size.1).saturating_sub(5);
        let first = self.sidebar_first(shown);
        let waiting = self.busy.is_some()
            || matches!(
                self.modal,
                Some(Modal::Rm {
                    pr: modal::PrLine::Checking,
                    ..
                }) | Some(Modal::Rm { pending: true, .. })
            );
        let spinning = on
            && (waiting
                || self
                    .rows
                    .iter()
                    .skip(first)
                    .take(shown)
                    .any(|r| self.marks(&r.entry.id).contains(&'✻')));
        self.spinner = spinning.then(|| anim::spinner(now.duration_since(self.born)));
        self.spin_due = spinning.then(|| now + anim::SPIN_EVERY);
    }

    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        rx: Receiver<Msg>,
    ) -> Result<()> {
        self.draw(terminal)?;
        while !self.quit {
            let now = Instant::now();
            let first = match self.next_wake(now) {
                None => rx.recv().context("event channels closed")?,
                Some((at, wake)) => match rx.recv_timeout(at.saturating_duration_since(now)) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        match wake {
                            Wake::Edge => {
                                self.edge_step();
                                self.edge_due = Some(Instant::now() + EDGE_EVERY);
                            }
                            Wake::Which => self.which = true,
                            Wake::Frame => {}
                        }
                        self.draw(terminal)?;
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        anyhow::bail!("event channels closed")
                    }
                },
            };
            self.handle(first);
            drain(&rx, FRAME, |m| {
                self.handle(m);
                !self.quit
            });
            if self
                .sel
                .as_ref()
                .is_some_and(|s| self.focus != Some(s.pane))
            {
                self.clear_sel();
            }
            if self.welcome.is_some() && self.active.is_some() {
                self.dismiss_welcome();
            }
            self.draw(terminal)?;
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
        if let Err(e) = trace::span("send", || self.tx.send(&msg)) {
            self.fail(format!("daemon: {e}"));
        }
    }

    /// Rebuilds the sidebar from the registry: one space per project.
    fn reload(&mut self) -> Result<()> {
        // Stamped before reading: a write that lands meanwhile shows on the
        // next look (RISK-48), and this one is not taken for someone else's.
        self.regwatch.sync();
        self.pending_rows = None;
        let state = registry::state_dir()?;
        let folders = trace::span("reload_folders", || crate::folders::load(&state))?;
        let reg_path = registry::default_path()?;
        let mut reg = trace::span("reload_registry", || Registry::load(&reg_path))?;
        if trace::span("reload_fill_roots", || actions::fill_roots(&mut reg)) {
            reg.save(&reg_path)?;
        }
        let rows = crate::sidebar::rows(&folders, reg, &crate::session::current());
        self.apply_rows(rows);
        Ok(())
    }

    /// The workspace on screen keeps its latest name and branch.
    fn apply_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        if let Some(s) = &mut self.active
            && let Some(row) = self.rows.iter().find(|r| r.entry.id == s.entry.id)
        {
            s.entry = row.entry.clone();
        }
        self.warm_streams();
    }

    fn overlay_open(&self) -> bool {
        self.modal.is_some() || self.finder.is_some() || self.settings.is_some()
    }

    /// Rows read again off the UI thread, with no write and no busy mark
    /// (REQ-155, REQ-156).
    fn reload_rows_in_background(&self) {
        let tx = self.events.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Rows(load_rows()));
        });
    }

    /// Rows another process wrote: shown now, or once the overlay closes.
    fn on_rows(&mut self, rows: Result<Vec<Row>>) {
        // Unreadable (half-written by a tool that doesn't rename): the rows
        // stay and the next change tries again, silently (REQ-157).
        if let Ok(rows) = rows {
            self.pending_rows = Some(rows);
            self.apply_pending_rows();
        }
    }

    fn apply_pending_rows(&mut self) {
        if !self.overlay_open()
            && let Some(rows) = self.pending_rows.take()
        {
            self.apply_rows(rows);
        }
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
        let guess =
            |p: &PaneInfo| p.agent.is_none() && program::is_agent_pane(&p.role, p.fg.as_deref());
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
        if !trace::on() {
            return self.handle_msg(m);
        }
        let kind = match &m {
            Msg::Snapshot { .. } | Msg::Daemon(DaemonMsg::Snapshot { .. }) => "handle_snapshot",
            Msg::History { .. } => "handle_history",
            Msg::Daemon(DaemonMsg::Tree { .. }) => "handle_tree",
            Msg::Daemon(DaemonMsg::Output { .. }) => "handle_output",
            Msg::Daemon(_) => "handle_daemon",
            Msg::Term(_) => "handle_term",
            _ => "handle_other",
        };
        trace::span(kind, || self.handle_msg(m));
    }

    fn handle_msg(&mut self, m: Msg) {
        match m {
            Msg::Daemon(d) => self.on_daemon(d),
            Msg::Snapshot {
                pane,
                role,
                parser,
                history,
            } => self.install_snapshot(pane, role, *parser, history),
            Msg::History { pane, seq, parser } => self.on_history(pane, seq, *parser),
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
                self.release_asks();
                self.apply_pending_rows();
                if self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.at.elapsed() > STATUS_FOR)
                {
                    self.status = None;
                }
                if self
                    .copied
                    .is_some_and(|(_, _, at)| at.elapsed() > BANNER_FOR)
                {
                    self.copied = None;
                }
                // Every 2 s the panes' state (✻ ? ⚡), every minute the PRs.
                if self.ticks.is_multiple_of(4) {
                    self.send(ClientMsg::List);
                    if self.regwatch.changed() {
                        self.reload_rows_in_background();
                    }
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
            Msg::Stream(s) => {
                self.streams.insert(s.entry.id.clone(), *s);
            }
            Msg::Rows(rows) => self.on_rows(rows),
        }
    }

    /// A pane's screen as the daemon holds it, with its parser built.
    fn install_snapshot(
        &mut self,
        pane: PaneId,
        role: String,
        parser: vt100::Parser<keys::Flags>,
        history: Option<u64>,
    ) {
        self.clear_sel_of(pane);
        self.panes.insert(
            pane,
            PaneView {
                role,
                parser,
                history: history.map(|seq| Pending {
                    seq,
                    ops: Vec::new(),
                    bytes: 0,
                }),
                exited: None,
                title: None,
            },
        );
        trace::span("fit", || self.fit());
        trace::mark("snap_installed", 0);
    }

    /// The full replay of a pane that was drawn from its `Screen`: what
    /// happened to the screen since is done again on it, and it takes the
    /// screen's place with its scrollback.
    fn on_history(&mut self, pane: PaneId, seq: u64, mut parser: vt100::Parser<keys::Flags>) {
        let Some(view) = self.panes.get_mut(&pane) else {
            return;
        };
        let Some(pending) = view.history.take_if(|h| h.seq == seq) else {
            return;
        };
        trace::span("history_swap", || {
            for op in pending.ops {
                match op {
                    Op::Out(bytes) => parser.process(&bytes),
                    Op::Size(rows, cols) => parser.screen_mut().set_size(rows, cols),
                }
            }
            let size = view.parser.screen().size();
            if parser.screen().size() != size {
                parser.screen_mut().set_size(size.0, size.1);
            }
            view.parser = parser;
        });
        self.clear_sel_of(pane);
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
                let parser = replay("snap_parse", rows, cols, &bytes);
                self.install_snapshot(pane, role, parser, None);
            }
            // The reader thread turns these into `Msg::Snapshot` and `Msg::History`.
            DaemonMsg::Screen { .. } | DaemonMsg::History { .. } | DaemonMsg::Resized { .. } => {}
            DaemonMsg::Output { pane, bytes } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    if let Some(h) = &mut v.history {
                        h.out(&bytes);
                        if h.bytes > PENDING_MAX {
                            v.history = None;
                        }
                    }
                    v.parser.process(&bytes);
                    if let Some(sel) = &mut self.sel
                        && sel.pane == pane
                        && !sel.follow(&mut v.parser)
                        && !self.selecting
                    {
                        self.sel = None;
                    }
                }
            }
            DaemonMsg::Tree { stream, tree } => {
                if self.current().is_some_and(|e| e.id == stream) {
                    trace::span("set_tree", || self.set_tree(tree));
                }
                self.send(ClientMsg::List);
            }
            DaemonMsg::Title { pane, title } => {
                if let Some(v) = self.panes.get_mut(&pane) {
                    v.title = Some(title);
                }
            }
            DaemonMsg::Hello { .. } | DaemonMsg::Spawned { .. } | DaemonMsg::Opened { .. } => {}
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
            // For a phone: the TUI never sends `TextView`.
            DaemonMsg::Text { .. }
            | DaemonMsg::TextHistory { .. }
            | DaemonMsg::Chat { .. }
            | DaemonMsg::Shell { .. }
            | DaemonMsg::ShellOut { .. } => {}
            DaemonMsg::Error { msg } => self.fail(msg),
            d @ (DaemonMsg::MobileState(_)
            | DaemonMsg::MobilePair { .. }
            | DaemonMsg::MobilePairDone { .. }) => self.on_mobile(d),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if self.welcome.is_some() {
            self.dismiss_welcome();
        }
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
                finder::Outcome::Reload(requested) => self.load_reviews(requested),
                finder::Outcome::Browse(url) => self.browse(&url),
            }
            return;
        }
        if matches!(self.modal, Some(Modal::Mobile(_) | Modal::Pair(..))) {
            self.mobile_key(k);
            return;
        }
        if let Some(m) = &mut self.modal {
            match m.key(k) {
                Outcome::Stay => {}
                Outcome::Cancel => self.modal = None,
                Outcome::Submit => self.submit_modal(),
                Outcome::ToDone => {
                    if let Some(Modal::Rm {
                        entry,
                        project: Some(project),
                        pr: modal::PrLine::Merged { done, .. },
                        ..
                    }) = self.modal.take()
                    {
                        let (plan, pr) = *done;
                        self.modal = Some(Modal::Done {
                            entry,
                            project,
                            plan,
                            pr,
                        });
                    }
                }
                Outcome::Back => {
                    if let Some(Modal::Setup(s)) = self.modal.take() {
                        let s = *s;
                        self.modal = Some(Modal::New {
                            plan: modal::new_plan(&s.project, &s.name),
                            text: s.name,
                            picked: false,
                            from: s.from,
                            project: s.project,
                        });
                    }
                }
            }
            return;
        }
        if self.walking {
            if self.walk_key(k.code) {
                return;
            }
            self.walking = false;
            if matches!(k.code, KeyCode::Enter | KeyCode::Esc) {
                return;
            }
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
            diffview::Action::Load(r) => self.load_part(f, r),
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
            code if self.walk_key(code) => self.walking = true,
            KeyCode::Char('h') => self.focus_towards(-1, 0),
            KeyCode::Char('l') => self.focus_towards(1, 0),
            KeyCode::Char('k') => self.focus_towards(0, -1),
            KeyCode::Char('j') => self.focus_towards(0, 1),
            KeyCode::Char('H') => self.move_pane(-1, 0),
            KeyCode::Char('L') => self.move_pane(1, 0),
            KeyCode::Char('K') => self.move_pane(0, -1),
            KeyCode::Char('J') => self.move_pane(0, 1),
            KeyCode::Char('q') => self.quit = true,
            code => match self.action(code) {
                Some("switch") => self.ask_switch(),
                Some("previous") => self.go_back(),
                Some("open") => self.ask_folder(),
                Some("file") => self.ask_file(),
                Some("sessions") => self.ask_session(),
                Some("mobile") => self.open_mobile(),
                Some("settings") => self.ask_settings(),
                Some("new") => self.new_worktree(),
                Some("rename") => self.ask_rename(),
                Some("sync") => self.run_sync(),
                Some("diff") => self.open_diff(),
                Some("review") => self.ask_review(),
                Some("remove") => self.ask_remove(),
                Some("pane") => self.new_pane(),
                Some("close") => self.close_pane(),
                Some("name") => self.ask_name(),
                Some("layout") => self.ask_save_layout(),
                Some("full") => {
                    self.full = !self.full;
                    self.fit();
                }
                Some(_) => {}
                None => {
                    let key = match code {
                        KeyCode::Char(' ') => "␣".to_string(),
                        KeyCode::Char(c) => c.to_string(),
                        other => format!("{other:?}").to_lowercase(),
                    };
                    let l = self.leader.label();
                    self.fail(format!("{l} {key} does nothing · {l} ? shows the keys"));
                }
            },
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

    /// An arrow while walking the sidebar: `↑↓` the previous or next open
    /// workspace, `←→` the previous or next project. False for any other key.
    fn walk_key(&mut self, code: KeyCode) -> bool {
        let target = match code {
            KeyCode::Up => self.walk(-1, false),
            KeyCode::Down => self.walk(1, false),
            KeyCode::Left => self.walk(-1, true),
            KeyCode::Right => self.walk(1, true),
            _ => return false,
        };
        match target {
            Some(e) => self.open_entry(e, false),
            None => self.fail("no other open workspace".into()),
        }
        true
    }

    /// The open workspace `dir` steps from the current one in the sidebar,
    /// wrapping around; by `project`, the first open row of each project.
    /// Closed rows are skipped: opening them would start their panes.
    fn walk(&self, dir: isize, project: bool) -> Option<Entry> {
        // Each project's rows: the unindented one and the worktrees under it.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            match groups.last_mut() {
                Some(g) if r.child => g.push(i),
                _ => groups.push(vec![i]),
            }
        }
        let open = |i: &usize| self.is_open(&self.rows[*i].entry.id);
        let stops: Vec<usize> = if project {
            groups
                .iter()
                .filter_map(|g| g.iter().copied().find(|i| open(i)))
                .collect()
        } else {
            (0..self.rows.len()).filter(open).collect()
        };
        let here = self
            .rows
            .iter()
            .position(|r| self.current().is_some_and(|c| c.id == r.entry.id));
        // Where the current row sits among the stops: its own group's stop
        // when moving by project.
        let at = here.and_then(|h| {
            if project {
                let g = groups.iter().find(|g| g.contains(&h))?;
                stops.iter().position(|s| g.contains(s))
            } else {
                stops.iter().position(|s| *s == h)
            }
        });
        let n = stops.len() as isize;
        let next = match at {
            Some(a) => (a as isize + dir).rem_euclid(n.max(1)) as usize,
            None if dir > 0 => 0,
            None => stops.len().checked_sub(1)?,
        };
        let row = *stops.get(next)?;
        (Some(row) != here).then(|| self.rows[row].entry.clone())
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

    /// `setup`: run the config's setup before the panes' programs (new
    /// streams, REQ-119).
    fn open_entry(&mut self, entry: Entry, setup: bool) {
        if self.current().is_some_and(|c| c.id == entry.id) {
            return;
        }
        trace::mark("switch_start", 0);
        self.recent.retain(|id| *id != entry.id);
        self.recent.insert(0, entry.id.clone());
        self.recent.truncate(50);
        trace::span("save_recent", || save_recent(&self.recent));
        if crate::folders::is_folder(&entry) && !entry.path.is_empty() {
            let dir = entry.path.clone();
            if let Err(e) = trace::span("folders_edit", || {
                crate::folders::edit(|all| {
                    all.add(&dir);
                })
            }) {
                self.fail(format!("{e:#}"));
            }
        }
        let stream = match trace::span("cached_stream", || self.cached_stream(&entry, setup)) {
            Some(s) => Ok(s),
            None => trace::span("resolve", || Stream::resolve(&entry)).inspect(|s| {
                self.streams.insert(entry.id.clone(), s.clone());
            }),
        };
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                self.fail(format!("{}: {e:#}", entry.name));
                return;
            }
        };
        self.send(ClientMsg::Detach);
        trace::span("forget", || self.forget_workspace());
        let already = self.is_open(&entry.id);
        self.active = Some(stream);

        if already {
            self.send(ClientMsg::Attach {
                stream: entry.id.clone(),
            });
            trace::mark("attach_sent", 0);
        } else if let Err(e) = trace::span("start_panes", || self.start_panes(setup)) {
            self.fail(format!("{}: {e:#}", entry.name));
        }
    }

    /// The stream resolved earlier off the UI thread, while no config it read
    /// has changed. A new worktree (`setup`) resolves fresh: its config was
    /// just written.
    fn cached_stream(&self, entry: &Entry, setup: bool) -> Option<Stream> {
        if setup {
            return None;
        }
        let mut s = self
            .streams
            .get(&entry.id)
            .filter(|s| s.entry.path == entry.path && s.entry.project == entry.project)?
            .clone();
        if !s.is_fresh() {
            return None;
        }
        // As a resolve would: the agent's conversation, or a new one for a
        // workspace that starts its panes now.
        if !entry.agent.is_empty() {
            s.session = entry.agent.clone();
        } else if !self.is_open(&entry.id) {
            s.session = registry::new_id().ok()?;
        }
        s.entry = entry.clone();
        Some(s)
    }

    /// Resolves, on a worker thread, the streams the sidebar lists and nobody
    /// asked for yet: `Stream::resolve` runs `git` (about 40 ms).
    fn warm_streams(&mut self) {
        let todo: Vec<Entry> = self
            .rows
            .iter()
            .filter(|r| !self.warming.contains(&r.entry.id))
            .map(|r| r.entry.clone())
            .collect();
        if todo.is_empty() {
            return;
        }
        self.warming.extend(todo.iter().map(|e| e.id.clone()));
        let tx = self.events.clone();
        thread::spawn(move || {
            for e in todo {
                if let Ok(s) = Stream::resolve(&e) {
                    let _ = tx.send(Msg::Stream(Box::new(s)));
                }
            }
        });
    }

    /// Asks the daemon to start the workspace's panes in its layout's shape.
    fn start_panes(&mut self, setup: bool) -> Result<()> {
        let stream = self.active.clone().context("no workspace")?;
        let specs = stream.open_specs(setup)?;
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
        self.send_pending_prompt(&stream.entry.id);
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
    /// process (REQ-53). A known agent in the foreground gives its own name
    /// instead of the title it set, and the flag to draw it as one (REQ-201).
    pub fn pane_title(&self, id: PaneId) -> (String, Option<String>, bool) {
        let leaf = self.tree.as_ref().and_then(|t| t.find(id));
        if let Some(name) = leaf.and_then(|l| l.name.clone()) {
            return (name, None, false);
        }
        let role = leaf.map(|l| l.role.clone()).unwrap_or_default();
        let fg = || {
            self.daemon_panes
                .iter()
                .find(|p| p.pane == id)
                .and_then(|p| p.fg.clone())
        };
        if let Some(agent) = fg().filter(|n| program::is_agent_program(n)) {
            return (role, Some(agent), true);
        }
        let title = self
            .panes
            .get(&id)
            .and_then(|v| v.title.clone())
            .filter(|t| !t.trim().is_empty());
        let runs = title.or_else(fg).filter(|r| *r != role);
        (role, runs, false)
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
                if let Some(h) = &mut view.history {
                    h.ops.push(Op::Size(rows, cols));
                }
                if self.sel.as_ref().is_some_and(|s| s.pane == id) {
                    self.sel = None;
                    self.selecting = false;
                    self.edge = None;
                }
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

    /// The first sidebar row shown when `shown` rows fit: the current one
    /// stays on screen.
    pub fn sidebar_first(&self, shown: usize) -> usize {
        let current = self
            .rows
            .iter()
            .position(|r| self.current().is_some_and(|c| c.id == r.entry.id))
            .unwrap_or(0);
        current.saturating_sub(shown.saturating_sub(1))
    }

    /// The sidebar row under `y`, as the sidebar draws them (REQ-50).
    fn sidebar_row_at(&self, y: u16) -> Option<usize> {
        let rows = self.size.1.saturating_sub(1).saturating_sub(4) as usize;
        let y = usize::from(y.checked_sub(2)?);
        if y >= rows {
            return None;
        }
        let i = self.sidebar_first(rows) + y;
        (i < self.rows.len()).then_some(i)
    }

    /// The mouse: click a workspace or a pane, roll the wheel over a pane or
    /// a picker, drag to select text, and pass everything to a program that
    /// asked for the mouse (a Shift + drag still selects, REQ-125).
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
        if click {
            self.clear_sel();
        }
        if self.sel_drag(&m) {
            return;
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
            let mut asked = None;
            match v {
                View::Diff(d) if wheel != 0 => d.scroll_by(wheel),
                View::Diff(d) if click => {
                    if let diffview::Action::Load(r) = d.click(x, y) {
                        asked = Some(r);
                    }
                }
                View::Md(md) if wheel != 0 => md.scroll_by(wheel),
                _ => {}
            }
            if let Some(r) = asked {
                self.load_part(id, r);
            }
            return;
        }
        let inside = x > r.x && x + 1 < r.x + r.w && y > r.y && y + 1 < r.y + r.h;
        let takes_mouse = |v: &PaneView| {
            v.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
                && !m.modifiers.contains(KeyModifiers::SHIFT)
        };
        if click && inside && self.panes.get(&id).is_some_and(|v| !takes_mouse(v)) {
            self.select_down(id, r, x, y);
            return;
        }
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
        } else if anim::enabled() {
            self.glide_scroll(id, -wheel, Instant::now());
        } else {
            let back = screen.scrollback().saturating_add_signed(-wheel);
            view.parser.screen_mut().set_scrollback(back);
        }
    }

    /// The selection, when it is in this pane.
    fn selection(&self, pane: PaneId) -> Option<&Sel> {
        self.sel.as_ref().filter(|s| s.pane == pane)
    }

    /// Forgets the selection (REQ-132).
    fn clear_sel(&mut self) {
        self.sel = None;
        self.selecting = false;
        self.edge = None;
    }

    fn clear_sel_of(&mut self, pane: PaneId) {
        if self.sel.as_ref().is_some_and(|s| s.pane == pane) {
            self.clear_sel();
        }
    }

    /// A left press in a pane that is not taking the mouse starts a
    /// selection; a double or triple click selects the word or the line and
    /// copies it (REQ-124, REQ-126).
    fn select_down(&mut self, id: PaneId, r: Rect, x: u16, y: u16) {
        let (col, row) = (x - r.x - 1, y - r.y - 1);
        let clicks = match self.clicks {
            Some((at, cx, cy, n)) if at.elapsed() < CLICK_WITHIN && (cx, cy) == (x, y) => {
                n.saturating_add(1)
            }
            _ => 1,
        };
        self.clicks = Some((Instant::now(), x, y, clicks));
        let Some(view) = self.panes.get_mut(&id) else {
            return;
        };
        let line = select::line_at(&mut view.parser, row);
        let at = Pos { line, col };
        if clicks == 1 {
            self.sel = Some(Sel::new(id, &mut view.parser, at, at));
            self.selecting = true;
            return;
        }
        let Some((cells, _)) = select::row_cells(&mut view.parser, line) else {
            return;
        };
        let (from, to) = if clicks == 2 {
            select::word_at(&cells, col)
        } else {
            select::line_span(&cells)
        };
        let (a, b) = (Pos { line, col: from }, Pos { line, col: to });
        self.sel = Some(Sel::new(id, &mut view.parser, a, b));
        self.copy_selection();
    }

    /// The drag and the release of a selection, wherever the pointer goes
    /// (REQ-124); above or below the pane it keeps scrolling (REQ-127).
    fn sel_drag(&mut self, m: &MouseEvent) -> bool {
        if !self.selecting {
            return false;
        }
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let pane = self.sel.as_ref().map(|s| s.pane);
                let rect = self
                    .pane_rects()
                    .into_iter()
                    .find(|(id, _)| Some(*id) == pane);
                let Some((_, r)) = rect else {
                    self.clear_sel();
                    return false;
                };
                let (w, h) = (r.w.saturating_sub(2), r.h.saturating_sub(2));
                if w == 0 || h == 0 {
                    return true;
                }
                self.edge = if m.row <= r.y {
                    Some(-1)
                } else if m.row + 1 >= r.y + r.h {
                    Some(1)
                } else {
                    None
                };
                let col = m.column.saturating_sub(r.x + 1).min(w - 1);
                let row = m.row.saturating_sub(r.y + 1).min(h - 1);
                self.extend(col, row);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.selecting = false;
                self.edge = None;
                if self.sel.as_ref().is_some_and(Sel::is_empty) {
                    self.sel = None;
                } else {
                    self.copy_selection();
                }
                true
            }
            _ => false,
        }
    }

    /// Moves the selection's far end to a visible cell.
    fn extend(&mut self, col: u16, row: u16) {
        let Some(sel) = &mut self.sel else {
            return;
        };
        let Some(view) = self.panes.get_mut(&sel.pane) else {
            return;
        };
        sel.head = Pos {
            line: select::line_at(&mut view.parser, row),
            col,
        };
    }

    /// One line of scroll while the drag is held above or below the pane.
    fn edge_step(&mut self) {
        let pane = self.sel.as_ref().map(|s| s.pane);
        let (Some(dir), Some(view)) = (self.edge, pane.and_then(|p| self.panes.get_mut(&p))) else {
            self.edge = None;
            return;
        };
        let screen = view.parser.screen_mut();
        let back = screen.scrollback().saturating_add_signed(-dir);
        screen.set_scrollback(back);
        let (rows, cols) = screen.size();
        let (row, col) = if dir < 0 {
            (0, 0)
        } else {
            (rows - 1, cols - 1)
        };
        self.extend(col, row);
    }

    /// Copies the selection through the outer terminal (REQ-128).
    fn copy_selection(&mut self) {
        let Some(sel) = &self.sel else {
            return;
        };
        let Some(view) = self.panes.get_mut(&sel.pane) else {
            return;
        };
        let text = select::text(&mut view.parser, sel);
        if text.is_empty() {
            return;
        }
        match select::copy(&text) {
            Ok(n) => self.copied = Some((sel.pane, n, Instant::now())),
            Err(e) => self.fail(format!("{e:#}")),
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
        self.clear_sel_of(id);
        // Typing goes back to the bottom of the history, as terminals do.
        if let Some(v) = self.panes.get_mut(&id) {
            v.parser.screen_mut().set_scrollback(0);
        }
        let parser = &self.panes[&id].parser;
        let screen = parser.screen();
        let kitty = parser.callbacks().0.flags(screen.alternate_screen());
        let bytes = keys::encode(k, screen.application_cursor(), kitty);
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
            self.no_workspace();
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

    fn no_workspace(&mut self) {
        let l = self.leader.label();
        self.fail(format!(
            "no workspace open: {l} {} opens a folder",
            key("open")
        ));
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
            self.show_session();
        }
        self.done(format!("session {old} is {new} now"));
    }

    /// Closes the session's workspaces in the daemon and forgets it. The
    /// worktrees stay on disk (REQ-273).
    fn remove_session(&mut self, name: &str, plan: &crate::session::RmPlan) {
        let state = match registry::state_dir() {
            Ok(s) => s,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        // The daemon first, so nothing runs in a session that is gone.
        for stream in plan.streams() {
            self.send(ClientMsg::Close {
                stream: stream.to_string(),
            });
        }
        self.send(ClientMsg::List);
        if let Err(e) = crate::session::rm(&state, name, &crate::session::current()) {
            return self.fail(format!("{e:#}"));
        }
        self.done(format!(
            "removed session {name}; its worktrees stay on disk"
        ));
        self.ask_session();
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
        self.show_session();
        self.done(format!("session {name}"));
    }

    /// The session just set, from nothing on screen: the next List opens
    /// its last workspace.
    fn show_session(&mut self) {
        self.send(ClientMsg::Detach);
        self.forget_workspace();
        self.recent = load_recent();
        self.fresh = true;
        if let Err(e) = self.reload() {
            self.fail(format!("{e:#}"));
        }
        self.send(ClientMsg::List);
    }

    /// Nothing on screen: no workspace, panes, viewers or focus.
    fn forget_workspace(&mut self) {
        self.active = None;
        self.panes.clear();
        self.views.clear();
        self.pending_view = None;
        self.tree = None;
        self.focus = None;
        self.full = false;
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
            return self.no_workspace();
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

    /// `^␣ R`: the open pull requests of the current project (REQ-160). The
    /// picker opens at once and gh runs off this thread.
    fn ask_review(&mut self) {
        if self.current_repo_stream("review").is_none() {
            return;
        }
        self.finder = Some(finder::Finder::reviews(false));
        self.load_reviews(false);
    }

    /// Asks gh for the list the picker waits for.
    fn load_reviews(&mut self, requested: bool) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.background("loading pull requests".into(), move || {
            let gh = crate::connectors::github::Client::default();
            let prs = gh.open_prs(std::path::Path::new(&entry.path), requested)?;
            Ok(Job::Prs { prs, requested })
        });
    }

    /// `↵` on a PR: its head, the worktree and the context, off this thread
    /// (REQ-165, REQ-166).
    fn start_review(&mut self, pr: crate::connectors::github::PrSummary) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.background(format!("fetching #{}", pr.number), move || {
            let project = Project::open(std::path::Path::new(&entry.path))?;
            let gh = crate::connectors::github::Client::default();
            let r = crate::review::review_stream(
                &project,
                &gh,
                &pr,
                &registry::default_path()?,
                &registry::state_dir()?,
            )?;
            Ok(Job::Reviewed(Box::new(r)))
        });
    }

    /// The agent of a review is briefed as soon as its panes are asked for;
    /// the daemon waits for it to be quiet (REQ-171).
    fn send_pending_prompt(&mut self, id: &str) {
        if let Some((stream, text)) = self.pending_prompt.take_if(|(s, _)| s == id) {
            self.send(ClientMsg::Prompt {
                stream,
                text,
                pane: None,
            });
        }
    }

    /// Opens a url in the browser (REQ-180).
    fn browse(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Reaped off this thread, so the opener leaves no zombie.
                thread::spawn(move || child.wait());
                self.say(format!("opened {url}"));
            }
            Err(e) => self.fail(format!("{opener}: {e}")),
        }
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
            finder::Pick::Review(pr) => return self.start_review(*pr),
            finder::Pick::Session(s) => return self.switch_session(&s),
            finder::Pick::NewSession(s) => return self.new_session(&s),
            finder::Pick::DeleteSession(s) => {
                if s == crate::session::current() {
                    self.fail(format!("cannot delete {s}: it is the session you are in"));
                    return;
                }
                self.background(format!("checking session {s}"), move || {
                    let state = registry::state_dir()?;
                    let plan = crate::session::rm_plan(&state, &s)?;
                    Ok(Job::SessionRm { name: s, plan })
                });
                return;
            }
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

    /// The current workspace's panes as a layout: each pane's role back to
    /// the run it came from in the opening tree (RAT-24), a pane added later
    /// as a shell, viewers left out.
    fn live_layout(&self) -> Option<Node> {
        let s = self.active.as_ref()?;
        let runs: BTreeMap<String, String> = s
            .tree
            .leaves()
            .into_iter()
            .map(|l| (l.role, l.run))
            .collect();
        let run = |l: &PaneLeaf| -> String {
            if l.role.starts_with("view:") {
                return String::new();
            }
            let base = match l.role.rsplit_once('-') {
                Some((b, n)) if n.chars().all(|c| c.is_ascii_digit()) => b,
                _ => &l.role,
            };
            runs.get(&l.role)
                .or_else(|| runs.get(base))
                .cloned()
                .unwrap_or_else(|| match base {
                    "editor" | "agent" => base.to_string(),
                    b if b.starts_with("dev:") => b.to_string(),
                    _ => "shell".into(),
                })
        };
        prune(self.tree.as_ref()?.to_node(&run)).map(|n| even(&n))
    }

    /// REQ-121: a worktree whose panes aren't its project's layout.
    pub fn layout_changed(&self) -> bool {
        let Some(s) = &self.active else {
            return false;
        };
        // A review is always the diff beside the agent: nothing to save.
        !crate::folders::is_folder(&s.entry)
            && s.entry.review_base.is_empty()
            && self
                .live_layout()
                .is_some_and(|live| live != even(&s.cfg.layout.tree()))
    }

    /// `p`: saves the panes on screen as the project's layout (REQ-120).
    fn ask_save_layout(&mut self) {
        let Some(s) = self.active.clone() else {
            return;
        };
        let Some(new) = self.live_layout() else {
            return;
        };
        let path = match &s.cfg.source {
            Some(p) => p.clone(),
            None => match crate::connectors::git::Repo::open(std::path::Path::new(&s.entry.path)) {
                Ok(repo) => repo.root.join(".jw.toml"),
                Err(_) => {
                    self.fail(format!(
                        "{} is not a git project: no layout to save",
                        s.entry.name
                    ));
                    return;
                }
            },
        };
        let old = s.cfg.source.as_ref().map(|_| even(&s.cfg.layout.tree()));
        if old.as_ref() == Some(&new) {
            self.say("the panes already match the project's layout".into());
            return;
        }
        self.modal = Some(Modal::SaveLayout { path, old, new });
    }

    /// `w`: asks the new worktree's name, offering the lowest free `ws-N`
    /// (REQ-110); it starts from the current workspace's branch.
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
        match Project::open(std::path::Path::new(&from.path)) {
            Ok(project) => {
                self.modal = Some(Modal::New {
                    plan: modal::new_plan(&project, &name),
                    text: name,
                    picked: true,
                    from,
                    project,
                });
            }
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    /// Creates the worktree the `w` modal named, set up and shown at once.
    fn create_worktree(&mut self, from: Entry, project: Project, name: String) {
        self.background(format!("creating {name}"), move || {
            created(&project, from, name, None)
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
                "{} is a project folder: jw never deletes it. {l} {} on its last pane closes it",
                entry.name,
                key("close")
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
        trace::mark("rm_ask", 0);
        self.busy = Some(format!("checking {}", entry.name));
        self.modal = Some(Modal::Rm {
            entry: entry.clone(),
            project: None,
            plan: actions::RmPlan::default(),
            typed: String::new(),
            error: None,
            pr: modal::PrLine::Checking,
            pending: true,
        });
        trace::mark("rm_modal", 0);
        let tx = self.events.clone();
        thread::spawn(move || {
            let send = |job| {
                let _ = tx.send(Msg::Job(Box::new(job)));
            };
            // git on the worktree needs no project: it runs while the project
            // opens, and the modal opens as soon as the project is there
            // (REQ-257, REQ-268).
            let asked = entry.clone();
            let state =
                thread::spawn(move || trace::span("rm_local", || actions::worktree_state(&asked)));
            let opened = trace::span("rm_project", || {
                std::iter::once(&entry.path)
                    .chain(&siblings)
                    .find_map(|p| Project::open(std::path::Path::new(p)).ok())
                    .context("no checkout of this project left to find its repository")
            });
            let project = match opened {
                Ok(p) => p,
                Err(e) => {
                    return send(Job::RmWork {
                        id: entry.id,
                        plan: Err(format!("{e:#}")),
                    });
                }
            };
            send(Job::RmProject {
                id: entry.id.clone(),
                project: project.clone(),
            });
            let exists = project.repo.branch_exists(&entry.branch);
            let plan = state
                .join()
                .unwrap_or_else(|p| std::panic::resume_unwind(p))
                .map(|state| actions::rm_plan_from(&entry, state, exists))
                .map_err(|e| format!("{e:#}"));
            send(Job::RmWork { id: entry.id, plan });
        });
    }

    /// What gh says about the PR of the branch the remove modal is open on.
    /// It runs after the modal is up and only fills its PR line (REQ-259).
    fn ask_pr(&mut self, entry: Entry, project: Project) {
        self.background(format!("{PR_BUSY}{}", entry.name), move || {
            use crate::connectors::PullRequests;
            let prs = prcache::client();
            let found = trace::span("rm_gh", || {
                prs.for_branch(&project.repo.root, &entry.branch)
            });
            let line = match found {
                Err(_) => modal::PrLine::Unknown,
                Ok(None) => modal::PrLine::NoPr,
                Ok(Some(pr)) if pr.state == "MERGED" => {
                    match trace::span("rm_done_plan", || {
                        actions::done_plan(&project, &entry, &prs)
                    }) {
                        Ok((plan, pr)) => modal::PrLine::Merged {
                            number: pr.number,
                            done: Box::new((plan, pr)),
                        },
                        Err(_) => modal::PrLine::Info(format!("#{} merged", pr.number)),
                    }
                }
                Ok(Some(pr)) => modal::PrLine::Info(format!("#{} {}", pr.number, pr.status())),
            };
            Ok(Job::RmPr { id: entry.id, line })
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
        // REQ-123: a project root diffs too, against its default branch. A
        // repository with no origin has no branch in the sidebar, so look
        // for `.git` itself.
        let Some(entry) = self.current().cloned() else {
            self.no_workspace();
            return;
        };
        if !std::path::Path::new(&entry.path).join(".git").exists() {
            self.fail(format!(
                "{} is not a git repository: diff needs one",
                entry.name
            ));
            return;
        }
        self.load_diff(None);
    }

    fn load_diff(&mut self, into: Option<PaneId>) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.background(format!("diffing {}", entry.name), move || {
            let stream = Stream::resolve(&entry)?;
            let general = crate::diff::load(
                std::path::Path::new(&entry.path),
                &crate::diff::Scope::Branch {
                    base: stream.base.clone(),
                },
            )
            .map_err(|e| format!("{e:#}"));
            Ok(Job::Diff {
                name: entry.name,
                base: stream.base,
                dir: entry.path,
                general,
                into,
            })
        });
    }

    /// Runs the git work a diff pane asked for, off the UI thread (REQ-235).
    fn load_part(&mut self, pane: PaneId, req: diffview::Request) {
        let Some(View::Diff(d)) = self.views.get(&pane) else {
            return;
        };
        let (dir, base) = (d.dir.clone(), d.base.clone());
        let diffview::Request { slot, seq } = req;
        self.background("diffing".into(), move || {
            use crate::diff::{Scope, commits, load};
            let path = std::path::Path::new(&dir);
            let result = match &slot {
                diffview::Slot::General => {
                    load(path, &Scope::Branch { base }).map(diffview::Part::Files)
                }
                diffview::Slot::Uncommitted => {
                    load(path, &Scope::Uncommitted).map(diffview::Part::Files)
                }
                diffview::Slot::Commits => commits(path, &base, 200).map(diffview::Part::List),
                diffview::Slot::Commit(sha) => {
                    load(path, &Scope::Commit(sha.clone())).map(diffview::Part::Files)
                }
            }
            .map_err(|e| format!("{e:#}"));
            Ok(Job::DiffPart {
                pane,
                slot,
                seq,
                result,
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
            // Their keys never submit: `mobile_key` handles them.
            Modal::Mobile(_) | Modal::Pair(..) => {}
            Modal::Close { entry, .. } => self.close(&entry),
            Modal::ClosePane { pane, .. } => self.send(ClientMsg::Kill { pane }),
            Modal::Resend { stream, prompt, .. } => {
                self.send(ClientMsg::Prompt {
                    stream,
                    text: prompt,
                    pane: None,
                });
            }
            Modal::SaveLayout { path, new, .. } => {
                match crate::core::config::save_layout(&path, &new) {
                    Ok(()) => {
                        if let Some(s) = &mut self.active {
                            s.cfg.layout.set_tree(&new);
                            s.cfg.source.get_or_insert(path.clone());
                        }
                        self.done(format!("saved the layout to {}", finder::tilde(&path)));
                    }
                    Err(e) => self.fail(format!("{e:#}")),
                }
            }
            Modal::New {
                from,
                project,
                text,
                ..
            } if project.cfg.source.is_none() => {
                // REQ-115: a project with no config is set up first.
                self.background(format!("looking at {}", project.name), move || {
                    let draft = crate::init::detect(&project.repo, &project.name, false)?;
                    Ok(Job::SetupReady(Box::new(setup::Setup::new(
                        from, project, text, &draft,
                    ))))
                });
            }
            Modal::New {
                from,
                project,
                text,
                ..
            } => self.create_worktree(from, project, text),
            Modal::Setup(s) => {
                // REQ-117: write .jw.toml, then create with it.
                let s = *s;
                let choice = s.choice();
                let (from, name) = (s.from, s.name);
                let repo = s.project.repo;
                self.background(format!("creating {name}"), move || {
                    let path = crate::init::write_project(&repo, &choice)?;
                    let project = Project::open(&repo.root)?;
                    created(&project, from, name, Some(path.display().to_string()))
                });
            }
            Modal::Rename {
                entry,
                project,
                text,
                ..
            } => {
                // The panes keep running: their cwd is the same directory
                // at its new path (REQ-111).
                self.background(format!("renaming {}", entry.name), move || {
                    let reg = registry::default_path()?;
                    let renamed = actions::rename(&project, &entry, &text, &reg)?;
                    Ok(Job::Renamed {
                        entry: renamed,
                        from: entry.path,
                    })
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
            Modal::RmSession { name, plan, .. } => self.remove_session(&name, &plan),
            Modal::Done {
                entry,
                project,
                plan,
                ..
            } => self.remove(entry, project, plan),
            Modal::Rm {
                entry,
                project,
                plan,
                typed,
                pr,
                pending,
                ..
            } => {
                let unconfirmed = plan.loses_work() && typed != entry.name;
                if pending || project.is_none() || unconfirmed {
                    self.modal = Some(Modal::Rm {
                        error: unconfirmed.then(|| format!("type {} to confirm", entry.name)),
                        entry,
                        project,
                        plan,
                        typed,
                        pr,
                        pending,
                    });
                    return;
                }
                if let Some(project) = project {
                    self.remove(entry, project, plan);
                }
            }
        }
    }

    /// Deletes a worktree that done's or rm's checks passed: its panes
    /// first, so no process holds the directory.
    fn remove(&mut self, entry: Entry, project: Project, plan: actions::RmPlan) {
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

    /// A renamed worktree's panes stay as they are: the daemon learns the
    /// new folder for its next restart, and the workspace on screen takes
    /// the new entry, so new panes open there.
    fn moved(&mut self, entry: Entry, from: String) {
        let stream = Stream::resolve(&entry);
        self.send(ClientMsg::Moved {
            stream: entry.id.clone(),
            from: from.into(),
            to: entry.path.clone().into(),
            env: stream.as_ref().map(|s| s.env()).unwrap_or_default(),
        });
        if self.current().is_some_and(|c| c.id == entry.id) {
            match stream {
                Ok(s) => self.active = Some(s),
                Err(e) => self.fail(format!("{}: {e:#}", entry.name)),
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
            self.forget_workspace();
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
        // A PR line's answer reads no registry and ends only its own wait: a
        // removal that began meanwhile keeps its indicator.
        let job = match job {
            Job::RmPr { id, line } => {
                if self.busy.as_deref().is_some_and(|b| b.starts_with(PR_BUSY)) {
                    self.busy = None;
                }
                match &mut self.modal {
                    Some(Modal::Rm { entry, pr, .. }) if entry.id == id => *pr = line,
                    _ => trace::mark("rm_pr_dropped", 0),
                }
                return;
            }
            Job::RmWork { id, plan } => {
                match (&mut self.modal, plan) {
                    (
                        Some(Modal::Rm {
                            entry,
                            plan: shown,
                            pending,
                            ..
                        }),
                        Ok(plan),
                    ) if entry.id == id => {
                        trace::mark("rm_work", 0);
                        *shown = plan;
                        *pending = false;
                    }
                    (Some(Modal::Rm { entry, .. }), Err(e)) if entry.id == id => {
                        self.modal = None;
                        self.fail(e);
                    }
                    _ => trace::mark("rm_work_dropped", 0),
                }
                return;
            }
            job => job,
        };
        self.busy = None;
        // A diff pane's answer touches no registry: no reload on its account,
        // or every `)` would read the registry on the UI thread (REQ-237).
        let job = match job {
            Job::DiffPart {
                pane,
                slot,
                seq,
                result,
            } => {
                let more = match self.views.get_mut(&pane) {
                    Some(View::Diff(d)) => d.apply(slot, seq, result),
                    _ => None,
                };
                if let Some(r) = more {
                    self.load_part(pane, r);
                }
                return;
            }
            job => job,
        };
        let reload = self.reload();
        match job {
            Job::Created {
                entry,
                setup,
                wrote,
            } => {
                let mut msg = format!("created {} on {}", entry.name, entry.branch);
                if let Some(w) = wrote {
                    msg += &format!("; wrote {w}");
                }
                self.done(msg);
                self.open_entry(entry, setup);
            }
            Job::SetupReady(s) => self.modal = Some(Modal::Setup(s)),
            Job::SessionRm { name, plan } => {
                let ids: Vec<&str> = plan.streams().collect();
                let mine: Vec<&PaneInfo> = self
                    .daemon_panes
                    .iter()
                    .filter(|p| ids.contains(&p.stream.as_str()))
                    .collect();
                let working = mine
                    .iter()
                    .filter(|p| p.at_work())
                    .map(|p| format!("pane {} of {}", p.pane, p.stream))
                    .collect();
                let panes = mine.len();
                self.modal = Some(Modal::RmSession {
                    name,
                    plan,
                    working,
                    panes,
                    typed: String::new(),
                });
            }
            // Taken at the top, before `busy` is cleared.
            Job::RmPr { .. } | Job::RmWork { .. } => {}
            Job::Removed { name, note } => {
                self.done(match note {
                    Some(n) => format!("removed {name}; {n}"),
                    None => format!("removed {name}"),
                });
            }
            Job::Failed(e) => {
                // A list that could not load leaves no empty picker (REQ-162).
                if self.finder.as_ref().is_some_and(|f| f.loading_reviews()) {
                    self.finder = None;
                }
                self.fail(e);
            }
            Job::Prs { prs, requested } => {
                if let Some(f) = &mut self.finder {
                    f.set_reviews(prs, requested);
                }
            }
            Job::Reviewed(r) => {
                let r = *r;
                let (id, name, n) = (r.entry.id.clone(), r.entry.name.clone(), r.entry.pr);
                if r.existed {
                    self.done(format!("{name} is at the head of #{n} now"));
                    self.open_entry(r.entry, false);
                    self.modal = Some(Modal::Resend {
                        stream: id,
                        name,
                        prompt: r.prompt,
                    });
                } else {
                    self.done(format!("reviewing #{n} in {name}"));
                    self.pending_prompt = Some((id, r.prompt));
                    self.open_entry(r.entry, r.setup);
                    // Whatever `open_entry` did not send is not for later.
                    self.pending_prompt = None;
                }
            }
            Job::Renamed { entry, from } => {
                self.done(format!("renamed to {} on {}", entry.name, entry.branch));
                self.moved(entry, from);
            }
            Job::Said(msg) => self.done(msg),
            Job::DiffPart { .. } => unreachable!("on_job answers it before the registry reload"),
            Job::Diff {
                name,
                base,
                dir,
                general,
                into,
            } => {
                let view = View::Diff(diffview::DiffView::new(name, base, dir, general));
                match into {
                    Some(id) => {
                        self.views.insert(id, view);
                    }
                    None => self.show_view("view:diff".into(), view),
                }
            }
            Job::RmProject { id, project } => match &mut self.modal {
                Some(Modal::Rm {
                    entry,
                    project: slot,
                    ..
                }) if entry.id == id => {
                    let entry = entry.clone();
                    *slot = Some(project.clone());
                    self.ask_pr(entry, project);
                }
                _ => trace::mark("rm_project_dropped", 0),
            },
        }
        if let Err(e) = reload {
            self.fail(format!("{e:#}"));
        }
    }
}

/// Makes worktree `name` of `project` from `from`'s branch (worker thread).
fn created(project: &Project, from: Entry, name: String, wrote: Option<String>) -> Result<Job> {
    let o = NewOptions {
        name,
        from: from.branch,
        branch: String::new(),
        session: None,
    };
    let entry = actions::new_stream(project, &o, &registry::default_path()?)?;
    Ok(Job::Created {
        entry,
        setup: !project.cfg.setup.is_empty(),
        wrote,
    })
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

/// Replays a `Snapshot`'s bytes into a new parser: up to 2 MiB, so it runs on
/// the reader thread, not the UI's.
fn replay(name: &str, rows: u16, cols: u16, bytes: &[u8]) -> vt100::Parser<keys::Flags> {
    trace::span(name, || {
        let mut parser =
            vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK, keys::Flags::default());
        parser.process(bytes);
        parser
    })
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

/// The key an action has now, as shown (`␣`, `tab`, `o`).
fn key(action: &str) -> String {
    crate::settings::label(&crate::settings::get().key(action))
}

/// Shells don't count as "running something" when closing.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "nu"];

/// The terminal size inside a pane's border.
fn inner(r: Rect) -> (u16, u16) {
    (r.w.saturating_sub(2).max(1), r.h.saturating_sub(2).max(1))
}

/// `n` without the leaves whose run is empty; their siblings take the space.
fn prune(n: Node) -> Option<Node> {
    if n.run.is_some() {
        return n.run.as_deref().is_some_and(|r| !r.is_empty()).then_some(n);
    }
    let (Some(dir), Some(a), Some(b)) = (n.split, n.a, n.b) else {
        return None;
    };
    match (prune(*a), prune(*b)) {
        (Some(a), Some(b)) => Some(Node::split(dir, n.ratio.unwrap_or(0.5), a, b)),
        (one, other) => one.or(other),
    }
}

/// `n` with every ratio given and rounded to hundredths, so a layout and
/// the live tree it opened compare equal.
fn even(n: &Node) -> Node {
    match (n.split, &n.a, &n.b) {
        (Some(dir), Some(a), Some(b)) => Node::split(
            dir,
            (n.ratio.unwrap_or(0.5) * 100.0).round() / 100.0,
            even(a),
            even(b),
        ),
        _ => n.clone(),
    }
}

/// The registry and the session's folders, read and nothing else: no
/// `fill_roots`, no save, no migration.
fn load_rows() -> Result<Vec<Row>> {
    let folders = crate::folders::Folders::load(&crate::folders::default_path()?)?;
    let reg = Registry::load(&registry::default_path()?)?;
    Ok(crate::sidebar::rows(
        &folders,
        reg,
        &crate::session::current(),
    ))
}

#[cfg(test)]
mod drain_tests {
    use super::drain;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A burst is applied whole, then drawn.
    #[test]
    fn applies_what_is_queued() {
        let (tx, rx) = mpsc::channel();
        for i in 0..100 {
            tx.send(i).unwrap();
        }
        let mut got = 0;
        drain(&rx, Duration::from_secs(5), |_| {
            got += 1;
            true
        });
        assert_eq!(got, 100);
    }

    /// Output that never pauses for 2 ms still gets a frame.
    #[test]
    fn a_stream_that_never_pauses_still_redraws() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while tx.send(()).is_ok() {
                std::thread::sleep(Duration::from_micros(200));
            }
        });
        let start = Instant::now();
        drain(&rx, Duration::from_millis(20), |()| true);
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn stops_when_asked() {
        let (tx, rx) = mpsc::channel();
        for i in 0..10 {
            tx.send(i).unwrap();
        }
        let mut got = Vec::new();
        drain(&rx, Duration::from_secs(5), |i| {
            got.push(i);
            i < 3
        });
        assert_eq!(got, [0, 1, 2, 3]);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::{even, prune};
    use crate::layout::{Dir, Node};

    /// REQ-120, 121: viewers drop out, and a missing ratio equals 0.5.
    #[test]
    fn live_layouts_compare_with_their_config() {
        let t = Node::split(
            Dir::Down,
            0.7,
            Node::split(Dir::Right, 0.5, Node::leaf("editor"), Node::leaf("")),
            Node::leaf("shell"),
        );
        let want = Node::split(Dir::Down, 0.7, Node::leaf("editor"), Node::leaf("shell"));
        assert_eq!(prune(t), Some(want));
        assert_eq!(prune(Node::leaf("")), None);
        let mut bare = Node::split(Dir::Right, 0.5, Node::leaf("a"), Node::leaf("b"));
        bare.ratio = None;
        assert_eq!(
            even(&bare),
            Node::split(Dir::Right, 0.5, Node::leaf("a"), Node::leaf("b"))
        );
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    use crate::proto::AgentState;
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;

    /// `tick_motion` reads the process-wide settings: one test at a time.
    static SETTINGS: Mutex<()> = Mutex::new(());

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            ..Entry::default()
        }
    }

    fn stream(id: &str) -> Stream {
        Stream {
            entry: entry(id),
            cfg: Default::default(),
            base: String::new(),
            tree: Node::leaf(""),
            session: String::new(),
            stamp: Default::default(),
            jw_session: "main".into(),
        }
    }

    fn working(id: &str) -> PaneInfo {
        PaneInfo {
            pane: 1,
            stream: id.into(),
            role: "agent".into(),
            exited: None,
            fg: None,
            busy: false,
            bell: false,
            agent: Some(AgentState::Working),
            cwd: None,
            name: None,
        }
    }

    /// An app on three workspaces, `a` current, with `animations` as given.
    /// The guard keeps other tests off the settings meanwhile.
    fn app(animations: bool) -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::set(crate::settings::Settings {
            animations: Some(animations),
            ..Default::default()
        });
        let dir = std::env::temp_dir().join(format!("jwm{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let _ = std::fs::remove_file(&sock);
        let _listener = UnixListener::bind(&sock).unwrap();
        let client = Client::connect(&sock).unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut app = App::bare(client, tx, Leader::DEFAULT, (120, 35));
        app.rows = ["a", "b", "c"]
            .map(|id| Row {
                entry: entry(id),
                child: false,
            })
            .into();
        app.active = Some(stream("a"));
        (app, guard)
    }

    fn running(id: &str, pane: PaneId, role: &str, fg: &str) -> PaneInfo {
        PaneInfo {
            pane,
            stream: id.into(),
            role: role.into(),
            exited: None,
            fg: Some(fg.into()),
            busy: true,
            bell: false,
            agent: None,
            cwd: None,
            name: None,
        }
    }

    #[test]
    fn an_agent_in_a_shell_pane_gets_the_agent_marks() {
        let (mut app, _g) = app(false);
        app.daemon_panes = vec![running("a", 7, "shell", "claude")];
        assert_eq!(app.marks("a"), ['✻']);
        app.daemon_panes = vec![running("a", 7, "shell", "nvim")];
        assert!(app.marks("a").is_empty());
    }

    #[test]
    fn the_title_names_an_agent_program_and_flags_it() {
        let (mut app, _g) = app(false);
        app.daemon_panes = vec![
            running("a", 7, "shell", "claude"),
            running("a", 8, "shell", "nvim"),
        ];
        assert_eq!(
            app.pane_title(7),
            (String::new(), Some("claude".into()), true)
        );
        assert_eq!(
            app.pane_title(8),
            (String::new(), Some("nvim".into()), false)
        );
    }

    #[test]
    fn the_band_starts_on_the_current_row_without_motion() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.tick_motion(t0);
        assert_eq!(app.band, Some(0));
        assert!(!app.motion.active(t0));
    }

    #[test]
    fn a_switch_glides_the_band_and_kicks_the_loop() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.tick_motion(t0);
        app.active = Some(stream("c"));
        app.tick_motion(t0);
        assert_eq!(app.band, Some(0), "still on the old row at the start");
        assert!(app.motion.active(t0 + MS(100)));
        app.tick_motion(t0 + MS(60));
        let mid = app.band.unwrap();
        assert!(mid > 0 && mid <= 2, "on its way: {mid}");
        let end = t0 + anim::SWITCH;
        app.tick_motion(end);
        assert_eq!(app.band, Some(2));
        assert!(!app.motion.active(end));
    }

    #[test]
    fn with_animations_off_the_band_jumps_and_nothing_moves() {
        let (mut app, _g) = app(false);
        let t0 = Instant::now();
        app.tick_motion(t0);
        app.active = Some(stream("b"));
        app.tick_motion(t0);
        assert_eq!(app.band, Some(1));
        assert!(!app.motion.active(t0));
        assert!(app.switch.is_none());
    }

    #[test]
    fn with_no_workspace_there_is_no_band() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.tick_motion(t0);
        app.active = None;
        app.tick_motion(t0);
        assert_eq!(app.band, None);
        assert!(app.bar.is_none());
    }

    #[test]
    fn a_working_agent_spins_and_the_loop_knows_when_to_step() {
        let (mut app, _g) = app(true);
        app.daemon_panes = vec![working("a")];
        let t0 = app.born + MS(160);
        app.tick_motion(t0);
        assert_eq!(app.spinner, Some(anim::spinner(MS(160))));
        assert_eq!(app.spin_due, Some(t0 + anim::SPIN_EVERY));
        app.daemon_panes.clear();
        app.tick_motion(t0 + MS(80));
        assert_eq!((app.spinner, app.spin_due), (None, None));
    }

    #[test]
    fn with_animations_off_the_spinner_stays_still() {
        let (mut app, _g) = app(false);
        app.daemon_panes = vec![working("a")];
        app.tick_motion(Instant::now());
        assert_eq!((app.spinner, app.spin_due), (None, None));
    }

    fn project() -> Project {
        Project {
            name: "p".into(),
            repo: crate::connectors::git::Repo {
                root: "/r".into(),
                common_dir: "/r/.git".into(),
                remote: String::new(),
            },
            cfg: Default::default(),
        }
    }

    fn rm_modal(id: &str, plan: actions::RmPlan, typed: &str, pr: modal::PrLine) -> Modal {
        Modal::Rm {
            entry: Entry {
                name: "fast-rm".into(),
                ..entry(id)
            },
            project: Some(project()),
            plan,
            typed: typed.into(),
            error: None,
            pr,
            pending: false,
        }
    }

    fn dirty() -> actions::RmPlan {
        actions::RmPlan {
            dirty: vec![" M a.rs".into()],
            ..Default::default()
        }
    }

    fn merged() -> modal::PrLine {
        modal::PrLine::Merged {
            number: 45,
            done: Box::new((actions::RmPlan::default(), Default::default())),
        }
    }

    /// REQ-259, REQ-261: gh's late answer changes the PR line and nothing
    /// else: what was typed stays, and the name is still required.
    #[test]
    fn a_late_pr_answer_changes_only_the_pr_line() {
        let (mut app, _g) = app(true);
        app.modal = Some(rm_modal("a", dirty(), "fa", modal::PrLine::Checking));
        app.on_job(Job::RmPr {
            id: "a".into(),
            line: merged(),
        });
        let Some(Modal::Rm {
            typed, plan, pr, ..
        }) = &app.modal
        else {
            panic!("the modal went away");
        };
        assert_eq!(typed, "fa");
        assert!(plan.loses_work());
        assert!(matches!(pr, modal::PrLine::Merged { number: 45, .. }));
        app.submit_modal();
        assert!(matches!(
            &app.modal,
            Some(Modal::Rm { error: Some(_), typed, .. }) if typed == "fa"
        ));
    }

    /// A PR answer ends the "checking the PR" wait, and leaves a removal that
    /// began meanwhile showing as busy.
    #[test]
    fn a_pr_answer_leaves_another_jobs_busy_alone() {
        let (mut app, _g) = app(true);
        let pr = || Job::RmPr {
            id: "a".into(),
            line: modal::PrLine::NoPr,
        };
        app.busy = Some(format!("{PR_BUSY}wt1"));
        app.on_job(pr());
        assert_eq!(app.busy, None);
        app.busy = Some("removing wt1".into());
        app.on_job(pr());
        assert_eq!(app.busy.as_deref(), Some("removing wt1"));
    }

    /// REQ-260: no modal, or another one, takes the answer.
    #[test]
    fn a_late_pr_answer_for_another_modal_is_dropped() {
        let (mut app, _g) = app(true);
        app.on_job(Job::RmPr {
            id: "a".into(),
            line: merged(),
        });
        assert!(app.modal.is_none());
        app.modal = Some(rm_modal(
            "b",
            actions::RmPlan::default(),
            "",
            modal::PrLine::Checking,
        ));
        app.on_job(Job::RmPr {
            id: "a".into(),
            line: merged(),
        });
        assert!(matches!(
            app.modal,
            Some(Modal::Rm {
                pr: modal::PrLine::Checking,
                ..
            })
        ));
    }

    /// REQ-262: a merged PR offers the done path, and only while nothing is
    /// typed; `d` is a letter of the name when the name must be typed.
    #[test]
    fn done_is_offered_only_while_nothing_is_typed() {
        let key = |m: &mut Modal, c: KeyCode| m.key(KeyEvent::from(c));
        let mut clean = rm_modal("a", actions::RmPlan::default(), "", merged());
        assert!(matches!(
            key(&mut clean, KeyCode::Char('d')),
            Outcome::ToDone
        ));
        let mut must_type = rm_modal("a", dirty(), "", merged());
        assert!(matches!(
            key(&mut must_type, KeyCode::Char('d')),
            Outcome::Stay
        ));
        // That `d` went into the typed name; `⇥` works from an empty one.
        let mut must_type = rm_modal("a", dirty(), "", merged());
        assert!(matches!(key(&mut must_type, KeyCode::Tab), Outcome::ToDone));
        let mut typing = rm_modal("a", dirty(), "f", merged());
        assert!(matches!(key(&mut typing, KeyCode::Tab), Outcome::Stay));
        let mut unknown = rm_modal("a", actions::RmPlan::default(), "", modal::PrLine::Unknown);
        assert!(matches!(
            key(&mut unknown, KeyCode::Char('d')),
            Outcome::Stay
        ));
    }

    fn shown(m: &Modal, spin: char) -> Vec<String> {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        t.draw(|f| m.draw(f, spin)).unwrap();
        let buf = t.backend().buffer();
        (0..24u16)
            .map(|y| {
                (0..80u16)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// REQ-258, REQ-259: the PR row is there from the first frame, so its
    /// answer moves no other line of the modal.
    #[test]
    fn the_pr_row_fills_in_place() {
        let (_app, _g) = app(true);
        let plan = actions::RmPlan {
            deletes: vec!["/w/wt1".into()],
            ..Default::default()
        };
        let waiting = shown(
            &rm_modal("a", plan.clone(), "", modal::PrLine::Checking),
            '⠋',
        );
        let row = waiting.iter().position(|l| l.contains("PR  ⠋ checking…"));
        let row = row.expect("no PR row while waiting");
        for (line, text) in [
            (
                modal::PrLine::Merged {
                    number: 45,
                    done: Box::new((actions::RmPlan::default(), Default::default())),
                },
                "#45 merged",
            ),
            (modal::PrLine::Info("#46 open".into()), "#46 open"),
            (modal::PrLine::Unknown, "unknown (gh failed or timed out)"),
            (modal::PrLine::NoPr, "none"),
        ] {
            let after = shown(&rm_modal("a", plan.clone(), "", line), '⠋');
            assert!(after[row].contains(text), "{:?}", after[row]);
            for (i, (a, b)) in waiting.iter().zip(&after).enumerate() {
                if i != row {
                    assert_eq!(a, b, "line {i} moved");
                }
            }
        }
        let merged = shown(&rm_modal("a", plan, "", merged()), '⠋');
        assert!(
            merged[row].contains(" d  finish as done"),
            "{:?}",
            merged[row]
        );
    }

    fn pending_modal(id: &str) -> Modal {
        let Modal::Rm {
            entry,
            project,
            plan,
            typed,
            error,
            pr,
            ..
        } = rm_modal(id, actions::RmPlan::default(), "", modal::PrLine::Checking)
        else {
            unreachable!()
        };
        Modal::Rm {
            entry,
            project,
            plan,
            typed,
            error,
            pr,
            pending: true,
        }
    }

    /// REQ-268: while git has not said what the worktree holds, only `esc`
    /// acts; nothing is typed and nothing confirms.
    #[test]
    fn nothing_acts_while_git_is_still_asked() {
        let (mut app, _g) = app(true);
        let mut m = pending_modal("a");
        for c in [
            KeyCode::Enter,
            KeyCode::Char('y'),
            KeyCode::Char('n'),
            KeyCode::Char('d'),
            KeyCode::Char('f'),
            KeyCode::Tab,
            KeyCode::Backspace,
        ] {
            assert!(matches!(m.key(KeyEvent::from(c)), Outcome::Stay), "{c:?}");
        }
        assert!(matches!(
            m.key(KeyEvent::from(KeyCode::Esc)),
            Outcome::Cancel
        ));
        app.modal = Some(m);
        app.submit_modal();
        assert!(matches!(
            &app.modal,
            Some(Modal::Rm { pending: true, typed, .. }) if typed.is_empty()
        ));
    }

    /// REQ-268: the work row is there from the first frame and gives way to
    /// the plan; the name is asked empty when work would be lost.
    #[test]
    fn git_fills_the_plan_of_an_open_modal() {
        let (mut app, _g) = app(true);
        let waiting = shown(&pending_modal("a"), '⠋');
        assert!(
            waiting
                .iter()
                .any(|l| l.contains("Work ⠋ checking files and commits…"))
        );
        assert!(waiting.iter().any(|l| l.contains("wait for the check")));
        app.modal = Some(pending_modal("a"));
        app.on_job(Job::RmWork {
            id: "a".into(),
            plan: Ok(dirty()),
        });
        let Some(
            m @ Modal::Rm {
                pending,
                typed,
                plan,
                ..
            },
        ) = &app.modal
        else {
            panic!("the modal went away");
        };
        assert!(!pending && typed.is_empty() && plan.loses_work());
        let after = shown(m, '⠋');
        assert!(after.iter().any(|l| l.contains("1 uncommitted file(s)")));
        assert!(after.iter().any(|l| l.contains("Type fast-rm")));
        assert!(!after.iter().any(|l| l.contains("Work ")));
    }

    /// REQ-268: the modal opens with no project, and neither `↵` nor a
    /// project for another modal makes it act.
    #[test]
    fn the_modal_waits_for_its_project_too() {
        let (mut app, _g) = app(true);
        let Modal::Rm {
            entry, plan, pr, ..
        } = pending_modal("a")
        else {
            unreachable!()
        };
        app.modal = Some(Modal::Rm {
            entry,
            project: None,
            plan,
            typed: String::new(),
            error: None,
            pr,
            pending: false,
        });
        app.submit_modal();
        assert!(matches!(&app.modal, Some(Modal::Rm { project: None, .. })));
        app.on_job(Job::RmProject {
            id: "b".into(),
            project: project(),
        });
        assert!(matches!(&app.modal, Some(Modal::Rm { project: None, .. })));
    }

    /// REQ-269: a remove that loses work says so in the footer and the status
    /// bar, and one that does not keeps `y`.
    #[test]
    fn the_footer_says_when_the_name_must_be_typed() {
        let footer = |m: &Modal| {
            let rows = shown(m, '⠋');
            rows.into_iter().find(|l| l.contains("cancel")).unwrap()
        };
        let lossy = rm_modal("a", dirty(), "", modal::PrLine::NoPr);
        assert!(footer(&lossy).contains("remove once the name is typed"));
        let clean = rm_modal("a", actions::RmPlan::default(), "", modal::PrLine::NoPr);
        assert!(footer(&clean).contains(" y  remove"));
        let (mut app, _g) = app(true);
        app.modal = Some(lossy);
        assert_eq!(draw::mode_of(&app), draw::Mode::ConfirmName);
        app.modal = Some(pending_modal("a"));
        assert_eq!(draw::mode_of(&app), draw::Mode::ConfirmWait);
        app.modal = Some(clean);
        assert_eq!(draw::mode_of(&app), draw::Mode::Confirm);
    }

    /// REQ-268: git's answer for a closed or another modal is dropped; its
    /// failure closes the modal it belongs to.
    #[test]
    fn git_answers_only_the_modal_they_belong_to() {
        let (mut app, _g) = app(true);
        app.on_job(Job::RmWork {
            id: "a".into(),
            plan: Ok(dirty()),
        });
        assert!(app.modal.is_none());
        app.modal = Some(pending_modal("b"));
        app.on_job(Job::RmWork {
            id: "a".into(),
            plan: Ok(dirty()),
        });
        assert!(matches!(app.modal, Some(Modal::Rm { pending: true, .. })));
        app.on_job(Job::RmWork {
            id: "b".into(),
            plan: Err("git failed".into()),
        });
        assert!(app.modal.is_none());
    }

    /// REQ-258: the PR line spins while gh is asked.
    #[test]
    fn the_pr_line_spins_while_gh_is_asked() {
        let (mut app, _g) = app(true);
        app.modal = Some(rm_modal(
            "a",
            actions::RmPlan::default(),
            "",
            modal::PrLine::Checking,
        ));
        app.tick_motion(Instant::now());
        assert!(app.spinner.is_some() && app.spin_due.is_some());
        app.modal = Some(rm_modal(
            "a",
            actions::RmPlan::default(),
            "",
            modal::PrLine::Unknown,
        ));
        app.tick_motion(Instant::now());
        assert_eq!((app.spinner, app.spin_due), (None, None));
    }

    #[test]
    fn a_working_agent_below_the_sidebar_does_not_wake_the_loop() {
        let (mut app, _g) = app(true);
        app.size = (120, 6);
        app.rows = (0..20)
            .map(|i| Row {
                entry: entry(&format!("w{i}")),
                child: false,
            })
            .collect();
        app.active = Some(stream("w0"));
        app.daemon_panes = vec![working("w19")];
        app.tick_motion(Instant::now());
        assert_eq!(app.spin_due, None);
    }

    #[test]
    fn an_idle_app_blocks_until_a_message_comes() {
        let (mut app, _g) = app(true);
        assert!(app.next_wake(Instant::now()).is_none());
    }

    #[test]
    fn a_transition_wakes_for_the_next_frame_after_the_last_draw() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.last_draw = t0;
        app.motion.kick(MS(100), t0);
        let (at, wake) = app.next_wake(t0 + MS(5)).unwrap();
        assert_eq!(at, t0 + FRAME);
        assert!(matches!(wake, Wake::Frame));
        assert!(app.next_wake(t0 + MS(100)).is_none(), "blocks once it ends");
    }

    #[test]
    fn a_held_drag_wakes_for_the_edge_and_releasing_it_stops_it() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.edge = Some(1);
        let (at, wake) = app.next_wake(t0).unwrap();
        assert_eq!(at, t0 + EDGE_EVERY);
        assert!(matches!(wake, Wake::Edge));
        app.edge = None;
        assert!(app.next_wake(t0).is_none());
        assert!(app.edge_due.is_none());
    }

    #[test]
    fn the_leader_wakes_for_the_which_key_until_it_shows() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.leader_at = Some(t0);
        let (at, wake) = app.next_wake(t0).unwrap();
        assert_eq!(at, t0 + which_after());
        assert!(matches!(wake, Wake::Which));
        app.which = true;
        assert!(app.next_wake(t0).is_none());
    }

    #[test]
    fn the_soonest_wake_wins() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.last_draw = t0;
        app.motion.kick(MS(100), t0);
        app.spin_due = Some(t0 + MS(80));
        app.edge = Some(1);
        app.edge_due = Some(t0 + MS(60));
        let (at, wake) = app.next_wake(t0).unwrap();
        assert_eq!(at, t0 + FRAME);
        assert!(matches!(wake, Wake::Frame));
    }

    #[test]
    fn a_message_wakes_the_loop_when_it_starts_to_leave() {
        let (mut app, _g) = app(true);
        let t0 = Instant::now();
        app.status = Some(Status {
            text: "done".into(),
            tone: Tone::Done,
            at: t0,
        });
        let (at, _) = app.next_wake(t0 + MS(500)).unwrap();
        assert_eq!(at, t0 + STATUS_FOR - anim::MESSAGE_EDGE);
        assert!(app.next_wake(t0 + STATUS_FOR).is_none());
    }

    #[test]
    fn a_replayed_snapshot_is_the_screen_its_bytes_draw() {
        let p = replay("t", 3, 10, b"hi\r\nthere");
        assert_eq!(p.screen().size(), (3, 10));
        assert_eq!(p.screen().contents(), "hi\nthere");
    }

    #[test]
    fn a_history_takes_the_screens_place_with_what_happened_since() {
        let (mut app, _g) = app(true);
        let screen = replay("t", 3, 10, b"3\r\n4\r\n5");
        app.install_snapshot(1, "shell".into(), screen, Some(7));
        app.on_daemon(DaemonMsg::Output {
            pane: 1,
            bytes: b"\r\n6".to_vec(),
        });

        // An older history (a pane attached again since) is not this one's.
        let stale = replay("t", 3, 10, b"x");
        app.on_history(1, 6, stale);
        assert!(app.panes[&1].history.is_some());

        let full = replay("t", 3, 10, b"1\r\n2\r\n3\r\n4\r\n5");
        app.on_history(1, 7, full);
        let view = app.panes.get_mut(&1).unwrap();
        assert!(view.history.is_none());
        assert_eq!(view.parser.screen().contents(), "4\n5\n6");
        view.parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(view.parser.screen().scrollback(), 3);
    }

    fn rows(ids: &[&str]) -> Vec<Row> {
        ids.iter()
            .map(|id| Row {
                entry: entry(id),
                child: false,
            })
            .collect()
    }

    fn ids(app: &App) -> Vec<&str> {
        app.rows.iter().map(|r| r.entry.id.as_str()).collect()
    }

    #[test]
    fn rows_from_outside_replace_the_sidebar_and_keep_the_open_workspace() {
        let (mut app, _g) = app(false);
        app.on_rows(Ok(rows(&["a", "b", "c", "d"])));
        assert_eq!(ids(&app), ["a", "b", "c", "d"]);
        assert_eq!(app.active.as_ref().unwrap().entry.id, "a");
        assert!(app.pending_rows.is_none());
    }

    #[test]
    fn rows_wait_while_a_modal_is_open_and_land_when_it_closes() {
        let (mut app, _g) = app(false);
        app.modal = Some(Modal::Name {
            pane: 1,
            text: String::new(),
        });
        app.on_rows(Ok(rows(&["a", "z"])));
        assert_eq!(ids(&app), ["a", "b", "c"], "the modal's rows stay put");
        app.apply_pending_rows();
        assert_eq!(ids(&app), ["a", "b", "c"]);
        app.modal = None;
        app.apply_pending_rows();
        assert_eq!(ids(&app), ["a", "z"]);
        assert!(app.pending_rows.is_none());
    }

    #[test]
    fn rows_that_could_not_be_read_change_nothing() {
        let (mut app, _g) = app(false);
        app.on_rows(Err(anyhow::anyhow!("truncated")));
        assert_eq!(ids(&app), ["a", "b", "c"]);
        assert!(app.status.is_none(), "and say nothing");
    }

    #[test]
    fn the_idle_loop_still_blocks_with_the_registry_watch() {
        let (mut app, _g) = app(false);
        assert!(app.next_wake(Instant::now()).is_none());
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// An app whose daemon is a thread that hands back what it is sent.
    fn listening(name: &str) -> (App, mpsc::Receiver<ClientMsg>) {
        let dir = std::env::temp_dir().join(format!("jwr{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let (got_tx, got_rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            while let Ok(Some(m)) = crate::proto::read_frame::<ClientMsg>(&mut s) {
                if got_tx.send(m).is_err() {
                    break;
                }
            }
        });
        let client = Client::connect(&sock).unwrap();
        let (tx, _rx) = mpsc::channel();
        (App::bare(client, tx, Leader::DEFAULT, (120, 35)), got_rx)
    }

    fn prompt_of(m: ClientMsg) -> (String, String) {
        match m {
            ClientMsg::Prompt { stream, text, .. } => (stream, text),
            _ => panic!("expected a Prompt"),
        }
    }

    /// REQ-171: the prompt goes to the review's own workspace once, and
    /// another workspace opening does not take it.
    #[test]
    fn the_review_prompt_goes_out_only_for_its_workspace() {
        let (mut app, got) = listening("own");
        app.pending_prompt = Some(("r1".into(), "Review #7".into()));
        app.send_pending_prompt("other");
        assert!(got.recv_timeout(Duration::from_millis(150)).is_err());
        assert!(app.pending_prompt.is_some());

        app.send_pending_prompt("r1");
        let sent = prompt_of(got.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(sent, ("r1".to_string(), "Review #7".to_string()));
        assert!(app.pending_prompt.is_none());
        app.send_pending_prompt("r1");
        assert!(got.recv_timeout(Duration::from_millis(150)).is_err());
    }

    /// REQ-173: only a yes sends the prompt to a review that existed.
    #[test]
    fn a_yes_sends_the_prompt_again_and_a_no_does_not() {
        let (mut app, got) = listening("resend");
        let ask = || Modal::Resend {
            stream: "r1".into(),
            name: "review-7".into(),
            prompt: "Review #7".into(),
        };
        app.modal = Some(ask());
        assert!(matches!(
            app.modal
                .as_mut()
                .unwrap()
                .key(KeyEvent::from(KeyCode::Esc)),
            Outcome::Cancel
        ));
        assert!(got.recv_timeout(Duration::from_millis(150)).is_err());

        app.modal = Some(ask());
        assert!(matches!(
            app.modal
                .as_mut()
                .unwrap()
                .key(KeyEvent::from(KeyCode::Char('y'))),
            Outcome::Submit
        ));
        app.submit_modal();
        let sent = prompt_of(got.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(sent.0, "r1");
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// An app with a daemon that says nothing, and the channel its jobs
    /// answer on.
    fn quiet(name: &str) -> (App, mpsc::Receiver<Msg>) {
        let dir = std::env::temp_dir().join(format!("jwd{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            while let Ok(Some(_)) = crate::proto::read_frame::<ClientMsg>(&mut s) {}
        });
        let client = Client::connect(&sock).unwrap();
        let (tx, rx) = mpsc::channel();
        (App::bare(client, tx, Leader::DEFAULT, (120, 35)), rx)
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn screen(app: &mut App, id: PaneId) -> String {
        let Some(View::Diff(d)) = app.views.get_mut(&id) else {
            panic!("no diff pane");
        };
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 14)).unwrap();
        t.draw(|f| d.draw(f, f.area())).unwrap();
        let buf = t.backend().buffer();
        (0..14u16)
            .map(|y| {
                (0..130u16)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// REQ-235, REQ-237: `2` asks for Uncommitted, git runs on a worker, the
    /// answer comes back as a job and fills the view; the answer is taken
    /// without reading the registry again.
    #[test]
    fn a_view_is_loaded_by_a_job_and_filled_by_its_answer() {
        let (mut app, rx) = quiet("job");
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        std::fs::write(repo.path().join("a.txt"), "hello\n").unwrap();
        let view = diffview::DiffView::new(
            "t".into(),
            String::new(),
            repo.path().to_string_lossy().into_owned(),
            Ok(crate::diff::Loaded::Files(Vec::new())),
        );
        app.views.insert(7, View::Diff(view));
        app.focus = Some(7);

        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert!(screen(&mut app, 7).contains("loading"));
        let Msg::Job(job) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("expected a job");
        };
        assert!(matches!(*job, Job::DiffPart { .. }));
        app.on_job(*job);
        let s = screen(&mut app, 7);
        assert!(s.contains("a.txt") && s.contains("untracked"), "{s}");
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "nothing else asked"
        );
    }
}
