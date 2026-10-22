//! The field at the foot of a shell pane drawn as blocks, as the phone's composer is drawn
//! (prototype: `docs/prototypes/tui.html`, spec REQ-254 to REQ-258): a band of its own colour
//! with room around it, a row of chips, the line the shell holds (the green prompt, the text
//! or what to type, the send mark), growing as the text wraps. The shell owns the line; this
//! only draws what the pane node says it holds.

use contract::PaneInput;

use super::{Grid, Rgb, STOP, Style, clip, look};

/// Rows of text a shell pane's field grows to before it shows a window of them that follows the cursor.
pub const MAX_TEXT_ROWS: usize = 5;
/// The composer's limit never goes below this (REQ-703)...
const MIN_LIMIT: usize = 5;
/// ...and leaves the list this many rows.
const LIST_KEEPS: usize = 4;
/// The foot row and the line of work with the blank row over it, which the limit counts whether or not a turn runs.
const FOOT_AND_WORK: usize = 3;
/// Rows of the band that are not text: a free one over the chips, the chips, a free one between them and the text, and a free one under the text.
const AROUND: usize = 4;
/// The send mark, at the right of the first row of text.
pub const SEND: &str = "➤";
/// Columns the prompt takes before the text, and those kept at its right for the marks.
pub const PROMPT: usize = 2;
const MARKS: usize = 4;

/// How a field is drawn besides its text: the stop mark when something runs and the words of the
/// empty line.
pub struct Look<'a> {
    pub stop: Option<&'a str>,
    pub ask: &'a str,
}

/// A run of characters of the text, `from` to `to`, that has a colour of its own: a command, or
/// the `!` of a shell line.
pub type Mark = (usize, usize, Rgb);

/// Where each row of `chars` begins when it is wrapped to `width` as `wrap` does; with the cursor
/// at the end of a last row that is full, one more row, empty, is where it is.
pub fn row_starts(chars: &[char], width: usize, cursor: usize) -> Vec<usize> {
    let mut starts = vec![0];
    let mut len = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            starts.push(i + 1);
            len = 0;
            continue;
        }
        if len >= width {
            starts.push(i);
            len = 0;
        }
        len += 1;
    }
    if cursor >= chars.len() && len >= width {
        starts.push(chars.len());
    }
    starts
}

/// The rows of text a field of a pane `ih` rows high grows to before it scrolls (REQ-703): a
/// third of the pane, at least five, and at most what leaves the list four rows; never below one.
pub fn limit(ih: usize) -> usize {
    let room = ih.saturating_sub(FOOT_AND_WORK + AROUND + LIST_KEEPS);
    (ih / 3).max(MIN_LIMIT).min(room).max(1)
}

/// Where a window of `limit` rows begins in a text of `rows` rows whose cursor is on row `cursor`:
/// the `offset` it had, moved the least that puts the cursor in view (REQ-704).
pub fn window_start(rows: usize, limit: usize, cursor: usize, offset: usize) -> usize {
    let last = rows.saturating_sub(limit);
    let mut at = offset.min(last);
    if cursor < at {
        at = cursor;
    } else if cursor >= at + limit {
        at = cursor + 1 - limit;
    }
    at.min(last)
}

/// The thumb of the bar, `(from, length)` in a bar `v` rows long, for a window of `v` rows at
/// `offset` in a text of `n` rows (REQ-706). It touches the top only at offset 0 and the bottom
/// only at the last offset, where the bar has two rows to spare.
pub fn thumb(n: usize, v: usize, offset: usize) -> (usize, usize) {
    let len = ((v * v) as f64 / n as f64).round().max(1.0) as usize;
    let travel = v.saturating_sub(len);
    let mut at = if n <= v {
        0
    } else {
        (offset as f64 / (n - v) as f64 * travel as f64).round() as usize
    };
    if travel >= 2 {
        if offset > 0 {
            at = at.max(1);
        }
        if offset + v < n {
            at = at.min(travel - 1);
        }
    }
    (at, len)
}

