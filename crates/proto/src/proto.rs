//! Wire format between the TUI client and the daemon (docs/specs/rust-tui.md,
//! "Protocol"): each frame is a big-endian u32 length followed by that many
//! bytes of JSON.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::layout::{Dir, Keyed, Tree};

pub type PaneId = u64;

/// Bumped whenever a message changes shape: a client and a daemon from
/// different builds refuse each other instead of misreading (RISK-14).
pub const PROTOCOL: u32 = 8;

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
const MAX_FRAME: u32 = 16 << 20;

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
    /// Enter. Answered with `Prompted` or `Error`.
    Prompt {
        stream: String,
        text: String,
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
    Output {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    /// The pane's process ended. The pane and its last screen stay until Kill.
    Exited {
        pane: PaneId,
        status: i32,
    },
    Spawned {
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
            _ => None,
        }
    }

    fn join_raw(&mut self, raw: Vec<u8>) {
        if let DaemonMsg::Output { bytes, .. } | DaemonMsg::Snapshot { bytes, .. } = self {
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

/// Where the daemon writes its pid, next to the socket.
pub fn pidfile(socket: &Path) -> PathBuf {
    socket.with_extension("pid")
}

/// A leaf the client draws itself (a diff, a Markdown file): it has an id
/// and a place in the tree, but no process.
pub fn is_view(role: &str) -> bool {
    role.starts_with("view:")
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
        let mut r: &[u8] = &(MAX_FRAME + 1).to_be_bytes();
        assert!(read_frame::<ClientMsg>(&mut r).is_err());
    }
}
