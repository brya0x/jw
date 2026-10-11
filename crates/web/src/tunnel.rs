//! The Cloudflare quick tunnel in front of the bridge (mobile spec, REQ-1, 9).
//!
//! `cloudflared` is a child process. Its tunnel is offered to a phone only
//! once the name resolves in DNS, which happens about a minute after
//! `cloudflared` says it is registered (RISK-3). Every wait takes a `cancel`
//! check, so stopping mobile does not wait out a timeout.
//!
//! `JW_CLOUDFLARED` names another program to run in its place, and then the
//! DNS wait is skipped: that is how tests stand in for the real tunnel.

use std::io::{self, BufRead, BufReader, Read};
use std::net::ToSocketAddrs;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use std::fmt;

/// `cloudflared` says its tunnel is registered well before the name resolves
/// (about a minute, measured), so the QR waits for both.
const REGISTER_WITHIN: Duration = Duration::from_secs(60);
const RESOLVE_WITHIN: Duration = Duration::from_secs(240);
/// How often a wait looks at `cancel`.
const SLICE: Duration = Duration::from_millis(200);

/// Why a tunnel did not come up. Its text is shown to the person at the PC.
#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn fail<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

/// How far the tunnel has got, for whoever is waiting to show it.
#[derive(Debug, Clone)]
pub enum Step {
    /// `cloudflared` is running. The handle lets whoever stops mobile kill it
    /// at once, even while `up` still waits.
    Started(Tunnel),
    /// It registered a tunnel under this name.
    Registered(String),
    /// Waiting for the name to resolve.
    Resolving(String),
}

/// `cloudflared`, killed whenever its last handle goes (REQ-9): on a normal
/// end, on an error, and on a panic. Handles are cloned so that one can kill
/// it while another still waits for it.
#[derive(Debug, Clone)]
pub struct Tunnel(Arc<Mutex<Proc>>);

#[derive(Debug)]
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Tunnel {
    fn new(child: Child) -> Self {
        Self(Arc::new(Mutex::new(Proc(child))))
    }

    /// Ends the process without waiting for it.
    pub fn kill(&self) {
        let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).0.kill();
    }

    /// Whether the process has ended by itself (or was killed).
    pub fn exited(&self) -> bool {
        let mut proc = self.0.lock().unwrap_or_else(|e| e.into_inner());
        matches!(proc.0.try_wait(), Ok(Some(_)))
    }

    pub fn pid(&self) -> u32 {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).0.id()
    }
}

#[derive(Debug)]
enum Tell {
    Host(String),
    Registered,
    Ended(Vec<String>),
}

/// How long each wait lasts; tests make them short.
#[derive(Clone, Copy)]
pub struct Waits {
    pub register: Duration,
    pub resolve: Duration,
    /// Between two looks at DNS.
    pub poll: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            register: REGISTER_WITHIN,
            resolve: RESOLVE_WITHIN,
            poll: Duration::from_secs(2),
        }
    }
}

/// `JW_CLOUDFLARED` as a program, or nothing: an empty or blank value is
/// "not set", so a shell that exports it empty does not skip the DNS wait.
/// It is a seam for tests, and no setting of a user's.
fn program_from(var: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    var.filter(|v| !v.to_string_lossy().trim().is_empty())
}

