//! The daemon owns every terminal (docs/specs/rust-tui.md, REQ-3…5, 10, 14)
//! and each workspace's tree of panes (REQ-34): one PTY per pane plus a
//! vt100 parser that keeps its screen, so clients can come and go while the
//! processes keep running. Every change to a tree goes to the clients
//! watching that workspace and to `session.json`.
//!
//! Threads: one accept loop, a reader and a writer per client, and a reader
//! per pane. A pane's `state` lock covers both its parser and its
//! subscribers, which is what makes "snapshot, then output" race free. Locks
//! are taken in the order saving → workspaces → panes → a pane's state.

// The code says `crate::proto` and `crate::layout`, as it did when this
// was one crate.
use jw_core::{core, layout, program};
use jw_proto::kitty::{self, Kitty};
use jw_proto::proto;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

mod chat;
mod mobile;
mod outbox;
mod ring;

use jw_web::bridge::Look;
use mobile::{Mobile, Tuning};
use outbox::Tx;
use ring::Ring;

use crate::layout::{Dir, Tree};
use crate::proto::{
    AgentState, ClientMsg, DaemonMsg, NewPane, PROTOCOL, PaneId, PaneInfo, PaneLeaf, Run, Theme,
    is_view, pidfile, read_frame, write_frame,
};

/// How often the panes' output is written out, between tree changes.
const SAVE_EVERY: Duration = Duration::from_secs(30);

/// How long a signal waits for mobile to stop (REQ-9).
const MOBILE_STOP_WITHIN: Duration = Duration::from_secs(2);

/// Lines of scrollback each pane keeps.
const SCROLLBACK: usize = 10_000;

/// How often a followed transcript is read for what it gained.
const CHAT_EVERY: Duration = Duration::from_millis(300);

/// How often the phones watching a pane are sent its screen, when it changed.
const TEXT_EVERY: Duration = Duration::from_millis(80);

/// Runs the daemon on `socket` until the process is killed, or stopped by a
/// signal (see `save_on_signal`). `look` is what a phone's `ready` carries
/// besides the protocol: the themes live in the TUI crate, which this one
/// cannot depend on, so the binary hands them in (mobile spec, RISK-19).
pub fn run(socket: &Path, look: Look) -> Result<()> {
    let listener = bind(socket)?;
    let pidfile = pidfile(socket);
    fs::write(&pidfile, format!("{}\n", std::process::id()))
        .with_context(|| format!("writing {}", pidfile.display()))?;

    let daemon = Arc::new(Daemon::new(
        session_path(socket),
        Mobile::new(socket.to_path_buf(), look, Tuning::default()),
    ));
    daemon.restore();
    save_on_signal(Arc::clone(&daemon), socket.to_path_buf(), pidfile)?;
    let saver = Arc::clone(&daemon);
    thread::spawn(move || {
        loop {
            thread::sleep(SAVE_EVERY);
            saver.save_scrollback();
        }
    });
    let chatter = Arc::clone(&daemon);
    thread::spawn(move || {
        loop {
            thread::sleep(CHAT_EVERY);
            chatter.chat_poll();
        }
    });
    let texter = Arc::clone(&daemon);
    thread::spawn(move || {
        loop {
            thread::sleep(TEXT_EVERY);
            texter.flush_text();
        }
    });
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let daemon = Arc::clone(&daemon);
                thread::spawn(move || daemon.serve(stream));
            }
            Err(e) => eprintln!("jw daemon: accept: {e}"),
        }
    }
    Ok(())
}

/// The write end of the pipe `on_signal` wakes `save_on_signal`'s thread
/// through.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    // SAFETY: write(2) is async-signal-safe; the fd stays open for good.
    unsafe { libc::write(fd, [0u8].as_ptr().cast(), 1) };
}

/// On SIGTERM, SIGINT or SIGHUP, saves `session.json` and every pane's
/// scrollback, then exits. Without it a stop loses what the panes printed
/// since the last save, up to `SAVE_EVERY` of it. A handler may only do
/// async-signal-safe work, so it writes a byte to a pipe and a thread saves.
fn save_on_signal(daemon: Arc<Daemon>, socket: PathBuf, pidfile: PathBuf) -> Result<()> {
    let mut fds = [0; 2];
    // SAFETY: plain libc calls on fds we own. The panes' processes don't
    // inherit the pipe (CLOEXEC), and exec resets the handler for them.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error()).context("creating the signal pipe");
        }
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        SIGNAL_PIPE.store(fds[1], Ordering::Relaxed);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
    // SAFETY: the read end is ours alone from here on.
    let mut wake = unsafe { fs::File::from_raw_fd(fds[0]) };
    thread::spawn(move || {
        let _ = wake.read(&mut [0u8]);
        daemon.save();
        // REQ-9: `cloudflared` and every phone go before the daemon does, and
        // a hung stop does not keep it from exiting.
        daemon.mobile.stop(MOBILE_STOP_WITHIN);
        let _ = fs::remove_file(&socket);
        let _ = fs::remove_file(&pidfile);
        std::process::exit(0);
    });
    Ok(())
}

/// `session.json` in the state dir for the usual socket; next to the socket
/// for any other (tests, a second daemon), so they never touch the real one.
fn session_path(socket: &Path) -> Option<PathBuf> {
    if socket == crate::proto::default_socket_path() {
        crate::core::registry::state_dir()
            .ok()
            .map(|d| d.join("session.json"))
    } else {
        socket.parent().map(|d| d.join("session.json"))
    }
}

/// Binds the socket, refusing if another daemon answers on it and removing
/// the file a dead one left behind.
fn bind(socket: &Path) -> Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    if UnixStream::connect(socket).is_ok() {
        bail!("a jw daemon is already listening on {}", socket.display());
    }
    match fs::remove_file(socket) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing stale {}", socket.display()));
        }
        _ => {}
    }
    let listener =
        UnixListener::bind(socket).with_context(|| format!("binding {}", socket.display()))?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

struct Daemon {
    panes: Mutex<BTreeMap<PaneId, Arc<Pane>>>,
    workspaces: Mutex<BTreeMap<String, Ws>>,
    /// Where the trees are saved after every change; `None` keeps no file.
    session: Option<PathBuf>,
    /// Held for a whole save, outside every other lock.
    saving: Mutex<()>,
    /// What the client draws panes on, for the programs that ask (S14).
    theme: Arc<Mutex<Theme>>,
    next_pane: AtomicU64,
    next_client: AtomicU64,
    /// The conversations of the agents a phone watches (mobile REQ-90). Its
    /// lock is taken on its own: never with a pane's or a workspace's.
    chats: Mutex<BTreeMap<PaneId, ChatWatch>>,
    /// Mobile: its own locks and threads, none shared with the panes
    /// (REQ-40).
    mobile: Mobile,
}

/// A transcript followed for the phones that watch it.
struct ChatWatch {
    tail: chat::Tail,
    session: String,
    cwd: PathBuf,
    subs: Vec<Sub>,
}

/// An open workspace: how its panes are laid out, and who watches it.
struct Ws {
    tree: Tree<PaneLeaf>,
    subs: Vec<Sub>,
}

impl Ws {
    /// Sends the tree to every watcher, dropping the ones that went away.
    fn tell(&mut self, stream: &str) {
        let msg = DaemonMsg::Tree {
            stream: stream.to_string(),
            tree: self.tree.clone(),
        };
        self.subs.retain(|s| s.tx.send(msg.clone()).is_ok());
    }
}

