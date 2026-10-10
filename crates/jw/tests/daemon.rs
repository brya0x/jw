//! The daemon as a real process on a socket in a tempdir (REQ-3, 4, 5, 10, 14).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use jw_core::layout::{Dir, Tree};
use jw_proto::client::Client;
use jw_proto::proto::pidfile;
use jw_proto::proto::{ClientMsg, DaemonMsg, NewPane, PROTOCOL, PaneId, PaneInfo, PaneLeaf, Theme};

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

    /// Kills the daemon and starts another on the same socket, as after a
    /// crash or a reboot.
    fn restart(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child = Command::new(EXE)
            .arg("daemon")
            .env("JW_SOCKET", &self.socket)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while Client::connect(&self.socket).is_err() {
            assert!(Instant::now() < deadline, "daemon never listened");
            thread::sleep(Duration::from_millis(20));
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

/// The next message that isn't a workspace's Tree (tests of single panes
/// don't care how they are laid out).
fn recv(c: &mut Client) -> DaemonMsg {
    loop {
        match next(c) {
            DaemonMsg::Tree { .. } => {}
            m => return m,
        }
    }
}

fn next(c: &mut Client) -> DaemonMsg {
    c.recv().unwrap().expect("daemon hung up")
}

/// Skips messages until the next Tree, and returns its panes in order.
fn next_tree(c: &mut Client) -> Tree<PaneLeaf> {
    loop {
        if let DaemonMsg::Tree { tree, .. } = next(c) {
            return tree;
        }
    }
}

fn ids(t: &Tree<PaneLeaf>) -> Vec<PaneId> {
    t.leaves().into_iter().map(|l| l.id).collect()
}

fn cat() -> NewPane {
    NewPane {
        role: "shell".into(),
        cmd: Some("cat".into()),
        resume: None,
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    }
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
    // A round trip, so the Detach is done before the output below.
    watcher.send(&ClientMsg::List).unwrap();
    while !matches!(next(&mut watcher), DaemonMsg::Panes { .. }) {}
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
    let a = spawn(&mut c, "a", "exec cat", 80, 24);
    let b = spawn(&mut c, "b", "exec cat", 80, 24);

    // Linux's dash keeps running `sh -c cat` as sh, with cat its child; with
    // exec the pane's process is cat everywhere, once sh has exec'd: ask
    // until it has.
    let mut other = d.client();
    let deadline = Instant::now() + TIMEOUT;
    let panes = loop {
        other.send(&ClientMsg::List).unwrap();
        let panes = loop {
            if let DaemonMsg::Panes { panes } = recv(&mut other) {
                break panes;
            }
        };
        if panes.iter().all(|p| p.fg.as_deref() == Some("cat")) || Instant::now() > deadline {
            break panes;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let info = |pane, stream: &str| PaneInfo {
        pane,
        stream: stream.into(),
        role: "shell".into(),
        exited: None,
        fg: Some("cat".into()),
        busy: false,
        bell: false,
        agent: None,
        cwd: None,
    };
    // Every pane says where it is; whether they printed in the last 2 s
    // depends on timing.
    assert!(
        panes
            .iter()
            .all(|p| p.cwd.as_deref().is_some_and(|d| !d.is_empty()))
    );
    let panes: Vec<PaneInfo> = panes
        .into_iter()
        .map(|p| PaneInfo {
            busy: false,
            cwd: None,
            ..p
        })
        .collect();
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

#[test]
fn hello_answers_with_the_protocol() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::Hello { protocol: PROTOCOL }).unwrap();
    assert_eq!(next(&mut c), DaemonMsg::Hello { protocol: PROTOCOL });
}

/// REQ-34: every change to a workspace's panes reaches its tree, the
/// clients watching it, and session.json.
#[test]
fn the_daemon_keeps_each_workspaces_tree() {
    let d = Daemon::start();
    let mut c = d.client();
    let tree = Tree::Split {
        dir: Dir::Right,
        ratio: 0.5,
        a: Box::new(Tree::Leaf(cat())),
        b: Box::new(Tree::Leaf(cat())),
    };
    c.send(&ClientMsg::Open {
        stream: "w".into(),
        tree,
    })
    .unwrap();
    let t = next_tree(&mut c);
    let [a, b] = ids(&t)[..] else {
        panic!("want two panes, got {t:?}")
    };
    for _ in 0..2 {
        assert!(matches!(next(&mut c), DaemonMsg::Snapshot { .. }));
    }

    // A second client sees the same tree, then every later change.
    let mut watcher = d.client();
    watcher
        .send(&ClientMsg::Attach { stream: "w".into() })
        .unwrap();
    assert_eq!(ids(&next_tree(&mut watcher)), [a, b]);

    c.send(&ClientMsg::Split {
        pane: a,
        dir: Dir::Down,
        new: cat(),
    })
    .unwrap();
    let t = next_tree(&mut c);
    let new = ids(&t)[1];
    assert_eq!(ids(&t), [a, new, b]);
    assert_eq!(ids(&next_tree(&mut watcher)), [a, new, b]);
    // The watcher gets the new pane's screen, then its output.
    loop {
        match next(&mut watcher) {
            DaemonMsg::Snapshot { pane, .. } if pane == new => break,
            DaemonMsg::Snapshot { .. } | DaemonMsg::Output { .. } => {}
            m => panic!("want the new pane's Snapshot, got {m:?}"),
        }
    }

    c.send(&ClientMsg::Swap { a, b }).unwrap();
    assert_eq!(ids(&next_tree(&mut c)), [b, new, a]);

    c.send(&ClientMsg::Name {
        pane: a,
        name: Some(" logs ".into()),
    })
    .unwrap();
    let t = next_tree(&mut c);
    let named: Vec<Option<String>> = t.leaves().into_iter().map(|l| l.name.clone()).collect();
    assert_eq!(named, [None, None, Some("logs".to_string())]);

    c.send(&ClientMsg::Kill { pane: new }).unwrap();
    assert_eq!(ids(&next_tree(&mut c)), [b, a]);

    let session = d.socket.with_file_name("session.json");
    let saved = std::fs::read_to_string(&session).unwrap();
    assert!(saved.contains("\"logs\""), "{saved}");
    assert!(saved.contains("\"id\": \"w\""), "{saved}");

    c.send(&ClientMsg::Close { stream: "w".into() }).unwrap();
    c.send(&ClientMsg::List).unwrap();
    loop {
        if let DaemonMsg::Panes { panes } = next(&mut c) {
            assert!(panes.is_empty(), "{panes:?}");
            break;
        }
    }
    let saved = std::fs::read_to_string(&session).unwrap();
    assert!(!saved.contains("\"w\""), "{saved}");
}

#[test]
fn the_window_title_a_program_sets_arrives() {
    let d = Daemon::start();
    let mut c = d.client();
    let mut pane = cat();
    pane.cmd = Some("printf '\\033]2;hello there\\007'; cat".into());
    c.send(&ClientMsg::Open {
        stream: "t".into(),
        tree: Tree::Leaf(pane),
    })
    .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "no Title");
        if let DaemonMsg::Title { title, .. } = next(&mut c) {
            assert_eq!(title, "hello there");
            break;
        }
    }
    // A client attaching later gets it with the screen.
    let mut late = d.client();
    late.send(&ClientMsg::Attach { stream: "t".into() })
        .unwrap();
    loop {
        if let DaemonMsg::Title { title, .. } = next(&mut late) {
            assert_eq!(title, "hello there");
            break;
        }
    }
}

/// Viewers (REQ-57): a leaf with no process, kept in the tree and the
/// session, removed like any pane.
#[test]
fn a_viewer_is_a_leaf_without_a_process() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::Open {
        stream: "v".into(),
        tree: Tree::Leaf(cat()),
    })
    .unwrap();
    let shell = ids(&next_tree(&mut c))[0];
    let mut view = cat();
    view.role = "view:diff".into();
    c.send(&ClientMsg::Split {
        pane: shell,
        dir: Dir::Right,
        new: view,
    })
    .unwrap();
    let t = next_tree(&mut c);
    let diff = ids(&t)[1];
    assert_eq!(t.find(diff).unwrap().role, "view:diff");

    c.send(&ClientMsg::List).unwrap();
    loop {
        if let DaemonMsg::Panes { panes } = next(&mut c) {
            assert_eq!(panes.len(), 1, "only the shell runs: {panes:?}");
            break;
        }
    }
    let saved = std::fs::read_to_string(d.socket.with_file_name("session.json")).unwrap();
    assert!(saved.contains("view:diff"), "{saved}");

    c.send(&ClientMsg::Name {
        pane: diff,
        name: Some("review".into()),
    })
    .unwrap();
    assert_eq!(
        next_tree(&mut c).find(diff).unwrap().name.as_deref(),
        Some("review")
    );
    // REQ-71: a reused reader keeps the file it shows now.
    c.send(&ClientMsg::Role {
        pane: diff,
        role: "view:md:docs/x.md".into(),
    })
    .unwrap();
    assert_eq!(
        next_tree(&mut c).find(diff).unwrap().role,
        "view:md:docs/x.md"
    );
    c.send(&ClientMsg::Role {
        pane: shell,
        role: "view:diff".into(),
    })
    .unwrap();
    assert!(matches!(next(&mut c), DaemonMsg::Error { .. }));

    c.send(&ClientMsg::Kill { pane: diff }).unwrap();
    assert_eq!(ids(&next_tree(&mut c)), [shell]);
}

