//! The client end of the socket: connect, and start the daemon when nobody
//! is listening (REQ-3).

use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::proto::{ClientMsg, DaemonMsg, read_frame, write_frame};

/// How long a freshly started daemon gets to open its socket.
const START_TIMEOUT: Duration = Duration::from_secs(3);

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> io::Result<Self> {
        Ok(Self {
            stream: UnixStream::connect(socket)?,
        })
    }

    /// Connects to the daemon on `socket`, starting `exe daemon` detached
    /// first if nothing answers. The daemon removes a stale socket file.
    pub fn connect_or_start(socket: &Path, exe: &Path) -> Result<Self> {
        if let Ok(c) = Self::connect(socket) {
            return Ok(c);
        }
        let log = socket.with_extension("log");
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .with_context(|| format!("opening {}", log.display()))?;

        let mut cmd = Command::new(exe);
        cmd.arg("daemon")
            .env("JW_SOCKET", socket)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        // SAFETY: setsid is async-signal-safe. A new session detaches the
        // daemon from this terminal, so closing it doesn't send SIGHUP.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {} daemon", exe.display()))?;
        // Reap it if it dies while we are still around, instead of a zombie.
        thread::spawn(move || child.wait());

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            match Self::connect(socket) {
                Ok(c) => return Ok(c),
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Err(e) => bail!("daemon did not open {}: {e}", socket.display()),
            }
        }
    }

    pub fn send(&mut self, msg: &ClientMsg) -> io::Result<()> {
        write_frame(&mut self.stream, msg)
    }

    /// The next message, or `None` once the daemon closed the connection.
    pub fn recv(&mut self) -> io::Result<Option<DaemonMsg>> {
        read_frame(&mut self.stream)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
}
