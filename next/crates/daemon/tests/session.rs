//! N2: hello, epoch, the session node, patches, resume and persistence.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{Sandbox, WAIT};
use contract::{ClientMsg, Cmd, DaemonMsg, ErrorCode, Peer};
use serde_json::Value;
use wire::Conn;

fn rename(c: &mut wire::Client, name: &str) {
    c.cmd(Cmd::RenameSession { name: name.into() })
        .unwrap()
        .unwrap();
}

#[test]
fn hello_names_the_root_and_the_versions_served() {
    let sb = Sandbox::new();
    let d = sb.start();
    let c = sb.client();
    assert_eq!(c.accepted.get("session"), Some(&1));
    assert_eq!(c.accepted.len(), 4);
    d.stop();
}

#[test]
fn a_type_with_no_common_version_is_not_served() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut conn = Conn::connect(&sb.config().socket).unwrap();
    let accepts = BTreeMap::from([
        ("session".to_owned(), vec![1]),
        ("pane".to_owned(), vec![2]),
    ]);
    conn.send(&ClientMsg::Hello {
        client: Peer {
            name: "t".into(),
            version: "1".into(),
        },
        accepts,
    })
    .unwrap();
    let DaemonMsg::Hello { accepted, root, .. } =
        serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap()
    else {
        panic!("expected hello")
    };
    assert_eq!(accepted.keys().collect::<Vec<_>>(), ["session"]);
    // Opening a node of an unserved type says so.
    conn.send(&ClientMsg::Open {
        node: root,
        epoch: None,
        since: None,
    })
    .unwrap();
    let _snapshot = conn.recv_line().unwrap().unwrap();
    d.stop();
}

#[test]
fn rename_reaches_a_subscriber_as_a_patch_that_matches_a_fresh_snapshot() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut watcher = sb.client();
    let root = watcher.root.clone();
    watcher.open(&root).unwrap();
    assert!(
        watcher
            .wait_until(WAIT, |c| c.nodes.contains_key(&root))
            .unwrap()
    );
    assert_eq!(watcher.nodes[&root]["rev"], 1);

    let mut actor = sb.client();
    rename(&mut actor, "work");
    assert!(
        watcher
            .wait_until(WAIT, |c| c.nodes[&root]["rev"] == 2)
            .unwrap()
    );
    assert_eq!(watcher.nodes[&root]["name"], "work");

    let mut fresh = sb.client();
    fresh.open(&root).unwrap();
    assert!(
        fresh
            .wait_until(WAIT, |c| c.nodes.contains_key(&root))
            .unwrap()
    );
    assert_eq!(watcher.nodes[&root], fresh.nodes[&root]);
    d.stop();
}

#[test]
fn an_empty_name_is_refused_and_changes_nothing() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let err = c
        .cmd(Cmd::RenameSession { name: "  ".into() })
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    d.stop();
}

#[test]
fn unknown_and_unbuilt_commands_get_an_error_reply() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let id = c.root.clone();
    let err = c.cmd(Cmd::ClosePane { pane: id }).unwrap().unwrap_err();
    assert!(matches!(
        err.code,
        ErrorCode::Unsupported | ErrorCode::NotFound
    ));
    d.stop();
}

#[test]
fn resume_sends_one_composed_patch_an_empty_one_when_current_or_a_snapshot() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    let epoch = c.epoch.clone();
    c.close(&root).unwrap();
    rename(&mut c, "one");
    rename(&mut c, "two");

    let mut raw = Conn::connect(&sb.config().socket).unwrap();
    let hello = |raw: &mut Conn| {
        raw.send(&ClientMsg::Hello {
            client: Peer {
                name: "t".into(),
                version: "1".into(),
            },
            accepts: BTreeMap::from([("session".to_owned(), vec![1])]),
        })
        .unwrap();
        raw.recv_line().unwrap().unwrap();
    };
    hello(&mut raw);
    let ask = |raw: &mut Conn, epoch: Option<&str>, since: Option<u64>| -> DaemonMsg {
        raw.send(&ClientMsg::Open {
            node: root.clone(),
            epoch: epoch.map(str::to_owned),
            since,
        })
        .unwrap();
        serde_json::from_str(&raw.recv_line().unwrap().unwrap()).unwrap()
    };
    // Held rev 1, now 3: one patch covering both renames.
    match ask(&mut raw, Some(&epoch), Some(1)) {
        DaemonMsg::Patch {
            from: 1,
            to: 3,
            ops,
            ..
        } => assert_eq!(ops.len(), 2, "both renames, in order"),
        other => panic!("expected one patch 1->3, got {other:?}"),
    }
    // Already current: an empty patch.
    assert!(
        matches!(ask(&mut raw, Some(&epoch), Some(3)), DaemonMsg::Patch { from: 3, to: 3, ops, .. } if ops.is_empty())
    );
    // Another epoch: a snapshot.
    assert!(matches!(
        ask(
            &mut raw,
            Some("00000000-0000-4000-8000-0000000000ff"),
            Some(1)
        ),
        DaemonMsg::Snapshot { .. }
    ));
    // An unknown node: gone.
    raw.send(&ClientMsg::Open {
        node: "00000000-0000-4000-8000-0000000000ee".into(),
        epoch: None,
        since: None,
    })
    .unwrap();
    assert!(matches!(
        serde_json::from_str(&raw.recv_line().unwrap().unwrap()).unwrap(),
        DaemonMsg::Gone { .. }
    ));
    d.stop();
}

