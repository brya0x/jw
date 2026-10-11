//! The bridge: a WebSocket on loopback that relays one phone to one daemon
//! connection, frame for frame (mobile spec, REQ-8 to 11).
//!
//! A binary WebSocket message is a jw-proto frame without its length: the
//! kind byte and what follows. A text message is a control message; the
//! bridge answers the ones it knows with an error `{"t":"error","code","msg"}`.

use std::io::{self, ErrorKind};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use jw_proto::proto::{
    ClientMsg, DaemonMsg, MAX_FRAME, PROTOCOL, read_frame, read_raw, write_frame, write_raw,
};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error as WsError, Message, WebSocket};

/// How long a phone may take to finish the WebSocket handshake, and the
/// daemon to answer `Hello`.
const SETUP: Duration = Duration::from_secs(10);
/// The WebSocket is read with this timeout so one thread can also write: a
/// daemon frame waits at most this long for its turn.
const TICK: Duration = Duration::from_millis(10);
/// A phone that takes this long to accept bytes is gone.
#[cfg(not(test))]
const STALLED: Duration = Duration::from_secs(10);
#[cfg(test)]
const STALLED: Duration = Duration::from_millis(300);
/// Daemon frames waiting for a slow phone. When it is full the daemon
/// reader blocks, the socket fills, and the daemon's own queue limit takes
/// over (it drops and resyncs), so memory here stays bounded.
const QUEUE: usize = 64;
/// Daemon frames written per turn before the phone is read again.
const BURST: usize = 32;

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

impl Bridge {
    /// Relays every WebSocket that connects to `listener` to the daemon on
    /// `daemon`. The listener must be on a loopback address: whatever reaches
    /// the bridge reaches every pane, so it is never offered to a network.
    pub fn start(listener: TcpListener, daemon: PathBuf) -> io::Result<Self> {
        let addr = listener.local_addr()?;
        if !addr.ip().is_loopback() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "the bridge listens on loopback only",
            ));
        }
        let shared = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            next: AtomicU64::new(0),
            conns: Mutex::new(Vec::new()),
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

    /// Closes every connection and returns once their threads are done
    /// (REQ-9). Stopping twice is fine.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.shared.stopping.store(true, Ordering::SeqCst);
        // `accept` blocks; a connection to ourselves wakes it.
        let _ = TcpStream::connect(self.addr);
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
            let _ = serve(stream, &daemon);
            shared.forget(id);
        }));
    }
    shared.close_all();
    for h in conns {
        let _ = h.join();
    }
}

/// What the daemon reader hands to the thread that owns the WebSocket.
enum Event {
    Frame(Vec<u8>),
    Closed,
}

fn serve(stream: TcpStream, socket: &PathBuf) -> Result<(), WsError> {
    stream.set_read_timeout(Some(SETUP))?;
    stream.set_write_timeout(Some(STALLED))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME as usize))
        .max_frame_size(Some(MAX_FRAME as usize));
    let mut ws = tungstenite::accept_with_config(stream, Some(config))
        .map_err(|e| WsError::Io(io::Error::other(e.to_string())))?;

    let mut daemon = match connect(socket) {
        Ok(d) => d,
        Err(refusal) => return refuse(&mut ws, refusal),
    };
    ws.get_ref().set_read_timeout(Some(TICK))?;

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

    let result = relay(&mut ws, &mut daemon, &rx);

    let _ = daemon.shutdown(Shutdown::Both);
    let _ = ws.get_ref().shutdown(Shutdown::Both);
    drop(rx);
    let _ = reader.join();
    result
}

