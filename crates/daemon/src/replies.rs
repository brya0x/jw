//! What a program asks its terminal, and what the terminal answers (spec REQ-266 to REQ-268).
//!
//! The daemon is the terminal of every pane, so it is the one to answer. A program that asks
//! (neovim ends with «primary device attributes» and waits for the answer, up to a second, before it
//! leaves; claude and most of the TUI libraries ask the same at the start) is held up until a
//! terminal that never answers times it out. This reads the output of a pane for those queries and
//! says what a terminal says; the caller writes it to the pane's input.
//!
//! Only questions with a fixed or a cheap answer are answered. The colours of the screen are not:
//! they are the client's theme, and a wrong answer would change what a program draws.

/// A question a program asked, read from its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `CSI c`: what kind of terminal this is.
    PrimaryAttributes,
    /// `CSI > c`.
    SecondaryAttributes,
    /// `CSI 5 n`: is the terminal working.
    Status,
    /// `CSI 6 n`: where the cursor is.
    CursorPosition,
    /// `CSI > q`: the terminal's name and version.
    Version,
    /// `CSI ? u`: which keyboard enhancements (kitty) are on.
    KittyKeys,
    /// `CSI Ps $ p` and `CSI ? Ps $ p`: the state of a mode.
    Mode { private: bool, mode: u16 },
    /// `CSI 18 t`: the size of the text area in characters.
    TextArea,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi,
    /// Inside a string (OSC, DCS, APC…): its bytes are text, not queries.
    Str,
}

/// Reads a pane's output for queries. It keeps its state between reads, so a query that is split
/// between two of them is answered once.
#[derive(Default)]
pub struct Queries {
    state: State,
    /// The parameter and intermediate bytes of the CSI being read.
    seq: Vec<u8>,
}

/// More than this many bytes between `CSI` and its final byte is not a query.
const SEQ_MAX: usize = 24;

impl Queries {
    /// The queries in `bytes`, each with the offset just after it, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Query)> {
        let mut found = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => self.after_escape(b),
                // As in a terminal: `ESC` ends a string (`ESC \` is its end) and begins what follows.
                State::Str => match b {
                    0x07 | 0x18 | 0x1a => self.state = State::Ground,
                    0x1b => self.state = State::Esc,
                    _ => {}
                },
                State::Csi => match b {
                    0x20..=0x3f => {
                        if self.seq.len() < SEQ_MAX {
                            self.seq.push(b);
                        } else {
                            self.state = State::Ground;
                        }
                    }
                    0x40..=0x7e => {
                        self.state = State::Ground;
                        if let Some(q) = classify(&self.seq, b) {
                            found.push((i + 1, q));
                        }
                    }
                    0x1b => self.state = State::Esc,
                    // A control character inside a sequence is carried out and the sequence goes on.
                    0x00..=0x1f => {}
                    _ => self.state = State::Ground,
                },
            }
        }
        found
    }

    /// The byte after `ESC`.
    fn after_escape(&mut self, b: u8) {
        self.state = match b {
            b'[' => {
                self.seq.clear();
                State::Csi
            }
            b']' | b'P' | b'_' | b'^' | b'X' => State::Str,
            0x1b => State::Esc,
            _ => State::Ground,
        };
    }
}

/// The query a CSI with these parameter and intermediate bytes and this final byte is, if any.
fn classify(seq: &[u8], last: u8) -> Option<Query> {
    let (prefix, rest) = match seq.first() {
        Some(p @ (b'?' | b'>' | b'=' | b'<')) => (Some(*p), &seq[1..]),
        _ => (None, seq),
    };
    let split = rest
        .iter()
        .position(|b| (0x20..=0x2f).contains(b))
        .unwrap_or(rest.len());
    let (params, inter) = rest.split_at(split);
    let number = || std::str::from_utf8(params).ok()?.parse::<u16>().ok();
    let empty_or_zero = params.is_empty() || params == b"0";
    match (prefix, last, inter) {
        (None, b'c', []) if empty_or_zero => Some(Query::PrimaryAttributes),
        (Some(b'>'), b'c', []) if empty_or_zero => Some(Query::SecondaryAttributes),
        (None, b'n', []) if params == b"5" => Some(Query::Status),
        (None, b'n', []) if params == b"6" => Some(Query::CursorPosition),
        (Some(b'>'), b'q', []) if empty_or_zero => Some(Query::Version),
        (Some(b'?'), b'u', []) if params.is_empty() => Some(Query::KittyKeys),
        (None | Some(b'?'), b'p', [b'$']) => number().map(|mode| Query::Mode {
            private: prefix.is_some(),
            mode,
        }),
        (None, b't', []) if params == b"18" => Some(Query::TextArea),
        _ => None,
    }
}