/// REQ-51: List says which panes printed lately and which rang the bell;
/// typing into a pane clears its bell.
#[test]
fn list_reports_busy_and_the_bell() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "b", "printf 'ready\\a'; cat", 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "ready");
    let list = |c: &mut Client| {
        c.send(&ClientMsg::List).unwrap();
        let panes = loop {
            if let DaemonMsg::Panes { panes } = recv(c) {
                break panes;
            }
        };
        panes
            .into_iter()
            .find(|p| p.pane == pane)
            .map(|p| (p.busy, p.bell))
            .unwrap()
    };
    let deadline = Instant::now() + TIMEOUT;
    let mut quiet = list(&mut c);
    assert!(quiet.1, "the bell rang");
    while quiet.0 {
        assert!(Instant::now() < deadline, "never went quiet");
        thread::sleep(Duration::from_millis(300));
        quiet = list(&mut c);
    }
    assert_eq!(quiet, (false, true));
    c.send(&ClientMsg::Input {
        pane,
        bytes: b"x".to_vec(),
    })
    .unwrap();
    assert!(!list(&mut c).1, "input clears the bell");
}

/// REQ-15: a new daemon starts the workspaces session.json describes, with
/// their names, and runs an agent's resume command instead of its start.
/// REQ-114: the modes the old process turned on are off in the new one.
#[test]
fn a_restarted_daemon_brings_the_workspaces_back() {
    let mut d = Daemon::start();
    let mut c = d.client();
    let mut agent = cat();
    agent.role = "agent".into();
    agent.cmd =
        Some(r"printf '\033[?1003h\033[?1006h\033[?2004h\033[?1h'; echo started; cat".into());
    agent.resume = Some("echo resumed; cat".into());
    c.send(&ClientMsg::Open {
        stream: "r".into(),
        tree: Tree::Split {
            dir: Dir::Down,
            ratio: 0.5,
            a: Box::new(Tree::Leaf(agent)),
            b: Box::new(Tree::Leaf(cat())),
        },
    })
    .unwrap();
    let ids_before = ids(&next_tree(&mut c));
    // Open doesn't subscribe: attach to watch the agent start.
    c.send(&ClientMsg::Attach { stream: "r".into() }).unwrap();
    let mut before = vt100::Parser::new(24, 80, 0);
    loop {
        if let DaemonMsg::Snapshot { pane, bytes, .. } = recv(&mut c)
            && pane == ids_before[0]
        {
            before.process(&bytes);
            break;
        }
    }
    read_until(&mut c, ids_before[0], &mut before, "started");
    let modes = |p: &vt100::Parser| {
        let s = p.screen();
        (
            s.mouse_protocol_mode(),
            s.mouse_protocol_encoding(),
            s.bracketed_paste(),
            s.application_cursor(),
        )
    };
    assert_eq!(
        modes(&before),
        (
            vt100::MouseProtocolMode::AnyMotion,
            vt100::MouseProtocolEncoding::Sgr,
            true,
            true
        )
    );
    c.send(&ClientMsg::Name {
        pane: ids_before[1],
        name: Some("kept".into()),
    })
    .unwrap();
    while next_tree(&mut c)
        .find(ids_before[1])
        .unwrap()
        .name
        .is_none()
    {}
    drop(c);
    // A change a client has seen is already saved.
    let saved = std::fs::read_to_string(d.socket.with_file_name("session.json")).unwrap();
    assert!(saved.contains("kept"), "{saved}");

    d.restart();
    let mut c = d.client();
    c.send(&ClientMsg::Attach { stream: "r".into() }).unwrap();
    let t = next_tree(&mut c);
    let leaves: Vec<(String, Option<String>)> = t
        .leaves()
        .into_iter()
        .map(|l| (l.role.clone(), l.name.clone()))
        .collect();
    assert_eq!(
        leaves,
        [
            ("agent".to_string(), None),
            ("shell".to_string(), Some("kept".to_string()))
        ]
    );
    let agent = ids(&t)[0];
    let mut screen = vt100::Parser::new(24, 80, 0);
    loop {
        match next(&mut c) {
            DaemonMsg::Snapshot { pane, bytes, .. } if pane == agent => {
                screen.process(&bytes);
                break;
            }
            _ => {}
        }
    }
    read_until(&mut c, agent, &mut screen, "resumed");
    // REQ-72: what it printed before the restart is still there.
    let text = screen.screen().contents();
    assert!(
        text.contains("started") && text.contains("restored"),
        "{text}"
    );
    assert_eq!(
        modes(&screen),
        (
            vt100::MouseProtocolMode::None,
            vt100::MouseProtocolEncoding::Default,
            false,
            false
        )
    );
}

