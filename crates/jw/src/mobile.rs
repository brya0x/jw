//! `jw mobile`: the phone's way in (docs/specs/mobile.md, REQ-1, 3, 6, 9, 29, 30).
//!
//! It runs the web server on a loopback port, puts a Cloudflare quick tunnel
//! in front of it, and prints a QR the phone app scans. Nothing here talks
//! to the daemon: that is `jw-web`'s job, and only after a phone is let in.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use jw_web::auth::Store;
use jw_web::bridge::{Bridge, Config, Confirm, Timings};
use qrcode::{Color, QrCode};

use crate::client::Client;
use crate::core;
use crate::proto::{self, ClientMsg, DaemonMsg, PROTOCOL};

const USAGE: &str =
    "usage: jw mobile [--read-only] [--port <n>] | jw mobile devices | jw mobile revoke <device>";

/// `cloudflared` says its tunnel is registered well before the name resolves
/// (about a minute, measured), so the QR waits for both.
const REGISTER_WITHIN: Duration = Duration::from_secs(60);
const RESOLVE_WITHIN: Duration = Duration::from_secs(240);
/// How long the person at the PC has to answer "pair this phone?".
const ASK_WITHIN: Duration = Duration::from_secs(60);

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [a] if a == "devices" => devices(),
        [a, who] if a == "revoke" => revoke(who),
        _ => serve(parse(args)?),
    }
}

struct Opts {
    read_only: bool,
    /// 0 is any free port, so a tunnel left over from an earlier run points
    /// at nothing.
    port: u16,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut opts = Opts {
        read_only: false,
        port: 0,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--read-only" => opts.read_only = true,
            "--port" => {
                opts.port = args.next().and_then(|p| p.parse().ok()).context(USAGE)?;
            }
            _ => bail!(USAGE),
        }
    }
    Ok(opts)
}

/// Without a daemon every phone would only be told there is none (REQ-30),
/// and a daemon of another build would be found out after the wait for the
/// tunnel and a pairing, so both are checked first.
fn check_daemon() -> Result<()> {
    let mut c = Client::connect(&proto::socket_path())
        .context("no jw daemon is running; open jw first, then run jw mobile")?;
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    c.send(&ClientMsg::Hello { protocol: PROTOCOL })?;
    loop {
        match c.recv().context("the daemon did not answer")? {
            Some(DaemonMsg::Hello { protocol }) if protocol == PROTOCOL => return Ok(()),
            Some(DaemonMsg::Hello { protocol }) => bail!(
                "the daemon runs protocol {protocol} and this jw {PROTOCOL}: jw server stop, then open jw again"
            ),
            Some(_) => {}
            None => bail!("the daemon hung up"),
        }
    }
}

fn store() -> Result<Store> {
    Ok(Store::open(core::config::dir()?.join("mobile.json")))
}

fn devices() -> Result<()> {
    let devices = store()?.devices()?;
    if devices.is_empty() {
        println!("no phone is paired; jw mobile prints the code to pair one");
        return Ok(());
    }
    let now = now();
    let width = devices
        .iter()
        .map(|d| d.name.chars().count())
        .max()
        .unwrap_or(0);
    println!(
        "{:<8}  {:<width$}  {:<12}  last seen",
        "id", "name", "paired"
    );
    for d in devices {
        println!(
            "{:<8}  {:<width$}  {:<12}  {}",
            d.id,
            d.name,
            ago(now, d.created),
            ago(now, d.last_seen),
        );
    }
    Ok(())
}

