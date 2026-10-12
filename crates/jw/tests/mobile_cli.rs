//! `jw mobile` as a client of the daemon (mobile spec, REQ-30, 41).
//!
//! Every process here gets one environment (`Env::command`): its own socket,
//! config and state, a `PATH` without Homebrew, and `JW_CLOUDFLARED` naming a
//! stub, so nothing can reach the user's daemon or start a real tunnel. The
//! daemon `jw mobile` starts is detached, so `Env` kills it, and the stub, by
//! pid when the test ends, however it ended. A daemon's own log is
//! `jw.log` next to the socket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jw_proto::client::Client;
use jw_proto::proto::{ClientMsg, DaemonMsg, MobileState, PROTOCOL, read_frame, write_frame};
use tungstenite::{Message, WebSocket};

const EXE: &str = env!("CARGO_BIN_EXE_jw");
const TIMEOUT: Duration = Duration::from_secs(15);
const HOST: &str = "jw-test-stub.trycloudflare.com";

#[derive(Clone, Copy, PartialEq)]
enum Stub {
    Registers,
    Missing,
}

struct Env {
    dir: tempfile::TempDir,
    socket: PathBuf,
}

impl Env {
    fn new(stub: Stub) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("stub.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > \"$STUB_DIR/pid\"\necho \"$@\" > \"$STUB_DIR/args\"\n\
                 echo \"INF |  https://{HOST}  |\" >&2\n\
                 echo \"INF Registered tunnel connection connIndex=0\" >&2\n\
                 exec sleep 600\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = dir.path().join("jw.sock");
        let env = Self { dir, socket };
        if stub == Stub::Missing {
            std::fs::remove_file(script).unwrap();
        }
        env
    }

    /// `jw mobile <args>`, in the isolated environment.
    fn command(&self, args: &[&str]) -> Command {
        let dir = self.dir.path();
        let mut cmd = Command::new(EXE);
        cmd.arg("mobile")
            .args(args)
            .env_clear()
            .env("JW_SOCKET", &self.socket)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("HOME", dir)
            .env("STUB_DIR", dir)
            // Never unset: the daemon would look for the real address and listen on it.
            .env("JW_LAN_ADDR", "off")
            .env("PATH", "/usr/bin:/bin");
        // Without the stub there is no `cloudflared` anywhere.
        if dir.join("stub.sh").exists() {
            cmd.env("JW_CLOUDFLARED", dir.join("stub.sh"));
        }
        cmd
    }

