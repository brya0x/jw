//! Selecting text in a pane (addendum 12): where the selection sits in the
//! pane's history, the text it covers, and the OSC 52 that copies it.

use std::io::Write;

use anyhow::{Result, bail};

use super::SCROLLBACK;
use crate::proto::PaneId;

/// The most text one copy carries; some terminals drop a longer OSC 52 (RISK-39).
const MAX_COPY: usize = 75_000;

/// A cell, with its row counted from the oldest line of the history, so it
/// stays on its text while the pane scrolls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pos {
    pub line: usize,
    pub col: u16,
}

pub struct Sel {
    pub pane: PaneId,
    pub anchor: Pos,
    pub head: Pos,
    /// The history's length when last looked at, to place rows on screen.
    len: usize,
    alt: bool,
}

/// How many lines of history the pane keeps. Reads it by clamping the
/// offset, which vt100 does not expose.
fn history_len<CB: vt100::Callbacks>(p: &mut vt100::Parser<CB>) -> usize {
    let screen = p.screen_mut();
    let was = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    screen.set_scrollback(was);
    len
}

/// The line under a visible row.
pub fn line_at<CB: vt100::Callbacks>(p: &mut vt100::Parser<CB>, row: u16) -> usize {
    let len = history_len(p);
    len - p.screen().scrollback() + usize::from(row)
}

impl Sel {
    pub fn new<CB: vt100::Callbacks>(
        pane: PaneId,
        p: &mut vt100::Parser<CB>,
        anchor: Pos,
        head: Pos,
    ) -> Self {
        Self {
            pane,
            anchor,
            head,
            len: history_len(p),
            alt: p.screen().alternate_screen(),
        }
    }

    pub fn ordered(&self) -> (Pos, Pos) {
        let (a, b) = (self.anchor, self.head);
        if (a.line, a.col) <= (b.line, b.col) {
            (a, b)
        } else {
            (b, a)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The line shown on the first row of a pane scrolled back by `back`.
    pub fn base(&self, back: usize) -> usize {
        self.len.saturating_sub(back)
    }

    /// The columns `[from, to)` the selection covers on `line`.
    pub fn cols_on(&self, line: usize, width: u16) -> Option<(u16, u16)> {
        let (s, e) = self.ordered();
        if line < s.line || line > e.line {
            return None;
        }
        let from = if line == s.line { s.col } else { 0 };
        let to = if line == e.line {
            e.col.saturating_add(1)
        } else {
            width
        };
        Some((from.min(width), to.min(width)))
    }

    /// Looks at the pane after its program printed. False when the history
    /// can no longer keep the selection on its text (REQ-131): the screen
    /// switched, the history shrank, or it is full and drops its oldest line.
    pub fn follow<CB: vt100::Callbacks>(&mut self, p: &mut vt100::Parser<CB>) -> bool {
        let len = history_len(p);
        let alt = p.screen().alternate_screen();
        let kept = alt == self.alt && len >= self.len && len < SCROLLBACK;
        self.len = len;
        self.alt = alt;
        kept
    }
}

/// A line's cells as text, and whether it wraps to the next line. A blank
/// cell is a space; the second half of a wide character is empty.
pub fn row_cells<CB: vt100::Callbacks>(
    p: &mut vt100::Parser<CB>,
    line: usize,
) -> Option<(Vec<String>, bool)> {
    let len = history_len(p);
    let (height, width) = p.screen().size();
    let (back, row) = if line < len {
        (len - line, 0)
    } else {
        (0, line - len)
    };
    let row = u16::try_from(row).ok().filter(|r| *r < height)?;
    let was = p.screen().scrollback();
    let screen = p.screen_mut();
    screen.set_scrollback(back);
    let cells = (0..width)
        .map(|col| match screen.cell(row, col) {
            Some(c) if c.is_wide_continuation() => String::new(),
            Some(c) if !c.contents().is_empty() => c.contents().to_string(),
            _ => " ".to_string(),
        })
        .collect();
    let wrapped = screen.row_wrapped(row);
    screen.set_scrollback(was);
    Some((cells, wrapped))
}

/// What the selection covers: trailing spaces cut from each row that does
/// not wrap, rows joined by a newline unless the row wraps (REQ-128).
pub fn text<CB: vt100::Callbacks>(p: &mut vt100::Parser<CB>, sel: &Sel) -> String {
    let (s, e) = sel.ordered();
    let width = p.screen().size().1;
    let mut out = String::new();
    for line in s.line..=e.line {
        let Some((cells, wrapped)) = row_cells(p, line) else {
            break;
        };
        let (from, to) = sel.cols_on(line, width).unwrap_or((0, 0));
        let mut row = cells[usize::from(from.min(to))..usize::from(to)].concat();
        if !wrapped {
            row.truncate(row.trim_end().len());
        }
        out.push_str(&row);
        if line < e.line && !wrapped {
            out.push('\n');
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || "_./~:-".contains(c)
}

/// The word under `col`: letters, digits and `_ . / ~ : -`. Just the cell
/// itself when it is not in a word.
pub fn word_at(cells: &[String], col: u16) -> (u16, u16) {
    let mut chars: Vec<char> = Vec::with_capacity(cells.len());
    for c in cells {
        let ch = c.chars().next().or(chars.last().copied()).unwrap_or(' ');
        chars.push(ch);
    }
    let i = usize::from(col);
    if !chars.get(i).copied().is_some_and(is_word) {
        return (col, col);
    }
    let mut from = i;
    while from > 0 && is_word(chars[from - 1]) {
        from -= 1;
    }
    let mut to = i;
    while to + 1 < chars.len() && is_word(chars[to + 1]) {
        to += 1;
    }
    (from as u16, to as u16)
}

/// A line up to its last non-blank cell.
pub fn line_span(cells: &[String]) -> (u16, u16) {
    let last = cells.iter().rposition(|c| !c.trim().is_empty());
    (0, last.unwrap_or(0) as u16)
}

fn base64(bytes: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]);
            } else {
                out.push(b'=');
            }
        }
    }
    out
}