/// REQ-72: a client that attaches late gets the scrollback from before.
#[test]
fn a_late_client_gets_the_scrollback() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "sb", "seq 1 3000; cat", 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "3000");

    let mut late = d.client();
    late.send(&ClientMsg::Attach {
        stream: "sb".into(),
    })
    .unwrap();
    let bytes = loop {
        if let DaemonMsg::Snapshot { pane: p, bytes, .. } = recv(&mut late)
            && p == pane
        {
            break bytes;
        }
    };
    let mut screen = vt100::Parser::new(24, 80, 5000);
    screen.process(&bytes);
    assert!(screen.screen().contents().contains("3000"));
    screen.screen_mut().set_scrollback(usize::MAX);
    let top = screen.screen().contents();
    assert!(top.starts_with("1\n2\n3\n"), "{top}");
}

/// REQ-93, 97, 100: the daemon answers a program's kitty keyboard query
/// before its device attributes query, and a
/// late client's Snapshot ends with the program's whole stack.
#[test]
fn the_kitty_keyboard_flags_are_answered_and_replayed() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(
        &mut c,
        "kk",
        r"printf '\033[>1u\033[>5u\033[?u\033[c'; cat",
        80,
        24,
    );
    // The reply comes in as typed input, and the tty echoes it.
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "[?5u^[[?1;2c");

    let mut late = d.client();
    late.send(&ClientMsg::Attach {
        stream: "kk".into(),
    })
    .unwrap();
    let bytes = loop {
        if let DaemonMsg::Snapshot { pane: p, bytes, .. } = recv(&mut late)
            && p == pane
        {
            break bytes;
        }
    };
    assert!(
        bytes.ends_with(b"\x1b[<17u\x1b[=0u\x1b[>1u\x1b[>5u"),
        "{:?}",
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(40)..])
    );
}

