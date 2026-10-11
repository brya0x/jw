//! The bridge in front of a real daemon (mobile spec, REQ-4, 8, 10).

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use jw_proto::client::Client;
use jw_proto::proto::{ClientMsg, DaemonMsg, PROTOCOL, read_frame, write_frame};
use jw_web::auth::{self, Store};
use jw_web::bridge::{Bridge, Config, Timings};
use tungstenite::{Message, WebSocket};

const EXE: &str = env!("CARGO_BIN_EXE_jw");
const TIMEOUT: Duration = Duration::from_secs(10);

/// `jw daemon` on its own socket, killed when the test ends.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Phone(WebSocket<TcpStream>);

impl Phone {
    fn connect(bridge: &Bridge) -> Self {
        let s = TcpStream::connect(bridge.addr()).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        Self(
            tungstenite::client(format!("ws://{}", bridge.addr()), s)
                .unwrap()
                .0,
        )
    }

    /// Authenticates, and waits for `ready`: the daemon is there and the
    /// protocol matches.
    fn login(&mut self, secret: &str) {
        let hello = serde_json::json!({ "t": "auth", "device": secret });
        self.0.send(Message::text(hello.to_string())).unwrap();
        match self.0.read().unwrap() {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["t"], "ready", "{v}");
                assert_eq!(v["protocol"], PROTOCOL);
            }
            other => panic!("want ready, got {other:?}"),
        }
    }

    /// A frame without its length, as the bridge expects it.
    fn send(&mut self, msg: &ClientMsg) {
        let mut b = Vec::new();
        write_frame(&mut b, msg).unwrap();
        self.0.send(Message::binary(b[4..].to_vec())).unwrap();
    }

    fn recv(&mut self) -> DaemonMsg {
        match self.0.read().unwrap() {
            Message::Binary(b) => {
                let mut f = (b.len() as u32).to_be_bytes().to_vec();
                f.extend_from_slice(&b);
                read_frame(&mut f.as_slice()).unwrap().unwrap()
            }
            other => panic!("want a frame, got {other:?}"),
        }
    }
}

#[test]
fn a_phone_drives_a_pane_through_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("jw.sock");
    let _daemon = Daemon(
        Command::new(EXE)
            .arg("daemon")
            .env("JW_SOCKET", &socket)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + TIMEOUT;
    while Client::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon never listened");
        thread::sleep(Duration::from_millis(20));
    }
    let store = Arc::new(Store::open(dir.path().join("mobile.json")));
    let secret = auth::random_hex(32).unwrap();
    store.add("test phone", &auth::hash(&secret)).unwrap();
    let bridge = Bridge::start(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        socket,
        Config {
            store,
            read_only: false,
            confirm: Arc::new(|_| false),
            look: Arc::new(|| serde_json::json!({"palettes": [], "settings": {}})),
            timings: Timings::default(),
        },
    )
    .unwrap();
    let mut phone = Phone::connect(&bridge);
    phone.login(&secret);

    phone.send(&ClientMsg::Hello { protocol: PROTOCOL });
    assert_eq!(phone.recv(), DaemonMsg::Hello { protocol: PROTOCOL });

    phone.send(&ClientMsg::Spawn {
        stream: "phone".into(),
        role: "shell".into(),
        cmd: Some("cat".into()),
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    });
    let pane = loop {
        match phone.recv() {
            DaemonMsg::Spawned { pane } => break pane,
            DaemonMsg::Tree { .. } => {}
            other => panic!("want Spawned, got {other:?}"),
        }
    };

    phone.send(&ClientMsg::Attach {
        stream: "phone".into(),
    });
    let mut screen = vt100::Parser::new(24, 80, 0);
    let mut sent = false;
    let deadline = Instant::now() + TIMEOUT;
    while !screen.screen().contents().contains("hello from the phone") {
        assert!(Instant::now() < deadline, "the pane never echoed");
        match phone.recv() {
            DaemonMsg::Snapshot { bytes, .. } => {
                screen.process(&bytes);
                if !sent {
                    sent = true;
                    phone.send(&ClientMsg::Input {
                        pane,
                        bytes: b"hello from the phone\n".to_vec(),
                    });
                }
            }
            DaemonMsg::Output { pane: p, bytes } if p == pane => screen.process(&bytes),
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    }
}