struct Pane {
    /// Its workspace; it changes when the workspace's session is renamed.
    stream: Mutex<String>,
    role: String,
    /// How it was started, for `session.json`; its folder changes when
    /// its worktree is renamed.
    started: Mutex<Started>,
    io: Mutex<PaneIo>,
    state: Mutex<PaneState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Started {
    cmd: Option<String>,
    #[serde(default)]
    resume: Option<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}

struct PaneIo {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

struct PaneState {
    parser: vt100::Parser<Titles>,
    subs: Vec<Sub>,
    exited: Option<i32>,
    /// When the pane last printed: an agent that just started is still
    /// drawing, and a prompt typed then is lost (RISK-3).
    last_output: Instant,
    /// The last window title its program set (OSC 0/2).
    title: Option<String>,
    /// It rang since its last input.
    bell: bool,
    /// Its last output, for the scrollback of clients and restarts.
    ring: Ring,
    /// What its agent's hooks reported last.
    agent: Option<AgentState>,
    /// Its screen changed since the last `Text` went to the phones watching it.
    text_dirty: bool,
}

/// Catches what a pane's program says beyond its screen: the window title,
/// the bell or a desktop notification (OSC 9 / 777), which agents use when
/// they wait for an answer, and the questions it asks its terminal.
#[derive(Default)]
struct Titles {
    new: Option<String>,
    rang: bool,
    /// Asked since the reader last answered.
    queries: Vec<Query>,
    /// It set mode 2031: tell it when the theme turns dark or light.
    scheme: bool,
    /// Its program's kitty keyboard flags (REQ-92).
    kitty: Kitty,
}

/// A question a pane's program asks its terminal, answered on its PTY
/// (REQ-81–84, REQ-89).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Query {
    /// OSC 10 ?
    Fg,
    /// OSC 11 ?
    Bg,
    /// CSI ? 996 n
    Scheme,
    /// CSI ? 2031 $ p
    SchemeMode,
    /// CSI 6 n
    Cursor,
    /// CSI c
    Attrs,
    /// CSI ? u, with the flags in use when it was asked (REQ-93)
    Kitty(u8),
}

impl Query {
    fn answer(self, theme: &Theme, scheme: bool, screen: &vt100::Screen) -> Vec<u8> {
        let rgb = |code: u8, [r, g, b]: [u8; 3]| {
            format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\")
        };
        match self {
            Self::Fg => rgb(10, theme.fg),
            Self::Bg => rgb(11, theme.bg),
            Self::Scheme => return scheme_report(theme.dark),
            Self::SchemeMode => format!("\x1b[?2031;{}$y", if scheme { 1 } else { 2 }),
            Self::Cursor => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            Self::Attrs => "\x1b[?1;2c".to_string(),
            Self::Kitty(f) => format!("\x1b[?{f}u"),
        }
        .into_bytes()
    }
}

/// What a terminal says when its colour scheme is or turns dark or light
/// (CSI ? 997 ; 1|2 n).
fn scheme_report(dark: bool) -> Vec<u8> {
    format!("\x1b[?997;{}n", if dark { 1 } else { 2 }).into_bytes()
}

impl vt100::Callbacks for Titles {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.new = Some(String::from_utf8_lossy(title).into_owned());
    }

    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.rang = true;
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"9" | b"777", ..] => self.rang = true,
            // OSC 10 ; ? ; ? asks for the foreground, then the background.
            [code @ (b"10" | b"11"), asks @ ..] => {
                let colours = if *code == b"10" {
                    &[Query::Fg, Query::Bg][..]
                } else {
                    &[Query::Bg][..]
                };
                for (ask, q) in asks.iter().zip(colours) {
                    if *ask == b"?" {
                        self.queries.push(*q);
                    }
                }
            }
            _ => {}
        }
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let alt = screen.alternate_screen();
        if self.kitty.csi(alt, i1, params, c).is_some() {
            self.queries.push(Query::Kitty(self.kitty.flags(alt)));
        }
        let has = |n: u16| params.iter().any(|p| *p == [n]);
        match (i1, i2, c) {
            (Some(b'?'), None, 'n') if has(996) => self.queries.push(Query::Scheme),
            // vt100 passes every parameter of a `?h` it doesn't know.
            (Some(b'?'), None, 'h' | 'l') if has(2031) => self.scheme = c == 'h',
            (Some(b'?'), Some(b'$'), 'p') if has(2031) => self.queries.push(Query::SchemeMode),
            (None, None, 'n') if has(6) => self.queries.push(Query::Cursor),
            (None, None, 'c') if params.is_empty() || has(0) => self.queries.push(Query::Attrs),
            _ => {}
        }
    }
}

/// How recent output must be for a pane to count as busy.
const BUSY: Duration = Duration::from_secs(2);

/// One client's interest in one pane or workspace.
struct Sub {
    client: u64,
    tx: Tx,
}

impl PaneState {
    /// Sends to every subscriber, dropping the ones whose client went away.
    /// `Output` goes to the clients that draw a terminal, `Text` to the ones
    /// that asked for `TextView`.
    fn broadcast(&mut self, msg: &DaemonMsg) {
        let wants_text = match msg {
            DaemonMsg::Output { .. } => Some(false),
            DaemonMsg::Text { .. } => Some(true),
            _ => None,
        };
        self.subs.retain(|s| {
            wants_text.is_some_and(|w| s.tx.text() != w) || s.tx.send(msg.clone()).is_ok()
        });
    }
}

/// Adds `client` to `subs` once.
fn subscribe(subs: &mut Vec<Sub>, client: u64, tx: &Tx) {
    if !subs.iter().any(|s| s.client == client) {
        subs.push(Sub {
            client,
            tx: tx.clone(),
        });
    }
}

/// Every mode vt100 tracks, back to its default: application cursor and
/// keypad, the cursor shown, bracketed paste, mouse tracking and encodings.
const MODES_OFF: &[u8] =
    b"\x1b[?1l\x1b>\x1b[?25h\x1b[?2004l\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l";

/// The pane for a client that starts watching it: its last output, which
/// rebuilds the scrollback (REQ-72), then the exact screen on top.
fn snapshot(id: PaneId, pane: &Pane, st: &PaneState) -> DaemonMsg {
    let screen = st.parser.screen();
    let (rows, cols) = screen.size();
    let mut bytes = st.ring.bytes();
    bytes.extend_from_slice(&screen_bytes(st));
    DaemonMsg::Snapshot {
        pane: id,
        role: pane.role.clone(),
        cols,
        rows,
        bytes,
    }
}

/// The exact screen, the end of every snapshot: the mode, the cells and the
/// kitty keyboard stack. Alone, it draws the pane without its scrollback.
fn screen_bytes(st: &PaneState) -> Vec<u8> {
    let screen = st.parser.screen();
    let mut bytes = b"\x1b[0m".to_vec();
    bytes.extend_from_slice(if screen.alternate_screen() {
        b"\x1b[?1049h"
    } else {
        b"\x1b[?1049l"
    });
    bytes.extend_from_slice(&screen.state_formatted());
    bytes.extend_from_slice(
        &st.parser
            .callbacks()
            .kitty
            .replay(screen.alternate_screen()),
    );
    bytes
}

/// The pane's screen as the styled rows a `TextView` client draws.
fn text_screen(id: PaneId, st: &PaneState) -> DaemonMsg {
    let screen = st.parser.screen();
    let (rows, cols) = screen.size();
    DaemonMsg::Text {
        pane: id,
        cols,
        rows,
        alt: screen.alternate_screen(),
        app_cursor: screen.application_cursor(),
        cursor: (!screen.hide_cursor()).then(|| screen.cursor_position()),
        lines: (0..rows).map(|r| runs_of(screen, r, cols)).collect(),
    }
}

