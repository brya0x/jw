//! The daemon hosting mobile (mobile spec, REQ-9, 33 to 40), as a real
//! process on a socket in a tempdir.
//!
//! No test here can start a real tunnel: every daemon gets a `PATH` without
//! Homebrew, and `JW_CLOUDFLARED` names a stub that prints what `cloudflared`
//! does and then `exec`s `sleep`, so killing the stub's pid kills the sleep
//! too. State (`mobile.json`, settings, themes) lives in the test's own dir.

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use jw_proto::client::Client;
use jw_proto::proto::{ClientMsg, DaemonMsg, MobileState, PROTOCOL};
use tungstenite::{Message, WebSocket};

const EXE: &str = env!("CARGO_BIN_EXE_jw");
const TIMEOUT: Duration = Duration::from_secs(10);
const HOST: &str = "jw-test-stub.trycloudflare.com";

/// What the stand-in for `cloudflared` does.
#[derive(Clone, Copy, PartialEq)]
enum Stub {
    /// Prints the tunnel's name and the registered line, then runs on.
    Registers,
    /// Prints nothing and runs on: the tunnel never comes up.
    Hangs,
    /// No `JW_CLOUDFLARED`, and no `cloudflared` on `PATH`.
    Missing,
}

struct Rig {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
    _owned: Option<tempfile::TempDir>,
}

impl Rig {
    fn start(stub: Stub) -> Self {
        let owned = tempfile::tempdir().unwrap();
        let dir = owned.path().to_path_buf();
        let mut rig = Self::start_in(&dir, stub);
        rig._owned = Some(owned);
        rig
    }

