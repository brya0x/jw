//! The bridge: a WebSocket on loopback that relays one phone to one daemon
//! connection, frame for frame (mobile spec, REQ-4 to 11).
//!
//! A binary WebSocket message is a jw-proto frame without its length: the
//! kind byte and what follows. A text message is a control message, a JSON
//! object tagged `t`. What a phone sees besides frames:
//!
//! - `ready{bridge,protocol,read_only,palettes,settings}` once it is let in and the daemon is
//!   there, and `paired{device,id}` before it when it has just paired;
//! - `error{code,msg}` with a `code` the app can act on: `unexpected`,
//!   `timeout`, `limited`, `expired`, `unknown`, `denied`, `revoked`,
//!   `read_only`, `unsupported`, `protocol`, `daemon`.
//!
//! Once in, a phone may ask `workspaces{session?}` and `sessions`, and is
//! answered `workspaces{session,items}` and `sessions{items}` from what the
//! PC's jw has on disk (REQ-12), and `diff{ws}` with that workspace's changes
//! (REQ-13). They only read, so they pass in read-only. A diff runs git, so it
//! is computed on a thread of its own and one at a time per phone.
//!
//! A phone is let in by `pair` or `auth` and nothing else; until then the
//! daemon is not even connected to.

use rustls::ServerConfig;
use std::io::{self, ErrorKind};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use jw_proto::proto::clean;
use jw_proto::proto::{
    ClientMsg, DaemonMsg, MAX_FRAME, PROTOCOL, read_frame, read_raw, write_frame, write_raw,
};
use serde::Deserialize;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::auth::{self, Device, Limiter, Pairing, Refused, Store};
use crate::tls::{self, Conn};

/// How long the daemon may take to answer `Hello`.
const SETUP: Duration = Duration::from_secs(10);
/// The WebSocket is read with this timeout so one thread can also write: a
/// daemon frame waits at most this long for its turn.
const TICK: Duration = Duration::from_millis(10);
/// Daemon frames waiting for a slow phone. When it is full the daemon
/// reader blocks, the socket fills, and the daemon's own queue limit takes
/// over (it drops and resyncs), so memory here stays bounded.
const QUEUE: usize = 64;
/// Daemon frames written per turn before the phone is read again.
const BURST: usize = 32;

/// Every wait the bridge has, so a test can make them short.
#[derive(Clone, Debug)]
pub struct Timings {
    /// How long a pairing token is good for (REQ-2).
    pub pairing_ttl: Duration,
    /// `fail_max` failed attempts within `fail_window` lock an address out
    /// for `lockout` (REQ-5).
    pub fail_max: usize,
    pub fail_window: Duration,
    pub lockout: Duration,
    /// How long a new connection has to send `pair` or `auth`, not counting
    /// the time spent waiting for the person at the PC (REQ-32).
    pub preauth: Duration,
    /// Connections that have not authenticated yet, at most.
    pub max_preauth: usize,
    /// A ping this often; a phone silent for twice as long is dropped
    /// (REQ-31).
    pub ping: Duration,
    /// How often a live connection looks its device up again (REQ-6).
    pub revoke_check: Duration,
    /// A phone that takes this long to accept bytes is gone.
    pub stalled: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            pairing_ttl: Duration::from_secs(120),
            fail_max: 5,
            fail_window: Duration::from_secs(60),
            lockout: Duration::from_secs(300),
            preauth: Duration::from_secs(10),
            max_preauth: 16,
            ping: Duration::from_secs(30),
            revoke_check: Duration::from_secs(2),
            stalled: Duration::from_secs(10),
        }
    }
}

/// What the person at the PC is asked to confirm (REQ-3): the name the phone
/// gave and the code it shows. Both are cleaned of control characters first.
pub struct Pair<'a> {
    pub name: &'a str,
    pub code: &'a str,
}

/// Answers whether the code on the phone is the one printed here.
pub type Confirm = Arc<dyn Fn(&Pair) -> bool + Send + Sync>;

/// What `ready` carries for the phone's look: an object with `palettes` and
/// `settings` (REQ-17). Asked at every `ready`, so a theme changed while the
/// bridge runs reaches the next phone. The jw binary hands it in, because
/// the themes are in the TUI crate and this one does not depend on it.
pub type Look = Arc<dyn Fn() -> serde_json::Value + Send + Sync>;

/// The colour roles of a palette (`tui/themes/schema.json`).
const ROLES: [&str; 16] = [
    "bg", "panel", "line", "fg", "dim", "sel", "blue", "green", "yellow", "red", "magenta", "cyan",
    "add_bg", "del_bg", "add_word", "del_word",
];

fn hex(v: &serde_json::Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// A palette the app can draw with: a short name, `dark`, and every role as
/// `#rrggbb`. Anything else in it is left out.
fn sane_palette(p: &serde_json::Value) -> Option<serde_json::Value> {
    let name = p
        .get("name")?
        .as_str()
        .filter(|n| !n.is_empty() && n.len() <= 64)?;
    let dark = p.get("dark")?.as_bool()?;
    let colors = p.get("colors")?.as_object()?;
    let mut kept = serde_json::Map::new();
    for role in ROLES {
        let c = colors.get(role).filter(|c| hex(c))?;
        kept.insert(role.into(), c.clone());
    }
    Some(serde_json::json!({ "name": name, "dark": dark, "colors": kept }))
}

/// What `ready` sends of the look: the palettes that fit and the two theme
/// slots as short names, or `[]` and `{}`. The callback is the jw binary's, but
/// what a phone is sent is decided here, whatever the callback returned, so a
/// theme file that parsed but is not a palette cannot reach the app (REQ-17).
fn sane_look(look: &serde_json::Value) -> (serde_json::Value, serde_json::Value) {
    let palettes: Vec<serde_json::Value> = look
        .get("palettes")
        .and_then(|p| p.as_array())
        .map(|all| all.iter().filter_map(sane_palette).take(200).collect())
        .unwrap_or_default();
    let slot = |k: &str| {
        look.get("settings")
            .and_then(|s| s.get(k))
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty() && n.len() <= 64)
            .map(|n| serde_json::Value::String(n.to_string()))
    };
    let mut settings = serde_json::Map::new();
    for k in ["dark", "light"] {
        if let Some(v) = slot(k) {
            settings.insert(k.into(), v);
        }
    }
    (
        serde_json::Value::Array(palettes),
        serde_json::Value::Object(settings),
    )
}

/// What a phone can ask the PC to list (REQ-12).
pub enum Query {
    /// The workspaces of a session, by name.
    Workspaces(String),
    /// Every session.
    Sessions,
    /// A workspace's changes in one view, by workspace id (REQ-13).
    Diff(String, DiffScope),
    /// The commits of a workspace's branch, by workspace id, for the Commits view.
    Commits(String),
    /// The entries of a directory of the PC (REQ-77), by the path the phone sent.
    Ls(String),
    /// A file's text (REQ-79), by the path the phone sent.
    Read(String),
}

/// Which changes of a workspace a phone asks for: the TUI's three views. This crate cannot depend on
/// core, so the daemon maps it to core's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffScope {
    /// Committed work against the base.
    General,
    /// Staged, unstaged and untracked.
    Uncommitted,
    /// One commit, by its hex id (7 to 40 digits, from the `commits` answer).
    Commit(String),
}

impl DiffScope {
    /// `general`, `uncommitted` or `commit:<hex>`; anything else is not a scope. The id is checked here, before
    /// it can reach git.
    fn parse(text: &str) -> Option<DiffScope> {
        match text {
            "general" => Some(DiffScope::General),
            "uncommitted" => Some(DiffScope::Uncommitted),
            _ => {
                let sha = text.strip_prefix("commit:")?;
                let ok =
                    (7..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit());
                ok.then(|| DiffScope::Commit(sha.to_ascii_lowercase()))
            }
        }
    }

    fn word(&self) -> String {
        match self {
            DiffScope::General => "general".into(),
            DiffScope::Uncommitted => "uncommitted".into(),
            DiffScope::Commit(sha) => format!("commit:{sha}"),
        }
    }
}

/// Answers a `Query` with its items, or why not. Like `Look`, the daemon hands
/// it in: it reads jw's registry and folders, which this crate does not know.
pub type Lister =
    Arc<dyn Fn(&Query, &AtomicBool) -> Result<serde_json::Value, String> + Send + Sync>;

/// Which listener a phone came in on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The loopback listener behind the tunnel.
    Tunnel = 0,
    /// The listener on the PC's private address, with TLS.
    Wifi = 1,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Tunnel => "tunnel",
            Route::Wifi => "wifi",
        }
    }
}

/// Where a phone can reach the PC besides where it is now (REQ-61).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Endpoints {
    /// The wifi listener: its address, port and certificate fingerprint.
    pub lan: Option<(String, u16, String)>,
    /// The tunnel's host, once it can be reached.
    pub tunnel: Option<String>,
}

impl Endpoints {
    fn message(&self) -> Message {
        Message::text(
            serde_json::json!({
                "t": "endpoints",
                "lan": self.lan.as_ref().map(|(host, port, fp)| {
                    serde_json::json!({ "host": host, "port": port, "fp": fp })
                }),
                "tunnel": self.tunnel,
            })
            .to_string(),
        )
    }
}

pub struct Config {
    pub store: Arc<Store>,
    /// Drop everything but looking (REQ-7).
    pub read_only: bool,
    pub confirm: Confirm,
    pub look: Look,
    pub lister: Lister,
    pub timings: Timings,
}

/// The wifi listener's own stop flag and thread. It is polled, not woken by a
/// connection to itself: when the PC's address has gone (the network changed),
/// a connect to it would leave through the routing table and might never come
/// back, and the thread that waits for it would hang the stop or the rebind.
struct Wifi {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    accept: JoinHandle<()>,
}

/// How often the wifi listener looks for a connection and for a stop.
const WIFI_POLL: Duration = Duration::from_millis(50);

/// Listening, and the connections it relays.
pub struct Bridge {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
    /// The wifi listener, while there is one.
    wifi: Mutex<Option<Wifi>>,
    daemon: PathBuf,
}

struct Shared {
    stopping: AtomicBool,
    next: AtomicU64,
    conns: Mutex<Vec<(u64, TcpStream)>>,
    store: Arc<Store>,
    /// Can change while phones are connected (REQ-37).
    read_only: AtomicBool,
    /// The devices connected now, one entry per connection.
    online: Mutex<Vec<(String, Route)>>,
    confirm: Confirm,
    look: Look,
    lister: Lister,
    t: Timings,
    pairing: Mutex<Option<Pairing>>,
    limiter: Mutex<Limiter>,
    /// Connections that have not authenticated, one count for each listener:
    /// the wifi cannot use up the seats the tunnel needs to pair.
    preauth: [AtomicUsize; 2],
    /// What `ready` is followed by, and what every phone is told again when
    /// it changes (REQ-61).
    endpoints: Mutex<(u64, Endpoints)>,
}