/// The last `count` rows of the scrollback, oldest first. The screen is moved
/// up through the history a page at a time and put back.
fn history_rows(parser: &mut vt100::Parser<Titles>, count: usize) -> Vec<Vec<Run>> {
    let screen = parser.screen_mut();
    screen.set_scrollback(usize::MAX);
    let total = screen.scrollback();
    let (rows, cols) = screen.size();
    let mut at = total.min(count);
    let mut out = Vec::with_capacity(at);
    while at > 0 {
        screen.set_scrollback(at);
        let take = at.min(rows as usize);
        for r in 0..take {
            out.push(runs_of(screen, r as u16, cols));
        }
        at -= take;
    }
    screen.set_scrollback(0);
    out
}

/// One colour in the wire's encoding (see `proto::Run`).
fn color_code(c: vt100::Color) -> u32 {
    match c {
        vt100::Color::Default => 0,
        vt100::Color::Idx(i) => 1 << 24 | u32::from(i),
        vt100::Color::Rgb(r, g, b) => {
            2 << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
        }
    }
}

/// Row `row` of the visible screen as runs of one style, its blank end left off.
fn runs_of(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let attrs = u8::from(cell.bold())
            | u8::from(cell.dim()) << 1
            | u8::from(cell.italic()) << 2
            | u8::from(cell.underline()) << 3
            | u8::from(cell.inverse()) << 4;
        let (fg, bg) = (color_code(cell.fgcolor()), color_code(cell.bgcolor()));
        let text = if cell.has_contents() {
            cell.contents()
        } else {
            " "
        };
        match runs.last_mut() {
            Some(r) if (r.2, r.3, r.4) == (fg, bg, attrs) => r.1.push_str(text),
            _ => runs.push((col, text.to_string(), fg, bg, attrs)),
        }
    }
    // A blank end that draws nothing (no background, underline or inverse).
    while let Some(last) = runs.last_mut() {
        if last.3 != 0 || last.4 & 0b1_1000 != 0 {
            break;
        }
        let kept = last.1.trim_end_matches(' ').len();
        last.1.truncate(kept);
        if !last.1.is_empty() {
            break;
        }
        runs.pop();
    }
    runs
}

/// A snapshot in two: the `Screen` to draw at once and the `History` to
/// replay behind it (`SplitSnapshots`).
fn split_snapshot(id: PaneId, pane: &Pane, st: &PaneState) -> [DaemonMsg; 2] {
    let (rows, cols) = st.parser.screen().size();
    let screen = screen_bytes(st);
    let mut history = st.ring.bytes();
    history.extend_from_slice(&screen);
    [
        DaemonMsg::Screen {
            pane: id,
            role: pane.role.clone(),
            cols,
            rows,
            bytes: screen,
        },
        DaemonMsg::History {
            pane: id,
            cols,
            rows,
            bytes: history,
        },
    ]
}

