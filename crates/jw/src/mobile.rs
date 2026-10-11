//! `jw mobile`: a client of the daemon's mobile service (docs/specs/mobile.md,
//! REQ-30, 41). The daemon hosts the bridge and the tunnel and keeps them up
//! after this exits; this shows each step, the QR and the link, and answers
//! "pair this phone?" at the keyboard.
//!
//! Only `jw mobile` itself starts a daemon. `stop` talks to one that is
//! there or says mobile is not running, and `devices` and `revoke` read and
//! write the store, so they work with mobile off and with no daemon at all.
//!
//! A failed mobile prints why and exits 1. Mobile stopped from elsewhere (the
//! TUI, `jw mobile stop`) exits 0, and the daemon hanging up exits 1. Ctrl-C
//! has no handler: the default ends only this process, and mobile stays up
//! because the daemon is in a session of its own.

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use jw_core::ago::ago;
use jw_core::qr::Qr;
use jw_web::auth::Store;

use crate::client::Client;
use crate::core;
use crate::proto::{self, ClientMsg, DaemonMsg, MobileState, PROTOCOL, clean as clean_text};

/// The longest name or code of a phone shown here.
const NAME_MAX: usize = 40;

/// What a phone chose, cleaned before it reaches this terminal (REQ-27).
fn clean(s: &str) -> String {
    clean_text(s, NAME_MAX)
}

const USAGE: &str = "usage: jw mobile [--read-only] | stop | devices | revoke <device>";

/// How long `stop` waits to see mobile go off.
const STOP_WITHIN: Duration = Duration::from_secs(5);

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [a] if a == "devices" => devices(),
        [a, who] if a == "revoke" => revoke(who),
        [a] if a == "stop" => stop(),
        _ => show(parse(args)?),
    }
}

struct Opts {
    read_only: bool,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut opts = Opts { read_only: false };
    for arg in args {
        match arg.as_str() {
            "--read-only" => opts.read_only = true,
            _ => bail!(USAGE),
        }
    }
    Ok(opts)
}

/// Asks the daemon its protocol before anything of mobile is sent to it: a
/// daemon of another build does not understand `Mobile*`, and would hang up
/// on them (REQ-30).
fn hello(c: &mut Client) -> Result<()> {
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    c.send(&ClientMsg::Hello { protocol: PROTOCOL })?;
    loop {
        match c.recv().context("the daemon did not answer")? {
            Some(DaemonMsg::Hello { protocol }) if protocol == PROTOCOL => break,
            Some(DaemonMsg::Hello { protocol }) => bail!(
                "the daemon runs protocol {protocol} and this jw {PROTOCOL}: jw server stop, then open jw again"
            ),
            Some(_) => {}
            None => bail!("the daemon hung up"),
        }
    }
    c.set_read_timeout(None)?;
    Ok(())
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
    let names: Vec<String> = devices.iter().map(|d| clean(&d.name)).collect();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    println!(
        "{:<8}  {:<width$}  {:<12}  last seen",
        "id", "name", "paired"
    );
    for (d, name) in devices.iter().zip(&names) {
        println!(
            "{:<8}  {:<width$}  {:<12}  {}",
            clean(&d.id),
            name,
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
                clean(&d.name),
                clean(&d.id)
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

/// `jw mobile stop`: asks a daemon that is there, and never starts one.
fn stop() -> Result<()> {
    let Ok(mut c) = Client::connect(&proto::socket_path()) else {
        println!("mobile is not running");
        return Ok(());
    };
    hello(&mut c)?;
    c.send(&ClientMsg::MobileWatch)?;
    if state(&mut c)?.phase == "off" {
        println!("mobile is not running");
        return Ok(());
    }
    c.send(&ClientMsg::MobileStop)?;
    // One deadline for the whole wait, not one per read: a daemon that keeps
    // saying something else cannot keep this going.
    let until = Instant::now() + STOP_WITHIN;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("mobile did not stop within {STOP_WITHIN:?}; the daemon may be stuck");
        }
        c.set_read_timeout(Some(left))?;
        let phase = state(&mut c).map_err(|e| {
            // Only a read that ran out of time is "did not stop": a hung-up
            // daemon or a real I/O error says what it was.
            let timed_out = e.downcast_ref::<io::Error>().is_some_and(|io| {
                matches!(
                    io.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                )
            });
            if timed_out {
                anyhow::anyhow!(
                    "mobile did not stop within {STOP_WITHIN:?}; the daemon may be stuck"
                )
            } else {
                e
            }
        })?;
        if phase.phase == "off" {
            println!("mobile stopped; phones were cut off");
            return Ok(());
        }
    }
}

/// The next `MobileState`.
fn state(c: &mut Client) -> Result<MobileState> {
    loop {
        match c.recv().context("the daemon did not answer")? {
            Some(DaemonMsg::MobileState(s)) => return Ok(s),
            Some(DaemonMsg::Error { msg }) => bail!("{msg}"),
            Some(_) => {}
            None => bail!("the daemon hung up"),
        }
    }
}

enum Event {
    Msg(DaemonMsg),
    Closed,
}

/// What the daemon says, on a channel, so the loop can also watch the keyboard.
fn events(mut c: Client) -> Receiver<Event> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let event = match c.recv() {
                Ok(Some(m)) => Event::Msg(m),
                _ => Event::Closed,
            };
            let closed = matches!(event, Event::Closed);
            if tx.send(event).is_err() || closed {
                return;
            }
        }
    });
    rx
}