    fn cli(&self, args: &[&str]) -> Cli {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = Arc::new(Mutex::new(String::new()));
        for mut pipe in [
            Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stdout.take().unwrap()),
        ] {
            let out = out.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 512];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        return;
                    }
                    out.lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
        }
        Cli { child, stdin, out }
    }

    /// `jw mobile <args>` to the end: exit status, stdout and stderr.
    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = self.command(args).stdin(Stdio::null()).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn pid(&self, file: &str) -> Option<i32> {
        let path = if file == "daemon" {
            self.socket.with_extension("pid")
        } else {
            self.dir.path().join(file)
        };
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    fn client(&self) -> Client {
        let mut c = Client::connect(&self.socket).unwrap();
        c.set_read_timeout(Some(TIMEOUT)).unwrap();
        c.send(&ClientMsg::Hello { protocol: PROTOCOL }).unwrap();
        c
    }

    fn state(&self) -> MobileState {
        let mut c = self.client();
        c.send(&ClientMsg::MobileWatch).unwrap();
        loop {
            if let DaemonMsg::MobileState(s) = c.recv().unwrap().unwrap() {
                return s;
            }
        }
    }

    fn bridge_port(&self) -> u16 {
        let args = std::fs::read_to_string(self.dir.path().join("args")).unwrap();
        let url = args
            .split_whitespace()
            .find(|a| a.starts_with("http://"))
            .unwrap();
        url.rsplit(':').next().unwrap().parse().unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for file in ["pid", "daemon"] {
            if let Some(pid) = self.pid(file)
                && alive(pid)
            {
                // SAFETY: a pid this test's own daemon or its stub wrote.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        if std::thread::panicking()
            && let Ok(log) = std::fs::read_to_string(self.socket.with_extension("log"))
        {
            eprintln!("--- the daemon's log ---\n{log}");
        }
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

struct Cli {
    child: Child,
    stdin: ChildStdin,
    out: Arc<Mutex<String>>,
}

impl Cli {
    fn text(&self) -> String {
        self.out.lock().unwrap().clone()
    }

    /// Waits until the output holds `needle`; returns the output so far.
    fn wait_for(&self, needle: &str) -> String {
        let until = Instant::now() + TIMEOUT;
        loop {
            let text = self.text();
            if text.contains(needle) {
                return text;
            }
            assert!(Instant::now() < until, "never saw {needle:?} in:\n{text}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn say(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn exit(&mut self) -> Option<std::process::ExitStatus> {
        let until = Instant::now() + TIMEOUT;
        while Instant::now() < until {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The pairing link in the output.
fn link(text: &str) -> String {
    let at = text.find("jw://").expect("a link");
    text[at..]
        .split_whitespace()
        .next()
        .unwrap()
        .trim_end_matches('\x1b')
        .to_string()
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

    fn pair(&mut self, token: &str, name: &str) -> serde_json::Value {
        let v = serde_json::json!({"t": "pair", "token": token, "name": name, "code": "482913"});
        self.0.send(Message::text(v.to_string())).unwrap();
        loop {
            if let Message::Text(t) = self.0.read().unwrap() {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }
}

#[test]
fn it_starts_a_daemon_and_mobile_and_shows_each_step_then_the_link() {
    let env = Env::new(Stub::Registers);
    let cli = env.cli(&[]);
    let text = cli.wait_for("Ctrl-C leaves mobile running");
    for step in ["cloudflared started", "tunnel registered"] {
        assert!(text.contains(step), "{step} missing in:\n{text}");
    }
    let link = link(&text);
    assert!(link.starts_with(&format!("jw://{HOST}/")), "{link}");
    assert_eq!(link.rsplit('/').next().unwrap().len(), 32);
    assert!(text.contains('▀'), "no QR");
    assert!(alive(env.pid("daemon").expect("a daemon was started")));
    assert!(!text.contains("Read-only"));
}

#[test]
fn read_only_is_said_with_the_link() {
    let env = Env::new(Stub::Registers);
    let cli = env.cli(&["--read-only"]);
    let text = cli.wait_for("Read-only: phones can look, not type");
    assert!(text.contains("jw://"));
    assert!(env.state().read_only);
}

#[test]
fn ctrl_c_ends_the_command_and_leaves_mobile_running() {
    let env = Env::new(Stub::Registers);
    let mut cli = env.cli(&[]);
    cli.wait_for("Ctrl-C leaves mobile running");
    let stub = env.pid("pid").unwrap();
    // SAFETY: the `jw mobile` this test started, and only it.
    unsafe { libc::kill(cli.child.id() as i32, libc::SIGINT) };
    assert!(cli.exit().is_some(), "the command did not end");
    assert_eq!(env.state().phase, "ready");
    assert!(alive(stub), "the tunnel went with the command");
}

#[test]
fn the_question_is_answered_at_the_keyboard() {
    let env = Env::new(Stub::Registers);
    let mut cli = env.cli(&[]);
    let token = link(&cli.wait_for("Ctrl-C leaves mobile running"))
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let port = env.bridge_port();

    let mut phone = Phone::connect(port);
    let asked = thread::spawn(move || (phone.pair(&token, "Pixel\u{1b}[31m 8"), phone));
    let text = cli.wait_for("[y/N]");
    assert!(
        text.contains("Pair \"Pixel[31m 8\"? The phone shows 482 913."),
        "{text}"
    );
    assert!(
        !text.contains("Pixel\u{1b}[31m"),
        "an escape reached the terminal"
    );
    cli.say("y");
    let (answer, _phone) = asked.join().unwrap();
    assert_eq!(answer["t"], "paired", "{answer}");
}

#[test]
fn anything_but_yes_refuses_the_phone() {
    let env = Env::new(Stub::Registers);
    let mut cli = env.cli(&[]);
    let token = link(&cli.wait_for("Ctrl-C leaves mobile running"))
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let mut phone = Phone::connect(env.bridge_port());
    let asked = thread::spawn(move || phone.pair(&token, "Pixel"));
    cli.wait_for("[y/N]");
    cli.say("");
    assert_eq!(asked.join().unwrap()["code"], "denied");
}

#[test]
fn enter_asks_for_a_new_code() {
    let env = Env::new(Stub::Registers);
    let mut cli = env.cli(&[]);
    cli.wait_for("Ctrl-C leaves mobile running");
    cli.say("");
    let until = Instant::now() + TIMEOUT;
    let links = loop {
        let text = cli.text();
        let links: Vec<String> = text
            .split_whitespace()
            .filter(|w| w.starts_with("jw://"))
            .map(str::to_string)
            .collect();
        if links.len() >= 2 {
            break links;
        }
        assert!(Instant::now() < until, "no second code in:\n{text}");
        thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(links[0], links[1]);
    // And the first one no longer works.
    let token = links[0].rsplit('/').next().unwrap().to_string();
    let mut phone = Phone::connect(env.bridge_port());
    assert_eq!(phone.pair(&token, "Pixel")["code"], "unknown");
}

#[test]
fn stop_ends_mobile_and_the_command_that_was_showing_it() {
    let env = Env::new(Stub::Registers);
    let mut cli = env.cli(&[]);
    cli.wait_for("Ctrl-C leaves mobile running");
    let stub = env.pid("pid").unwrap();
    let (ok, out, err) = env.run(&["stop"]);
    assert!(ok, "{err}");
    assert!(out.contains("mobile stopped"), "{out}");
    assert!(gone(stub, Duration::from_secs(2)));
    let status = cli.exit().expect("the showing command ended");
    assert!(status.success());
    assert!(cli.text().contains("mobile was stopped"));
}

#[test]
fn stop_with_nothing_running_says_so_and_starts_no_daemon() {
    let env = Env::new(Stub::Registers);
    let (ok, out, err) = env.run(&["stop"]);
    assert!(ok, "{err}");
    assert!(out.contains("mobile is not running"), "{out}");
    assert!(!env.socket.exists(), "a daemon was started to stop nothing");

    // And with a daemon that has mobile off.
    let cli = env.cli(&[]);
    cli.wait_for("Ctrl-C leaves mobile running");
    assert!(env.run(&["stop"]).0);
    let (ok, out, _) = env.run(&["stop"]);
    assert!(ok);
    assert!(out.contains("mobile is not running"), "{out}");
}

#[test]
fn without_cloudflared_it_says_what_to_install_and_exits_1() {
    let env = Env::new(Stub::Missing);
    let (ok, out, err) = env.run(&[]);
    assert!(!ok);
    assert!(err.contains("cloudflared is not installed"), "{err}");
    assert!(!out.contains("jw://") && !err.contains("jw://"));
}

#[test]
fn a_daemon_of_another_protocol_is_refused_before_anything_of_mobile_is_sent() {
    use std::os::unix::net::UnixListener;
    let env = Env::new(Stub::Registers);
    let listener = UnixListener::bind(&env.socket).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { return };
            while let Ok(Some(msg)) = read_frame::<ClientMsg>(&mut c) {
                let hello = matches!(msg, ClientMsg::Hello { .. });
                log.lock().unwrap().push(msg);
                if hello {
                    let _ = write_frame(
                        &mut c,
                        &DaemonMsg::Hello {
                            protocol: PROTOCOL - 1,
                        },
                    );
                }
            }
        }
    });
    let (ok, out, err) = env.run(&[]);
    assert!(!ok);
    assert!(err.contains("jw server stop"), "{err}");
    assert!(err.contains(&format!("protocol {}", PROTOCOL - 1)), "{err}");
    assert!(!out.contains("jw://") && !err.contains("jw://"));
    thread::sleep(Duration::from_millis(200));
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.as_slice(),
        [ClientMsg::Hello { protocol: PROTOCOL }],
        "only Hello may reach an old daemon"
    );
    drop(seen);
    // `stop` is held to the same rule.
    assert!(!env.run(&["stop"]).0);
    assert_eq!(env.pid("daemon"), None, "a daemon was started");
}

#[test]
fn devices_and_revoke_work_with_no_daemon_and_start_none() {
    let env = Env::new(Stub::Registers);
    let config = env.dir.path().join("config").join("jw");
    std::fs::create_dir_all(&config).unwrap();
    let store = jw_web::auth::Store::open(config.join("mobile.json"));
    store.add("Pixel 8", &jw_web::auth::hash("secret")).unwrap();
    store
        .add(
            "Evil\u{1b}]0;owned\u{7}\u{202e}lexiP",
            &jw_web::auth::hash("other"),
        )
        .unwrap();

    let (ok, out, err) = env.run(&["devices"]);
    assert!(ok, "{err}");
    assert!(out.contains("Pixel 8"), "{out}");
    // What a phone named itself does not reach this terminal (REQ-27).
    assert!(out.contains("Evil]0;ownedlexiP"), "{out}");
    assert!(
        !out.contains('\u{1b}') && !out.contains('\u{7}') && !out.contains('\u{202e}'),
        "{out:?}"
    );
    let (ok, out, err) = env.run(&["revoke", "Pixel 8"]);
    assert!(ok, "{err}");
    assert!(out.contains("revoked Pixel 8"), "{out}");
    assert_eq!(store.devices().unwrap().len(), 1);
    assert!(!env.socket.exists(), "a daemon was started");
}

/// What a fake daemon does after it is asked to stop mobile.
#[derive(Clone, Copy)]
enum AfterStop {
    /// Keep saying `ready`, never `off`.
    KeepReady,
    /// Close the connection.
    HangUp,
}

/// A daemon that speaks the protocol, says mobile is ready, and then does
/// `after` when asked to stop it.
fn fake_daemon(env: &Env, after: AfterStop) {
    use std::os::unix::net::UnixListener;
    let listener = UnixListener::bind(&env.socket).unwrap();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { return };
            thread::spawn(move || {
                let ready = || MobileState {
                    phase: "ready".into(),
                    step: String::new(),
                    host: HOST.into(),
                    payload: String::new(),
                    expires_in: 0,
                    read_only: false,
                    devices: vec![],
                    error: String::new(),
                    custom_tunnel: false,
                    lan: String::new(),
                    tunnel: String::new(),
                };
                while let Ok(Some(msg)) = read_frame::<ClientMsg>(&mut c) {
                    match msg {
                        ClientMsg::Hello { .. } => {
                            let _ = write_frame(&mut c, &DaemonMsg::Hello { protocol: PROTOCOL });
                        }
                        ClientMsg::MobileWatch => {
                            let _ = write_frame(&mut c, &DaemonMsg::MobileState(ready()));
                        }
                        ClientMsg::MobileStop => match after {
                            AfterStop::HangUp => return,
                            AfterStop::KeepReady => loop {
                                if write_frame(&mut c, &DaemonMsg::MobileState(ready())).is_err() {
                                    return;
                                }
                                thread::sleep(Duration::from_millis(100));
                            },
                        },
                        _ => {}
                    }
                }
            });
        }
    });
}

#[test]
fn stop_has_one_deadline_for_a_daemon_that_never_says_off() {
    let env = Env::new(Stub::Registers);
    fake_daemon(&env, AfterStop::KeepReady);
    let t = Instant::now();
    let (ok, out, err) = env.run(&["stop"]);
    assert!(!ok, "{out}");
    assert!(err.contains("did not stop"), "{err}");
    // 5 s in all, not 5 s a message (which a chatty daemon would stretch for good).
    assert!(t.elapsed() < Duration::from_secs(9), "{:?}", t.elapsed());
}

#[test]
fn a_daemon_that_hangs_up_is_not_reported_as_slow_to_stop() {
    let env = Env::new(Stub::Registers);
    fake_daemon(&env, AfterStop::HangUp);
    let (ok, _out, err) = env.run(&["stop"]);
    assert!(!ok);
    assert!(err.contains("hung up"), "{err}");
    assert!(!err.contains("did not stop"), "{err}");
}

#[test]
fn a_stand_in_tunnel_program_is_warned_about_where_mobile_is_shown() {
    let env = Env::new(Stub::Registers);
    let cli = env.cli(&[]);
    let text = cli.wait_for("Ctrl-C leaves mobile running");
    assert!(text.contains("warning: JW_CLOUDFLARED is set"), "{text}");
    assert!(env.state().custom_tunnel);
}