/// Starts `cloudflared` for `http://127.0.0.1:<port>` and waits until the
/// tunnel can be reached. `on_step` hears each stage; `cancel` is asked
/// during every wait and ends it with an error when it says yes.
pub fn up(
    port: u16,
    waits: Waits,
    cancel: &dyn Fn() -> bool,
    on_step: &mut dyn FnMut(Step),
) -> Result<(Tunnel, String), Error> {
    let custom = program_from(std::env::var_os("JW_CLOUDFLARED"));
    let program = custom.clone().unwrap_or_else(|| "cloudflared".into());
    let mut child = Command::new(&program)
        .args([
            "tunnel",
            "--url",
            &format!("http://127.0.0.1:{port}"),
            "--no-autoupdate",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound if custom.is_none() => {
                Error("cloudflared is not installed; brew install cloudflared".into())
            }
            _ => Error(format!("starting {}: {e}", program.to_string_lossy())),
        })?;
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return fail("cloudflared has no output");
    };
    let told = watch(stderr);
    let tunnel = Tunnel::new(child);
    on_step(Step::Started(tunnel.clone()));

    let host = wait_for_tunnel(&told, waits.register, cancel)?;
    on_step(Step::Registered(host.clone()));
    if custom.is_none() {
        on_step(Step::Resolving(host.clone()));
        wait_to_resolve(&host, waits, cancel)?;
    }
    Ok((tunnel, host))
}

/// Reads `cloudflared`'s output to the end. The pipe must be emptied for as
/// long as it runs, or `cloudflared` blocks on a full one.
fn watch(stderr: impl io::Read + Send + 'static) -> Receiver<Tell> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        let (mut sent_host, mut sent_registered) = (false, false);
        let mut reader = BufReader::new(stderr);
        let mut raw = Vec::new();
        while read_line_capped(&mut reader, &mut raw, LINE_MAX).is_ok_and(|n| n > 0) {
            let line = String::from_utf8_lossy(&raw).trim_end().to_string();
            if !sent_host && let Some(host) = find_host(&line) {
                sent_host = true;
                let _ = tx.send(Tell::Host(host));
            }
            if !sent_registered && line.contains("Registered tunnel connection") {
                sent_registered = true;
                let _ = tx.send(Tell::Registered);
            }
            last.push(line);
            if last.len() > 6 {
                last.remove(0);
            }
        }
        let _ = tx.send(Tell::Ended(last));
    });
    rx
}

/// The longest piece of output looked at as a line; a runaway line without
/// a newline is cut into pieces of this size instead of growing in memory.
const LINE_MAX: usize = 8192;

/// One line, or `cap` bytes of a longer one, into `buf`; how many bytes.
fn read_line_capped(r: &mut impl BufRead, buf: &mut Vec<u8>, cap: usize) -> io::Result<usize> {
    buf.clear();
    r.by_ref().take(cap as u64).read_until(b'\n', buf)
}

