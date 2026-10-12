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
fn watchers_are_told_of_a_new_size_once_and_in_order_with_the_output() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "s", "cat", 80, 24);
    let mut watcher = d.client();
    attach(&mut watcher, "s", pane);

    // The same size twice is one change; the bytes after it come after the notice.
    for _ in 0..2 {
        c.send(&ClientMsg::Resize {
            pane,
            cols: 100,
            rows: 30,
        })
        .unwrap();
    }
    c.send(&ClientMsg::Input {
        pane,
        bytes: b"after\n".to_vec(),
    })
    .unwrap();

    let mut sizes = Vec::new();
    let mut seen = String::new();
    let deadline = Instant::now() + TIMEOUT;
    while !seen.contains("after") {
        assert!(Instant::now() < deadline, "never saw the output");
        match recv(&mut watcher) {
            DaemonMsg::Resized {
                pane: p,
                cols,
                rows,
            } if p == pane => {
                assert!(seen.is_empty(), "the notice came after output: {seen:?}");
                sizes.push((cols, rows));
            }
            DaemonMsg::Output { pane: p, bytes } if p == pane => {
                seen.push_str(&String::from_utf8_lossy(&bytes))
            }
            _ => {}
        }
    }
    assert_eq!(sizes, [(100, 30)]);
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

