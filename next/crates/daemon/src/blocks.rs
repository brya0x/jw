//! Shell blocks: the marks a shell integration prints are scanned out of a pane's bytes, and
//! the bytes of each command become that command's block, rendered by vt100 into rows of
//! styled spans (so a client draws spans and needs no terminal emulator).

use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use contract::{Block, BlocksMode, Color, Span};

/// At most this many blocks per pane, rows per block and bytes of output in all.
pub const MAX_BLOCKS: usize = 100;
pub const MAX_ROWS: u16 = 500;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// A mark longer than this is not a mark.
const MARK_MAX: usize = 8 * 1024;
/// The sequences that switch to the alternate screen.
const ALT_SCREEN: [&[u8]; 3] = [b"\x1b[?1049h", b"\x1b[?1047h", b"\x1b[?47h"];
const ALT_SCREEN_MAX: usize = 8;

/// What a shell integration reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// OSC 133;A: a prompt is about to be drawn.
    Prompt,
    /// OSC 133;B: the prompt is drawn and input starts.
    Input,
    /// OSC 133;C with the command (633;E) and folder (633;P) before it: the command runs.
    Command { cmd: String, cwd: String },
    /// OSC 133;D: the command ended.
    Done(Option<i32>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Data(Vec<u8>),
    Mark(Mark),
}

#[derive(Default)]
enum State {
    #[default]
    Text,
    Esc,
    /// Inside `ESC ]`: the bytes after it so far.
    Osc(Vec<u8>),
    /// Inside an OSC, after an ESC that may start the string terminator.
    OscEsc(Vec<u8>),
}

/// Splits a byte stream into data and marks. Only OSC 133 and 633 are taken out; everything
/// else, other OSC sequences included, passes through untouched.
#[derive(Default)]
pub struct Scanner {
    state: State,
    cmd: String,
    cwd: String,
}

fn unb64(text: &str) -> String {
    // Standard base64 decoding, no dependency: the integration scripts encode with `base64`.
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=') {
        let Some(v) = T.iter().position(|t| *t == c) else {
            continue;
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn take_data(data: &mut Vec<u8>, out: &mut Vec<Piece>) {
    if !data.is_empty() {
        out.push(Piece::Data(std::mem::take(data)));
    }
}

impl Scanner {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        let mut data: Vec<u8> = Vec::new();
        for &b in bytes {
            match std::mem::take(&mut self.state) {
                State::Text => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        data.push(b);
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.state = State::Osc(Vec::new());
                    } else {
                        data.extend([0x1b, b]);
                    }
                }
                State::Osc(mut body) => match b {
                    0x07 => self.end_osc(&body, &[0x07], &mut data, &mut out),
                    0x1b => self.state = State::OscEsc(body),
                    _ if body.len() >= MARK_MAX => {
                        data.extend([0x1b, b']']);
                        data.extend(&body);
                        data.push(b);
                    }
                    _ => {
                        body.push(b);
                        self.state = State::Osc(body);
                    }
                },
                State::OscEsc(body) => {
                    if b == b'\\' {
                        self.end_osc(&body, &[0x1b, b'\\'], &mut data, &mut out);
                    } else {
                        // Not a terminator: the ESC started something else inside the string.
                        data.extend([0x1b, b']']);
                        data.extend(&body);
                        data.extend([0x1b, b]);
                    }
                }
            }
        }
        take_data(&mut data, &mut out);
        out
    }

    fn end_osc(
        &mut self,
        body: &[u8],
        terminator: &[u8],
        data: &mut Vec<u8>,
        out: &mut Vec<Piece>,
    ) {
        match self.finish(body) {
            Finished::Mark(m) => {
                take_data(data, out);
                out.push(Piece::Mark(m));
            }
            Finished::Remembered => {}
            Finished::Passthrough => {
                data.extend([0x1b, b']']);
                data.extend(body);
                data.extend(terminator);
            }
        }
    }

    /// What a complete OSC string turned out to be.
    fn finish(&mut self, body: &[u8]) -> Finished {
        let text = String::from_utf8_lossy(body);
        match text.split_once(';') {
            Some(("133", rest)) => {
                let mut parts = rest.split(';');
                match parts.next() {
                    Some("A") => Finished::Mark(Mark::Prompt),
                    Some("B") => Finished::Mark(Mark::Input),
                    Some("C") => Finished::Mark(Mark::Command {
                        cmd: std::mem::take(&mut self.cmd),
                        cwd: std::mem::take(&mut self.cwd),
                    }),
                    Some("D") => {
                        Finished::Mark(Mark::Done(parts.next().and_then(|c| c.parse().ok())))
                    }
                    _ => Finished::Passthrough,
                }
            }
            Some(("633", rest)) => {
                if let Some(c) = rest.strip_prefix("E;") {
                    self.cmd = unb64(c);
                    Finished::Remembered
                } else if let Some(p) = rest.strip_prefix("P;Cwd=") {
                    self.cwd = unb64(p);
                    Finished::Remembered
                } else {
                    Finished::Passthrough
                }
            }
            _ => Finished::Passthrough,
        }
    }
}