/// The local time as `HH:MM`.
fn clock() -> String {
    // SAFETY: time and localtime_r only write into the values given.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking pane thread must not take the whole daemon down with it.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What `session.json` holds: every open workspace, enough to start it again.
#[derive(Serialize, Deserialize)]
struct Session {
    workspaces: Vec<SavedWs>,
    /// The last theme a client reported (REQ-86).
    #[serde(default)]
    theme: Option<Theme>,
}

#[derive(Serialize, Deserialize)]
struct SavedWs {
    id: String,
    tree: Tree<SavedPane>,
}

#[derive(Serialize, Deserialize)]
struct SavedPane {
    /// The pane's id when it was saved: its scrollback file.
    #[serde(default)]
    id: Option<PaneId>,
    role: String,
    name: Option<String>,
    /// How its process started; none for a viewer (`view:…`).
    #[serde(flatten)]
    started: Option<Started>,
}

impl Daemon {
    fn new(session: Option<PathBuf>, mobile: Mobile) -> Self {
        Self {
            panes: Mutex::default(),
            workspaces: Mutex::default(),
            session,
            saving: Mutex::default(),
            theme: Arc::default(),
            next_pane: AtomicU64::new(0),
            next_client: AtomicU64::new(0),
            chats: Mutex::default(),
            mobile,
        }
    }

    fn serve(self: Arc<Self>, stream: UnixStream) {
        let client = self.next_client.fetch_add(1, Ordering::Relaxed);
        let tx = Tx::new();

        let mut out = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => return eprintln!("jw daemon: client {client}: {e}"),
        };
        let (me, rx) = (Arc::clone(&self), tx.clone());
        thread::spawn(move || {
            while let Some(msg) = rx.recv() {
                if write_frame(&mut out, &msg).is_err() {
                    break;
                }
                for pane in rx.stale() {
                    me.resync(&rx, pane);
                }
            }
            rx.close();
            let _ = out.shutdown(std::net::Shutdown::Both);
        });

        let mut input = stream;
        loop {
            let msg = match read_frame::<ClientMsg>(&mut input) {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(DaemonMsg::Error { msg: e.to_string() });
                    break;
                }
            };
            if let Err(e) = self.handle(client, &tx, msg) {
                let _ = tx.send(DaemonMsg::Error {
                    msg: format!("{e:#}"),
                });
            }
        }
        // REQ-4: a client leaving only drops its subscriptions.
        tx.close();
        self.unsubscribe(client);
        self.mobile.forget(client);
    }

    fn handle(&self, client: u64, tx: &Tx, msg: ClientMsg) -> Result<()> {
        match msg {
            ClientMsg::Hello { .. } => {
                let _ = tx.send(DaemonMsg::Hello { protocol: PROTOCOL });
            }
            ClientMsg::SplitSnapshots => tx.set_split(),
            ClientMsg::TextView => tx.set_text(),
            ClientMsg::ChatWatch { pane } => self.chat_watch(client, tx, pane)?,
            ClientMsg::ChatUnwatch { pane } => {
                if let Some(w) = lock(&self.chats).get_mut(&pane) {
                    w.subs.retain(|s| s.client != client);
                }
            }
            ClientMsg::TextHistory { pane, count } => {
                let pane_id = pane;
                let pane = self.pane(pane)?;
                let mut st = lock(&pane.state);
                let lines = history_rows(&mut st.parser, (count as usize).min(SCROLLBACK));
                let _ = tx.send(DaemonMsg::TextHistory {
                    pane: pane_id,
                    lines,
                });
            }
            ClientMsg::MobileWatch => self.mobile.watch(client, tx),
            ClientMsg::MobileStart { read_only } => self.mobile.start(read_only, tx),
            ClientMsg::MobileStop => self.mobile.stop(Duration::ZERO),
            ClientMsg::MobileCode => self.mobile.code()?,
            ClientMsg::MobileReadOnly { on } => self.mobile.read_only(on),
            ClientMsg::MobileAnswer { request, yes } => self.mobile.answer(client, request, yes),
            ClientMsg::MobileRevoke { device } => self.mobile.revoke(&device)?,
            ClientMsg::Attach { stream } => self.attach(client, tx, &stream),
            ClientMsg::Detach => self.unsubscribe(client),
            ClientMsg::Prompt { stream, text, pane } => {
                let panes = self.panes_of(&stream);
                let tx = tx.clone();
                // Waiting can take seconds, and finding an agent by program
                // runs `ps`: other messages from this client must not queue
                // behind either.
                thread::spawn(move || {
                    let sent = agent_pane(&stream, panes, pane)
                        .and_then(|(id, pane)| prompt(&pane, &text).map(|()| id));
                    let msg = match sent {
                        Ok(id) => DaemonMsg::Prompted { pane: id },
                        Err(e) => DaemonMsg::Error {
                            msg: format!("{e:#}"),
                        },
                    };
                    let _ = tx.send(msg);
                });
            }
            ClientMsg::List => {
                // `ps` runs per pane: not with every other client waiting
                // on the panes' lock.
                let all: Vec<(PaneId, Arc<Pane>)> = lock(&self.panes)
                    .iter()
                    .map(|(id, p)| (*id, Arc::clone(p)))
                    .collect();
                let panes = all
                    .iter()
                    .map(|(id, p)| {
                        let (exited, busy, bell, agent) = {
                            let st = lock(&p.state);
                            let agent = st.agent.filter(|_| st.exited.is_none());
                            (st.exited, st.last_output.elapsed() < BUSY, st.bell, agent)
                        };
                        let leader = match exited {
                            None => leader_of(p),
                            Some(_) => None,
                        };
                        let fg = leader.and_then(process_name);
                        let cwd = leader.and_then(process_cwd);
                        PaneInfo {
                            pane: *id,
                            stream: lock(&p.stream).clone(),
                            role: p.role.clone(),
                            exited,
                            fg,
                            busy: busy && exited.is_none(),
                            bell,
                            agent,
                            cwd,
                        }
                    })
                    .collect();
                let _ = tx.send(DaemonMsg::Panes { panes });
            }
            ClientMsg::Rekey { from, to } => self.rekey(&from, &to)?,
            ClientMsg::Moved {
                stream,
                from,
                to,
                env,
            } => self.moved(&stream, &from, &to, &env),
            ClientMsg::Agent { pane, state } => {
                lock(&self.pane(pane)?.state).agent = Some(state);
            }
            ClientMsg::Theme(theme) => self.set_theme(theme),
            ClientMsg::Input { pane, bytes } => {
                let pane = self.pane(pane)?;
                {
                    let mut st = lock(&pane.state);
                    st.bell = false;
                    // The person answered what the agent waits for (from the TUI or a
                    // phone): say so now, the agent's own hooks correct it later.
                    if st.agent == Some(AgentState::Waiting) {
                        st.agent = Some(answered(&bytes));
                    }
                }
                let mut io = lock(&pane.io);
                io.writer.write_all(&bytes)?;
                io.writer.flush()?;
            }
            ClientMsg::Resize {
                pane: id,
                cols,
                rows,
            } => {
                let pane = self.pane(id)?;
                lock(&pane.io).master.resize(size(cols, rows))?;
                let mut st = lock(&pane.state);
                if st.parser.screen().size() != (rows, cols) {
                    st.parser.screen_mut().set_size(rows, cols);
                    st.text_dirty = true;
                    st.broadcast(&DaemonMsg::Resized {
                        pane: id,
                        cols,
                        rows,
                    });
                }
            }
            ClientMsg::Spawn {
                stream,
                role,
                cmd,
                cwd,
                env,
                cols,
                rows,
            } => {
                let new = NewPane {
                    role,
                    cmd,
                    resume: None,
                    cwd,
                    env,
                    cols,
                    rows,
                };
                // spawn says Spawned itself, before the pane's first byte.
                let id = self.spawn(&stream, new, Some((client, tx)), None)?;
                self.place(&stream, id);
            }
            ClientMsg::Open { stream, tree } => {
                if !lock(&self.workspaces).contains_key(&stream) {
                    self.open(&stream, tree)?;
                }
                self.attach(client, tx, &stream);
            }
            ClientMsg::Split { pane, dir, new } => {
                let pane = self.split(pane, dir, new)?;
                let _ = tx.send(DaemonMsg::Opened { pane });
            }
            ClientMsg::Dock { stream, share, new } => self.dock(&stream, share, new)?,
            ClientMsg::Kill { pane } => self.kill(pane)?,
            ClientMsg::Close { stream } => {
                let ids: Vec<PaneId> = self.panes_of(&stream).into_iter().map(|p| p.0).collect();
                lock(&self.workspaces).remove(&stream);
                for id in ids {
                    self.stop(id);
                }
                self.save();
            }
            ClientMsg::Ratio {
                stream,
                path,
                ratio,
            } => {
                self.edit(&stream, |t| {
                    if !t.set_ratio(&path, ratio) {
                        bail!("no split there in {stream}");
                    }
                    Ok(())
                })?;
            }
            ClientMsg::Swap { a, b } => {
                let stream = self.stream_of(a)?;
                self.edit(&stream, |t| {
                    if !t.swap(a, b) {
                        bail!("panes {a} and {b} are not both in {stream}");
                    }
                    Ok(())
                })?;
            }
            ClientMsg::Name { pane, name } => {
                let stream = self.stream_of(pane)?;
                let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                self.edit(&stream, |t| {
                    t.find_mut(pane).context("no such pane")?.name = name;
                    Ok(())
                })?;
            }
            ClientMsg::Role { pane, role } => {
                let stream = self.stream_of(pane)?;
                self.edit(&stream, |t| {
                    let leaf = t.find_mut(pane).context("no such pane")?;
                    if !is_view(&leaf.role) || !is_view(&role) {
                        bail!("only a viewer changes what it shows");
                    }
                    leaf.role = role;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    /// Subscribes `client` to a workspace: its tree, then each pane's screen.
    fn attach(&self, client: u64, tx: &Tx, stream: &str) {
        let ids: Vec<PaneId> = {
            let mut all = lock(&self.workspaces);
            match all.get_mut(stream) {
                Some(ws) => {
                    let _ = tx.send(DaemonMsg::Tree {
                        stream: stream.to_string(),
                        tree: ws.tree.clone(),
                    });
                    subscribe(&mut ws.subs, client, tx);
                    ws.tree.leaves().into_iter().map(|l| l.id).collect()
                }
                None => Vec::new(),
            }
        };
        for id in ids {
            self.watch(client, tx, id);
        }
    }

    /// The pane's screen (and status and title) for `client`, then its live
    /// output: under the pane's lock, so nothing falls between.
    fn watch(&self, client: u64, tx: &Tx, id: PaneId) {
        let Ok(pane) = self.pane(id) else { return };
        let mut st = lock(&pane.state);
        if tx.text() {
            let _ = tx.send(text_screen(id, &st));
        } else if tx.split() {
            for m in split_snapshot(id, &pane, &st) {
                let _ = tx.send(m);
            }
        } else {
            let _ = tx.send(snapshot(id, &pane, &st));
        }
        if let Some(status) = st.exited {
            let _ = tx.send(DaemonMsg::Exited { pane: id, status });
        }
        if let Some(title) = st.title.clone() {
            let _ = tx.send(DaemonMsg::Title { pane: id, title });
        }
        subscribe(&mut st.subs, client, tx);
    }

    /// A workspace's new id, when its session was renamed: its panes and
    /// tree stay, under the new id.
    fn rekey(&self, from: &str, to: &str) -> Result<()> {
        {
            let mut all = lock(&self.workspaces);
            if all.contains_key(to) {
                bail!("{to} is already open");
            }
            let Some(ws) = all.remove(from) else {
                return Ok(());
            };
            for (_, pane) in self.panes_of(from) {
                *lock(&pane.stream) = to.to_string();
            }
            all.insert(to.to_string(), ws);
        }
        self.save();
        self.tell(to);
        Ok(())
    }

    /// Catches a client up on a pane whose output it fell behind on.
    fn resync(&self, tx: &Tx, id: PaneId) {
        match self.pane(id) {
            Ok(pane) => {
                let st = lock(&pane.state);
                tx.resync(id, snapshot(id, &pane, &st));
            }
            // Gone meanwhile: nothing to catch up on.
            Err(_) => tx.forget(id),
        }
    }

    /// Starts every pane of a new workspace; none is left behind on failure.
    /// A viewer leaf (`view:*`) only gets an id (REQ-181).
    fn open(&self, stream: &str, tree: Tree<NewPane>) -> Result<()> {
        let mut started = Vec::new();
        let tree = tree.try_map(&mut |new: NewPane| {
            let leaf = self.start(stream, new)?;
            started.push(leaf.id);
            Ok::<_, anyhow::Error>(leaf)
        });
        let tree = match tree {
            Ok(t) => t,
            Err(e) => {
                for id in started {
                    self.stop(id);
                }
                return Err(e);
            }
        };
        lock(&self.workspaces).insert(
            stream.to_string(),
            Ws {
                tree,
                subs: Vec::new(),
            },
        );
        self.save();
        Ok(())
    }

    /// A pane from the old `Spawn`: the first of its workspace, or one more
    /// on the right of the last.
    fn place(&self, stream: &str, id: PaneId) {
        let Ok(pane) = self.pane(id) else { return };
        let leaf = PaneLeaf {
            id,
            role: pane.role.clone(),
            name: None,
        };
        {
            let mut all = lock(&self.workspaces);
            match all.remove(stream) {
                None => {
                    all.insert(
                        stream.to_string(),
                        Ws {
                            tree: Tree::Leaf(leaf),
                            subs: Vec::new(),
                        },
                    );
                }
                Some(Ws { tree, subs }) => {
                    let last = tree.leaves().last().map_or(0, |l| l.id);
                    let (tree, _) = tree.insert(last, Dir::Right, leaf);
                    all.insert(stream.to_string(), Ws { tree, subs });
                }
            }
        }
        self.save();
        self.tell(stream);
    }

    fn split(&self, beside: PaneId, dir: Dir, mut new: NewPane) -> Result<PaneId> {
        let stream = self.stream_of(beside)?;
        // A client that cannot work out the stream's `JW_*` variables (a phone) sends none: the new pane
        // takes its neighbour's, which are the stream's, so `jw ls` and the hooks work in it.
        if new.env.is_empty() {
            let neighbour = lock(&self.panes).get(&beside).cloned();
            if let Some(p) = neighbour {
                new.env = lock(&p.started).env.clone();
            }
        }
        let leaf = self.start(&stream, new)?;
        self.add(&stream, leaf, |tree, leaf| {
            let (tree, missing) = tree.insert(beside, dir, leaf);
            let placed = match missing {
                Some(_) => Err(anyhow!("pane {beside} is not in {stream}")),
                None => Ok(()),
            };
            (tree, placed)
        })
    }

    /// A pane along the right edge of the whole workspace.
    fn dock(&self, stream: &str, share: f32, new: NewPane) -> Result<()> {
        let leaf = self.start(stream, new)?;
        self.add(stream, leaf, |tree, leaf| (tree.dock(leaf, share), Ok(())))
            .map(|_| ())
    }

    /// A new leaf for `stream`: a process, or just an id for a viewer.
    fn start(&self, stream: &str, new: NewPane) -> Result<PaneLeaf> {
        let role = new.role.clone();
        let id = if is_view(&role) {
            self.new_id()
        } else {
            self.spawn(stream, new, None, None)?
        };
        Ok(PaneLeaf {
            id,
            role,
            name: None,
        })
    }

    /// Puts a new pane into `stream`'s tree with `place`, saves, then shows
    /// it to the workspace's watchers. A pane `place` refuses is stopped.
    fn add(
        &self,
        stream: &str,
        leaf: PaneLeaf,
        place: impl FnOnce(Tree<PaneLeaf>, PaneLeaf) -> (Tree<PaneLeaf>, Result<()>),
    ) -> Result<PaneId> {
        let id = leaf.id;
        let placed = {
            let mut all = lock(&self.workspaces);
            match all.remove(stream) {
                Some(Ws { tree, subs }) => {
                    let (tree, placed) = place(tree, leaf);
                    all.insert(stream.to_string(), Ws { tree, subs });
                    placed
                }
                None => Err(anyhow!("{stream} is not open")),
            }
        };
        if let Err(e) = placed {
            self.stop(id);
            return Err(e);
        }
        self.save();
        let mut all = lock(&self.workspaces);
        let Some(ws) = all.get_mut(stream) else {
            return Ok(id);
        };
        ws.tell(stream);
        let watchers: Vec<(u64, Tx)> = ws.subs.iter().map(|s| (s.client, s.tx.clone())).collect();
        for (client, tx) in watchers {
            self.watch(client, &tx, id);
        }
        Ok(id)
    }

    /// Stops a pane and takes it out of its tree; the last one closes the
    /// workspace.
    fn kill(&self, id: PaneId) -> Result<()> {
        let stream = self.stream_of(id)?;
        self.stop(id);
        {
            let mut all = lock(&self.workspaces);
            if let Some(Ws { tree, subs }) = all.remove(&stream)
                && let (Some(tree), _) = tree.remove(id)
            {
                all.insert(stream.clone(), Ws { tree, subs });
            }
        }
        self.save();
        self.tell(&stream);
        Ok(())
    }

    /// Changes a workspace's tree in place, saves, then tells its watchers:
    /// a change a client has seen is already on disk.
    fn edit(&self, stream: &str, f: impl FnOnce(&mut Tree<PaneLeaf>) -> Result<()>) -> Result<()> {
        {
            let mut all = lock(&self.workspaces);
            let ws = all
                .get_mut(stream)
                .with_context(|| format!("{stream} is not open"))?;
            f(&mut ws.tree)?;
        }
        self.save();
        self.tell(stream);
        Ok(())
    }

    /// Sends `stream`'s tree to its watchers, if it is still open.
    fn tell(&self, stream: &str) {
        if let Some(ws) = lock(&self.workspaces).get_mut(stream) {
            ws.tell(stream);
        }
    }

    /// Kills the pane's process (if still running) and forgets the pane.
    fn stop(&self, id: PaneId) {
        let Some(pane) = lock(&self.panes).remove(&id) else {
            return;
        };
        if lock(&pane.state).exited.is_none() {
            let _ = lock(&pane.io).killer.kill();
        }
    }

    /// Starts again the workspaces a previous daemon left in
    /// `session.json` (REQ-15): same trees and names, agents with their
    /// resume command. A workspace that can't start (its folder is gone) is
    /// dropped with a line in the log.
    fn restore(&self) {
        let Some(path) = &self.session else { return };
        let Ok(data) = fs::read(path) else { return };
        let session: Session = match serde_json::from_slice(&data) {
            Ok(s) => s,
            Err(e) => return eprintln!("jw daemon: {}: {e}", path.display()),
        };
        if let Some(theme) = session.theme {
            *lock(&self.theme) = theme;
        }
        for ws in session.workspaces {
            let tree = ws.tree.try_map(&mut |p: SavedPane| {
                let id = match p.started {
                    None => self.new_id(),
                    Some(s) => {
                        if !s.cwd.is_dir() {
                            bail!("{} is gone", s.cwd.display());
                        }
                        let new = NewPane {
                            role: p.role.clone(),
                            cmd: s.resume.clone().or(s.cmd),
                            resume: s.resume,
                            cwd: s.cwd,
                            env: s.env,
                            cols: 80,
                            rows: 24,
                        };
                        let history = p.id.and_then(|old| fs::read(self.scrollback(old)?).ok());
                        self.spawn(&ws.id, new, None, history)?
                    }
                };
                Ok(PaneLeaf {
                    id,
                    role: p.role,
                    name: p.name,
                })
            });
            match tree {
                Ok(tree) => {
                    lock(&self.workspaces).insert(
                        ws.id,
                        Ws {
                            tree,
                            subs: Vec::new(),
                        },
                    );
                }
                Err(e) => {
                    for (id, _) in self.panes_of(&ws.id) {
                        self.stop(id);
                    }
                    eprintln!("jw daemon: not restoring {}: {e:#}", ws.id);
                }
            }
        }
        self.save();
    }

    /// Writes `session.json` (temp file + rename) and the scrollback. A
    /// failure is logged, not fatal: the panes matter more than the file.
    fn save(&self) {
        if let Err(e) = self.try_save() {
            eprintln!("jw daemon: {e:#}");
        }
    }

    /// `save`, with the error. One save at a time: they share the temp
    /// files, and a slower one would rename an older tree over a newer.
    fn try_save(&self) -> Result<()> {
        let _saving = lock(&self.saving);
        let Some(path) = &self.session else {
            return Ok(());
        };
        let session = {
            let all = lock(&self.workspaces);
            let panes = lock(&self.panes);
            Session {
                workspaces: all
                    .iter()
                    .filter_map(|(id, ws)| {
                        let tree = ws
                            .tree
                            .clone()
                            .try_map(&mut |l: PaneLeaf| {
                                let started = match panes.get(&l.id) {
                                    Some(p) => Some(lock(&p.started).clone()),
                                    None if is_view(&l.role) => None,
                                    None => return Err(()),
                                };
                                Ok::<_, ()>(SavedPane {
                                    id: Some(l.id),
                                    role: l.role,
                                    name: l.name,
                                    started,
                                })
                            })
                            .ok()?;
                        Some(SavedWs {
                            id: id.clone(),
                            tree,
                        })
                    })
                    .collect(),
                theme: Some(*lock(&self.theme)),
            }
        };
        let write = || -> Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&session)?)?;
            fs::rename(&tmp, path)?;
            Ok(())
        };
        let written = write().with_context(|| format!("saving {}", path.display()));
        self.write_scrollback();
        written
    }

    /// Where a pane's output is saved: `scrollback/<id>.bin` next to
    /// `session.json`.
    fn scrollback(&self, id: PaneId) -> Option<PathBuf> {
        Some(
            self.session
                .as_ref()?
                .with_file_name("scrollback")
                .join(format!("{id}.bin")),
        )
    }

    /// The timer's save: only the output.
    fn save_scrollback(&self) {
        let _saving = lock(&self.saving);
        self.write_scrollback();
    }

    /// Writes the output of every pane that printed since the last time, and
    /// drops the files of panes that are gone.
    fn write_scrollback(&self) {
        let Some(dir) = self
            .scrollback(0)
            .and_then(|p| p.parent().map(Path::to_path_buf))
        else {
            return;
        };
        let panes: Vec<(PaneId, Arc<Pane>)> = lock(&self.panes)
            .iter()
            .map(|(id, p)| (*id, Arc::clone(p)))
            .collect();
        let mut keep = std::collections::BTreeSet::new();
        for (id, pane) in panes {
            let Some(path) = self.scrollback(id) else {
                continue;
            };
            keep.insert(path.clone());
            let bytes = {
                let mut st = lock(&pane.state);
                if !st.ring.dirty {
                    continue;
                }
                st.ring.dirty = false;
                st.ring.bytes()
            };
            let write = || -> Result<()> {
                fs::create_dir_all(&dir)?;
                let tmp = path.with_extension("bin.tmp");
                fs::write(&tmp, &bytes)?;
                fs::rename(&tmp, &path)?;
                Ok(())
            };
            if let Err(e) = write() {
                eprintln!("jw daemon: saving {}: {e:#}", path.display());
            }
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for f in rd.flatten() {
                if !keep.contains(&f.path()) {
                    let _ = fs::remove_file(f.path());
                }
            }
        }
    }

    /// The workspace a pane or a viewer belongs to.
    fn stream_of(&self, id: PaneId) -> Result<String> {
        if let Ok(p) = self.pane(id) {
            return Ok(lock(&p.stream).clone());
        }
        lock(&self.workspaces)
            .iter()
            .find(|(_, ws)| ws.tree.find(id).is_some())
            .map(|(s, _)| s.clone())
            .with_context(|| format!("no pane {id}"))
    }

    /// Keeps what the client draws on, and tells each program that asked
    /// for it (mode 2031) when it turns dark or light (REQ-83).
    fn set_theme(&self, theme: Theme) {
        let old = std::mem::replace(&mut *lock(&self.theme), theme);
        if old == theme {
            return;
        }
        self.save();
        if old.dark == theme.dark {
            return;
        }
        let report = scheme_report(theme.dark);
        let panes: Vec<_> = lock(&self.panes).values().cloned().collect();
        for pane in panes {
            let wants = {
                let st = lock(&pane.state);
                st.exited.is_none() && st.parser.callbacks().scheme
            };
            if wants {
                let mut io = lock(&pane.io);
                let _ = io
                    .writer
                    .write_all(&report)
                    .and_then(|()| io.writer.flush());
            }
        }
    }

    /// Ids start at 1 and are never reused, for viewers as for processes.
    fn new_id(&self) -> PaneId {
        self.next_pane.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Starts `client` on the conversation of the agent in `id`: all it has now,
    /// then each change (`chat_poll`). A pane that runs no Claude session is
    /// told so once.
    fn chat_watch(&self, client: u64, tx: &Tx, id: PaneId) -> Result<()> {
        let pane = self.pane(id)?;
        let (session, cwd) = {
            let started = lock(&pane.started);
            let session = [started.cmd.as_deref(), started.resume.as_deref()]
                .into_iter()
                .flatten()
                .find_map(chat::session_id);
            (session, started.cwd.clone())
        };
        let Some(session) = session else {
            let _ = tx.send(DaemonMsg::Chat {
                pane: id,
                known: false,
                title: None,
                reset: true,
                blocks: Vec::new(),
            });
            return Ok(());
        };
        let mut chats = lock(&self.chats);
        let w = chats.entry(id).or_insert_with(|| ChatWatch {
            tail: chat::Tail::new(None),
            session: session.clone(),
            cwd,
            subs: Vec::new(),
        });
        w.tail.locate(&w.session, &w.cwd);
        // Whoever already watches gets what is new before this client is
        // brought up to date.
        let news = w.tail.poll();
        if news != chat::Changes::default() {
            let msg = DaemonMsg::Chat {
                pane: id,
                known: true,
                title: news.title,
                reset: false,
                blocks: news.blocks,
            };
            w.subs.retain(|s| s.tx.send(msg.clone()).is_ok());
        }
        let _ = tx.send(DaemonMsg::Chat {
            pane: id,
            known: true,
            title: w.tail.title().map(str::to_string),
            reset: true,
            blocks: w.tail.last(chat::FIRST),
        });
        subscribe(&mut w.subs, client, tx);
        Ok(())
    }

    /// Reads what the followed transcripts gained and tells their watchers;
    /// one nobody watches any more is let go.
    fn chat_poll(&self) {
        let mut chats = lock(&self.chats);
        chats.retain(|id, w| {
            w.tail.locate(&w.session, &w.cwd);
            let news = w.tail.poll();
            if news != chat::Changes::default() {
                let msg = DaemonMsg::Chat {
                    pane: *id,
                    known: true,
                    title: news.title,
                    reset: false,
                    blocks: news.blocks,
                };
                w.subs.retain(|s| s.tx.send(msg.clone()).is_ok());
            }
            !w.subs.is_empty()
        });
    }

    /// Sends each pane's screen to the clients that watch it as text, when it
    /// changed since the last time. Every pane is looked at, one lock at a time.
    fn flush_text(&self) {
        let panes: Vec<(PaneId, Arc<Pane>)> = lock(&self.panes)
            .iter()
            .map(|(id, p)| (*id, Arc::clone(p)))
            .collect();
        for (id, pane) in panes {
            let mut st = lock(&pane.state);
            if !st.text_dirty || !st.subs.iter().any(|s| s.tx.text()) {
                st.text_dirty = false;
                continue;
            }
            st.text_dirty = false;
            let msg = text_screen(id, &st);
            st.broadcast(&msg);
        }
    }

    fn pane(&self, id: PaneId) -> Result<Arc<Pane>> {
        lock(&self.panes)
            .get(&id)
            .cloned()
            .with_context(|| format!("no pane {id}"))
    }

    fn panes_of(&self, stream: &str) -> Vec<(PaneId, Arc<Pane>)> {
        lock(&self.panes)
            .iter()
            .filter(|(_, p)| *lock(&p.stream) == stream)
            .map(|(id, p)| (*id, Arc::clone(p)))
            .collect()
    }

    fn unsubscribe(&self, client: u64) {
        for w in lock(&self.chats).values_mut() {
            w.subs.retain(|s| s.client != client);
        }
        for ws in lock(&self.workspaces).values_mut() {
            ws.subs.retain(|s| s.client != client);
        }
        let panes: Vec<_> = lock(&self.panes).values().cloned().collect();
        for pane in panes {
            lock(&pane.state).subs.retain(|s| s.client != client);
        }
    }

    /// A workspace's folder moved under its running panes: a restart
    /// starts them where it is now, with its new variables.
    fn moved(&self, stream: &str, from: &Path, to: &Path, env: &BTreeMap<String, String>) {
        for (_, pane) in self.panes_of(stream) {
            let mut started = lock(&pane.started);
            if let Ok(rest) = started.cwd.strip_prefix(from) {
                started.cwd = if rest.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(rest)
                };
            }
            started
                .env
                .extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.save();
    }

    /// Starts a pane's process. `sub` is subscribed before its first byte
    /// can arrive; without one, watchers get a Snapshot later.
    fn spawn(
        &self,
        stream: &str,
        new: NewPane,
        sub: Option<(u64, &Tx)>,
        history: Option<Vec<u8>>,
    ) -> Result<PaneId> {
        let id = self.new_id();
        let NewPane {
            role,
            cmd,
            resume,
            cwd,
            env,
            cols,
            rows,
        } = new;
        let pair = native_pty_system().openpty(size(cols, rows))?;

        let mut builder = match &cmd {
            Some(cmd) => {
                let mut b = CommandBuilder::new("sh");
                b.args(["-c", cmd]);
                b
            }
            None => CommandBuilder::new_default_prog(),
        };
        builder.cwd(&cwd);
        builder.env("TERM", "xterm-256color");
        // REQ-88: what programs that only read the environment go by.
        let dark = lock(&self.theme).dark;
        builder.env("COLORFGBG", if dark { "15;0" } else { "0;15" });
        for (k, v) in &env {
            builder.env(k, v);
        }
        builder.env("JW_PANE_ID", id.to_string());

        let mut child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("starting {role} in {}", cwd.display()))?;
        // Only the child may hold the slave, or the reader never sees EOF.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let pane = Arc::new(Pane {
            stream: Mutex::new(stream.to_string()),
            role,
            started: Mutex::new(Started {
                cmd,
                resume,
                cwd,
                env,
            }),
            io: Mutex::new(PaneIo {
                writer: pair.master.take_writer()?,
                killer: child.clone_killer(),
                master: pair.master,
            }),
            state: Mutex::new(PaneState {
                parser: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    SCROLLBACK,
                    Titles::default(),
                ),
                subs: Vec::new(),
                exited: None,
                last_output: Instant::now(),
                title: None,
                bell: false,
                ring: Ring::default(),
                agent: None,
                text_dirty: false,
            }),
        });
        if let Some(mut bytes) = history {
            // REQ-72: what the pane showed before the restart, then a line.
            bytes.extend_from_slice(
                format!(
                    "\x1b[?1049l\x1b[0m\r\n\x1b[2m── restored {} ──\x1b[0m\r\n",
                    clock()
                )
                .as_bytes(),
            );
            // REQ-98: the new process asked for no keyboard flags yet.
            bytes.extend_from_slice(kitty::CLEAR);
            // REQ-114: nor for the old one's mouse, paste and key modes.
            bytes.extend_from_slice(MODES_OFF);
            let mut st = lock(&pane.state);
            st.parser.process(&bytes);
            st.ring.push(&bytes);
            // REQ-87: the old process asked those, not this one.
            *st.parser.callbacks_mut() = Titles::default();
        }
        if let Some((client, tx)) = sub {
            // Its reader hasn't started: Spawned goes before any output.
            let _ = tx.send(DaemonMsg::Spawned { pane: id });
            subscribe(&mut lock(&pane.state).subs, client, tx);
        }
        lock(&self.panes).insert(id, Arc::clone(&pane));

        let theme = Arc::clone(&self.theme);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut st = lock(&pane.state);
                        st.parser.process(&buf[..n]);
                        st.ring.push(&buf[..n]);
                        st.last_output = Instant::now();
                        st.text_dirty = true;
                        st.broadcast(&DaemonMsg::Output {
                            pane: id,
                            bytes: buf[..n].to_vec(),
                        });
                        if std::mem::take(&mut st.parser.callbacks_mut().rang) {
                            st.bell = true;
                        }
                        if let Some(title) = st.parser.callbacks_mut().new.take() {
                            st.title = Some(title.clone());
                            st.broadcast(&DaemonMsg::Title { pane: id, title });
                        }
                        let queries = std::mem::take(&mut st.parser.callbacks_mut().queries);
                        if queries.is_empty() {
                            continue;
                        }
                        let theme = *lock(&theme);
                        let scheme = st.parser.callbacks().scheme;
                        let reply: Vec<u8> = queries
                            .iter()
                            .flat_map(|q| q.answer(&theme, scheme, st.parser.screen()))
                            .collect();
                        // RISK-27: never hold the state while writing.
                        drop(st);
                        let mut io = lock(&pane.io);
                        let _ = io.writer.write_all(&reply).and_then(|()| io.writer.flush());
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    // Linux reports the child closing the slave as EIO.
                    Err(_) => break,
                }
            }
            // REQ-14: keep the screen, record the status, tell whoever watches.
            let status = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
            let mut st = lock(&pane.state);
            st.exited = Some(status);
            st.broadcast(&DaemonMsg::Exited { pane: id, status });
        });
        Ok(id)
    }
}

