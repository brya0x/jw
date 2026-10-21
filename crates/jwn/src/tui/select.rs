//! Selecting text with the mouse (spec REQ-304 to REQ-310): a drag in the list of blocks of a pane
//! covers the commands and the output from where it began to where it ended, line after line, and
//! what it covers is lit and copied when the button is let go. This is what is selected and what
//! it says; where the mouse is and what is drawn are the client's.

use super::shell::{Kind, Line, window_lines};
use contract::Block;

/// A place in the list of a pane: a line, counted from its first, and a column of its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

/// What a drag covers, in the list of the pane `pane`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub pane: String,
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    /// The first end and the last, whichever way the drag went.
    pub fn ends(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The characters of a line `len` long that are covered, from and up to: the first line from where
    /// the drag began, the last up to and including where it ended, the lines between whole.
    pub fn on_line(&self, line: usize, len: usize) -> Option<(usize, usize)> {
        let (a, b) = self.ends();
        if line < a.line || line > b.line {
            return None;
        }
        let from = if line == a.line { a.col } else { 0 };
        let to = if line == b.line {
            (b.col + 1).min(len)
        } else {
            len
        };
        (from < to).then_some((from, to))
    }
}

/// While a drag is held past the edge of a list, it scrolls a line this often (ms).
pub const EDGE_MS: u64 = 60;

/// A drag with the button still down, over a list whose lines are numbered (REQ-462): where it
/// began and whether the pointer ever moved; what each list does with it is its own.
#[derive(Debug)]
pub struct Pull {
    /// The cell it began on: a press that never moved is the click it was.
    pub start: (usize, usize),
    pub anchor: Point,
    pub moved: bool,
    /// Past the top (`true`) or the bottom of the list, since when: the list scrolls while it is.
    scrolling: Option<(bool, u64)>,
}

impl Pull {
    pub fn new(start: (usize, usize), anchor: Point) -> Pull {
        Pull {
            start,
            anchor,
            moved: false,
            scrolling: None,
        }
    }

    /// The pointer is at `head`, past the top (`Some(true)`) or the bottom of the list or inside it.
    pub fn to(&mut self, head: Point, edge: Option<bool>, now_ms: u64) {
        self.moved |= head != self.anchor;
        self.scrolling = match (edge, self.scrolling) {
            (Some(up), Some((was, since))) if up == was => Some((up, since)),
            (Some(up), _) => Some((up, now_ms)),
            (None, _) => None,
        };
    }

    /// The way the list is to scroll a line now (`true`: up), when the drag is held past its edge
    /// and it is time.
    pub fn due(&self, now_ms: u64) -> Option<bool> {
        match self.scrolling {
            Some((up, since)) if now_ms >= since + EDGE_MS => Some(up),
            _ => None,
        }
    }

    /// The list scrolled a line at `now_ms`.
    pub fn scrolled(&mut self, now_ms: u64) {
        if let Some((_, since)) = self.scrolling.as_mut() {
            *since = now_ms;
        }
    }
}

/// What a double click takes as part of a word: letters and digits of any alphabet (`ñ`, `é`) and
/// what a path, an option or a name holds (REQ-637).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~')
}

/// The word under column `col` of `text`: where it begins and where it ends, both included. A
/// space or a mark that no word holds is none, and a full stop or a dash that ends a word is not
/// part of it (`palabra.` is `palabra`).
pub fn word_at(text: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if !chars.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let from = chars[..col]
        .iter()
        .rposition(|c| !is_word(*c))
        .map_or(0, |i| i + 1);
    let mut to = col
        + chars[col..]
            .iter()
            .position(|c| !is_word(*c))
            .unwrap_or(chars.len() - col)
        - 1;
    while to > from && matches!(chars[to], '.' | '-') {
        to -= 1;
    }
    (col <= to).then_some((from, to))
}

/// The word under column `col` of `text`, a line `line` of the list, as the two points that select
/// it (REQ-637).
pub fn word_span(line: usize, text: &str, col: usize) -> Option<(Point, Point)> {
    let (from, to) = word_at(text, col)?;
    Some((Point { line, col: from }, Point { line, col: to }))
}

/// The text of a line of the list: what its spans say.
pub fn text_of(line: &Line) -> String {
    line.spans.iter().map(|(t, _)| t.as_str()).collect()
}

/// How many characters the text of a line has.
pub fn len_of(line: &Line) -> usize {
    line.spans.iter().map(|(t, _)| t.chars().count()).sum()
}

/// The prompt mark that goes before a command and is not part of it.
const PROMPT: &str = "❯ ";

