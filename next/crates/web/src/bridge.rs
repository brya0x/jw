//! The WebSocket bridge: a phone is let in (REQ-73) by `auth` or a confirmed `pair`, and
//! then its text messages and the daemon's lines are passed to each other (REQ-79).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use contract::{BridgeMsg, Denied, PhoneMsg};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::{Response as HttpResponse, StatusCode};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::auth::{Limiter, Pairing, Store};

/// Longest message either way (REQ-79) and the longest a phone may send before it is let in.
const MAX_MESSAGE: usize = 1 << 20;
const MAX_FIRST: usize = 4096;
/// How long a phone may take to say who it is.
const HELLO: Duration = Duration::from_secs(10);
/// How often the relay looks at the wire when it is quiet, and at the device list.
const TICK: Duration = Duration::from_millis(10);
const REVOKED_CHECK: Duration = Duration::from_millis(500);
const MAX_CONNECTIONS: usize = 32;

/// Who is asking to pair, for the person at the PC to compare.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub name: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Timeout,
}

/// Where the PC can be reached besides loopback; phones are told when it changes (REQ-82).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Endpoints {
    pub lan: Option<String>,
    pub tunnel: Option<String>,
}

pub type Confirm = dyn Fn(&Ask) -> Answer + Send + Sync;

/// A byte stream a WebSocket can run over: the socket itself, or TLS on it.
pub trait Link: Read + Write + Send + 'static {
    fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()>;
}

impl Link for TcpStream {
    fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, d)
    }
}

/// Where a connection came from decides which address a failure is counted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Through the tunnel: the peer is `cloudflared`, the phone is `Cf-Connecting-Ip`.
    Tunnel,
    /// Straight from the network.
    Direct,
}

pub struct Config {
    /// Must be a loopback address.
    pub bind: SocketAddr,
    pub daemon_socket: PathBuf,
    pub store: Arc<Store>,
    pub pairing: Arc<Pairing>,
    pub confirm: Arc<Confirm>,
}

pub(crate) struct Ctx {
    daemon_socket: PathBuf,
    store: Arc<Store>,
    pairing: Arc<Pairing>,
    confirm: Arc<Confirm>,
    limiter: Limiter,
    stop: AtomicBool,
    open: AtomicUsize,
    /// The current addresses and how many times they have been set.
    endpoints: Mutex<(u64, Endpoints)>,
}

