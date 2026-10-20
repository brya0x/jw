use std::fs;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use contract::{ClientMsg, DaemonMsg, ErrorCode};
use wire::{Conn, Stream, write_line};

use crate::commands;
use crate::core::{Core, OUTBOX};
use crate::integration;
use crate::model::Model;
use crate::panes;
use crate::persist;
use crate::procs;

/// The hub behind one lock. Hold it for bookkeeping only, never across I/O.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<Core>>);

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, Core> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub socket: PathBuf,
    pub state_dir: PathBuf,
    /// The shell every pane runs; `$SHELL` (else `/bin/sh`) when `None`.
    pub shell: Option<String>,
    /// A loopback TCP listener for development: a phone on a USB cable reaches it through
    /// `adb reverse`. Anything on this machine that connects controls every pane, so it is
    /// off unless asked for (`$<PREFIX>_TCP`).
    pub tcp: Option<SocketAddr>,
    /// Extra environment for every pane's shell.
    pub env: Vec<(String, String)>,
    /// Install the shell integration (blocks); `$<PREFIX>_SHELL_BLOCKS=0` turns it off.
    pub blocks: bool,
    /// The phone link: the tunnel and the private-address listener.
    pub link: crate::mobile::LinkConfig,
    /// The binary an agent's hooks run (`<exe> hook <event>`); none gives agents no hooks.
    pub hook_exe: Option<PathBuf>,
    /// `gh`, to look at pull requests; none turns that off.
    pub gh: Option<PathBuf>,
    /// How often the pull requests of the worktrees in use are looked at.
    pub github_every: Duration,
    /// Swap the `claude` a person types in a pane for the binary's own, which talks to the
    /// engine headless and draws a conversation; on unless `$<PREFIX>_CONVERSATION` is `0`.
    pub conversation: bool,
    /// The folder of the personal settings of projects; `<config dir>/projects` when from the
    /// identity (REQ-483).
    pub projects: PathBuf,
    /// Every conversation is given the Claude in Chrome tools (`--chrome`); on unless
    /// `config.toml` says `chrome = false`, and a conversation's own `--chrome` or `--no-chrome`
    /// wins (REQ-634).
    pub chrome: bool,
}

/// Whether `config.toml` leaves Claude in Chrome on for the conversations: `chrome = false` turns it
/// off, and anything else, a file with no such key or one that is not TOML, leaves it on.
pub fn chrome_in(config: &str) -> bool {
    config
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("chrome").and_then(toml::Value::as_bool))
        .unwrap_or(true)
}

impl Config {
    pub fn from_identity() -> Config {
        Config {
            socket: identity::socket_path(),
            state_dir: identity::state_dir(),
            shell: None,
            tcp: std::env::var(identity::env_var("TCP"))
                .ok()
                .and_then(|v| v.parse().ok()),
            env: Vec::new(),
            blocks: std::env::var(identity::env_var("SHELL_BLOCKS")).map_or(true, |v| v != "0"),
            link: crate::mobile::LinkConfig::from_identity(),
            hook_exe: std::env::current_exe().ok(),
            gh: crate::github::find(),
            github_every: Duration::from_secs(60),
            conversation: std::env::var(identity::env_var("CONVERSATION"))
                .map_or(true, |v| v != "0"),
            projects: identity::config_dir().join("projects"),
            chrome: chrome_in(
                &std::fs::read_to_string(identity::config_dir().join("config.toml"))
                    .unwrap_or_default(),
            ),
        }
    }

    pub fn session_file(&self) -> PathBuf {
        self.state_dir.join("session.json")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.socket.with_extension("pid")
    }
}

pub struct Daemon {
    config: Config,
    shared: Shared,
    stop: Arc<AtomicBool>,
    engine: Option<JoinHandle<()>>,
    accept: Option<JoinHandle<()>>,
    tcp: Option<(SocketAddr, JoinHandle<()>)>,
}