/// Phone to daemon and daemon to phone, until either side ends.
fn relay(
    ws: &mut WebSocket<TcpStream>,
    daemon: &mut UnixStream,
    rx: &mpsc::Receiver<Event>,
) -> Result<(), WsError> {
    loop {
        for _ in 0..BURST {
            match rx.try_recv() {
                Ok(Event::Frame(body)) => ws.write(Message::binary(body))?,
                Ok(Event::Closed) => {
                    return refuse(ws, Refusal::Daemon("the daemon hung up".into()));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Writes block up to `STALLED`, so a flush that times out is a phone
        // that stopped reading, not an idle one.
        ws.flush()?;
        match ws.read() {
            Ok(Message::Binary(body)) => write_raw(daemon, &body)?,
            Ok(Message::Text(_)) => ws.send(error("unsupported", "no control messages yet"))?,
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(WsError::Io(e)) if idle(&e) => {}
            Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

fn idle(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Why a phone is turned away.
enum Refusal {
    /// There is no daemon, or it did not answer.
    Daemon(String),
    /// The daemon speaks another protocol than this bridge.
    Protocol(u32),
}

/// Connects to a daemon that is already running (the bridge never starts
/// one) and checks its protocol, which the daemon does not (REQ-8). The
/// answer to this `Hello` is not forwarded; the phone sends its own.
fn connect(socket: &PathBuf) -> Result<UnixStream, Refusal> {
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

fn refuse(ws: &mut WebSocket<TcpStream>, why: Refusal) -> Result<(), WsError> {
    let msg = match why {
        Refusal::Daemon(m) => error("daemon", &m),
        Refusal::Protocol(p) => error(
            "protocol",
            &format!("the daemon speaks protocol {p}, this bridge {PROTOCOL}"),
        ),
    };
    let _ = ws.send(msg);
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

    type Phone = WebSocket<TcpStream>;

    fn start(socket: PathBuf) -> Bridge {
        Bridge::start(TcpListener::bind("127.0.0.1:0").unwrap(), socket).unwrap()
    }

    fn phone(b: &Bridge) -> Phone {
        let s = TcpStream::connect(b.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tungstenite::client(format!("ws://{}", b.addr()), s)
            .unwrap()
            .0
    }

    /// A message as a phone sends it: a frame without its length.
    fn body(msg: &ClientMsg) -> Message {
        let mut b = Vec::new();
        write_frame(&mut b, msg).unwrap();
        Message::binary(b[4..].to_vec())
    }

    fn msg(p: &mut Phone) -> DaemonMsg {
        match p.read().unwrap() {
            Message::Binary(b) => {
                let mut f = (b.len() as u32).to_be_bytes().to_vec();
                f.extend_from_slice(&b);
                read_frame(&mut f.as_slice()).unwrap().unwrap()
            }
            other => panic!("want a frame, got {other:?}"),
        }
    }

    fn error_code(p: &mut Phone) -> String {
        match p.read().unwrap() {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["t"], "error");
                v["code"].as_str().unwrap().to_string()
            }
            other => panic!("want an error, got {other:?}"),
        }
    }

    fn closed(p: &mut Phone) {
        loop {
            match p.read() {
                Ok(Message::Close(_)) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    /// An `Input` for this pane is answered with far more output than a socket holds.
    const FLOOD: u64 = 99;

    /// A daemon that answers `Hello` with `protocol` and echoes `Input` as
    /// `Output`; with `hang_up` it closes right after its answer.
    fn fake_daemon(dir: &std::path::Path, protocol: u32, hang_up: bool) -> PathBuf {
        let socket = dir.join("jw.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            let Ok((mut c, _)) = listener.accept() else {
                return;
            };
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
        socket
    }

    #[test]
    fn relays_frames_both_ways_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL, false));
        let mut p = phone(&b);
        // The first thing back is this Output, so the answer to the bridge's
        // own Hello was not forwarded.
        let bytes: Vec<u8> = (0..=255).collect();
        p.send(body(&ClientMsg::Input {
            pane: 4,
            bytes: bytes.clone(),
        }))
        .unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Output { pane: 4, bytes });
        p.send(body(&ClientMsg::Hello { protocol: PROTOCOL }))
            .unwrap();
        assert_eq!(msg(&mut p), DaemonMsg::Hello { protocol: PROTOCOL });
    }

    #[test]
    fn a_daemon_on_another_protocol_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL + 1, false));
        let mut p = phone(&b);
        assert_eq!(error_code(&mut p), "protocol");
        closed(&mut p);
    }

    #[test]
    fn no_daemon_is_an_error_not_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("none.sock");
        let b = start(socket.clone());
        let mut p = phone(&b);
        assert_eq!(error_code(&mut p), "daemon");
        closed(&mut p);
        assert!(!socket.exists(), "the bridge must not start a daemon");
    }

    #[test]
    fn a_daemon_that_hangs_up_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL, true));
        let mut p = phone(&b);
        assert_eq!(error_code(&mut p), "daemon");
        closed(&mut p);
    }

    #[test]
    fn an_empty_binary_message_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL, false));
        let mut p = phone(&b);
        p.send(Message::binary(Vec::new())).unwrap();
        closed(&mut p);
    }

    #[test]
    fn a_control_message_is_answered_and_the_relay_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL, false));
        let mut p = phone(&b);
        p.send(Message::text("{\"t\":\"workspaces\"}")).unwrap();
        assert_eq!(error_code(&mut p), "unsupported");
        p.send(body(&ClientMsg::Input {
            pane: 1,
            bytes: b"x".to_vec(),
        }))
        .unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let mut b = start(fake_daemon(dir.path(), PROTOCOL, false));
        let mut p = phone(&b);
        p.send(body(&ClientMsg::Input {
            pane: 1,
            bytes: b"x".to_vec(),
        }))
        .unwrap();
        msg(&mut p);
        b.stop();
        closed(&mut p);
        b.stop();
    }

    #[test]
    fn a_phone_that_stops_reading_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let b = start(fake_daemon(dir.path(), PROTOCOL, false));
        let mut p = phone(&b);
        p.send(body(&ClientMsg::Input {
            pane: FLOOD,
            bytes: Vec::new(),
        }))
        .unwrap();
        // 64 MiB comes back and the phone reads none of it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !b.shared.conns.lock().unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the stalled phone was never dropped"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn it_listens_on_loopback_only() {
        let any = TcpListener::bind("0.0.0.0:0").unwrap();
        let err = Bridge::start(any, PathBuf::from("/nowhere")).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
}