impl Shared {
    /// Keeps a handle on a connection so `stop` can close it. `None` once
    /// the bridge is stopping.
    fn register(&self, s: &TcpStream) -> Option<u64> {
        let mut conns = lock(&self.conns);
        if self.stopping.load(Ordering::SeqCst) {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        conns.push((id, s.try_clone().ok()?));
        Some(id)
    }

    fn forget(&self, id: u64) {
        lock(&self.conns).retain(|(i, _)| *i != id);
    }

    fn close_all(&self) {
        for (_, s) in lock(&self.conns).iter() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// A place among the connections that have not authenticated (REQ-32).
struct Slot<'a>(&'a AtomicUsize);

impl<'a> Slot<'a> {
    fn take(shared: &'a Shared, route: Route) -> Option<Self> {
        let count = &shared.preauth[route as usize];
        if count.fetch_add(1, Ordering::SeqCst) >= shared.t.max_preauth {
            count.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(count))
    }
}

/// A lock whose holder panicked is still good: what these guard (the list of
/// sockets, the pairing token, the limiter) stays valid, and a poisoned lock
/// must not panic the relay thread of every phone after it.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A device counted as connected for as long as this lives.
struct Online<'a>(&'a Shared, String, Route);

impl<'a> Online<'a> {
    fn enter(shared: &'a Shared, device: &str, route: Route) -> Self {
        shared
            .online
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((device.to_string(), route));
        Self(shared, device.to_string(), route)
    }
}

impl Drop for Online<'_> {
    fn drop(&mut self) {
        // A poisoned lock must not make a drop panic, which would abort.
        let mut online = self.0.online.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = online
            .iter()
            .position(|(d, r)| *d == self.1 && *r == self.2)
        {
            online.remove(at);
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Bridge {
    /// Relays every phone that gets in on `listener` to the daemon on
    /// `daemon`. The listener must be on a loopback address: whatever reaches
    /// the bridge reaches every pane unless it is turned away, and the daemon
    /// has no peer check of its own.
    pub fn start(listener: TcpListener, daemon: PathBuf, config: Config) -> io::Result<Self> {
        let daemon_path = daemon.clone();
        let addr = listener.local_addr()?;
        if !addr.ip().is_loopback() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "the bridge listens on loopback only",
            ));
        }
        let t = config.timings;
        let shared = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            next: AtomicU64::new(0),
            conns: Mutex::new(Vec::new()),
            store: config.store,
            read_only: AtomicBool::new(config.read_only),
            online: Mutex::new(Vec::new()),
            look: config.look,
            lister: config.lister,
            confirm: config.confirm,
            limiter: Mutex::new(Limiter::new(t.fail_max, t.fail_window, t.lockout)),
            pairing: Mutex::new(None),
            preauth: [AtomicUsize::new(0), AtomicUsize::new(0)],
            endpoints: Mutex::new((0, Endpoints::default())),
            t,
        });
        let accept = {
            let shared = shared.clone();
            thread::spawn(move || accept_loop(listener, daemon, shared))
        };
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
            wifi: Mutex::new(None),
            daemon: daemon_path,
        })
    }

    /// Also serves phones on `listener`, with TLS from `config` (REQ-56): the
    /// same gate as the tunnel's, but a peer here is who it says it is, so the
    /// limiter keys on its own address and never on `Cf-Connecting-Ip`
    /// (REQ-60). The listener must be on a private address (or loopback, which
    /// is never more exposed); anything else is refused. Replaces the wifi
    /// listener there was.
    pub fn add_wifi(
        &self,
        listener: TcpListener,
        config: Arc<ServerConfig>,
    ) -> io::Result<SocketAddr> {
        let addr = listener.local_addr()?;
        tls::check_addr(&addr)?;
        self.drop_wifi();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (shared, daemon, stop) = (self.shared.clone(), self.daemon.clone(), stop.clone());
            thread::spawn(move || accept_wifi(listener, daemon, shared, config, stop))
        };
        *lock(&self.wifi) = Some(Wifi { addr, stop, accept });
        Ok(addr)
    }

    /// Stops listening on the wifi, and waits for the accept thread, which
    /// looks at its flag every [`WIFI_POLL`]. The phones already on it are left
    /// to end on their own.
    pub fn drop_wifi(&self) {
        let Some(wifi) = lock(&self.wifi).take() else {
            return;
        };
        wifi.stop.store(true, Ordering::SeqCst);
        let _ = wifi.accept.join();
    }

    pub fn wifi_addr(&self) -> Option<SocketAddr> {
        lock(&self.wifi).as_ref().map(|w| w.addr)
    }

    /// Tells every phone connected, and every one that connects, where else
    /// it can reach the PC (REQ-61). A phone is told at its next turn.
    pub fn set_endpoints(&self, endpoints: Endpoints) {
        let mut e = lock(&self.shared.endpoints);
        if e.1 != endpoints {
            *e = (e.0 + 1, endpoints);
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A new pairing token, which ends the one before it (REQ-2).
    pub fn new_pairing(&self) -> io::Result<String> {
        let pairing = Pairing::new(Instant::now())?;
        let token = pairing.token().to_string();
        *lock(&self.shared.pairing) = Some(pairing);
        Ok(token)
    }

    /// How long the current pairing token has left; `None` when there is
    /// none, it was used, or it ended.
    pub fn pairing_left(&self) -> Option<Duration> {
        let pairing = lock(&self.shared.pairing);
        pairing
            .as_ref()?
            .left(self.shared.t.pairing_ttl, Instant::now())
    }

    /// From now on a phone can only look, or can type again (REQ-37). It
    /// applies to the phones already connected, at their next message.
    pub fn set_read_only(&self, on: bool) {
        self.shared.read_only.store(on, Ordering::SeqCst);
    }

    /// The ids of the devices connected now, once per connection.
    pub fn connected(&self) -> Vec<String> {
        self.connected_routes()
            .into_iter()
            .map(|(d, _)| d)
            .collect()
    }

    /// The same, with the listener each came in on (REQ-65).
    pub fn connected_routes(&self) -> Vec<(String, Route)> {
        self.shared
            .online
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Closes every phone's socket and refuses new ones, without waiting for
    /// the threads: what has to happen before anything slow (REQ-9).
    pub fn close(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // The phones go first (REQ-9): nothing below waits on the network.
        self.shared.close_all();
        // The tunnel's `accept` blocks, and a connection to ourselves (loopback,
        // so it always arrives) wakes it. The wifi listener polls its flag.
        let _ = TcpStream::connect(self.addr);
    }

    /// Closes every connection and returns once their threads are done
    /// (REQ-9). Stopping twice is fine.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.close();
        let _ = accept.join();
        if let Some(wifi) = lock(&self.wifi).take() {
            wifi.stop.store(true, Ordering::SeqCst);
            let _ = wifi.accept.join();
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, daemon: PathBuf, shared: Arc<Shared>) {
    let mut conns: Vec<JoinHandle<()>> = Vec::new();
    for conn in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = conn else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        conns.retain(|h| !h.is_finished());
        conns.push(spawn_serve(stream, &daemon, &shared, Route::Tunnel, None));
    }
    shared.close_all();
    for h in conns {
        let _ = h.join();
    }
}

/// The wifi listener's accept loop: it polls, so a stop (the bridge's or this
/// listener's own) is seen within [`WIFI_POLL`] whatever the network is doing.
fn accept_wifi(
    listener: TcpListener,
    daemon: PathBuf,
    shared: Arc<Shared>,
    tls: Arc<ServerConfig>,
    stop: Arc<AtomicBool>,
) {
    let mut conns: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::SeqCst) && !shared.stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Where accepted sockets inherit it (macOS), non-blocking would
                // make every read and its timeout mean something else.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                conns.retain(|h| !h.is_finished());
                conns.push(spawn_serve(
                    stream,
                    &daemon,
                    &shared,
                    Route::Wifi,
                    Some(tls.clone()),
                ));
            }
            // Nothing to accept yet, or a failure that passes: look again after a poll.
            Err(_) => thread::sleep(WIFI_POLL),
        }
    }
    // Only the bridge stopping ends the phones on it; dropping this listener
    // leaves them alone.
    if shared.stopping.load(Ordering::SeqCst) {
        shared.close_all();
        for h in conns {
            let _ = h.join();
        }
    }
}

fn spawn_serve(
    stream: TcpStream,
    daemon: &Path,
    shared: &Arc<Shared>,
    route: Route,
    tls: Option<Arc<ServerConfig>>,
) -> JoinHandle<()> {
    let (daemon, shared) = (daemon.to_path_buf(), shared.clone());
    thread::spawn(move || {
        let Some(id) = shared.register(&stream) else {
            return;
        };
        let _ = serve(stream, &shared, &daemon, route, tls);
        shared.forget(id);
    })
}

/// What a phone sends to be let in.
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Control {
    Pair {
        token: String,
        name: String,
        code: String,
    },
    Auth {
        device: String,
    },
}

/// What the daemon reader hands to the thread that owns the WebSocket.
enum Event {
    Frame(Vec<u8>),
    Closed,
}

/// Answers computed on another thread, for the relay to write: a list reads
/// files and runs git, a diff more, and neither may stop the pings or the
/// frames meanwhile (REQ-40).
struct Later {
    tx: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    /// A diff is being computed for this phone: one at a time.
    diff: Arc<AtomicBool>,
    /// Lists being computed for this phone, at most `MAX_LISTS`.
    lists: Arc<AtomicUsize>,
    /// Listings and reads of the PC's files, at most `MAX_FILES`: a hung mount must not hold the other asks.
    files: Arc<AtomicUsize>,
    /// Set when this phone is gone: what is still being computed for it
    /// stops (a diff runs git, which would go on for its whole deadline).
    gone: Arc<AtomicBool>,
}

/// How many `workspaces` and `sessions` a phone can have in flight.
const MAX_LISTS: usize = 4;

/// How many `ls` and `read` a phone can have in flight.
const MAX_FILES: usize = 2;

fn serve(
    stream: TcpStream,
    shared: &Shared,
    socket: &Path,
    route: Route,
    tls: Option<Arc<ServerConfig>>,
) -> Result<(), WsError> {
    let t = &shared.t;
    let Some(slot) = Slot::take(shared, route) else {
        return Ok(());
    };
    let mut conn = match tls {
        Some(config) => Conn::tls(stream, config)?,
        None => Conn::plain(stream),
    };
    let peer = conn.peer_ip()?.to_string();
    conn.set_read_timeout(Some(t.preauth))?;
    conn.set_write_timeout(Some(t.stalled))?;
    // The whole way in, TLS and HTTP included, takes at most `preauth`, however
    // slowly the bytes come: a read's own timeout starts over with each byte.
    conn.deadline(Instant::now() + t.preauth);
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME as usize))
        .max_frame_size(Some(MAX_FRAME as usize));

    // A browser sends `Origin`, a phone app does not (REQ-29). Behind
    // `cloudflared` every connection is from loopback and the tunnel says
    // who is really there (REQ-5).
    let mut via: Option<String> = None;
    // The error type is tungstenite's `Callback`, not ours to shrink.
    #[allow(clippy::result_large_err)]
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if req.headers().contains_key("origin") {
            return Err(tungstenite::http::Response::builder()
                .status(403)
                .body(Some("not for browsers".to_string()))
                .unwrap());
        }
        via = req
            .headers()
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(64).collect());
        Ok(resp)
    };
    let mut ws = tungstenite::accept_hdr_with_config(conn, check, Some(config))
        .map_err(|e| WsError::Io(io::Error::other(e.to_string())))?;
    // `Cf-Connecting-Ip` is the tunnel's word about who is there. On the wifi
    // anyone can send it, so the peer's own address is the key (REQ-60), by
    // the listener the phone came in on and not by what the address looks like.
    let key = match route {
        Route::Tunnel => via.unwrap_or(peer),
        Route::Wifi => peer,
    };

    let Some(device) = authenticate(&mut ws, shared, &key)? else {
        return Ok(());
    };
    drop(slot);

    // Only now does anything reach the daemon (REQ-4).
    let mut daemon = match connect(socket) {
        Ok(d) => d,
        Err(refusal) => return refuse_daemon(&mut ws, refusal),
    };
    ws.get_mut().no_deadline();
    ws.get_mut().set_read_timeout(Some(TICK))?;
    let _online = Online::enter(shared, &device.id, route);
    let (palettes, settings) = sane_look(&(shared.look)());
    ws.send(Message::text(
        serde_json::json!({
            "t": "ready",
            "bridge": 1,
            "protocol": PROTOCOL,
            "read_only": shared.read_only.load(Ordering::SeqCst),
            // A callback that left a key out gives the app an empty one, not
            // a `null` it would have to guard against.
            "palettes": palettes,
            "settings": settings,
        })
        .to_string(),
    ))?;
    // Cloned out of the lock before the send: a phone slow to read would otherwise hold
    // it, and every other phone's relay looks at it each turn.
    let (mut seen, first) = {
        let e = lock(&shared.endpoints);
        (e.0, e.1.message())
    };
    ws.send(first)?;

    let (tx, rx) = mpsc::sync_channel(QUEUE);
    let mut from_daemon = daemon.try_clone()?;
    let reader = thread::spawn(move || {
        // A frame read in pieces would be lost if this stopped on a timeout,
        // so the socket is read blocking and `serve` closes it to end this.
        while let Ok(Some(body)) = read_raw(&mut from_daemon) {
            if tx.send(Event::Frame(body)).is_err() {
                return;
            }
        }
        let _ = tx.send(Event::Closed);
    });

    let (reply_tx, replies) = mpsc::channel();
    let later = Later {
        tx: reply_tx,
        rx: replies,
        diff: Arc::new(AtomicBool::new(false)),
        lists: Arc::new(AtomicUsize::new(0)),
        files: Arc::new(AtomicUsize::new(0)),
        gone: Arc::new(AtomicBool::new(false)),
    };
    let result = relay(
        &mut ws,
        &mut daemon,
        &rx,
        &later,
        shared,
        &device,
        &mut seen,
    );
    later.gone.store(true, Ordering::SeqCst);

    let _ = daemon.shutdown(Shutdown::Both);
    let _ = ws.get_ref().shutdown();
    drop(rx);
    let _ = reader.join();
    result
}

