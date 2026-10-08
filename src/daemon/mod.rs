//! The daemon owns every terminal (docs/specs/rust-tui.md, REQ-3…5, 10, 14):
//! one PTY per pane plus a vt100 parser that keeps its screen, so clients can
//! come and go while the processes keep running.
//!
//! Threads: one accept loop, a reader and a writer per client, and a reader
//! per pane. A pane's `state` lock covers both its parser and its
//! subscribers, which is what makes "snapshot, then output" race free.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use anyhow::{Context, Result, bail};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::proto::{ClientMsg, DaemonMsg, PaneId, PaneInfo, read_frame, write_frame};

/// Lines of scrollback each pane keeps.
const SCROLLBACK: usize = 10_000;

/// Runs the daemon on `socket` until the process is killed.
pub fn run(socket: &Path) -> Result<()> {
    let listener = bind(socket)?;
    let pidfile = pidfile(socket);
    fs::write(&pidfile, format!("{}\n", std::process::id()))
        .with_context(|| format!("writing {}", pidfile.display()))?;

    let daemon = Arc::new(Daemon::default());
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

/// Where the daemon writes its pid, next to the socket.
pub fn pidfile(socket: &Path) -> PathBuf {
    socket.with_extension("pid")
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

#[derive(Default)]
struct Daemon {
    panes: Mutex<BTreeMap<PaneId, Arc<Pane>>>,
    next_pane: AtomicU64,
    next_client: AtomicU64,
}

struct Pane {
    stream: String,
    role: String,
    io: Mutex<PaneIo>,
    state: Mutex<PaneState>,
}

struct PaneIo {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

struct PaneState {
    parser: vt100::Parser,
    subs: Vec<Sub>,
    exited: Option<i32>,
}

/// One client's interest in one pane.
struct Sub {
    client: u64,
    tx: Sender<DaemonMsg>,
}

impl PaneState {
    /// Sends to every subscriber, dropping the ones whose client went away.
    fn broadcast(&mut self, msg: &DaemonMsg) {
        self.subs.retain(|s| s.tx.send(msg.clone()).is_ok());
    }

    fn subscribe(&mut self, client: u64, tx: &Sender<DaemonMsg>) {
        if !self.subs.iter().any(|s| s.client == client) {
            self.subs.push(Sub {
                client,
                tx: tx.clone(),
            });
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking pane thread must not take the whole daemon down with it.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Daemon {
    fn serve(self: Arc<Self>, stream: UnixStream) {
        let client = self.next_client.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<DaemonMsg>();

        let mut out = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => return eprintln!("jw daemon: client {client}: {e}"),
        };
        thread::spawn(move || {
            for msg in rx {
                if write_frame(&mut out, &msg).is_err() {
                    break;
                }
            }
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
        self.unsubscribe(client);
    }

    fn handle(&self, client: u64, tx: &Sender<DaemonMsg>, msg: ClientMsg) -> Result<()> {
        match msg {
            ClientMsg::Attach { stream } => {
                for pane in self.panes_of(&stream) {
                    let mut st = lock(&pane.1.state);
                    let screen = st.parser.screen();
                    let (rows, cols) = screen.size();
                    let snapshot = DaemonMsg::Snapshot {
                        pane: pane.0,
                        role: pane.1.role.clone(),
                        cols,
                        rows,
                        bytes: screen.state_formatted(),
                    };
                    let _ = tx.send(snapshot);
                    if let Some(status) = st.exited {
                        let _ = tx.send(DaemonMsg::Exited {
                            pane: pane.0,
                            status,
                        });
                    }
                    st.subscribe(client, tx);
                }
            }
            ClientMsg::Detach => self.unsubscribe(client),
            ClientMsg::List => {
                let panes = lock(&self.panes)
                    .iter()
                    .map(|(id, p)| PaneInfo {
                        pane: *id,
                        stream: p.stream.clone(),
                        role: p.role.clone(),
                        exited: lock(&p.state).exited,
                    })
                    .collect();
                let _ = tx.send(DaemonMsg::Panes { panes });
            }
            ClientMsg::Input { pane, bytes } => {
                let pane = self.pane(pane)?;
                let mut io = lock(&pane.io);
                io.writer.write_all(&bytes)?;
                io.writer.flush()?;
            }
            ClientMsg::Resize { pane, cols, rows } => {
                let pane = self.pane(pane)?;
                lock(&pane.io).master.resize(size(cols, rows))?;
                lock(&pane.state).parser.screen_mut().set_size(rows, cols);
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
                let id = self.spawn(client, tx, stream, role, cmd, &cwd, env, cols, rows)?;
                let _ = tx.send(DaemonMsg::Spawned { pane: id });
            }
            ClientMsg::Kill { pane } => {
                let pane = lock(&self.panes)
                    .remove(&pane)
                    .with_context(|| format!("no pane {pane}"))?;
                if lock(&pane.state).exited.is_none() {
                    let _ = lock(&pane.io).killer.kill();
                }
            }
        }
        Ok(())
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
            .filter(|(_, p)| p.stream == stream)
            .map(|(id, p)| (*id, Arc::clone(p)))
            .collect()
    }

    fn unsubscribe(&self, client: u64) {
        let panes: Vec<_> = lock(&self.panes).values().cloned().collect();
        for pane in panes {
            lock(&pane.state).subs.retain(|s| s.client != client);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        client: u64,
        tx: &Sender<DaemonMsg>,
        stream: String,
        role: String,
        cmd: Option<String>,
        cwd: &Path,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId> {
        let id = self.next_pane.fetch_add(1, Ordering::Relaxed) + 1;
        let pair = native_pty_system().openpty(size(cols, rows))?;

        let mut builder = match cmd {
            Some(cmd) => {
                let mut b = CommandBuilder::new("sh");
                b.args(["-c", &cmd]);
                b
            }
            None => CommandBuilder::new_default_prog(),
        };
        builder.cwd(cwd);
        builder.env("TERM", "xterm-256color");
        for (k, v) in env {
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
            stream,
            role,
            io: Mutex::new(PaneIo {
                writer: pair.master.take_writer()?,
                killer: child.clone_killer(),
                master: pair.master,
            }),
            state: Mutex::new(PaneState {
                parser: vt100::Parser::new(rows, cols, SCROLLBACK),
                subs: Vec::new(),
                exited: None,
            }),
        });
        // Subscribe before the reader starts so the spawner misses nothing.
        lock(&pane.state).subscribe(client, tx);
        lock(&self.panes).insert(id, Arc::clone(&pane));

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut st = lock(&pane.state);
                        st.parser.process(&buf[..n]);
                        st.broadcast(&DaemonMsg::Output {
                            pane: id,
                            bytes: buf[..n].to_vec(),
                        });
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

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}
