//! REQ-81: the TLS listener, spoken to as a phone does: by pinned fingerprint only.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::time::Duration;

use contract::{BridgeMsg, Denied};
use rustls::{ClientConnection, StreamOwned};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};
use web::auth::{Pairing, Store};
use web::bridge::{Answer, Bridge, Config, Lan};
use web::tls::Pinned;

type Tls = StreamOwned<ClientConnection, TcpStream>;

struct Rig {
    _dir: tempfile::TempDir,
    bridge: Bridge,
    store: Arc<Store>,
    cert: std::path::PathBuf,
}

fn rig(cert: Option<std::path::PathBuf>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            std::thread::spawn(move || {
                let mut out = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if writeln!(out, "{}", serde_json::json!({"t": "echo", "line": line})).is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    let store = Arc::new(Store::new(dir.path().join("devices.json")));
    let cert = cert.unwrap_or_else(|| dir.path().join("link-cert.pem"));
    let bridge = Bridge::start(Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        lan: Some(Lan {
            ip: "127.0.0.1".parse().unwrap(),
            port: 0,
            cert: cert.clone(),
        }),
        daemon_socket: sock,
        store: store.clone(),
        pairing: Arc::new(Pairing::new(Duration::from_secs(120))),
        confirm: Arc::new(|_| Answer::Yes),
        host: None,
    })
    .unwrap();
    Rig {
        _dir: dir,
        bridge,
        store,
        cert,
    }
}

fn connect(addr: std::net::SocketAddr, fp: &str) -> Result<WebSocket<Tls>, String> {
    let sock = TcpStream::connect(addr).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let conn = ClientConnection::new(
        Pinned::client_config(fp).unwrap(),
        "jwn.local".try_into().unwrap(),
    )
    .unwrap();
    let req = format!("ws://{addr}").into_client_request().unwrap();
    tungstenite::client(req, StreamOwned::new(conn, sock))
        .map(|(ws, _)| ws)
        .map_err(|e| e.to_string())
}

fn hear(ws: &mut WebSocket<Tls>) -> Option<serde_json::Value> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

#[test]
fn a_phone_that_pins_the_fingerprint_talks_over_tls_and_one_that_does_not_cannot() {
    let r = rig(None);
    let (addr, fp) = r.bridge.lan.clone().unwrap();
    let (id, secret) = r.store.add("Pixel").unwrap();
    let mut ws = connect(addr, &fp).unwrap();
    ws.send(Message::text(
        serde_json::json!({"t": "auth", "device": id, "secret": secret}).to_string(),
    ))
    .unwrap();
    assert_eq!(hear(&mut ws).unwrap()["t"], "ready");
    ws.send(Message::text(r#"{"t":"close","node":"x"}"#))
        .unwrap();
    loop {
        let v = hear(&mut ws).unwrap();
        if v["t"] == "echo" {
            break;
        }
    }
    assert!(
        connect(addr, &"0".repeat(64)).is_err(),
        "a different fingerprint is refused by the phone"
    );
}

#[test]
fn the_direct_route_needs_no_tunnel_header_and_counts_failures_by_peer() {
    let r = rig(None);
    let (addr, fp) = r.bridge.lan.clone().unwrap();
    for _ in 0..5 {
        let mut ws = connect(addr, &fp).unwrap();
        ws.send(Message::text(
            serde_json::json!({"t": "auth", "device": "0".repeat(16), "secret": "0".repeat(64)})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(
            serde_json::from_value::<BridgeMsg>(hear(&mut ws).unwrap()).unwrap(),
            BridgeMsg::Denied {
                reason: Denied::BadSecret
            }
        );
    }
    let mut ws = connect(addr, &fp).unwrap();
    assert_eq!(
        serde_json::from_value::<BridgeMsg>(hear(&mut ws).unwrap()).unwrap(),
        BridgeMsg::Denied {
            reason: Denied::LockedOut
        }
    );
}

#[test]
fn the_certificate_survives_a_restart() {
    let first = rig(None);
    let fp = first.bridge.lan.clone().unwrap().1;
    let path = first.cert.clone();
    let copy = std::env::temp_dir().join(format!("jwn-cert-{}.pem", std::process::id()));
    std::fs::copy(&path, &copy).unwrap();
    first.bridge.stop();
    let second = rig(Some(copy.clone()));
    assert_eq!(second.bridge.lan.clone().unwrap().1, fp);
    let _ = std::fs::remove_file(copy);
}

#[test]
fn a_lan_listener_refuses_a_public_or_unspecified_address() {
    for ip in ["0.0.0.0", "8.8.8.8"] {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            lan: Some(Lan {
                ip: ip.parse().unwrap(),
                port: 0,
                cert: dir.path().join("c.pem"),
            }),
            daemon_socket: dir.path().join("x.sock"),
            store: Arc::new(Store::new(dir.path().join("d.json"))),
            pairing: Arc::new(Pairing::new(Duration::from_secs(1))),
            confirm: Arc::new(|_| Answer::No),
            host: None,
        };
        assert!(Bridge::start(cfg).is_err(), "{ip}");
    }
}
