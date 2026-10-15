//! The shell as blocks in the terminal client: colours, the lines a block list becomes, and
//! the clipboard. Pure, so it is tested without a terminal.

use contract::{Block, Color, Span};

use super::{Grid, Rect, Rgb, Style, base, clip, look};

/// The 256-colour palette: the sixteen named colours are the theme's (black and bright white
/// turn into the text colour on a light theme, where a literal one would vanish), the rest
/// the usual cube and gray ramp.
pub fn palette(i: u8) -> Rgb {
    let dark = look::is_dark();
    match i {
        0 if dark => look::line(),
        0 | 15 if !dark => look::fg(),
        1 | 9 => look::red(),
        2 | 10 => look::green(),
        3 | 11 => look::yellow(),
        4 | 12 => look::blue(),
        5 | 13 => look::magenta(),
        6 | 14 => look::cyan(),
        7 => look::fg(),
        8 => look::dim(),
        15 => (0xff, 0xff, 0xff),
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            (level(n / 36), level(n % 36 / 6), level(n % 6))
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

pub fn color_of(c: &Color) -> Rgb {
    match c {
        Color::Index(i) => palette(*i),
        Color::Rgb(hex) => {
            let h = hex.trim_start_matches('#');
            let part = |a: usize| h.get(a..a + 2).and_then(|p| u8::from_str_radix(p, 16).ok());
            match (part(0), part(2), part(4)) {
                (Some(r), Some(g), Some(b)) => (r, g, b),
                _ => look::fg(),
            }
        }
    }
}

pub fn span_style(s: &Span) -> Style {
    Style {
        fg: s.fg.as_ref().map(color_of),
        bg: s.bg.as_ref().map(color_of),
        bold: s.b,
        italic: s.i,
        underline: s.u,
        reverse: false,
    }
}

/// One line of the list of blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub block: Option<u64>,
    /// The bar at the left: how the block is going.
    pub bar: Option<Rgb>,
    pub spans: Vec<(String, Style)>,
    /// Text at the right edge (the time, or how it ended).
    pub right: Option<(String, Style)>,
}

pub fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// The last two folders of a path, so it fits a header.
pub fn short(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() > 2 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        path.to_owned()
    }
}

/// Whether a block's output has anything to see: a program that draws on the normal screen (claude)
/// has painted once a row of its own has more than blanks in it.
pub fn has_ink(out: &[Vec<Span>]) -> bool {
    out.iter().flatten().any(|s| !s.t.trim().is_empty())
}

/// The program a command line names: the file name of its first word.
pub fn program_name(cmd: &str) -> String {
    base(cmd.split_whitespace().next().unwrap_or(cmd)).to_owned()
}

/// The frames of the spinner of a program that is starting, one every 110 ms.
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// What a program that has the pane's screen is doing when the pane cannot show its screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// It has not painted anything yet.
    Starting,
    /// It left its screen and has not finished: the shell's own screen is back under it.
    Closing,
}

/// What a pane says instead of its screen: the spinner and `iniciando <name>…` (and that `^C`
/// cancels) or `cerrando <name>…` (and that it is waiting for it to end), in the middle of its box
/// (spec REQ-259, REQ-264).
pub fn loading(g: &mut Grid, (x, y, w, h): Rect, phase: Phase, name: &str, now_ms: u64) {
    let frame = SPIN[(now_ms / 110) as usize % SPIN.len()];
    let (what, hint) = match phase {
        Phase::Starting => ("iniciando", "^C cancela"),
        Phase::Closing => ("cerrando", "esperando a que termine"),
    };
    let line = clip(&format!("{frame} {what} {name}…"), w.saturating_sub(4));
    let mid = y + h / 2;
    let centre = |text: &str| x + w.saturating_sub(text.chars().count()) / 2;
    g.put(
        centre(&line),
        mid.saturating_sub(1),
        &line,
        Style::fg(look::yellow()),
    );
    // The border is the last row of the box: in a box too short for both lines the hint is left out.
    if mid + 1 < y + h.saturating_sub(1) {
        g.put(centre(hint), mid + 1, hint, Style::fg(look::dim()));
    }
}

/// How many lines a block takes in the list: its two heading lines, the notices, its output and a gap.
/// It does not depend on the width, so the list can be measured without building it.
fn block_height(b: &Block) -> usize {
    2 + usize::from(b.truncated) + usize::from(b.full) + b.out.len() + 1
}

/// How many lines the list of `blocks` has.
pub fn total_lines(blocks: &[Block]) -> usize {
    blocks.iter().map(block_height).sum()
}

/// At most `rows` lines of the list from line `start` (as `block_lines` has them): only the blocks
/// that reach them are built, so what a pane costs to draw is what it shows and not its history.
pub fn window_lines(
    blocks: &[Block],
    now_ms: u64,
    width: usize,
    start: usize,
    rows: usize,
) -> Vec<Line> {
    let mut at = 0;
    let mut out = Vec::new();
    for b in blocks {
        let height = block_height(b);
        if at + height > start && out.len() < rows {
            let lines = block_lines(std::slice::from_ref(b), now_ms, width);
            out.extend(
                lines
                    .into_iter()
                    .skip(start.saturating_sub(at))
                    .take(rows - out.len()),
            );
        }
        at += height;
        if out.len() >= rows {
            break;
        }
    }
    out
}