/// What the selection says: the covered part of each command and output line, one to a line, the
/// folder and time, the notes and the gaps left out, and a command without its prompt mark.
pub fn selected_text(blocks: &[Block], now_ms: u64, width: usize, sel: &Selection) -> String {
    let (a, b) = sel.ends();
    // Only the blocks that reach the lines it covers are built.
    let lines = window_lines(blocks, now_ms, width, a.line, b.line - a.line + 1);
    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let at = a.line + i;
        if !matches!(line.kind, Kind::Command | Kind::Output) {
            continue;
        }
        let text: Vec<char> = text_of(line).chars().collect();
        let Some((from, to)) = sel.on_line(at, text.len()) else {
            continue;
        };
        let skip = if line.kind == Kind::Command {
            PROMPT.chars().count().saturating_sub(from)
        } else {
            0
        };
        out.push(text[from..to].iter().skip(skip).collect());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::Span;

    fn block(id: u64, cmd: &str, out: &[&str]) -> Block {
        Block {
            id,
            cmd: cmd.into(),
            cwd: "/w".into(),
            started: 0,
            running: false,
            code: Some(0),
            ms: Some(5),
            truncated: false,
            full: false,
            interrupted: false,
            restart: false,
            out: out
                .iter()
                .map(|t| {
                    vec![Span {
                        t: (*t).into(),
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

    fn at(line: usize, col: usize) -> Point {
        Point { line, col }
    }

    fn sel(a: Point, b: Point) -> Selection {
        Selection {
            pane: "p".into(),
            anchor: a,
            head: b,
        }
    }

    #[test]
    fn a_drag_either_way_covers_the_same() {
        let (a, b) = (at(2, 4), at(5, 1));
        assert_eq!(sel(a, b).ends(), (a, b));
        assert_eq!(sel(b, a).ends(), (a, b));
    }

    #[test]
    fn the_first_line_is_covered_from_where_it_began_the_last_to_where_it_ended_and_the_rest_whole()
    {
        let s = sel(at(2, 4), at(4, 1));
        assert_eq!(s.on_line(1, 10), None);
        assert_eq!(s.on_line(2, 10), Some((4, 10)));
        assert_eq!(s.on_line(3, 10), Some((0, 10)));
        assert_eq!(
            s.on_line(4, 10),
            Some((0, 2)),
            "up to and including the column it ended on"
        );
        assert_eq!(s.on_line(5, 10), None);
        assert_eq!(s.on_line(3, 0), None, "an empty line has nothing to cover");
        assert_eq!(
            sel(at(2, 20), at(2, 30)).on_line(2, 10),
            None,
            "past the end of the text"
        );
        assert_eq!(
            sel(at(2, 3), at(2, 3)).on_line(2, 10),
            Some((3, 4)),
            "one character"
        );
        assert_eq!(sel(at(2, 3), at(2, 99)).on_line(2, 10), Some((3, 10)));
    }

    /// Lines: 0 folder, 1 `❯ git status`, 2 and 3 its output, 4 gap, 5 folder, 6 `❯ ls`, 7 `a b`, 8 gap.
    fn two() -> Vec<Block> {
        vec![
            block(1, "git status", &["On branch main", "nothing to commit"]),
            block(2, "ls", &["a b"]),
        ]
    }

    #[test]
    fn the_text_is_the_commands_and_output_between_the_ends_without_the_rest() {
        let blocks = two();
        let all = sel(at(0, 0), at(8, 99));
        assert_eq!(
            selected_text(&blocks, 0, 40, &all),
            "git status\nOn branch main\nnothing to commit\nls\na b",
            "no folder, no time, no prompt mark, no gap"
        );
        let part = sel(at(2, 3), at(3, 6));
        assert_eq!(selected_text(&blocks, 0, 40, &part), "branch main\nnothing");
        let backwards = sel(at(3, 6), at(2, 3));
        assert_eq!(
            selected_text(&blocks, 0, 40, &backwards),
            "branch main\nnothing"
        );
    }

    #[test]
    fn a_command_selected_from_the_middle_or_from_its_prompt_mark_says_the_same_command() {
        let blocks = two();
        assert_eq!(
            selected_text(&blocks, 0, 40, &sel(at(1, 0), at(1, 99))),
            "git status"
        );
        assert_eq!(
            selected_text(&blocks, 0, 40, &sel(at(1, 1), at(1, 99))),
            "git status"
        );
        assert_eq!(
            selected_text(&blocks, 0, 40, &sel(at(1, 2), at(1, 99))),
            "git status"
        );
        assert_eq!(
            selected_text(&blocks, 0, 40, &sel(at(1, 6), at(1, 99))),
            "status"
        );
    }

    #[test]
    fn a_selection_of_nothing_that_can_be_selected_says_nothing() {
        let blocks = two();
        assert_eq!(selected_text(&blocks, 0, 40, &sel(at(0, 0), at(0, 5))), "");
        assert_eq!(
            selected_text(&blocks, 0, 40, &sel(at(40, 0), at(50, 5))),
            ""
        );
        assert_eq!(selected_text(&[], 0, 40, &sel(at(0, 0), at(3, 5))), "");
    }

    /// REQ-637: what a double click takes.
    #[test]
    fn a_word_has_letters_of_any_alphabet_and_what_a_path_or_an_option_holds() {
        assert_eq!(
            word_at("hola ñandú", 6),
            Some((5, 9)),
            "the accents are letters"
        );
        assert_eq!(
            word_at("ver src/main.rs.", 6),
            Some((4, 14)),
            "a path, without its full stop"
        );
        assert_eq!(word_at("a-b -v", 5), Some((4, 5)), "an option");
        assert_eq!(word_at("hola ñandú", 4), None, "a space is no word");
        assert_eq!(
            word_at("palabra.", 7),
            None,
            "the full stop that ends it is not part of it"
        );
        assert_eq!(word_at("hola", 9), None, "past the end of the line");
        assert_eq!(word_at("", 0), None);
    }
}
