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
//! PC's jw has on disk (REQ-12). Both only read, so they pass in read-only.
//!
//! A phone is let in by `pair` or `auth` and nothing else; until then the
//! daemon is not even connected to.

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

/// What a phone can ask the PC to list (REQ-12).
pub enum Query {
    /// The workspaces of a session, by name.
    Workspaces(String),
    /// Every session.
    Sessions,
}

/// Answers a `Query` with its items, or why not. Like `Look`, the daemon hands
/// it in: it reads jw's registry and folders, which this crate does not know.
pub type Lister = Arc<dyn Fn(&Query) -> Result<serde_json::Value, String> + Send + Sync>;

pub struct Config {
    pub store: Arc<Store>,
    /// Drop everything but looking (REQ-7).
    pub read_only: bool,
    pub confirm: Confirm,
    pub look: Look,
    pub lister: Lister,
    pub timings: Timings,
}

/// Listening, and the connections it relays.
pub struct Bridge {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

struct Shared {
    stopping: AtomicBool,
    next: AtomicU64,
    conns: Mutex<Vec<(u64, TcpStream)>>,
    store: Arc<Store>,
    /// Can change while phones are connected (REQ-37).
    read_only: AtomicBool,
    /// The devices connected now, one entry per connection.
    online: Mutex<Vec<String>>,
    confirm: Confirm,
    look: Look,
    lister: Lister,
    t: Timings,
    pairing: Mutex<Option<Pairing>>,
    limiter: Mutex<Limiter>,
    preauth: AtomicUsize,
}

impl Shared {
    /// Keeps a handle on a connection so `stop` can close it. `None` once
    /// the bridge is stopping.
    fn register(&self, s: &TcpStream) -> Option<u64> {
        let mut conns = self.conns.lock().unwrap();
        if self.stopping.load(Ordering::SeqCst) {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        conns.push((id, s.try_clone().ok()?));
        Some(id)
    }

    fn forget(&self, id: u64) {
        self.conns.lock().unwrap().retain(|(i, _)| *i != id);
    }

    fn close_all(&self) {
        for (_, s) in self.conns.lock().unwrap().iter() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// A place among the connections that have not authenticated (REQ-32).
struct Slot<'a>(&'a AtomicUsize);

impl<'a> Slot<'a> {
    fn take(shared: &'a Shared) -> Option<Self> {
        if shared.preauth.fetch_add(1, Ordering::SeqCst) >= shared.t.max_preauth {
            shared.preauth.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(&shared.preauth))
    }
}

/// A device counted as connected for as long as this lives.
struct Online<'a>(&'a Shared, String);

impl<'a> Online<'a> {
    fn enter(shared: &'a Shared, device: &str) -> Self {
        shared
            .online
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(device.to_string());
        Self(shared, device.to_string())
    }
}

impl Drop for Online<'_> {
    fn drop(&mut self) {
        // A poisoned lock must not make a drop panic, which would abort.
        let mut online = self.0.online.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = online.iter().position(|d| *d == self.1) {
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
            preauth: AtomicUsize::new(0),
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
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A new pairing token, which ends the one before it (REQ-2).
    pub fn new_pairing(&self) -> io::Result<String> {
        let pairing = Pairing::new(Instant::now())?;
        let token = pairing.token().to_string();
        *self.shared.pairing.lock().unwrap() = Some(pairing);
        Ok(token)
    }

    /// How long the current pairing token has left; `None` when there is
    /// none, it was used, or it ended.
    pub fn pairing_left(&self) -> Option<Duration> {
        let pairing = self.shared.pairing.lock().unwrap();
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
        // `accept` blocks; a connection to ourselves wakes it.
        let _ = TcpStream::connect(self.addr);
        self.shared.close_all();
    }

    /// Closes every connection and returns once their threads are done
    /// (REQ-9). Stopping twice is fine.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.close();
        let _ = accept.join();
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
        let (daemon, shared) = (daemon.clone(), shared.clone());
        conns.push(thread::spawn(move || {
            let Some(id) = shared.register(&stream) else {
                return;
            };
            let _ = serve(stream, &shared, &daemon);
            shared.forget(id);
        }));
    }
    shared.close_all();
    for h in conns {
        let _ = h.join();
    }
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

fn serve(stream: TcpStream, shared: &Shared, socket: &Path) -> Result<(), WsError> {
    let t = &shared.t;
    let Some(slot) = Slot::take(shared) else {
        return Ok(());
    };
    let peer = stream.peer_addr()?.ip().to_string();
    stream.set_read_timeout(Some(t.preauth))?;
    stream.set_write_timeout(Some(t.stalled))?;
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
    let mut ws = tungstenite::accept_hdr_with_config(stream, check, Some(config))
        .map_err(|e| WsError::Io(io::Error::other(e.to_string())))?;
    let key = via.unwrap_or(peer);

    let Some(device) = authenticate(&mut ws, shared, &key)? else {
        return Ok(());
    };
    drop(slot);

    // Only now does anything reach the daemon (REQ-4).
    let mut daemon = match connect(socket) {
        Ok(d) => d,
        Err(refusal) => return refuse_daemon(&mut ws, refusal),
    };
    ws.get_ref().set_read_timeout(Some(TICK))?;
    let _online = Online::enter(shared, &device.id);
    let look = (shared.look)();
    ws.send(Message::text(
        serde_json::json!({
            "t": "ready",
            "bridge": 1,
            "protocol": PROTOCOL,
            "read_only": shared.read_only.load(Ordering::SeqCst),
            // A callback that left a key out gives the app an empty one, not
            // a `null` it would have to guard against.
            "palettes": look.get("palettes").cloned().unwrap_or_else(|| serde_json::json!([])),
            "settings": look.get("settings").cloned().unwrap_or_else(|| serde_json::json!({})),
        })
        .to_string(),
    ))?;

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

    let result = relay(&mut ws, &mut daemon, &rx, shared, &device);

    let _ = daemon.shutdown(Shutdown::Both);
    let _ = ws.get_ref().shutdown(Shutdown::Both);
    drop(rx);
    let _ = reader.join();
    result
}

/// Reads the first message and lets the phone in or turns it away (REQ-3 to
/// 5). `None` means it was turned away and told why.
fn authenticate(
    ws: &mut WebSocket<TcpStream>,
    shared: &Shared,
    key: &str,
) -> Result<Option<Device>, WsError> {
    let deadline = Instant::now() + shared.t.preauth;
    if shared.limiter.lock().unwrap().locked(key, Instant::now()) {
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

    let failed = |why: &str, msg: &str, ws: &mut WebSocket<TcpStream>| {
        shared.limiter.lock().unwrap().failed(key, Instant::now());
        refuse(ws, why, msg)
    };
    let Ok(control) = serde_json::from_str::<Control>(&text) else {
        failed("unexpected", "pair or auth first", ws)?;
        return Ok(None);
    };
    match control {
        Control::Auth { device } => match shared.store.find(&auth::hash(&device))? {
            Some(d) => {
                shared.limiter.lock().unwrap().succeeded(key);
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
            let taken = match shared.pairing.lock().unwrap().as_mut() {
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
            shared.limiter.lock().unwrap().succeeded(key);
            ws.send(Message::text(
                serde_json::json!({ "t": "paired", "device": secret, "id": device.id }).to_string(),
            ))?;
            Ok(Some(device))
        }
    }
}

/// Phone to daemon and daemon to phone, until either side ends.
fn relay(
    ws: &mut WebSocket<TcpStream>,
    daemon: &mut UnixStream,
    rx: &mpsc::Receiver<Event>,
    shared: &Shared,
    device: &Device,
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
                    Message::Text(text) => ws.send(answer(shared, &text))?,
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
    Workspaces { session: Option<String> },
    Sessions,
}

/// The answer to one control message of a phone: the list asked for, or an
/// error saying why not.
fn answer(shared: &Shared, text: &str) -> Message {
    let Ok(ask) = serde_json::from_str::<Ask>(text) else {
        return error("unsupported", "ask workspaces or sessions");
    };
    match ask {
        Ask::Workspaces { session } => {
            let session = session.unwrap_or_else(|| "main".into());
            match (shared.lister)(&Query::Workspaces(session.clone())) {
                Ok(items) => Message::text(
                    serde_json::json!({ "t": "workspaces", "session": session, "items": items })
                        .to_string(),
                ),
                Err(why) => error("unsupported", &why),
            }
        }
        Ask::Sessions => match (shared.lister)(&Query::Sessions) {
            Ok(items) => {
                Message::text(serde_json::json!({ "t": "sessions", "items": items }).to_string())
            }
            Err(why) => error("unsupported", &why),
        },
    }
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

fn refuse_daemon(ws: &mut WebSocket<TcpStream>, why: Refusal) -> Result<(), WsError> {
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
fn refuse(ws: &mut WebSocket<TcpStream>, code: &str, msg: &str) -> Result<(), WsError> {
    let _ = ws.send(error(code, msg));
    let _ = ws.close(None);
    let _ = ws.flush();
    Ok(())
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

    type Phone = WebSocket<TcpStream>;

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
                lister: Arc::new(|q| match q {
                    Query::Workspaces(s) if s == "nope" => Err("no session named nope".into()),
                    Query::Workspaces(s) => Ok(serde_json::json!([{ "id": "w1", "session": s }])),
                    Query::Sessions => Ok(serde_json::json!([{ "name": "main", "workspaces": 1 }])),
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
        tungstenite::client(req, s)
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
                    "palettes": [{"name": format!("t{n}"), "dark": true, "colors": {}}],
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
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w1" }));
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
            r.bridge.shared.preauth.load(Ordering::SeqCst) == 2
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
        // `diff` is not built yet (W3): an error, and the relay goes on.
        say(&mut p, serde_json::json!({ "t": "diff", "ws": "w1" }));
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
            lister: Arc::new(|_| Ok(serde_json::json!([]))),
            timings: Timings::default(),
        };
        let err = Bridge::start(any, PathBuf::from("/nowhere"), config)
            .err()
            .unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
}
