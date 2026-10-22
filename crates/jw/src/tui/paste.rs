//! A long paste in the composer's line: one character that stands for the text, with a label
//! drawn in its place (REQ-709 to REQ-712, RAT-89). The pure part: when a paste folds, what the
//! label says at a width, and which characters are kept for folds.

/// The private-use block a fold is kept in: the character `FIRST + k` is the `k`-th fold of the
/// composer's table.
pub const FIRST: u32 = 0xE000;
pub const LAST: u32 = 0xF8FF;

/// A paste folds from this many rows, or this many characters (REQ-709).
const ROWS: usize = 5;
const CHARS: usize = 300;
/// The beginning the label quotes, in characters.
const BEGIN: usize = 16;

/// What was pasted and the number it has in the message, with what its label says of it (worked
/// out once: the field asks for the label at every frame).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub n: usize,
    pub body: String,
    size: String,
    begin: String,
}

/// Whether `c` is the character of a fold.
pub fn is_fold(c: char) -> bool {
    (FIRST..=LAST).contains(&(c as u32))
}

/// The character of the `k`-th fold, none when the block has no more room.
pub fn char_of(k: usize) -> Option<char> {
    u32::try_from(k)
        .ok()
        .and_then(|k| FIRST.checked_add(k))
        .filter(|c| *c <= LAST)
        .and_then(char::from_u32)
}

/// The place of `c` in the table, when it is a fold's.
pub fn index_of(c: char) -> Option<usize> {
    is_fold(c).then(|| (c as u32 - FIRST) as usize)
}

/// The rows a text has: a final line break does not begin one.
fn rows(text: &str) -> usize {
    text.strip_suffix('\n').unwrap_or(text).split('\n').count()
}

/// Whether a paste is long enough to fold (REQ-709).
pub fn folds(text: &str) -> bool {
    rows(text) >= ROWS || text.chars().count() >= CHARS
}

/// What is typed or pasted, with the characters of the block that holds the folds written as
/// U+FFFD (REQ-712): a person's text cannot be taken for a fold.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if is_fold(c) { '\u{FFFD}' } else { c })
        .collect()
}

impl Fold {
    pub fn new(n: usize, body: String) -> Fold {
        let size = match rows(&body) {
            1 => format!("{} caracteres", body.chars().count()),
            r => format!("+{r} líneas"),
        };
        let first = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let begin = if first.chars().count() > BEGIN {
            let cut: String = first.chars().take(BEGIN).collect();
            format!("{cut}…")
        } else {
            first.to_owned()
        };
        Fold {
            n,
            body,
            size,
            begin,
        }
    }

    /// The label as the field draws it in a row of `width` characters: whole when it fits, then
    /// without its beginning, then without its size, then cut (REQ-711).
    pub fn label(&self, width: usize) -> String {
        let n = self.n;
        let full = format!("[Texto pegado #{n} {} «{}»]", self.size, self.begin);
        let short = format!("[Texto pegado #{n} {}]", self.size);
        let least = format!("[Texto pegado #{n}]");
        for label in [full, short, least.clone()] {
            if label.chars().count() <= width {
                return label;
            }
        }
        let keep = width.saturating_sub(2);
        let cut: String = least.chars().take(keep).collect();
        format!("{cut}…]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(n: usize, body: &str) -> Fold {
        Fold::new(n, body.to_owned())
    }

    fn lines(n: usize) -> String {
        (1..=n)
            .map(|i| format!("línea {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_paste_folds_from_five_rows_or_three_hundred_characters() {
        assert!(!folds(&lines(4)));
        assert!(folds(&lines(5)));
        assert!(!folds(&"x".repeat(299)));
        assert!(folds(&"x".repeat(300)));
        assert!(
            !folds("git status\ngit diff --stat"),
            "two rows go in as typed"
        );
    }

    #[test]
    fn a_final_line_break_does_not_begin_a_row() {
        assert!(!folds(&format!("{}\n", lines(4))));
        assert!(folds(&format!("{}\n", lines(5))));
    }

    #[test]
    fn the_label_says_the_rows_or_the_characters_and_the_beginning() {
        assert_eq!(
            fold(1, &lines(12)).label(80),
            "[Texto pegado #1 +12 líneas «línea 1»]"
        );
        assert_eq!(
            fold(2, &"y".repeat(700)).label(80),
            "[Texto pegado #2 700 caracteres «yyyyyyyyyyyyyyyy…»]"
        );
        assert_eq!(
            fold(1, "\n  \n   hola   \nmás").label(80),
            "[Texto pegado #1 +4 líneas «hola»]",
            "the first row that is not blank, trimmed"
        );
    }

    #[test]
    fn a_label_that_does_not_fit_loses_its_beginning_then_its_size_then_its_end() {
        let f = fold(3, &lines(12));
        assert_eq!(f.label(30), "[Texto pegado #3 +12 líneas]");
        assert_eq!(f.label(20), "[Texto pegado #3]");
        assert_eq!(f.label(10), "[Texto p…]");
        assert_eq!(f.label(8), "[Texto…]");
        for w in 8..60 {
            assert!(f.label(w).chars().count() <= w, "width {w}");
        }
    }

    #[test]
    fn the_characters_of_a_fold_are_a_block_with_a_limit() {
        assert_eq!(char_of(0), Some('\u{E000}'));
        assert_eq!(index_of('\u{E003}'), Some(3));
        assert_eq!(index_of('a'), None);
        assert_eq!(char_of((LAST - FIRST) as usize), char::from_u32(LAST));
        assert_eq!(char_of((LAST - FIRST) as usize + 1), None);
    }

    #[test]
    fn what_is_typed_cannot_be_taken_for_a_fold() {
        assert_eq!(clean("a\u{E000}b\u{F8FF}"), "a\u{FFFD}b\u{FFFD}");
        assert_eq!(clean("hola"), "hola");
    }
}