/// What an agent that was `Waiting` is after `bytes` reach it: an Esc alone
/// refuses, so its turn is over; a key or text answers, so it goes on; a
/// terminal's own report (focus, mouse, cursor: they start `ESC [`, `ESC ]`
/// or `ESC O`) answers nothing.
fn answered(bytes: &[u8]) -> AgentState {
    match bytes {
        [] | [0x1b, b'[' | b']' | b'O', ..] => AgentState::Waiting,
        [0x1b] => AgentState::Idle,
        _ => AgentState::Working,
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The process group in the foreground of a pane's terminal.
fn leader_of(pane: &Pane) -> Option<libc::pid_t> {
    lock(&pane.io).master.process_group_leader()
}

/// Where a prompt goes (REQ-194 to REQ-196): the pane with role `agent`, else
/// the live pane with the lowest id that has a known agent in the foreground.
/// A named pane (REQ-249 to REQ-251) is that one or an error.
fn agent_pane(
    stream: &str,
    mut panes: Vec<(PaneId, Arc<Pane>)>,
    want: Option<PaneId>,
) -> Result<(PaneId, Arc<Pane>)> {
    if let Some(id) = want {
        return named_agent(id, panes);
    }
    panes.sort_by_key(|(id, _)| *id);
    if let Some(i) = panes.iter().position(|(_, p)| p.role == "agent") {
        return Ok(panes.swap_remove(i));
    }
    panes
        .into_iter()
        .find(|(_, p)| {
            // The state guard is gone before `io` and `ps` (daemon AGENTS.md).
            let live = lock(&p.state).exited.is_none();
            live && leader_of(p)
                .and_then(process_name)
                .is_some_and(|name| program::is_agent_program(&name))
        })
        .with_context(|| format!("{stream} has no agent pane open"))
}

/// The pane `id` of a stream's panes, if it is live and an agent's: role
/// `agent` or a known agent in the foreground.
fn named_agent(id: PaneId, panes: Vec<(PaneId, Arc<Pane>)>) -> Result<(PaneId, Arc<Pane>)> {
    let (_, pane) = panes
        .into_iter()
        .find(|(i, _)| *i == id)
        .with_context(|| format!("pane {id} is not in this workspace"))?;
    // The state guard is gone before `io` and `ps` (daemon AGENTS.md).
    let exited = lock(&pane.state).exited.is_some();
    if exited {
        bail!("pane {id} has exited");
    }
    let program = leader_of(&pane).and_then(process_name);
    if program::is_agent_pane(&pane.role, program.as_deref()) {
        return Ok((id, pane));
    }
    bail!(
        "pane {id} is not an agent (it runs {})",
        program.as_deref().unwrap_or("no program")
    )
}

/// The command name of a process (`ps -o comm=`), without its directory.
fn process_name(pid: libc::pid_t) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let name = name
        .rsplit('/')
        .next()
        .unwrap_or(&name)
        .trim_start_matches('-');
    (!name.is_empty()).then(|| name.to_string())
}

/// The directory a process is in: where the pane's shell has `cd`'d to.
#[cfg(target_os = "macos")]
fn process_cwd(pid: libc::pid_t) -> Option<String> {
    // SAFETY: a plain C struct of integers and chars: all zeroes is valid.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: vip_path is MAXPATHLEN chars, NUL-terminated by the kernel.
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    Some(path.to_string_lossy().into_owned()).filter(|p| !p.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn process_cwd(pid: libc::pid_t) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.display().to_string())
}

/// How long a pane must be silent before a prompt is typed into it, and how
/// long to wait for that at most.
const QUIET: Duration = Duration::from_millis(1200);
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Types `text` into an agent's pane once it has gone quiet (REQ-13): as a
/// bracketed paste when the app asked for it, so newlines in the text don't
/// submit early, then Enter on its own.
fn prompt(pane: &Pane, text: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        let st = lock(&pane.state);
        if st.exited.is_some() {
            bail!("the agent has exited");
        }
        let quiet = st.last_output.elapsed();
        drop(st);
        if quiet >= QUIET {
            break;
        }
        if start.elapsed() > READY_TIMEOUT {
            bail!(
                "the agent kept printing for {}s; try again when it settles",
                READY_TIMEOUT.as_secs()
            );
        }
        thread::sleep(QUIET - quiet);
    }
    let paste = lock(&pane.state).parser.screen().bracketed_paste();
    let body = if paste {
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text.to_string()
    };
    {
        let mut io = lock(&pane.io);
        io.writer.write_all(body.as_bytes())?;
        io.writer.flush()?;
    }
    // Apps that take pastes often debounce them: Enter right behind the
    // paste can land inside it.
    thread::sleep(Duration::from_millis(150));
    let mut io = lock(&pane.io);
    io.writer.write_all(b"\r")?;
    io.writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(chunks: &[&[u8]]) -> Titles {
        let mut p = vt100::Parser::new_with_callbacks(24, 80, 0, Titles::default());
        for c in chunks {
            p.process(c);
        }
        std::mem::take(p.callbacks_mut())
    }

    #[test]
    fn an_answer_ends_the_wait_and_a_terminal_report_does_not() {
        assert_eq!(answered(b"\r"), AgentState::Working);
        assert_eq!(answered(b"y"), AgentState::Working);
        assert_eq!(answered(b"\x1b"), AgentState::Idle);
        for report in [
            &b"\x1b[I"[..],
            b"\x1b[<0;1;1M",
            b"\x1b]11;rgb:0/0/0\x07",
            b"",
        ] {
            assert_eq!(answered(report), AgentState::Waiting, "{report:?}");
        }
    }

    #[test]
    fn queries_are_caught_across_reads_and_among_other_modes() {
        let t = parse(&[
            b"\x1b]10;?;?\x07\x1b]1",
            b"1;?\x1b\\\x1b[?1049;2031h\x1b[?996n",
        ]);
        assert_eq!(t.queries, [Query::Fg, Query::Bg, Query::Bg, Query::Scheme]);
        assert!(t.scheme);
        assert!(!parse(&[b"\x1b[?2031h\x1b[?2031l"]).scheme);
        // A colour being set is not a question.
        assert!(parse(&[b"\x1b]11;#000000\x07"]).queries.is_empty());
    }

    /// Clients' threads save at once (and the timer, and a signal): the file
    /// must always parse, and the last save must hold the last change.
    #[test]
    fn concurrent_saves_leave_a_whole_session() {
        let dir = std::env::temp_dir().join(format!("jw-daemon-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let daemon = Arc::new(Daemon::new(
            Some(path.clone()),
            Mobile::new(
                PathBuf::new(),
                Arc::new(|| serde_json::json!({})),
                Tuning::default(),
            ),
        ));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let daemon = Arc::clone(&daemon);
                thread::spawn(move || {
                    for i in 0..40 {
                        let leaf = PaneLeaf {
                            id: t * 1000 + i,
                            role: "view:diff".into(),
                            name: Some("x".repeat(64 * (i as usize % 7))),
                        };
                        let ws = Ws {
                            tree: Tree::Leaf(leaf),
                            subs: Vec::new(),
                        };
                        lock(&daemon.workspaces).insert(format!("ws-{t}-{}", i % 5), ws);
                        daemon.try_save().unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let saved: Session = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let ids: Vec<String> = saved.workspaces.into_iter().map(|w| w.id).collect();
        let want: Vec<String> = lock(&daemon.workspaces).keys().cloned().collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(ids, want);
    }

    #[test]
    fn colours_are_answered_in_sixteen_bits() {
        let screen = vt100::Parser::default();
        let theme = Theme::default();
        assert_eq!(
            Query::Bg.answer(&theme, false, screen.screen()),
            b"\x1b]11;rgb:2828/2c2c/3434\x1b\\"
        );
    }
}
