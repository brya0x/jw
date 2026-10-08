//! Wire format between the TUI client and the daemon (docs/specs/rust-tui.md,
//! "Protocol"): each frame is a big-endian u32 length followed by that many
//! bytes of JSON. Open/Close arrive in later parts.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub type PaneId = u64;

/// Frames bigger than this are a bug or garbage on the socket, not output.
const MAX_FRAME: u32 = 16 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ClientMsg {
    /// Subscribe to every pane of `stream`: a Snapshot each, then live Output.
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
    /// Kill the pane's process (if still running) and forget the pane.
    Kill {
        pane: PaneId,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum DaemonMsg {
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
