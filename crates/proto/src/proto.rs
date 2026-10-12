//! Wire format between the daemon and its clients (docs/specs/rust-tui.md,
//! "Protocol"): each frame is a big-endian u32 length, then a JSON message,
//! or a JSON head and the message's terminal bytes raw (see `write_frame`).

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::layout::{Dir, Keyed, Tree};

pub type PaneId = u64;

/// Bumped whenever a message changes shape: a client and a daemon from
/// different builds refuse each other instead of misreading (RISK-14).
pub const PROTOCOL: u32 = 18;

/// A run of cells of one row that share a style: its first column, its
/// text (a wide character counts two columns; the cells after it are not
/// in the text), the foreground and the background (0 default, `1 << 24 | i`
/// a palette index, `2 << 24 | 0xrrggbb` a colour) and the attributes
/// (bold 1, dim 2, italic 4, underline 8, inverse 16). A row's blank end is
/// left out.
pub type Run = (u16, String, u32, u32, u8);

/// One block of an agent's conversation, as its transcript has it (mobile
/// REQ-90). `kind` is `user` or `text` (a message: `text`), `think`
/// (`result` says how long it took) or `tool` (`name`, `arg` the one line
/// that sums the call up, `body` the command, the diff or the output, and
/// `result` the line that sums its result up, `None` until it has one).
/// `i` numbers the conversation's blocks from its start: a block with a
/// number the client has is that block again, changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatBlock {
    pub i: u32,
    pub kind: String,
    pub text: String,
    pub name: String,
    pub arg: String,
    pub result: Option<String>,
    pub body: Option<String>,
    pub error: bool,
}

/// One command of a shell and its output (mobile REQ-92). `out` is its output
/// as plain text, the last lines of it (`lines` counts them all); `code` is
/// its exit status once it ended; `ms` how long it ran (so far, while
/// `running`); `full` says a program drew the whole screen (nvim, htop), which
/// keeps no output. `i` numbers the pane's commands from its start: a block
/// with a number the client has is that block again, changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellBlock {
    pub i: u32,
    pub cmd: String,
    pub cwd: String,
    pub out: String,
    pub lines: u32,
    pub code: Option<i32>,
    pub ms: u64,
    pub running: bool,
    pub full: bool,
}

/// A pane to start. `cmd` runs through `sh -c`; `None` starts `$SHELL`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPane {
    pub role: String,
    pub cmd: Option<String>,
    /// What to run instead when the daemon starts it again after a restart
    /// (an agent's resume command, REQ-15).
    #[serde(default)]
    pub resume: Option<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

/// A running pane as a leaf of its workspace's tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLeaf {
    pub id: PaneId,
    pub role: String,
    /// What the user called it (`^␣ n`); shown instead of its title.
    pub name: Option<String>,
}

impl Keyed for PaneLeaf {
    fn key(&self) -> u64 {
        self.id
    }
}

