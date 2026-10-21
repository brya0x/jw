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

/// A daemon whose phone link listens on loopback (the private address) and whose tunnel
/// program takes a second to say its host, so that what comes first can be seen.
fn start_with_slow_tunnel(sb: &Sandbox) -> Daemon {
    let script = sb.dir.join("cloudflared");
    std::fs::write(
        &script,
        "#!/bin/sh\nsleep 1\necho 'INF https://slow-link-3.trycloudflare.com' >&2\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
    config.link = LinkConfig {
        tunnel: Some(script.to_string_lossy().into()),
        resolve: false,
        lan: true,
        lan_ip: Some(std::net::Ipv4Addr::LOCALHOST),
    };
    Daemon::start(config).unwrap()
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn token_of(uri: &str) -> String {
    uri.split('/')
        .nth(3)
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_owned()
}

/// REQ-606: a tunnel that ends (the real one died four minutes after it started) is noticed, and
/// another with another host name is opened, with the same pairing token in the link.
#[test]
fn a_tunnel_that_ends_is_replaced_by_another_with_the_same_token() {
    let sb = Sandbox::new();
    let counter = sb.dir.join("runs");
    let script = sb.dir.join("cloudflared");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nn=$(cat {c} 2>/dev/null || echo 0)\nn=$((n+1))\necho $n > {c}\necho \"INF https://again-$n.trycloudflare.com\" >&2\nsleep 2\n",
            c = counter.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
    config.link = LinkConfig {
        tunnel: Some(script.to_string_lossy().into()),
        resolve: false,
        lan: true,
        lan_ip: Some(std::net::Ipv4Addr::LOCALHOST),
    };
    let d = Daemon::start(config).unwrap();
    let mut pc = sb.client();
    let first = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    let token = token_of(first["uri"].as_str().unwrap());
    let tunnel_of = |pc: &mut wire::Client| {
        pc.cmd(Cmd::MobileLink {})
            .unwrap()
            .unwrap()
            .unwrap()
            .get("tunnel")
            .and_then(|t| t.as_str().map(str::to_owned))
    };
    let wait_for = |pc: &mut wire::Client, want: &dyn Fn(Option<&str>) -> bool| {
        let end = std::time::Instant::now() + Duration::from_secs(25);
        loop {
            let now = tunnel_of(pc);
            if want(now.as_deref()) || std::time::Instant::now() > end {
                return now;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let one = wait_for(&mut pc, &|t| t.is_some());
    assert_eq!(one.as_deref(), Some("again-1.trycloudflare.com"));
    // it ends after two seconds; the watcher looks every five and opens another
    let two = wait_for(&mut pc, &|t| {
        t.is_some_and(|t| t != "again-1.trycloudflare.com")
    });
    assert_eq!(
        two.as_deref(),
        Some("again-2.trycloudflare.com"),
        "no second tunnel came"
    );
    let link = pc.cmd(Cmd::MobileLink {}).unwrap().unwrap().unwrap();
    let uri = link["uri"].as_str().unwrap();
    assert!(uri.ends_with("&t=again-2.trycloudflare.com"), "{uri}");
    assert_eq!(
        token_of(uri),
        token,
        "the QR a phone is scanning stays good"
    );
    // turned off, it is not opened again
    pc.cmd(Cmd::MobileStop {}).unwrap().unwrap();
    std::thread::sleep(Duration::from_secs(7));
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().trim(),
        "2",
        "a link that is off opens no tunnel"
    );
    d.stop();
}

#[test]
fn the_private_address_answers_at_once_and_the_tunnel_comes_later_and_stays() {
    let sb = Sandbox::new();
    let d = start_with_slow_tunnel(&sb);
    let mut pc = sb.client();

    let begun = std::time::Instant::now();
    let first = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    assert!(
        begun.elapsed() < Duration::from_millis(700),
        "it did not wait for the tunnel: {:?}",
        begun.elapsed()
    );
    let uri = first["uri"].as_str().unwrap();
    assert!(uri.contains("://127.0.0.1:"), "{uri}");
    assert!(!uri.contains("&t="), "no tunnel yet: {uri}");
    assert_eq!(first["pending"], true);
    assert!(first.get("tunnel").is_none());
    let token = token_of(uri);

    // The same token, with the tunnel in the link, once it is up.
    let mut link = first.clone();
    let end = std::time::Instant::now() + Duration::from_secs(8);
    while link["pending"] == true && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        link = pc.cmd(Cmd::MobileLink {}).unwrap().unwrap().unwrap();
    }
    assert_eq!(link["pending"], false, "{link}");
    assert_eq!(link["tunnel"], "slow-link-3.trycloudflare.com");
    let uri = link["uri"].as_str().unwrap();
    assert!(uri.ends_with("&t=slow-link-3.trycloudflare.com"), "{uri}");
    assert_eq!(
        token_of(uri),
        token,
        "the QR a phone is scanning stays good"
    );

    // The tunnel stays until the link is turned off: asking again, or the client going,
    // leaves it as it is.
    let pidfile = sb.dir.join("state/tunnel.pid");
    let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_owned();
    assert!(alive(&pid));
    let again = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    assert_eq!(again["pending"], false);
    assert_eq!(again["tunnel"], "slow-link-3.trycloudflare.com");
    drop(pc);
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(&pid), "the client going did not end the tunnel");
    assert_eq!(std::fs::read_to_string(&pidfile).unwrap().trim(), pid);

    let mut pc = sb.client();
    pc.cmd(Cmd::MobileStop {}).unwrap().unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while alive(&pid) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(&pid), "turning the link off ends the tunnel");
    d.stop();
}

#[test]
fn a_link_that_is_off_has_nothing_to_show_and_without_a_tunnel_nothing_is_pending() {
    let sb = Sandbox::new();
    let mut config = sb.config();
    config.link = LinkConfig {
        tunnel: None,
        resolve: false,
        lan: true,
        lan_ip: Some(std::net::Ipv4Addr::LOCALHOST),
    };
    let d = Daemon::start(config).unwrap();
    let mut pc = sb.client();
    let e = pc.cmd(Cmd::MobileLink {}).unwrap().unwrap_err();
    assert_eq!(e.code, contract::ErrorCode::Io);
    let started = pc.cmd(Cmd::MobileStart {}).unwrap().unwrap().unwrap();
    assert_eq!(started["pending"], false);
    assert_eq!(
        pc.cmd(Cmd::MobileLink {}).unwrap().unwrap().unwrap()["uri"],
        started["uri"],
        "the same link, the same token"
    );
    d.stop();
}
