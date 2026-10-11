//! Mobile in the TUI (docs/specs/mobile.md, REQ-42 to 49): the Mobile modal
//! (`^␣ m`), the question "pair this phone?" that opens over the screen, and
//! the indicator in the status bar. The service is the daemon's; this is a
//! client of it, and `jw mobile` is another.
//!
//! The latest `MobileState` lives in `App::mobile`, once, and every view is
//! drawn from it. A countdown (the code's time left, the question's minute)
//! runs from when the message arrived, so the daemon need not send a state a
//! second. The look follows `docs/specs/mobile-modal-prototype.html`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::App;
use super::modal::{Modal, boxed, key_line};
use crate::ago::ago;
use crate::proto::{ClientMsg, DaemonMsg, MobileDevice, MobileState};
use crate::qr::Qr;
use crate::theme::p;

/// How long the person has to answer a phone, as the daemon counts it
/// (REQ-36); only the countdown shown here is ours.
const ASK_FOR: Duration = Duration::from_secs(60);
/// A pair question that jumps in over the screen swallows keys for this long:
/// the person may be typing into a pane, and a stray `y` must not pair a
/// phone nobody compared the code of (RAT-8).
const ASK_GUARD: Duration = Duration::from_millis(500);
/// The code's life, for the bar (REQ-2).
const CODE_FOR: u64 = 120;
/// The least the window is for the QR, whatever its size (REQ-47); the usual
/// link, version 5 with a four-module quiet zone (45 modules), needs 70×30.
const NEED: (u16, u16) = (66, 28);
/// The width of the column of words beside the QR.
const INFO: usize = 20;

/// What the TUI knows of mobile, from `MobileWatch` on (REQ-42).
#[derive(Default)]
pub struct Mobile {
    /// The latest state and when it came.
    state: Option<(MobileState, Instant)>,
    /// Questions waiting for a moment to be asked (REQ-45), oldest first. The
    /// one on screen is in its modal, not here.
    held: VecDeque<Ask>,
}

/// A phone asking to pair.
pub struct Ask {
    pub request: u64,
    name: String,
    code: String,
    /// When `MobilePair` arrived: the daemon's minute runs from here.
    arrived: Instant,
    /// When it went on screen: the guard against a key typed ahead runs from
    /// here.
    shown: Instant,
}

/// What only the Mobile modal remembers: whether it shows the paired
/// phones, which one is selected and which one a revoke asks about.
#[derive(Default)]
pub struct View {
    devices: bool,
    sel: usize,
    revoke: Option<usize>,
}

impl View {
    /// A revoke waits for its y or n: the bar says CONFIRM (REQ-45).
    pub fn asking(&self) -> bool {
        self.devices && self.revoke.is_some()
    }
}

impl Mobile {
    pub fn state(&self) -> Option<&MobileState> {
        self.state.as_ref().map(|(s, _)| s)
    }

    fn phase(&self) -> &str {
        self.state().map_or("off", |s| s.phase.as_str())
    }

    /// Seconds the code has left, counted from when the state came.
    fn left(&self) -> u64 {
        self.state.as_ref().map_or(0, |(s, at)| {
            s.expires_in.saturating_sub(at.elapsed().as_secs())
        })
    }

    /// Whether a question waits for its moment.
    pub fn waiting(&self) -> bool {
        !self.held.is_empty()
    }
}

