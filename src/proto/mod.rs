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
pub const PROTOCOL: u32 = 4;

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
    /// Which panes exist, for every stream: answered with `Panes`.
    List,
    /// Type `text` into the stream's agent pane once it is quiet, then
    /// Enter. Answered with `Prompted` or `Error`.
    Prompt {
        stream: String,
        text: String,
    },
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

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|&n| n <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one frame. `Ok(None)` is a clean end of stream between frames.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>> {
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
    Ok(Some(serde_json::from_slice(&body)?))
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
    fn oversized_frame_is_rejected() {
        let mut r: &[u8] = &(MAX_FRAME + 1).to_be_bytes();
        assert!(read_frame::<ClientMsg>(&mut r).is_err());
    }
}