/// One thread owns stdin for the whole run and passes its lines on. At the
/// end of input it just stops: no line, no exit.
fn lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });
    rx
}

/// `jw mobile`: start mobile if it is off, show it, and answer pair
/// questions until it stops, fails, or Ctrl-C ends this.
fn show(opts: Opts) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut c =
        Client::connect_or_start(&proto::socket_path(), &exe).context("starting the jw daemon")?;
    hello(&mut c)?;
    c.send(&ClientMsg::MobileWatch)?;
    let mut out = c.try_clone()?;
    let events = events(c);
    let input = lines();
    let mut view = View::default();
    let mut first = true;
    loop {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Msg(DaemonMsg::MobileState(s))) => {
                if first {
                    first = false;
                    if matches!(s.phase.as_str(), "off" | "failed") {
                        // What it said was before this start: not news.
                        out.send(&ClientMsg::MobileStart {
                            read_only: opts.read_only,
                        })?;
                        continue;
                    }
                    if opts.read_only && !s.read_only {
                        eprintln!("  mobile was already running: read-only is now on");
                        out.send(&ClientMsg::MobileReadOnly { on: true })?;
                    }
                }
                if view.state(&s)? == Flow::Done {
                    return Ok(());
                }
            }
            Ok(Event::Msg(DaemonMsg::MobilePair {
                request,
                name,
                code,
            })) => {
                // A key pressed before the question must not answer it.
                while input.try_recv().is_ok() {}
                view.ask(request, &name, &code);
            }
            Ok(Event::Msg(DaemonMsg::MobilePairDone { request })) => view.done(request),
            Ok(Event::Msg(DaemonMsg::Error { msg })) => eprintln!("  the daemon says: {msg}"),
            Ok(Event::Msg(_)) => {}
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                bail!("the daemon hung up")
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        while let Ok(line) = input.try_recv() {
            out.send(&view.line(&line))?;
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
enum Flow {
    Go,
    Done,
}

/// What `jw mobile` has put on the screen, so each thing is said once.
#[derive(Default)]
struct View {
    /// The step last printed.
    step: String,
    /// Mobile has been seen running, so an `off` now means it was stopped.
    began: bool,
    /// The code on show.
    code: String,
    /// The question waiting for a y or an n.
    pending: Option<u64>,
    /// The phones connected, by id.
    online: BTreeMap<String, String>,
    /// The stand-in tunnel warning was printed.
    warned: bool,
}

impl View {
    fn state(&mut self, s: &MobileState) -> Result<Flow> {
        if s.custom_tunnel && !self.warned {
            // A stand-in program is never active unseen.
            self.warned = true;
            eprintln!(
                "  warning: JW_CLOUDFLARED is set in the daemon: the tunnel is that program, not Cloudflare"
            );
        }
        match s.phase.as_str() {
            "starting" => {
                self.began = true;
                if s.step != self.step {
                    eprintln!("  {}", step_text(&s.step));
                    self.step = s.step.clone();
                }
            }
            "ready" => {
                self.began = true;
                self.phones(s);
                if s.payload.is_empty() {
                    if !self.code.is_empty() {
                        eprintln!("\n  The code was used or ran out. Enter makes a new one.");
                        self.code.clear();
                    }
                } else if s.payload != self.code {
                    self.code = s.payload.clone();
                    show_code(s)?;
                }
            }
            "failed" => bail!("{}", clean(&s.error)),
            _ if self.began => {
                eprintln!("\n  mobile was stopped; phones were cut off");
                return Ok(Flow::Done);
            }
            _ => {}
        }
        Ok(Flow::Go)
    }

    /// Says which phones came and went.
    fn phones(&mut self, s: &MobileState) {
        let now: BTreeMap<String, String> = s
            .devices
            .iter()
            .filter(|d| d.connected)
            .map(|d| (d.id.clone(), clean(&d.name)))
            .collect();
        for (id, name) in &now {
            if !self.online.contains_key(id) {
                eprintln!("  {name} connected");
            }
        }
        for (id, name) in &self.online {
            if !now.contains_key(id) {
                eprintln!("  {name} disconnected");
            }
        }
        self.online = now;
    }

    /// Puts the question; the next line answers it.
    fn ask(&mut self, request: u64, name: &str, code: &str) {
        self.pending = Some(request);
        eprint!(
            "\n  Pair {:?}? The phone shows {}. [y/N] ",
            clean(name),
            group(&clean(code))
        );
    }

    /// The question is over: someone else answered, or time ran out.
    fn done(&mut self, request: u64) {
        if self.pending == Some(request) {
            self.pending = None;
            eprintln!("\n  (answered somewhere else, or it ran out of time)");
        }
    }

    /// What a line typed means: the answer to a question waiting, else a new
    /// code.
    fn line(&mut self, line: &str) -> ClientMsg {
        match self.pending.take() {
            Some(request) => ClientMsg::MobileAnswer {
                request,
                yes: matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
            },
            None => ClientMsg::MobileCode,
        }
    }
}

/// What the daemon's step keys say, in words.
fn step_text(step: &str) -> &'static str {
    match step {
        "launching" => "starting cloudflared",
        "started" => "cloudflared started",
        "registered" => "tunnel registered",
        "resolving" => "waiting for the name to resolve (about a minute)",
        _ => "starting",
    }
}