fn revoke(who: &str) -> Result<()> {
    match store()?.revoke(who)? {
        Some(d) => {
            println!(
                "revoked {} ({}); it is cut off within seconds",
                d.name, d.id
            );
            Ok(())
        }
        None => bail!("no device {who:?}; jw mobile devices lists them"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

/// Set by SIGINT and SIGTERM; the loop below notices and shuts down.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// `cloudflared`, killed whenever this value goes (REQ-9): on a normal end,
/// on an error, and on a panic.
struct Tunnel(Child);

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

enum Tell {
    Host(String),
    Registered,
    Ended(Vec<String>),
}

fn serve(opts: Opts) -> Result<()> {
    check_daemon()?;
    let store = Arc::new(store()?);

    let listener = TcpListener::bind(("127.0.0.1", opts.port)).context("binding a local port")?;
    let port = listener.local_addr()?.port();

    let input = lines();
    let bridge = Bridge::start(
        listener,
        proto::socket_path(),
        Config {
            store,
            read_only: opts.read_only,
            confirm: ask(input.clone()),
            timings: Timings::default(),
        },
    )?;

    // SAFETY: the handler only stores into an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }

    let mut child = Command::new("cloudflared")
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
            io::ErrorKind::NotFound => {
                anyhow::anyhow!("cloudflared is not installed; brew install cloudflared")
            }
            _ => anyhow::Error::new(e).context("starting cloudflared"),
        })?;
    let told = watch(child.stderr.take().context("cloudflared has no output")?);
    let _tunnel = Tunnel(child);

    let host = wait_for_tunnel(&told)?;
    eprintln!("tunnel up at {host}");
    wait_to_resolve(&host)?;

    show_code(&bridge, &host, opts.read_only)?;
    while !STOP.load(Ordering::SeqCst) {
        if matches!(told.try_recv(), Ok(Tell::Ended(_))) {
            bail!("cloudflared stopped; run jw mobile again");
        }
        // Enter asks for a new code; while a phone is being asked, the
        // prompt holds the lines and this waits its turn.
        if let Ok(rx) = input.try_lock()
            && rx.try_recv().is_ok()
        {
            drop(rx);
            show_code(&bridge, &host, opts.read_only)?;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("\nstopping");
    drop(bridge);
    Ok(())
}

/// One thread owns stdin for the whole run and passes its lines on, so a
/// question never waits on a read that nobody can cancel.
fn lines() -> Arc<Mutex<Receiver<String>>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });
    Arc::new(Mutex::new(rx))
}

/// Asks at the PC whether to pair, showing the code the phone shows, so
/// whoever copied the QR cannot pair as someone else (REQ-3).
fn ask(input: Arc<Mutex<Receiver<String>>>) -> Confirm {
    Arc::new(move |p| {
        let rx = input.lock().unwrap();
        while rx.try_recv().is_ok() {}
        eprint!(
            "\n  Pair {:?}? The phone shows {}. [y/N] ",
            p.name,
            group(p.code)
        );
        let _ = io::stderr().flush();
        // In slices, so Ctrl-C does not wait out the whole minute.
        let until = Instant::now() + ASK_WITHIN;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => return matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
                Err(RecvTimeoutError::Timeout)
                    if Instant::now() < until && !STOP.load(Ordering::SeqCst) => {}
                Err(_) => return false,
            }
        }
    })
}