/// Reads the first message and lets the phone in or turns it away (REQ-3 to
/// 5). `None` means it was turned away and told why.
fn authenticate(
    ws: &mut WebSocket<Conn>,
    shared: &Shared,
    key: &str,
) -> Result<Option<Device>, WsError> {
    let deadline = ws
        .get_ref()
        .deadline_at()
        .unwrap_or_else(|| Instant::now() + shared.t.preauth);
    if lock(&shared.limiter).locked(key, Instant::now()) {
        refuse(ws, "limited", "too many failed attempts, try again later")?;
        return Ok(None);
    }
    let text = loop {
        if Instant::now() >= deadline {
            refuse(ws, "timeout", "send pair or auth first")?;
            return Ok(None);
        }
        match ws.read() {
            Ok(Message::Text(text)) => break text,
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_)) => return Ok(None),
            Ok(_) => {
                refuse(ws, "unexpected", "pair or auth first")?;
                return Ok(None);
            }
            Err(WsError::Io(e)) if idle(&e) => {}
            Err(_) => return Ok(None),
        }
    };

    let failed = |why: &str, msg: &str, ws: &mut WebSocket<Conn>| {
        lock(&shared.limiter).failed(key, Instant::now());
        refuse(ws, why, msg)
    };
    let Ok(control) = serde_json::from_str::<Control>(&text) else {
        failed("unexpected", "pair or auth first", ws)?;
        return Ok(None);
    };
    match control {
        Control::Auth { device } => match shared.store.find(&auth::hash(&device))? {
            Some(d) => {
                lock(&shared.limiter).succeeded(key);
                shared.store.touch(&d.id)?;
                Ok(Some(d))
            }
            None => {
                failed("unknown", "this device is not paired", ws)?;
                Ok(None)
            }
        },
        Control::Pair { token, name, code } => {
            let name = clean(&name, 40);
            let name = if name.is_empty() {
                "phone".into()
            } else {
                name
            };
            if !(4..=12).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_digit()) {
                failed("unexpected", "the code is 4 to 12 digits", ws)?;
                return Ok(None);
            }
            let taken = match lock(&shared.pairing).as_mut() {
                Some(p) => p.take(&token, shared.t.pairing_ttl, Instant::now()),
                None => Err(Refused::Unknown),
            };
            match taken {
                Err(Refused::Unknown) => {
                    failed("unknown", "that code is not for this PC", ws)?;
                    return Ok(None);
                }
                Err(Refused::Expired) => {
                    failed("expired", "that code was used or is too old", ws)?;
                    return Ok(None);
                }
                Ok(()) => {}
            }
            if !(shared.confirm)(&Pair {
                name: &name,
                code: &code,
            }) {
                failed("denied", "the PC refused this phone", ws)?;
                return Ok(None);
            }
            let secret = auth::random_hex(32)?;
            let device = shared.store.add(&name, &auth::hash(&secret))?;
            lock(&shared.limiter).succeeded(key);
            ws.send(Message::text(
                serde_json::json!({ "t": "paired", "device": secret, "id": device.id }).to_string(),
            ))?;
            Ok(Some(device))
        }
    }
}