enum Finished {
    Mark(Mark),
    /// A command's text or folder, kept for the `C` mark that follows.
    Remembered,
    /// Not ours: the terminal gets it.
    Passthrough,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Running {
    id: u64,
    cmd: String,
    cwd: String,
    started: u64,
    clock: Instant,
    parser: vt100::Parser,
    newlines: usize,
    full: bool,
    /// The end of the previous bytes, so a sequence split between reads is still seen.
    tail: Vec<u8>,
}

/// The blocks of one pane.
pub struct BlockStore {
    next_id: u64,
    done: VecDeque<Block>,
    running: Option<Running>,
    integrated: bool,
    cols: u16,
    /// Changes whenever what a client would see changes.
    version: u64,
}

fn to_color(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Index(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(format!("#{r:02x}{g:02x}{b:02x}"))),
    }
}

/// The screen's rows as spans: neighbouring cells of one style are one span, trailing blanks
/// and blank rows at the end are dropped.
pub fn rows_of(screen: &vt100::Screen) -> Vec<Vec<Span>> {
    let (rows, cols) = screen.size();
    let mut out: Vec<Vec<Span>> = Vec::new();
    for r in 0..rows {
        let mut spans: Vec<Span> = Vec::new();
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let text = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            }
            .to_owned();
            let (mut fg, mut bg) = (to_color(cell.fgcolor()), to_color(cell.bgcolor()));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
                fg = fg.or(Some(Color::Index(0)));
                bg = bg.or(Some(Color::Index(7)));
            }
            let style = (
                fg.clone(),
                bg.clone(),
                cell.bold(),
                cell.italic(),
                cell.underline(),
            );
            match spans.last_mut() {
                Some(s) if (s.fg.clone(), s.bg.clone(), s.b, s.i, s.u) == style => {
                    s.t.push_str(&text)
                }
                _ => spans.push(Span {
                    t: text,
                    fg,
                    bg,
                    b: style.2,
                    i: style.3,
                    u: style.4,
                }),
            }
        }
        while spans
            .last()
            .is_some_and(|s| s.t.trim().is_empty() && s.bg.is_none() && !s.u)
        {
            spans.pop();
        }
        if let Some(last) = spans.last_mut()
            && last.bg.is_none()
        {
            last.t = last.t.trim_end().to_owned();
        }
        out.push(spans);
    }
    while out.last().is_some_and(|r| r.is_empty()) {
        out.pop();
    }
    out
}

fn weight(b: &Block) -> usize {
    b.cmd.len()
        + b.cwd.len()
        + 64
        + b.out
            .iter()
            .map(|r| 8 + r.iter().map(|s| s.t.len() + 24).sum::<usize>())
            .sum::<usize>()
}