    /// A daemon on the state of an earlier one, as after a restart.
    fn start_in(dir: &Path, stub: Stub) -> Self {
        let socket = dir.join("jw.sock");
        let script = dir.join("stub.sh");
        let body = match stub {
            Stub::Registers => format!(
                "#!/bin/sh\necho $$ > \"$STUB_DIR/pid\"\necho \"$@\" > \"$STUB_DIR/args\"\n\
                 echo \"INF |  https://{HOST}  |\" >&2\n\
                 echo \"INF Registered tunnel connection connIndex=0\" >&2\n\
                 exec sleep 600\n"
            ),
            _ => "#!/bin/sh\necho $$ > \"$STUB_DIR/pid\"\necho \"$@\" > \"$STUB_DIR/args\"\nexec sleep 600\n"
                .to_string(),
        };
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_file(dir.join("pid"));
        let mut cmd = Command::new(EXE);
        cmd.arg("daemon")
            .env_clear()
            .env("JW_SOCKET", &socket)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("HOME", dir)
            .env("STUB_DIR", dir)
            .env("PATH", "/usr/bin:/bin");
        if stub != Stub::Missing {
            cmd.env("JW_CLOUDFLARED", &script);
        }
        let child = cmd.spawn().unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while Client::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "daemon never listened");
            thread::sleep(Duration::from_millis(20));
        }
        Self {
            child,
            socket,
            dir: dir.to_path_buf(),
            _owned: None,
        }
    }

    fn client(&self) -> Client {
        let mut c = Client::connect(&self.socket).unwrap();
        c.set_read_timeout(Some(TIMEOUT)).unwrap();
        c.send(&ClientMsg::Hello { protocol: PROTOCOL }).unwrap();
        c
    }

    /// A client that watches, with its first state read.
    fn watcher(&self) -> Client {
        let mut c = self.client();
        c.send(&ClientMsg::MobileWatch).unwrap();
        state(&mut c);
        c
    }

    /// The pid the stub wrote, once it has.
    fn stub_pid(&self) -> i32 {
        let path = self.dir.join("pid");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(s) = std::fs::read_to_string(&path)
                && let Ok(pid) = s.trim().parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "the stub never ran");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The port the bridge listens on, as `cloudflared` was told.
    fn bridge_port(&self) -> u16 {
        let args = std::fs::read_to_string(self.dir.join("args")).unwrap();
        let url = args
            .split_whitespace()
            .find(|a| a.starts_with("http://"))
            .unwrap();
        url.rsplit(':').next().unwrap().parse().unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The stub, if it is still there, then the daemon: only our own pids.
        if let Ok(s) = std::fs::read_to_string(self.dir.join("pid"))
            && let Ok(pid) = s.trim().parse::<i32>()
            && alive(pid)
        {
            // SAFETY: a pid this test's daemon started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn gone(pid: i32, within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if !alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    !alive(pid)
}

/// The next `MobileState`.
fn state(c: &mut Client) -> MobileState {
    loop {
        match c.recv().unwrap().expect("the daemon hung up") {
            DaemonMsg::MobileState(s) => return s,
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    }
}

/// States until one satisfies `done`, and every one seen.
fn until(c: &mut Client, done: impl Fn(&MobileState) -> bool) -> Vec<MobileState> {
    let mut seen = Vec::new();
    loop {
        let s = state(c);
        let stop = done(&s);
        seen.push(s);
        if stop {
            return seen;
        }
    }
}

fn ready(c: &mut Client) -> MobileState {
    until(c, |s| s.phase == "ready").pop().unwrap()
}

fn token(s: &MobileState) -> String {
    s.payload.rsplit('/').next().unwrap().to_string()
}

struct Phone(WebSocket<TcpStream>);

impl Phone {
    fn connect(port: u16) -> Self {
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        Self(
            tungstenite::client(format!("ws://127.0.0.1:{port}"), s)
                .unwrap()
                .0,
        )
    }

    fn say(&mut self, v: serde_json::Value) {
        self.0.send(Message::text(v.to_string())).unwrap();
    }

    fn json(&mut self) -> serde_json::Value {
        loop {
            match self.0.read().unwrap() {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Close(_) => panic!("the bridge closed the phone"),
                _ => {}
            }
        }
    }
}

/// Starts mobile and waits for its code.
fn up(watcher: &mut Client) -> MobileState {
    watcher
        .send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    ready(watcher)
}

#[test]
fn start_walks_the_steps_to_ready_and_the_stub_runs() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    w.send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    let seen = until(&mut w, |s| s.phase == "ready");
    let steps: Vec<&str> = seen.iter().map(|s| s.step.as_str()).collect();
    assert!(steps.contains(&"launching"), "{steps:?}");
    assert!(steps.contains(&"started"), "{steps:?}");
    assert!(steps.contains(&"registered"), "{steps:?}");
    let s = seen.last().unwrap();
    assert_eq!(s.host, HOST);
    assert!(
        s.payload.starts_with(&format!("jw://{HOST}/")),
        "{}",
        s.payload
    );
    assert_eq!(token(s).len(), 32);
    assert!(
        s.expires_in > 100 && s.expires_in <= 120,
        "{}",
        s.expires_in
    );
    assert!(alive(rig.stub_pid()));
}

#[test]
fn the_daemon_starts_with_mobile_off_and_a_restart_leaves_it_off() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    assert!(!rig.dir.join("pid").exists(), "the stub ran by itself");
    up(&mut w);
    let pid = rig.stub_pid();
    let dir = rig.dir.clone();
    let mut rig = rig;
    let _ = rig.child.kill();
    let _ = rig.child.wait();
    // SAFETY: the stub of the daemon above.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    assert!(gone(pid, TIMEOUT));
    let _ = std::fs::remove_file(dir.join("jw.sock"));
    let again = Rig::start_in(&dir, Stub::Registers);
    let mut c = again.client();
    c.send(&ClientMsg::MobileWatch).unwrap();
    assert_eq!(state(&mut c).phase, "off");
    thread::sleep(Duration::from_millis(300));
    assert!(!dir.join("pid").exists(), "a restart started mobile");
    drop(again);
}

#[test]
fn start_twice_changes_nothing() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let first = up(&mut w);
    let pid = rig.stub_pid();
    let mut other = rig.client();
    other
        .send(&ClientMsg::MobileStart { read_only: true })
        .unwrap();
    let again = state(&mut other);
    assert_eq!(again.phase, "ready");
    assert_eq!(again.payload, first.payload);
    assert!(!again.read_only, "a second start changed the mode");
    assert_eq!(rig.stub_pid(), pid);
}

