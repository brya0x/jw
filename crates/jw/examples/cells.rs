//! Prints the screen a byte stream leaves, cell by cell, in the form the phone app's
//! emulator (`android/term`) is tested against: `cells.rs` is the oracle, the same vt100
//! the daemon runs. `android/term/golden/regen.sh` makes the expected files with it.
//!
//! `cells [rows cols]`, the stream on stdin.

use std::io::Read;
use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode};

fn color(c: Color) -> String {
    match c {
        Color::Default => "D".into(),
        Color::Idx(i) => format!("I{i}"),
        Color::Rgb(r, g, b) => format!("R{r},{g},{b}"),
    }
}

fn main() {
    let a: Vec<u16> = std::env::args()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let (rows, cols) = (
        a.first().copied().unwrap_or(24),
        a.get(1).copied().unwrap_or(80),
    );
    let mut b = Vec::new();
    std::io::stdin().read_to_end(&mut b).unwrap();
    let mut p = vt100::Parser::new(rows, cols, 0);
    p.process(&b);
    let s = p.screen();
    let (cr, cc) = s.cursor_position();
    println!("size {rows} {cols}");
    println!("cursor {cr} {cc} hidden={}", s.hide_cursor() as u8);
    let mode = match s.mouse_protocol_mode() {
        MouseProtocolMode::None => "none",
        MouseProtocolMode::Press => "press",
        MouseProtocolMode::PressRelease => "press-release",
        MouseProtocolMode::ButtonMotion => "button-motion",
        MouseProtocolMode::AnyMotion => "any-motion",
    };
    let enc = match s.mouse_protocol_encoding() {
        MouseProtocolEncoding::Default => "default",
        MouseProtocolEncoding::Utf8 => "utf8",
        MouseProtocolEncoding::Sgr => "sgr",
    };
    println!(
        "modes alt={} appcursor={} appkeypad={} paste={} mouse={mode}/{enc}",
        s.alternate_screen() as u8,
        s.application_cursor() as u8,
        s.application_keypad() as u8,
        s.bracketed_paste() as u8,
    );
    let mut attrs = Vec::new();
    for r in 0..rows {
        let mut line = String::new();
        for c in 0..cols {
            let cell = s.cell(r, c).unwrap();
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                line.push_str(cell.contents());
            } else {
                line.push(' ');
            }
            let mut f = String::new();
            for (on, ch) in [
                (cell.bold(), 'b'),
                (cell.dim(), 'd'),
                (cell.italic(), 'i'),
                (cell.underline(), 'u'),
                (cell.inverse(), 'v'),
            ] {
                if on {
                    f.push(ch);
                }
            }
            let (fg, bg) = (color(cell.fgcolor()), color(cell.bgcolor()));
            if f.is_empty() && fg == "D" && bg == "D" {
                continue;
            }
            if f.is_empty() {
                f.push('-');
            }
            attrs.push(format!("cell {r} {c} fg={fg} bg={bg} f={f}"));
        }
        println!("row {r} |{}|", line.trim_end());
    }
    for l in attrs {
        println!("{l}");
    }
}