/// The bytes that put `text` on the outer terminal's clipboard.
fn osc52(text: &str) -> Vec<u8> {
    let mut out = b"\x1b]52;c;".to_vec();
    out.extend(base64(text.as_bytes()));
    out.push(0x07);
    out
}

/// Copies `text` through the outer terminal; the characters copied.
pub fn copy(text: &str) -> Result<usize> {
    if text.len() > MAX_COPY {
        bail!(
            "selection too large to copy ({} KB, the limit is {} KB)",
            text.len() / 1000,
            MAX_COPY / 1000
        );
    }
    let mut out = std::io::stdout().lock();
    out.write_all(&osc52(text))?;
    out.flush()?;
    Ok(text.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(rows: u16, cols: u16, input: &str) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 100);
        p.process(input.as_bytes());
        p
    }

    fn sel(p: &mut vt100::Parser, a: (usize, u16), b: (usize, u16)) -> Sel {
        let pos = |(line, col)| Pos { line, col };
        Sel::new(1, p, pos(a), pos(b))
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), coded.as_bytes(), "{plain:?}");
        }
    }

    #[test]
    fn osc52_wraps_the_text_for_the_clipboard() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn the_history_is_counted_from_its_oldest_line() {
        let mut p = parser(2, 10, "a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(history_len(&mut p), 3);
        assert_eq!(line_at(&mut p, 0), 3);
        p.screen_mut().set_scrollback(2);
        assert_eq!(line_at(&mut p, 0), 1);
        assert_eq!(p.screen().scrollback(), 2);
    }

    #[test]
    fn text_spans_the_history_and_the_screen() {
        let mut p = parser(2, 10, "a\r\nb\r\nc\r\nd\r\ne");
        let s = sel(&mut p, (1, 0), (3, 0));
        assert_eq!(text(&mut p, &s), "b\nc\nd");
        assert_eq!(p.screen().scrollback(), 0, "the view stays where it was");
    }

    #[test]
    fn text_cuts_trailing_spaces_and_the_ends_of_the_range() {
        let mut p = parser(3, 20, "hello world\r\nsecond row\r\n");
        let s = sel(&mut p, (0, 6), (1, 5));
        assert_eq!(text(&mut p, &s), "world\nsecond");
    }

    #[test]
    fn a_soft_wrapped_row_joins_the_next_with_nothing() {
        let mut p = parser(3, 5, "abcdefgh\r\nnext");
        let s = sel(&mut p, (0, 0), (2, 3));
        assert_eq!(text(&mut p, &s), "abcdefgh\nnext");
        let within = sel(&mut p, (0, 0), (1, 2));
        assert_eq!(text(&mut p, &within), "abcdefgh");
    }

    #[test]
    fn a_wide_character_is_copied_once() {
        let mut p = parser(2, 10, "日本 go");
        let s = sel(&mut p, (0, 0), (0, 9));
        assert_eq!(text(&mut p, &s), "日本 go");
    }

    #[test]
    fn words_stop_at_spaces_and_keep_path_characters() {
        let cells = |s: &str| row_cells_of(s);
        let row = cells("cd ~/ws/jw-core ok");
        assert_eq!(word_at(&row, 5), (3, 14));
        assert_eq!(word_at(&row, 2), (2, 2));
        assert_eq!(word_at(&row, 16), (16, 17));
    }

    #[test]
    fn a_line_ends_at_its_last_character() {
        let row = row_cells_of("ls -la     ");
        assert_eq!(line_span(&row), (0, 5));
    }

    #[test]
    fn follow_keeps_the_selection_while_the_history_grows() {
        let mut p = parser(2, 10, "a\r\nb\r\nc");
        let mut s = sel(&mut p, (0, 0), (1, 0));
        p.process(b"\r\nd");
        assert!(s.follow(&mut p));
        assert_eq!(s.base(0), history_len(&mut p));
    }

    #[test]
    fn follow_drops_the_selection_when_the_screen_switches() {
        let mut p = parser(2, 10, "a\r\nb\r\nc");
        let mut s = sel(&mut p, (0, 0), (1, 0));
        p.process(b"\x1b[?1049h");
        assert!(!s.follow(&mut p));
    }

    #[test]
    fn cols_on_covers_the_middle_rows_whole() {
        let mut p = parser(4, 10, "");
        let s = sel(&mut p, (0, 4), (2, 1));
        assert_eq!(s.cols_on(0, 10), Some((4, 10)));
        assert_eq!(s.cols_on(1, 10), Some((0, 10)));
        assert_eq!(s.cols_on(2, 10), Some((0, 2)));
        assert_eq!(s.cols_on(3, 10), None);
    }

    fn row_cells_of(s: &str) -> Vec<String> {
        s.chars().map(|c| c.to_string()).collect()
    }
}