/// REQ-79: SIGTERM saves what the panes printed since the last save.
#[test]
fn sigterm_saves_the_scrollback_before_exiting() {
    let mut d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "term", "echo printed-before-sigterm; cat", 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "printed-before-sigterm");

    let status = Command::new("kill")
        .args(["-TERM", &d.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        d.child.wait().unwrap().success(),
        "the daemon exits cleanly"
    );
    let saved = std::fs::read(
        d.socket
            .with_file_name("scrollback")
            .join(format!("{pane}.bin")),
    )
    .expect("the pane's scrollback is saved");
    assert!(String::from_utf8_lossy(&saved).contains("printed-before-sigterm"));
    assert!(!d.socket.exists(), "the socket is removed");
}

/// REQ-80: `jw server status` and `jw server stop`.
#[test]
fn server_status_and_stop() {
    let mut d = Daemon::start();
    let jw = |arg: &str| {
        let out = Command::new(EXE)
            .args(["server", arg])
            .env("JW_SOCKET", &d.socket)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let mut c = d.client();
    spawn(&mut c, "srv", "cat", 80, 24);
    let status = jw("status");
    assert!(
        status.starts_with("running") && status.contains("1 pane running"),
        "{status}"
    );
    assert!(jw("stop").starts_with("stopped"));
    assert!(d.child.wait().unwrap().success());
    assert!(jw("status").starts_with("not running"));
    assert!(jw("stop").starts_with("not running"));
}

#[test]
fn a_dragged_border_keeps_its_place() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::Open {
        stream: "g".into(),
        tree: Tree::Split {
            dir: Dir::Right,
            ratio: 0.5,
            a: Box::new(Tree::Leaf(cat())),
            b: Box::new(Tree::Leaf(cat())),
        },
    })
    .unwrap();
    next_tree(&mut c);
    c.send(&ClientMsg::Ratio {
        stream: "g".into(),
        path: vec![],
        ratio: 0.3,
    })
    .unwrap();
    assert_eq!(next_tree(&mut c).ratio_at(&[]), Some(0.3));
    c.send(&ClientMsg::Ratio {
        stream: "g".into(),
        path: vec![true],
        ratio: 0.3,
    })
    .unwrap();
    assert!(
        matches!(recv(&mut c), DaemonMsg::Error { .. }),
        "b is a leaf"
    );
}