/// Strips what could move a terminal; a phone chose these words (REQ-27).
fn clean(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

impl App {
    /// `^␣ m` (REQ-43).
    pub(super) fn open_mobile(&mut self) {
        self.modal = Some(Modal::Mobile(View::default()));
    }

    /// What the daemon says about mobile (REQ-42, 45).
    pub(super) fn on_mobile(&mut self, d: DaemonMsg) {
        match d {
            DaemonMsg::MobileState(s) => self.mobile.state = Some((s, Instant::now())),
            DaemonMsg::MobilePair {
                request,
                name,
                code,
            } => {
                let on_screen =
                    matches!(&self.modal, Some(Modal::Pair(a, _)) if a.request == request);
                if !on_screen && !self.mobile.held.iter().any(|a| a.request == request) {
                    self.mobile.held.push_back(Ask {
                        request,
                        name: clean(&name, 40),
                        code: clean(&code, 12),
                        arrived: Instant::now(),
                        shown: Instant::now(),
                    });
                }
            }
            DaemonMsg::MobilePairDone { request } => {
                self.mobile.held.retain(|a| a.request != request);
                if matches!(&self.modal, Some(Modal::Pair(a, _)) if a.request == request)
                    && let Some(Modal::Pair(_, back)) = self.modal.take()
                {
                    self.modal = back.map(Modal::Mobile);
                }
            }
            _ => {}
        }
        self.release_asks();
    }

    /// Opens the next question when nothing else has the keys (REQ-45): not
    /// over a picker, the settings, another modal, a half-typed leader or a
    /// selection. Over the Mobile modal it does, and gives it back after.
    pub(super) fn release_asks(&mut self) {
        if self.mobile.held.is_empty() {
            return;
        }
        let free = matches!(self.modal, None | Some(Modal::Mobile(_)))
            && self.finder.is_none()
            && self.settings.is_none()
            && self.welcome.is_none()
            && !self.leading()
            && !self.walking
            && !self.selecting;
        if !free {
            return;
        }
        let Some(mut ask) = self.mobile.held.pop_front() else {
            return;
        };
        ask.shown = Instant::now();
        let back = match self.modal.take() {
            Some(Modal::Mobile(v)) => Some(v),
            _ => None,
        };
        self.modal = Some(Modal::Pair(Box::new(ask), back));
    }

    /// A key while the Mobile modal or a pair question is open.
    pub(super) fn mobile_key(&mut self, k: KeyEvent) {
        match self.modal.take() {
            Some(Modal::Pair(ask, back)) => self.pair_key(k, ask, back),
            Some(Modal::Mobile(v)) => self.view_key(k, v),
            other => self.modal = other,
        }
        self.release_asks();
    }

    /// Only `y` pairs the phone; `n` and `esc` refuse it. Enter does neither,
    /// so a key typed ahead cannot answer.
    fn pair_key(&mut self, k: KeyEvent, ask: Box<Ask>, back: Option<View>) {
        let yes = match k.code {
            _ if ask.shown.elapsed() < ASK_GUARD => None,
            KeyCode::Char('y') => Some(true),
            KeyCode::Char('n') | KeyCode::Esc => Some(false),
            _ => None,
        };
        let Some(yes) = yes else {
            self.modal = Some(Modal::Pair(ask, back));
            return;
        };
        self.send(ClientMsg::MobileAnswer {
            request: ask.request,
            yes,
        });
        self.done(if yes {
            format!("paired “{}”", ask.name)
        } else {
            "pairing refused".into()
        });
        self.modal = back.map(Modal::Mobile);
    }

    fn view_key(&mut self, k: KeyEvent, mut v: View) {
        let state = self.mobile.state().cloned();
        let phase = state.as_ref().map_or("off", |s| s.phase.as_str());
        let devices = state.as_ref().map_or(&[][..], |s| s.devices.as_slice());
        if v.devices {
            if let Some(i) = v.revoke {
                match k.code {
                    KeyCode::Char('y') => {
                        if let Some(d) = devices.get(i) {
                            self.send(ClientMsg::MobileRevoke {
                                device: d.id.clone(),
                            });
                        }
                        v.revoke = None;
                    }
                    KeyCode::Char('n') | KeyCode::Esc => v.revoke = None,
                    _ => {}
                }
            } else {
                match k.code {
                    KeyCode::Esc => v.devices = false,
                    KeyCode::Up => v.sel = v.sel.saturating_sub(1),
                    KeyCode::Down => v.sel = (v.sel + 1).min(devices.len().saturating_sub(1)),
                    KeyCode::Char('x') if !devices.is_empty() => v.revoke = Some(v.sel),
                    _ => {}
                }
            }
            self.modal = Some(Modal::Mobile(v));
            return;
        }
        // `esc` hides the modal and never stops mobile (REQ-44).
        match (phase, k.code) {
            (_, KeyCode::Esc) => return,
            ("off" | "failed", KeyCode::Enter) => {
                self.send(ClientMsg::MobileStart { read_only: false });
            }
            ("off" | "ready", KeyCode::Char('d')) => {
                v.devices = true;
                v.sel = 0;
            }
            ("starting" | "ready", KeyCode::Char('s')) => self.send(ClientMsg::MobileStop),
            ("ready", KeyCode::Char('n')) => self.send(ClientMsg::MobileCode),
            ("ready", KeyCode::Char('r')) => {
                let on = !state.as_ref().is_some_and(|s| s.read_only);
                self.send(ClientMsg::MobileReadOnly { on });
            }
            ("ready", KeyCode::Char('c')) => self.copy_link(),
            _ => {}
        }
        self.modal = Some(Modal::Mobile(v));
    }

    /// `c`: the link on the clipboard, by OSC 52 (REQ-47).
    fn copy_link(&mut self) {
        let payload = self
            .mobile
            .state()
            .map(|s| s.payload.clone())
            .filter(|p| !p.is_empty());
        match payload {
            None => self.fail("there is no code to copy; n makes a new one".into()),
            Some(link) => match super::select::copy(&link) {
                Ok(_) => self.done("link copied".into()),
                Err(e) => self.fail(format!("copy: {e:#}")),
            },
        }
    }
}

/// Draws the Mobile modal or the pair question, whichever is open.
pub(super) fn draw(f: &mut Frame, app: &App) {
    match &app.modal {
        Some(Modal::Mobile(v)) => view(f, app, v),
        Some(Modal::Pair(ask, _)) => pair(f, ask),
        _ => {}
    }
}

fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(p().dim))
}

