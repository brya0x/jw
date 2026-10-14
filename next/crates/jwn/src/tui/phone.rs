//! The phone link inside the terminal client (prototype: `docs/prototypes/tui-phone.html`):
//! what `jwn pair` does, as a box that `m` opens. The daemon is asked from a thread of its
//! own, over its own connection, because turning the link on waits for the tunnel (up to
//! a minute and a half) and the client must keep working meanwhile. The thread tells the
//! client what happened with [`Event`]s and is told what to do with [`Say`]s; the box and
//! the state are `View`'s.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use contract::Cmd;
use qrcode::{Color as Module, QrCode};
use wire::Client;

use super::{Grid, Rect, Rgb, Style, clip, look};

/// What the phone needs to reach this PC.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Link {
    pub uri: String,
    /// `address:port` on the private network.
    pub lan: Option<String>,
    /// The host name of the tunnel, when there is one.
    pub tunnel: Option<String>,
    /// The tunnel is still opening: the link over the private address is good meanwhile.
    pub pending: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    Off,
    Starting,
    Ready(Link),
    /// A phone asks to pair: its name, and the code both screens show.
    Asking {
        link: Link,
        name: String,
        code: String,
    },
    Failed(String),
}

impl State {
    /// The link is on (a phone may be asking).
    pub fn is_on(&self) -> bool {
        matches!(self, State::Ready(_) | State::Asking { .. })
    }
}

/// What the thread reports.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    Ready(Link),
    Failed(String),
    Ask {
        name: String,
        code: String,
    },
    /// The question was answered; how many phones are paired now.
    Answered {
        yes: bool,
    },
    Paired(usize),
    Off,
}

/// What the client tells the thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Say {
    Answer(bool),
    Stop,
}

/// The thread, from the client's side.
pub struct Worker {
    pub events: Receiver<Event>,
    says: Sender<Say>,
}

impl Worker {
    pub fn spawn() -> Worker {
        Worker::spawn_at(identity::socket_path())
    }

    fn spawn_at(socket: PathBuf) -> Worker {
        let (event_tx, events) = channel();
        let (says, say_rx) = channel();
        thread::spawn(move || work(&socket, &event_tx, &say_rx));
        Worker { events, says }
    }

    pub fn say(&self, say: Say) {
        let _ = self.says.send(say);
    }
}

/// How long a wait for a phone lasts, so that a `Stop` is heard within it.
const LISTEN_SECONDS: u32 = 2;
/// How long the answer to a pairing question is waited for before it counts as no.
const ANSWER_WAIT: Duration = Duration::from_secs(120);
const START_WAIT: Duration = Duration::from_secs(120);

/// Ask the daemon for a command's result: its message when it refuses, and why when it cannot be asked.
fn ask(client: &mut Client, cmd: Cmd, wait: Duration) -> Result<Option<serde_json::Value>, String> {
    client
        .cmd_within(cmd, wait)
        .map_err(|e| e.to_string())?
        .map_err(|e| e.message)
}

/// Turn the link on and report it; then listen for phones until told to stop or until the
/// daemon cannot be reached.
fn work(socket: &Path, events: &Sender<Event>, says: &Receiver<Say>) {
    let tell = |e: Event| events.send(e).is_ok();
    let started = Client::connect(socket, "jwn-tui-phone")
        .map_err(|e| format!("cannot reach {}: {e}", socket.display()))
        .and_then(|mut client| {
            let link = ask(&mut client, Cmd::MobileStart {}, START_WAIT)?
                .as_ref()
                .and_then(link_of)
                .ok_or("the daemon gave no link")?;
            Ok((client, link))
        });
    let (mut client, link) = match started {
        Ok(ok) => ok,
        Err(why) => {
            tell(Event::Failed(why));
            return;
        }
    };
    let pending = link.pending;
    if !tell(Event::Ready(link)) || !tell(Event::Paired(crate::pair::paired_count())) {
        return;
    }
    if let Err(why) = listen(&mut client, says, &tell, pending) {
        tell(Event::Failed(why));
    }
}