/// The quick tunnel's name in a line of `cloudflared`'s output: the first
/// `https://` on it that is one, since a line may carry other links first.
fn find_host(line: &str) -> Option<String> {
    line.match_indices("https://").find_map(|(at, _)| {
        let host: String = line[at + "https://".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
            .collect();
        host.ends_with(".trycloudflare.com").then_some(host)
    })
}

fn wait_for_tunnel(
    told: &Receiver<Tell>,
    within: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<String, Error> {
    let until = Instant::now() + within;
    let (mut host, mut registered) = (None, false);
    while host.is_none() || !registered {
        if cancel() {
            return fail("stopped");
        }
        let left = until.saturating_duration_since(Instant::now());
        match told.recv_timeout(left.min(SLICE)) {
            Ok(Tell::Host(h)) => host = Some(h),
            Ok(Tell::Registered) => registered = true,
            Ok(Tell::Ended(last)) => {
                return fail(format!(
                    "cloudflared stopped before the tunnel was up:\n{}",
                    last.join("\n")
                ));
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() < until => {}
            Err(_) => {
                return fail(format!(
                    "cloudflared did not bring a tunnel up in {within:?}"
                ));
            }
        }
    }
    host.ok_or_else(|| Error("no tunnel name".into()))
}

/// Whether a name resolves; the system's resolver, or a stand-in in tests.
type Resolver = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A tunnel that says it is registered still has no name in DNS for a while
/// (REQ-1), and a phone that scans too soon finds nothing.
fn wait_to_resolve(host: &str, waits: Waits, cancel: &dyn Fn() -> bool) -> Result<(), Error> {
    let system: Resolver = Arc::new(|h| (h, 443).to_socket_addrs().is_ok());
    wait_to_resolve_with(host, waits, cancel, system)
}

/// `getaddrinfo` has no timeout, so each lookup runs on a thread of its own
/// and this waits for it in slices: a hung resolver cannot keep a stop from
/// being heard (REQ-9), and cannot keep this past its deadline. The thread
/// of a lookup that never returns is left behind.
fn wait_to_resolve_with(
    host: &str,
    waits: Waits,
    cancel: &dyn Fn() -> bool,
    resolver: Resolver,
) -> Result<(), Error> {
    let until = Instant::now() + waits.resolve;
    let gave_up = || {
        fail(format!(
            "{host} did not resolve in {:?}; try again",
            waits.resolve
        ))
    };
    loop {
        let (tx, answer) = mpsc::channel();
        let (resolver, name) = (resolver.clone(), host.to_string());
        thread::spawn(move || {
            let _ = tx.send(resolver(&name));
        });
        loop {
            match answer.recv_timeout(SLICE) {
                Ok(true) => return Ok(()),
                Ok(false) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if cancel() {
                return fail("stopped");
            }
            if Instant::now() >= until {
                return gave_up();
            }
        }
        if Instant::now() >= until {
            return gave_up();
        }
        // In slices, so a cancel does not wait out the poll.
        let next = Instant::now() + waits.poll;
        while Instant::now() < next {
            if cancel() {
                return fail("stopped");
            }
            thread::sleep(SLICE.min(waits.poll));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};

    const BOX: &str = "2026-10-11T15:44:22Z INF |  https://guys-slowly-dietary-flags.trycloudflare.com                                       |\n";
    const REGISTERED: &str =
        "2026-10-11T15:44:25Z INF Registered tunnel connection connIndex=0 location=mia01\n";

    fn quick() -> Waits {
        Waits {
            register: Duration::from_millis(600),
            resolve: Duration::from_millis(600),
            poll: Duration::from_millis(100),
        }
    }

    #[test]
    fn the_tunnel_name_is_found_in_the_box_cloudflared_prints() {
        assert_eq!(
            find_host(BOX.trim_end()).as_deref(),
            Some("guys-slowly-dietary-flags.trycloudflare.com")
        );
        assert_eq!(
            find_host("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(find_host("see https://developers.cloudflare.com/x"), None);
    }

    #[test]
    fn the_name_and_the_registered_line_both_come_through() {
        let told = watch(Cursor::new(format!("{BOX}{REGISTERED}")));
        let host = wait_for_tunnel(&told, Duration::from_secs(2), &|| false).unwrap();
        assert_eq!(host, "guys-slowly-dietary-flags.trycloudflare.com");
    }

    #[test]
    fn a_tunnel_that_never_registers_gives_up_and_a_cancel_cuts_the_wait() {
        // The name without the registered line: the wait is not over.
        let (_tx, rx) = mpsc::channel::<Tell>();
        let t = Instant::now();
        let err = wait_for_tunnel(&rx, Duration::from_millis(300), &|| false).unwrap_err();
        assert!(err.0.contains("did not bring a tunnel up"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(2));

        let stop = AtomicBool::new(true);
        let t = Instant::now();
        let err = wait_for_tunnel(&rx, Duration::from_secs(30), &|| {
            stop.load(Ordering::SeqCst)
        })
        .unwrap_err();
        assert_eq!(err.0, "stopped");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn output_that_ends_early_is_reported_with_its_last_lines() {
        let told = watch(Cursor::new("INF starting\nERR no network\n"));
        let err = wait_for_tunnel(&told, Duration::from_secs(2), &|| false).unwrap_err();
        assert!(err.0.contains("ERR no network"), "{err}");
    }

    #[test]
    fn the_dns_wait_gives_up_and_a_cancel_cuts_it() {
        // `.invalid` never resolves (RFC 2606).
        let t = Instant::now();
        let err = wait_to_resolve("jw-test.invalid", quick(), &|| false).unwrap_err();
        assert!(err.0.contains("did not resolve"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(5));

        let t = Instant::now();
        let long = Waits {
            resolve: Duration::from_secs(60),
            ..quick()
        };
        let err = wait_to_resolve("jw-test.invalid", long, &|| true).unwrap_err();
        assert_eq!(err.0, "stopped");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_resolver_that_hangs_cannot_hold_up_a_stop_or_the_deadline() {
        let hang: Resolver = Arc::new(|_| {
            thread::sleep(Duration::from_secs(30));
            true
        });
        // A stop is heard within a slice or two, not after the lookup.
        let t = Instant::now();
        let stop = AtomicBool::new(false);
        let flag = &stop;
        let err = thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(300));
                flag.store(true, Ordering::SeqCst);
            });
            wait_to_resolve_with(
                "jw-test.invalid",
                Waits {
                    resolve: Duration::from_secs(60),
                    ..quick()
                },
                &|| flag.load(Ordering::SeqCst),
                hang.clone(),
            )
        })
        .unwrap_err();
        assert_eq!(err.0, "stopped");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        // And the deadline holds with nobody cancelling.
        let t = Instant::now();
        let err = wait_to_resolve_with("jw-test.invalid", quick(), &|| false, hang).unwrap_err();
        assert!(err.0.contains("did not resolve"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn a_name_that_resolves_on_a_later_try_is_found() {
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = tries.clone();
        let third: Resolver = Arc::new(move |_| n.fetch_add(1, Ordering::SeqCst) >= 2);
        wait_to_resolve_with("a.trycloudflare.com", quick(), &|| false, third).unwrap();
        assert_eq!(tries.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn the_test_seam_ignores_an_empty_value() {
        use std::ffi::OsString;
        assert_eq!(program_from(None), None);
        assert_eq!(program_from(Some(OsString::new())), None);
        assert_eq!(program_from(Some(OsString::from("  \t"))), None);
        assert_eq!(
            program_from(Some(OsString::from("/tmp/stub.sh"))),
            Some(OsString::from("/tmp/stub.sh"))
        );
    }

    #[test]
    fn a_link_that_is_not_the_tunnel_does_not_hide_it() {
        let line =
            "INF see https://developers.cloudflare.com/x and https://a-b.trycloudflare.com |";
        assert_eq!(find_host(line).as_deref(), Some("a-b.trycloudflare.com"));
    }

    #[test]
    fn a_runaway_line_is_read_in_pieces_and_a_later_host_is_still_found() {
        let mut out = "x".repeat(100_000);
        out.push('\n');
        out.push_str(BOX);
        out.push_str(REGISTERED);
        let mut r = BufReader::new(Cursor::new(out.clone()));
        let mut buf = Vec::new();
        assert_eq!(read_line_capped(&mut r, &mut buf, 1000).unwrap(), 1000);
        let told = watch(Cursor::new(out));
        let host = wait_for_tunnel(&told, Duration::from_secs(3), &|| false).unwrap();
        assert_eq!(host, "guys-slowly-dietary-flags.trycloudflare.com");
    }

    #[test]
    fn the_child_dies_with_its_handle() {
        let child = Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id();
        let tunnel = Tunnel::new(child);
        assert!(alive(pid));
        drop(tunnel);
        assert!(!alive(pid));
    }

    #[test]
    fn kill_ends_the_process_and_exited_says_so() {
        let tunnel = Tunnel::new(Command::new("sleep").arg("600").spawn().unwrap());
        assert!(!tunnel.exited());
        tunnel.kill();
        let until = Instant::now() + Duration::from_secs(2);
        while !tunnel.exited() && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(tunnel.exited());
    }

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}