/// Frames bigger than this are a bug or garbage on the socket, not output.
pub const MAX_FRAME: u32 = 16 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ClientMsg {
    /// The first message: answered with the daemon's own `Hello`.
    Hello {
        protocol: u32,
    },
    /// Subscribe to the workspace `stream`: its Tree, a Snapshot of each
    /// pane, then live Output and every later Tree.
    Attach {
        stream: String,
    },
    /// From now on this connection gets a pane it starts to watch as a
    /// `Screen` (small, at once) and then its `History` (the replay that
    /// rebuilds the scrollback), instead of one `Snapshot`.
    SplitSnapshots,
    /// From now on this connection gets a pane it watches as `Text` (its
    /// screen as styled rows, at most every 80 ms and only when it changed)
    /// instead of `Snapshot` and `Output`: a phone draws no terminal, so it
    /// needs neither the bytes nor the replay (mobile REQ-21).
    TextView,
    /// From now on this connection gets the conversation of the agent in
    /// `pane` as `Chat`: the last blocks at once, then each change.
    ChatWatch {
        pane: PaneId,
    },
    /// Stop getting `Chat` for `pane` (leaving the workspace does it too).
    ChatUnwatch {
        pane: PaneId,
    },
    /// From now on this connection gets the commands of the shell in `pane`
    /// as `Shell`: the last blocks at once, then each change.
    ShellWatch {
        pane: PaneId,
    },
    /// Stop getting `Shell` for `pane` (leaving the workspace does it too).
    ShellUnwatch {
        pane: PaneId,
    },
    /// The last `count` rows of the pane's scrollback, as `TextHistory`,
    /// oldest first (at most `SCROLLBACK`).
    TextHistory {
        pane: PaneId,
        count: u32,
    },
    /// Stop receiving output from every pane; the panes keep running.
    Detach,
    Input {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    Resize {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    /// Start a pane. `cmd` runs through `sh -c`; `None` starts `$SHELL`.
    /// The client that spawns it is subscribed to the new pane.
    Spawn {
        stream: String,
        role: String,
        cmd: Option<String>,
        cwd: PathBuf,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    },
    /// Start a workspace's panes in this shape and attach to it. A
    /// workspace already open is attached to as it is.
    Open {
        stream: String,
        tree: Tree<NewPane>,
    },
    /// Start a pane beside `pane`, sharing its space along `dir`.
    Split {
        pane: PaneId,
        dir: Dir,
        new: NewPane,
    },
    /// Put a pane along the right edge of the whole workspace, taking
    /// `share` of its width (a viewer, REQ-57).
    Dock {
        stream: String,
        share: f32,
        new: NewPane,
    },
    /// Kill the pane's process (if still running) and forget the pane; its
    /// sibling in the tree takes the space.
    Kill {
        pane: PaneId,
    },
    /// Kill every pane of the workspace.
    Close {
        stream: String,
    },
    /// Move the line between the two sides of a split: `a` keeps `ratio`.
    Ratio {
        stream: String,
        path: crate::layout::SplitPath,
        ratio: f32,
    },
    /// Trade the places of two panes of one workspace.
    Swap {
        a: PaneId,
        b: PaneId,
    },
    /// Name a pane; `None` goes back to its automatic title.
    Name {
        pane: PaneId,
        name: Option<String>,
    },
    /// Point a viewer at other content (`view:md:<file>`), so a client
    /// that attaches later, or a restart, shows what is on screen now.
    Role {
        pane: PaneId,
        role: String,
    },
    /// A workspace's id changed (its session was renamed): `from`'s panes
    /// and tree go on as `to`.
    Rekey {
        from: String,
        to: String,
    },
    /// A workspace's folder moved (its worktree was renamed): its panes
    /// keep running, and a restart starts them under `to` with `env`.
    Moved {
        stream: String,
        from: PathBuf,
        to: PathBuf,
        env: BTreeMap<String, String>,
    },
    /// What an agent in a pane is doing, from its hooks (`jw hook`).
    Agent {
        pane: PaneId,
        state: AgentState,
    },
    /// The colours the client draws panes on, so the daemon can answer a
    /// program that asks for them (S14).
    Theme(Theme),
    /// Which panes exist, for every stream: answered with `Panes`.
    List,
    /// Type `text` into the stream's agent pane once it is quiet, then
    /// Enter: the pane `pane` names, which must be a live agent of the
    /// stream, else the one the daemon picks (REQ-194). Answered with
    /// `Prompted` or `Error`.
    Prompt {
        stream: String,
        text: String,
        #[serde(default)]
        pane: Option<PaneId>,
    },
    /// Become a watcher of mobile: answered with `MobileState` now and after
    /// every change, and sent every `MobilePair`, until this client leaves
    /// (mobile spec, REQ-35).
    MobileWatch,
    /// Start mobile when it is off or failed (REQ-34).
    MobileStart {
        read_only: bool,
    },
    MobileStop,
    /// Replace the pairing token with a new one (REQ-38).
    MobileCode,
    MobileReadOnly {
        on: bool,
    },
    /// The person at the PC answers a `MobilePair` (REQ-36).
    MobileAnswer {
        request: u64,
        yes: bool,
    },
    MobileRevoke {
        device: String,
    },
}

/// What panes sit on: whether the theme is dark, and its default
/// foreground and background (REQ-81–83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub dark: bool,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

impl Default for Theme {
    /// One Dark, until a client says otherwise (REQ-86).
    fn default() -> Self {
        Self {
            dark: true,
            fg: [0xab, 0xb2, 0xbf],
            bg: [0x28, 0x2c, 0x34],
        }
    }
}

/// One pane as `List` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane: PaneId,
    pub stream: String,
    pub role: String,
    /// The exit status once its process ended.
    pub exited: Option<i32>,
    /// The process in the foreground of its terminal, by name (`nvim`,
    /// `zsh`), when it still runs: what closing the pane would kill.
    #[serde(default)]
    pub fg: Option<String>,
    /// It printed in the last 2 s: an agent at work (REQ-51).
    #[serde(default)]
    pub busy: bool,
    /// It rang the bell (or sent a notification) since its last input: an
    /// agent waiting for an answer.
    #[serde(default)]
    pub bell: bool,
    /// What its agent reported last, when it runs one with jw's hooks.
    #[serde(default)]
    pub agent: Option<AgentState>,
    /// The directory its foreground process is in.
    #[serde(default)]
    pub cwd: Option<String>,
    /// What the user called it (`^␣ n`), the name a phone shows for the pane.
    #[serde(default)]
    pub name: Option<String>,
}

impl PaneInfo {
    /// Output in the last 2 s, or an agent working or waiting: stopping it
    /// now would interrupt something (REQ-274).
    pub fn at_work(&self) -> bool {
        self.busy || matches!(self.agent, Some(AgentState::Working | AgentState::Waiting))
    }
}

/// An agent's state as its hooks report it (REQ-73).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// It got a prompt or runs a tool.
    Working,
    /// It asks for something: a permission, an answer.
    Waiting,
    /// Its turn ended.
    Idle,
}