/// Every block as lines, oldest first: the folder and time, the command, the output, a gap.
pub fn block_lines(blocks: &[Block], now_ms: u64, width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for b in blocks {
        let bar = if b.running {
            look::yellow()
        } else if b.code.unwrap_or(0) != 0 {
            look::red()
        } else {
            look::green()
        };
        let (right, right_style) = if b.running {
            (
                format!("corriendo {}", duration(now_ms.saturating_sub(b.started))),
                Style::fg(look::dim()),
            )
        } else {
            let time = duration(b.ms.unwrap_or(0));
            match b.code {
                Some(c) if c != 0 => (format!("salió {c} · {time}"), Style::fg(look::red())),
                _ => (time, Style::fg(look::dim())),
            }
        };
        let id = Some(b.id);
        let room = width.saturating_sub(right.chars().count() + 3);
        out.push(Line {
            block: id,
            bar: Some(bar),
            spans: vec![(clip(&short(&b.cwd), room.max(4)), Style::fg(look::dim()))],
            right: Some((right, right_style)),
        });
        out.push(Line {
            block: id,
            bar: Some(bar),
            spans: vec![(format!("❯ {}", b.cmd), Style::default().bold())],
            right: None,
        });
        if b.truncated {
            out.push(Line {
                block: id,
                bar: Some(bar),
                spans: vec![("… el inicio se recortó".into(), Style::fg(look::dim()))],
                right: None,
            });
        }
        if b.full {
            out.push(Line {
                block: id,
                bar: Some(bar),
                spans: vec![("pantalla completa".into(), Style::fg(look::dim()))],
                right: None,
            });
        }
        for row in &b.out {
            out.push(Line {
                block: id,
                bar: Some(bar),
                spans: row.iter().map(|s| (s.t.clone(), span_style(s))).collect(),
                right: None,
            });
        }
        out.push(Line {
            block: None,
            bar: None,
            spans: vec![],
            right: None,
        });
    }
    out
}

/// What a block printed, as text.
pub fn output_text(b: &Block) -> String {
    b.out
        .iter()
        .map(|row| row.iter().map(|s| s.t.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The escape that asks the terminal to put `text` on the clipboard (OSC 52).
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use contract::Color;

    use super::*;

    #[test]
    fn colours_are_the_themes_for_the_named_ones_and_the_usual_ramp_for_the_rest() {
        assert_eq!(palette(1), look::red());
        assert_eq!(palette(2), look::green());
        assert_eq!(palette(16), (0, 0, 0));
        assert_eq!(palette(196), (255, 0, 0));
        assert_eq!(palette(231), (255, 255, 255));
        assert_eq!(palette(232), (8, 8, 8));
        assert_eq!(color_of(&Color::Rgb("#e5c07b".into())), (0xe5, 0xc0, 0x7b));
        assert_eq!(color_of(&Color::Rgb("nope".into())), look::fg());
    }

    #[test]
    fn base64_matches_the_reference_and_osc52_wraps_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"cargo test"), "Y2FyZ28gdGVzdA==");
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn durations_read_as_milliseconds_then_seconds() {
        assert_eq!(duration(120), "120ms");
        assert_eq!(duration(4200), "4.2s");
    }

    #[test]
    fn a_path_is_shortened_to_its_last_two_folders() {
        assert_eq!(short("/home/u/code/app-login"), "…/code/app-login");
        assert_eq!(short("/tmp"), "/tmp");
    }

    fn block(id: u64, cmd: &str, code: Option<i32>, running: bool) -> Block {
        Block {
            id,
            cmd: cmd.into(),
            cwd: "/w/x/y".into(),
            started: 1000,
            running,
            code,
            ms: code.map(|_| 4200),
            truncated: false,
            full: false,
            out: vec![vec![Span {
                t: "out".into(),
                fg: None,
                bg: None,
                b: false,
                i: false,
                u: false,
            }]],
        }
    }

    #[test]
    fn a_block_is_a_header_a_command_its_output_and_a_gap() {
        let lines = block_lines(&[block(1, "ls", Some(0), false)], 0, 40);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].right.as_ref().unwrap().0, "4.2s");
        assert_eq!(lines[1].spans[0].0, "❯ ls");
        assert_eq!(lines[2].spans[0].0, "out");
        assert!(lines[3].block.is_none() && lines[3].bar.is_none());
        assert!(
            lines[..3]
                .iter()
                .all(|l| l.bar == Some(look::green()) && l.block == Some(1))
        );
    }

    #[test]
    fn a_failure_is_red_and_says_how_it_ended() {
        let lines = block_lines(&[block(1, "false", Some(101), false)], 0, 40);
        assert_eq!(lines[0].bar, Some(look::red()));
        assert_eq!(lines[0].right.as_ref().unwrap().0, "salió 101 · 4.2s");
    }

    #[test]
    fn a_running_block_is_yellow_with_the_time_so_far() {
        let lines = block_lines(&[block(1, "sleep 9", None, true)], 3000, 40);
        assert_eq!(lines[0].bar, Some(look::yellow()));
        assert_eq!(lines[0].right.as_ref().unwrap().0, "corriendo 2.0s");
    }

    #[test]
    fn the_output_text_is_the_rows_joined() {
        let mut b = block(1, "x", Some(0), false);
        b.out = vec![
            vec![
                Span {
                    t: "a".into(),
                    fg: None,
                    bg: None,
                    b: false,
                    i: false,
                    u: false,
                },
                Span {
                    t: "b".into(),
                    fg: None,
                    bg: None,
                    b: true,
                    i: false,
                    u: false,
                },
            ],
            vec![Span {
                t: "c".into(),
                fg: None,
                bg: None,
                b: false,
                i: false,
                u: false,
            }],
        ];
        assert_eq!(output_text(&b), "ab\nc");
    }
}

