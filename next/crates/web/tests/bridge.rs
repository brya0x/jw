//! REQ-72 to REQ-79 against a real bridge, real WebSockets and a stand-in daemon that
//! echoes every line it is sent.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use contract::{BridgeMsg, Denied};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};
use web::auth::{Pairing, Store};
use web::bridge::{Answer, Ask, Bridge, Config, Endpoints};

struct Rig {
    _dir: tempfile::TempDir,
    bridge: Option<Bridge>,
    store: Arc<Store>,
    pairing: Arc<Pairing>,
    /// How many connections the stand-in daemon has accepted.
    contacted: Arc<AtomicUsize>,
    asked: Arc<Mutex<Vec<Ask>>>,
    answer: Arc<Mutex<Answer>>,
    stores_at: PathBuf,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let contacted = Arc::new(AtomicUsize::new(0));
    {
        let contacted = contacted.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                contacted.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut out = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        let echo = serde_json::json!({"t": "echo", "line": line});
                        if writeln!(out, "{echo}").is_err() {
                            break;
                        }
                    }
                });
            }
        });
    }
    let stores_at = dir.path().join("devices.json");
    let store = Arc::new(Store::new(stores_at.clone()));
    let pairing = Arc::new(Pairing::new(Duration::from_secs(120)));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let answer = Arc::new(Mutex::new(Answer::Yes));
    let confirm = {
        let (asked, answer) = (asked.clone(), answer.clone());
        Arc::new(move |a: &Ask| {
            asked.lock().unwrap().push(a.clone());
            *answer.lock().unwrap()
        })
    };
    let bridge = Bridge::start(Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        lan: None,
        daemon_socket: sock,
        store: store.clone(),
        pairing: pairing.clone(),
        confirm,
    })
    .unwrap();
    Rig {
        _dir: dir,
        bridge: Some(bridge),
        store,
        pairing,
        contacted,
        asked,
        answer,
        stores_at,
    }
}

type Ws = WebSocket<TcpStream>;

impl Rig {
    /// A phone connecting as the tunnel would present it.
    fn connect(&self, from: &str) -> Result<Ws, String> {
        let addr = self.bridge.as_ref().unwrap().addr;
        let mut req = format!("ws://{addr}").into_client_request().unwrap();
        req.headers_mut()
            .insert("Cf-Connecting-Ip", from.parse().unwrap());
        self.connect_with(req)
    }

    fn connect_with(&self, req: tungstenite::handshake::client::Request) -> Result<Ws, String> {
        let addr = self.bridge.as_ref().unwrap().addr;
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        tungstenite::client(req, stream)
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }
}

fn say(ws: &mut Ws, v: serde_json::Value) {
    ws.send(Message::text(v.to_string())).unwrap();
}

fn hear(ws: &mut Ws) -> Option<BridgeMsg> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

/// The next line from the daemon; the bridge's own `endpoints` messages are skipped.
fn line(ws: &mut Ws) -> Option<serde_json::Value> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                let v: serde_json::Value = serde_json::from_str(&t).ok()?;
                if v["t"] != "endpoints" {
                    return Some(v);
                }
            }
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn pair(ws: &mut Ws, token: &str) -> Option<BridgeMsg> {
    say(
        ws,
        serde_json::json!({"t": "pair", "token": token, "name": "Pixel", "code": "482913"}),
    );
    hear(ws)
}

#[test]
fn a_confirmed_pairing_gives_a_device_then_relays_lines() {
    let r = rig();
    let token = r.pairing.issue().unwrap();
    let mut ws = r.connect("203.0.113.7").unwrap();
    assert_eq!(r.contacted.load(Ordering::SeqCst), 0);
    let Some(BridgeMsg::Paired { device, secret }) = pair(&mut ws, &token) else {
        panic!("not paired");
    };
    assert_eq!(
        r.asked.lock().unwrap().as_slice(),
        [Ask {
            name: "Pixel".into(),
            code: "482913".into()
        }]
    );
    assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
    say(&mut ws, serde_json::json!({"t": "open", "node": "x"}));
    let echo = line(&mut ws).unwrap();
    assert_eq!(echo["t"], "echo");
    assert!(
        echo["line"].as_str().unwrap().contains("\"open\""),
        "{echo}"
    );
    assert_eq!(r.contacted.load(Ordering::SeqCst), 1);
    let text = std::fs::read_to_string(&r.stores_at).unwrap();
    assert!(
        text.contains(&device) && !text.contains(&secret),
        "only the hash is kept"
    );
}

