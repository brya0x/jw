//! The phone link inside the terminal client (prototype: `docs/prototypes/tui-phone.html`):
//! what `jwn pair` does, as a box that `m` opens. The daemon is asked from a thread of its
//! own, over its own connection, because turning the link on waits for the tunnel (up to
//! a minute and a half) and the client must keep working meanwhile. The thread tells the
//! client what happened with [`Event`]s and is told what to do with [`Say`]s; the box and
//! the state are `View`'s.

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
        let (event_tx, events) = channel();
        let (says, say_rx) = channel();
        thread::spawn(move || work(&event_tx, &say_rx));
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

fn work(events: &Sender<Event>, says: &Receiver<Say>) {
    let tell = |e: Event| events.send(e).is_ok();
    let socket = identity::socket_path();
    let mut client = match Client::connect(&socket, "jwn-tui-phone") {
        Ok(c) => c,
        Err(e) => {
            tell(Event::Failed(format!(
                "cannot reach {}: {e}",
                socket.display()
            )));
            return;
        }
    };
    let started = client.cmd_within(Cmd::MobileStart {}, START_WAIT);
    let link = match started {
        Ok(Ok(Some(v))) => match link_of(&v) {
            Some(link) => link,
            None => {
                tell(Event::Failed("the daemon gave no link".into()));
                return;
            }
        },
        Ok(Ok(None)) => {
            tell(Event::Failed("the daemon gave no link".into()));
            return;
        }
        Ok(Err(e)) => {
            tell(Event::Failed(e.message));
            return;
        }
        Err(e) => {
            tell(Event::Failed(e.to_string()));
            return;
        }
    };
    if !tell(Event::Ready(link)) || !tell(Event::Paired(crate::pair::paired_count())) {
        return;
    }
    loop {
        if let Ok(Say::Stop) = says.try_recv() {
            let _ = client.cmd(Cmd::MobileStop {});
            tell(Event::Off);
            return;
        }
        match client.cmd_within(
            Cmd::MobileWait {
                seconds: LISTEN_SECONDS,
            },
            Duration::from_secs(u64::from(LISTEN_SECONDS) + 10),
        ) {
            Ok(Ok(Some(ask))) => {
                let name = ask["name"].as_str().unwrap_or("?").to_owned();
                let code = ask["code"].as_str().unwrap_or("?").to_owned();
                if !tell(Event::Ask { name, code }) {
                    return;
                }
                let yes = match says.recv_timeout(ANSWER_WAIT) {
                    Ok(Say::Answer(yes)) => yes,
                    Ok(Say::Stop) => {
                        let _ = client.cmd(Cmd::MobileAnswer { yes: false });
                        let _ = client.cmd(Cmd::MobileStop {});
                        tell(Event::Off);
                        return;
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                let _ = client.cmd(Cmd::MobileAnswer { yes });
                tell(Event::Answered { yes });
                tell(Event::Paired(crate::pair::paired_count()));
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                tell(Event::Failed(e.message));
                return;
            }
            Err(e) => {
                tell(Event::Failed(e.to_string()));
                return;
            }
        }
    }
}

fn link_of(v: &serde_json::Value) -> Option<Link> {
    let text = |key: &str| v[key].as_str().map(str::to_owned);
    Some(Link {
        uri: text("uri")?,
        lan: text("lan"),
        tunnel: text("tunnel"),
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
                tunnel: Some("a.b".into())
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
}