#[test]
fn stop_kills_the_stub_and_closes_the_bridge() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    up(&mut w);
    let (pid, port) = (rig.stub_pid(), rig.bridge_port());
    w.send(&ClientMsg::MobileStop).unwrap();
    assert_eq!(
        until(&mut w, |s| s.phase == "off").last().unwrap().payload,
        ""
    );
    assert!(
        gone(pid, Duration::from_secs(2)),
        "the stub is still running"
    );
    let until_closed = Instant::now() + Duration::from_secs(2);
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(Instant::now() < until_closed, "the bridge still listens");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_kills_the_stub_before_the_daemon_exits() {
    let mut rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    up(&mut w);
    let pid = rig.stub_pid();
    // SAFETY: the daemon this test started.
    unsafe { libc::kill(rig.child.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(8);
    while rig.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the daemon did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        gone(pid, Duration::from_secs(1)),
        "the stub outlived the daemon"
    );
}

#[test]
fn a_stop_while_starting_ends_it_and_a_later_start_works() {
    let rig = Rig::start(Stub::Hangs);
    let mut w = rig.watcher();
    w.send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    until(&mut w, |s| s.step == "started");
    let first = rig.stub_pid();
    w.send(&ClientMsg::MobileStop).unwrap();
    until(&mut w, |s| s.phase == "off");
    assert!(gone(first, Duration::from_secs(2)));
    // The start it interrupted must not turn the state back on.
    thread::sleep(Duration::from_millis(700));
    let mut probe = rig.client();
    probe.send(&ClientMsg::MobileWatch).unwrap();
    assert_eq!(state(&mut probe).phase, "off");

    let _ = std::fs::remove_file(rig.dir.join("pid"));
    w.send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    until(&mut w, |s| s.step == "started");
    assert_ne!(rig.stub_pid(), first);
}

#[test]
fn without_cloudflared_it_fails_with_a_reason_and_try_again_starts_again() {
    let rig = Rig::start(Stub::Missing);
    let mut w = rig.watcher();
    for _ in 0..2 {
        w.send(&ClientMsg::MobileStart { read_only: false })
            .unwrap();
        let failed = until(&mut w, |s| s.phase == "failed").pop().unwrap();
        assert!(failed.error.contains("not installed"), "{}", failed.error);
        assert!(failed.payload.is_empty());
    }
}

#[test]
fn a_pane_keeps_answering_while_the_tunnel_hangs() {
    let rig = Rig::start(Stub::Hangs);
    let mut w = rig.watcher();
    w.send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    until(&mut w, |s| s.step == "started");

    let mut c = rig.client();
    c.send(&ClientMsg::Spawn {
        stream: "s".into(),
        role: "shell".into(),
        cmd: Some("cat".into()),
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    })
    .unwrap();
    let pane = loop {
        match c.recv().unwrap().expect("the daemon hung up") {
            DaemonMsg::Spawned { pane } => break pane,
            DaemonMsg::Error { msg } => panic!("daemon error: {msg}"),
            _ => {}
        }
    };
    c.send(&ClientMsg::Input {
        pane,
        bytes: b"still here\n".to_vec(),
    })
    .unwrap();
    let mut screen = vt100::Parser::new(24, 80, 0);
    let deadline = Instant::now() + TIMEOUT;
    while !screen.screen().contents().contains("still here") {
        assert!(Instant::now() < deadline, "the pane stopped answering");
        if let Some(DaemonMsg::Output { bytes, .. }) = c.recv().unwrap() {
            screen.process(&bytes);
        }
    }
    // And mobile is still starting, not done.
    c.send(&ClientMsg::MobileWatch).unwrap();
    assert_eq!(state(&mut c).phase, "starting");
}

/// Pairs a phone through `asker`, which says yes; returns it and its secret.
fn pair(rig: &Rig, code_state: &MobileState, asker: &mut Client, name: &str) -> (Phone, String) {
    let mut phone = Phone::connect(rig.bridge_port());
    phone.say(serde_json::json!({
        "t": "pair", "token": token(code_state), "name": name, "code": "482913",
    }));
    loop {
        if let DaemonMsg::MobilePair {
            request,
            name: n,
            code,
        } = asker.recv().unwrap().expect("the daemon hung up")
        {
            assert_eq!((n.as_str(), code.as_str()), (name, "482913"));
            asker
                .send(&ClientMsg::MobileAnswer { request, yes: true })
                .unwrap();
            break;
        }
    }
    let paired = phone.json();
    assert_eq!(paired["t"], "paired", "{paired}");
    let secret = paired["device"].as_str().unwrap().to_string();
    assert_eq!(phone.json()["t"], "ready");
    (phone, secret)
}

#[test]
fn two_watchers_are_asked_the_first_answer_wins_and_the_other_is_told() {
    let rig = Rig::start(Stub::Registers);
    let mut a = rig.watcher();
    let mut b = rig.watcher();
    let code = up(&mut a);
    let _ = ready(&mut b);

    let mut phone = Phone::connect(rig.bridge_port());
    phone.say(serde_json::json!({
        "t": "pair", "token": token(&code), "name": "Pixel 8", "code": "482913",
    }));
    let next_pair = |c: &mut Client| loop {
        if let DaemonMsg::MobilePair {
            request,
            name,
            code,
        } = c.recv().unwrap().unwrap()
        {
            assert_eq!((name.as_str(), code.as_str()), ("Pixel 8", "482913"));
            return request;
        }
    };
    let (ra, rb) = (next_pair(&mut a), next_pair(&mut b));
    assert_eq!(ra, rb);
    b.send(&ClientMsg::MobileAnswer {
        request: rb,
        yes: true,
    })
    .unwrap();
    assert_eq!(phone.json()["t"], "paired");
    loop {
        if a.recv().unwrap().unwrap() == (DaemonMsg::MobilePairDone { request: ra }) {
            break;
        }
    }
    let s = until(&mut a, |s| s.devices.len() == 1).pop().unwrap();
    assert_eq!(s.devices[0].name, "Pixel 8");
    assert_eq!(s.payload, "", "the code was spent");
}

#[test]
fn no_watcher_means_the_phone_is_refused() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let code = up(&mut w);
    drop(w);
    // The daemon notices the client left once it reads the hang-up.
    thread::sleep(Duration::from_millis(500));
    let mut phone = Phone::connect(rig.bridge_port());
    phone.say(serde_json::json!({
        "t": "pair", "token": token(&code), "name": "Pixel", "code": "482913",
    }));
    let v = phone.json();
    assert_eq!(v["code"], "denied", "{v}");
}