fn tone(s: impl Into<String>, c: Color) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(c))
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    key_line(pairs, 3)
}

fn view(f: &mut Frame, app: &App, v: &View) {
    let m = &app.mobile;
    let blank = Line::default;
    if v.devices {
        return devices(f, m.state().map_or(&[][..], |s| &s.devices), v);
    }
    let (title, lines, width) = match m.phase() {
        "starting" => (" Mobile ", starting(m, app.ticks), 64),
        "failed" => (" Mobile ", failed(m), 64),
        "ready" => return ready(f, app),
        _ => {
            let phones = m.state().map_or(0, |s| s.devices.len());
            (
                " Mobile ",
                vec![
                    Line::from(" Work on your sessions from a phone."),
                    Line::from(dim(
                        " Opens a Cloudflare tunnel to this PC and shows a QR for the app.",
                    )),
                    blank(),
                    Line::from(dim(" Stays on after you close jw (it runs in the daemon).")),
                    Line::from(vec![dim(" Paired phones: "), Span::raw(phones.to_string())]),
                    blank(),
                    keys(&[("↵", "start"), ("d", "devices"), ("esc", "close")]),
                ],
                68,
            )
        }
    };
    boxed(f, title.into(), lines, width, 4);
}

/// The steps as a checklist: done, in progress, not yet.
fn starting(m: &Mobile, ticks: u64) -> Vec<Line<'static>> {
    // What the daemon's step key says is done (the spec's Corrections).
    let now = match m.state().map_or("", |s| s.step.as_str()) {
        "started" => 2,
        "registered" | "resolving" => 3,
        _ => 1,
    };
    let spin = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][(ticks % 10) as usize];
    let items = [
        "cloudflared started",
        "tunnel registered",
        "waiting for the name to resolve (about a minute)",
    ];
    let mut lines = vec![Line::from(dim(" Starting…")), Line::default()];
    for (i, text) in items.iter().enumerate() {
        let n = i + 1;
        let (mark, style) = if n < now {
            (tone("●", p().green), Style::default().fg(p().fg))
        } else if n == now {
            (tone(spin, p().cyan), Style::default().fg(p().fg))
        } else {
            (dim("○"), Style::default().fg(p().dim))
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            mark,
            Span::styled(format!(" {text}"), style),
        ]));
    }
    lines.push(Line::default());
    lines.push(keys(&[("s", "stop"), ("esc", "hide")]));
    lines
}

/// The daemon's sentence for what went wrong (REQ-49). Not installing
/// `cloudflared` has its own copy; anything else is shown as it came,
/// cleaned and cut to the box.
fn failed(m: &Mobile) -> Vec<Line<'static>> {
    let error = m.state().map_or("", |s| s.error.as_str());
    let mut lines = Vec::new();
    if let Some((what, how)) = error
        .split_once("; ")
        .filter(|_| error.contains("not installed"))
    {
        lines.push(Line::from(tone(format!(" {}.", clean(what, 60)), p().red)));
        lines.push(Line::from(dim(" Install it, then start again:")));
        lines.push(Line::default());
        lines.push(Line::from(format!("   {}", clean(how, 56))));
    } else {
        for (shown, l) in error.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            if shown == 6 {
                lines.push(Line::from(dim(" …")));
                break;
            }
            let text = clean(l, 60);
            lines.push(Line::from(tone(
                format!(" {text}"),
                if shown == 0 { p().red } else { p().dim },
            )));
        }
        if lines.is_empty() {
            lines.push(Line::from(tone(" mobile failed.", p().red)));
        }
    }
    lines.push(Line::default());
    lines.push(keys(&[("↵", "try again"), ("esc", "close")]));
    lines
}