impl Query {
    /// What a terminal answers, with `screen` as it was when the query was read.
    pub fn reply(self, screen: &vt100::Screen) -> Vec<u8> {
        let text = match self {
            // A VT220 with colour.
            Query::PrimaryAttributes => "\x1b[?62;22c".to_owned(),
            Query::SecondaryAttributes => "\x1b[>1;10;0c".to_owned(),
            Query::Status => "\x1b[0n".to_owned(),
            Query::CursorPosition => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            Query::Version => format!(
                "\x1bP>|{}({})\x1b\\",
                identity::get().name,
                env!("CARGO_PKG_VERSION")
            ),
            // No enhancement is on.
            Query::KittyKeys => "\x1b[?0u".to_owned(),
            // «Not recognised»: a program that asks about a mode then does without it.
            Query::Mode { private, mode } => {
                format!("\x1b[{}{mode};0$y", if private { "?" } else { "" })
            }
            Query::TextArea => {
                let (rows, cols) = screen.size();
                format!("\x1b[8;{rows};{cols}t")
            }
        };
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queries(bytes: &[u8]) -> Vec<Query> {
        Queries::default()
            .feed(bytes)
            .into_iter()
            .map(|(_, q)| q)
            .collect()
    }

    #[test]
    fn every_query_that_is_answered_is_read() {
        assert_eq!(queries(b"\x1b[c"), [Query::PrimaryAttributes]);
        assert_eq!(queries(b"\x1b[0c"), [Query::PrimaryAttributes]);
        assert_eq!(queries(b"\x1b[>c"), [Query::SecondaryAttributes]);
        assert_eq!(queries(b"\x1b[>0c"), [Query::SecondaryAttributes]);
        assert_eq!(queries(b"\x1b[5n"), [Query::Status]);
        assert_eq!(queries(b"\x1b[6n"), [Query::CursorPosition]);
        assert_eq!(queries(b"\x1b[>0q"), [Query::Version]);
        assert_eq!(queries(b"\x1b[>q"), [Query::Version]);
        assert_eq!(queries(b"\x1b[?u"), [Query::KittyKeys]);
        assert_eq!(
            queries(b"\x1b[?2026$p\x1b[4$p"),
            [
                Query::Mode {
                    private: true,
                    mode: 2026
                },
                Query::Mode {
                    private: false,
                    mode: 4
                }
            ]
        );
        assert_eq!(queries(b"\x1b[18t"), [Query::TextArea]);
    }

    #[test]
    fn what_is_not_a_question_is_not_read() {
        // A colour, a cursor move, a title (push and pop), a mode that is set, a report that
        // looks like a query but has other parameters, and the replies a terminal itself sends.
        for text in [
            &b"\x1b[31m"[..],
            b"\x1b[10;20H",
            b"\x1b[22;0;0t",
            b"\x1b[?2004h",
            b"\x1b[7n",
            b"\x1b[?62;22c",
            b"\x1b[3;4R",
            b"plain text",
        ] {
            assert!(queries(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn what_looks_like_a_query_inside_a_string_is_text() {
        // The title of a window may hold anything but an escape: here it is plain text.
        assert!(queries(b"\x1b]0;a [c and 6n and [?u\x07").is_empty());
        assert!(queries(b"\x1bP1$r[c\x1b\\").is_empty());
        assert_eq!(
            queries(b"\x1b]0;t\x07\x1b[c"),
            [Query::PrimaryAttributes],
            "and the one after the string is read"
        );
        // An escape ends a string and begins what follows, as in a terminal.
        assert_eq!(queries(b"\x1b]0;a\x1b[c"), [Query::PrimaryAttributes]);
    }

    #[test]
    fn a_query_split_between_two_reads_is_answered_once_and_after_its_last_byte() {
        let mut q = Queries::default();
        assert!(q.feed(b"text\x1b").is_empty());
        assert!(q.feed(b"[").is_empty());
        assert_eq!(q.feed(b"6nmore"), [(2, Query::CursorPosition)]);
        assert!(q.feed(b"\x1b[31m").is_empty(), "the state is clean again");
    }

    #[test]
    fn the_offsets_are_where_each_query_ends() {
        let mut q = Queries::default();
        assert_eq!(
            q.feed(b"ab\x1b[6ncd\x1b[c"),
            [(6, Query::CursorPosition), (11, Query::PrimaryAttributes)]
        );
    }

    #[test]
    fn a_runaway_sequence_is_dropped() {
        let mut long = b"\x1b[".to_vec();
        long.extend(std::iter::repeat_n(b'1', 100));
        long.push(b'c');
        long.extend_from_slice(b"\x1b[c");
        assert_eq!(queries(&long), [Query::PrimaryAttributes]);
    }

    fn screen_after(bytes: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(10, 40, 0);
        p.process(bytes);
        p
    }

    #[test]
    fn the_answers_are_what_a_terminal_says() {
        let p = screen_after(b"ab\r\ncde");
        let s = p.screen();
        let text = |q: Query| String::from_utf8(q.reply(s)).unwrap();
        assert_eq!(text(Query::PrimaryAttributes), "\x1b[?62;22c");
        assert_eq!(text(Query::Status), "\x1b[0n");
        assert_eq!(
            text(Query::CursorPosition),
            "\x1b[2;4R",
            "row 2, column 4, from 1"
        );
        assert_eq!(text(Query::KittyKeys), "\x1b[?0u");
        assert_eq!(text(Query::TextArea), "\x1b[8;10;40t");
        assert_eq!(
            text(Query::Mode {
                private: true,
                mode: 2026
            }),
            "\x1b[?2026;0$y"
        );
        assert_eq!(
            text(Query::Mode {
                private: false,
                mode: 4
            }),
            "\x1b[4;0$y"
        );
        let version = text(Query::Version);
        assert!(
            version.starts_with("\x1bP>|") && version.ends_with("\x1b\\"),
            "{version:?}"
        );
        assert!(version.contains(&identity::get().name), "{version:?}");
    }
}