/// The rows of the text a field shows when the text has more than fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// How many rows it shows at most.
    pub limit: usize,
    /// Where it was: with it the window moves the least that shows the cursor (the composer's,
    /// which remembers it); without, it begins as far down as the cursor allows (a shell pane's).
    pub offset: Option<usize>,
    /// Whether a text that has more rows than the window says so: the bar and the count (REQ-706).
    pub bar: bool,
    /// The row the window begins at when the person looked elsewhere with the wheel (REQ-707):
    /// it wins over the cursor, which may then be out of view.
    pub pin: Option<usize>,
}

impl Window {
    /// The shell pane's: five rows, following the cursor (REQ-256).
    pub const FOLLOW: Window = Window {
        limit: MAX_TEXT_ROWS,
        offset: None,
        bar: false,
        pin: None,
    };
}

/// The text of the field wrapped to its rows, and where the cursor is in them.
#[derive(Debug, PartialEq, Eq)]
pub struct Field {
    rows: Vec<String>,
    /// The row of the whole text the first of `rows` is, and how many rows the whole text has.
    first: usize,
    total: usize,
    /// The bar and the count are drawn when the text has more rows than are shown.
    bar: bool,
    /// The cursor is in one of the rows shown.
    seen: bool,
    cursor: (usize, usize),
    /// The cells (row, column) that a selection covers.
    selected: Vec<(usize, usize)>,
    /// The cells (row, column) of the marks, with their colour.
    lit: Vec<(usize, usize, Rgb)>,
    /// The character of the text each row begins with, for a click to say which one it is on.
    starts: Vec<usize>,
}

/// Columns of text a pane `iw` wide holds in a row of the field.
pub fn text_width(iw: usize) -> usize {
    iw.saturating_sub(PROMPT + MARKS + 2).max(8)
}

impl Field {
    /// The row of the band, counted from its top, the first row of text is on.
    pub const FIRST_TEXT: usize = 3;

    /// The field of a pane `iw` wide. `input` is what the shell holds at its prompt; without it
    /// (an older daemon, a shell that does not say where its prompt ends) `screen` is the line
    /// of the pane's screen the cursor is on, with its column; with neither, nothing is shown.
    pub fn new(input: Option<&PaneInput>, screen: Option<(&str, usize)>, iw: usize) -> Field {
        let width = text_width(iw);
        match (input, screen) {
            (Some(i), _) => wrap(&i.text, i.cursor as usize, width, None, &[], Window::FOLLOW),
            (None, Some((line, col))) => Field {
                rows: vec![line.to_owned()],
                first: 0,
                total: 1,
                bar: false,
                seen: true,
                cursor: (0, col),
                selected: Vec::new(),
                lit: Vec::new(),
                starts: Vec::new(),
            },
            (None, None) => Field {
                rows: vec![String::new()],
                first: 0,
                total: 1,
                bar: false,
                seen: true,
                cursor: (0, 0),
                selected: Vec::new(),
                lit: Vec::new(),
                starts: Vec::new(),
            },
        }
    }

    /// The field of a line the person edits here, with the characters `sel` (from, to) selected
    /// and the `marks` in their colours, showing the rows `window` says.
    pub fn selecting(
        input: &PaneInput,
        sel: Option<(usize, usize)>,
        marks: &[Mark],
        iw: usize,
        window: Window,
    ) -> Field {
        wrap(
            &input.text,
            input.cursor as usize,
            text_width(iw),
            sel,
            marks,
            window,
        )
    }

    /// How many rows of text are shown, and how many the whole text has.
    pub fn shown_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn total_rows(&self) -> usize {
        self.total
    }

    /// The row of the whole text that the first row shown is: the offset a window that
    /// remembers keeps for the next frame.
    pub fn first_row(&self) -> usize {
        self.first
    }

    /// The character of the text that is at `col` of the shown row `row`: none past the end of the row.
    pub fn index_at(&self, row: usize, col: usize) -> Option<usize> {
        let text = self.rows.get(row)?;
        (col < text.chars().count()).then(|| self.starts.get(row).map(|s| s + col))?
    }

    /// Nothing is typed (or nothing can be shown), so the row says what to type.
    fn is_empty(&self) -> bool {
        self.rows.iter().all(String::is_empty)
    }

    /// Rows of the band: the text and what is around it.
    pub fn band_rows(&self) -> usize {
        AROUND + self.rows.len()
    }

