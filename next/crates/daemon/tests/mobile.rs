//! N26: the phone link hosted by a real daemon. A stand-in `cloudflared` prints a host name;
//! the "phone" reaches the bridge's loopback port the way the tunnel would.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use common::Sandbox;
use contract::{BridgeMsg, ClientMsg, Cmd, DaemonMsg, Denied};
use daemon::{Daemon, LinkConfig};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

fn start_with_tunnel(sb: &Sandbox) -> Daemon {
    let script = sb.dir.join("cloudflared");
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'INF https://fake-link-9.trycloudflare.com' >&2\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
    config.link = LinkConfig {
        tunnel: Some(script.to_string_lossy().into()),
        resolve: false,
        lan: false,
        lan_ip: None,
    };
    Daemon::start(config).unwrap()
}

type Ws = WebSocket<std::net::TcpStream>;

fn phone(local: &str) -> Ws {
    let mut req = format!("ws://{local}").into_client_request().unwrap();
    req.headers_mut()
        .insert("Cf-Connecting-Ip", "203.0.113.5".parse().unwrap());
    let stream = std::net::TcpStream::connect(local).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    tungstenite::client(req, stream).unwrap().0
}

fn text(ws: &mut Ws) -> serde_json::Value {
    loop {
        if let Message::Text(t) = ws.read().expect("a message") {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

#[test]
fn a_phone_pairs_through_the_daemon_and_then_sees_the_session() {
    let sb = Sandbox::new();
    let d = start_with_tunnel(&sb);
    let mut pc = sb.client();
    let started = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    assert_eq!(started["tunnel"], "fake-link-9.trycloudflare.com");
    let uri = started["uri"].as_str().unwrap();
    let scheme = format!("{}://fake-link-9.trycloudflare.com/", identity::get().name);
    assert!(uri.starts_with(&scheme), "{uri}");
    let token = uri.strip_prefix(&scheme).unwrap().to_owned();

    // The person at `jwn pair` waits for the question; the phone asks.
    let socket = sb.config().socket;
    let person = std::thread::spawn(move || {
        let mut pc = wire::Client::connect(&socket, "pair").unwrap();
        let ask = pc
            .cmd(Cmd::MobileWait { seconds: 10 })
            .unwrap()
            .unwrap()
            .unwrap();
        pc.cmd(Cmd::MobileAnswer { yes: true }).unwrap().unwrap();
        ask
    });
    std::thread::sleep(Duration::from_millis(300));
    let mut ws = phone(started["local"].as_str().unwrap());
    ws.send(Message::text(
        serde_json::json!({"t": "pair", "token": token, "name": "Pixel", "code": "482913"})
            .to_string(),
    ))
    .unwrap();
    let paired = serde_json::from_value::<BridgeMsg>(text(&mut ws)).unwrap();
    assert!(matches!(paired, BridgeMsg::Paired { .. }), "{paired:?}");
    assert_eq!(person.join().unwrap()["code"], "482913");
    let ready = serde_json::from_value::<BridgeMsg>(text(&mut ws)).unwrap();
    assert!(
        matches!(&ready, BridgeMsg::Ready { host: Some(h) } if !h.is_empty() && !h.ends_with(".local")),
        "the phone is told the PC's name: {ready:?}"
    );

    // From here the phone speaks the daemon's own protocol.
    let hello = ClientMsg::Hello {
        client: contract::Peer {
            name: "phone".into(),
            version: "0".into(),
        },
        accepts: [("session".to_owned(), vec![1])].into(),
    };
    ws.send(Message::text(serde_json::to_string(&hello).unwrap()))
        .unwrap();
    let mut root = None;
    while root.is_none() {
        let v = text(&mut ws);
        if let Ok(DaemonMsg::Hello { root: r, .. }) = serde_json::from_value::<DaemonMsg>(v) {
            root = Some(r);
        }
    }
    ws.send(Message::text(
        serde_json::to_string(&ClientMsg::Open {
            node: root.unwrap(),
            epoch: None,
            since: None,
        })
        .unwrap(),
    ))
    .unwrap();
    loop {
        if matches!(
            serde_json::from_value::<DaemonMsg>(text(&mut ws)),
            Ok(DaemonMsg::Snapshot { .. })
        ) {
            break;
        }
    }

    // The link cannot be reconfigured from the phone.
    ws.send(Message::text(
        r#"{"t":"cmd","id":"x1","cmd":{"cmd":"mobile_stop"}}"#,
    ))
    .unwrap();
    loop {
        let v = text(&mut ws);
        if v["id"] == "x1" {
            assert_eq!(v["error"]["code"], "unsupported");
            break;
        }
    }
    d.stop();
}

#[test]
fn a_phone_asking_when_no_one_is_at_the_pc_is_turned_away_and_the_token_is_spent() {
    let sb = Sandbox::new();
    let d = start_with_tunnel(&sb);
    let mut pc = sb.client();
    let started = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    let uri = started["uri"].as_str().unwrap();
    let token = uri.rsplit('/').next().unwrap();
    let mut ws = phone(started["local"].as_str().unwrap());
    ws.send(Message::text(
        serde_json::json!({"t": "pair", "token": token, "name": "Pixel", "code": "482913"})
            .to_string(),
    ))
    .unwrap();
    assert_eq!(
        serde_json::from_value::<BridgeMsg>(text(&mut ws)).unwrap(),
        BridgeMsg::Denied {
            reason: Denied::Timeout
        }
    );
    let reply = pc.cmd(Cmd::MobileAnswer { yes: true }).unwrap();
    assert!(reply.is_err(), "nothing is asking");
    d.stop();
}

#[test]
fn without_a_tunnel_or_a_network_the_link_says_so() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut pc = sb.client();
    let reply = pc.cmd(Cmd::MobileStart {}).unwrap();
    assert!(reply.is_err(), "no way to reach this PC");
    d.stop();
}

/// REQ-86: a daemon that restarts with phones paired opens the link again, on the same port, with
/// nobody running `jwn pair`.
#[test]
fn a_restarted_daemon_is_found_again_by_a_paired_phone_on_the_same_port() {
    let sb = Sandbox::new();
    let mut config = sb.config();
    config.link = LinkConfig {
        tunnel: None,
        resolve: false,
        lan: true,
        lan_ip: Some("127.0.0.1".parse().unwrap()),
    };
    let first = Daemon::start(config.clone()).unwrap();
    let mut pc = sb.client();
    let started = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    let lan: std::net::SocketAddr = started["lan"].as_str().unwrap().parse().unwrap();
    let uri = started["uri"].as_str().unwrap();
    let fp = uri.split("fp=").nth(1).unwrap().to_owned();
    let token = uri
        .split('/')
        .nth(3)
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_owned();

    let socket = sb.config().socket;
    let person = std::thread::spawn(move || {
        let mut pc = wire::Client::connect(&socket, "pair").unwrap();
        pc.cmd(Cmd::MobileWait { seconds: 10 }).unwrap().unwrap();
        pc.cmd(Cmd::MobileAnswer { yes: true }).unwrap().unwrap();
    });
    std::thread::sleep(Duration::from_millis(300));
    let mut ws = phone(started["local"].as_str().unwrap());
    ws.send(Message::text(
        serde_json::json!({"t": "pair", "token": token, "name": "Pixel", "code": "482913"})
            .to_string(),
    ))
    .unwrap();
    let Ok(BridgeMsg::Paired { device, secret }) =
        serde_json::from_value::<BridgeMsg>(text(&mut ws))
    else {
        panic!("not paired");
    };
    person.join().unwrap();
    drop(ws);
    first.stop();

    // Nobody runs `jwn pair` now: the phone only knows the address and the fingerprint it paired with.
    let second = Daemon::start(config).unwrap();
    let mut tls = None;
    for _ in 0..40 {
        if let Ok(sock) = std::net::TcpStream::connect(lan) {
            tls = Some(sock);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let sock = tls.expect("the link is open again on the same port");
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let conn = rustls::ClientConnection::new(
        web::tls::Pinned::client_config(&fp).unwrap(),
        "jwn.local".try_into().unwrap(),
    )
    .unwrap();
    let req = format!("ws://{lan}").into_client_request().unwrap();
    let (mut ws, _) = tungstenite::client(req, rustls::StreamOwned::new(conn, sock)).unwrap();
    ws.send(Message::text(
        serde_json::json!({"t": "auth", "device": device, "secret": secret}).to_string(),
    ))
    .unwrap();
    let ready = loop {
        if let Message::Text(t) = ws.read().unwrap() {
            break serde_json::from_str::<BridgeMsg>(&t).unwrap();
        }
    };
    assert!(matches!(ready, BridgeMsg::Ready { .. }), "{ready:?}");
    second.stop();
}