/// Phone to daemon and daemon to phone, until either side ends.
fn relay(
    ws: &mut WebSocket<Conn>,
    daemon: &mut UnixStream,
    rx: &mpsc::Receiver<Event>,
    later: &Later,
    shared: &Shared,
    device: &Device,
    seen: &mut u64,
) -> Result<(), WsError> {
    let t = &shared.t;
    let start = Instant::now();
    let (mut heard, mut pinged, mut looked) = (start, start, start);
    loop {
        for _ in 0..BURST {
            match rx.try_recv() {
                Ok(Event::Frame(body)) => ws.write(Message::binary(body))?,
                Ok(Event::Closed) => {
                    return refuse_daemon(ws, Refusal::Daemon("the daemon hung up".into()));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        while let Ok(reply) = later.rx.try_recv() {
            ws.write(reply)?;
        }
        // Where else the PC can be reached changed (REQ-61).
        let changed = {
            let e = lock(&shared.endpoints);
            (e.0 != *seen).then(|| (e.0, e.1.clone()))
        };
        if let Some((epoch, endpoints)) = changed {
            *seen = epoch;
            ws.write(endpoints.message())?;
        }
        let now = Instant::now();
        if now.duration_since(looked) >= t.revoke_check {
            looked = now;
            if matches!(shared.store.find(&device.hash), Ok(None)) {
                return refuse(ws, "revoked", "this device was revoked");
            }
        }
        if now.duration_since(heard) > t.ping * 2 {
            return Ok(());
        }
        if now.duration_since(pinged) >= t.ping {
            pinged = now;
            ws.write(Message::Ping(Vec::new().into()))?;
        }
        // Writes block up to `stalled`, so a flush that times out is a phone
        // that stopped reading, not an idle one.
        ws.flush()?;
        match ws.read() {
            Ok(message) => {
                heard = Instant::now();
                match message {
                    Message::Binary(body) if mobile_control(&body) => {
                        ws.send(error("unsupported", "mobile is controlled at the PC"))?;
                    }
                    Message::Binary(body)
                        if shared.read_only.load(Ordering::SeqCst) && !looking(&body) =>
                    {
                        ws.send(error("read_only", "this bridge is read-only"))?;
                    }
                    Message::Binary(body) => write_raw(daemon, &body)?,
                    Message::Text(text) => {
                        let refusal = match serde_json::from_str::<Ask>(&text) {
                            Ok(ask) => start_ask(shared, later, ask),
                            Err(_) => Some(error(
                                "unsupported",
                                "ask workspaces, sessions, diff, commits, ls or read",
                            )),
                        };
                        if let Some(refusal) = refusal {
                            ws.send(refusal)?;
                        }
                    }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            Err(WsError::Io(e)) if idle(&e) => {}
            Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// What a phone may ask in the control channel once it is in.
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Ask {
    Workspaces {
        session: Option<String>,
    },
    Sessions,
    /// `scope`: `general` (the default), `uncommitted` or `commit:<hex>`.
    Diff {
        ws: String,
        scope: Option<String>,
    },
    Commits {
        ws: String,
    },
    Ls {
        path: String,
    },
    Read {
        path: String,
    },
}

/// Which of a phone's seats an answer takes.
enum Seat {
    Diff(Arc<AtomicBool>),
    /// A counter of asks in flight and how many are allowed.
    List(Arc<AtomicUsize>, usize),
}

impl Seat {
    /// Takes the seat, or says it is full.
    fn take(&self) -> bool {
        match self {
            Seat::Diff(busy) => !busy.swap(true, Ordering::SeqCst),
            Seat::List(n, cap) => {
                if n.fetch_add(1, Ordering::SeqCst) >= *cap {
                    n.fetch_sub(1, Ordering::SeqCst);
                    return false;
                }
                true
            }
        }
    }
}

/// Gives the seat back when the thread ends, however it ends: a lister that
/// panics must not leave every later ask refused.
struct Release(Seat);

impl Drop for Release {
    fn drop(&mut self) {
        match &self.0 {
            Seat::Diff(busy) => busy.store(false, Ordering::SeqCst),
            Seat::List(n, _) => {
                n.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

/// Starts what a phone asked on a thread of its own; the answer comes back
/// through `later`, with `t` and the ask's own fields in it. `Some` is the
/// refusal to send now, because that kind has its seats full.
fn start_ask(shared: &Shared, later: &Later, ask: Ask) -> Option<Message> {
    // The ask's own fields, echoed in its answer and in its error, so the phone can tell which ask an answer is for.
    let (query, tag, fields): (Query, &'static str, Vec<(&'static str, String)>) = match ask {
        Ask::Workspaces { session } => {
            let session = session.unwrap_or_else(|| "main".into());
            (
                Query::Workspaces(session.clone()),
                "workspaces",
                vec![("session", session)],
            )
        }
        Ask::Sessions => (Query::Sessions, "sessions", Vec::new()),
        Ask::Diff { ws, scope } => {
            let word = scope.unwrap_or_else(|| "general".into());
            let Some(parsed) = DiffScope::parse(&word) else {
                return Some(error_for(
                    "diff",
                    &[("ws", ws)],
                    "unsupported",
                    "scope is general, uncommitted or commit:<hex id>",
                ));
            };
            (
                Query::Diff(ws.clone(), parsed.clone()),
                "diff",
                vec![("ws", ws), ("scope", parsed.word())],
            )
        }
        Ask::Commits { ws } => (Query::Commits(ws.clone()), "commits", vec![("ws", ws)]),
        // `req` is the path as the phone sent it: the answer's own `path` is the resolved one.
        Ask::Ls { path } => (Query::Ls(path.clone()), "ls", vec![("req", path)]),
        Ask::Read { path } => (Query::Read(path.clone()), "read", vec![("req", path)]),
    };
    let seat = match query {
        Query::Diff(..) | Query::Commits(_) => Seat::Diff(later.diff.clone()),
        Query::Ls(_) | Query::Read(_) => Seat::List(later.files.clone(), MAX_FILES),
        _ => Seat::List(later.lists.clone(), MAX_LISTS),
    };
    if !seat.take() {
        return Some(error_for(
            tag,
            &fields,
            "unsupported",
            "busy: wait for the answer before asking again",
        ));
    }
    let (lister, tx, gone) = (shared.lister.clone(), later.tx.clone(), later.gone.clone());
    thread::spawn(move || {
        let _release = Release(seat);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lister(&query, &gone)));
        let reply = match result {
            Ok(Ok(serde_json::Value::Object(mut obj))) => {
                obj.insert("t".into(), tag.into());
                for (k, v) in fields {
                    obj.insert(k.into(), v.into());
                }
                Message::text(serde_json::Value::Object(obj).to_string())
            }
            Ok(Ok(_)) => error_for(tag, &fields, "unsupported", "the answer is not an object"),
            Ok(Err(why)) => error_for(tag, &fields, "unsupported", &why),
            Err(_) => error_for(tag, &fields, "unsupported", "that could not be answered"),
        };
        let _ = tx.send(reply);
    });
    None
}

/// The tag of a frame: the `t` of its JSON head, for both kinds of frame.
fn tag_of(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Tag {
        t: String,
    }
    let head = match body.first() {
        Some(0) => &body[1..],
        Some(1) => {
            let n = body.get(1..5)?;
            let n = u32::from_be_bytes([n[0], n[1], n[2], n[3]]) as usize;
            body.get(5..5 + n)?
        }
        _ => return None,
    };
    serde_json::from_slice::<Tag>(head).ok().map(|t| t.t)
}

/// Whether a frame only looks (REQ-7). It is decided on the message's own
/// tag, for both kinds of frame, and anything else is refused, so a message
/// added later is refused until it is allowed here.
fn looking(body: &[u8]) -> bool {
    matches!(
        tag_of(body).as_deref(),
        Some("Hello" | "Attach" | "Detach" | "List")
    )
}

/// Whether a frame drives mobile itself (`MobileWatch`, `MobileAnswer`,
/// `MobileRevoke`…): that is for whoever is at the PC. A phone that got in
/// could otherwise watch the pairing code, answer its own questions or stop
/// the tunnel under the others.
fn mobile_control(body: &[u8]) -> bool {
    tag_of(body).is_some_and(|t| t.starts_with("Mobile"))
}

fn idle(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Why a phone is turned away after it got in.
enum Refusal {
    /// There is no daemon, or it did not answer.
    Daemon(String),
    /// The daemon speaks another protocol than this bridge.
    Protocol(u32),
}

/// Connects to a daemon that is already running (the bridge never starts
/// one) and checks its protocol, which the daemon does not (REQ-8). The
/// answer to this `Hello` is not forwarded; the phone sends its own.
fn connect(socket: &Path) -> Result<UnixStream, Refusal> {
    let gone = |e: &dyn std::fmt::Display| Refusal::Daemon(format!("no daemon on the socket: {e}"));
    let mut daemon = UnixStream::connect(socket).map_err(|e| gone(&e))?;
    daemon.set_read_timeout(Some(SETUP)).map_err(|e| gone(&e))?;
    write_frame(&mut daemon, &ClientMsg::Hello { protocol: PROTOCOL }).map_err(|e| gone(&e))?;
    loop {
        match read_frame::<DaemonMsg>(&mut daemon).map_err(|e| gone(&e))? {
            Some(DaemonMsg::Hello { protocol }) if protocol == PROTOCOL => break,
            Some(DaemonMsg::Hello { protocol }) => return Err(Refusal::Protocol(protocol)),
            Some(_) => {}
            None => return Err(gone(&"it hung up")),
        }
    }
    daemon.set_read_timeout(None).map_err(|e| gone(&e))?;
    Ok(daemon)
}

fn refuse_daemon(ws: &mut WebSocket<Conn>, why: Refusal) -> Result<(), WsError> {
    match why {
        Refusal::Daemon(m) => refuse(ws, "daemon", &m),
        Refusal::Protocol(p) => refuse(
            ws,
            "protocol",
            &format!("the daemon speaks protocol {p}, this bridge {PROTOCOL}"),
        ),
    }
}

/// Tells the phone why, and closes.
fn refuse(ws: &mut WebSocket<Conn>, code: &str, msg: &str) -> Result<(), WsError> {
    let _ = ws.send(error(code, msg));
    let _ = ws.close(None);
    let _ = ws.flush();
    Ok(())
}

/// An error that says which ask it answers (`ask` and the ask's own fields, additive to `error`'s): an
/// error has no request id, and a phone with a diff and a commit list in flight needs to know whose it is.
fn error_for(ask: &str, fields: &[(&str, String)], code: &str, msg: &str) -> Message {
    let mut v = serde_json::json!({ "t": "error", "code": code, "msg": msg, "ask": ask });
    for (k, val) in fields {
        v[*k] = val.clone().into();
    }
    Message::text(v.to_string())
}

fn error(code: &str, msg: &str) -> Message {
    Message::text(serde_json::json!({ "t": "error", "code": code, "msg": msg }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicUsize;
    use tungstenite::client::IntoClientRequest;

    /// A phone's end of a connection: plain (the tunnel's listener) or in TLS (the wifi's).
    enum Wire {
        Plain(TcpStream),
        Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    }

    impl Wire {
        fn sock(&self) -> &TcpStream {
            match self {
                Wire::Plain(s) => s,
                Wire::Tls(t) => &t.sock,
            }
        }
        fn set_read_timeout(&self, to: Option<Duration>) -> io::Result<()> {
            self.sock().set_read_timeout(to)
        }
    }

    impl io::Read for Wire {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self {
                Wire::Plain(s) => io::Read::read(s, buf),
                Wire::Tls(t) => io::Read::read(t, buf),
            }
        }
    }

    impl io::Write for Wire {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self {
                Wire::Plain(s) => io::Write::write(s, buf),
                Wire::Tls(t) => io::Write::write(t, buf),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            match self {
                Wire::Plain(s) => io::Write::flush(s),
                Wire::Tls(t) => io::Write::flush(t),
            }
        }
    }

    type Phone = WebSocket<Wire>;

    /// An `Input` for this pane is answered with far more output than a socket holds.
    const FLOOD: u64 = 99;

    struct Opts {
        protocol: u32,
        hang_up: bool,
        no_daemon: bool,
        read_only: bool,
        timings: Timings,
        confirm: Confirm,
        look: Look,
        lister: Lister,
    }

    impl Default for Opts {
        fn default() -> Self {
            Self {
                protocol: PROTOCOL,
                hang_up: false,
                no_daemon: false,
                read_only: false,
                timings: Timings {
                    fail_max: 3,
                    revoke_check: Duration::from_millis(50),
                    stalled: Duration::from_millis(300),
                    ..Timings::default()
                },
                confirm: Arc::new(|_| true),
                look: Arc::new(|| serde_json::json!({"palettes": [], "settings": {}})),
                lister: Arc::new(|q, cancel| match q {
                    Query::Workspaces(s) if s == "nope" => Err("no session named nope".into()),
                    Query::Workspaces(s) if s == "boom" => panic!("a bug in the lister"),
                    Query::Workspaces(s) => {
                        if s == "slow" {
                            thread::sleep(Duration::from_millis(700));
                        }
                        Ok(serde_json::json!({
                            "items": [{ "id": "w1", "session": s }],
                            "truncated": false,
                        }))
                    }
                    Query::Sessions => {
                        Ok(serde_json::json!({ "items": [{ "name": "main", "workspaces": 1 }] }))
                    }
                    Query::Diff(id, _) | Query::Commits(id) if id == "boom" => {
                        panic!("a bug in the diff")
                    }
                    Query::Diff(id, _) if id == "hang" => {
                        // Works until its phone is gone, like git would.
                        while !cancel.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(10));
                        }
                        WENT.store(true, Ordering::SeqCst);
                        Err("cancelled".into())
                    }
                    Query::Diff(id, _) | Query::Commits(id) if id == "nope" => {
                        Err("no workspace nope".into())
                    }
                    Query::Commits(_) => {
                        Ok(serde_json::json!({ "commits": [], "more": false, "no_base": false }))
                    }
                    Query::Ls(path) if path == "/hang" => {
                        while !cancel.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err("cancelled".into())
                    }
                    Query::Ls(path) if path == "/nope" => Err("no such file or folder".into()),
                    Query::Ls(path) => Ok(
                        serde_json::json!({ "path": path, "parent": null, "entries": [], "total": 0, "truncated": false }),
                    ),
                    Query::Read(path) => Ok(
                        serde_json::json!({ "path": path, "kind": "text", "size": 2, "truncated": false, "text": "hi", "blocks": null }),
                    ),
                    Query::Diff(id, _) => {
                        if id == "slow" {
                            thread::sleep(Duration::from_millis(700));
                        }
                        Ok(
                            serde_json::json!({ "title": format!("{id} vs main"), "truncated": false, "files": [] }),
                        )
                    }
                }),
            }
        }
    }

    struct Rig {
        bridge: Bridge,
        store: Arc<Store>,
        accepted: Arc<AtomicUsize>,
        socket: PathBuf,
        dir: tempfile::TempDir,
    }

    fn rig(o: Opts) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("jw.sock");
        let accepted = Arc::new(AtomicUsize::new(0));
        if !o.no_daemon {
            fake_daemon(&socket, o.protocol, o.hang_up, accepted.clone());
        }
        let store = Arc::new(Store::open(dir.path().join("mobile.json")));
        let bridge = Bridge::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            socket.clone(),
            Config {
                store: store.clone(),
                read_only: o.read_only,
                confirm: o.confirm,
                look: o.look,
                lister: o.lister,
                timings: o.timings,
            },
        )
        .unwrap();
        Rig {
            bridge,
            store,
            accepted,
            socket,
            dir,
        }
    }

    /// A daemon that answers `Hello` with `protocol` and echoes `Input` as
    /// `Output`; with `hang_up` it closes right after its answer. It counts
    /// the connections it takes.
    fn fake_daemon(socket: &Path, protocol: u32, hang_up: bool, accepted: Arc<AtomicUsize>) {
        let listener = UnixListener::bind(socket).unwrap();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { return };
                accepted.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    let Ok(Some(ClientMsg::Hello { .. })) = read_frame::<ClientMsg>(&mut c) else {
                        return;
                    };
                    write_frame(&mut c, &DaemonMsg::Hello { protocol }).unwrap();
                    if hang_up {
                        return;
                    }
                    while let Ok(Some(m)) = read_frame::<ClientMsg>(&mut c) {
                        let reply = match m {
                            ClientMsg::Hello { .. } => DaemonMsg::Hello { protocol },
                            ClientMsg::Input { pane, .. } if pane == FLOOD => {
                                for _ in 0..256 {
                                    let big = DaemonMsg::Output {
                                        pane,
                                        bytes: vec![b'x'; 256 << 10],
                                    };
                                    if write_frame(&mut c, &big).is_err() {
                                        return;
                                    }
                                }
                                continue;
                            }
                            ClientMsg::Input { pane, bytes } => DaemonMsg::Output { pane, bytes },
                            _ => continue,
                        };
                        if write_frame(&mut c, &reply).is_err() {
                            return;
                        }
                    }
                });
            }
        });
    }

    fn open(b: &Bridge, headers: &[(&str, &str)]) -> Result<Phone, String> {
        let s = TcpStream::connect(b.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut req = format!("ws://{}", b.addr).into_client_request().unwrap();
        for (k, v) in headers {
            let name = tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
            req.headers_mut().insert(name, v.parse().unwrap());
        }
        tungstenite::client(req, Wire::Plain(s))
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }

    fn phone(b: &Bridge) -> Phone {
        open(b, &[]).unwrap()
    }

    /// A device already paired: its token.
    fn enrol(r: &Rig, name: &str) -> String {
        let secret = auth::random_hex(32).unwrap();
        r.store.add(name, &auth::hash(&secret)).unwrap();
        secret
    }

    /// The next message that is not a ping or a pong.
    fn next(p: &mut Phone) -> Message {
        loop {
            match p.read().unwrap() {
                Message::Ping(_) | Message::Pong(_) => {}
                other => return other,
            }
        }
    }

    fn json(p: &mut Phone) -> serde_json::Value {
        match next(p) {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("want a control message, got {other:?}"),
        }
    }

    fn code(p: &mut Phone) -> String {
        let v = json(p);
        assert_eq!(v["t"], "error", "{v}");
        v["code"].as_str().unwrap().to_string()
    }

    fn say(p: &mut Phone, v: serde_json::Value) {
        p.send(Message::text(v.to_string())).unwrap();
    }

    /// Authenticates and reads `ready`.
    fn login(p: &mut Phone, secret: &str) {
        say(p, serde_json::json!({ "t": "auth", "device": secret }));
        let v = json(p);
        assert_eq!(v["t"], "ready", "{v}");
        // What follows `ready` is always `endpoints` (REQ-61).
        let e = json(p);
        assert_eq!(e["t"], "endpoints", "{e}");
    }

    fn pair(p: &mut Phone, token: &str, name: &str, code: &str) {
        say(
            p,
            serde_json::json!({ "t": "pair", "token": token, "name": name, "code": code }),
        );
    }

    /// A message as a phone sends it: a frame without its length.
    fn body(msg: &ClientMsg) -> Message {
        let mut b = Vec::new();
        write_frame(&mut b, msg).unwrap();
        Message::binary(b[4..].to_vec())
    }

    fn msg(p: &mut Phone) -> DaemonMsg {
        match next(p) {
            Message::Binary(b) => {
                let mut f = (b.len() as u32).to_be_bytes().to_vec();
                f.extend_from_slice(&b);
                read_frame(&mut f.as_slice()).unwrap().unwrap()
            }
            other => panic!("want a frame, got {other:?}"),
        }
    }

    /// The connection ends: a close or a reset, but not a mere silence.
    fn closed(p: &mut Phone) {
        loop {
            match p.read() {
                Ok(Message::Close(_)) => return,
                Ok(_) => {}
                Err(WsError::Io(e)) if idle(&e) => panic!("the connection is still open"),
                Err(_) => return,
            }
        }
    }

    fn input(pane: u64, bytes: &[u8]) -> ClientMsg {
        ClientMsg::Input {
            pane,
            bytes: bytes.to_vec(),
        }
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "never: {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn relays_frames_both_ways_untouched() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "Pixel 8"));
        // The first thing back is this Output, so the answer to the bridge's
        // own Hello was not forwarded.
        let bytes: Vec<u8> = (0..=255).collect();
        p.send(body(&input(4, &bytes))).unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Output { pane: 4, bytes });
        p.send(body(&ClientMsg::Hello { protocol: PROTOCOL }))
            .unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Hello { protocol: PROTOCOL });
    }

    #[test]
    fn nothing_reaches_the_daemon_before_auth() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        p.send(body(&ClientMsg::Hello { protocol: PROTOCOL }))
            .unwrap();
        assert_eq!(code(&mut p), "unexpected");
        closed(&mut p);
        let mut p = phone(&r.bridge);
        say(&mut p, serde_json::json!({ "t": "workspaces" }));
        assert_eq!(code(&mut p), "unexpected");
        closed(&mut p);
        let mut p = phone(&r.bridge);
        say(
            &mut p,
            serde_json::json!({ "t": "auth", "device": "not paired" }),
        );
        assert_eq!(code(&mut p), "unknown");
        closed(&mut p);
        assert_eq!(r.accepted.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_device_that_paired_gets_in_with_its_token() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        let v = {
            say(
                &mut p,
                serde_json::json!({ "t": "auth", "device": enrol(&r, "a") }),
            );
            json(&mut p)
        };
        assert_eq!(v["t"], "ready");
        assert_eq!(v["protocol"], PROTOCOL);
        assert_eq!(v["read_only"], false);
    }

    #[test]
    fn ready_carries_the_look_asked_at_that_moment() {
        let n = Arc::new(AtomicUsize::new(0));
        let count = n.clone();
        let r = rig(Opts {
            look: Arc::new(move || {
                let n = count.fetch_add(1, Ordering::SeqCst);
                serde_json::json!({
                    "palettes": [palette(&format!("t{n}"))],
                    "settings": {"dark": format!("t{n}"), "light": "one-light"},
                })
            }),
            ..Opts::default()
        });
        let device = enrol(&r, "a");
        for n in 0..2 {
            let mut p = phone(&r.bridge);
            say(&mut p, serde_json::json!({ "t": "auth", "device": device }));
            let v = json(&mut p);
            assert_eq!(v["t"], "ready", "{v}");
            assert_eq!(v["palettes"][0]["name"], format!("t{n}"));
            assert_eq!(v["settings"]["dark"], format!("t{n}"));
            assert_eq!(v["settings"]["light"], "one-light");
        }
    }

    fn palette(name: &str) -> serde_json::Value {
        let colors: serde_json::Map<String, serde_json::Value> = ROLES
            .iter()
            .map(|r| (r.to_string(), "#1a2b3c".into()))
            .collect();
        serde_json::json!({ "name": name, "dark": true, "colors": colors })
    }

    #[test]
    fn only_a_palette_the_app_can_draw_with_is_sent() {
        let mut no_role = palette("no-role");
        no_role["colors"].as_object_mut().unwrap().remove("cyan");
        let mut bad_hex = palette("bad-hex");
        bad_hex["colors"]["bg"] = "red".into();
        let mut extra = palette("extra");
        extra["note"] = "ignored".into();
        extra["colors"]["unknown"] = "#ffffff".into();
        let look = serde_json::json!({
            "palettes": [
                palette("good"), no_role, bad_hex, extra,
                { "name": "", "dark": true, "colors": {} },
                { "name": "x".repeat(65), "dark": false, "colors": palette("a")["colors"] },
                { "name": "no-dark", "colors": palette("a")["colors"] },
                "not an object", 7, null,
            ],
            "settings": { "dark": "good", "light": 7, "mode": "ignored" },
        });
        let (palettes, settings) = sane_look(&look);
        let names: Vec<&str> = palettes
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["good", "extra"], "{palettes}");
        // What is left of `extra` is the palette and nothing else.
        let extra = &palettes[1];
        assert!(
            extra.get("note").is_none() && extra["colors"].get("unknown").is_none(),
            "{extra}"
        );
        assert_eq!(settings, serde_json::json!({ "dark": "good" }));
        // Not even the right types: empty, never null.
        let (p, s) = sane_look(&serde_json::json!({ "palettes": "x", "settings": [1] }));
        assert_eq!((p, s), (serde_json::json!([]), serde_json::json!({})));
    }

    #[test]
    fn a_poisoned_lock_does_not_panic_the_phones_after_it() {
        let r = rig(Opts::default());
        // A thread that panics holding the limiter's and the pairing's locks.
        let shared = r.bridge.shared.clone();
        let _ = thread::spawn(move || {
            let _limiter = shared.limiter.lock().unwrap();
            let _pairing = shared.pairing.lock().unwrap();
            let _conns = shared.conns.lock().unwrap();
            panic!("a bug while holding the locks");
        })
        .join();
        assert!(r.bridge.shared.limiter.is_poisoned());
        // A device still gets in, and a new code still works.
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        assert!(r.bridge.new_pairing().is_ok());
        assert!(r.bridge.pairing_left().is_some());
    }

    #[test]
    fn a_look_without_its_keys_is_an_empty_one_not_null() {
        let r = rig(Opts {
            look: Arc::new(|| serde_json::json!({})),
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        // `login` read the ready; ask again with a second phone to see it whole.
        let mut q = phone(&r.bridge);
        say(
            &mut q,
            serde_json::json!({ "t": "auth", "device": enrol(&r, "b") }),
        );
        let v = json(&mut q);
        assert_eq!(v["palettes"], serde_json::json!([]), "{v}");
        assert_eq!(v["settings"], serde_json::json!({}), "{v}");
    }

    #[test]
    fn pairing_asks_the_pc_and_hands_out_a_token_it_keeps_only_hashed() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let r = rig(Opts {
            confirm: Arc::new(move |p| {
                log.lock()
                    .unwrap()
                    .push((p.name.to_string(), p.code.to_string()));
                true
            }),
            ..Opts::default()
        });
        let token = r.bridge.new_pairing().unwrap();
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "Pixel 8", "482913");
        let paid = json(&mut p);
        assert_eq!(paid["t"], "paired");
        assert_eq!(json(&mut p)["t"], "ready");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("Pixel 8".to_string(), "482913".to_string())]
        );
        let secret = paid["device"].as_str().unwrap();
        assert_eq!(secret.len(), 64);
        let file = std::fs::read_to_string(r.dir.path().join("mobile.json")).unwrap();
        assert!(!file.contains(secret), "the token itself was stored");
        let mut again = phone(&r.bridge);
        login(&mut again, secret);
    }

    #[test]
    fn a_pairing_token_is_spent_when_it_arrives() {
        let r = rig(Opts::default());
        let token = r.bridge.new_pairing().unwrap();
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "one", "111111");
        assert_eq!(json(&mut p)["t"], "paired");
        let mut q = phone(&r.bridge);
        pair(&mut q, &token, "two", "222222");
        assert_eq!(code(&mut q), "expired");
        assert_eq!(r.store.devices().unwrap().len(), 1);
    }

    #[test]
    fn a_refused_pairing_adds_nothing_and_spends_the_token() {
        let r = rig(Opts {
            confirm: Arc::new(|_| false),
            ..Opts::default()
        });
        let token = r.bridge.new_pairing().unwrap();
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "Pixel 8", "482913");
        assert_eq!(code(&mut p), "denied");
        assert!(r.store.devices().unwrap().is_empty());
        let mut q = phone(&r.bridge);
        pair(&mut q, &token, "Pixel 8", "482913");
        assert_eq!(code(&mut q), "expired");
    }

    #[test]
    fn an_old_pairing_token_is_refused() {
        let r = rig(Opts {
            timings: Timings {
                pairing_ttl: Duration::from_millis(50),
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        let token = r.bridge.new_pairing().unwrap();
        thread::sleep(Duration::from_millis(120));
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "Pixel 8", "482913");
        assert_eq!(code(&mut p), "expired");
    }

    #[test]
    fn a_wrong_pairing_token_is_unknown_and_a_new_one_ends_the_old() {
        let r = rig(Opts::default());
        let old = r.bridge.new_pairing().unwrap();
        let _new = r.bridge.new_pairing().unwrap();
        let mut p = phone(&r.bridge);
        pair(&mut p, &old, "Pixel 8", "482913");
        assert_eq!(code(&mut p), "unknown");
    }

    #[test]
    fn what_prints_on_the_pc_carries_no_escape_codes() {
        let seen = Arc::new(Mutex::new(String::new()));
        let log = seen.clone();
        let r = rig(Opts {
            confirm: Arc::new(move |p| {
                *log.lock().unwrap() = p.name.to_string();
                true
            }),
            ..Opts::default()
        });
        let token = r.bridge.new_pairing().unwrap();
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "\x1b[31mevil\x07\n", "482913");
        assert_eq!(json(&mut p)["t"], "paired");
        assert_eq!(*seen.lock().unwrap(), "[31mevil");
        let token = r.bridge.new_pairing().unwrap();
        let mut q = phone(&r.bridge);
        pair(&mut q, &token, "x", "12ab");
        assert_eq!(code(&mut q), "unexpected");
    }

    #[test]
    fn failed_attempts_lock_an_address_out_but_not_another() {
        let r = rig(Opts::default());
        let good = enrol(&r, "Pixel 8");
        for _ in 0..3 {
            let mut p = phone(&r.bridge);
            say(
                &mut p,
                serde_json::json!({ "t": "auth", "device": "guess" }),
            );
            assert_eq!(code(&mut p), "unknown");
        }
        let mut p = phone(&r.bridge);
        say(&mut p, serde_json::json!({ "t": "auth", "device": good }));
        assert_eq!(code(&mut p), "limited", "even the right token waits");
        // Behind the tunnel the address is the one it reports.
        let mut other = open(&r.bridge, &[("cf-connecting-ip", "203.0.113.9")]).unwrap();
        login(&mut other, &good);
    }

    #[test]
    fn a_browser_is_refused_at_the_handshake() {
        let r = rig(Opts::default());
        let enrolled = enrol(&r, "Pixel 8");
        let page = open(&r.bridge, &[("origin", "https://evil.example")]);
        assert!(page.is_err(), "a handshake with an Origin got through");
        let mut p = phone(&r.bridge);
        login(&mut p, &enrolled);
    }

    #[test]
    fn read_only_lets_looking_through_and_nothing_else() {
        let r = rig(Opts {
            read_only: true,
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        let v = {
            say(
                &mut p,
                serde_json::json!({ "t": "auth", "device": enrol(&r, "a") }),
            );
            json(&mut p)
        };
        assert_eq!(v["read_only"], true);
        assert_eq!(json(&mut p)["t"], "endpoints");
        // Kind 1 (carries bytes) and kind 0 alike.
        p.send(body(&input(1, b"rm -rf"))).unwrap();
        assert_eq!(code(&mut p), "read_only");
        p.send(body(&ClientMsg::Kill { pane: 1 })).unwrap();
        assert_eq!(code(&mut p), "read_only");
        p.send(body(&ClientMsg::Hello { protocol: PROTOCOL }))
            .unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Hello { protocol: PROTOCOL });
    }

    #[test]
    fn read_only_can_be_switched_while_a_phone_is_connected() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        let echoed = |p: &mut Phone| {
            p.send(body(&input(1, b"ls"))).unwrap();
            assert_eq!(
                msg(p),
                DaemonMsg::Output {
                    pane: 1,
                    bytes: b"ls".to_vec()
                }
            );
        };
        echoed(&mut p);
        r.bridge.set_read_only(true);
        p.send(body(&input(1, b"ls"))).unwrap();
        assert_eq!(code(&mut p), "read_only");
        r.bridge.set_read_only(false);
        echoed(&mut p);
    }

    #[test]
    fn a_phone_asks_for_workspaces_and_sessions_and_gets_the_lists() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "workspaces" }));
        let v = json(&mut p);
        assert_eq!(v["t"], "workspaces", "{v}");
        assert_eq!(v["session"], "main");
        assert_eq!(v["items"][0]["id"], "w1");
        say(
            &mut p,
            serde_json::json!({ "t": "workspaces", "session": "work" }),
        );
        let v = json(&mut p);
        assert_eq!(
            (v["session"].as_str(), v["items"][0]["session"].as_str()),
            (Some("work"), Some("work"))
        );
        say(&mut p, serde_json::json!({ "t": "sessions" }));
        let v = json(&mut p);
        assert_eq!(v["t"], "sessions");
        assert_eq!(v["items"][0]["name"], "main");
        // A session that is not there, and a request that is not one.
        say(
            &mut p,
            serde_json::json!({ "t": "workspaces", "session": "nope" }),
        );
        assert_eq!(code(&mut p), "unsupported");
        say(&mut p, serde_json::json!({ "t": "open", "path": "x" }));
        assert_eq!(code(&mut p), "unsupported");
        // The connection is still good for frames.
        p.send(body(&input(1, b"ls"))).unwrap();
        assert_eq!(
            msg(&mut p),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"ls".to_vec()
            }
        );
    }

    #[test]
    fn a_diff_is_answered_with_its_workspace_and_errors_say_why() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w1" }));
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["ws"].as_str()),
            (Some("diff"), Some("w1")),
            "{v}"
        );
        assert_eq!(v["title"], "w1 vs main");
        assert_eq!(v["files"], serde_json::json!([]));
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "nope" }));
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["code"].as_str()),
            (Some("error"), Some("unsupported"))
        );
        assert!(
            v["msg"].as_str().unwrap().contains("no workspace nope"),
            "{v}"
        );
    }

    #[test]
    fn a_diff_says_which_view_it_is_and_a_bad_view_never_reaches_the_lister() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        for (asked, echoed) in [
            (None, "general"),
            (Some("general"), "general"),
            (Some("uncommitted"), "uncommitted"),
            (Some("commit:ABCDEF1"), "commit:abcdef1"),
        ] {
            let mut ask = serde_json::json!({ "t": "diff", "ws": "w1" });
            if let Some(scope) = asked {
                ask["scope"] = scope.into();
            }
            say(&mut p, ask);
            let v = json(&mut p);
            assert_eq!(
                (v["t"].as_str(), v["scope"].as_str()),
                (Some("diff"), Some(echoed)),
                "{v}"
            );
        }
        // Not a hex id, too short, too long, an option, a path, another word: refused here, before git.
        for bad in [
            "commit:zz12345",
            "commit:abc123",
            &format!("commit:{}", "a".repeat(41)),
            "commit:--output=x",
            "commit:../../x",
            "commit:",
            "branch:main",
            "",
        ] {
            say(
                &mut p,
                serde_json::json!({ "t": "diff", "ws": "w1", "scope": bad }),
            );
            let v = json(&mut p);
            assert_eq!(
                (v["t"].as_str(), v["code"].as_str(), v["ask"].as_str()),
                (Some("error"), Some("unsupported"), Some("diff")),
                "{bad}: {v}"
            );
            assert_eq!(v["ws"], "w1");
        }
    }

    #[test]
    fn an_error_says_which_ask_it_answers() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(
            &mut p,
            serde_json::json!({ "t": "diff", "ws": "nope", "scope": "uncommitted" }),
        );
        let v = json(&mut p);
        assert_eq!(
            (v["ask"].as_str(), v["ws"].as_str(), v["scope"].as_str()),
            (Some("diff"), Some("nope"), Some("uncommitted")),
            "{v}"
        );
        say(&mut p, serde_json::json!({ "t": "commits", "ws": "nope" }));
        let v = json(&mut p);
        assert_eq!(
            (v["ask"].as_str(), v["ws"].as_str()),
            (Some("commits"), Some("nope")),
            "{v}"
        );
        say(
            &mut p,
            serde_json::json!({ "t": "workspaces", "session": "nope" }),
        );
        let v = json(&mut p);
        assert_eq!(v["ask"], "workspaces", "{v}");
    }

    #[test]
    fn commits_are_answered_and_share_the_diff_seat() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "commits", "ws": "w1" }));
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["ws"].as_str()),
            (Some("commits"), Some("w1")),
            "{v}"
        );
        assert_eq!(v["commits"], serde_json::json!([]));
        // One thing that runs git at a time: a commit list while a diff runs is `busy`, and says it is the list's.
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "slow" }));
        say(&mut p, serde_json::json!({ "t": "commits", "ws": "w1" }));
        let v = json(&mut p);
        assert_eq!(
            (v["code"].as_str(), v["ask"].as_str()),
            (Some("unsupported"), Some("commits")),
            "{v}"
        );
        assert!(v["msg"].as_str().unwrap().starts_with("busy"), "{v}");
        assert_eq!(json(&mut p)["t"], "diff");
    }

    #[test]
    fn ls_and_read_are_answered_with_the_path_asked_and_errors_say_which_ask() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "ls", "path": "~/x" }));
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["req"].as_str()),
            (Some("ls"), Some("~/x")),
            "{v}"
        );
        say(
            &mut p,
            serde_json::json!({ "t": "read", "path": "/a/b.md" }),
        );
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["req"].as_str(), v["text"].as_str()),
            (Some("read"), Some("/a/b.md"), Some("hi")),
            "{v}"
        );
        say(&mut p, serde_json::json!({ "t": "ls", "path": "/nope" }));
        let v = json(&mut p);
        assert_eq!(
            (v["code"].as_str(), v["ask"].as_str(), v["req"].as_str()),
            (Some("unsupported"), Some("ls"), Some("/nope")),
            "{v}"
        );
    }

    #[test]
    fn only_so_many_file_asks_run_at_once_and_a_hung_one_does_not_stop_the_others() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        // Two hung listings take both seats; a third is refused as busy, but the lists still answer.
        say(&mut p, serde_json::json!({ "t": "ls", "path": "/hang" }));
        say(&mut p, serde_json::json!({ "t": "ls", "path": "/hang" }));
        say(&mut p, serde_json::json!({ "t": "ls", "path": "/x" }));
        let v = json(&mut p);
        assert_eq!(
            (v["ask"].as_str(), v["code"].as_str()),
            (Some("ls"), Some("unsupported")),
            "{v}"
        );
        assert!(v["msg"].as_str().unwrap().starts_with("busy"), "{v}");
        say(&mut p, serde_json::json!({ "t": "sessions" }));
        assert_eq!(json(&mut p)["t"], "sessions");
    }

    #[test]
    fn a_slow_list_does_not_stop_the_relay_and_a_few_may_run_at_once() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(
            &mut p,
            serde_json::json!({ "t": "workspaces", "session": "slow" }),
        );
        let t = Instant::now();
        p.send(body(&input(1, b"ls"))).unwrap();
        assert_eq!(
            msg(&mut p),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"ls".to_vec()
            }
        );
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["session"].as_str()),
            (Some("workspaces"), Some("slow")),
            "{v}"
        );
        assert_eq!(v["truncated"], false);

        // Four at once are fine; a fifth is refused at once, not queued.
        for _ in 0..5 {
            say(
                &mut p,
                serde_json::json!({ "t": "workspaces", "session": "slow" }),
            );
        }
        let v = json(&mut p);
        assert_eq!(v["code"], "unsupported", "{v}");
        assert!(v["msg"].as_str().unwrap().contains("busy"), "{v}");
        for _ in 0..4 {
            assert_eq!(json(&mut p)["t"], "workspaces");
        }
        // And the seats are free again afterwards.
        say(&mut p, serde_json::json!({ "t": "sessions" }));
        assert_eq!(json(&mut p)["t"], "sessions");
    }

    #[test]
    fn a_lister_that_panics_gets_an_error_and_does_not_keep_the_seat() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        for _ in 0..6 {
            say(
                &mut p,
                serde_json::json!({ "t": "workspaces", "session": "boom" }),
            );
            let v = json(&mut p);
            assert_eq!(v["code"], "unsupported", "{v}");
            assert!(
                !v["msg"].as_str().unwrap().contains("busy"),
                "the seat was kept: {v}"
            );
        }
        // The diff has one seat, and a panic must not take it for good.
        for _ in 0..2 {
            say(&mut p, serde_json::json!({ "t": "diff", "ws": "boom" }));
            let v = json(&mut p);
            assert!(!v["msg"].as_str().unwrap().contains("busy"), "{v}");
        }
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w1" }));
        assert_eq!(json(&mut p)["t"], "diff");
        say(&mut p, serde_json::json!({ "t": "workspaces" }));
        assert_eq!(json(&mut p)["t"], "workspaces");
    }

    /// Set by the diff that was cancelled (one test uses it).
    static WENT: AtomicBool = AtomicBool::new(false);

    #[test]
    fn a_phone_that_goes_away_cancels_its_diff() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "hang" }));
        thread::sleep(Duration::from_millis(100));
        assert!(
            !WENT.load(Ordering::SeqCst),
            "cancelled while the phone is here"
        );
        drop(p);
        for _ in 0..100 {
            if WENT.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the diff went on after its phone left");
    }

    #[test]
    fn a_slow_diff_does_not_stop_the_relay_and_only_one_runs_at_a_time() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "slow" }));
        // A second one while the first runs is refused at once.
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "other" }));
        let v = json(&mut p);
        assert_eq!(v["code"], "unsupported", "{v}");
        assert!(v["msg"].as_str().unwrap().contains("busy"), "{v}");
        // And what a terminal does meanwhile still gets through, before the
        // diff, which is not done for another 600 ms.
        let t = Instant::now();
        p.send(body(&input(1, b"ls"))).unwrap();
        assert_eq!(
            msg(&mut p),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"ls".to_vec()
            }
        );
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        let v = json(&mut p);
        assert_eq!(
            (v["t"].as_str(), v["ws"].as_str()),
            (Some("diff"), Some("slow")),
            "{v}"
        );
        // Free again after it.
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w2" }));
        assert_eq!(json(&mut p)["ws"], "w2");
    }

    #[test]
    fn the_lists_only_read_so_read_only_lets_them_through() {
        let r = rig(Opts {
            read_only: true,
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        say(&mut p, serde_json::json!({ "t": "sessions" }));
        assert_eq!(json(&mut p)["t"], "sessions");
        say(&mut p, serde_json::json!({ "t": "workspaces" }));
        assert_eq!(json(&mut p)["t"], "workspaces");
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w1" }));
        assert_eq!(json(&mut p)["t"], "diff");
    }

    #[test]
    fn a_phone_cannot_drive_mobile_itself() {
        for sent in [
            ClientMsg::MobileWatch,
            ClientMsg::MobileAnswer {
                request: 1,
                yes: true,
            },
            ClientMsg::MobileStop,
            ClientMsg::MobileRevoke { device: "x".into() },
        ] {
            let r = rig(Opts::default());
            let mut p = phone(&r.bridge);
            login(&mut p, &enrol(&r, "a"));
            p.send(body(&sent)).unwrap();
            assert_eq!(code(&mut p), "unsupported", "{sent:?}");
            // Still connected, and what is not mobile control still flows.
            p.send(body(&input(1, b"ls"))).unwrap();
            assert_eq!(
                msg(&mut p),
                DaemonMsg::Output {
                    pane: 1,
                    bytes: b"ls".to_vec()
                }
            );
        }
    }

    #[test]
    fn connected_lists_each_device_while_it_is_there() {
        let r = rig(Opts::default());
        assert!(r.bridge.connected().is_empty());
        let secret = enrol(&r, "Pixel 8");
        let id = r.store.devices().unwrap()[0].id.clone();
        let mut p = phone(&r.bridge);
        login(&mut p, &secret);
        assert_eq!(r.bridge.connected(), vec![id]);
        drop(p);
        wait_until("the device left", || r.bridge.connected().is_empty());
    }

    #[test]
    fn the_pairing_token_has_time_left_until_it_is_used() {
        let r = rig(Opts::default());
        assert_eq!(r.bridge.pairing_left(), None);
        let token = r.bridge.new_pairing().unwrap();
        let left = r.bridge.pairing_left().unwrap();
        assert!(left > Duration::from_secs(100), "{left:?}");
        let mut p = phone(&r.bridge);
        pair(&mut p, &token, "Pixel", "482913");
        let v = json(&mut p);
        assert_eq!(v["t"], "paired", "{v}");
        assert_eq!(r.bridge.pairing_left(), None);
    }

    #[test]
    fn close_cuts_every_phone_without_waiting_for_a_question() {
        let (asked, hold) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
        );
        let (a, h) = (asked.clone(), hold.clone());
        let r = rig(Opts {
            confirm: Arc::new(move |_| {
                a.store(true, Ordering::SeqCst);
                while h.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(20));
                }
                false
            }),
            ..Opts::default()
        });
        let mut waiting = phone(&r.bridge);
        let token = r.bridge.new_pairing().unwrap();
        pair(&mut waiting, &token, "Pixel", "482913");
        wait_until("the question was asked", || asked.load(Ordering::SeqCst));
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        let t = Instant::now();
        r.bridge.close();
        assert!(t.elapsed() < Duration::from_secs(1));
        closed(&mut p);
        hold.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_device_revoked_while_connected_is_dropped() {
        let r = rig(Opts::default());
        let secret = enrol(&r, "Pixel 8");
        let id = r.store.devices().unwrap()[0].id.clone();
        let mut p = phone(&r.bridge);
        login(&mut p, &secret);
        r.store.revoke(&id).unwrap();
        assert_eq!(code(&mut p), "revoked");
        closed(&mut p);
        let mut q = phone(&r.bridge);
        say(&mut q, serde_json::json!({ "t": "auth", "device": secret }));
        assert_eq!(code(&mut q), "unknown");
    }

    #[test]
    fn a_phone_that_does_not_answer_pings_is_dropped_and_one_that_does_stays() {
        let r = rig(Opts {
            timings: Timings {
                ping: Duration::from_millis(60),
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        let mut mute = phone(&r.bridge);
        login(&mut mute, &enrol(&r, "mute"));
        let mut alive = phone(&r.bridge);
        login(&mut alive, &enrol(&r, "alive"));
        // Reading is what answers a ping, so `alive` reads for a while.
        alive
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            let _ = alive.read();
        }
        alive
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        alive.send(body(&input(1, b"x"))).unwrap();
        assert_eq!(
            msg(&mut alive),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"x".to_vec()
            }
        );
        // `mute` never read, so it was dropped long ago.
        mute.get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        wait_until("the mute phone was dropped", || {
            r.bridge.shared.conns.lock().unwrap().len() == 1
        });
    }

    #[test]
    fn a_connection_that_says_nothing_is_cut_off() {
        let r = rig(Opts {
            timings: Timings {
                preauth: Duration::from_millis(150),
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        assert_eq!(code(&mut p), "timeout");
        closed(&mut p);
    }

    #[test]
    fn only_so_many_connections_wait_to_authenticate() {
        let r = rig(Opts {
            timings: Timings {
                max_preauth: 2,
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        let _a = phone(&r.bridge);
        let _b = phone(&r.bridge);
        wait_until("two are waiting", || {
            r.bridge.shared.preauth[Route::Tunnel as usize].load(Ordering::SeqCst) == 2
        });
        assert!(open(&r.bridge, &[]).is_err(), "a third was let wait");
    }

    #[test]
    fn a_daemon_on_another_protocol_is_refused() {
        let r = rig(Opts {
            protocol: PROTOCOL + 1,
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        say(
            &mut p,
            serde_json::json!({ "t": "auth", "device": enrol(&r, "a") }),
        );
        assert_eq!(code(&mut p), "protocol");
        closed(&mut p);
    }

    #[test]
    fn no_daemon_is_an_error_not_a_start() {
        let r = rig(Opts {
            no_daemon: true,
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        say(
            &mut p,
            serde_json::json!({ "t": "auth", "device": enrol(&r, "a") }),
        );
        assert_eq!(code(&mut p), "daemon");
        closed(&mut p);
        assert!(!r.socket.exists(), "the bridge must not start a daemon");
    }

    #[test]
    fn a_daemon_that_hangs_up_ends_the_connection() {
        let r = rig(Opts {
            hang_up: true,
            ..Opts::default()
        });
        let mut p = phone(&r.bridge);
        say(
            &mut p,
            serde_json::json!({ "t": "auth", "device": enrol(&r, "a") }),
        );
        // `ready` is sent once the daemon has answered, then it hangs up.
        let first = json(&mut p);
        let c = if first["t"] == "ready" {
            assert_eq!(json(&mut p)["t"], "endpoints");
            code(&mut p)
        } else {
            first["code"].as_str().unwrap().into()
        };
        assert_eq!(c, "daemon");
        closed(&mut p);
    }

    #[test]
    fn an_empty_binary_message_ends_the_connection() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        p.send(Message::binary(Vec::new())).unwrap();
        closed(&mut p);
    }

    #[test]
    fn a_control_message_after_auth_is_answered_and_the_relay_goes_on() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        // `open` is not built yet: an error, and the relay goes on.
        say(&mut p, serde_json::json!({ "t": "open", "path": "x" }));
        assert_eq!(code(&mut p), "unsupported");
        p.send(body(&input(1, b"x"))).unwrap();
        assert_eq!(
            msg(&mut p),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"x".to_vec()
            }
        );
    }

    #[test]
    fn stop_closes_every_connection_and_waits() {
        let mut r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        p.send(body(&input(1, b"x"))).unwrap();
        msg(&mut p);
        r.bridge.stop();
        closed(&mut p);
        r.bridge.stop();
    }

    #[test]
    fn a_phone_that_stops_reading_is_dropped() {
        let r = rig(Opts::default());
        let mut p = phone(&r.bridge);
        login(&mut p, &enrol(&r, "a"));
        p.send(body(&input(FLOOD, &[]))).unwrap();
        // 64 MiB comes back and the phone reads none of it.
        wait_until("the stalled phone was dropped", || {
            r.bridge.shared.conns.lock().unwrap().is_empty()
        });
    }

    #[test]
    fn it_listens_on_loopback_only() {
        let dir = tempfile::tempdir().unwrap();
        let any = TcpListener::bind("0.0.0.0:0").unwrap();
        let config = Config {
            store: Arc::new(Store::open(dir.path().join("mobile.json"))),
            read_only: false,
            confirm: Arc::new(|_| true),
            look: Arc::new(|| serde_json::json!({})),
            lister: Arc::new(|_, _| Ok(serde_json::json!({}))),
            timings: Timings::default(),
        };
        let err = Bridge::start(any, PathBuf::from("/nowhere"), config)
            .err()
            .unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    // ---- the wifi listener (REQ-56 to REQ-61) -------------------------------

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    /// A client that accepts one certificate, the one whose fingerprint it holds.
    #[derive(Debug)]
    struct Pin {
        fp: String,
        algs: rustls::crypto::WebPkiSupportedAlgorithms,
    }

    impl ServerCertVerifier for Pin {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            if tls::fingerprint(end_entity.as_ref()) == self.fp {
                Ok(ServerCertVerified::assertion())
            } else {
                Err(rustls::Error::General("not the pinned certificate".into()))
            }
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.algs)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.algs)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.algs.supported_schemes()
        }
    }

    struct Wifi {
        rig: Rig,
        addr: SocketAddr,
        fp: String,
    }

    fn rig_wifi(o: Opts) -> Wifi {
        let rig = rig(o);
        let id = tls::load_or_create(rig.dir.path()).unwrap();
        let addr = rig
            .bridge
            .add_wifi(TcpListener::bind("127.0.0.1:0").unwrap(), id.config)
            .unwrap();
        Wifi {
            rig,
            addr,
            fp: id.fp,
        }
    }

    fn provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    /// A phone on the wifi that pins `fp` (`None`: checks as an ordinary client would).
    /// What a phone on the wifi trusts: the one certificate whose fingerprint it holds, or (`None`)
    /// nothing, as an ordinary client that cannot trust a self-signed certificate.
    fn client_config(fp: Option<&str>) -> Arc<rustls::ClientConfig> {
        let builder = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap();
        Arc::new(match fp {
            Some(fp) => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Pin {
                    fp: fp.into(),
                    algs: provider().signature_verification_algorithms,
                }))
                .with_no_client_auth(),
            None => builder
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        })
    }

    fn open_wifi(
        addr: SocketAddr,
        fp: Option<&str>,
        headers: &[(&str, &str)],
    ) -> Result<Phone, String> {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let conn = rustls::ClientConnection::new(
            client_config(fp),
            ServerName::try_from("jw-mobile.local").unwrap(),
        )
        .unwrap();
        let wire = Wire::Tls(Box::new(rustls::StreamOwned::new(conn, s)));
        let mut req = format!("wss://{addr}").into_client_request().unwrap();
        for (k, v) in headers {
            let name = tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
            req.headers_mut().insert(name, v.parse().unwrap());
        }
        tungstenite::client(req, wire)
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn a_phone_pairs_and_comes_back_on_the_wifi() {
        let w = rig_wifi(Opts::default());
        let token = w.rig.bridge.new_pairing().unwrap();
        let mut p = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        pair(&mut p, &token, "Pixel 8", "482913");
        assert_eq!(json(&mut p)["t"], "paired");
        let ready = json(&mut p);
        assert_eq!(ready["t"], "ready", "{ready}");
        assert_eq!(json(&mut p)["t"], "endpoints");
        wait_until("the phone is listed on the wifi", || {
            w.rig
                .bridge
                .connected_routes()
                .iter()
                .any(|(_, r)| *r == Route::Wifi)
        });
        let secret = enrol(&w.rig, "again");
        let mut q = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        login(&mut q, &secret);
        // The same bridge also serves the tunnel's listener, and says which is which.
        let mut t = phone(&w.rig.bridge);
        login(&mut t, &secret);
        wait_until("both routes are listed", || {
            let r = w.rig.bridge.connected_routes();
            r.iter().any(|(_, r)| *r == Route::Tunnel) && r.iter().any(|(_, r)| *r == Route::Wifi)
        });
    }

    #[test]
    fn a_client_that_does_not_hold_the_pin_is_refused() {
        let w = rig_wifi(Opts::default());
        // An ordinary client has no way to trust a self-signed certificate.
        assert!(open_wifi(w.addr, None, &[]).is_err());
        // And one that pins another certificate is refused too.
        assert!(open_wifi(w.addr, Some(&"0".repeat(64)), &[]).is_err());
        // Neither got as far as the gate: nothing was spent or counted.
        assert!(w.rig.bridge.connected().is_empty());
    }

    #[test]
    fn the_wifi_listener_turns_a_browser_away_like_the_tunnel_does() {
        let w = rig_wifi(Opts::default());
        let err = open_wifi(w.addr, Some(&w.fp), &[("origin", "https://evil.example")])
            .err()
            .expect("a browser is refused");
        assert!(
            err.contains("403") || err.to_lowercase().contains("http"),
            "{err}"
        );
    }

    #[test]
    fn on_the_wifi_cf_connecting_ip_does_not_pick_the_address_that_is_counted() {
        let w = rig_wifi(Opts::default());
        // Anyone on the network can send that header. Each failure says it comes
        // from somewhere else, and the lockout must still be reached.
        let tries = Opts::default().timings.fail_max;
        for n in 0..tries {
            let ip = format!("203.0.113.{n}");
            let mut p = open_wifi(w.addr, Some(&w.fp), &[("cf-connecting-ip", &ip)]).unwrap();
            say(&mut p, serde_json::json!({ "t": "auth", "device": "nope" }));
            assert_eq!(code(&mut p), "unknown");
        }
        let mut p =
            open_wifi(w.addr, Some(&w.fp), &[("cf-connecting-ip", "198.51.100.7")]).unwrap();
        assert_eq!(code(&mut p), "limited", "the header picked another address");
        // The control: on the tunnel's listener that header is what is counted.
        let ip_a = ("cf-connecting-ip", "192.0.2.1");
        for _ in 0..tries {
            let mut p = open(&w.rig.bridge, &[ip_a]).unwrap();
            say(&mut p, serde_json::json!({ "t": "auth", "device": "nope" }));
            assert_eq!(code(&mut p), "unknown");
        }
        let mut other = open(&w.rig.bridge, &[("cf-connecting-ip", "192.0.2.2")]).unwrap();
        say(
            &mut other,
            serde_json::json!({ "t": "auth", "device": "nope" }),
        );
        assert_eq!(
            code(&mut other),
            "unknown",
            "another address is not locked out"
        );
    }

    #[test]
    fn a_frame_bigger_than_a_tls_record_goes_through_the_wifi_whole() {
        let w = rig_wifi(Opts::default());
        let mut p = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        login(&mut p, &enrol(&w.rig, "a"));
        let bytes: Vec<u8> = (0..100_000u32).map(|n| (n % 251) as u8).collect();
        p.send(body(&input(4, &bytes))).unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Output { pane: 4, bytes });
    }

    #[test]
    fn a_device_revoked_on_the_wifi_is_cut_off_within_a_couple_of_seconds() {
        let w = rig_wifi(Opts::default());
        let secret = enrol(&w.rig, "Pixel 8");
        let id = w.rig.store.devices().unwrap()[0].id.clone();
        let mut p = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        login(&mut p, &secret);
        let t = Instant::now();
        w.rig.store.revoke(&id).unwrap();
        assert_eq!(code(&mut p), "revoked");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn stopping_the_bridge_closes_the_phones_on_the_wifi() {
        let mut w = rig_wifi(Opts::default());
        let mut p = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        login(&mut p, &enrol(&w.rig, "a"));
        let t = Instant::now();
        w.rig.bridge.stop();
        closed(&mut p);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(
            TcpStream::connect(w.addr).is_err(),
            "the wifi listener is gone"
        );
    }

    #[test]
    fn dropping_the_wifi_listener_leaves_the_tunnel_alone() {
        let w = rig_wifi(Opts::default());
        let mut t = phone(&w.rig.bridge);
        login(&mut t, &enrol(&w.rig, "a"));
        w.rig.bridge.drop_wifi();
        assert!(w.rig.bridge.wifi_addr().is_none());
        t.send(body(&input(1, b"x"))).unwrap();
        assert_eq!(
            msg(&mut t),
            DaemonMsg::Output {
                pane: 1,
                bytes: b"x".to_vec()
            }
        );
    }

    #[test]
    fn a_trickle_of_bytes_does_not_hold_a_seat_past_the_deadline() {
        let w = rig_wifi(Opts {
            timings: Timings {
                preauth: Duration::from_millis(400),
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        // A client that finishes the TLS handshake, then sends the HTTP upgrade
        // one byte at a time, often enough that no single read times out.
        let sock = TcpStream::connect(w.addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let conn = rustls::ClientConnection::new(
            client_config(Some(&w.fp)),
            ServerName::try_from("jw-mobile.local").unwrap(),
        )
        .unwrap();
        let mut wire = rustls::StreamOwned::new(conn, sock);
        let t = Instant::now();
        let mut cut_at = None;
        for b in b"GET / HTTP/1.1\r\nHost: x\r\nX-Slow: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .iter()
            .cycle()
        {
            if io::Write::write_all(&mut wire, &[*b])
                .and_then(|_| io::Write::flush(&mut wire))
                .is_err()
            {
                cut_at = Some(t.elapsed());
                break;
            }
            if t.elapsed() > Duration::from_secs(3) {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        let cut_at = cut_at.expect("the trickle was never cut off");
        assert!(
            cut_at < Duration::from_millis(1500),
            "cut at {cut_at:?}, preauth is 400 ms"
        );
        wait_until("the seat was given back", || {
            w.rig.bridge.shared.preauth[Route::Wifi as usize].load(Ordering::SeqCst) == 0
        });
    }

    #[test]
    fn dropping_and_stopping_the_wifi_do_not_wait_on_the_network() {
        let mut w = rig_wifi(Opts::default());
        let t = Instant::now();
        w.rig.bridge.drop_wifi();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        assert!(TcpStream::connect(w.addr).is_err(), "the listener is gone");
        // And again, and then a stop with a listener up.
        let id = tls::load_or_create(w.rig.dir.path()).unwrap();
        w.rig
            .bridge
            .add_wifi(TcpListener::bind("127.0.0.1:0").unwrap(), id.config)
            .unwrap();
        w.rig
            .bridge
            .add_wifi(
                TcpListener::bind("127.0.0.1:0").unwrap(),
                tls::load_or_create(w.rig.dir.path()).unwrap().config,
            )
            .unwrap();
        let t = Instant::now();
        w.rig.bridge.stop();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn the_wifi_cannot_use_up_the_seats_the_tunnel_needs() {
        let w = rig_wifi(Opts {
            timings: Timings {
                max_preauth: 2,
                preauth: Duration::from_secs(3),
                ..Opts::default().timings
            },
            ..Opts::default()
        });
        // Two silent connections fill the wifi's seats.
        let _a = TcpStream::connect(w.addr).unwrap();
        let _b = TcpStream::connect(w.addr).unwrap();
        wait_until("the wifi seats are taken", || {
            w.rig.bridge.shared.preauth[Route::Wifi as usize].load(Ordering::SeqCst) == 2
        });
        // The tunnel still takes phones in.
        let mut p = phone(&w.rig.bridge);
        login(&mut p, &enrol(&w.rig, "a"));
    }

    #[test]
    fn endpoints_follow_ready_and_every_change_reaches_the_phones_already_in() {
        let w = rig_wifi(Opts::default());
        let mut p = open_wifi(w.addr, Some(&w.fp), &[]).unwrap();
        say(
            &mut p,
            serde_json::json!({ "t": "auth", "device": enrol(&w.rig, "a") }),
        );
        assert_eq!(json(&mut p)["t"], "ready");
        let first = json(&mut p);
        assert_eq!(first["t"], "endpoints");
        assert!(
            first["lan"].is_null() && first["tunnel"].is_null(),
            "{first}"
        );
        w.rig.bridge.set_endpoints(Endpoints {
            lan: Some(("192.168.1.23".into(), 48211, w.fp.clone())),
            tunnel: Some("abc-def.trycloudflare.com".into()),
        });
        let v = json(&mut p);
        assert_eq!(v["t"], "endpoints");
        assert_eq!(v["lan"]["host"], "192.168.1.23");
        assert_eq!(v["lan"]["port"], 48211);
        assert_eq!(v["lan"]["fp"], w.fp.as_str());
        assert_eq!(v["tunnel"], "abc-def.trycloudflare.com");
        // A phone that comes in later is told at once, on either listener.
        let mut q = phone(&w.rig.bridge);
        say(
            &mut q,
            serde_json::json!({ "t": "auth", "device": enrol(&w.rig, "b") }),
        );
        assert_eq!(json(&mut q)["t"], "ready");
        assert_eq!(json(&mut q)["tunnel"], "abc-def.trycloudflare.com");
        // The tunnel stopping is `null`, and says so.
        w.rig.bridge.set_endpoints(Endpoints::default());
        assert!(json(&mut p)["tunnel"].is_null());
    }

    #[test]
    fn the_wifi_listener_refuses_every_address_that_is_not_private() {
        let r = rig(Opts::default());
        let id = tls::load_or_create(r.dir.path()).unwrap();
        let any = TcpListener::bind("0.0.0.0:0").unwrap();
        assert!(
            r.bridge.add_wifi(any, id.config.clone()).is_err(),
            "never every interface"
        );
        assert!(r.bridge.wifi_addr().is_none());
        // And the plain bridge still takes loopback only.
        assert!(
            Bridge::start(
                TcpListener::bind("0.0.0.0:0").unwrap(),
                r.socket.clone(),
                Config {
                    store: r.store.clone(),
                    read_only: false,
                    confirm: Arc::new(|_| true),
                    look: Arc::new(|| serde_json::json!({})),
                    lister: Arc::new(|_, _| Ok(serde_json::json!({}))),
                    timings: Timings::default(),
                },
            )
            .is_err()
        );
    }
}