/// The QR with its words beside it, or what to do when the window is too
/// small for it (REQ-47, 48).
fn ready(f: &mut Frame, app: &App) {
    let Some(s) = app.mobile.state() else { return };
    let left = app.mobile.left();
    let expired = s.payload.is_empty() || left == 0;
    let phones = s.devices.iter().filter(|d| d.connected).count();
    let area = f.area();
    // A code that ended, here or at the daemon, has no QR to draw.
    let qr = (!expired).then(|| Qr::new(&s.payload).ok()).flatten();
    let (qw, qh) = qr
        .as_ref()
        .map_or((0, 0), |q| (q.size() as u16, q.size().div_ceil(2) as u16));
    // The box is the QR, a space and the words, and below them the link and
    // the keys; the window needs a margin around it.
    let need = (
        NEED.0.max(qw + 1 + INFO as u16 + 4),
        NEED.1.max(qh + 3 + 2 + 2),
    );
    let small = qr.is_some() && (area.width < need.0 || area.height < need.1);
    if small {
        return too_small(f, s, need, (area.width, area.height));
    }

    let words = info(left, expired, phones, s.read_only);
    let mut lines: Vec<Line<'static>> = Vec::new();
    match &qr {
        Some(q) => {
            for (i, row) in q.cells().iter().enumerate() {
                // The QR is drawn black on white whatever the theme: a scanner
                // needs that contrast, so it is the one thing here whose
                // colours do not come from `theme::p()`.
                let mut spans: Vec<Span<'static>> = row
                    .iter()
                    .map(|&(top, bottom)| {
                        let c = |dark: bool| {
                            if dark {
                                Color::Rgb(0, 0, 0)
                            } else {
                                Color::Rgb(255, 255, 255)
                            }
                        };
                        Span::styled("▀", Style::default().fg(c(top)).bg(c(bottom)))
                    })
                    .collect();
                spans.push(Span::raw(" "));
                spans.extend(words.get(i).cloned().unwrap_or_default().spans);
                lines.push(Line::from(spans));
            }
        }
        None => lines.extend(words.iter().cloned()),
    }
    lines.push(Line::from(tone(
        format!(" {}", clean(&s.host, 60)),
        p().blue,
    )));
    lines.push(Line::default());
    // Two spaces between the keys, not three: it is the longest row here.
    lines.push(key_line(
        &[
            ("n", "new code"),
            ("d", "devices"),
            ("r", "read-only"),
            ("s", "stop"),
            ("esc", "hide"),
        ],
        2,
    ));
    let width = if qr.is_some() {
        qw + 1 + INFO as u16 + 2
    } else {
        68
    };
    boxed(f, " Mobile · running ".into(), lines, width, 2);
}

/// The column beside the QR.
fn info(left: u64, expired: bool, phones: usize, read_only: bool) -> Vec<Line<'static>> {
    let mut l = vec![
        Line::from("Scan it with"),
        Line::from("the jw app."),
        Line::default(),
    ];
    if expired {
        l.push(Line::from(tone("expired", p().red)));
        l.push(Line::from(dim("press n: new code")));
        l.push(Line::default());
    } else {
        let cells = 16usize;
        let filled = ((left.min(CODE_FOR) as usize * cells).div_ceil(CODE_FOR as usize)).min(cells);
        l.push(Line::from(dim("works once,")));
        l.push(Line::from(dim(format!(
            "for {}:{:02} more",
            left / 60,
            left % 60
        ))));
        l.push(Line::from(vec![
            tone("█".repeat(filled), p().cyan),
            tone("░".repeat(cells - filled), p().line),
        ]));
    }
    l.push(Line::default());
    l.push(Line::from(dim("phones now")));
    l.push(Line::from(tone(
        format!("{phones} connected"),
        if phones > 0 { p().green } else { p().dim },
    )));
    l.push(Line::default());
    l.push(Line::from(dim("read-only")));
    l.push(Line::from(vec![
        tone(
            if read_only { "on" } else { "off" },
            if read_only { p().yellow } else { p().dim },
        ),
        dim(" (r)"),
    ]));
    l
}

/// REQ-47: no QR below the size it needs; the link, masked, and `c`.
fn too_small(f: &mut Frame, s: &MobileState, need: (u16, u16), have: (u16, u16)) {
    let host: String = clean(&s.host, 14);
    let lines = vec![
        Line::from(tone(" The window is too small for the QR", p().yellow)),
        Line::from(dim(format!(
            " It needs {}×{}; this one is {}×{}.",
            need.0, need.1, have.0, have.1
        ))),
        Line::default(),
        Line::from(" Make it bigger, or use the link:"),
        Line::from(vec![tone(format!(" jw://{host}…/"), p().blue), dim("••••")]),
        Line::default(),
        keys(&[
            ("c", "copy link"),
            ("n", "new code"),
            ("s", "stop"),
            ("esc", "hide"),
        ]),
    ];
    boxed(f, " Mobile · running ".into(), lines, 64, 4);
}