/// REQ-76: a client that stops reading loses that pane's output instead of
/// growing the daemon, the other client gets everything, and the slow one
/// gets a fresh Snapshot once it reads again.
#[test]
fn a_slow_client_is_resynced_and_holds_nobody_up() {
    let d = Daemon::start();
    let mut fast = d.client();
    let flood = "read x; head -c 12000000 /dev/zero | tr '\\0' x; printf '\\nthe-end\\n'; cat";
    let pane = spawn(&mut fast, "slow", flood, 80, 24);
    let mut slow = d.client();
    attach(&mut slow, "slow", pane);

    fast.send(&ClientMsg::Input {
        pane,
        bytes: b"go\n".to_vec(),
    })
    .unwrap();
    let mut tail = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(
            Instant::now() < deadline,
            "the fast client never saw the end"
        );
        if let DaemonMsg::Output { pane: p, bytes } = recv(&mut fast)
            && p == pane
        {
            tail.extend_from_slice(&bytes);
            let keep = tail.len().saturating_sub(16);
            if String::from_utf8_lossy(&tail).contains("the-end") {
                break;
            }
            tail.drain(..keep);
        }
    }

    let mut got = 0usize;
    loop {
        assert!(
            Instant::now() < deadline,
            "the slow client was never resynced"
        );
        match recv(&mut slow) {
            DaemonMsg::Output { pane: p, bytes } if p == pane => got += bytes.len(),
            DaemonMsg::Snapshot { pane: p, bytes, .. } if p == pane => {
                let mut screen = vt100::Parser::new(24, 80, 0);
                screen.process(&bytes);
                if screen.screen().contents().contains("the-end") {
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(got < 12_000_000, "the slow client got all {got} bytes");
}

/// REQ-73: `jw hook` in a pane tells the daemon what its agent does.
#[test]
fn an_agent_hook_reports_its_state() {
    let d = Daemon::start();
    let mut c = d.client();
    let cmd = format!("{EXE} hook waiting </dev/null; echo hooked; cat");
    let pane = spawn(&mut c, "h", &cmd, 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "hooked");
    // The hook's connection is not this one: its Agent may be handled after
    // our List, so ask until it is.
    let deadline = Instant::now() + TIMEOUT;
    loop {
        c.send(&ClientMsg::List).unwrap();
        let panes = loop {
            if let DaemonMsg::Panes { panes } = recv(&mut c) {
                break panes;
            }
        };
        let p = panes.iter().find(|p| p.pane == pane).unwrap();
        if p.agent == Some(jw_proto::proto::AgentState::Waiting) {
            break;
        }
        assert!(Instant::now() < deadline, "agent state is {:?}", p.agent);
        thread::sleep(Duration::from_millis(20));
    }
}

/// A renamed session's workspace keeps its panes under its new id.
#[test]
fn a_workspace_follows_its_new_id() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "dir:/x#old", "cat", 80, 24);
    c.send(&ClientMsg::Rekey {
        from: "dir:/x#old".into(),
        to: "dir:/x#new".into(),
    })
    .unwrap();
    c.send(&ClientMsg::List).unwrap();
    let panes = loop {
        if let DaemonMsg::Panes { panes } = recv(&mut c) {
            break panes;
        }
    };
    assert_eq!(panes.len(), 1);
    assert_eq!(
        (panes[0].pane, panes[0].stream.as_str()),
        (pane, "dir:/x#new")
    );
    let saved = std::fs::read_to_string(d.socket.with_file_name("session.json")).unwrap();
    assert!(
        saved.contains(r#""id": "dir:/x#new""#) && !saved.contains(r#""id": "dir:/x#old""#),
        "{saved}"
    );
}

/// One Light, as the TUI would report it.
const LIGHT: Theme = Theme {
    dark: false,
    fg: [0x38, 0x3a, 0x42],
    bg: [0xfa, 0xfa, 0xfa],
};

/// A shell command that asks its terminal `query`, then shows what comes
/// back on its input (`cat -v` prints ESC as `^[`).
fn ask(query: &str) -> String {
    format!("stty -echo -icanon; printf '{query}'; cat -v")
}

/// REQ-81, 82, 88, 89: colours, the scheme, the cursor and the attributes
/// are answered from the theme, which a client changes.
#[test]
fn a_program_that_asks_gets_the_theme() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(
        &mut c,
        "q",
        &ask(r"\033]10;?\033\\\033]11;?\007\033[?996n\033[6n\033[c"),
        80,
        24,
    );
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "^[[?1;2c");
    let text = screen.screen().contents();
    for want in [
        r"^[]10;rgb:abab/b2b2/bfbf^[\",
        r"^[]11;rgb:2828/2c2c/3434^[\",
        "^[[?997;1n",
        "^[[1;1R",
    ] {
        assert!(text.contains(want), "no {want:?} in {text}");
    }

    c.send(&ClientMsg::Theme(LIGHT)).unwrap();
    let pane = spawn(
        &mut c,
        "q",
        &format!("echo \"[$COLORFGBG]\"; {}", ask(r"\033]11;?\007")),
        80,
        24,
    );
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut screen, "rgb:fafa/fafa/fafa");
    assert!(screen.screen().contents().contains("[0;15]"));
}