impl BlockStore {
    pub fn new(cols: u16) -> BlockStore {
        BlockStore {
            next_id: 1,
            done: VecDeque::new(),
            running: None,
            integrated: false,
            cols,
            version: 0,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The shell was started with the integration: its blocks exist (empty) from the start,
    /// so a client draws blocks before the first prompt.
    pub fn integrate(&mut self) {
        self.integrated = true;
    }

    pub fn set_cols(&mut self, cols: u16) {
        self.cols = cols;
    }

    pub fn mark(&mut self, mark: Mark) {
        match mark {
            Mark::Prompt | Mark::Input => {
                if !self.integrated {
                    self.integrated = true;
                    self.version += 1;
                }
            }
            Mark::Command { cmd, cwd } => {
                self.integrated = true;
                if self.running.is_some() {
                    self.finish(None);
                }
                self.running = Some(Running {
                    id: self.next_id,
                    cmd,
                    cwd,
                    started: now_ms(),
                    clock: Instant::now(),
                    parser: vt100::Parser::new(MAX_ROWS, self.cols.max(1), 0),
                    newlines: 0,
                    full: false,
                    tail: Vec::new(),
                });
                self.next_id += 1;
                self.version += 1;
            }
            Mark::Done(code) => self.finish(code),
        }
    }

    /// Bytes a command printed. Output outside a command (the prompt, typing) is not a block's.
    pub fn data(&mut self, bytes: &[u8]) {
        let Some(run) = self.running.as_mut() else {
            return;
        };
        if run.full {
            return;
        }
        run.newlines += bytes.iter().filter(|b| **b == b'\n').count();
        run.parser.process(bytes);
        // A program can enter the alternate screen and leave it within one read, so the
        // parser's state at the end says nothing: look for the switch itself.
        let mut seen = std::mem::take(&mut run.tail);
        seen.extend_from_slice(bytes);
        let entered = ALT_SCREEN
            .iter()
            .any(|seq| seen.windows(seq.len()).any(|w| w == *seq));
        run.tail = seen[seen.len().saturating_sub(ALT_SCREEN_MAX)..].to_vec();
        if entered || run.parser.screen().alternate_screen() {
            // A program took the whole screen: its output is the pane's screen, not a block's.
            run.full = true;
            run.parser = vt100::Parser::new(1, 1, 0);
        }
        self.version += 1;
    }

    fn finish(&mut self, code: Option<i32>) {
        let Some(run) = self.running.take() else {
            return;
        };
        let mut block = Self::block_of(&run);
        block.running = false;
        block.code = Some(code.unwrap_or(0));
        block.ms = Some(run.clock.elapsed().as_millis() as u64);
        self.done.push_back(block);
        while self.done.len() > MAX_BLOCKS {
            self.done.pop_front();
        }
        while self.done.iter().map(weight).sum::<usize>() > MAX_BYTES && self.done.len() > 1 {
            self.done.pop_front();
        }
        self.version += 1;
    }

    fn block_of(run: &Running) -> Block {
        Block {
            id: run.id,
            cmd: run.cmd.clone(),
            cwd: run.cwd.clone(),
            started: run.started,
            running: true,
            code: None,
            ms: None,
            truncated: run.newlines >= usize::from(MAX_ROWS),
            full: run.full,
            out: if run.full {
                Vec::new()
            } else {
                rows_of(run.parser.screen())
            },
        }
    }

    /// What a client sees, with the version it is a view of; `None` for a shell without the
    /// integration.
    pub fn view(&self) -> Option<(u64, BlocksMode, Vec<Block>)> {
        if !self.integrated {
            return None;
        }
        let mut blocks: Vec<Block> = self.done.iter().cloned().collect();
        let mut mode = BlocksMode::Blocks;
        if let Some(run) = &self.running {
            blocks.push(Self::block_of(run));
            if run.full {
                mode = BlocksMode::Full;
            }
        }
        Some((self.version, mode, blocks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_integrated_shell_has_empty_blocks_before_its_first_prompt() {
        let mut s = BlockStore::new(80);
        assert!(s.view().is_none(), "no integration, no blocks");
        s.integrate();
        let (_, mode, blocks) = s.view().unwrap();
        assert!(blocks.is_empty() && mode == BlocksMode::Blocks);
    }

    fn pieces(chunks: &[&[u8]]) -> Vec<Piece> {
        let mut s = Scanner::default();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(s.feed(c));
        }
        out
    }

    #[test]
    fn marks_are_taken_out_and_data_passes_through() {
        let p = pieces(&[b"hi\x1b]133;A\x07there"]);
        assert_eq!(
            p,
            [
                Piece::Data(b"hi".to_vec()),
                Piece::Mark(Mark::Prompt),
                Piece::Data(b"there".to_vec())
            ]
        );
    }

    #[test]
    fn a_mark_split_across_reads_is_still_one_mark() {
        let p = pieces(&[b"\x1b]13", b"3;D;3", b"\x07x"]);
        assert_eq!(
            p,
            [Piece::Mark(Mark::Done(Some(3))), Piece::Data(b"x".to_vec())]
        );
    }

    #[test]
    fn both_terminators_work() {
        let p = pieces(&[b"\x1b]133;B\x1b\\\x1b]133;D\x07"]);
        assert_eq!(p, [Piece::Mark(Mark::Input), Piece::Mark(Mark::Done(None))]);
    }

    #[test]
    fn a_command_carries_its_text_and_folder() {
        // "ls -la" and "/tmp" in base64.
        let p = pieces(&[b"\x1b]633;E;bHMgLWxh\x07\x1b]633;P;Cwd=L3RtcA==\x07\x1b]133;C\x07"]);
        assert_eq!(
            p,
            [Piece::Mark(Mark::Command {
                cmd: "ls -la".into(),
                cwd: "/tmp".into()
            })]
        );
    }

    #[test]
    fn other_sequences_are_left_alone() {
        let raw = b"\x1b]0;title\x07\x1b[31mred\x1b[0m";
        let p = pieces(&[raw]);
        assert_eq!(p, [Piece::Data(raw.to_vec())]);
    }

    fn store_run(out: &[u8]) -> Block {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: "x".into(),
            cwd: "/w".into(),
        });
        s.data(out);
        s.mark(Mark::Done(Some(2)));
        s.view().unwrap().2.pop().unwrap()
    }

    #[test]
    fn a_block_has_its_command_folder_status_and_output_rows() {
        let b = store_run(b"one\r\ntwo\r\n");
        assert_eq!(
            (b.id, b.cmd.as_str(), b.cwd.as_str(), b.running, b.code),
            (1, "x", "/w", false, Some(2))
        );
        assert!(b.ms.is_some());
        let text: Vec<String> = b
            .out
            .iter()
            .map(|r| r.iter().map(|s| s.t.clone()).collect())
            .collect();
        assert_eq!(text, ["one", "two"]);
    }

    #[test]
    fn colours_and_attributes_become_spans() {
        let b = store_run(b"ok \x1b[1;31mFAIL\x1b[0m \x1b[38;2;229;192;123mgold\x1b[0m\r\n");
        let row = &b.out[0];
        assert_eq!(row[0].t, "ok ");
        assert_eq!(
            (row[1].t.as_str(), row[1].fg.clone(), row[1].b),
            ("FAIL", Some(Color::Index(1)), true)
        );
        assert_eq!(row[3].fg, Some(Color::Rgb("#e5c07b".into())));
    }

    #[test]
    fn a_running_block_has_no_status_yet() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: "sleep 9".into(),
            cwd: "/".into(),
        });
        s.data(b"working\r\n");
        let (_, mode, blocks) = s.view().unwrap();
        assert_eq!(mode, BlocksMode::Blocks);
        assert!(blocks[0].running && blocks[0].code.is_none() && blocks[0].ms.is_none());
        assert_eq!(blocks[0].out.len(), 1);
    }

    #[test]
    fn a_program_that_takes_the_whole_screen_is_a_full_block_with_no_output() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: "nvim".into(),
            cwd: "/".into(),
        });
        s.data(b"\x1b[?1049hhello");
        let (_, mode, blocks) = s.view().unwrap();
        assert_eq!(mode, BlocksMode::Full);
        assert!(blocks[0].full && blocks[0].out.is_empty());
        s.mark(Mark::Done(Some(0)));
        let (_, mode, blocks) = s.view().unwrap();
        assert_eq!(mode, BlocksMode::Blocks);
        assert!(blocks[0].full && blocks[0].out.is_empty());
    }

    #[test]
    fn entering_and_leaving_the_alternate_screen_in_one_read_is_still_full() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: "x".into(),
            cwd: "/".into(),
        });
        s.data(b"\x1b[?1049hhello\x1b[?1049l");
        s.mark(Mark::Done(Some(0)));
        let b = &s.view().unwrap().2[0];
        assert!(b.full && b.out.is_empty());
    }

    #[test]
    fn the_switch_split_between_two_reads_is_found() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: "x".into(),
            cwd: "/".into(),
        });
        s.data(b"\x1b[?10");
        s.data(b"49hdraw\x1b[?1049l");
        s.mark(Mark::Done(Some(0)));
        assert!(s.view().unwrap().2[0].full);
    }

    #[test]
    fn nothing_is_shown_until_the_shell_reports_a_prompt() {
        let mut s = BlockStore::new(40);
        s.data(b"no prompt yet");
        assert!(s.view().is_none());
        s.mark(Mark::Prompt);
        assert_eq!(s.view().unwrap().2.len(), 0);
    }

    #[test]
    fn output_outside_a_command_is_not_a_blocks() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        s.data(b"~ $ typing");
        s.mark(Mark::Command {
            cmd: "a".into(),
            cwd: "/".into(),
        });
        s.data(b"result\r\n");
        s.mark(Mark::Done(Some(0)));
        s.data(b"~ $ ");
        let blocks = s.view().unwrap().2;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].out.len(), 1);
    }

    #[test]
    fn the_oldest_blocks_go_first_and_ids_are_never_reused() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        for i in 0..MAX_BLOCKS + 5 {
            s.mark(Mark::Command {
                cmd: format!("c{i}"),
                cwd: "/".into(),
            });
            s.mark(Mark::Done(Some(0)));
        }
        let blocks = s.view().unwrap().2;
        assert_eq!(blocks.len(), MAX_BLOCKS);
        assert_eq!(blocks[0].id, 6);
        assert_eq!(blocks.last().unwrap().id, (MAX_BLOCKS + 5) as u64);
    }

    #[test]
    fn a_very_long_output_is_marked_truncated() {
        let long: Vec<u8> = (0..600)
            .flat_map(|i| format!("line {i}\r\n").into_bytes())
            .collect();
        let b = store_run(&long);
        assert!(b.truncated);
        assert!(b.out.len() <= usize::from(MAX_ROWS));
    }
}