/// `482913` as `482 913`, which is how the phone shows it.
fn group(code: &str) -> String {
    code.as_bytes()
        .chunks(3)
        .map(|c| String::from_utf8_lossy(c))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads `cloudflared`'s output to the end. The pipe must be emptied for as
/// long as it runs, or `cloudflared` blocks on a full one.
fn watch(stderr: impl io::Read + Send + 'static) -> Receiver<Tell> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        let (mut sent_host, mut sent_registered) = (false, false);
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
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

/// The quick tunnel's name in a line of `cloudflared`'s output.
fn find_host(line: &str) -> Option<String> {
    let at = line.find("https://")? + "https://".len();
    let host: String = line[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
        .collect();
    host.ends_with(".trycloudflare.com").then_some(host)
}

fn wait_for_tunnel(told: &Receiver<Tell>) -> Result<String> {
    let until = Instant::now() + REGISTER_WITHIN;
    let (mut host, mut registered) = (None, false);
    while host.is_none() || !registered {
        if STOP.load(Ordering::SeqCst) {
            bail!("stopped");
        }
        let left = until.saturating_duration_since(Instant::now());
        match told.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(Tell::Host(h)) => host = Some(h),
            Ok(Tell::Registered) => registered = true,
            Ok(Tell::Ended(last)) => {
                bail!(
                    "cloudflared stopped before the tunnel was up:\n{}",
                    last.join("\n")
                )
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() < until => {}
            Err(_) => bail!("cloudflared did not bring a tunnel up in {REGISTER_WITHIN:?}"),
        }
    }
    host.context("no tunnel name")
}

/// A tunnel that says it is registered still has no name in DNS for a while
/// (REQ-1), and a phone that scans too soon finds nothing.
fn wait_to_resolve(host: &str) -> Result<()> {
    let until = Instant::now() + RESOLVE_WITHIN;
    eprint!("waiting for {host} to be reachable");
    while (host, 443).to_socket_addrs().is_err() {
        if STOP.load(Ordering::SeqCst) {
            bail!("stopped");
        }
        if Instant::now() >= until {
            bail!("{host} did not resolve in {RESOLVE_WITHIN:?}; run jw mobile again");
        }
        eprint!(".");
        let _ = io::stderr().flush();
        thread::sleep(Duration::from_secs(2));
    }
    eprintln!(" ready");
    Ok(())
}

fn show_code(bridge: &Bridge, host: &str, read_only: bool) -> Result<()> {
    let token = bridge.new_pairing()?;
    let payload = format!("jw://{host}/{token}");
    eprintln!("\n{}", qr(&payload)?);
    eprintln!("  Scan it with the jw app. It works once, for two minutes.");
    eprintln!("  {payload}");
    if read_only {
        eprintln!("  Read-only: phones can look, not type.");
    }
    eprintln!("  Enter makes a new code. jw mobile devices, jw mobile revoke <id>. Ctrl-C stops.");
    Ok(())
}

/// A QR drawn black on white with half blocks, two modules to a character
/// cell. The colours are true colour, not palette entries, which a terminal's
/// theme would remap (One Dark's white is grey).
fn qr(payload: &str) -> Result<String> {
    let code = QrCode::new(payload.as_bytes()).context("making the QR")?;
    let (w, quiet) = (code.width() as i64, 2);
    let modules = code.to_colors();
    let dark = |x: i64, y: i64| {
        (quiet..w + quiet).contains(&x)
            && (quiet..w + quiet).contains(&y)
            && modules[((y - quiet) * w + (x - quiet)) as usize] == Color::Dark
    };
    let size = w + 2 * quiet;
    let mut out = String::new();
    for y in (0..size).step_by(2) {
        for x in 0..size {
            let (top, bottom) = (dark(x, y), dark(x, y + 1));
            let rgb = |dark: bool| if dark { "0;0;0" } else { "255;255;255" };
            out.push_str(&format!("\x1b[38;2;{};48;2;{}m▀", rgb(top), rgb(bottom)));
        }
        out.push_str("\x1b[0m\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags() {
        let o = parse(&args(&[])).unwrap();
        assert!(!o.read_only);
        assert_eq!(o.port, 0);
        let o = parse(&args(&["--read-only", "--port", "8123"])).unwrap();
        assert!(o.read_only);
        assert_eq!(o.port, 8123);
        assert!(parse(&args(&["--port"])).is_err());
        assert!(parse(&args(&["--port", "x"])).is_err());
        assert!(parse(&args(&["--nope"])).is_err());
    }

    #[test]
    fn the_tunnel_name_is_found_in_the_box_cloudflared_prints() {
        let line = "2026-10-11T15:44:22Z INF |  https://guys-slowly-dietary-flags.trycloudflare.com                                       |";
        assert_eq!(
            find_host(line).as_deref(),
            Some("guys-slowly-dietary-flags.trycloudflare.com")
        );
        assert_eq!(
            find_host("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(find_host("see https://developers.cloudflare.com/x"), None);
    }

    #[test]
    fn a_code_is_grouped_in_threes() {
        assert_eq!(group("482913"), "482 913");
        assert_eq!(group("1234"), "123 4");
    }

    #[test]
    fn times_read_as_ages() {
        assert_eq!(ago(1000, 1000), "just now");
        assert_eq!(ago(1000, 1000 - 125), "2 min ago");
        assert_eq!(ago(100_000, 100_000 - 7300), "2 h ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 200_000), "2 d ago");
    }

    #[test]
    fn the_qr_is_a_square_of_half_blocks_with_a_quiet_zone() {
        let text =
            qr("jw://guys-slowly-dietary-flags.trycloudflare.com/00112233445566778899aabbccddeeff")
                .unwrap();
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows.len() >= 20, "{} rows", rows.len());
        let cols = rows[0].matches('▀').count();
        assert!(rows.iter().all(|r| r.matches('▀').count() == cols));
        assert!(
            rows.len() * 2 >= cols && rows.len() * 2 <= cols + 2,
            "square"
        );
        // The top rows are the quiet zone: white on white.
        assert!(rows[0].contains("\x1b[38;2;255;255;255;48;2;255;255;255m"));
        assert!(!rows[0].contains("38;2;0;0;0"));
    }
}
