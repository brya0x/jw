//! The last output of a pane, as the bytes the program wrote (REQ-72). A
//! client that attaches replays it to get the scrollback from before it
//! came, and a restarted daemon replays the saved copy into the new pane.

use std::collections::VecDeque;

/// How much output each pane keeps.
pub const RING: usize = 2 << 20;

#[derive(Default)]
pub struct Ring {
    buf: VecDeque<u8>,
    /// Older output was dropped, so the first line may start mid-sequence.
    cut: bool,
    /// Changed since it was last saved.
    pub dirty: bool,
}

impl Ring {
    pub fn push(&mut self, bytes: &[u8]) {
        self.dirty = true;
        if bytes.len() >= RING {
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - RING..]);
            self.cut = true;
            return;
        }
        let over = (self.buf.len() + bytes.len()).saturating_sub(RING);
        if over > 0 {
            self.buf.drain(..over);
            self.cut = true;
        }
        self.buf.extend(bytes);
    }

    /// The output kept, from the first whole line when older output was
    /// dropped, so a replay doesn't start inside an escape sequence.
    pub fn bytes(&self) -> Vec<u8> {
        let (a, b) = self.buf.as_slices();
        let mut all = Vec::with_capacity(self.buf.len());
        all.extend_from_slice(a);
        all.extend_from_slice(b);
        if self.cut
            && let Some(nl) = all.iter().position(|&c| c == b'\n')
        {
            all.drain(..=nl);
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_bytes_from_a_whole_line() {
        let mut r = Ring::default();
        r.push(b"first\n");
        assert_eq!(r.bytes(), b"first\n");
        let line = vec![b'x'; 1000];
        for _ in 0..(RING / 1000 + 10) {
            r.push(&line);
            r.push(b"\n");
        }
        let kept = r.bytes();
        assert!(kept.len() <= RING);
        assert!(!kept.starts_with(b"first"));
        assert!(kept.starts_with(&line), "starts at a line");
        r.push(&vec![b'y'; RING + 5]);
        assert!(r.bytes().len() <= RING);
    }
}