#[test]
fn a_new_code_ends_the_old_one() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let first = up(&mut w);
    w.send(&ClientMsg::MobileCode).unwrap();
    let second = until(&mut w, |s| {
        !s.payload.is_empty() && s.payload != first.payload
    })
    .pop()
    .unwrap();
    assert_ne!(token(&first), token(&second));
    let mut phone = Phone::connect(rig.bridge_port());
    phone.say(serde_json::json!({
        "t": "pair", "token": token(&first), "name": "Pixel", "code": "482913",
    }));
    assert_eq!(phone.json()["code"], "unknown");
}

#[test]
fn read_only_reaches_a_phone_that_comes_after() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let code = up(&mut w);
    let (phone, secret) = pair(&rig, &code, &mut w, "Pixel");
    drop(phone);
    w.send(&ClientMsg::MobileReadOnly { on: true }).unwrap();
    assert!(until(&mut w, |s| s.read_only).last().unwrap().read_only);
    let mut later = Phone::connect(rig.bridge_port());
    later.say(serde_json::json!({ "t": "auth", "device": secret }));
    let v = later.json();
    assert_eq!(v["t"], "ready", "{v}");
    assert_eq!(v["read_only"], true);
}

#[test]
fn revoking_cuts_a_connected_phone_and_drops_it_from_the_list() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let code = up(&mut w);
    let (mut phone, _) = pair(&rig, &code, &mut w, "Pixel");
    let listed = until(&mut w, |s| s.devices.iter().any(|d| d.connected))
        .pop()
        .unwrap();
    let id = listed.devices[0].id.clone();
    w.send(&ClientMsg::MobileRevoke { device: id }).unwrap();
    let t = Instant::now();
    let v = phone.json();
    assert_eq!(v["code"], "revoked", "{v}");
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(
        until(&mut w, |s| s.devices.is_empty())
            .last()
            .unwrap()
            .devices
            .is_empty()
    );
}