fn devices(f: &mut Frame, list: &[MobileDevice], v: &View) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut lines: Vec<Line<'static>> = Vec::new();
    if list.is_empty() {
        lines.push(Line::from(dim(" No phone is paired.")));
    }
    for (i, d) in list.iter().enumerate() {
        let name = clean(&d.name, 14);
        let mark = if d.connected {
            tone("●", p().green)
        } else {
            dim("○")
        };
        let mut line = Line::from(vec![
            Span::raw(" "),
            mark,
            Span::raw(format!(" {name:<14}")),
            dim(format!(
                "paired {:<9} seen {}",
                ago(now, d.paired),
                ago(now, d.last_seen)
            )),
        ]);
        if i == v.sel.min(list.len() - 1) {
            line = line.style(Style::default().bg(p().sel));
        }
        lines.push(line);
    }
    lines.push(Line::default());
    match v.revoke.and_then(|i| list.get(i)) {
        Some(d) => {
            lines.push(Line::from(vec![
                Span::raw(" Revoke "),
                Span::styled(
                    format!("“{}”", clean(&d.name, 14)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("? "),
                dim("It is cut off within seconds."),
            ]));
            lines.push(Line::default());
            lines.push(keys(&[("y", "revoke"), ("n", "keep")]));
        }
        None => lines.push(keys(&[("↑↓", "select"), ("x", "revoke"), ("esc", "back")])),
    }
    boxed(f, " Paired phones ".into(), lines, 64, 4);
}

/// REQ-45: the code goes to the person to compare with the phone's.
fn pair(f: &mut Frame, ask: &Ask) {
    let left = ASK_FOR.saturating_sub(ask.arrived.elapsed()).as_secs();
    let code = if ask.code.len() > 3 {
        format!("{} {}", &ask.code[..3], &ask.code[3..])
    } else {
        ask.code.clone()
    };
    let lines = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("“{}”", ask.name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            dim(" is asking to pair."),
        ]),
        Line::default(),
        Line::from(dim(" Does the phone show this code?")),
        Line::default(),
        Line::from(Span::styled(
            format!("   {code}"),
            Style::default().fg(p().yellow).add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from(dim(" If it does not match, someone else has the QR.")),
        Line::from(vec![
            dim(" Answer "),
            tone("n", p().red),
            dim(format!(". ({left}s)")),
        ]),
        Line::default(),
        keys(&[("y", "yes, pair it"), ("n", "no")]),
    ];
    boxed(f, " Pair a phone? ".into(), lines, 64, 4);
}

/// The right of the status bar (REQ-46): `? pair request` while a question
/// waits for its moment, and the mobile indicator while it is not off.
pub(super) fn status_right(app: &App) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    if app.mobile.waiting() {
        out.push(tone("? pair request", p().yellow));
        out.push(Span::raw("  "));
    }
    let Some(s) = app.mobile.state() else {
        return out;
    };
    let phones = s.devices.iter().filter(|d| d.connected).count();
    let (text, color) = match s.phase.as_str() {
        "off" => return out,
        // A failed start is still news: the modal says what to do.
        "failed" => ("◉ mobile".to_string(), p().red),
        _ if phones > 0 => (
            format!("◉ {phones} phone{}", if phones > 1 { "s" } else { "" }),
            p().green,
        ),
        _ => ("◉ mobile".to_string(), p().cyan),
    };
    out.push(tone(text, color));
    if s.read_only {
        out.push(tone(" ro", p().yellow));
    }
    out.push(Span::raw(" "));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{DaemonMsg, read_frame};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

    use super::super::keys::Leader;
    use crate::client::Client;

    /// An app whose daemon is a thread that hands back what it is sent.
    fn rig(name: &str) -> (App, mpsc::Receiver<ClientMsg>) {
        let dir = std::env::temp_dir().join(format!("jwmob{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let (got_tx, got) = mpsc::channel();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            while let Ok(Some(m)) = read_frame::<ClientMsg>(&mut s) {
                if got_tx.send(m).is_err() {
                    break;
                }
            }
        });
        let client = Client::connect(&sock).unwrap();
        let (tx, _rx) = mpsc::channel();
        (App::bare(client, tx, Leader::DEFAULT, (120, 40)), got)
    }

    fn state(phase: &str) -> MobileState {
        MobileState {
            phase: phase.into(),
            step: String::new(),
            host: "micro-interview-purchased-unlock.trycloudflare.com".into(),
            payload: String::new(),
            expires_in: 0,
            read_only: false,
            devices: vec![],
            error: String::new(),
        }
    }

    fn ready_state() -> MobileState {
        MobileState {
            payload: format!(
                "jw://micro-interview-purchased-unlock.trycloudflare.com/{}",
                "0123456789abcdef".repeat(2)
            ),
            expires_in: 118,
            devices: vec![
                MobileDevice {
                    id: "d1".into(),
                    name: "Pixel 8".into(),
                    paired: 0,
                    last_seen: 0,
                    connected: true,
                },
                MobileDevice {
                    id: "d2".into(),
                    name: "Tablet".into(),
                    paired: 0,
                    last_seen: 0,
                    connected: false,
                },
            ],
            ..state("ready")
        }
    }

    fn feed(app: &mut App, s: MobileState) {
        app.on_mobile(DaemonMsg::MobileState(s));
    }

    fn press(app: &mut App, c: KeyCode) {
        app.mobile_key(KeyEvent::from(c));
    }

    /// What the daemon was sent, once it arrives.
    fn sent(got: &mpsc::Receiver<ClientMsg>) -> ClientMsg {
        got.recv_timeout(Duration::from_secs(2)).expect("a message")
    }

    fn nothing(got: &mpsc::Receiver<ClientMsg>) {
        assert!(got.recv_timeout(Duration::from_millis(150)).is_err());
    }

    fn screen(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect::<String>()
    }

    fn bar(app: &App) -> String {
        status_right(app)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    fn open(app: &mut App) {
        app.open_mobile();
    }

    fn ask(app: &mut App, request: u64) {
        app.on_mobile(DaemonMsg::MobilePair {
            request,
            name: "Pixel 8".into(),
            code: "482913".into(),
        });
    }

    /// Past the guard that swallows keys as the question opens.
    fn settled(app: &mut App) {
        if let Some(Modal::Pair(a, _)) = &mut app.modal {
            a.shown -= Duration::from_secs(1);
        }
    }

    #[test]
    fn each_view_sends_the_message_its_keys_name() {
        let (mut app, got) = rig("keys");
        open(&mut app);
        // Off: ↵ starts, d lists, esc closes and sends nothing.
        press(&mut app, KeyCode::Enter);
        assert_eq!(sent(&got), ClientMsg::MobileStart { read_only: false });
        press(&mut app, KeyCode::Char('s'));
        nothing(&got);
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        nothing(&got);

        // Starting: s stops.
        feed(&mut app, state("starting"));
        open(&mut app);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(sent(&got), ClientMsg::MobileStop);
        press(&mut app, KeyCode::Esc);

        // Ready: n, r, s.
        feed(&mut app, ready_state());
        open(&mut app);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(sent(&got), ClientMsg::MobileCode);
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(sent(&got), ClientMsg::MobileReadOnly { on: true });
        feed(
            &mut app,
            MobileState {
                read_only: true,
                ..ready_state()
            },
        );
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(sent(&got), ClientMsg::MobileReadOnly { on: false });
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(sent(&got), ClientMsg::MobileStop);
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        nothing(&got);

        // Failed: ↵ tries again.
        feed(
            &mut app,
            MobileState {
                error: "cloudflared is not installed; brew install cloudflared".into(),
                ..state("failed")
            },
        );
        open(&mut app);
        press(&mut app, KeyCode::Enter);
        assert_eq!(sent(&got), ClientMsg::MobileStart { read_only: false });
    }

    #[test]
    fn revoking_asks_y_or_n_and_sends_the_devices_id() {
        let (mut app, got) = rig("revoke");
        feed(&mut app, ready_state());
        open(&mut app);
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('x'));
        assert!(screen(&app, 100, 40).contains("Revoke “Tablet”?"));
        // Enter is not a yes.
        press(&mut app, KeyCode::Enter);
        nothing(&got);
        press(&mut app, KeyCode::Char('n'));
        nothing(&got);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            sent(&got),
            ClientMsg::MobileRevoke {
                device: "d2".into()
            }
        );
        // esc backs out of the list, then hides the modal.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(&app.modal, Some(Modal::Mobile(v)) if !v.devices));
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    #[test]
    fn the_question_opens_over_the_screen_and_only_y_pairs() {
        let (mut app, got) = rig("pair");
        feed(&mut app, ready_state());
        ask(&mut app, 7);
        assert!(matches!(&app.modal, Some(Modal::Pair(a, None)) if a.request == 7));
        let text = screen(&app, 100, 40);
        assert!(
            text.contains("Pair a phone?") && text.contains("482 913"),
            "{text}"
        );
        assert!(text.contains("“Pixel 8”"));
        // Keys typed as it opens do nothing, and Enter never does.
        press(&mut app, KeyCode::Char('y'));
        nothing(&got);
        settled(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('x'));
        nothing(&got);
        assert!(matches!(app.modal, Some(Modal::Pair(..))));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            sent(&got),
            ClientMsg::MobileAnswer {
                request: 7,
                yes: true
            }
        );
        assert!(app.modal.is_none());

        ask(&mut app, 8);
        settled(&mut app);
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            sent(&got),
            ClientMsg::MobileAnswer {
                request: 8,
                yes: false
            }
        );
    }

    #[test]
    fn over_the_mobile_modal_it_gives_the_modal_back() {
        let (mut app, got) = rig("over");
        feed(&mut app, ready_state());
        open(&mut app);
        press(&mut app, KeyCode::Char('d'));
        ask(&mut app, 3);
        assert!(matches!(&app.modal, Some(Modal::Pair(_, Some(v))) if v.devices));
        settled(&mut app);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(
            sent(&got),
            ClientMsg::MobileAnswer {
                request: 3,
                yes: false
            }
        );
        assert!(matches!(&app.modal, Some(Modal::Mobile(v)) if v.devices));
    }

    #[test]
    fn another_modal_or_a_picker_holds_it_back_until_it_closes() {
        let (mut app, _got) = rig("held");
        feed(&mut app, ready_state());
        app.modal = Some(Modal::Name {
            pane: 1,
            text: String::new(),
        });
        ask(&mut app, 5);
        assert!(
            matches!(app.modal, Some(Modal::Name { .. })),
            "it took over a modal"
        );
        assert!(app.mobile.waiting());
        assert!(bar(&app).contains("? pair request"));
        app.modal = None;
        app.release_asks();
        assert!(matches!(&app.modal, Some(Modal::Pair(a, _)) if a.request == 5));
        assert!(!bar(&app).contains("? pair request"));

        // A half-typed leader holds it too.
        app.modal = None;
        app.leader_at = Some(Instant::now());
        ask(&mut app, 6);
        assert!(app.modal.is_none());
        app.leader_at = None;
        app.release_asks();
        assert!(matches!(&app.modal, Some(Modal::Pair(a, _)) if a.request == 6));
    }

    #[test]
    fn a_question_that_was_held_counts_its_minute_from_when_it_arrived() {
        let (mut app, got) = rig("clock");
        app.modal = Some(Modal::Name {
            pane: 1,
            text: String::new(),
        });
        ask(&mut app, 4);
        app.mobile.held[0].arrived -= Duration::from_secs(40);
        app.modal = None;
        app.release_asks();
        let text = screen(&app, 100, 40);
        let left: u64 = text
            .split("(")
            .find_map(|s| s.split_once("s)").and_then(|(n, _)| n.parse().ok()))
            .expect("a countdown");
        assert!((18..=20).contains(&left), "{left}s in:\n{text}");
        // And the guard started when it was shown, not when it arrived.
        press(&mut app, KeyCode::Char('y'));
        nothing(&got);
        assert!(matches!(app.modal, Some(Modal::Pair(..))));
    }

    #[test]
    fn done_closes_the_question_whether_it_is_open_or_waiting() {
        let (mut app, _got) = rig("done");
        ask(&mut app, 1);
        assert!(matches!(app.modal, Some(Modal::Pair(..))));
        ask(&mut app, 2);
        assert!(app.mobile.waiting());
        app.on_mobile(DaemonMsg::MobilePairDone { request: 2 });
        assert!(!app.mobile.waiting());
        app.on_mobile(DaemonMsg::MobilePairDone { request: 1 });
        assert!(app.modal.is_none());
        // A question seen twice is one.
        ask(&mut app, 9);
        ask(&mut app, 9);
        assert!(!app.mobile.waiting());
    }

    #[test]
    fn what_a_phone_chose_is_drawn_without_control_characters() {
        let (mut app, _got) = rig("clean");
        app.on_mobile(DaemonMsg::MobilePair {
            request: 1,
            name: "Pix\u{1b}[31mel\u{7}".into(),
            code: "48\u{1b}2913".into(),
        });
        let text = screen(&app, 100, 40);
        assert!(
            text.contains("“Pix[31mel”") && text.contains("482 913"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn the_indicator_has_three_forms_and_a_read_only_mark() {
        let (mut app, _got) = rig("bar");
        assert_eq!(bar(&app), "");
        feed(&mut app, state("off"));
        assert_eq!(bar(&app), "");
        feed(
            &mut app,
            MobileState {
                devices: vec![],
                ..ready_state()
            },
        );
        assert_eq!(bar(&app).trim(), "◉ mobile");
        feed(&mut app, ready_state());
        assert_eq!(bar(&app).trim(), "◉ 1 phone");
        let mut two = ready_state();
        two.devices[1].connected = true;
        feed(&mut app, two.clone());
        assert_eq!(bar(&app).trim(), "◉ 2 phones");
        two.read_only = true;
        feed(&mut app, two);
        assert_eq!(bar(&app).trim(), "◉ 2 phones ro");
        // The colours: cyan, green, yellow for `ro`.
        let spans = status_right(&app);
        assert_eq!(spans[0].style.fg, Some(p().green));
        assert_eq!(spans[1].style.fg, Some(p().yellow));
        feed(
            &mut app,
            MobileState {
                devices: vec![],
                ..ready_state()
            },
        );
        assert_eq!(status_right(&app)[0].style.fg, Some(p().cyan));
        feed(&mut app, state("starting"));
        assert_eq!(bar(&app).trim(), "◉ mobile");
    }

    #[test]
    fn the_ready_view_draws_the_qr_and_counts_down_from_when_it_came() {
        let (mut app, _got) = rig("qr");
        feed(&mut app, ready_state());
        open(&mut app);
        let text = screen(&app, 70, 30);
        assert!(text.contains("Mobile · running"), "{text}");
        assert!(text.contains('▀'), "no QR");
        assert!(
            text.contains("for 1:5")
                || text.contains("for 1:4")
                || text.contains("for 1:3")
                || text.contains("for 1:58"),
            "{text}"
        );
        assert!(text.contains("1 connected") && text.contains("micro-interview"));
        // Time passes on the client.
        if let Some((_, at)) = &mut app.mobile.state {
            *at -= Duration::from_secs(119);
        }
        let text = screen(&app, 70, 30);
        assert!(
            text.contains("expired") && text.contains("press n"),
            "{text}"
        );
        assert!(!text.contains('▀'));
    }

    #[test]
    fn below_the_size_it_needs_the_qr_gives_way_to_the_link() {
        let (mut app, _got) = rig("small");
        feed(&mut app, ready_state());
        open(&mut app);
        let text = screen(&app, 62, 20);
        assert!(text.contains("too small for the QR"), "{text}");
        assert!(
            text.contains("It needs 70×30; this one is 62×20."),
            "{text}"
        );
        assert!(!text.contains('▀') && text.contains("copy link"));
        // The token is not on screen.
        assert!(!text.contains("0123456789abcdef"), "{text}");
        // Exactly the size: the QR is drawn.
        assert!(screen(&app, 70, 30).contains('▀'));
        assert!(!screen(&app, 69, 30).contains('▀'));
        assert!(!screen(&app, 70, 29).contains('▀'));
    }

    #[test]
    fn starting_is_a_checklist_that_follows_the_steps() {
        let (mut app, _got) = rig("steps");
        let mut s = state("starting");
        s.step = "registered".into();
        feed(&mut app, s);
        open(&mut app);
        let text = screen(&app, 100, 40);
        assert!(
            text.contains("● cloudflared started") && text.contains("● tunnel registered"),
            "{text}"
        );
        assert!(text.contains("waiting for the name to resolve"));
        assert!(text.contains("stop") && text.contains("hide"), "{text}");
    }

    #[test]
    fn failures_show_the_daemons_words() {
        let (mut app, _got) = rig("fail");
        feed(
            &mut app,
            MobileState {
                error: "cloudflared is not installed; brew install cloudflared".into(),
                ..state("failed")
            },
        );
        open(&mut app);
        let text = screen(&app, 100, 40);
        assert!(text.contains("cloudflared is not installed."), "{text}");
        assert!(text.contains("brew install cloudflared") && text.contains("try again"));
        feed(
            &mut app,
            MobileState {
                error: "cloudflared stopped before the tunnel was up:\n\u{1b}[31mERR no network\nline 3".into(),
                ..state("failed")
            },
        );
        let text = screen(&app, 100, 40);
        assert!(
            text.contains("cloudflared stopped before the tunnel was up:"),
            "{text}"
        );
        assert!(text.contains("[31mERR no network") && !text.contains('\u{1b}'));
    }

    #[test]
    fn the_confirm_chip_follows_the_question() {
        let (mut app, _got) = rig("chip");
        assert_ne!(
            super::super::draw::mode_of(&app),
            super::super::draw::Mode::Confirm
        );
        ask(&mut app, 1);
        assert_eq!(
            super::super::draw::mode_of(&app),
            super::super::draw::Mode::Confirm
        );
    }
}