#[cfg(test)]
mod starting_tests {
    use super::*;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<Span>> {
        rows.iter()
            .map(|r| {
                r.iter()
                    .map(|t| Span {
                        t: (*t).into(),
                        fg: None,
                        bg: None,
                        b: false,
                        i: false,
                        u: false,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_program_has_painted_when_a_row_of_its_own_has_more_than_blanks() {
        assert!(!has_ink(&[]));
        assert!(!has_ink(&rows(&[&["   "], &[""]])));
        assert!(has_ink(&rows(&[&["  "], &["╭─ claude", "  "]])));
    }

    #[test]
    fn a_command_names_its_program_by_the_file_name_of_its_first_word() {
        assert_eq!(program_name("claude --resume 12"), "claude");
        assert_eq!(program_name("/usr/local/bin/claude"), "claude");
        assert_eq!(program_name("  nvim ."), "nvim");
        assert_eq!(program_name(""), "");
    }
}

#[cfg(test)]
mod loading_tests {
    use super::*;

    #[test]
    fn the_loading_says_what_starts_and_leaves_the_border_of_a_short_box_alone() {
        let mut g = Grid::new(30, 8);
        loading(&mut g, (0, 0, 30, 8), Phase::Starting, "claude", 0);
        let rows = g.rows_plain();
        assert!(
            rows.iter().any(|r| r.contains("⠋ iniciando claude…")),
            "{rows:?}"
        );
        assert!(rows.iter().any(|r| r.contains("^C cancela")), "{rows:?}");
        let mut g = Grid::new(30, 4);
        loading(&mut g, (0, 0, 30, 4), Phase::Starting, "claude", 110);
        let rows = g.rows_plain();
        assert!(
            rows[1].contains("⠙ iniciando claude…"),
            "the spinner moves: {rows:?}"
        );
        assert!(
            rows[3].trim().is_empty(),
            "the last row of a box is its border: {rows:?}"
        );
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn block(id: u64, out: usize, truncated: bool, full: bool, running: bool) -> Block {
        Block {
            id,
            cmd: format!("cmd {id}"),
            cwd: "/w".into(),
            started: 0,
            running,
            code: (!running).then_some(0),
            ms: (!running).then_some(5),
            truncated,
            full,
            out: (0..out)
                .map(|r| {
                    vec![Span {
                        t: format!("row {r} of {id}"),
                        fg: None,
                        bg: None,
                        b: false,
                        i: false,
                        u: false,
                    }]
                })
                .collect(),
        }
    }

    fn list() -> Vec<Block> {
        vec![
            block(1, 0, false, false, false),
            block(2, 5, true, false, false),
            block(3, 3, false, true, false),
            block(4, 12, false, false, false),
            block(5, 2, false, false, true),
        ]
    }

    #[test]
    fn the_height_of_a_list_is_the_lines_it_builds() {
        let blocks = list();
        assert_eq!(total_lines(&blocks), block_lines(&blocks, 0, 40).len());
        assert_eq!(total_lines(&[]), 0);
    }

    #[test]
    fn a_window_is_the_same_lines_as_the_whole_list_has_there() {
        let blocks = list();
        let whole = block_lines(&blocks, 1_000, 40);
        let text = |ls: &[Line]| -> Vec<(Option<u64>, String)> {
            ls.iter()
                .map(|l| {
                    (
                        l.block,
                        l.spans.iter().map(|s| s.0.clone()).collect::<String>(),
                    )
                })
                .collect()
        };
        for start in 0..=whole.len() + 2 {
            for rows in [0, 1, 3, 7, whole.len(), whole.len() + 5] {
                let want: Vec<Line> = whole.iter().skip(start).take(rows).cloned().collect();
                let got = window_lines(&blocks, 1_000, 40, start, rows);
                assert_eq!(text(&got), text(&want), "start {start}, rows {rows}");
            }
        }
    }
}
