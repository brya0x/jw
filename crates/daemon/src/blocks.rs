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
    /// OSC 633;P;Pwd: the folder the shell is in, at a prompt.
    Folder(String),
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
                } else if let Some(p) = rest.strip_prefix("P;Pwd=") {
                    Finished::Mark(Mark::Folder(unb64(p)))
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

pub fn now_ms() -> u64 {
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
    /// The folder the shell said it is in at its last prompt.
    folder: Option<String>,
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
        // The style of the span being built, as the terminal has it: cells are compared on it, and
        // the colours are turned into the contract's only when a span starts.
        let mut open = None;
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
            };
            let style = (
                cell.fgcolor(),
                cell.bgcolor(),
                cell.inverse(),
                cell.bold(),
                cell.italic(),
                cell.underline(),
            );
            if open == Some(style)
                && let Some(s) = spans.last_mut()
            {
                s.t.push_str(text);
                continue;
            }
            open = Some(style);
            let (mut fg, mut bg) = (to_color(style.0), to_color(style.1));
            if style.2 {
                std::mem::swap(&mut fg, &mut bg);
                fg = fg.or(Some(Color::Index(0)));
                bg = bg.or(Some(Color::Index(7)));
            }
            spans.push(Span {
                t: text.to_owned(),
                fg,
                bg,
                b: style.3,
                i: style.4,
                u: style.5,
            });
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

/// The command that clears the pane's history: `clear` alone (`clear -x` keeps what is above).
fn is_clear(cmd: &str) -> bool {
    cmd.trim() == "clear"
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
            folder: None,
            cols,
            version: 0,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The shell was started with the integration and no program: its blocks exist (empty) from
    /// the start, so a client draws blocks before the first prompt. A program started with `-c`
    /// leaves the store as it is until its first mark: the `C` of the script that reports it
    /// (REQ-1056) or the shell's first prompt (REQ-844).
    pub fn integrate(&mut self) {
        self.integrated = true;
    }

    /// Take the blocks a daemon saved before it stopped (spec REQ-298 to REQ-300): the history as it
    /// was, a command that was running as interrupted (it ran until `saved_at` as far as anyone
    /// knows), and a mark where this daemon began unless the last block already is one. Nothing
    /// is taken when there was nothing saved.
    pub fn restore(&mut self, saved: Vec<Block>, saved_at: u64, now: u64) {
        let mut blocks: VecDeque<Block> = saved
            .into_iter()
            .map(|mut b| {
                if b.running {
                    b.running = false;
                    b.interrupted = true;
                    b.code = None;
                    b.ms = Some(saved_at.saturating_sub(b.started));
                }
                b
            })
            .collect();
        if blocks.is_empty() {
            return;
        }
        self.next_id = self
            .next_id
            .max(blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1);
        if !blocks.back().is_some_and(|b| b.restart) {
            blocks.push_back(Block {
                id: self.next_id,
                cmd: String::new(),
                cwd: String::new(),
                started: now,
                running: false,
                code: None,
                ms: None,
                truncated: false,
                full: false,
                interrupted: false,
                restart: true,
                out: Vec::new(),
            });
            self.next_id += 1;
        }
        while blocks.len() > MAX_BLOCKS {
            blocks.pop_front();
        }
        while blocks.iter().map(weight).sum::<usize>() > MAX_BYTES && blocks.len() > 1 {
            blocks.pop_front();
        }
        self.done = blocks;
        self.integrated = true;
        self.version += 1;
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
                // fish 4 marks the start of a command itself, before our script can say what
                // it is, and then our own mark follows: the second one names the same command.
                if let Some(run) = self.running.as_mut().filter(|r| r.cmd.is_empty()) {
                    run.cmd = cmd;
                    run.cwd = cwd;
                    self.version += 1;
                    return;
                }
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
            Mark::Folder(folder) => self.folder = Some(folder),
        }
    }

    /// The folder the shell said it is in at its last prompt.
    pub fn folder(&self) -> Option<&str> {
        self.folder.as_deref()
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
        if is_clear(&run.cmd) && code == Some(0) {
            self.done.clear();
            self.version += 1;
            return;
        }
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
            interrupted: false,
            restart: false,
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

    #[test]
    fn a_store_not_integrated_has_no_view_until_its_first_mark_and_then_blocks() {
        // A pane started with a program (REQ-844): nothing integrates it at the start.
        let mut s = BlockStore::new(80);
        s.data(b"output of a program\n");
        assert!(s.view().is_none(), "no prompt yet: the pane is its screen");
        s.mark(Mark::Prompt);
        let (_, mode, blocks) = s.view().unwrap();
        assert!(blocks.is_empty() && mode == BlocksMode::Blocks);
        s.mark(Mark::Command {
            cmd: "ls".into(),
            cwd: "/".into(),
        });
        s.data(b"x\n");
        s.mark(Mark::Done(Some(0)));
        assert_eq!(s.view().unwrap().2.len(), 1, "blocks work as in any shell");
    }

    /// REQ-1056 and REQ-1057: what the `-c` script prints around a program (`launch::script`) opens a
    /// block with no prompt before it, its output is the block's while it runs, and `D` closes it
    /// with the status; the prompt of the shell that takes over adds no block.
    #[test]
    fn the_marks_of_the_script_make_the_program_a_running_block_that_ends_with_its_status() {
        // "pnpm dev" and "/w" in base64.
        let mut scanner = Scanner::default();
        let mut s = BlockStore::new(40);
        let mut feed = |s: &mut BlockStore, bytes: &[u8]| {
            for piece in scanner.feed(bytes) {
                match piece {
                    Piece::Data(d) => s.data(&d),
                    Piece::Mark(m) => s.mark(m),
                }
            }
        };
        assert!(
            s.view().is_none(),
            "nothing before the program says it runs"
        );
        feed(
            &mut s,
            b"\x1b]633;E;cG5wbSBkZXY=\x07\x1b]633;P;Cwd=L3c=\x07\x1b]133;C\x07ready\r\n",
        );
        let (_, mode, blocks) = s.view().unwrap();
        assert_eq!(mode, BlocksMode::Blocks);
        let [b] = &blocks[..] else {
            panic!("{blocks:?}")
        };
        assert_eq!(
            (b.cmd.as_str(), b.cwd.as_str(), b.running),
            ("pnpm dev", "/w", true)
        );
        assert_eq!(b.out[0][0].t, "ready", "its output while it runs");
        feed(&mut s, b"\x1b]133;D;130\x07\x1b]133;A\x07$ \x1b]133;B\x07");
        let (_, _, blocks) = s.view().unwrap();
        let [b] = &blocks[..] else {
            panic!("{blocks:?}")
        };
        assert_eq!((b.running, b.code), (false, Some(130)));
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
    fn clear_that_succeeds_drops_every_block_and_leaves_none_of_its_own() {
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        for cmd in ["ls", "git status"] {
            s.mark(Mark::Command {
                cmd: cmd.into(),
                cwd: "/w".into(),
            });
            s.mark(Mark::Done(Some(0)));
        }
        assert_eq!(s.view().unwrap().2.len(), 2);
        let before = s.version();
        s.mark(Mark::Command {
            cmd: " clear ".into(),
            cwd: "/w".into(),
        });
        s.data(b"\x1b[H\x1b[2J\x1b[3J");
        s.mark(Mark::Done(Some(0)));
        assert!(s.view().unwrap().2.is_empty());
        assert!(s.version() > before, "the clients are told");
        s.mark(Mark::Command {
            cmd: "pwd".into(),
            cwd: "/w".into(),
        });
        s.mark(Mark::Done(Some(0)));
        assert_eq!(
            s.view().unwrap().2[0].id,
            4,
            "ids go on, none is reused after a clear"
        );
    }

    #[test]
    fn clear_with_arguments_or_that_fails_is_a_block_like_any_other() {
        for (cmd, code) in [("clear -x", 0), ("clear", 1), ("clearish", 0)] {
            let mut s = BlockStore::new(40);
            s.mark(Mark::Prompt);
            s.mark(Mark::Command {
                cmd: "ls".into(),
                cwd: "/w".into(),
            });
            s.mark(Mark::Done(Some(0)));
            s.mark(Mark::Command {
                cmd: cmd.into(),
                cwd: "/w".into(),
            });
            s.mark(Mark::Done(Some(code)));
            assert_eq!(s.view().unwrap().2.len(), 2, "{cmd} {code}");
        }
        // One that ended with no status (another command began first) did not finish its work.
        let mut s = BlockStore::new(40);
        s.mark(Mark::Prompt);
        for cmd in ["ls", "clear", "pwd"] {
            s.mark(Mark::Command {
                cmd: cmd.into(),
                cwd: "/w".into(),
            });
        }
        s.mark(Mark::Done(Some(0)));
        assert_eq!(s.view().unwrap().2.len(), 3);
    }

    fn saved(id: u64, cmd: &str, running: bool) -> Block {
        Block {
            id,
            cmd: cmd.into(),
            cwd: "/w".into(),
            started: 1000,
            running,
            code: (!running).then_some(0),
            ms: (!running).then_some(5),
            truncated: false,
            full: false,
            interrupted: false,
            restart: false,
            out: Vec::new(),
        }
    }

    #[test]
    fn a_restored_history_keeps_its_blocks_cuts_the_running_one_and_marks_the_restart() {
        let mut s = BlockStore::new(40);
        s.restore(
            vec![saved(1, "ls", false), saved(7, "cargo watch", true)],
            4000,
            9000,
        );
        let (_, _, blocks) = s.view().expect("a restored history needs no prompt first");
        assert_eq!(blocks.len(), 3);
        assert_eq!((blocks[0].id, blocks[0].interrupted), (1, false));
        let cut = &blocks[1];
        assert_eq!(
            (cut.id, cut.running, cut.interrupted, cut.code, cut.ms),
            (7, false, true, None, Some(3000)),
            "it had run until the daemon last saved"
        );
        let mark = &blocks[2];
        assert_eq!((mark.id, mark.restart, mark.started), (8, true, 9000));
        // The next command goes on after them.
        s.mark(Mark::Command {
            cmd: "pwd".into(),
            cwd: "/w".into(),
        });
        s.mark(Mark::Done(Some(0)));
        assert_eq!(s.view().unwrap().2.last().unwrap().id, 9);
    }

    #[test]
    fn a_second_restart_with_nothing_run_between_does_not_add_a_second_mark() {
        let mut s = BlockStore::new(40);
        s.restore(vec![saved(1, "ls", false)], 2000, 3000);
        let first = s.view().unwrap().2;
        let mut again = BlockStore::new(40);
        again.restore(first.clone(), 4000, 5000);
        assert_eq!(
            again.view().unwrap().2,
            first,
            "the mark that is there says it"
        );
    }

    #[test]
    fn nothing_saved_restores_nothing_and_the_caps_hold_for_what_is() {
        let mut s = BlockStore::new(40);
        s.restore(Vec::new(), 1, 2);
        assert!(
            s.view().is_none(),
            "no integration, no blocks, until a prompt"
        );
        let many: Vec<Block> = (1..=(MAX_BLOCKS as u64 + 30))
            .map(|i| saved(i, "x", false))
            .collect();
        s.restore(many, 1, 2);
        let blocks = s.view().unwrap().2;
        assert_eq!(blocks.len(), MAX_BLOCKS);
        assert!(blocks.last().unwrap().restart);
        assert_eq!(blocks[0].id, 32, "the oldest went first");
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
    fn rows_are_spans_by_style_with_the_blanks_of_the_end_left_out() {
        let mut p = vt100::Parser::new(6, 12, 0);
        // A red run that ends the first row and starts the second: each row has its own span.
        p.process(b"ab\x1b[31mcdefghij\x1b[mkl");
        p.process(b"\x1b[31mxy\x1b[0m   ");
        // A row painted to its edge keeps the blanks that carry a background, a reversed cell with
        // the default colours is index 0 on 7, and a wide character is one cell of text.
        p.process(b"\x1b[3;1H\x1b[44mbar\x1b[0m\x1b[3;12H\x1b[44m \x1b[0m");
        p.process("\x1b[4;1H\x1b[7m NORMAL \x1b[0m\x1b[5;1H日a".as_bytes());
        let rows = rows_of(p.screen());
        let first: Vec<_> = rows[0].iter().map(|s| s.t.as_str()).collect();
        assert_eq!(first, ["ab", "cdefghij", "kl"]);
        assert_eq!(rows[0][1].fg, Some(Color::Index(1)));
        assert_eq!(
            rows[1][0].t, "xy",
            "the style of the end of a row is not carried to the next"
        );
        assert_eq!(rows[1][0].fg, Some(Color::Index(1)));
        assert_eq!(rows[1].len(), 1, "blanks with the default look are dropped");
        let painted: Vec<_> = rows[2]
            .iter()
            .map(|s| (s.t.as_str(), s.bg.clone()))
            .collect();
        assert_eq!(
            painted,
            [
                ("bar", Some(Color::Index(4))),
                ("        ", None),
                (" ", Some(Color::Index(4)))
            ]
        );
        assert_eq!(rows[3][0].t, " NORMAL ");
        assert_eq!(
            (rows[3][0].fg.clone(), rows[3][0].bg.clone()),
            (Some(Color::Index(0)), Some(Color::Index(7)))
        );
        assert_eq!(rows[4][0].t, "日a");
        assert_eq!(rows.len(), 5, "the blank row at the end is dropped");
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

    /// REQ-625: the folder a shell says at its prompt is taken out of the stream, whole or split
    /// between reads, and the store keeps the last one; it is not a block and not screen text.
    #[test]
    fn the_folder_a_shell_says_at_its_prompt_is_taken_out_and_the_last_one_is_kept() {
        let mut sc = Scanner::default();
        let mut s = BlockStore::new(80);
        // `/Users/me/ws` in base64, in two reads and with the string terminator ESC \
        let pieces: Vec<Piece> = [
            &b"~ $ \x1b]633;P;Pwd=L1VzZXJz"[..],
            &b"L21lL3dz\x1b\\rest"[..],
        ]
        .into_iter()
        .flat_map(|chunk| sc.feed(chunk))
        .collect();
        let mut text = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Data(d) => text.extend(d),
                Piece::Mark(m) => s.mark(m),
            }
        }
        assert_eq!(String::from_utf8_lossy(&text), "~ $ rest");
        assert_eq!(s.folder(), Some("/Users/me/ws"));
        assert_eq!(s.view().map(|v| v.2.len()), None, "no block was made");
        // a later one replaces it
        for piece in sc.feed(b"\x1b]633;P;Pwd=Lw==\x07") {
            if let Piece::Mark(m) = piece {
                s.mark(m);
            }
        }
        assert_eq!(s.folder(), Some("/"));
    }

    #[test]
    fn a_second_start_mark_names_the_command_the_first_one_began() {
        let mut s = BlockStore::new(80);
        s.mark(Mark::Prompt);
        s.mark(Mark::Command {
            cmd: String::new(),
            cwd: String::new(),
        });
        s.mark(Mark::Command {
            cmd: "echo hi".into(),
            cwd: "/tmp".into(),
        });
        s.data(b"hi\r\n");
        s.mark(Mark::Done(Some(0)));
        s.mark(Mark::Done(Some(0)));
        let blocks = s.view().unwrap().2;
        assert_eq!(blocks.len(), 1, "one command, one block: {blocks:?}");
        assert_eq!(
            (blocks[0].cmd.as_str(), blocks[0].cwd.as_str()),
            ("echo hi", "/tmp")
        );
        assert_eq!(blocks[0].code, Some(0));
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
