//! The daemon as a real process on a socket in a tempdir (REQ-3, 4, 5, 10, 14).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use jw::client::Client;
use jw::daemon::pidfile;
use jw::proto::{ClientMsg, DaemonMsg, PaneId, PaneInfo};

const EXE: &str = env!("CARGO_BIN_EXE_jw");
const TIMEOUT: Duration = Duration::from_secs(10);

/// A daemon we started; killed when the test ends.
struct Daemon {
    child: Child,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("jw.sock");
        let child = Command::new(EXE)
            .arg("daemon")
            .env("JW_SOCKET", &socket)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while Client::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "daemon never listened");
            thread::sleep(Duration::from_millis(20));
        }
        Self {
            child,
            socket,
            _dir: dir,
        }
    }

    fn client(&self) -> Client {
        let c = Client::connect(&self.socket).unwrap();
        c.set_read_timeout(Some(TIMEOUT)).unwrap();
        c
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(c: &mut Client, stream: &str, cmd: &str, cols: u16, rows: u16) -> PaneId {
    c.send(&ClientMsg::Spawn {
        stream: stream.into(),
        role: "shell".into(),
        cmd: Some(cmd.into()),
        cwd: std::env::temp_dir(),
        env: BTreeMap::from([("JW_NAME".into(), stream.into())]),
        cols,
        rows,
    })
    .unwrap();
    match recv(c) {
        DaemonMsg::Spawned { pane } => pane,
        other => panic!("want Spawned, got {other:?}"),
    }
}

fn recv(c: &mut Client) -> DaemonMsg {
    c.recv().unwrap().expect("daemon hung up")
}

/// Feeds Output for `pane` into `screen` until its text contains `want`.
fn read_until(c: &mut Client, pane: PaneId, screen: &mut vt100::Parser, want: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while !screen.screen().contents().contains(want) {
        assert!(Instant::now() < deadline, "never saw {want:?}");
        match recv(c) {
            DaemonMsg::Output { pane: p, bytes } if p == pane => screen.process(&bytes),
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    }
}

/// Attaches and returns the first message, which must be the pane's Snapshot,
/// rendered into a parser of the size it reports.
fn attach(c: &mut Client, stream: &str, pane: PaneId) -> vt100::Parser {
    c.send(&ClientMsg::Attach {
        stream: stream.into(),
    })
    .unwrap();
    match recv(c) {
        DaemonMsg::Snapshot {
            pane: p,
            cols,
            rows,
            bytes,
            ..
        } if p == pane => {
            let mut screen = vt100::Parser::new(rows, cols, 0);
            screen.process(&bytes);
            screen
        }
        other => panic!("want Snapshot first, got {other:?}"),
    }
}

#[test]
fn panes_survive_the_client_and_reattach_with_their_screen() {
    let d = Daemon::start();
    let mut c1 = d.client();
    let pane = spawn(&mut c1, "s1", "cat", 80, 24);
    c1.send(&ClientMsg::Input {
        pane,
        bytes: b"hello jw\n".to_vec(),
    })
    .unwrap();
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c1, pane, &mut screen, "hello jw");
    drop(c1);

    let mut c2 = d.client();
    let screen = attach(&mut c2, "s1", pane);
    assert!(screen.screen().contents().contains("hello jw"));

    // Still the same live `cat`, and output reaches the new client.
    let mut screen = screen;
    c2.send(&ClientMsg::Input {
        pane,
        bytes: b"again\n".to_vec(),
    })
    .unwrap();
    read_until(&mut c2, pane, &mut screen, "again");
}

#[test]
fn attach_only_sends_the_streams_panes_and_detach_stops_output() {
    let d = Daemon::start();
    let mut c = d.client();
    let a = spawn(&mut c, "a", "cat", 80, 24);
    let _b = spawn(&mut c, "b", "cat", 80, 24);

    let mut watcher = d.client();
    attach(&mut watcher, "a", a);
    watcher.send(&ClientMsg::Detach).unwrap();
    c.send(&ClientMsg::Input {
        pane: a,
        bytes: b"quiet\n".to_vec(),
    })
    .unwrap();
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, a, &mut screen, "quiet");

    watcher
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    assert!(
        watcher.recv().is_err(),
        "a detached client must get nothing"
    );
}

#[test]
fn list_reports_every_pane() {
    let d = Daemon::start();
    let mut c = d.client();
    let a = spawn(&mut c, "a", "cat", 80, 24);
    let b = spawn(&mut c, "b", "cat", 80, 24);

    let mut other = d.client();
    other.send(&ClientMsg::List).unwrap();
    let panes = loop {
        if let DaemonMsg::Panes { panes } = recv(&mut other) {
            break panes;
        }
    };
    let info = |pane, stream: &str| PaneInfo {
        pane,
        stream: stream.into(),
        role: "shell".into(),
        exited: None,
        fg: Some("cat".into()),
    };
    assert_eq!(panes, [info(a, "a"), info(b, "b")]);
}

#[test]
fn resize_reaches_the_pty_and_the_screen() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "s", "read _; stty size; cat", 80, 24);
    c.send(&ClientMsg::Resize {
        pane,
        cols: 100,
        rows: 30,
    })
    .unwrap();
    c.send(&ClientMsg::Input {
        pane,
        bytes: b"\n".to_vec(),
    })
    .unwrap();
    let mut screen = vt100::Parser::new(30, 100, 0);
    read_until(&mut c, pane, &mut screen, "30 100");

    let mut other = d.client();
    other
        .send(&ClientMsg::Attach { stream: "s".into() })
        .unwrap();
    match recv(&mut other) {
        DaemonMsg::Snapshot {
            cols: 100,
            rows: 30,
            ..
        } => {}
        m => panic!("snapshot should report the new size, got {m:?}"),
    }
}

