//! The kitty keyboard protocol's flag stacks of one pane (REQ-92..94), as its
//! program sets them with `CSI > f u`, `CSI < n u` and `CSI = f ; m u`. The
//! daemon and every client keep one per pane, fed by their vt100 parser's
//! `unhandled_csi`; the flags reach clients in-band, like DECCKM (RAT-15).

/// How many pushes a stack keeps; older ones fall off the bottom.
const DEPTH: usize = 16;

/// Pops that empty any stack: `DEPTH` saved entries, then the current one.
pub const CLEAR: &[u8] = b"\x1b[<17u";

/// The flags in use and the ones saved under them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Stack {
    cur: u8,
    saved: Vec<u8>,
}

impl Stack {
    fn push(&mut self, f: u8) {
        self.saved.push(self.cur);
        if self.saved.len() > DEPTH {
            self.saved.remove(0);
        }
        self.cur = f;
    }

    fn pop(&mut self, n: u16) {
        for _ in 0..n.max(1) {
            self.cur = self.saved.pop().unwrap_or(0);
        }
    }
}

/// A pane's two stacks: the kitty spec keeps the main and the alternate
/// screen's apart.
#[derive(Debug, Clone, Default)]
pub struct Kitty {
    main: Stack,
    alt: Stack,
    /// Whether the last sequence came on the alternate screen: entering it
    /// starts its stack empty (REQ-94).
    on_alt: bool,
}

impl Kitty {
    /// Takes one CSI sequence the parser didn't handle; `alt` is whether the
    /// alternate screen is in use. Returns the reply to a query (REQ-93).
    pub fn csi(
        &mut self,
        alt: bool,
        i1: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) -> Option<Vec<u8>> {
        if c != 'u' {
            return None;
        }
        if alt && !self.on_alt {
            self.alt = Stack::default();
        }
        self.on_alt = alt;
        let p = |i: usize| params.get(i).and_then(|p| p.first()).copied();
        let st = if alt { &mut self.alt } else { &mut self.main };
        match i1 {
            Some(b'>') => st.push(flags(p(0))),
            Some(b'<') => st.pop(p(0).unwrap_or(1)),
            Some(b'=') => {
                let f = flags(p(0));
                match p(1).unwrap_or(1) {
                    1 => st.cur = f,
                    2 => st.cur |= f,
                    3 => st.cur &= !f,
                    _ => {}
                }
            }
            Some(b'?') => return Some(format!("\x1b[?{}u", st.cur).into_bytes()),
            _ => {}
        }
        None
    }

    /// The flags keys are encoded with on the screen in use.
    pub fn flags(&self, alt: bool) -> u8 {
        match (alt, self.on_alt) {
            (false, _) => self.main.cur,
            (true, true) => self.alt.cur,
            // Entered since the last sequence: still empty.
            (true, false) => 0,
        }
    }

    /// Sequences that leave a tracker that read the pane's (possibly cut)
    /// history with this stack for the screen in use (REQ-97).
    pub fn replay(&self, alt: bool) -> Vec<u8> {
        let st = match (alt, self.on_alt) {
            (false, _) => &self.main,
            (true, true) => &self.alt,
            (true, false) => &Stack {
                cur: 0,
                saved: Vec::new(),
            },
        };
        let mut out = CLEAR.to_vec();
        let mut entries = st.saved.iter().chain([&st.cur]);
        if let Some(base) = entries.next() {
            out.extend_from_slice(format!("\x1b[={base}u").as_bytes());
        }
        for f in entries {
            out.extend_from_slice(format!("\x1b[>{f}u").as_bytes());
        }
        out
    }
}

/// The kitty flags are five bits.
fn flags(p: Option<u16>) -> u8 {
    (p.unwrap_or(0) & 0x1f) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `s` (sequences of the form `ESC [ i n ; m u`) to `k`.
    fn feed(k: &mut Kitty, alt: bool, s: &str) -> Vec<u8> {
        let mut replies = Vec::new();
        for seq in s.split("\x1b[").filter(|s| !s.is_empty()) {
            let body = seq.strip_suffix('u').expect("ends in u");
            let (i1, rest) = match body.as_bytes().first() {
                Some(&b) if b"<=>?".contains(&b) => (Some(b), &body[1..]),
                _ => (None, body),
            };
            let nums: Vec<Vec<u16>> = rest
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|n| vec![n.parse().unwrap()])
                .collect();
            let params: Vec<&[u16]> = nums.iter().map(Vec::as_slice).collect();
            replies.extend(k.csi(alt, i1, &params, 'u').unwrap_or_default());
        }
        replies
    }

    #[test]
    fn push_pop_and_past_empty() {
        let mut k = Kitty::default();
        feed(&mut k, false, "\x1b[>1u\x1b[>5u");
        assert_eq!(k.flags(false), 5);
        feed(&mut k, false, "\x1b[<u");
        assert_eq!(k.flags(false), 1);
        feed(&mut k, false, "\x1b[<3u");
        assert_eq!(k.flags(false), 0);
    }

    #[test]
    fn set_modes() {
        let mut k = Kitty::default();
        feed(&mut k, false, "\x1b[=1u");
        assert_eq!(k.flags(false), 1);
        feed(&mut k, false, "\x1b[=4;2u");
        assert_eq!(k.flags(false), 5);
        feed(&mut k, false, "\x1b[=1;3u");
        assert_eq!(k.flags(false), 4);
    }

    #[test]
    fn query_replies_with_the_top() {
        let mut k = Kitty::default();
        assert_eq!(feed(&mut k, false, "\x1b[?u"), b"\x1b[?0u");
        assert_eq!(feed(&mut k, false, "\x1b[>1u\x1b[>5u\x1b[?u"), b"\x1b[?5u");
    }

    #[test]
    fn depth_is_capped() {
        let mut k = Kitty::default();
        for _ in 0..20 {
            feed(&mut k, false, "\x1b[>1u");
        }
        assert_eq!(k.main.saved.len(), DEPTH);
        feed(&mut k, false, std::str::from_utf8(CLEAR).unwrap());
        assert_eq!(k.main, Stack::default());
    }

    #[test]
    fn alternate_screen_has_its_own_stack_and_starts_empty() {
        let mut k = Kitty::default();
        feed(&mut k, false, "\x1b[>1u");
        assert_eq!(k.flags(true), 0);
        feed(&mut k, true, "\x1b[>5u");
        assert_eq!((k.flags(false), k.flags(true)), (1, 5));
        // Left and came back: the old alternate stack is gone.
        feed(&mut k, false, "\x1b[?u");
        assert_eq!(k.flags(true), 0);
        feed(&mut k, true, "\x1b[?u");
        assert_eq!(k.flags(true), 0);
    }

    #[test]
    fn other_sequences_are_ignored() {
        let mut k = Kitty::default();
        assert_eq!(k.csi(false, Some(b'>'), &[&[1]], 'm'), None);
        feed(&mut k, false, "\x1b[u");
        assert_eq!(k.flags(false), 0);
    }

    #[test]
    fn replay_round_trips() {
        let mut k = Kitty::default();
        feed(&mut k, false, "\x1b[=2u\x1b[>1u\x1b[>5u");
        // A client whose history held other pushes.
        let mut c = Kitty::default();
        feed(&mut c, false, "\x1b[>8u\x1b[>8u\x1b[>8u");
        let replay = String::from_utf8(k.replay(false)).unwrap();
        feed(&mut c, false, &replay);
        for _ in 0..4 {
            assert_eq!(c.flags(false), k.flags(false));
            feed(&mut k, false, "\x1b[<u");
            feed(&mut c, false, "\x1b[<u");
        }
    }
}