/// The loop of the link: each phone that asks is told to the client, and its answer is
/// passed on. `Ok` when the link was turned off.
fn listen(
    client: &mut Client,
    says: &Receiver<Say>,
    tell: &impl Fn(Event) -> bool,
    mut pending: bool,
) -> Result<(), String> {
    loop {
        if let Ok(Say::Stop) = says.try_recv() {
            return stop(client, tell);
        }
        // The tunnel opens in the daemon: the link is asked for again, with the same token,
        // until it says the tunnel is up or that there is none.
        if pending
            && let Some(link) = ask(client, Cmd::MobileLink {}, START_WAIT)?
                .as_ref()
                .and_then(link_of)
        {
            pending = link.pending;
            if !pending {
                tell(Event::Ready(link));
            }
        }
        let wait = Cmd::MobileWait {
            seconds: LISTEN_SECONDS,
        };
        let within = Duration::from_secs(u64::from(LISTEN_SECONDS) + 10);
        let Some(asked) = ask(client, wait, within)? else {
            continue;
        };
        let text = |key: &str| asked[key].as_str().unwrap_or("?").to_owned();
        if !tell(Event::Ask {
            name: text("name"),
            code: text("code"),
        }) {
            return Ok(());
        }
        let yes = match says.recv_timeout(ANSWER_WAIT) {
            Ok(Say::Answer(yes)) => yes,
            Ok(Say::Stop) => {
                ask(client, Cmd::MobileAnswer { yes: false }, START_WAIT)?;
                return stop(client, tell);
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        ask(client, Cmd::MobileAnswer { yes }, START_WAIT)?;
        tell(Event::Answered { yes });
        tell(Event::Paired(crate::pair::paired_count()));
    }
}

fn stop(client: &mut Client, tell: &impl Fn(Event) -> bool) -> Result<(), String> {
    ask(client, Cmd::MobileStop {}, START_WAIT)?;
    tell(Event::Off);
    Ok(())
}

fn link_of(v: &serde_json::Value) -> Option<Link> {
    let text = |key: &str| v[key].as_str().map(str::to_owned);
    Some(Link {
        uri: text("uri")?,
        lan: text("lan"),
        tunnel: text("tunnel"),
        pending: v["pending"].as_bool().unwrap_or(false),
    })
}

/// The QR of `text` as the rows of half blocks that draw it: dark modules on a light ground
/// with a quiet zone, whatever the theme (a scanner wants them so).
pub fn qr_rows(text: &str) -> Option<Vec<String>> {
    const QUIET: usize = 2;
    let code = QrCode::new(text).ok()?;
    let width = code.width();
    let colours = code.to_colors();
    let dark = |x: usize, y: usize| {
        (QUIET..QUIET + width).contains(&x)
            && (QUIET..QUIET + width).contains(&y)
            && colours[(y - QUIET) * width + (x - QUIET)] == Module::Dark
    };
    let side = width + 2 * QUIET;
    Some(
        (0..side)
            .step_by(2)
            .map(|y| {
                (0..side)
                    .map(|x| match (dark(x, y), dark(x, y + 1)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect()
            })
            .collect(),
    )
}

/// The box that `m` opens: title, border colour and lines, by the state.
fn text(state: &State, paired: usize) -> (&'static str, Rgb, Vec<(String, Style)>) {
    let plain = |s: &str| (s.to_owned(), Style::fg(look::fg()));
    let dim = |s: &str| (s.to_owned(), Style::fg(look::dim()));
    let blank = || (String::new(), Style::default());
    match state {
        State::Asking { name, code, .. } => (
            "emparejar un teléfono",
            look::yellow(),
            vec![
                (
                    format!("¿Emparejar «{name}»?"),
                    Style::fg(look::fg()).bold(),
                ),
                blank(),
                plain(&format!("El teléfono muestra el código  {code}")),
                dim("Solo si es el mismo que ves en la pantalla del teléfono."),
                blank(),
                dim(hint(state)),
            ],
        ),
        State::Failed(why) => (
            "teléfono",
            look::red(),
            vec![
                plain("No se pudo encender el enlace con el teléfono:"),
                (clip(why, 70), Style::fg(look::red())),
                blank(),
                dim(hint(state)),
            ],
        ),
        State::Starting => (
            "teléfono",
            look::magenta(),
            vec![
                plain("Encendiendo el enlace…"),
                dim("Con el túnel de Cloudflare puede tardar hasta un minuto."),
                blank(),
                dim(hint(state)),
            ],
        ),
        State::Ready(link) => (
            "teléfono",
            look::magenta(),
            link_lines(link, paired, hint(state)),
        ),
        State::Off => (
            "teléfono",
            look::magenta(),
            vec![dim("El enlace está apagado.")],
        ),
    }
}

fn link_lines(link: &Link, paired: usize, hint: &str) -> Vec<(String, Style)> {
    let plain = |s: &str| (s.to_owned(), Style::fg(look::fg()));
    let dim = |s: &str| (s.to_owned(), Style::fg(look::dim()));
    let mut lines = vec![
        (
            "Escanea el código con la app".to_owned(),
            Style::fg(look::fg()).bold(),
        ),
        dim("o pega este enlace en «+ emparejar otro PC»:"),
        (link.uri.clone(), Style::fg(look::cyan())),
        (String::new(), Style::default()),
    ];
    if let Some(lan) = &link.lan {
        lines.push(plain(&format!("red local   {lan}")));
    }
    lines.push(match &link.tunnel {
        Some(host) => plain(&format!("túnel       {host}")),
        None if link.pending => (
            "túnel       abriendo… (hasta un minuto; la red local ya sirve)".to_owned(),
            Style::fg(look::yellow()),
        ),
        None => (
            "túnel       sin túnel: solo desde tu red".to_owned(),
            Style::fg(look::yellow()),
        ),
    });
    lines.push((String::new(), Style::default()));
    lines.push(dim(&match paired {
        0 => "ningún teléfono emparejado todavía".to_owned(),
        1 => "1 teléfono emparejado".to_owned(),
        n => format!("{n} teléfonos emparejados"),
    }));
    lines.push(dim("Esperando al teléfono…"));
    lines.push((String::new(), Style::default()));
    lines.push(dim(hint));
    lines
}

/// The keys of the box, for its mode chip.
pub fn hint(state: &State) -> &'static str {
    match state {
        State::Asking { .. } => "y emparejar · cualquier otra tecla rechaza",
        State::Starting => "esc cerrar (el enlace sigue abriéndose)",
        State::Failed(_) => "cualquier tecla para seguir",
        _ => "c copiar enlace · s apagar el enlace · esc cerrar",
    }
}

/// Draw the box over `area`, near its top and centred. With the link ready the QR goes at its
/// left when the area is big enough for it; otherwise the link text alone is shown (`c`
/// copies it).
pub fn draw(g: &mut Grid, (x, y, w, h): Rect, state: &State, paired: usize) {
    let (title, border, lines) = text(state, paired);
    let qr = match state {
        State::Ready(link) => qr_rows(&link.uri),
        _ => None,
    };
    let text_width = lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(54);
    // The QR column, a gutter, and the lines at its right.
    let qr_w = qr.as_ref().map_or(0, |q| q[0].chars().count());
    let qr_h = qr.as_ref().map_or(0, Vec::len);
    let fits = qr.is_some() && w >= qr_w + text_width + 12 && h >= qr_h + 6;
    let bw = if fits {
        qr_w + text_width + 12
    } else {
        (text_width + 6).max(title.chars().count() + 6)
    }
    .min(w.saturating_sub(2))
    .max(20);
    let bh = if fits {
        qr_h.max(lines.len()) + 4
    } else {
        lines.len() + 4
    };
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 2);
    for r in 0..bh {
        for c in 0..bw {
            if by + r < g.rows && bx + c < g.cols {
                g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(look::panel()));
            }
        }
    }
    g.boxed(
        (bx, by, bw, bh),
        title,
        border,
        Style::fg(look::fg()).bold(),
        None,
    );
    let left = if fits { bx + 3 + qr_w + 4 } else { bx + 3 };
    let room = (bx + bw).saturating_sub(left + 3);
    for (i, (text, style)) in lines.iter().enumerate() {
        // Beside the QR the keys sit at the bottom, in line with its last row.
        let row = if fits && i + 1 == lines.len() {
            by + bh - 2
        } else {
            by + 2 + i
        };
        g.put(left, row, &clip(text, room), style.on(look::panel()));
    }
    if let (true, Some(rows)) = (fits, &qr) {
        let light = Style {
            fg: Some((0, 0, 0)),
            bg: Some((255, 255, 255)),
            ..Style::default()
        };
        for (i, row) in rows.iter().enumerate() {
            g.put(bx + 3, by + 2 + i, row, light);
        }
    } else if qr.is_some() {
        g.put(
            left,
            by + bh - 3,
            "agranda la terminal para ver el QR",
            Style::fg(look::dim()).on(look::panel()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link {
            uri: "jwn://192.168.1.20:7871/t0k?fp=ab".into(),
            lan: Some("192.168.1.20:7871".into()),
            tunnel: None,
            pending: false,
        }
    }

    #[test]
    fn the_qr_is_dark_modules_inside_a_quiet_zone_two_rows_to_a_line() {
        let rows = qr_rows("jwn://a/b").expect("a short link fits a code");
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == width), "a square");
        assert_eq!(rows.len(), width.div_ceil(2));
        assert!(rows[0].chars().all(|c| c == ' '), "a quiet zone above");
        assert!(rows.iter().any(|r| r.contains('█')));
        assert!(
            rows.iter()
                .all(|r| r.starts_with("  ") && r.ends_with("  ")),
            "and at the sides"
        );
    }

    #[test]
    fn a_link_without_a_tunnel_says_so_and_keeps_the_local_address() {
        let lines = link_lines(&link(), 0, "h");
        let all: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert!(all.contains(&"red local   192.168.1.20:7871"), "{all:?}");
        assert!(
            all.contains(&"túnel       sin túnel: solo desde tu red"),
            "{all:?}"
        );
        assert!(all.contains(&"ningún teléfono emparejado todavía"));
        let lines = link_lines(&link(), 2, "h");
        assert!(lines.iter().any(|(l, _)| l == "2 teléfonos emparejados"));
    }

    #[test]
    fn the_link_reply_is_read_with_its_optional_parts() {
        let v = serde_json::json!({"uri": "jwn://x", "lan": "10.0.0.2:1", "tunnel": "a.b"});
        assert_eq!(
            link_of(&v),
            Some(Link {
                uri: "jwn://x".into(),
                lan: Some("10.0.0.2:1".into()),
                tunnel: Some("a.b".into()),
                pending: false,
            })
        );
        assert_eq!(
            link_of(&serde_json::json!({"uri": "jwn://x"}))
                .unwrap()
                .tunnel,
            None
        );
        assert_eq!(link_of(&serde_json::json!({})), None);
    }
    /// A daemon of its own in a short private directory, with a phone link that listens on loopback and has no tunnel.
    struct Pc {
        dir: PathBuf,
        daemon: Option<daemon::Daemon>,
    }

    impl Pc {
        fn start(link: daemon::LinkConfig) -> Pc {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = PathBuf::from(format!(
                "/tmp/jwn-ph-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let config = daemon::Config {
                socket: dir.join("s.sock"),
                state_dir: dir.join("state"),
                shell: Some("/bin/sh".into()),
                tcp: None,
                env: Vec::new(),
                blocks: true,
                link,
                hook_exe: None,
                gh: None,
                github_every: Duration::from_millis(100),
            };
            Pc {
                dir,
                daemon: Some(daemon::Daemon::start(config).expect("a daemon")),
            }
        }

        fn socket(&self) -> PathBuf {
            self.dir.join("s.sock")
        }
    }

    impl Drop for Pc {
        fn drop(&mut self) {
            if let Some(d) = self.daemon.take() {
                d.stop();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn loopback() -> daemon::LinkConfig {
        daemon::LinkConfig {
            tunnel: None,
            resolve: false,
            lan: true,
            lan_ip: Some(std::net::Ipv4Addr::LOCALHOST),
        }
    }

    fn next(w: &Worker) -> Event {
        w.events
            .recv_timeout(Duration::from_secs(10))
            .expect("the thread reports")
    }

    #[test]
    fn the_thread_turns_the_link_on_reports_it_and_turns_it_off_when_told() {
        let pc = Pc::start(loopback());
        let w = Worker::spawn_at(pc.socket());
        let Event::Ready(link) = next(&w) else {
            panic!("the link was not reported")
        };
        assert!(
            link.uri
                .starts_with(&format!("{}://127.0.0.1:", identity::get().name)),
            "{}",
            link.uri
        );
        assert!(
            link.lan
                .as_deref()
                .is_some_and(|a| a.starts_with("127.0.0.1:"))
        );
        assert_eq!(link.tunnel, None, "there is no tunnel program");
        assert!(matches!(next(&w), Event::Paired(_)));
        w.say(Say::Stop);
        assert_eq!(next(&w), Event::Off);
        // The daemon's link is off: a new thread turns it on again.
        let again = Worker::spawn_at(pc.socket());
        assert!(matches!(next(&again), Event::Ready(_)));
        assert!(matches!(next(&again), Event::Paired(_)));
        again.say(Say::Stop);
        assert_eq!(next(&again), Event::Off);
    }

    #[test]
    fn a_daemon_that_cannot_open_the_link_gives_its_reason() {
        let pc = Pc::start(daemon::LinkConfig::off());
        let w = Worker::spawn_at(pc.socket());
        let Event::Failed(why) = next(&w) else {
            panic!("a failure was expected")
        };
        assert!(why.contains("no way to reach this PC"), "{why}");
    }

    #[test]
    fn no_daemon_is_a_failure_that_names_the_socket() {
        let w = Worker::spawn_at(PathBuf::from("/tmp/jwn-ph-nobody-here.sock"));
        let Event::Failed(why) = next(&w) else {
            panic!("a failure was expected")
        };
        assert!(why.contains("jwn-ph-nobody-here.sock"), "{why}");
    }
    #[test]
    fn the_link_comes_over_the_network_first_and_again_with_the_tunnel_when_it_is_up() {
        use std::os::unix::fs::PermissionsExt;
        // A tunnel program that takes a second to say its host.
        let dir = PathBuf::from(format!("/tmp/jwn-ph-slow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("cloudflared");
        std::fs::write(
            &script,
            "#!/bin/sh\nsleep 1\necho 'INF https://slow-4.trycloudflare.com' >&2\nexec sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pc = Pc::start(daemon::LinkConfig {
            tunnel: Some(script.to_string_lossy().into()),
            resolve: false,
            lan: true,
            lan_ip: Some(std::net::Ipv4Addr::LOCALHOST),
        });
        let w = Worker::spawn_at(pc.socket());
        let Event::Ready(first) = next(&w) else {
            panic!("the link was not reported")
        };
        assert!(first.pending && first.tunnel.is_none(), "{first:?}");
        assert!(first.lan.is_some(), "it did not wait for the tunnel");
        assert!(matches!(next(&w), Event::Paired(_)));
        let Event::Ready(second) = next(&w) else {
            panic!("the link with the tunnel was not reported")
        };
        assert!(!second.pending);
        assert_eq!(second.tunnel.as_deref(), Some("slow-4.trycloudflare.com"));
        assert!(
            second.uri.ends_with("&t=slow-4.trycloudflare.com"),
            "{}",
            second.uri
        );
        assert_eq!(
            second.uri.split('/').nth(3).unwrap().split('?').next(),
            first.uri.split('/').nth(3).unwrap().split('?').next(),
            "the same token: the QR on the screen stays good"
        );
        w.say(Say::Stop);
        assert_eq!(next(&w), Event::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