impl std::str::FromStr for AgentState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "working" => Ok(Self::Working),
            "waiting" => Ok(Self::Waiting),
            "idle" => Ok(Self::Idle),
            _ => Err(format!(
                "unknown agent state {s:?}: working, waiting or idle"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum DaemonMsg {
    Hello {
        protocol: u32,
    },
    /// A workspace's panes as they are laid out, after every change.
    Tree {
        stream: String,
        tree: Tree<PaneLeaf>,
    },
    /// The window title a pane's program set (OSC 0/2).
    Title {
        pane: PaneId,
        title: String,
    },
    /// The pane's whole screen as escape sequences (vt100 `state_formatted`):
    /// feeding it to an empty parser of the same size reproduces it.
    Snapshot {
        pane: PaneId,
        role: String,
        cols: u16,
        rows: u16,
        bytes: Vec<u8>,
    },
    /// Only the exact screen, as a `Snapshot` has it at its end: what a
    /// `SplitSnapshots` client gets first, so the pane can be drawn at once.
    Screen {
        pane: PaneId,
        role: String,
        cols: u16,
        rows: u16,
        bytes: Vec<u8>,
    },
    /// What a `Snapshot`'s bytes would have been (the last output, then the
    /// screen), sent right after the `Screen`. Replayed into a parser of
    /// this size it has the scrollback that `Screen` lacks; the output that
    /// came since goes on top.
    History {
        pane: PaneId,
        cols: u16,
        rows: u16,
        bytes: Vec<u8>,
    },
    Output {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    /// The pane's screen as styled rows, for a client that sent `TextView`.
    /// It replaces the screen the client had. `cursor` is `None` when the
    /// program hid it.
    Text {
        pane: PaneId,
        cols: u16,
        rows: u16,
        alt: bool,
        /// The program asked for application cursor keys: the arrows are `ESC O A`.
        app_cursor: bool,
        cursor: Option<(u16, u16)>,
        lines: Vec<Vec<Run>>,
    },
    /// The conversation of the agent in `pane`: `reset` says these blocks are
    /// all the client has (it starts or starts again), else they are the ones
    /// that are new or changed. `title` is the name Claude gave the
    /// conversation, when it is known or changed.
    Chat {
        pane: PaneId,
        /// The pane runs a Claude session whose transcript is followed; false
        /// for any other pane, which has no conversation to show.
        known: bool,
        /// The name the person gave the conversation in Claude (`/rename`).
        name: Option<String>,
        /// What Claude called the conversation itself.
        title: Option<String>,
        reset: bool,
        blocks: Vec<ChatBlock>,
    },
    /// The commands of the shell in `pane`: `reset` says these blocks are all
    /// the client has, else they are the ones new or changed. `known` is false
    /// while the shell has marked no command (it is not one jw started, or
    /// has not run one yet): the pane is then drawn as text.
    Shell {
        pane: PaneId,
        known: bool,
        reset: bool,
        blocks: Vec<ShellBlock>,
    },
    /// The answer to `TextHistory`: rows of the scrollback, oldest first.
    TextHistory {
        pane: PaneId,
        lines: Vec<Vec<Run>>,
    },
    /// The pane's screen is now `cols` by `rows`: a client's `Resize` changed it. It comes in
    /// order with the `Output` around it, so a client drawing the pane at its own size
    /// resizes at the right byte (mobile REQ-70).
    Resized {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    /// The pane's process ended. The pane and its last screen stay until Kill.
    Exited {
        pane: PaneId,
        status: i32,
    },
    Spawned {
        pane: PaneId,
    },
    /// The pane a `Split` made, to the client that asked (REQ-205). Unlike
    /// `Spawned` it does not subscribe the client to the pane.
    Opened {
        pane: PaneId,
    },
    Panes {
        panes: Vec<PaneInfo>,
    },
    Prompted {
        pane: PaneId,
    },
    Error {
        msg: String,
    },
    /// Mobile as it is now, to every watcher on each change (REQ-34, 35).
    /// It carries the pairing token in `payload` (RISK-16): never log it.
    MobileState(MobileState),
    /// A phone asks to pair; `name` and `code` are what it showed, cleaned
    /// of control characters (REQ-36).
    MobilePair {
        request: u64,
        name: String,
        code: String,
    },
    /// The question is over: answered by someone else, timed out, or mobile
    /// stopped.
    MobilePairDone {
        request: u64,
    },
}

/// Where mobile stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileState {
    /// `off`, `starting`, `ready` or `failed`.
    pub phase: String,
    /// While `starting`, how far it got, as a key the client words itself:
    /// `launching` (cloudflared is being run), `started`, `registered` (the
    /// tunnel has a name) or `resolving` (waiting for the name to resolve).
    #[serde(default)]
    pub step: String,
    /// The tunnel's name, once registered.
    #[serde(default)]
    pub host: String,
    /// While `ready` and the code is live: `jw://<host>/<token>`, the QR's
    /// text. Empty when the code was used or ended.
    #[serde(default)]
    pub payload: String,
    /// Seconds the code has left; 0 when there is none.
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub devices: Vec<MobileDevice>,
    /// While `failed`: what went wrong, for the person to read.
    #[serde(default)]
    pub error: String,
    /// `JW_CLOUDFLARED` is set in the daemon's environment: what runs as the
    /// tunnel is a stand-in program, not Cloudflare's. Whoever shows mobile
    /// says so, so nobody has one active without seeing it.
    #[serde(default)]
    pub custom_tunnel: bool,
    /// The wifi listener as `host:port`, or empty when there is none (no
    /// private address, or mobile is off). With one, `phase` is `ready` as soon
    /// as it listens, whatever the tunnel does (REQ-56, REQ-67).
    #[serde(default)]
    pub lan: String,
    /// The tunnel's own phase: `off`, `starting`, `ready` or `failed`. `step`
    /// describes it while it starts and `error` says why when it failed with the
    /// wifi still up.
    #[serde(default)]
    pub tunnel: String,
}

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDevice {
    pub id: String,
    pub name: String,
    /// Unix seconds.
    pub paired: u64,
    pub last_seen: u64,
    /// A phone is connected to the bridge now.
    pub connected: bool,
    /// While `connected`: which listener it came in on, `wifi` or `tunnel`
    /// (REQ-65). Empty otherwise.
    #[serde(default)]
    pub route: String,
}

/// A message that carries terminal bytes sends them raw after a JSON head,
/// not as a JSON array of numbers (RISK-9, REQ-76).
pub trait Raw: Sized {
    /// The message with its bytes left out, and the bytes; `None` for a
    /// message without any.
    fn split_raw(&self) -> Option<(Self, &[u8])>;
    /// Puts the bytes back into a head `split_raw` made.
    fn join_raw(&mut self, raw: Vec<u8>);
}

impl Raw for ClientMsg {
    fn split_raw(&self) -> Option<(Self, &[u8])> {
        match self {
            ClientMsg::Input { pane, bytes } => Some((
                ClientMsg::Input {
                    pane: *pane,
                    bytes: Vec::new(),
                },
                bytes,
            )),
            _ => None,
        }
    }

    fn join_raw(&mut self, raw: Vec<u8>) {
        if let ClientMsg::Input { bytes, .. } = self {
            *bytes = raw;
        }
    }
}

impl Raw for DaemonMsg {
    fn split_raw(&self) -> Option<(Self, &[u8])> {
        match self {
            DaemonMsg::Output { pane, bytes } => Some((
                DaemonMsg::Output {
                    pane: *pane,
                    bytes: Vec::new(),
                },
                bytes,
            )),
            DaemonMsg::Snapshot {
                pane,
                role,
                cols,
                rows,
                bytes,
            } => Some((
                DaemonMsg::Snapshot {
                    pane: *pane,
                    role: role.clone(),
                    cols: *cols,
                    rows: *rows,
                    bytes: Vec::new(),
                },
                bytes,
            )),
            DaemonMsg::Screen {
                pane,
                role,
                cols,
                rows,
                bytes,
            } => Some((
                DaemonMsg::Screen {
                    pane: *pane,
                    role: role.clone(),
                    cols: *cols,
                    rows: *rows,
                    bytes: Vec::new(),
                },
                bytes,
            )),
            DaemonMsg::History {
                pane,
                cols,
                rows,
                bytes,
            } => Some((
                DaemonMsg::History {
                    pane: *pane,
                    cols: *cols,
                    rows: *rows,
                    bytes: Vec::new(),
                },
                bytes,
            )),
            _ => None,
        }
    }

    fn join_raw(&mut self, raw: Vec<u8>) {
        if let DaemonMsg::Output { bytes, .. }
        | DaemonMsg::Snapshot { bytes, .. }
        | DaemonMsg::Screen { bytes, .. }
        | DaemonMsg::History { bytes, .. } = self
        {
            *bytes = raw;
        }
    }
}

/// The bytes a message carries, which is what fills a client's queue.
pub fn raw_len<T: Raw>(msg: &T) -> usize {
    msg.split_raw().map_or(0, |(_, b)| b.len())
}

/// A frame is a u32 length, then a kind: 0 and a JSON message, or 1, a u32
/// length, a JSON head, and the message's bytes as they are.
pub fn write_frame<T: Serialize + Raw>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = match msg.split_raw() {
        None => {
            let mut b = vec![0u8];
            serde_json::to_writer(&mut b, msg)?;
            b
        }
        Some((head, raw)) => {
            let head = serde_json::to_vec(&head)?;
            let mut b = Vec::with_capacity(5 + head.len() + raw.len());
            b.push(1u8);
            b.extend_from_slice(&(head.len() as u32).to_be_bytes());
            b.extend_from_slice(&head);
            b.extend_from_slice(raw);
            b
        }
    };
    let len = u32::try_from(body.len())
        .ok()
        .filter(|&n| n <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one frame. `Ok(None)` is a clean end of stream between frames.
pub fn read_frame<T: DeserializeOwned + Raw>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    match body.first() {
        Some(0) => Ok(Some(serde_json::from_slice(&body[1..])?)),
        Some(1) => {
            let n = body
                .get(1..5)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .filter(|n| 5 + n <= body.len())
                .ok_or_else(|| bad("bad frame head"))?;
            let mut msg: T = serde_json::from_slice(&body[5..5 + n])?;
            msg.join_raw(body.split_off(5 + n));
            Ok(Some(msg))
        }
        _ => Err(bad("unknown frame kind")),
    }
}

/// A frame's body is its kind byte and what follows; a relay forwards it
/// without decoding, so it checks only what `read_frame` checks before it
/// decodes.
fn check_body(body: &[u8]) -> io::Result<()> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    match body.first() {
        None => Err(bad("empty frame")),
        Some(0 | 1) if body.len() as u64 <= u64::from(MAX_FRAME) => Ok(()),
        Some(0 | 1) => Err(bad("frame too large")),
        Some(_) => Err(bad("unknown frame kind")),
    }
}

/// Reads one frame's body (its kind byte and what follows) as it is, for a
/// relay. `Ok(None)` is a clean end of stream between frames.
pub fn read_raw(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    check_body(&body)?;
    Ok(Some(body))
}

/// Writes a body `read_raw` returned, or one a peer sent without its length.
pub fn write_raw(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    check_body(body)?;
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// Where the daemon writes its pid, next to the socket.
pub fn pidfile(socket: &Path) -> PathBuf {
    socket.with_extension("pid")
}

/// A leaf the client draws itself (a diff, a Markdown file): it has an id
/// and a place in the tree, but no process.
pub fn is_view(role: &str) -> bool {
    role.starts_with("view:")
}

/// A name or a code a phone chose, made safe to print on the PC (mobile spec,
/// REQ-27): no control characters, which would move a terminal, and none of
/// the Unicode bidirectional controls and marks, which would make a name read
/// differently from what it is. At most `max` characters, trimmed.
pub fn clean(s: &str, max: usize) -> String {
    let bidi = |c: char| {
        matches!(c,
            '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    let s: String = s
        .chars()
        .filter(|c| !c.is_control() && !bidi(*c))
        .take(max)
        .collect();
    s.trim().to_string()
}

/// `$JW_SOCKET`, else `$XDG_RUNTIME_DIR/jw/jw.sock`, else
/// `~/.local/state/jw/jw.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("JW_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    default_socket_path()
}

/// Where the socket is when `$JW_SOCKET` doesn't say otherwise.
pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        return Path::new(&dir).join("jw").join("jw.sock");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    Path::new(&home).join(".local/state/jw/jw.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let msgs = vec![
            ClientMsg::Attach { stream: "s".into() },
            ClientMsg::Input {
                pane: 7,
                bytes: b"hi\n".to_vec(),
            },
            ClientMsg::Detach,
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m).unwrap();
        }
        let mut r = buf.as_slice();
        for m in &msgs {
            assert_eq!(read_frame::<ClientMsg>(&mut r).unwrap().as_ref(), Some(m));
        }
        assert_eq!(read_frame::<ClientMsg>(&mut r).unwrap(), None);
    }

    #[test]
    fn mobile_messages_round_trip() {
        let state = MobileState {
            phase: "ready".into(),
            step: String::new(),
            host: "a-b.trycloudflare.com".into(),
            payload: "jw://a-b.trycloudflare.com/00ff".into(),
            expires_in: 118,
            read_only: true,
            devices: vec![MobileDevice {
                id: "d1".into(),
                name: "Pixel".into(),
                paired: 10,
                last_seen: 20,
                connected: true,
                route: "wifi".into(),
            }],
            error: String::new(),
            custom_tunnel: true,
            lan: "192.168.1.23:48211".into(),
            tunnel: "ready".into(),
        };
        let down = vec![
            DaemonMsg::MobileState(state),
            DaemonMsg::MobilePair {
                request: 4,
                name: "Pixel".into(),
                code: "482913".into(),
            },
            DaemonMsg::MobilePairDone { request: 4 },
        ];
        let up = vec![
            ClientMsg::MobileWatch,
            ClientMsg::MobileStart { read_only: true },
            ClientMsg::MobileStop,
            ClientMsg::MobileCode,
            ClientMsg::MobileReadOnly { on: false },
            ClientMsg::MobileAnswer {
                request: 4,
                yes: true,
            },
            ClientMsg::MobileRevoke {
                device: "d1".into(),
            },
        ];
        let mut buf = Vec::new();
        for m in &down {
            write_frame(&mut buf, m).unwrap();
        }
        for m in &up {
            write_frame(&mut buf, m).unwrap();
        }
        let mut r = buf.as_slice();
        for m in &down {
            assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap().as_ref(), Some(m));
        }
        for m in &up {
            assert_eq!(read_frame::<ClientMsg>(&mut r).unwrap().as_ref(), Some(m));
        }
    }

    #[test]
    fn what_a_phone_chose_is_cleaned_before_anyone_prints_it() {
        assert_eq!(clean("Pixel\u{1b}[31m 8\u{7}\n", 40), "Pixel[31m 8");
        // A right-to-left override would make "Pixel" read as "lexiP".
        assert_eq!(
            clean("\u{202e}lexiP\u{2066}!\u{2069}\u{200f}", 40),
            "lexiP!"
        );
        assert_eq!(clean(&"a".repeat(100), 40).chars().count(), 40);
        assert_eq!(clean("  Pixel  ", 40), "Pixel");
        assert_eq!(clean("\u{1b}\u{202e}", 40), "");
    }

    #[test]
    fn bytes_travel_raw() {
        let out = DaemonMsg::Output {
            pane: 3,
            bytes: (0..=255).collect(),
        };
        let snap = DaemonMsg::Snapshot {
            pane: 3,
            role: "shell".into(),
            cols: 80,
            rows: 24,
            bytes: b"\x1b[2Jhi".to_vec(),
        };
        let screen = DaemonMsg::Screen {
            pane: 3,
            role: "shell".into(),
            cols: 80,
            rows: 24,
            bytes: b"screen".to_vec(),
        };
        let history = DaemonMsg::History {
            pane: 3,
            cols: 80,
            rows: 24,
            bytes: (0..=255).collect(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &screen).unwrap();
        write_frame(&mut buf, &history).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap(), Some(screen));
        assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap(), Some(history));
        let mut buf = Vec::new();
        write_frame(&mut buf, &out).unwrap();
        // 4 length + 1 kind + 4 head length + the head + 256 raw bytes.
        assert!(buf.len() < 4 + 1 + 4 + 60 + 256, "{} bytes", buf.len());
        write_frame(&mut buf, &snap).unwrap();
        write_frame(&mut buf, &DaemonMsg::Prompted { pane: 1 }).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap(), Some(out));
        assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap(), Some(snap));
        assert_eq!(
            read_frame::<DaemonMsg>(&mut r).unwrap(),
            Some(DaemonMsg::Prompted { pane: 1 })
        );
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let too_big = (MAX_FRAME + 1).to_be_bytes();
        assert!(read_frame::<ClientMsg>(&mut &too_big[..]).is_err());
        assert!(read_raw(&mut &too_big[..]).is_err());
    }

    #[test]
    fn raw_relay_keeps_every_byte() {
        let msgs = vec![
            DaemonMsg::Hello { protocol: PROTOCOL },
            DaemonMsg::Output {
                pane: 3,
                bytes: (0..=255).collect(),
            },
            DaemonMsg::Prompted { pane: 1 },
        ];
        let mut sent = Vec::new();
        for m in &msgs {
            write_frame(&mut sent, m).unwrap();
        }
        let mut relayed = Vec::new();
        let mut r = sent.as_slice();
        while let Some(body) = read_raw(&mut r).unwrap() {
            write_raw(&mut relayed, &body).unwrap();
        }
        assert_eq!(relayed, sent);
        let mut r = relayed.as_slice();
        for m in &msgs {
            assert_eq!(read_frame::<DaemonMsg>(&mut r).unwrap().as_ref(), Some(m));
        }
    }

    #[test]
    fn raw_rejects_a_body_without_a_kind() {
        assert!(write_raw(&mut Vec::new(), &[]).is_err());
        assert!(write_raw(&mut Vec::new(), &[7, 1, 2]).is_err());
        let mut r: &[u8] = &[0, 0, 0, 0];
        assert!(read_raw(&mut r).is_err());
        let mut r: &[u8] = &[0, 0, 0, 2, 9, 9];
        assert!(read_raw(&mut r).is_err());
    }

    #[test]
    fn raw_stops_cleanly_at_the_end() {
        let mut r: &[u8] = &[];
        assert_eq!(read_raw(&mut r).unwrap(), None);
    }

    /// REQ-254: a `Prompt` without `pane` reads as `None`; it came with protocol 11 and every later one reads it so.
    #[test]
    fn a_prompt_without_a_pane_names_none() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"t":"Prompt","stream":"s","text":"hi"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Prompt {
                stream: "s".into(),
                text: "hi".into(),
                pane: None
            }
        );
    }
}