/// REQ-83, 84: a switch reaches the programs that set mode 2031, and only
/// them.
#[test]
fn a_switch_reaches_the_programs_that_asked_for_it() {
    let d = Daemon::start();
    let mut c = d.client();
    let on = spawn(&mut c, "s", &ask(r"\033[?2031h\033[?2031$p"), 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, on, &mut screen, "^[[?2031;1$y");
    let mut c2 = d.client();
    let off = spawn(&mut c2, "s", &ask(r"\033[?2031$p"), 80, 24);
    let mut quiet = vt100::Parser::new(24, 80, 0);
    read_until(&mut c2, off, &mut quiet, "^[[?2031;2$y");

    c.send(&ClientMsg::Theme(LIGHT)).unwrap();
    read_until(&mut c, on, &mut screen, "^[[?997;2n");
    c2.send(&ClientMsg::Input {
        pane: off,
        bytes: b"x".to_vec(),
    })
    .unwrap();
    read_until(&mut c2, off, &mut quiet, "$yx");
    assert!(!quiet.screen().contents().contains("997"));
}

/// REQ-86, 87: the theme outlives the daemon, and a restored pane's old
/// questions are not answered again.
#[test]
fn the_theme_survives_a_restart_and_old_queries_stay_unanswered() {
    let mut d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::Theme(LIGHT)).unwrap();
    let mut agent = cat();
    agent.role = "agent".into();
    agent.cmd = Some(ask(r"\033]11;?\007"));
    agent.resume = Some("stty -echo -icanon; echo resumed; cat -v".into());
    c.send(&ClientMsg::Open {
        stream: "t".into(),
        tree: Tree::Leaf(agent),
    })
    .unwrap();
    let pane = ids(&next_tree(&mut c))[0];
    let mut screen = attach(&mut c, "t", pane);
    read_until(&mut c, pane, &mut screen, "rgb:fafa");
    // A tree change saves the scrollback, query and answer included.
    c.send(&ClientMsg::Name {
        pane,
        name: Some("saved".into()),
    })
    .unwrap();
    while next_tree(&mut c).find(pane).unwrap().name.is_none() {}
    drop(c);
    let saved = std::fs::read_to_string(d.socket.with_file_name("session.json")).unwrap();
    assert!(saved.contains(r#""dark": false"#), "{saved}");

    d.restart();
    let mut c = d.client();
    c.send(&ClientMsg::Attach { stream: "t".into() }).unwrap();
    let pane = ids(&next_tree(&mut c))[0];
    let mut screen = vt100::Parser::new(24, 80, 0);
    loop {
        if let DaemonMsg::Snapshot { pane: p, bytes, .. } = next(&mut c)
            && p == pane
        {
            screen.process(&bytes);
            break;
        }
    }
    read_until(&mut c, pane, &mut screen, "resumed");
    c.send(&ClientMsg::Input {
        pane,
        bytes: b"x".to_vec(),
    })
    .unwrap();
    read_until(&mut c, pane, &mut screen, "x");
    let text = screen.screen().contents();
    assert!(text.contains("restored"), "{text}");
    assert_eq!(text.matches("rgb:").count(), 1, "{text}");

    let fresh = spawn(&mut c, "t", &ask(r"\033]11;?\007"), 80, 24);
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, fresh, &mut screen, "rgb:fafa/fafa/fafa");
}