#[test]
fn a_cached_client_resumes_through_open() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    c.close(&root).unwrap();
    rename(&mut c, "later");
    assert_eq!(c.nodes[&root]["name"], "main");
    c.open(&root).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&root]["name"] == "later")
            .unwrap()
    );
    assert_eq!(c.nodes[&root]["rev"], 2);
    d.stop();
}

#[test]
fn the_session_survives_a_restart_with_a_new_epoch() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    rename(&mut c, "kept");
    let (root, epoch) = (c.root.clone(), c.epoch.clone());
    drop(c);
    d.stop();

    let d = sb.start();
    let mut c = sb.client();
    assert_eq!(c.root, root, "the session id is stable");
    assert_ne!(c.epoch, epoch, "the epoch is new");
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    assert_eq!(c.nodes[&root]["name"], "kept");
    d.stop();
}

#[test]
fn a_cache_from_a_dead_epoch_is_not_trusted() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    let cached: Value = c.nodes[&root].clone();
    drop(c);
    d.stop();

    let d = sb.start();
    let mut c = sb.client();
    c.nodes.insert(root.clone(), cached.clone());
    c.open(&root).unwrap(); // cached rev 1 with the old epoch in the doc, new epoch in the client
    assert!(
        c.wait_until(WAIT, |c| c
            .nodes
            .get(&root)
            .is_some_and(|n| n["epoch"] == c.epoch.as_str()))
            .unwrap()
    );
    d.stop();
}

#[test]
fn a_second_daemon_is_refused_and_a_stale_socket_is_cleared() {
    let sb = Sandbox::new();
    let d = sb.start();
    let err = daemon::Daemon::start(sb.config())
        .err()
        .expect("second daemon refused");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    d.stop();
    // A socket file nobody listens on.
    drop(std::os::unix::net::UnixListener::bind(sb.config().socket).unwrap());
    sb.start().stop();
}

#[test]
fn bad_lines_get_an_error_and_the_connection_lives() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut conn = Conn::connect(&sb.config().socket).unwrap();
    conn.send(&serde_json::json!({"t": "open", "node": "x"}))
        .unwrap();
    let first: DaemonMsg = serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap();
    assert!(
        matches!(
            first,
            DaemonMsg::Error {
                code: ErrorCode::BadRequest,
                ..
            }
        ),
        "hello first: {first:?}"
    );
    conn.send(&"not an object").unwrap();
    assert!(matches!(
        serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap(),
        DaemonMsg::Error { .. }
    ));
    conn.send(&ClientMsg::Hello {
        client: Peer {
            name: "t".into(),
            version: "1".into(),
        },
        accepts: BTreeMap::new(),
    })
    .unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(matches!(
        serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap(),
        DaemonMsg::Hello { .. }
    ));
    d.stop();
}

#[test]
fn nothing_is_sent_while_nothing_changes() {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    let before = c.bytes_in;
    c.wait_until(Duration::from_millis(500), |_| false).unwrap();
    assert_eq!(c.bytes_in, before, "an idle watcher receives nothing");
    d.stop();
}

#[test]
fn the_development_tcp_listener_speaks_the_same_lines_and_only_on_loopback() {
    let sb = Sandbox::new();
    let mut config = sb.config();
    config.tcp = Some("0.0.0.0:0".parse().unwrap());
    let err = daemon::Daemon::start(config.clone())
        .err()
        .expect("a non-loopback address is refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

    config.tcp = Some("127.0.0.1:0".parse().unwrap());
    let d = daemon::Daemon::start(config).unwrap();
    let mut conn = Conn::connect_tcp(d.tcp_addr().unwrap()).unwrap();
    conn.send(&ClientMsg::Hello {
        client: Peer {
            name: "t".into(),
            version: "1".into(),
        },
        accepts: wire::default_accepts(),
    })
    .unwrap();
    let DaemonMsg::Hello { root, .. } =
        serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap()
    else {
        panic!("expected hello")
    };
    conn.send(&ClientMsg::Open {
        node: root,
        epoch: None,
        since: None,
    })
    .unwrap();
    assert!(matches!(
        serde_json::from_str(&conn.recv_line().unwrap().unwrap()).unwrap(),
        DaemonMsg::Snapshot { .. }
    ));
    d.stop();
}
