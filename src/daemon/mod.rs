//! The daemon owns every terminal (docs/specs/rust-tui.md, REQ-3…5, 10, 14)
//! and each workspace's tree of panes (REQ-34): one PTY per pane plus a
//! vt100 parser that keeps its screen, so clients can come and go while the
//! processes keep running. Every change to a tree goes to the clients
//! watching that workspace and to `session.json`.
//!
//! Threads: one accept loop, a reader and a writer per client, and a reader
//! per pane. A pane's `state` lock covers both its parser and its
//! subscribers, which is what makes "snapshot, then output" race free. Locks
//! are taken in the order workspaces → panes → a pane's state.

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
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;

use crate::layout::{Dir, Tree};
use crate::proto::{
    ClientMsg, DaemonMsg, NewPane, PROTOCOL, PaneId, PaneInfo, PaneLeaf, read_frame, write_frame,
};

/// Lines of scrollback each pane keeps.
const SCROLLBACK: usize = 10_000;

/// Runs the daemon on `socket` until the process is killed.
pub fn run(socket: &Path) -> Result<()> {
    let listener = bind(socket)?;
    let pidfile = pidfile(socket);
    fs::write(&pidfile, format!("{}\n", std::process::id()))
        .with_context(|| format!("writing {}", pidfile.display()))?;

    let daemon = Arc::new(Daemon::new(session_path(socket)));
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

struct Daemon {
    panes: Mutex<BTreeMap<PaneId, Arc<Pane>>>,
    workspaces: Mutex<BTreeMap<String, Ws>>,
    /// Where the trees are saved after every change; `None` keeps no file.
    session: Option<PathBuf>,
    next_pane: AtomicU64,
    next_client: AtomicU64,
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
    stream: String,
    role: String,
    /// How it was started, for `session.json`.
    started: Started,
    io: Mutex<PaneIo>,
    state: Mutex<PaneState>,
}

#[derive(Debug, Clone, Serialize)]
struct Started {
    cmd: Option<String>,
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
}

/// Catches the window titles a pane's program sets.
#[derive(Default)]
struct Titles {
    new: Option<String>,
}

impl vt100::Callbacks for Titles {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.new = Some(String::from_utf8_lossy(title).into_owned());
    }
}

/// One client's interest in one pane or workspace.
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

/// What `session.json` holds: every open workspace, enough to start it again.
#[derive(Serialize)]
struct Session {
    workspaces: Vec<SavedWs>,
}

#[derive(Serialize)]
struct SavedWs {
    id: String,
    tree: Tree<SavedPane>,
}

#[derive(Serialize)]
struct SavedPane {
    role: String,
    name: Option<String>,
    /// How its process started; none for a viewer (`view:…`).
    #[serde(flatten)]
    started: Option<Started>,
}

/// A leaf the client draws itself (a diff, a Markdown file): it has an id
/// and a place in the tree, but no process.
pub fn is_view(role: &str) -> bool {
    role.starts_with("view:")
}

impl Daemon {
    fn new(session: Option<PathBuf>) -> Self {
        Self {
            panes: Mutex::default(),
            workspaces: Mutex::default(),
            session,
            next_pane: AtomicU64::new(0),
            next_client: AtomicU64::new(0),
        }
    }

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
            ClientMsg::Hello { .. } => {
                let _ = tx.send(DaemonMsg::Hello { protocol: PROTOCOL });
            }
            ClientMsg::Attach { stream } => self.attach(client, tx, &stream),
            ClientMsg::Detach => self.unsubscribe(client),
            ClientMsg::Prompt { stream, text } => {
                let (id, pane) = self
                    .panes_of(&stream)
                    .into_iter()
                    .find(|(_, p)| p.role == "agent")
                    .with_context(|| format!("{stream} has no agent pane open"))?;
                let tx = tx.clone();
                // Waiting can take seconds; other messages from this client
                // must not queue behind it.
                thread::spawn(move || {
                    let msg = match prompt(&pane, &text) {
                        Ok(()) => DaemonMsg::Prompted { pane: id },
                        Err(e) => DaemonMsg::Error {
                            msg: format!("{e:#}"),
                        },
                    };
                    let _ = tx.send(msg);
                });
            }
            ClientMsg::List => {
                let panes = lock(&self.panes)
                    .iter()
                    .map(|(id, p)| {
                        let exited = lock(&p.state).exited;
                        let fg = match exited {
                            None => lock(&p.io)
                                .master
                                .process_group_leader()
                                .and_then(process_name),
                            Some(_) => None,
                        };
                        PaneInfo {
                            pane: *id,
                            stream: p.stream.clone(),
                            role: p.role.clone(),
                            exited,
                            fg,
                        }
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
                let new = NewPane {
                    role,
                    cmd,
                    cwd,
                    env,
                    cols,
                    rows,
                };
                let id = self.spawn(&stream, new, Some((client, tx)))?;
                self.place(&stream, id);
                let _ = tx.send(DaemonMsg::Spawned { pane: id });
            }
            ClientMsg::Open { stream, tree } => {
                if !lock(&self.workspaces).contains_key(&stream) {
                    self.open(&stream, tree)?;
                }
                self.attach(client, tx, &stream);
            }
            ClientMsg::Split { pane, dir, new } => self.split(pane, dir, new)?,
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
        }
        Ok(())
    }