#[test]
fn an_exited_pane_keeps_its_screen_and_status() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "s", "echo hi; exit 3", 80, 24);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "never exited");
        if let DaemonMsg::Exited { pane: p, status } = recv(&mut c) {
            assert_eq!((p, status), (pane, 3));
            break;
        }
    }

    let mut late = d.client();
    let screen = attach(&mut late, "s", pane);
    assert!(screen.screen().contents().contains("hi"));
    match recv(&mut late) {
        DaemonMsg::Exited { pane: p, status: 3 } if p == pane => {}
        m => panic!("want Exited after the snapshot, got {m:?}"),
    }

    late.send(&ClientMsg::Kill { pane }).unwrap();
    late.send(&ClientMsg::Input {
        pane,
        bytes: b"x".to_vec(),
    })
    .unwrap();
    assert!(
        matches!(recv(&mut late), DaemonMsg::Error { .. }),
        "killed pane is gone"
    );
}

#[test]
fn the_pane_gets_the_env_and_its_id() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(
        &mut c,
        "envs",
        "echo \"[$JW_NAME:$JW_PANE_ID]\"; cat",
        80,
        24,
    );
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, &format!("[envs:{pane}]"));
}

#[test]
fn a_second_daemon_on_the_same_socket_refuses() {
    let d = Daemon::start();
    let out = Command::new(EXE)
        .arg("daemon")
        .env("JW_SOCKET", &d.socket)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already listening"));
}

/// Kills a daemon the client started, found through its pidfile.
struct Started(PathBuf);

impl Drop for Started {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(pidfile(&self.0)) {
            let _ = Command::new("kill")
                .arg(pid.trim())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

fn autostart(socket: &Path) -> (Client, Started) {
    let c = Client::connect_or_start(socket, Path::new(EXE)).unwrap();
    c.set_read_timeout(Some(TIMEOUT)).unwrap();
    (c, Started(socket.to_path_buf()))
}

#[test]
fn the_client_starts_the_daemon_when_nobody_listens() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run/jw.sock");
    let (mut c, _guard) = autostart(&socket);
    let pane = spawn(&mut c, "s", "cat", 80, 24);
    drop(c);

    // Connecting again reuses the same daemon: the pane is still there.
    let (mut c, _guard2) = autostart(&socket);
    attach(&mut c, "s", pane);
}

#[test]
fn a_stale_socket_file_does_not_block_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("jw.sock");
    // A daemon killed with SIGKILL leaves its socket file behind. (Binding a
    // listener in this process instead races with the other tests' forks on
    // macOS, where CLOEXEC is set after socket() and a child can inherit it.)
    let mut crashed = Command::new(EXE)
        .arg("daemon")
        .env("JW_SOCKET", &socket)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    while Client::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon never listened");
        thread::sleep(Duration::from_millis(20));
    }
    crashed.kill().unwrap();
    crashed.wait().unwrap();
    assert!(socket.exists());

    let (mut c, _guard) = autostart(&socket);
    spawn(&mut c, "s", "cat", 80, 24);
}

#[test]
fn prompt_waits_for_quiet_then_pastes_and_presses_enter() {
    let d = Daemon::start();
    let mut c = d.client();
    // An "agent" that asks for bracketed paste and shows its raw input.
    c.send(&ClientMsg::Spawn {
        stream: "s".into(),
        role: "agent".into(),
        cmd: Some("printf '\\033[?2004h'; stty raw -echo; cat -v".into()),
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    })
    .unwrap();
    let pane = match recv(&mut c) {
        DaemonMsg::Spawned { pane } => pane,
        other => panic!("want Spawned, got {other:?}"),
    };

    c.send(&ClientMsg::Prompt {
        stream: "s".into(),
        text: "fix the\nbug".into(),
    })
    .unwrap();
    let mut screen = vt100::Parser::new(24, 80, 0);
    let deadline = Instant::now() + TIMEOUT;
    let mut prompted = false;
    while !(prompted && screen.screen().contents().contains("^M")) {
        assert!(Instant::now() < deadline, "prompt never arrived");
        match recv(&mut c) {
            DaemonMsg::Output { pane: p, bytes } if p == pane => screen.process(&bytes),
            DaemonMsg::Prompted { pane: p } => {
                assert_eq!(p, pane);
                prompted = true;
            }
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    }
    let text = screen.screen().contents();
    assert!(text.contains("^[[200~fix the"), "{text}");
    assert!(text.contains("bug^[[201~^M"), "{text}");
}

#[test]
fn prompt_without_an_agent_pane_is_an_error() {
    let d = Daemon::start();
    let mut c = d.client();
    spawn(&mut c, "s", "cat", 80, 24);
    c.send(&ClientMsg::Prompt {
        stream: "s".into(),
        text: "hi".into(),
    })
    .unwrap();
    loop {
        match recv(&mut c) {
            DaemonMsg::Error { msg } => {
                assert!(msg.contains("no agent"), "{msg}");
                break;
            }
            DaemonMsg::Prompted { .. } => panic!("there is no agent"),
            _ => {}
        }
    }
}