/// REQ-111: a renamed worktree's panes keep running, and a restart starts
/// them in the folder's new place.
#[test]
fn a_moved_workspace_restarts_in_its_new_folder() {
    let mut d = Daemon::start();
    let dir = d.socket.parent().unwrap().to_path_buf();
    let (old, new) = (dir.join("ws-1"), dir.join("login"));
    std::fs::create_dir(&old).unwrap();
    let mut c = d.client();
    c.send(&ClientMsg::Spawn {
        stream: "m".into(),
        role: "shell".into(),
        cmd: Some("cat".into()),
        cwd: old.clone(),
        env: BTreeMap::from([("JW_NAME".into(), "ws-1".into())]),
        cols: 80,
        rows: 24,
    })
    .unwrap();
    let pane = match recv(&mut c) {
        DaemonMsg::Spawned { pane } => pane,
        other => panic!("want Spawned, got {other:?}"),
    };
    std::fs::rename(&old, &new).unwrap();
    c.send(&ClientMsg::Moved {
        stream: "m".into(),
        from: old.clone(),
        to: new.clone(),
        env: BTreeMap::from([("JW_NAME".into(), "login".into())]),
    })
    .unwrap();
    c.send(&ClientMsg::List).unwrap();
    let panes = loop {
        if let DaemonMsg::Panes { panes } = recv(&mut c) {
            break panes;
        }
    };
    // The same pane, still running.
    assert_eq!(panes.len(), 1);
    assert_eq!((panes[0].pane, panes[0].exited), (pane, None));
    let saved = std::fs::read_to_string(d.socket.with_file_name("session.json")).unwrap();
    let new_s = new.display().to_string();
    assert!(
        saved.contains(&new_s) && !saved.contains(&old.display().to_string()),
        "{saved}"
    );
    assert!(saved.contains(r#""JW_NAME": "login""#), "{saved}");
    drop(c);

    d.restart();
    let mut c = d.client();
    c.send(&ClientMsg::List).unwrap();
    let panes = loop {
        if let DaemonMsg::Panes { panes } = recv(&mut c) {
            break panes;
        }
    };
    assert_eq!(panes.len(), 1, "the workspace came back");
    assert_eq!(panes[0].stream, "m");
}

/// REQ-66: `jw ls` and `jw sessions` read a daemon of this build, and work
/// without one.
#[test]
fn the_cli_reads_the_daemon_or_none() {
    let d = Daemon::start();
    let state = tempfile::tempdir().unwrap();
    let jw = |args: &[&str], socket: &Path| {
        let out = Command::new(EXE)
            .args(args)
            .env("JW_SOCKET", socket)
            .env("XDG_STATE_HOME", state.path())
            .env_remove("JW_SESSION")
            .output()
            .unwrap();
        assert!(out.status.success(), "jw {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let none = d.socket.with_file_name("none.sock");
    for socket in [d.socket.as_path(), none.as_path()] {
        assert_eq!(jw(&["ls", "--json"], socket).trim(), "[]");
        assert!(jw(&["sessions"], socket).contains("main"));
    }
}
