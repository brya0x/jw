//! The WebSocket bridge: a phone is let in (REQ-73) by `auth` or a confirmed `pair`, and
//! then its text messages and the daemon's lines are passed to each other (REQ-79).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
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
use crate::tls::{self, Identity, TlsLink};

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

/// The TLS listener on the PC's own network (REQ-81).
pub struct Lan {
    /// Loopback or private (`tls::bindable`).
    pub ip: Ipv4Addr,
    pub port: u16,
    /// Where the certificate is kept.
    pub cert: PathBuf,
}

pub struct Config {
    /// Must be a loopback address.
    pub bind: SocketAddr,
    pub lan: Option<Lan>,
    pub daemon_socket: PathBuf,
    pub store: Arc<Store>,
    pub pairing: Arc<Pairing>,
    pub confirm: Arc<Confirm>,
    /// The PC's name, told to every phone that gets in.
    pub host: Option<String>,
}

pub(crate) struct Ctx {
    daemon_socket: PathBuf,
    store: Arc<Store>,
    pairing: Arc<Pairing>,
    confirm: Arc<Confirm>,
    host: Option<String>,
    limiter: Limiter,
    stop: AtomicBool,
    open: AtomicUsize,
    /// The current addresses and how many times they have been set.
    endpoints: Mutex<(u64, Endpoints)>,
}

pub struct Bridge {
    pub addr: SocketAddr,
    /// Where the TLS listener is, and the fingerprint of its certificate (the QR's `fp`).
    pub lan: Option<(SocketAddr, String)>,
    ctx: Arc<Ctx>,
    accepts: Vec<JoinHandle<()>>,
}

/// What a listener does with an accepted socket.
type Handler = Arc<dyn Fn(TcpStream, Option<IpAddr>, &Arc<Ctx>) + Send + Sync>;

impl Bridge {
    /// Listen on loopback; every connection is a tunnel's.
    pub fn start(cfg: Config) -> io::Result<Bridge> {
        if !cfg.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a loopback address", cfg.bind),
            ));
        }
        // Everything that can fail is done before any thread starts, so a failure leaves nothing running.
        let listener = TcpListener::bind(cfg.bind)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let lan_setup = cfg.lan.as_ref().map(lan_listener).transpose()?;
        let ctx = Arc::new(Ctx {
            daemon_socket: cfg.daemon_socket,
            store: cfg.store,
            pairing: cfg.pairing,
            confirm: cfg.confirm,
            host: cfg.host,
            limiter: Limiter::default(),
            stop: AtomicBool::new(false),
            open: AtomicUsize::new(0),
            endpoints: Mutex::new((0, Endpoints::default())),
        });
        let tunnel: Handler = Arc::new(|s, peer, ctx| serve(s, peer, ctx, Route::Tunnel));
        let mut accepts = vec![listen("bridge-accept", listener, &ctx, tunnel)?];
        let mut lan = None;
        if let Some((listener, tls_config, at, fp)) = lan_setup {
            lan = Some((at, fp));
            let direct: Handler = Arc::new(move |s, peer, ctx| {
                if let Ok(link) = TlsLink::new(&tls_config, s) {
                    serve(link, peer, ctx, Route::Direct);
                }
            });
            accepts.push(listen("bridge-accept-lan", listener, &ctx, direct)?);
        }
        Ok(Bridge {
            addr,
            lan,
            ctx,
            accepts,
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
        for t in self.accepts.drain(..) {
            let _ = t.join();
        }
    }
}

/// The TLS listener of the private address: bound, with its certificate, ready to be served.
fn lan_listener(
    l: &Lan,
) -> io::Result<(TcpListener, Arc<rustls::ServerConfig>, SocketAddr, String)> {
    if !tls::bindable(l.ip) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is neither loopback nor a private address", l.ip),
        ));
    }
    let identity = Identity::load_or_create(&l.cert)?;
    let config = identity.server_config()?;
    let listener = TcpListener::bind((l.ip, l.port))?;
    listener.set_nonblocking(true)?;
    let at = listener.local_addr()?;
    Ok((listener, config, at, identity.fingerprint()))
}

fn listen(
    name: &str,
    listener: TcpListener,
    ctx: &Arc<Ctx>,
    handler: Handler,
) -> io::Result<JoinHandle<()>> {
    let ctx = ctx.clone();
    thread::Builder::new()
        .name(name.into())
        .spawn(move || accept_loop(&listener, &ctx, &handler))
}

fn accept_loop(listener: &TcpListener, ctx: &Arc<Ctx>, handler: &Handler) {
    while !ctx.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if ctx.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    ctx.open.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let spawned = thread::Builder::new().name("bridge-conn".into()).spawn({
                    let (ctx, handler) = (ctx.clone(), handler.clone());
                    move || {
                        if stream.set_nonblocking(false).is_ok() {
                            let peer = stream.peer_addr().ok().map(|a| a.ip());
                            handler(stream, peer, &ctx);
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
    send(
        ws,
        &BridgeMsg::Ready {
            host: ctx.host.clone(),
        },
    )
    .then_some(device)
}

/// The reply to a `mobile_*` command from a phone, which only the PC's own tools may send
/// (REQ-84): the phone link cannot be reconfigured through the phone link.
fn refused_control(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let is_control = v["t"] == "cmd"
        && v["cmd"]["cmd"]
            .as_str()
            .is_some_and(|c| c.starts_with("mobile_"));
    is_control.then(|| {
        serde_json::json!({
            "t": "reply", "id": v["id"], "ok": false,
            "error": {"code": "unsupported", "message": "the phone link is controlled from the PC"}
        })
        .to_string()
    })
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
    // The version of the endpoints this phone was last told; none yet.
    let mut told = None;
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
        if told != Some(version) {
            told = Some(version);
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
                if t.contains('\n') {
                    break;
                }
                if let Some(reply) = refused_control(&t) {
                    if ws.send(Message::text(reply)).is_err() {
                        break;
                    }
                } else if writeln!(out, "{t}").is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mobile_commands_are_refused() {
        let reply =
            refused_control(r#"{"t":"cmd","id":"c1","cmd":{"cmd":"mobile_stop"}}"#).unwrap();
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            (
                v["id"].as_str(),
                v["ok"].as_bool(),
                v["error"]["code"].as_str()
            ),
            (Some("c1"), Some(false), Some("unsupported"))
        );
        assert!(
            refused_control(r#"{"t":"cmd","id":"c2","cmd":{"cmd":"write","pane":"p","data":"x"}}"#)
                .is_none()
        );
        assert!(refused_control(r#"{"t":"open","node":"mobile_x"}"#).is_none());
        // the commands of a conversation of its own are a phone's like any other workspace's (REQ-819)
        for cmd in [
            r#"{"cmd":"chat_new"}"#,
            r#"{"cmd":"archive_chat","workspace":"w"}"#,
            r#"{"cmd":"resume_chat","workspace":"w"}"#,
            r#"{"cmd":"delete_chat","workspace":"w"}"#,
            r#"{"cmd":"promote_chat","workspace":"w","to":"x"}"#,
        ] {
            let line = format!(r#"{{"t":"cmd","id":"c1","cmd":{cmd}}}"#);
            assert!(refused_control(&line).is_none(), "{cmd}");
        }
        assert!(refused_control("not json").is_none());
    }
}