    /// Subscribes `client` to a workspace: its tree, then each pane's screen.
    fn attach(&self, client: u64, tx: &Sender<DaemonMsg>, stream: &str) {
        let ids: Vec<PaneId> = {
            let mut all = lock(&self.workspaces);
            match all.get_mut(stream) {
                Some(ws) => {
                    let _ = tx.send(DaemonMsg::Tree {
                        stream: stream.to_string(),
                        tree: ws.tree.clone(),
                    });
                    if !ws.subs.iter().any(|s| s.client == client) {
                        ws.subs.push(Sub {
                            client,
                            tx: tx.clone(),
                        });
                    }
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
    fn watch(&self, client: u64, tx: &Sender<DaemonMsg>, id: PaneId) {
        let Ok(pane) = self.pane(id) else { return };
        let mut st = lock(&pane.state);
        let screen = st.parser.screen();
        let (rows, cols) = screen.size();
        let _ = tx.send(DaemonMsg::Snapshot {
            pane: id,
            role: pane.role.clone(),
            cols,
            rows,
            bytes: screen.state_formatted(),
        });
        if let Some(status) = st.exited {
            let _ = tx.send(DaemonMsg::Exited { pane: id, status });
        }
        if let Some(title) = st.title.clone() {
            let _ = tx.send(DaemonMsg::Title { pane: id, title });
        }
        st.subscribe(client, tx);
    }

    /// Starts every pane of a new workspace; none is left behind on failure.
    fn open(&self, stream: &str, tree: Tree<NewPane>) -> Result<()> {
        let mut started = Vec::new();
        let tree = tree.try_map(&mut |new: NewPane| {
            let role = new.role.clone();
            let id = self.spawn(stream, new, None)?;
            started.push(id);
            Ok::<_, anyhow::Error>(PaneLeaf {
                id,
                role,
                name: None,
            })
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
                    let mut ws = Ws { tree, subs };
                    ws.tell(stream);
                    all.insert(stream.to_string(), ws);
                }
            }
        }
        self.save();
    }

    fn split(&self, beside: PaneId, dir: Dir, new: NewPane) -> Result<()> {
        let stream = self.stream_of(beside)?;
        let role = new.role.clone();
        let id = if is_view(&role) {
            self.next_pane.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.spawn(&stream, new, None)?
        };
        {
            let mut all = lock(&self.workspaces);
            let Some(Ws { tree, subs }) = all.remove(&stream) else {
                drop(all);
                self.stop(id);
                bail!("{stream} is not open");
            };
            let leaf = PaneLeaf {
                id,
                role,
                name: None,
            };
            let (tree, missing) = tree.insert(beside, dir, leaf);
            let mut ws = Ws { tree, subs };
            ws.tell(&stream);
            let watchers: Vec<(u64, Sender<DaemonMsg>)> =
                ws.subs.iter().map(|s| (s.client, s.tx.clone())).collect();
            all.insert(stream.clone(), ws);
            if missing.is_some() {
                drop(all);
                self.stop(id);
                bail!("pane {beside} is not in {stream}");
            }
            for (client, tx) in watchers {
                self.watch(client, &tx, id);
            }
        }
        self.save();
        Ok(())
    }

    /// A pane along the right edge of the whole workspace.
    fn dock(&self, stream: &str, share: f32, new: NewPane) -> Result<()> {
        let role = new.role.clone();
        let id = if is_view(&role) {
            self.next_pane.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.spawn(stream, new, None)?
        };
        {
            let mut all = lock(&self.workspaces);
            let Some(Ws { tree, subs }) = all.remove(stream) else {
                drop(all);
                self.stop(id);
                bail!("{stream} is not open");
            };
            let leaf = PaneLeaf {
                id,
                role,
                name: None,
            };
            let mut ws = Ws {
                tree: tree.dock(leaf, share),
                subs,
            };
            ws.tell(stream);
            let watchers: Vec<(u64, Sender<DaemonMsg>)> =
                ws.subs.iter().map(|s| (s.client, s.tx.clone())).collect();
            all.insert(stream.to_string(), ws);
            for (client, tx) in watchers {
                self.watch(client, &tx, id);
            }
        }
        self.save();
        Ok(())
    }

    /// Stops a pane and takes it out of its tree; the last one closes the
    /// workspace.
    fn kill(&self, id: PaneId) -> Result<()> {
        let stream = self.stream_of(id)?;
        self.stop(id);
        {
            let mut all = lock(&self.workspaces);
            if let Some(Ws { tree, subs }) = all.remove(&stream) {
                let (tree, _) = tree.remove(id);
                if let Some(tree) = tree {
                    let mut ws = Ws { tree, subs };
                    ws.tell(&stream);
                    all.insert(stream, ws);
                }
            }
        }
        self.save();
        Ok(())
    }

    /// Changes a workspace's tree in place, then tells its watchers and
    /// saves.
    fn edit(&self, stream: &str, f: impl FnOnce(&mut Tree<PaneLeaf>) -> Result<()>) -> Result<()> {
        {
            let mut all = lock(&self.workspaces);
            let ws = all
                .get_mut(stream)
                .with_context(|| format!("{stream} is not open"))?;
            f(&mut ws.tree)?;
            ws.tell(stream);
        }
        self.save();
        Ok(())
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

    /// Writes `session.json` (temp file + rename). A failure is logged, not
    /// fatal: the panes matter more than the file.
    fn save(&self) {
        let Some(path) = &self.session else { return };
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
                                    Some(p) => Some(p.started.clone()),
                                    None if is_view(&l.role) => None,
                                    None => return Err(()),
                                };
                                Ok::<_, ()>(SavedPane {
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
        if let Err(e) = write() {
            eprintln!("jw daemon: saving {}: {e:#}", path.display());
        }
    }

    /// The workspace a pane or a viewer belongs to.
    fn stream_of(&self, id: PaneId) -> Result<String> {
        if let Ok(p) = self.pane(id) {
            return Ok(p.stream.clone());
        }
        lock(&self.workspaces)
            .iter()
            .find(|(_, ws)| ws.tree.find(id).is_some())
            .map(|(s, _)| s.clone())
            .with_context(|| format!("no pane {id}"))
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
        for ws in lock(&self.workspaces).values_mut() {
            ws.subs.retain(|s| s.client != client);
        }
        let panes: Vec<_> = lock(&self.panes).values().cloned().collect();
        for pane in panes {
            lock(&pane.state).subs.retain(|s| s.client != client);
        }
    }

    /// Starts a pane's process. `sub` is subscribed before its first byte
    /// can arrive; without one, watchers get a Snapshot later.
    fn spawn(
        &self,
        stream: &str,
        new: NewPane,
        sub: Option<(u64, &Sender<DaemonMsg>)>,
    ) -> Result<PaneId> {
        let id = self.next_pane.fetch_add(1, Ordering::Relaxed) + 1;
        let NewPane {
            role,
            cmd,
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
            stream: stream.to_string(),
            role,
            started: Started { cmd, cwd, env },
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
            }),
        });
        if let Some((client, tx)) = sub {
            lock(&pane.state).subscribe(client, tx);
        }
        lock(&self.panes).insert(id, Arc::clone(&pane));

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut st = lock(&pane.state);
                        st.parser.process(&buf[..n]);
                        st.last_output = Instant::now();
                        st.broadcast(&DaemonMsg::Output {
                            pane: id,
                            bytes: buf[..n].to_vec(),
                        });
                        if let Some(title) = st.parser.callbacks_mut().new.take() {
                            st.title = Some(title.clone());
                            st.broadcast(&DaemonMsg::Title { pane: id, title });
                        }
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
