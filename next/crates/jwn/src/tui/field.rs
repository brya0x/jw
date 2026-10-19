//! The field at the foot of a shell pane drawn as blocks, as the phone's composer is drawn
//! (prototype: `docs/prototypes/tui.html`, spec REQ-254 to REQ-258): a band of its own colour
//! with room around it, a row of chips, the line the shell holds (the green prompt, the text
//! or what to type, the send mark), growing as the text wraps. The shell owns the line; this
//! only draws what the pane node says it holds.

use contract::PaneInput;

use super::{Grid, Rgb, STOP, Style, clip, look};

/// Rows of text the field grows to before it shows the end of the line only.
pub const MAX_TEXT_ROWS: usize = 5;
/// Rows of the band that are not text: a free one over the chips, the chips, a free one between them and the text, and a free one under the text.
const AROUND: usize = 4;
/// The send mark, at the right of the first row of text.
pub const SEND: &str = "➤";
/// Columns the prompt takes before the text, and those kept at its right for the marks.
const PROMPT: usize = 2;
const MARKS: usize = 4;

/// How a field is drawn besides its text: the stop mark when something runs, the words of the
/// empty line, and the word the line starts with that has a colour of its own (a command).
pub struct Look<'a> {
    pub stop: Option<&'a str>,
    pub ask: &'a str,
    pub lead: Option<(String, Rgb)>,
}

/// The text of the field wrapped to its rows, and where the cursor is in them.
#[derive(Debug, PartialEq, Eq)]
pub struct Field {
    rows: Vec<String>,
    cursor: (usize, usize),
    /// The cells (row, column) that a selection covers.
    selected: Vec<(usize, usize)>,
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
            (Some(i), _) => wrap(&i.text, i.cursor as usize, width, None),
            (None, Some((line, col))) => Field {
                rows: vec![line.to_owned()],
                cursor: (0, col),
                selected: Vec::new(),
            },
            (None, None) => Field {
                rows: vec![String::new()],
                cursor: (0, 0),
                selected: Vec::new(),
            },
        }
    }

    /// The field of a line the person edits here, with the characters `sel` (from, to) selected.
    pub fn selecting(input: &PaneInput, sel: Option<(usize, usize)>, iw: usize) -> Field {
        wrap(&input.text, input.cursor as usize, text_width(iw), sel)
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
            lead: None,
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
        let Look { stop, ask, lead } = look;
        for r in 0..self.band_rows() {
            g.paint_bg(ix.saturating_sub(1), y + r, iw + 2, look::panel());
        }
        let mut x = ix + 1;
        for (text, colour) in chips {
            let room = (ix + iw).saturating_sub(x + 2);
            if room < 3 {
                break;
            }
            let pill = format!(" {} ", clip(text, room));
            g.put(x, y + 1, &pill, Style::fg(*colour).on(look::sel()));
            x += pill.chars().count() + 1;
        }
        let ground = look::panel();
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
        if let Some((word, colour)) = lead
            && self
                .rows
                .first()
                .is_some_and(|r| r.starts_with(word.as_str()))
        {
            let width = word.chars().count().min(text_width(iw));
            for x in tx..tx + width {
                if first < g.rows && x < g.cols {
                    let st = &mut g.cells[first * g.cols + x].1;
                    st.fg = Some(*colour);
                    st.bold = true;
                }
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
        if focused && at.0 < send_at(ix, iw) && at.1 < g.rows && at.0 < g.cols {
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
/// placed at its offset, and at most `MAX_TEXT_ROWS` rows kept: the last ones, or those that
/// hold the cursor when it is further up.
fn wrap(text: &str, cursor: usize, width: usize, sel: Option<(usize, usize)>) -> Field {
    let mut rows = vec![String::new()];
    let mut selected = Vec::new();
    let mut at = (0, 0);
    let mut placed = false;
    for (i, c) in text.chars().enumerate() {
        if c != '\n' && rows.last().is_some_and(|r| r.chars().count() >= width) {
            rows.push(String::new());
        }
        if i == cursor {
            at = (rows.len() - 1, rows.last().map_or(0, |r| r.chars().count()));
            placed = true;
        }
        if c == '\n' {
            rows.push(String::new());
        } else {
            let at = rows.len() - 1;
            let col = rows[at].chars().count();
            if sel.is_some_and(|(from, to)| (from..to).contains(&i)) {
                selected.push((at, col));
            }
            rows[at].push(c);
        }
    }
    if !placed {
        if rows.last().is_some_and(|r| r.chars().count() >= width) {
            rows.push(String::new());
        }
        at = (rows.len() - 1, rows.last().map_or(0, |r| r.chars().count()));
    }
    let start = rows.len().saturating_sub(MAX_TEXT_ROWS).min(at.0);
    let kept: Vec<String> = rows.into_iter().skip(start).take(MAX_TEXT_ROWS).collect();
    Field {
        rows: kept,
        cursor: (at.0 - start, at.1),
        selected: selected
            .into_iter()
            .filter_map(|(r, c)| r.checked_sub(start).map(|r| (r, c)))
            .filter(|(r, _)| *r < MAX_TEXT_ROWS)
            .collect(),
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

    #[test]
    fn a_selection_is_the_cells_of_its_characters_across_rows_and_line_breaks() {
        let w = text_width(30);
        let text = format!("{}ab\ncd", "x".repeat(w));
        let from = w;
        let f = Field::selecting(&input(&text, 0), Some((from, from + 4)), 30);
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
}