impl Daemon {
    pub fn start(config: Config) -> io::Result<Daemon> {
        for dir in [Some(config.state_dir.as_path()), config.socket.parent()]
            .into_iter()
            .flatten()
        {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        if let Some(addr) = config.tcp
            && !addr.ip().is_loopback()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{addr} is not a loopback address; the TCP listener is for development only"
                ),
            ));
        }
        claim_socket(&config.socket)?;
        let listener = UnixListener::bind(&config.socket)?;
        fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
        fs::write(config.pid_file(), std::process::id().to_string())?;

        let model = persist::load(&config.session_file()).unwrap_or_else(Model::new);
        let integration_dir = config.blocks.then(|| config.state_dir.join("shell"));
        if let Some(dir) = &integration_dir {
            integration::install(dir)?;
        }
        let shell = config.shell.clone().unwrap_or_else(default_shell);
        let (events, event_rx) = std::sync::mpsc::channel();
        // Every pane knows its own daemon, so what runs in it (`jwn claude`, `jwn hook`) reaches
        // this one and never another that happens to listen at the default place.
        let mut pane_env = config.env.clone();
        pane_env.push((
            identity::env_var("SOCKET"),
            config.socket.to_string_lossy().into_owned(),
        ));
        pane_env.push((
            identity::env_var("STATE_DIR"),
            config.state_dir.to_string_lossy().into_owned(),
        ));
        if config.conversation
            && let Some(exe) = &config.hook_exe
        {
            pane_env.push((identity::env_var("BIN"), exe.to_string_lossy().into_owned()));
            pane_env.push((identity::env_var("CONVERSATION"), "1".into()));
        }
        let shared = Shared(Arc::new(Mutex::new(Core::new(
            model,
            Some(config.session_file()),
            events,
            shell,
            pane_env,
            integration_dir,
            Arc::new(crate::mobile::Hub::new(
                config.socket.clone(),
                config.state_dir.clone(),
                config.link.clone(),
            )),
        ))));
        shared.lock().history_dir = Some(crate::history::dir(&config.state_dir));
        shared.lock().conv_dir = Some(config.state_dir.join("conversations"));
        shared.lock().projects_dir = config.projects.clone();
        shared.lock().gh = config.gh.clone();
        shared.lock().state_dir = Some(config.state_dir.clone());
        shared.lock().swap = config.conversation && config.hook_exe.is_some();
        shared.lock().chrome = config.chrome;
        shared.lock().mobile.clone().resume();
        if let Some(exe) = &config.hook_exe {
            shared.lock().hooks = Some(crate::launch::write_hooks(&config.state_dir, exe)?);
        }
        let engine = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("engine".into())
                .spawn(move || panes::engine(shared, event_rx))?
        };
        panes::restore(&shared);
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (shared, stop) = (shared.clone(), stop.clone());
            thread::Builder::new()
                .name("procs".into())
                .spawn(move || procs::run(shared, stop))?;
        }
        {
            let (shared, stop) = (shared.clone(), stop.clone());
            thread::Builder::new()
                .name("changes".into())
                .spawn(move || crate::diff::run(shared, stop))?;
        }
        {
            let (shared, stop) = (shared.clone(), stop.clone());
            thread::Builder::new()
                .name("follow".into())
                .spawn(move || crate::engine::follow_run(shared, stop))?;
        }
        if let Some(program) = config.gh.clone() {
            let (shared, stop, every) = (shared.clone(), stop.clone(), config.github_every);
            thread::Builder::new()
                .name("github".into())
                .spawn(move || crate::github::run(shared, stop, program, every))?;
        }
        let accept = {
            let stop = stop.clone();
            let shared = shared.clone();
            thread::Builder::new()
                .name("accept".into())
                .spawn(move || accept_loop(listener.incoming(), shared, stop))?
        };
        let tcp = match config.tcp {
            Some(addr) => {
                let listener = TcpListener::bind(addr)?;
                let bound = listener.local_addr()?;
                eprintln!(
                    "listening on tcp {bound}: any process on this machine can control every pane (development only)"
                );
                let (stop, shared) = (stop.clone(), shared.clone());
                let handle = thread::Builder::new()
                    .name("accept-tcp".into())
                    .spawn(move || accept_loop(listener.incoming(), shared, stop))?;
                Some((bound, handle))
            }
            None => None,
        };
        Ok(Daemon {
            config,
            shared,
            stop,
            engine: Some(engine),
            accept: Some(accept),
            tcp,
        })
    }

    /// The address the development TCP listener is bound to, if it is on.
    pub fn tcp_addr(&self) -> Option<SocketAddr> {
        self.tcp.as_ref().map(|(addr, _)| *addr)
    }

    /// Block until the daemon is stopped (it is stopped by killing the process).
    pub fn wait(mut self) {
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // From here the shells that end are the daemon's doing, not a program that ended: the
        // agents that were running come back after a restart (REQ-391).
        self.shared.lock().stopping = true;
        // Wake the accept loop; if the socket file is already gone nothing can, so leave
        // the thread rather than wait for it forever.
        let woken = UnixStream::connect(&self.config.socket).is_ok();
        if let Some((addr, handle)) = self.tcp.take()
            && std::net::TcpStream::connect(addr).is_ok()
        {
            let _ = handle.join();
        }
        if let Some(h) = self.accept.take()
            && woken
        {
            let _ = h.join();
        }
        let hub = self.shared.lock().mobile.clone();
        hub.stop();
        crate::engine::save_due(&self.shared, true);
        let events = {
            let mut core = self.shared.lock();
            core.shutdown();
            core.events.clone()
        };
        let _ = events.send(crate::pty::Event::Stop);
        if let Some(h) = self.engine.take() {
            let _ = h.join();
        }
        let _ = fs::remove_file(&self.config.socket);
        let _ = fs::remove_file(self.config.pid_file());
    }
}

fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// Refuse to take over a live daemon's socket; clear a stale one.
fn claim_socket(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("a daemon is already listening on {}", path.display()),
        ));
    }
    fs::remove_file(path)
}

fn accept_loop<I, S>(incoming: I, shared: Shared, stop: Arc<AtomicBool>)
where
    I: Iterator<Item = io::Result<S>>,
    S: Into<Stream>,
{
    for stream in incoming {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => {
                let stream: Stream = stream.into();
                let shared = shared.clone();
                let spawned = thread::Builder::new().name("client".into()).spawn(move || {
                    if let Err(e) = serve(&shared, stream) {
                        eprintln!("client ended: {e}");
                    }
                });
                if let Err(e) = spawned {
                    eprintln!("cannot start a client thread: {e}");
                }
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
}

fn serve(shared: &Shared, stream: Stream) -> io::Result<()> {
    let mut conn = Conn::from_stream(stream.try_clone()?)?;
    let (tx, rx) = sync_channel::<DaemonMsg>(OUTBOX);
    let mut out = conn.try_clone_writer()?;
    thread::Builder::new()
        .name("writer".into())
        .spawn(move || {
            for msg in rx {
                if write_line(&mut out, &msg).is_err() {
                    break;
                }
            }
        })?;
    let id = shared.lock().add_client(tx, stream);
    let result = read_loop(shared, id, &mut conn);
    shared.lock().remove_client(id);
    result
}

fn read_loop(shared: &Shared, id: u64, conn: &mut Conn) -> io::Result<()> {
    while let Some(line) = conn.recv_line()? {
        let msg: ClientMsg = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                shared.lock().error(
                    id,
                    ErrorCode::BadRequest,
                    &format!("not a contract message: {e}"),
                );
                continue;
            }
        };
        handle(shared, id, msg);
    }
    Ok(())
}

fn handle(shared: &Shared, id: u64, msg: ClientMsg) {
    if !matches!(msg, ClientMsg::Hello { .. }) && !shared.lock().has_said_hello(id) {
        shared
            .lock()
            .error(id, ErrorCode::BadRequest, "send hello first");
        return;
    }
    match msg {
        ClientMsg::Hello { accepts, .. } => {
            let mut core = shared.lock();
            if core.has_said_hello(id) {
                core.error(id, ErrorCode::BadRequest, "hello was already sent");
            } else {
                core.hello(id, &accepts);
            }
        }
        ClientMsg::Open { node, epoch, since } => {
            shared.lock().open(id, &node, epoch.as_deref(), since)
        }
        ClientMsg::Close { node } => shared.lock().close(id, &node),
        ClientMsg::Cmd { id: cmd_id, cmd } => {
            let outcome = commands::outcome(commands::run(shared, cmd));
            shared.lock().send(
                id,
                DaemonMsg::Reply {
                    id: cmd_id,
                    outcome,
                },
            );
        }
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::chrome_in;

    /// REQ-634: on unless `config.toml` says `chrome = false`.
    #[test]
    fn chrome_is_on_unless_the_config_says_false() {
        assert!(chrome_in(""));
        assert!(chrome_in("suggestions = false\n"));
        assert!(chrome_in("chrome = true\n"));
        assert!(!chrome_in("chrome = false\n"));
        // not a boolean, or not TOML: on
        assert!(chrome_in("chrome = \"no\"\n"));
        assert!(chrome_in("this is = = not toml"));
    }
}