    /// Draw the field in the band whose top row is `y`, at the pane's inner `x` and `iw`. The chips
    /// are pills over it; `running` puts the stop mark beside the send mark.
    pub fn draw(
        &self,
        g: &mut Grid,
        at: (usize, usize, usize),
        chips: &[(String, Rgb)],
        running: bool,
        focused: bool,
    ) {
        let ask = if running {
            "va al programa"
        } else {
            "escribe un comando"
        };
        let look = Look {
            stop: running.then_some(STOP),
            ask,
        };
        self.draw_with(g, at, chips, focused, &look);
    }

    /// The same band with the look the caller gives it.
    pub fn draw_with(
        &self,
        g: &mut Grid,
        (ix, y, iw): (usize, usize, usize),
        chips: &[(String, Rgb)],
        focused: bool,
        look: &Look,
    ) {
        let Look { stop, ask } = look;
        for r in 0..self.band_rows() {
            g.paint_bg(ix.saturating_sub(1), y + r, iw + 2, look::panel());
        }
        let more = self.bar && self.total > self.rows.len();
        let count = format!(
            "↑{} ↓{}",
            self.first,
            self.total - self.first - self.rows.len()
        );
        // The bar is in the last column of the box and the count ends before it; the chips stop short of the count.
        let count_at = (ix + iw).saturating_sub(1 + count.chars().count());
        let chips_end = if more {
            count_at.saturating_sub(1)
        } else {
            ix + iw
        };
        let mut x = ix + 1;
        for (text, colour) in chips {
            let room = chips_end.saturating_sub(x + 2);
            if room < 3 {
                break;
            }
            let pill = format!(" {} ", clip(text, room));
            g.put(x, y + 1, &pill, Style::fg(*colour).on(look::sel()));
            x += pill.chars().count() + 1;
        }
        let ground = look::panel();
        if more {
            g.put(
                count_at,
                y + 1,
                &count,
                Style::fg(look::yellow()).on(ground),
            );
        }
        let tx = ix + 1 + PROMPT;
        let first = y + Self::FIRST_TEXT;
        g.put(
            ix + 1,
            first,
            "❯",
            Style::fg(look::green()).bold().on(ground),
        );
        for (k, row) in self.rows.iter().enumerate() {
            g.put(
                tx,
                first + k,
                &clip(row, text_width(iw)),
                Style::fg(look::fg()).on(ground),
            );
        }
        if more {
            let (from, len) = thumb(self.total, self.rows.len(), self.first);
            for k in 0..self.rows.len() {
                let (mark, colour) = if (from..from + len).contains(&k) {
                    ("┃", look::blue())
                } else {
                    ("│", look::dim())
                };
                g.put(ix + iw - 1, first + k, mark, Style::fg(colour).on(ground));
            }
        }
        for &(row, col, colour) in &self.lit {
            let (x, y) = (tx + col, first + row);
            if x < send_at(ix, iw) && y < g.rows && x < g.cols {
                let st = &mut g.cells[y * g.cols + x].1;
                st.fg = Some(colour);
                st.bold = true;
            }
        }
        if self.is_empty() {
            g.put(
                tx,
                first,
                &clip(ask, stop_at(ix, iw).saturating_sub(tx + 1)),
                Style::fg(look::dim()).on(ground),
            );
        }
        if let Some(stop) = stop {
            g.put(
                stop_at(ix, iw),
                first,
                stop,
                Style::fg(look::red()).bold().on(ground),
            );
        }
        g.put(
            send_at(ix, iw),
            first,
            SEND,
            Style::fg(look::blue()).bold().on(ground),
        );
        for &(row, col) in &self.selected {
            let (x, y) = (tx + col, first + row);
            if x < send_at(ix, iw) && y < g.rows && x < g.cols {
                g.cells[y * g.cols + x].1.bg = Some(look::sel());
            }
        }
        let (row, col) = self.cursor;
        let at = (tx + col, first + row);
        if focused && self.seen && at.0 < send_at(ix, iw) && at.1 < g.rows && at.0 < g.cols {
            g.cells[at.1 * g.cols + at.0].1.reverse = true;
        }
    }
}