/// `482913` as `482 913`, which is how the phone shows it.
fn group(code: &str) -> String {
    code.as_bytes()
        .chunks(3)
        .map(|c| String::from_utf8_lossy(c))
        .collect::<Vec<_>>()
        .join(" ")
}

fn show_code(s: &MobileState) -> Result<()> {
    eprintln!("\n{}", ansi(&Qr::new(&s.payload)?, true_colour()));
    eprintln!("  Scan it with the jw app. It works once, for two minutes.");
    eprintln!("  {}", s.payload);
    if s.read_only {
        eprintln!("  Read-only: phones can look, not type.");
    }
    eprintln!("  Enter makes a new code. jw mobile devices | revoke <id> | stop.");
    eprintln!("  Ctrl-C leaves mobile running.");
    Ok(())
}

/// Whether the terminal says it does 24-bit colour (`COLORTERM`).
fn true_colour() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| matches!(v.as_str(), "truecolor" | "24bit"))
}

/// A QR drawn black on white with half blocks, two modules to a character
/// cell. The colours are true colour, not palette entries, which a terminal's
/// theme would remap (One Dark's white is grey).
fn ansi(qr: &Qr, truecolor: bool) -> String {
    // Without 24-bit colour, the two ends of the 256-colour cube: 16 is
    // black and 231 is white in every terminal that has the cube.
    let paint = |top: bool, bottom: bool| {
        if truecolor {
            let rgb = |dark: bool| if dark { "0;0;0" } else { "255;255;255" };
            format!("\x1b[38;2;{};48;2;{}m▀", rgb(top), rgb(bottom))
        } else {
            let c = |dark: bool| if dark { 16 } else { 231 };
            format!("\x1b[38;5;{};48;5;{}m▀", c(top), c(bottom))
        }
    };
    let mut out = String::new();
    for row in qr.cells() {
        for (top, bottom) in row {
            out.push_str(&paint(top, bottom));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags() {
        assert!(!parse(&args(&[])).unwrap().read_only);
        assert!(parse(&args(&["--read-only"])).unwrap().read_only);
        // The daemon picks the port now.
        assert!(parse(&args(&["--port", "8123"])).is_err());
        assert!(parse(&args(&["--nope"])).is_err());
    }

    #[test]
    fn a_code_is_grouped_in_threes() {
        assert_eq!(group("482913"), "482 913");
        assert_eq!(group("1234"), "123 4");
    }

    #[test]
    fn what_a_phone_sent_is_printed_without_control_characters() {
        assert_eq!(clean("Pixel\x1b[31m 8\x07\n"), "Pixel[31m 8");
        assert_eq!(clean("\u{202e}lexiP"), "lexiP");
    }

    #[test]
    fn a_line_answers_the_question_waiting_else_asks_for_a_new_code() {
        let mut v = View::default();
        assert_eq!(v.line(""), ClientMsg::MobileCode);
        v.ask(7, "Pixel", "482913");
        assert_eq!(
            v.line("Y"),
            ClientMsg::MobileAnswer {
                request: 7,
                yes: true
            }
        );
        // Answered: the next Enter is a new code again.
        assert_eq!(v.line(""), ClientMsg::MobileCode);
        v.ask(8, "Pixel", "482913");
        assert_eq!(
            v.line(""),
            ClientMsg::MobileAnswer {
                request: 8,
                yes: false
            }
        );
        v.ask(9, "Pixel", "482913");
        v.done(9);
        assert_eq!(v.line("y"), ClientMsg::MobileCode);
    }

    #[test]
    fn the_qr_is_drawn_two_modules_to_a_cell_in_true_colour() {
        let qr = Qr::new("jw://a.trycloudflare.com/00112233445566778899aabbccddeeff").unwrap();
        let text = ansi(&qr, true);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), qr.size().div_ceil(2));
        assert!(rows.iter().all(|r| r.matches('▀').count() == qr.size()));
        assert!(rows[0].contains("\x1b[38;2;255;255;255;48;2;255;255;255m"));
        assert!(!rows[0].contains("38;2;0;0;0"));
        // A terminal that does not say it has true colour gets the cube's
        // black and white.
        let plain = ansi(&qr, false);
        assert!(!plain.contains("38;2;") && plain.contains("\x1b[38;5;231;48;5;231m"));
        assert!(plain.contains("\x1b[38;5;16;48;5;"));
    }

    #[test]
    fn the_steps_have_words() {
        for step in ["launching", "started", "registered", "resolving"] {
            assert_ne!(step_text(step), "starting", "{step}");
        }
    }
}