#[test]
fn ready_carries_the_palettes_and_the_slots() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    let code = up(&mut w);
    let (_phone, secret) = pair(&rig, &code, &mut w, "Pixel");
    let mut later = Phone::connect(rig.bridge_port());
    later.say(serde_json::json!({ "t": "auth", "device": secret }));
    let v = later.json();
    assert!(v["palettes"].as_array().unwrap().len() >= 10, "{v}");
    assert_eq!(v["settings"]["dark"], "one-dark");
    assert_eq!(v["settings"]["light"], "one-light");
}

#[test]
fn cloudflared_dying_while_ready_fails_mobile_and_a_new_start_runs_a_new_stub() {
    let rig = Rig::start(Stub::Registers);
    let mut w = rig.watcher();
    up(&mut w);
    let (pid, port) = (rig.stub_pid(), rig.bridge_port());
    // SAFETY: the stub of the daemon this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let failed = until(&mut w, |s| s.phase == "failed").pop().unwrap();
    assert!(
        failed.error.contains("cloudflared stopped"),
        "{}",
        failed.error
    );
    assert!(failed.payload.is_empty());
    let until_closed = Instant::now() + Duration::from_secs(3);
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(Instant::now() < until_closed, "the bridge still listens");
        thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(rig.dir.join("pid"));
    w.send(&ClientMsg::MobileStart { read_only: false })
        .unwrap();
    ready(&mut w);
    assert_ne!(rig.stub_pid(), pid);
}

#[test]
fn a_paired_phone_is_told_the_workspaces_and_sessions_of_the_pcs_jw() {
    use jw_core::core::registry::{Entry, Registry};
    use jw_core::folders::{Folder, Folders};
    let rig = Rig::start(Stub::Registers);
    // What the TUI would have on disk: a plain folder and two worktrees of a
    // project whose folder is not open.
    let state = rig.dir.join("state").join("jw");
    let main = jw_core::session::dir(&state, "main").unwrap();
    std::fs::create_dir_all(&main).unwrap();
    Folders {
        folders: vec![Folder {
            dir: "/nowhere/notes".into(),
            opened: false,
            created: "2026-10-11T00:00:00Z".into(),
            agent: None,
            plain: true,
            name: Some("Notes".into()),
        }],
    }
    .save(&main.join("folders.json"))
    .unwrap();
    let wt = |id: &str, name: &str, pr: u64| Entry {
        id: id.into(),
        name: name.into(),
        project: "jw".into(),
        branch: format!("feat/{name}"),
        path: format!("/nowhere/jw-wt/{name}"),
        pr,
        root: "/nowhere/jw".into(),
        ..Entry::default()
    };
    Registry {
        entries: vec![wt("jw-beta", "beta", 0), wt("jw-alpha", "alpha", 43)],
    }
    .save(&state.join("workspaces.json"))
    .unwrap();

    let mut w = rig.watcher();
    let code = up(&mut w);
    let (mut phone, _) = pair(&rig, &code, &mut w, "Pixel");

    phone.say(serde_json::json!({ "t": "workspaces" }));
    let v = phone.json();
    assert_eq!(v["t"], "workspaces", "{v}");
    assert_eq!(v["session"], "main");
    let items = v["items"].as_array().unwrap();
    let names: Vec<(&str, &str)> = items
        .iter()
        .map(|i| (i["kind"].as_str().unwrap(), i["name"].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            ("folder", "Notes"),
            ("root", "jw"),
            ("worktree", "alpha"),
            ("worktree", "beta")
        ],
        "{v}"
    );
    assert_eq!(items[2]["parent"], items[1]["id"]);
    assert_eq!(items[2]["pr"], 43);
    assert_eq!(items[3]["pr"], serde_json::Value::Null);
    assert_eq!(items[2]["branch"], "feat/alpha");
    assert_eq!(items[0]["parent"], serde_json::Value::Null);

    phone.say(serde_json::json!({ "t": "sessions" }));
    let v = phone.json();
    assert_eq!(
        v["items"],
        serde_json::json!([{ "name": "main", "workspaces": 3 }]),
        "{v}"
    );

    phone.say(serde_json::json!({ "t": "workspaces", "session": "nope" }));
    assert_eq!(phone.json()["code"], "unsupported");
}