pub struct Bridge {
    pub addr: SocketAddr,
    ctx: Arc<Ctx>,
    accept: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Listen on loopback; every connection is a tunnel's.
    pub fn start(cfg: Config) -> io::Result<Bridge> {
        if !cfg.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a loopback address", cfg.bind),
            ));
        }
        let listener = TcpListener::bind(cfg.bind)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let ctx = Arc::new(Ctx {
            daemon_socket: cfg.daemon_socket,
            store: cfg.store,
            pairing: cfg.pairing,
            confirm: cfg.confirm,
            limiter: Limiter::default(),
            stop: AtomicBool::new(false),
            open: AtomicUsize::new(0),
            endpoints: Mutex::new((0, Endpoints::default())),
        });
        let accept = {
            let ctx = ctx.clone();
            thread::Builder::new()
                .name("bridge-accept".into())
                .spawn(move || accept_loop(&listener, &ctx, Route::Tunnel))?
        };
        Ok(Bridge {
            addr,
            ctx,
            accept: Some(accept),
        })
    }

    /// Say where the PC can be reached now; every connected phone is told.
    pub fn set_endpoints(&self, now: Endpoints) {
        let mut slot = self.ctx.endpoints.lock().unwrap_or_else(|e| e.into_inner());
        if slot.1 != now {
            *slot = (slot.0 + 1, now);
        }
    }

    /// Stop accepting and end every relay.
    pub fn stop(mut self) {
        self.ctx.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(listener: &TcpListener, ctx: &Arc<Ctx>, route: Route) {
    while !ctx.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if ctx.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    ctx.open.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let spawned = thread::Builder::new().name("bridge-conn".into()).spawn({
                    let ctx = ctx.clone();
                    move || {
                        if stream.set_nonblocking(false).is_ok() {
                            let peer = stream.peer_addr().ok().map(|a| a.ip());
                            serve(stream, peer, &ctx, route);
                        }
                        ctx.open.fetch_sub(1, Ordering::SeqCst);
                    }
                });
                if spawned.is_err() {
                    ctx.open.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(TICK * 2),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn refuse(status: u16, why: &str) -> ErrorResponse {
    let mut r = HttpResponse::new(Some(why.to_owned()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN);
    r
}

/// The handshake's decision: no browser pages (REQ-72), and through the tunnel a client
/// address (REQ-77). `ErrorResponse` is the dependency's type, large as it is.
#[allow(clippy::result_large_err)]
fn vet(
    req: &Request,
    resp: Response,
    route: Route,
    via: &mut Option<String>,
) -> Result<Response, ErrorResponse> {
    if req.headers().contains_key("origin") {
        return Err(refuse(403, "a browser page may not use this"));
    }
    *via = req
        .headers()
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if route == Route::Tunnel
        && via
            .as_deref()
            .and_then(|v| v.parse::<IpAddr>().ok())
            .is_none()
    {
        return Err(refuse(400, "no client address"));
    }
    Ok(resp)
}

/// One phone, from its handshake to its end. (The handshake callback's `ErrorResponse` is the
/// dependency's large type.)
#[allow(clippy::result_large_err)]
pub(crate) fn serve<S: Link>(stream: S, peer: Option<IpAddr>, ctx: &Arc<Ctx>, route: Route) {
    let _ = stream.set_read_timeout(Some(HELLO));
    let mut via: Option<String> = None;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let handshake = tungstenite::accept_hdr_with_config(
        stream,
        |req: &Request, resp: Response| vet(req, resp, route, &mut via),
        Some(config),
    );
    let Ok(mut ws) = handshake else { return };
    let key = match route {
        Route::Tunnel => via.unwrap_or_default(),
        Route::Direct => peer.map(|p| p.to_string()).unwrap_or_default(),
    };
    let Some(device) = admit(&mut ws, ctx, &key) else {
        let _ = ws.close(None);
        let _ = ws.flush();
        return;
    };
    relay(ws, ctx, &device);
}

fn send<S: Link>(ws: &mut WebSocket<S>, msg: &BridgeMsg) -> bool {
    serde_json::to_string(msg).is_ok_and(|t| ws.send(Message::text(t)).is_ok())
}

fn deny<S: Link>(ws: &mut WebSocket<S>, reason: Denied) -> Option<String> {
    send(ws, &BridgeMsg::Denied { reason });
    None
}

fn hex_of(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Let a phone in or say why not; the device id when it is let in. Nothing here touches the daemon.
fn admit<S: Link>(ws: &mut WebSocket<S>, ctx: &Ctx, key: &str) -> Option<String> {
    if !ctx.limiter.allowed(key) {
        return deny(ws, Denied::LockedOut);
    }
    let first = loop {
        match ws.read() {
            Ok(Message::Text(t)) if t.len() <= MAX_FIRST => break t,
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            _ => {
                ctx.limiter.failed(key);
                return None;
            }
        }
    };
    let Ok(msg) = serde_json::from_str::<PhoneMsg>(&first) else {
        ctx.limiter.failed(key);
        return None;
    };
    let device = match msg {
        PhoneMsg::Auth { device, secret } => {
            if !hex_of(&device, 16)
                || !hex_of(&secret, 64)
                || ctx.store.find(&device, &secret).is_none()
            {
                ctx.limiter.failed(key);
                return deny(ws, Denied::BadSecret);
            }
            device
        }
        PhoneMsg::Pair { token, name, code } => {
            let sane = hex_of(&token, 32)
                && (1..=64).contains(&name.chars().count())
                && !name.chars().any(char::is_control)
                && (4..=12).contains(&code.len())
                && code.bytes().all(|b| b.is_ascii_digit());
            if !sane || !ctx.pairing.take(&token) {
                ctx.limiter.failed(key);
                return deny(ws, Denied::BadToken);
            }
            match (ctx.confirm)(&Ask {
                name: name.clone(),
                code,
            }) {
                Answer::Yes => {}
                Answer::No => return deny(ws, Denied::Declined),
                Answer::Timeout => return deny(ws, Denied::Timeout),
            }
            let Ok((id, secret)) = ctx.store.add(&name) else {
                return None;
            };
            if !send(
                ws,
                &BridgeMsg::Paired {
                    device: id.clone(),
                    secret,
                },
            ) {
                return None;
            }
            id
        }
    };
    ctx.limiter.cleared(key);
    let _ = ctx.store.touch(&device);
    send(ws, &BridgeMsg::Ready).then_some(device)
}

/// Pass text messages and daemon lines to each other until either side ends or the device
/// is revoked.
fn relay<S: Link>(mut ws: WebSocket<S>, ctx: &Ctx, device: &str) {
    let Ok(daemon) = UnixStream::connect(&ctx.daemon_socket) else {
        let _ = ws.close(None);
        return;
    };
    let Ok(reader) = daemon.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel::<String>(256);
    let reading = thread::Builder::new()
        .name("bridge-daemon".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    if reading.is_err() {
        return;
    }
    let _ = ws.get_ref().set_read_timeout(Some(TICK));
    let mut out = &daemon;
    let mut checked = Instant::now();
    let mut told = 0;
    'relay: loop {
        if ctx.stop.load(Ordering::SeqCst) {
            break;
        }
        if checked.elapsed() >= REVOKED_CHECK {
            checked = Instant::now();
            if !ctx.store.is_paired(device) {
                break;
            }
        }
        let (version, endpoints) = ctx
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if version != told || told == 0 {
            told = version.max(1);
            let msg = BridgeMsg::Endpoints {
                lan: endpoints.lan,
                tunnel: endpoints.tunnel,
            };
            if !send(&mut ws, &msg) {
                break;
            }
        }
        loop {
            match rx.try_recv() {
                Ok(line) => {
                    if ws.send(Message::text(line)).is_err() {
                        break 'relay;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'relay,
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                if t.contains('\n') || writeln!(out, "{t}").is_err() {
                    break;
                }
            }
            Ok(Message::Close(_) | Message::Binary(_)) => break,
            Ok(_) => {}
            Err(WsError::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    let _ = daemon.shutdown(std::net::Shutdown::Both);
}