#[test]
fn a_paired_device_comes_back_with_its_secret_and_a_wrong_one_never_reaches_the_daemon() {
    let r = rig();
    let (id, secret) = r.store.add("Pixel").unwrap();
    let mut bad = r.connect("203.0.113.8").unwrap();
    say(
        &mut bad,
        serde_json::json!({"t": "auth", "device": id, "secret": "0".repeat(64)}),
    );
    assert_eq!(
        hear(&mut bad),
        Some(BridgeMsg::Denied {
            reason: Denied::BadSecret
        })
    );
    assert_eq!(r.contacted.load(Ordering::SeqCst), 0, "REQ-73");
    let mut ws = r.connect("203.0.113.8").unwrap();
    say(
        &mut ws,
        serde_json::json!({"t": "auth", "device": id, "secret": secret}),
    );
    assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
    say(&mut ws, serde_json::json!({"t": "close", "node": "x"}));
    assert_eq!(line(&mut ws).unwrap()["t"], "echo");
}

#[test]
fn a_token_is_spent_even_when_the_person_says_no_or_does_not_answer() {
    let r = rig();
    let token = r.pairing.issue().unwrap();
    *r.answer.lock().unwrap() = Answer::No;
    let mut ws = r.connect("203.0.113.9").unwrap();
    assert_eq!(
        pair(&mut ws, &token),
        Some(BridgeMsg::Denied {
            reason: Denied::Declined
        })
    );
    let mut again = r.connect("203.0.113.9").unwrap();
    assert_eq!(
        pair(&mut again, &token),
        Some(BridgeMsg::Denied {
            reason: Denied::BadToken
        })
    );
    let token = r.pairing.issue().unwrap();
    *r.answer.lock().unwrap() = Answer::Timeout;
    let mut late = r.connect("203.0.113.9").unwrap();
    assert_eq!(
        pair(&mut late, &token),
        Some(BridgeMsg::Denied {
            reason: Denied::Timeout
        })
    );
    assert!(r.store.list().unwrap().is_empty());
    assert_eq!(r.contacted.load(Ordering::SeqCst), 0);
}

#[test]
fn five_failures_from_one_address_lock_it_out_and_others_are_not_affected() {
    let r = rig();
    for _ in 0..5 {
        let mut ws = r.connect("198.51.100.1").unwrap();
        say(
            &mut ws,
            serde_json::json!({"t": "auth", "device": "0".repeat(16), "secret": "0".repeat(64)}),
        );
        assert_eq!(
            hear(&mut ws),
            Some(BridgeMsg::Denied {
                reason: Denied::BadSecret
            })
        );
    }
    let mut ws = r.connect("198.51.100.1").unwrap();
    assert_eq!(
        hear(&mut ws),
        Some(BridgeMsg::Denied {
            reason: Denied::LockedOut
        })
    );
    let mut other = r.connect("198.51.100.2").unwrap();
    say(
        &mut other,
        serde_json::json!({"t": "auth", "device": "0".repeat(16), "secret": "0".repeat(64)}),
    );
    assert_eq!(
        hear(&mut other),
        Some(BridgeMsg::Denied {
            reason: Denied::BadSecret
        })
    );
}

#[test]
fn a_browser_page_and_a_request_without_a_client_address_are_refused() {
    let r = rig();
    let addr = r.bridge.as_ref().unwrap().addr;
    let mut req = format!("ws://{addr}").into_client_request().unwrap();
    req.headers_mut()
        .insert("Cf-Connecting-Ip", "203.0.113.1".parse().unwrap());
    req.headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    assert!(r.connect_with(req).is_err(), "REQ-72: no Origin");
    let bare = format!("ws://{addr}").into_client_request().unwrap();
    assert!(
        r.connect_with(bare).is_err(),
        "REQ-77: the tunnel always sends the address"
    );
}