/// Mobile REQ-76: a pane split from a phone, which sends no env, still gets its neighbour's `JW_*`.
#[test]
fn a_split_without_env_takes_its_neighbours() {
    let d = Daemon::start();
    let mut c = d.client();
    let a = spawn(&mut c, "kin", "cat", 80, 24);
    let mut new = cat();
    new.cmd = Some("echo \"[$JW_NAME]\"; cat".into());
    c.send(&ClientMsg::Split {
        pane: a,
        dir: Dir::Right,
        new,
    })
    .unwrap();
    let opened = loop {
        if let DaemonMsg::Opened { pane } = next(&mut c) {
            break pane;
        }
    };
    // Attached to the stream, the pane's greeting comes as its Snapshot or as Output.
    let _ = attach(&mut c, "kin", a);
    let mut heard = String::new();
    while !heard.contains("[kin]") {
        match next(&mut c) {
            DaemonMsg::Snapshot { pane, bytes, .. } | DaemonMsg::Output { pane, bytes }
                if pane == opened =>
            {
                heard.push_str(&String::from_utf8_lossy(&bytes));
            }
            _ => {}
        }
    }
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
        pane: None,
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

/// REQ-168, REQ-171, REQ-177: a review opens as the diff beside the agent
/// (the tree `Stream` gives it), the diff has no process, and the prompt the
/// TUI sends after `Open` reaches the agent. The review adds no message.
#[test]
fn a_review_opens_as_a_diff_and_an_agent_and_the_agent_is_briefed() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = |role: &str, cmd: Option<&str>| NewPane {
        role: role.into(),
        cmd: cmd.map(Into::into),
        ..cat()
    };
    let mut leaves = [
        pane("view:diff", None),
        pane(
            "agent",
            Some("printf '\\033[?2004h'; stty raw -echo; cat -v"),
        ),
    ]
    .into_iter();
    let tree = Tree::from_node(&jw_core::stream::review_tree(), &mut leaves).unwrap();
    c.send(&ClientMsg::Open {
        stream: "r".into(),
        tree,
    })
    .unwrap();
    let t = next_tree(&mut c);
    let roles: Vec<_> = t.leaves().iter().map(|l| l.role.clone()).collect();
    assert_eq!(roles, ["view:diff", "agent"]);
    let agent = ids(&t)[1];

    c.send(&ClientMsg::Prompt {
        stream: "r".into(),
        text: "Review pull request #7.".into(),
        pane: None,
    })
    .unwrap();
    c.send(&ClientMsg::List).unwrap();
    let mut screen = vt100::Parser::new(24, 80, 0);
    let deadline = Instant::now() + TIMEOUT;
    let (mut prompted, mut listed) = (false, false);
    while !(prompted && listed && screen.screen().contents().contains("^M")) {
        assert!(Instant::now() < deadline, "the agent was never briefed");
        match recv(&mut c) {
            DaemonMsg::Output { pane: p, bytes } if p == agent => screen.process(&bytes),
            DaemonMsg::Prompted { pane: p } => {
                assert_eq!(p, agent);
                prompted = true;
            }
            DaemonMsg::Panes { panes } => {
                assert_eq!(panes.len(), 1, "only the agent runs: {panes:?}");
                listed = true;
            }
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    }
    let text = screen.screen().contents();
    assert!(
        text.contains("^[[200~Review pull request #7.^[[201~^M"),
        "{text}"
    );
}

#[test]
fn prompt_without_an_agent_pane_is_an_error() {
    let d = Daemon::start();
    let mut c = d.client();
    spawn(&mut c, "s", "cat", 80, 24);
    c.send(&ClientMsg::Prompt {
        stream: "s".into(),
        text: "hi".into(),
        pane: None,
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

/// REQ-205: a Split answers its requester with the new pane's id, and only
/// the requester; a watcher learns of it through the Tree alone. A viewer
/// gets an id and no process (REQ-181).
#[test]
fn a_split_tells_only_its_requester_the_new_pane() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::Open {
        stream: "w".into(),
        tree: Tree::Leaf(cat()),
    })
    .unwrap();
    let a = ids(&next_tree(&mut c))[0];
    let mut watcher = d.client();
    watcher
        .send(&ClientMsg::Attach { stream: "w".into() })
        .unwrap();
    next_tree(&mut watcher);

    let mut view = cat();
    view.role = "view:diff".into();
    c.send(&ClientMsg::Split {
        pane: a,
        dir: Dir::Right,
        new: view,
    })
    .unwrap();
    let opened = loop {
        if let DaemonMsg::Opened { pane } = next(&mut c) {
            break pane;
        }
    };
    assert_eq!(ids(&next_tree(&mut watcher)), [a, opened]);

    // Nothing Opened reaches the watcher: the next thing it hears is the
    // Tree of a later change.
    c.send(&ClientMsg::Name {
        pane: a,
        name: Some("x".into()),
    })
    .unwrap();
    loop {
        match next(&mut watcher) {
            DaemonMsg::Opened { .. } => panic!("the watcher got Opened"),
            DaemonMsg::Tree { .. } => break,
            _ => {}
        }
    }

    c.send(&ClientMsg::List).unwrap();
    loop {
        if let DaemonMsg::Panes { panes } = next(&mut c) {
            assert!(panes.iter().all(|p| p.pane != opened), "{panes:?}");
            break;
        }
    }
}

/// REQ-204..221: `jw pane open|ls|close` against a running daemon, in a
/// state dir of its own with one folder workspace, which a TUI would show.
#[test]
fn jw_pane_opens_lists_and_closes_panes() {
    let d = Daemon::start();
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let dir = work.path().canonicalize().unwrap();
    let dir = dir.to_str().unwrap().to_string();
    let main = state.path().join("jw").join("sessions").join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(
        main.join("folders.json"),
        format!(r#"{{"folders":[{{"dir":{dir:?},"created":"2026-01-01T00:00:00Z","name":"w"}}]}}"#),
    )
    .unwrap();
    let jw = |args: &[&str]| {
        Command::new(EXE)
            .args(args)
            .env("JW_SOCKET", &d.socket)
            .env("XDG_STATE_HOME", state.path())
            .env_remove("JW_SESSION")
            .env_remove("JW_PANE_ID")
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| {
        let out = jw(args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };

    // A workspace the daemon does not run.
    let out = jw(&["pane", "ls", "w"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not open"));

    let mut tui = d.client();
    tui.send(&ClientMsg::Open {
        stream: format!("dir:{dir}"),
        tree: Tree::Leaf(cat()),
    })
    .unwrap();
    let first = ids(&next_tree(&mut tui))[0];

    // The TUI sees the pane with no action of its own (REQ-209).
    let diff: PaneId = ok(&["pane", "open", "w", "--run", "view:diff"])
        .trim()
        .parse()
        .unwrap();
    let t = next_tree(&mut tui);
    assert_eq!(ids(&t), [first, diff]);
    assert_eq!(t.find(diff).unwrap().role, "view:diff");

    let run: PaneId = ok(&[
        "pane",
        "open",
        "w",
        "--run",
        "cat",
        "--split",
        "down",
        "--of",
        &first.to_string(),
    ])
    .trim()
    .parse()
    .unwrap();
    let t = next_tree(&mut tui);
    assert_eq!(ids(&t), [first, run, diff]);
    assert_eq!(t.find(run).unwrap().role, "run");

    let table = ok(&["pane", "ls", "w"]);
    assert!(
        table.contains("view:diff") && table.contains("viewer"),
        "{table}"
    );
    assert!(
        table.contains("run") && table.contains("running"),
        "{table}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&ok(&["pane", "ls", "w", "--json"])).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 3);

    // A pane outside the workspace is refused; so is the last one, below.
    assert!(
        !jw(&["pane", "open", "w", "--run", "cat", "--of", "999999"])
            .status
            .success()
    );
    assert!(!jw(&["pane", "close", "w", "999999"]).status.success());

    ok(&["pane", "close", "w", &run.to_string()]);
    assert_eq!(ids(&next_tree(&mut tui)), [first, diff]);
    ok(&["pane", "close", "w", &diff.to_string()]);
    assert_eq!(ids(&next_tree(&mut tui)), [first]);
    let out = jw(&["pane", "close", "w", &first.to_string()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--force"));
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

/// REQ-194: a `shell` pane with an agent in the foreground takes `jw prompt`.
#[test]
fn prompt_reaches_an_agent_program_in_a_shell_pane() {
    let d = Daemon::start();
    let mut c = d.client();
    // A link to `cat` named `claude`: the foreground process is called that
    // (a script would show as its interpreter, a copy of `cat` is killed by
    // macOS's code signing).
    let bin = tempfile::tempdir().unwrap();
    let claude = bin.path().join("claude");
    std::os::unix::fs::symlink("/bin/cat", &claude).unwrap();
    let pane = spawn(&mut c, "s", &format!("exec {}", claude.display()), 80, 24);

    let mut screen = vt100::Parser::new(24, 80, 0);
    let deadline = Instant::now() + TIMEOUT;
    let reply = loop {
        c.send(&ClientMsg::Prompt {
            stream: "s".into(),
            text: "hi there".into(),
            pane: None,
        })
        .unwrap();
        let msg = loop {
            if let m @ (DaemonMsg::Prompted { .. } | DaemonMsg::Error { .. }) = recv(&mut c) {
                break m;
            }
        };
        // `exec` may not have run yet: ask again until the name says claude.
        if matches!(msg, DaemonMsg::Prompted { .. }) || Instant::now() > deadline {
            break msg;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        matches!(reply, DaemonMsg::Prompted { pane: p } if p == pane),
        "{reply:?}"
    );
    read_until(&mut c, pane, &mut screen, "hi there");
}

/// One prompt into `pane` of `stream`, and the daemon's answer.
fn prompt_once(c: &mut Client, stream: &str, pane: PaneId, text: &str) -> DaemonMsg {
    c.send(&ClientMsg::Prompt {
        stream: stream.into(),
        text: text.into(),
        pane: Some(pane),
    })
    .unwrap();
    loop {
        if let m @ (DaemonMsg::Prompted { .. } | DaemonMsg::Error { .. }) = recv(c) {
            return m;
        }
    }
}

/// Ask until the pane's program is the agent: right after a `spawn` its `exec`
/// may not have run, and the daemon says "it runs sh".
fn prompt_named(c: &mut Client, stream: &str, pane: PaneId, text: &str) -> DaemonMsg {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let msg = prompt_once(c, stream, pane, text);
        let retry = matches!(&msg, DaemonMsg::Error { msg } if msg.contains("it runs"));
        if !retry || Instant::now() > deadline {
            return msg;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// A link to `cat` named `claude` (see the REQ-194 test above), in its own dir.
fn fake_claude() -> (tempfile::TempDir, String) {
    let bin = tempfile::tempdir().unwrap();
    let claude = bin.path().join("claude");
    std::os::unix::fs::symlink("/bin/cat", &claude).unwrap();
    let cmd = format!("exec {}", claude.display());
    (bin, cmd)
}

/// REQ-249: `--pane` sends the text to the named agent, not the lowest one.
#[test]
fn prompt_with_a_pane_reaches_that_agent() {
    let d = Daemon::start();
    let mut c = d.client();
    let (_bin, cmd) = fake_claude();
    let first = spawn(&mut c, "s", &cmd, 80, 24);
    let second = spawn(&mut c, "s", &cmd, 80, 24);
    assert!(first < second);

    let reply = prompt_named(&mut c, "s", second, "for the second");
    assert!(
        matches!(reply, DaemonMsg::Prompted { pane } if pane == second),
        "{reply:?}"
    );
    let mut screen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, second, &mut screen, "for the second");
}

/// REQ-250, REQ-251: a pane of another workspace, a pane that is no agent and
/// one that has exited are refused with an `Error`.
#[test]
fn prompt_with_a_pane_refuses_what_is_not_a_live_agent_of_the_workspace() {
    let d = Daemon::start();
    let mut c = d.client();
    let (_bin, cmd) = fake_claude();
    let _agent = spawn(&mut c, "s", &cmd, 80, 24);
    let elsewhere = spawn(&mut c, "t", &cmd, 80, 24);
    let plain = spawn(&mut c, "s", "cat", 80, 24);
    let gone = spawn(&mut c, "s", "exit 0", 80, 24);

    let DaemonMsg::Error { msg } = prompt_named(&mut c, "s", elsewhere, "x") else {
        panic!("a pane of another workspace took a prompt");
    };
    assert!(msg.contains("not in this workspace"), "{msg}");

    let DaemonMsg::Error { msg } = prompt_once(&mut c, "s", plain, "x") else {
        panic!("a plain `cat` took a prompt");
    };
    assert!(msg.contains("is not an agent"), "{msg}");

    let deadline = Instant::now() + TIMEOUT;
    loop {
        let reply = prompt_named(&mut c, "s", gone, "x");
        let DaemonMsg::Error { msg } = reply else {
            panic!("an exited pane took a prompt: {reply:?}");
        };
        if msg.contains("has exited") {
            break;
        }
        assert!(Instant::now() < deadline, "{msg}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// REQ-192: a client that asks for `SplitSnapshots` gets the screen first and the
/// history behind it, and the two together are what one `Snapshot` was.
#[test]
fn a_split_client_gets_the_screen_then_the_history() {
    let d = Daemon::start();
    let mut c = d.client();
    let pane = spawn(&mut c, "s", "seq 1 60; cat", 80, 24);
    let mut seen = vt100::Parser::new(24, 80, 0);
    read_until(&mut c, pane, &mut seen, "60");

    let mut plain = d.client();
    plain
        .send(&ClientMsg::Attach { stream: "s".into() })
        .unwrap();
    let full = match recv(&mut plain) {
        DaemonMsg::Snapshot { bytes, .. } => bytes,
        m => panic!("want a Snapshot, got {m:?}"),
    };

    let mut split = d.client();
    split.send(&ClientMsg::SplitSnapshots).unwrap();
    split
        .send(&ClientMsg::Attach { stream: "s".into() })
        .unwrap();
    let screen = loop {
        match recv(&mut split) {
            DaemonMsg::Screen {
                pane: p,
                cols: 80,
                rows: 24,
                bytes,
                ..
            } if p == pane => break bytes,
            DaemonMsg::Tree { .. } => {}
            m => panic!("want the Screen first, got {m:?}"),
        }
    };
    let history = match recv(&mut split) {
        DaemonMsg::History {
            pane: p,
            cols: 80,
            rows: 24,
            bytes,
        } if p == pane => bytes,
        m => panic!("want the History next, got {m:?}"),
    };
    assert_eq!(history, full, "History is what a Snapshot carried");

    let mut lite = vt100::Parser::new(24, 80, 100);
    lite.process(&screen);
    let mut whole = vt100::Parser::new(24, 80, 100);
    whole.process(&history);
    assert_eq!(lite.screen().contents(), whole.screen().contents());
    lite.screen_mut().set_scrollback(usize::MAX);
    whole.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(
        lite.screen().scrollback(),
        0,
        "the Screen has no scrollback"
    );
    assert!(whole.screen().scrollback() > 0, "the History has it");
}

#[test]
fn jw_session_rm_closes_its_panes_and_forgets_it_but_keeps_the_checkouts() {
    let d = Daemon::start();
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().canonicalize().unwrap().display().to_string();
    let checkout = tempfile::tempdir().unwrap();
    let sessions = state.path().join("jw").join("sessions");
    for name in ["main", "dev"] {
        std::fs::create_dir_all(sessions.join(name)).unwrap();
        std::fs::write(
            sessions.join(name).join("folders.json"),
            format!(
                r#"{{"folders":[{{"dir":{dir:?},"created":"2026-01-01T00:00:00Z","name":"w"}}]}}"#
            ),
        )
        .unwrap();
    }
    // dev has a worktree git cannot read, which counts as work at risk.
    std::fs::write(
        state.path().join("jw").join("workspaces.json"),
        format!(
            r#"{{"entries":[{{"id":"wt1","name":"wt","path":{:?},"session":"dev"}}]}}"#,
            checkout.path().display().to_string()
        ),
    )
    .unwrap();
    let jw = |args: &[&str], session: Option<&str>| {
        let mut c = Command::new(EXE);
        c.args(args)
            .env("JW_SOCKET", &d.socket)
            .env("XDG_STATE_HOME", state.path())
            .env_remove("JW_SESSION");
        if let Some(s) = session {
            c.env("JW_SESSION", s);
        }
        c.output().unwrap()
    };
    let stderr = |o: &std::process::Output| String::from_utf8_lossy(&o.stderr).into_owned();

    let dev_stream = jw_core::folders::id_in("dev", &dir);
    let main_stream = jw_core::folders::id_in("main", &dir);
    let mut c = d.client();
    spawn(&mut c, &dev_stream, "cat", 80, 24);
    spawn(&mut c, &main_stream, "cat", 80, 24);

    // The session you are in, and one that is not there.
    let out = jw(&["session", "rm", "dev"], Some("dev"));
    assert!(!out.status.success() && stderr(&out).contains("the session you are in"));
    let out = jw(&["session", "rm", "nope"], None);
    assert!(!out.status.success() && stderr(&out).contains("no session nope"));

    // At risk: nothing is removed without --yes.
    let out = jw(&["session", "rm", "dev"], None);
    assert!(!out.status.success(), "{out:?}");
    assert!(stderr(&out).contains("work at risk"), "{}", stderr(&out));
    assert!(sessions.join("dev").is_dir());

    let out = jw(&["session", "rm", "dev", "--yes"], None);
    assert!(out.status.success(), "{out:?}");
    assert!(!sessions.join("dev").exists());
    assert!(checkout.path().is_dir(), "the checkout stays");
    let reg = std::fs::read_to_string(state.path().join("jw").join("workspaces.json")).unwrap();
    assert!(!reg.contains("wt1"));

    // Only dev's pane went.
    let mut l = d.client();
    l.send(&ClientMsg::List).unwrap();
    let listed = loop {
        if let DaemonMsg::Panes { panes } = next(&mut l) {
            break panes;
        }
    };
    assert!(listed.iter().all(|p| p.stream != dev_stream));
    assert!(listed.iter().any(|p| p.stream == main_stream));
    assert!(sessions.join("main").is_dir());
}

/// The text of a row of runs.
fn row_text(row: &[jw_proto::proto::Run]) -> String {
    row.iter().map(|r| r.1.as_str()).collect()
}

#[test]
fn a_text_view_client_gets_the_screen_as_styled_rows_and_no_output() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::TextView).unwrap();
    let pane = spawn(
        &mut c,
        "s",
        "printf 'hello \\033[31mred\\033[0m\\n'; sleep 30",
        40,
        5,
    );
    let deadline = Instant::now() + TIMEOUT;
    let lines = loop {
        assert!(Instant::now() < deadline, "no screen with the text");
        match recv(&mut c) {
            DaemonMsg::Text {
                pane: p,
                cols,
                rows,
                lines,
                ..
            } if p == pane && row_text(&lines[0]).contains("hello red") => {
                assert_eq!((cols, rows, lines.len()), (40, 5, 5));
                break lines;
            }
            DaemonMsg::Output { .. } => panic!("a text client got terminal output"),
            _ => {}
        }
    };
    // `red` is in palette colour 1; the blank end of the row is not sent.
    let red = lines[0].iter().find(|r| r.1.starts_with("red")).unwrap();
    assert_eq!((red.0, red.2), (6, 1 << 24 | 1));
    assert_eq!(row_text(&lines[0]), "hello red");
    assert!(lines[4].is_empty());
}

#[test]
fn a_text_view_client_asks_for_the_history_and_gets_the_last_rows_oldest_first() {
    let d = Daemon::start();
    let mut c = d.client();
    c.send(&ClientMsg::TextView).unwrap();
    let pane = spawn(&mut c, "s", "seq 1 60; sleep 30", 20, 5);
    // Wait until the last number is on the screen, and note which row it starts at.
    let deadline = Instant::now() + TIMEOUT;
    let first_on_screen: u32 = loop {
        assert!(Instant::now() < deadline, "seq never finished");
        if let DaemonMsg::Text { lines, .. } = recv(&mut c)
            && lines.iter().any(|l| row_text(l) == "60")
        {
            break row_text(&lines[0]).parse().unwrap();
        }
    };
    c.send(&ClientMsg::TextHistory { pane, count: 10 }).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let lines = loop {
        assert!(Instant::now() < deadline, "no history");
        if let DaemonMsg::TextHistory { pane: p, lines } = recv(&mut c)
            && p == pane
        {
            break lines;
        }
    };
    // The rows just above the screen, oldest first.
    let got: Vec<u32> = lines.iter().map(|l| row_text(l).parse().unwrap()).collect();
    let want: Vec<u32> = (first_on_screen - 10..first_on_screen).collect();
    assert_eq!(got, want);
}

#[test]
fn a_client_that_draws_a_terminal_still_gets_output_while_another_watches_text() {
    let d = Daemon::start();
    let mut text = d.client();
    text.send(&ClientMsg::TextView).unwrap();
    let pane = spawn(&mut text, "s", "cat", 40, 5);
    let mut tui = d.client();
    tui.send(&ClientMsg::Attach { stream: "s".into() }).unwrap();
    text.send(&ClientMsg::Input {
        pane,
        bytes: b"ping\n".to_vec(),
    })
    .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let (mut saw_output, mut saw_text) = (false, false);
    while !(saw_output && saw_text) {
        assert!(
            Instant::now() < deadline,
            "output {saw_output} text {saw_text}"
        );
        if let Ok(Some(DaemonMsg::Output { bytes, .. })) = tui.recv() {
            saw_output |= String::from_utf8_lossy(&bytes).contains("ping");
        }
        text.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        if let Ok(Some(DaemonMsg::Text { lines, .. })) = text.recv() {
            saw_text |= lines.iter().any(|l| row_text(l).contains("ping"));
        }
    }
}