/// Where the send mark is, and the stop mark beside it, for a pane whose inner box is at `ix`
/// and `iw` wide.
pub fn send_at(ix: usize, iw: usize) -> usize {
    (ix + iw).saturating_sub(3)
}

pub fn stop_at(ix: usize, iw: usize) -> usize {
    send_at(ix, iw).saturating_sub(STOP.chars().count() + 2)
}

/// `text` broken into rows of `width` characters (and at its own line breaks), the cursor
/// placed at its offset, and at most `window.limit` rows kept: those the window says, or, with no
/// offset, the last ones, or those that hold the cursor when it is further up.
fn wrap(
    text: &str,
    cursor: usize,
    width: usize,
    sel: Option<(usize, usize)>,
    marks: &[Mark],
    window: Window,
) -> Field {
    let mut rows = vec![String::new()];
    let mut starts = vec![0];
    let mut selected = Vec::new();
    let mut lit = Vec::new();
    let mut at = (0, 0);
    let mut placed = false;
    for (i, c) in text.chars().enumerate() {
        if c != '\n' && rows.last().is_some_and(|r| r.chars().count() >= width) {
            rows.push(String::new());
            starts.push(i);
        }
        if i == cursor {
            at = (rows.len() - 1, rows.last().map_or(0, |r| r.chars().count()));
            placed = true;
        }
        if c == '\n' {
            rows.push(String::new());
            starts.push(i + 1);
        } else {
            let at = rows.len() - 1;
            let col = rows[at].chars().count();
            if sel.is_some_and(|(from, to)| (from..to).contains(&i)) {
                selected.push((at, col));
            }
            if let Some(&(_, _, colour)) =
                marks.iter().find(|(from, to, _)| (*from..*to).contains(&i))
            {
                lit.push((at, col, colour));
            }
            rows[at].push(c);
        }
    }
    if !placed {
        if rows.last().is_some_and(|r| r.chars().count() >= width) {
            rows.push(String::new());
            starts.push(text.chars().count());
        }
        at = (rows.len() - 1, rows.last().map_or(0, |r| r.chars().count()));
    }
    let total = rows.len();
    let limit = window.limit;
    let start = match (window.pin, window.offset) {
        (Some(pin), _) => pin.min(total.saturating_sub(limit)),
        (None, Some(offset)) => window_start(total, limit, at.0, offset),
        (None, None) => total.saturating_sub(limit).min(at.0),
    };
    let kept: Vec<String> = rows.into_iter().skip(start).take(limit).collect();
    Field {
        rows: kept,
        first: start,
        total,
        bar: window.bar,
        seen: (start..start + limit).contains(&at.0),
        cursor: (at.0.saturating_sub(start), at.1),
        selected: selected
            .into_iter()
            .filter_map(|(r, c)| r.checked_sub(start).map(|r| (r, c)))
            .filter(|(r, _)| *r < limit)
            .collect(),
        lit: lit
            .into_iter()
            .filter_map(|(r, c, k)| r.checked_sub(start).map(|r| (r, c, k)))
            .filter(|(r, ..)| *r < limit)
            .collect(),
        starts: starts.into_iter().skip(start).take(limit).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str, cursor: u32) -> PaneInput {
        PaneInput {
            text: text.into(),
            cursor,
        }
    }

    #[test]
    fn a_short_line_is_one_row_with_the_cursor_after_it() {
        let f = Field::new(Some(&input("git status", 10)), None, 60);
        assert_eq!(f.rows, ["git status"]);
        assert_eq!((f.cursor, f.is_empty(), f.band_rows()), ((0, 10), false, 5));
    }

    #[test]
    fn nothing_typed_is_empty_and_says_what_to_type() {
        let f = Field::new(Some(&input("", 0)), None, 60);
        assert_eq!((f.rows.len(), f.cursor, f.is_empty()), (1, (0, 0), true));
        assert!(
            Field::new(None, None, 60).is_empty(),
            "a running command shows no line"
        );
    }

    #[test]
    fn a_line_longer_than_the_row_wraps_and_the_band_grows() {
        let w = text_width(30);
        let text = "x".repeat(w * 2 + 3);
        let f = Field::new(Some(&input(&text, (w * 2 + 3) as u32)), None, 30);
        assert_eq!(
            f.rows.iter().map(|r| r.chars().count()).collect::<Vec<_>>(),
            [w, w, 3]
        );
        assert_eq!(f.cursor, (2, 3));
        assert_eq!(f.band_rows(), 7);
    }

    #[test]
    fn a_line_that_fills_its_last_row_puts_the_cursor_on_a_new_one() {
        let w = text_width(30);
        let f = Field::new(Some(&input(&"x".repeat(w), w as u32)), None, 30);
        assert_eq!((f.rows.len(), f.cursor), (2, (1, 0)));
    }

    #[test]
    fn the_cursor_in_the_middle_of_a_wrapped_line_is_where_it_is() {
        let w = text_width(30);
        let f = Field::new(Some(&input(&"x".repeat(w + 4), 2)), None, 30);
        assert_eq!(f.cursor, (0, 2));
        let f = Field::new(Some(&input(&"x".repeat(w + 4), (w + 1) as u32)), None, 30);
        assert_eq!(f.cursor, (1, 1));
    }

    #[test]
    fn a_line_break_of_the_text_starts_a_row() {
        let f = Field::new(Some(&input("ab\ncd", 5)), None, 60);
        assert_eq!(f.rows, ["ab", "cd"]);
        assert_eq!(f.cursor, (1, 2));
    }

    #[test]
    fn more_than_five_rows_keep_the_last_ones_or_those_with_the_cursor() {
        let w = text_width(30);
        let text = "x".repeat(w * 8);
        let end = Field::new(Some(&input(&text, (w * 8) as u32)), None, 30);
        assert_eq!(end.rows.len(), MAX_TEXT_ROWS);
        assert_eq!(
            end.cursor.0,
            MAX_TEXT_ROWS - 1,
            "the cursor is on the last row kept"
        );
        let top = Field::new(Some(&input(&text, 1)), None, 30);
        assert_eq!((top.rows.len(), top.cursor), (MAX_TEXT_ROWS, (0, 1)));
    }

    /// REQ-703: a third of the pane, at least five, and what leaves the list four rows.
    #[test]
    fn the_limit_is_a_third_of_the_pane_at_least_five_and_leaves_the_list_four_rows() {
        for (ih, want) in [
            (36, 12),
            (30, 10),
            (24, 8),
            (18, 6),
            (16, 5),
            (15, 4),
            (12, 1),
            (11, 1),
            (8, 1),
            (0, 1),
        ] {
            assert_eq!(limit(ih), want, "a pane {ih} rows high");
        }
        for ih in 12..80 {
            let (foot, work) = (1, 2);
            assert!(
                ih >= foot + AROUND + limit(ih) + work + LIST_KEEPS,
                "the list keeps four rows at {ih}"
            );
        }
    }

    /// REQ-704: the offset moves the least that shows the cursor, and stays inside the text.
    #[test]
    fn the_window_moves_the_least_that_shows_the_cursor_and_stays_inside_the_text() {
        // rows 0..14, a window of 8
        assert_eq!(window_start(14, 8, 3, 0), 0, "inside: it stays");
        assert_eq!(
            window_start(14, 8, 7, 0),
            0,
            "the last row of the window: it stays"
        );
        assert_eq!(window_start(14, 8, 8, 0), 1, "one below: one row");
        assert_eq!(
            window_start(14, 8, 13, 0),
            6,
            "far below: the cursor is the last row"
        );
        assert_eq!(
            window_start(14, 8, 2, 6),
            2,
            "above: the cursor is the first row"
        );
        assert_eq!(
            window_start(14, 8, 6, 6),
            6,
            "the first row of the window: it stays"
        );
        assert_eq!(window_start(14, 8, 5, 6), 5, "one above: one row");
        assert_eq!(window_start(14, 8, 10, 6), 6, "inside after a move down");
        assert_eq!(window_start(6, 8, 5, 3), 0, "a text that fits begins at 0");
        assert_eq!(
            window_start(10, 8, 9, 6),
            2,
            "an offset past the end is clamped"
        );
        assert_eq!(window_start(10, 4, 0, 99), 0, "and the cursor still shows");
        assert_eq!(window_start(1, 1, 0, 0), 0);
    }

    /// REQ-704: with an offset the cursor walks inside the window and the text does not slide.
    #[test]
    fn a_window_with_an_offset_keeps_its_rows_while_the_cursor_moves_inside_it() {
        let w = text_width(30);
        let text = "x".repeat(w * 14);
        let at = |cursor: usize, offset: usize| {
            Field::selecting(
                &input(&text, cursor as u32),
                None,
                &[],
                30,
                Window {
                    limit: 8,
                    offset: Some(offset),
                    bar: true,
                    pin: None,
                },
            )
        };
        let top = at(0, 0);
        assert_eq!(
            (top.rows.len(), top.first_row(), top.total, top.cursor),
            (8, 0, 14, (0, 0))
        );
        let down = at(w * 7 + 2, 0);
        assert_eq!(
            (down.first_row(), down.cursor),
            (0, (7, 2)),
            "the cursor reaches the last row and the text has not moved"
        );
        let past = at(w * 8, 0);
        assert_eq!(
            (past.first_row(), past.cursor),
            (1, (7, 0)),
            "one row, at the edge"
        );
        // at the end of a full last row the cursor is on a new, empty one: 15 rows
        let end = at(w * 14, 0);
        assert_eq!((end.total, end.first_row()), (15, 7));
        assert_eq!(
            end.starts.len(),
            8,
            "the starts are those of the rows shown"
        );
        assert_eq!(end.starts[0], w * 7);
        let back = at(w * 3, 6);
        assert_eq!((back.first_row(), back.cursor), (3, (0, 0)));
    }

    /// REQ-708: the shell's field keeps five rows and follows the cursor, the same text with no offset.
    #[test]
    fn a_field_with_no_offset_follows_the_cursor_as_the_shells_does() {
        let w = text_width(30);
        let text = "x".repeat(w * 14);
        let mid = Field::selecting(&input(&text, (w * 7) as u32), None, &[], 30, Window::FOLLOW);
        assert_eq!((mid.rows.len(), mid.first_row(), mid.cursor.0), (5, 7, 0));
    }

    /// REQ-706: the thumb's length, and its place that touches an end of the bar only at the end of the text.
    #[test]
    fn the_thumb_is_as_long_as_the_window_is_of_the_text_and_touches_an_end_only_at_the_end() {
        // 14 rows, a window of 8: the thumb is 5 of 8 and travels 3
        assert_eq!(thumb(14, 8, 0), (0, 5));
        assert_eq!(thumb(14, 8, 1), (1, 5), "off the top as soon as it moves");
        assert_eq!(
            thumb(14, 8, 5),
            (2, 5),
            "and short of the bottom until the last offset"
        );
        assert_eq!(thumb(14, 8, 6), (3, 5));
        // a long text: the thumb is one row and still shows which end it is at
        assert_eq!(thumb(100, 8, 0), (0, 1));
        assert_eq!(thumb(100, 8, 1), (1, 1));
        assert_eq!(thumb(100, 8, 91), (6, 1));
        assert_eq!(thumb(100, 8, 92), (7, 1));
        // a bar with no room to spare has nothing to keep off the ends
        assert_eq!(thumb(9, 2, 0), (0, 1));
        assert_eq!(thumb(9, 2, 7), (1, 1));
        assert_eq!(thumb(1, 1, 0), (0, 1));
    }

    fn drawn(f: &Field, chips: &[(String, Rgb)]) -> Vec<String> {
        let mut g = Grid::new(50, 14);
        f.draw_with(
            &mut g,
            (2, 0, 44),
            chips,
            true,
            &Look {
                stop: None,
                ask: "",
            },
        );
        g.rows_plain()
    }

    fn long_text(rows: usize, cursor: usize, offset: usize, bar: bool) -> Field {
        let w = text_width(44);
        let text = "x".repeat(w * rows);
        Field::selecting(
            &input(&text, (w * cursor) as u32),
            None,
            &[],
            44,
            Window {
                limit: 8,
                offset: Some(offset),
                bar,
                pin: None,
            },
        )
    }

    /// REQ-706: a bar at the right of the window's rows and the count at the end of the chips' row, one column before it.
    #[test]
    fn a_text_with_more_rows_than_the_window_draws_a_bar_and_a_count() {
        let rows = drawn(&long_text(14, 3, 0, true), &[]);
        let bar: Vec<char> = (3..11).map(|y| rows[y].chars().nth(45).unwrap()).collect();
        assert_eq!(
            bar,
            ['┃', '┃', '┃', '┃', '┃', '│', '│', '│'],
            "the thumb at the top"
        );
        assert!(
            rows[1].ends_with("↑0 ↓6"),
            "the count ends a column before the bar: {:?}",
            rows[1]
        );
        assert_eq!(
            rows[1].chars().count(),
            45,
            "and nothing is at the bar's column on that row"
        );
        let rows = drawn(&long_text(14, 13, 6, true), &[]);
        let bar: Vec<char> = (3..11).map(|y| rows[y].chars().nth(45).unwrap()).collect();
        assert_eq!(
            bar,
            ['│', '│', '│', '┃', '┃', '┃', '┃', '┃'],
            "and at the bottom"
        );
        assert!(rows[1].contains("↑6 ↓0"), "{:?}", rows[1]);
    }

    /// REQ-706, REQ-708: nothing is drawn when everything fits, and a field with no bar says nothing.
    #[test]
    fn nothing_is_drawn_when_the_text_fits_or_the_field_is_a_shells() {
        for f in [long_text(8, 3, 0, true), long_text(14, 3, 0, false)] {
            let rows = drawn(&f, &[]);
            assert!(
                rows.iter()
                    .all(|r| !r.contains('↑') && !r.contains('┃') && !r.contains('│'))
            );
        }
    }

    /// REQ-706, RISK-86: the chips are cut before the count and never drawn under it.
    #[test]
    fn the_chips_stop_short_of_the_count() {
        let chips = [(
            "⎇ a-branch-name-that-is-far-too-long-for-the-row-it-is-on".to_owned(),
            (1, 2, 3),
        )];
        let cells = |f: &Field| drawn(f, &chips)[1].chars().collect::<Vec<_>>();
        // the box is at 2 and 44 wide: the bar is in column 45, the count in 40 to 44
        let with = cells(&long_text(14, 3, 0, true));
        assert_eq!(with[40..45].iter().collect::<String>(), "↑0 ↓6");
        assert_eq!(
            (with[38], with[39]),
            (' ', ' '),
            "the pill's own space and one more between it and the count"
        );
        assert_ne!(with[37], ' ', "the chip has all the room that is left");
        let without = cells(&long_text(3, 1, 0, true));
        assert_ne!(
            without[41], ' ',
            "with no count the chip goes on under where it was"
        );
    }

    #[test]
    fn a_selection_is_the_cells_of_its_characters_across_rows_and_line_breaks() {
        let w = text_width(30);
        let text = format!("{}ab\ncd", "x".repeat(w));
        let from = w;
        let f = Field::selecting(
            &input(&text, 0),
            Some((from, from + 4)),
            &[],
            30,
            Window::FOLLOW,
        );
        assert_eq!(f.selected, [(1, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn without_the_daemons_input_the_screens_line_is_shown_as_it_is() {
        let f = Field::new(None, Some(("❯ ls -la", 8)), 60);
        assert_eq!(
            (f.rows.as_slice(), f.cursor, f.is_empty()),
            (&["❯ ls -la".to_owned()][..], (0, 8), false)
        );
    }

    /// The editor counts the rows of the line as the field draws them (REQ-680).
    #[test]
    fn the_rows_the_editor_counts_are_the_rows_that_are_drawn() {
        for (text, width) in [
            ("", 5),
            ("abc", 5),
            ("abcde", 5),
            ("abcdef", 5),
            ("abcde\nfg", 5),
            ("abcde\n", 5),
            ("\n\nab", 5),
            ("año ñandú\nxy", 4),
            ("abcdefghij", 5),
        ] {
            let chars: Vec<char> = text.chars().collect();
            for cursor in 0..=chars.len() {
                let drawn = wrap(
                    text,
                    cursor,
                    width,
                    None,
                    &[],
                    Window {
                        limit: usize::MAX,
                        offset: None,
                        bar: false,
                        pin: None,
                    },
                );
                assert_eq!(
                    row_starts(&chars, width, cursor),
                    drawn.starts,
                    "{text:?} at {width}, cursor {cursor}"
                );
            }
        }
    }
}
