//! The transport: JSON objects, one per line, over a Unix socket. `Conn` is the raw line
//! pipe both ends use; `Client` is a ready-made client that keeps the node cache.

mod client;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

pub use client::{Client, ClientError};

/// The node versions this build of the client library can read, for `hello`.
pub fn default_accepts() -> std::collections::BTreeMap<String, Vec<u32>> {
    ["session", "workspace", "pane"]
        .map(|t| (t.to_owned(), vec![1]))
        .into()
}

/// The longest line either side accepts; a longer one is a protocol error.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

pub struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    /// Bytes of a line read so far, kept across a read timeout.
    pending: Vec<u8>,
}

impl Conn {
    pub fn connect(path: &Path) -> io::Result<Conn> {
        Conn::from_stream(UnixStream::connect(path)?)
    }

    pub fn from_stream(stream: UnixStream) -> io::Result<Conn> {
        Ok(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            pending: Vec::new(),
        })
    }

    /// A second handle on the same socket, for a writer thread.
    pub fn try_clone_writer(&self) -> io::Result<UnixStream> {
        self.writer.try_clone()
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.writer.set_read_timeout(timeout)
    }

    pub fn send<T: Serialize>(&mut self, msg: &T) -> io::Result<()> {
        write_line(&mut self.writer, msg)
    }

    /// The next line without its newline, or `None` at end of stream. A read timeout is
    /// an `Err` of kind `WouldBlock`/`TimedOut` and loses nothing.
    pub fn recv_line(&mut self) -> io::Result<Option<String>> {
        loop {
            let room = (MAX_LINE + 1).saturating_sub(self.pending.len()) as u64;
            let n = (&mut self.reader)
                .take(room)
                .read_until(b'\n', &mut self.pending)?;
            if n == 0 {
                return Ok(None);
            }
            if self.pending.last() == Some(&b'\n') {
                self.pending.pop();
                let line = std::mem::take(&mut self.pending);
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "line is not UTF-8"));
            }
            if self.pending.len() > MAX_LINE {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
            }
        }
    }
}

/// Write one JSON object and a newline as a single `write`, so lines never interleave.
pub fn write_line<W: Write, T: Serialize>(out: &mut W, msg: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    bytes.push(b'\n');
    out.write_all(&bytes)?;
    out.flush()
}