#[test]
fn revoking_a_device_closes_its_connection_within_two_seconds() {
    let r = rig();
    let (id, secret) = r.store.add("Pixel").unwrap();
    let mut ws = r.connect("203.0.113.10").unwrap();
    say(
        &mut ws,
        serde_json::json!({"t": "auth", "device": id, "secret": secret}),
    );
    assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
    let started = Instant::now();
    assert!(r.store.revoke("Pixel").unwrap());
    assert!(line(&mut ws).is_none(), "the connection ends");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "REQ-78: {:?}",
        started.elapsed()
    );
    let mut again = r.connect("203.0.113.10").unwrap();
    say(
        &mut again,
        serde_json::json!({"t": "auth", "device": id, "secret": secret}),
    );
    assert_eq!(
        hear(&mut again),
        Some(BridgeMsg::Denied {
            reason: Denied::BadSecret
        })
    );
}

#[test]
fn a_message_with_a_newline_or_over_a_mebibyte_ends_the_connection() {
    let r = rig();
    let (id, secret) = r.store.add("Pixel").unwrap();
    let ready = |r: &Rig| {
        let mut ws = r.connect("203.0.113.11").unwrap();
        say(
            &mut ws,
            serde_json::json!({"t": "auth", "device": id, "secret": secret}),
        );
        assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
        ws
    };
    let mut ws = ready(&r);
    ws.send(Message::text(
        "{\"t\":\"close\",\"node\":\"a\"}\n{\"t\":\"close\",\"node\":\"b\"}",
    ))
    .unwrap();
    assert!(line(&mut ws).is_none(), "one message is one line");
    let mut ws = ready(&r);
    let big = "x".repeat((1 << 20) + 1);
    let _ = ws.send(Message::text(big));
    assert!(line(&mut ws).is_none(), "REQ-79");
}

#[test]
fn a_bridge_refuses_a_non_loopback_address() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        bind: "192.168.1.5:0".parse().unwrap(),
        lan: None,
        daemon_socket: dir.path().join("x.sock"),
        store: Arc::new(Store::new(dir.path().join("d.json"))),
        pairing: Arc::new(Pairing::new(Duration::from_secs(1))),
        confirm: Arc::new(|_| Answer::No),
    };
    assert!(Bridge::start(cfg).is_err());
}

#[test]
fn a_stopped_bridge_ends_its_relays() {
    let mut r = rig();
    let (id, secret) = r.store.add("Pixel").unwrap();
    let mut ws = r.connect("203.0.113.12").unwrap();
    say(
        &mut ws,
        serde_json::json!({"t": "auth", "device": id, "secret": secret}),
    );
    assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
    r.bridge.take().unwrap().stop();
    assert!(line(&mut ws).is_none());
}

#[test]
fn phones_are_told_the_addresses_after_ready_and_whenever_they_change() {
    let r = rig();
    let (id, secret) = r.store.add("Pixel").unwrap();
    r.bridge.as_ref().unwrap().set_endpoints(Endpoints {
        lan: None,
        tunnel: Some("quiet-river-1234.trycloudflare.com".into()),
    });
    let mut ws = r.connect("203.0.113.13").unwrap();
    say(
        &mut ws,
        serde_json::json!({"t": "auth", "device": id, "secret": secret}),
    );
    assert_eq!(hear(&mut ws), Some(BridgeMsg::Ready));
    assert_eq!(
        hear(&mut ws),
        Some(BridgeMsg::Endpoints {
            lan: None,
            tunnel: Some("quiet-river-1234.trycloudflare.com".into())
        })
    );
    r.bridge.as_ref().unwrap().set_endpoints(Endpoints {
        lan: Some("192.168.1.20:7443".into()),
        tunnel: None,
    });
    assert_eq!(
        hear(&mut ws),
        Some(BridgeMsg::Endpoints {
            lan: Some("192.168.1.20:7443".into()),
            tunnel: None
        })
    );
}
