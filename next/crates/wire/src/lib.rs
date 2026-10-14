//! The transport: JSON objects, one per line, over a Unix socket. `Conn` is the raw line
//! pipe both ends use; `Client` is a ready-made client that keeps the node cache.

mod client;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

pub use client::{Client, ClientError};

/// The node versions this build of the client library can read, for `hello`.
pub fn default_accepts() -> std::collections::BTreeMap<String, Vec<u32>> {
    ["session", "workspace", "pane", "blocks", "diff", "commits"]
        .map(|t| (t.to_owned(), vec![1]))
        .into()
}

/// The longest line either side accepts; a longer one is a protocol error.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// A connected socket of either kind: the same lines travel on both.
pub enum Stream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl From<UnixStream> for Stream {
    fn from(s: UnixStream) -> Self {
        Stream::Unix(s)
    }
}

impl From<TcpStream> for Stream {
    fn from(s: TcpStream) -> Self {
        Stream::Tcp(s)
    }
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        match self {
            Stream::Unix(s) => s.try_clone().map(Stream::Unix),
            Stream::Tcp(s) => s.try_clone().map(Stream::Tcp),
        }
    }

    pub fn shutdown(&self) -> io::Result<()> {
        match self {
            Stream::Unix(s) => s.shutdown(Shutdown::Both),
            Stream::Tcp(s) => s.shutdown(Shutdown::Both),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Unix(s) => s.set_read_timeout(timeout),
            Stream::Tcp(s) => s.set_read_timeout(timeout),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Unix(s) => s.read(buf),
            Stream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Unix(s) => s.write(buf),
            Stream::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Unix(s) => s.flush(),
            Stream::Tcp(s) => s.flush(),
        }
    }
}

pub struct Conn {
    reader: BufReader<Stream>,
    writer: Stream,
    /// Bytes of a line read so far, kept across a read timeout.
    pending: Vec<u8>,
}

impl Conn {
    pub fn connect(path: &Path) -> io::Result<Conn> {
        Conn::from_stream(UnixStream::connect(path)?)
    }

    pub fn connect_tcp(addr: SocketAddr) -> io::Result<Conn> {
        Conn::from_stream(TcpStream::connect(addr)?)
    }

    pub fn from_stream(stream: impl Into<Stream>) -> io::Result<Conn> {
        let stream = stream.into();
        Ok(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            pending: Vec::new(),
        })
    }

    /// A second handle on the same socket, for a writer thread.
    pub fn try_clone_writer(&self) -> io::Result<Stream> {
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
