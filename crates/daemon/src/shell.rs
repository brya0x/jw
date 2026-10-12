//! A shell's commands as blocks, a command and its output each, like Warp's (mobile REQ-92).
//!
//! jw gives the zsh it starts for a pane a few lines of its own (`install`,
//! `prepare`) that print an OSC 7770 sequence: `start` before each command (with
//! the command and the folder, in base64), `end` (with the status) before each
//! prompt, and `line` (the text being typed, in base64) on every redraw of the
//! line. The daemon watches the bytes the pane prints for them (`Scanner`) and keeps
//! what a command printed between its `start` and its `end` as raw bytes, apart from
//! the screen: a phone parses each block's bytes into a terminal of its own, so a
//! closed block never changes and the screen is not needed. A shell that was not
//! started by jw prints none, so it is not `known` and is drawn as text.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::Instant;

use crate::proto::ShellBlock;

/// The most bytes of one mark's body that are waited for.
const MARK_MAX: usize = 8 << 10;
/// The blocks kept per pane, the bytes of output kept of one, and of all of them.
const KEEP: usize = 200;
const BLOCK_MAX: usize = 512 << 10;
const TOTAL_MAX: usize = 16 << 20;
/// The most bytes of output a client that asks is sent at once, newest blocks first.
const REPLAY_MAX: usize = 4 << 20;

const PREFIX: &[u8] = b"\x1b]7770;";

/// What the shell prints before and after each command, for zsh. `.zshenv`,
/// `.zprofile` and `.zshrc` of the folder jw points `ZDOTDIR` at read the user's
/// own, so nothing of theirs is lost; the hooks come last, after whatever
/// their prompt does.
const ZSHENV: &str = r#"# jw's shell integration (mobile REQ-92): reads your own .zshenv.
if [[ -n "$JW_USER_ZDOTDIR" ]]; then
  __jw_z="$ZDOTDIR"; ZDOTDIR="$JW_USER_ZDOTDIR"
  [[ -f "$ZDOTDIR/.zshenv" ]] && . "$ZDOTDIR/.zshenv"
  JW_USER_ZDOTDIR="$ZDOTDIR"; ZDOTDIR="$__jw_z"; unset __jw_z
fi
"#;

const ZPROFILE: &str = r#"# jw's shell integration (mobile REQ-92): reads your own .zprofile.
if [[ -n "$JW_USER_ZDOTDIR" ]]; then
  __jw_z="$ZDOTDIR"; ZDOTDIR="$JW_USER_ZDOTDIR"
  [[ -f "$ZDOTDIR/.zprofile" ]] && . "$ZDOTDIR/.zprofile"
  JW_USER_ZDOTDIR="$ZDOTDIR"; ZDOTDIR="$__jw_z"; unset __jw_z
fi
"#;

const ZSHRC: &str = r#"# jw's shell integration (mobile REQ-92): reads your own .zshrc, then marks each command.
if [[ -n "$JW_USER_ZDOTDIR" ]]; then
  ZDOTDIR="$JW_USER_ZDOTDIR"; unset JW_USER_ZDOTDIR
  [[ -f "$ZDOTDIR/.zshrc" ]] && . "$ZDOTDIR/.zshrc"
  # The shell goes on as it was: with no ZDOTDIR of its own, none.
  if [[ -n "$JW_NO_ZDOTDIR" ]]; then unset ZDOTDIR JW_NO_ZDOTDIR; fi
fi
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  __jw_b64() { printf %s "$1" | base64 | tr -d '\n'; }
  __jw_preexec() { printf '\e]7770;start;%s;%s\a' "$(__jw_b64 "$1")" "$(__jw_b64 "$PWD")"; }
  __jw_precmd() { local s=$?; printf '\e]7770;end;%d\a' "$s"; }
  add-zsh-hook preexec __jw_preexec
  # First, so the status it reads is the command's, and the mark comes where the prompt starts.
  precmd_functions=(__jw_precmd ${precmd_functions:#__jw_precmd})
  # The line being typed, so a phone's input and the terminal's are one text.
  __jw_last_line=$'\x01'
  __jw_line() {
    [[ "$BUFFER" == "$__jw_last_line" ]] && return
    __jw_last_line="$BUFFER"
    printf '\e]7770;line;%s\a' "$(__jw_b64 "$BUFFER")"
  }
  autoload -Uz add-zle-hook-widget
  add-zle-hook-widget line-pre-redraw __jw_line
  add-zle-hook-widget line-finish __jw_line
fi
"#;

/// Writes the files zsh is pointed at into `dir`.
pub fn install(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in [
        (".zshenv", ZSHENV),
        (".zprofile", ZPROFILE),
        (".zshrc", ZSHRC),
    ] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
            std::fs::write(path, body)?;
        }
    }
    Ok(())
}

/// What a shell started for a pane needs to mark its commands: for zsh, the
/// folder of `install`, and where the user's own files are. `None` for any
/// other shell, which is left as it is.
pub fn prepare(
    shell: &str,
    dir: &Path,
    zdotdir: Option<&str>,
    home: &str,
) -> Option<Vec<(String, String)>> {
    let name = shell
        .rsplit('/')
        .next()
        .unwrap_or(shell)
        .trim_start_matches('-');
    if name != "zsh" {
        return None;
    }
    let own = zdotdir.filter(|z| !z.is_empty());
    let mut vars = vec![
        ("ZDOTDIR".into(), dir.display().to_string()),
        ("JW_USER_ZDOTDIR".into(), own.unwrap_or(home).to_string()),
    ];
    if own.is_none() {
        // The user has none: the shell must not be left with one.
        vars.push(("JW_NO_ZDOTDIR".into(), "1".into()));
    }
    Some(vars)
}

/// One thing a pane printed that the shell's state cares about.
#[derive(Debug, PartialEq)]
pub enum Piece {
    /// Bytes of output (the marks are not in them).
    Data(Vec<u8>),
    /// A command starts: what was typed and the folder it runs in.
    Start { cmd: String, cwd: String },
    /// A command ended; the prompt is drawn next.
    End { status: i32 },
    /// The line being typed at the prompt.
    Line { text: String },
}

/// Finds the marks in a pane's output, across the reads it comes in.
#[derive(Default)]
pub struct Scanner {
    /// The end of the last read, which may be the start of a mark.
    partial: Vec<u8>,
}

impl Scanner {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut buf = std::mem::take(&mut self.partial);
        buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let Some(i) = find(&buf[at..], PREFIX).map(|i| at + i) else {
                // The last bytes may be the start of a mark: they wait.
                let keep = (1..PREFIX.len())
                    .rev()
                    .find(|n| buf[at..].ends_with(&PREFIX[..*n]))
                    .unwrap_or(0);
                let end = buf.len() - keep;
                if end > at {
                    out.push(Piece::Data(buf[at..end].to_vec()));
                }
                self.partial = buf[end..].to_vec();
                return out;
            };
            if i > at {
                out.push(Piece::Data(buf[at..i].to_vec()));
            }
            let body = i + PREFIX.len();
            let Some((len, after)) = terminator(&buf[body..]) else {
                if buf.len() - i > MARK_MAX {
                    // Not a mark of ours: it is output.
                    out.push(Piece::Data(buf[i..].to_vec()));
                    return out;
                }
                self.partial = buf[i..].to_vec();
                return out;
            };
            if let Some(piece) = parse(&buf[body..body + len]) {
                out.push(piece);
            }
            at = body + after;
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Where a mark's body ends (BEL, or ESC `\`) and where what follows starts.
fn terminator(b: &[u8]) -> Option<(usize, usize)> {
    for (i, c) in b.iter().enumerate() {
        match c {
            0x07 => return Some((i, i + 1)),
            0x1b if b.get(i + 1) == Some(&b'\\') => return Some((i, i + 2)),
            _ => {}
        }
    }
    None
}

fn parse(body: &[u8]) -> Option<Piece> {
    let body = std::str::from_utf8(body).ok()?;
    let mut parts = body.split(';');
    match parts.next()? {
        "start" => Some(Piece::Start {
            cmd: decode(parts.next()?)?,
            cwd: decode(parts.next()?)?,
        }),
        "end" => Some(Piece::End {
            status: parts.next()?.trim().parse().ok()?,
        }),
        "line" => Some(Piece::Line {
            text: decode(parts.next().unwrap_or(""))?,
        }),
        _ => None,
    }
}

/// Standard base64, as the shell's `base64` writes it, to text.
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What the watchers of a shell are told, when it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    /// The shell is one of jw's (it has marked something).
    pub known: bool,
    /// A command runs: what is typed goes to it, not to the prompt.
    pub running: bool,
    /// The line being typed at the prompt.
    pub input: String,
}

/// What is new in a shell since its watchers were told: its state, the blocks that changed or are
/// new, and the bytes of output that came, by block.
#[derive(Debug, Clone, PartialEq)]
pub struct News {
    pub told: Told,
    pub blocks: Vec<ShellBlock>,
    pub outs: Vec<(u32, Vec<u8>)>,
}

struct Block {
    i: u32,
    cmd: String,
    cwd: String,
    code: Option<i32>,
    started: Instant,
    ms: u64,
    running: bool,
    /// A program drew the whole screen: no output is kept for it.
    full: bool,
    /// The start of the output was let go: it passed [`BLOCK_MAX`].
    truncated: bool,
    out: Vec<u8>,
}

impl Block {
    fn wire(&self) -> ShellBlock {
        ShellBlock {
            i: self.i,
            cmd: self.cmd.clone(),
            cwd: self.cwd.clone(),
            code: self.code,
            ms: if self.running {
                self.started.elapsed().as_millis() as u64
            } else {
                self.ms
            },
            running: self.running,
            full: self.full,
            truncated: self.truncated,
        }
    }
}

/// The state of one pane's shell and the blocks of its commands.
#[derive(Default)]
pub struct ShellState {
    scanner: Scanner,
    seen: bool,
    input: String,
    blocks: VecDeque<Block>,
    /// The number the next block gets.
    next: u32,
    /// The bytes of output held, of all the blocks.
    held: usize,
    told: Option<Told>,
    /// The numbers of the blocks changed since the watchers were told.
    changed: Vec<u32>,
    /// The output that came since the watchers were told, by block, in order.
    fresh: Vec<(u32, Vec<u8>)>,
}

impl ShellState {
    /// The marks and the output in what the pane printed.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<Piece> {
        self.scanner.feed(bytes)
    }

    fn running_block(&mut self) -> Option<&mut Block> {
        self.blocks.back_mut().filter(|b| b.running)
    }

    fn touch(&mut self, i: u32) {
        if !self.changed.contains(&i) {
            self.changed.push(i);
        }
    }

    /// A command starts.
    pub fn start(&mut self, cmd: String, cwd: String) {
        self.seen = true;
        self.finish(None);
        let i = self.next;
        self.next += 1;
        self.blocks.push_back(Block {
            i,
            cmd: cmd.chars().take(4000).collect(),
            cwd,
            code: None,
            started: Instant::now(),
            ms: 0,
            running: true,
            full: false,
            truncated: false,
            out: Vec::new(),
        });
        self.touch(i);
        while self.blocks.len() > KEEP || (self.held > TOTAL_MAX && self.blocks.len() > 1) {
            if let Some(old) = self.blocks.pop_front() {
                self.held -= old.out.len();
            }
        }
    }

    /// What a running command printed; `alt` says the screen is the alternate one now, which a
    /// block does not hold.
    pub fn data(&mut self, bytes: &[u8], alt: bool) {
        let Some(b) = self.blocks.back_mut().filter(|b| b.running) else {
            return;
        };
        if b.full {
            return;
        }
        let i = b.i;
        if alt {
            // A program that draws the whole screen: its escape codes are not a block's output.
            b.full = true;
            let dropped = std::mem::take(&mut b.out).len();
            self.held -= dropped;
            self.fresh.retain(|(n, _)| *n != i);
            self.touch(i);
            return;
        }
        b.out.extend_from_slice(bytes);
        self.held += bytes.len();
        if b.out.len() > BLOCK_MAX {
            let cut = b.out.len() - BLOCK_MAX;
            b.out.drain(..cut);
            b.truncated = true;
            self.held -= cut.min(self.held);
        }
        match self.fresh.last_mut() {
            Some((n, out)) if *n == i => out.extend_from_slice(bytes),
            _ => self.fresh.push((i, bytes.to_vec())),
        }
        self.touch(i);
    }

    /// A command ended, with `status`; the prompt is drawn next.
    pub fn end(&mut self, status: i32) {
        self.seen = true;
        self.finish(Some(status));
    }

    /// Ends the block that runs, with its status (`None`: it never said).
    fn finish(&mut self, status: Option<i32>) {
        let Some(b) = self.running_block() else {
            return;
        };
        let before = b.out.len();
        trim_eol_mark(&mut b.out);
        let cut = before - b.out.len();
        b.running = false;
        b.code = status;
        b.ms = b.started.elapsed().as_millis() as u64;
        let i = b.i;
        self.held -= cut.min(self.held);
        self.touch(i);
    }

    pub fn line(&mut self, text: String) {
        self.seen = true;
        self.input = text.chars().take(4000).collect();
    }

    pub fn state(&self) -> Told {
        Told {
            known: self.seen,
            running: self.blocks.back().is_some_and(|b| b.running),
            input: self.input.clone(),
        }
    }

    /// What changed since the watchers were told, or `None`.
    pub fn news(&mut self) -> Option<News> {
        let now = self.state();
        let moved = self.told.as_ref() != Some(&now);
        if !moved && self.changed.is_empty() && self.fresh.is_empty() {
            return None;
        }
        self.told = Some(now.clone());
        let changed = std::mem::take(&mut self.changed);
        let blocks = self
            .blocks
            .iter()
            .filter(|b| changed.contains(&b.i))
            .map(Block::wire)
            .collect();
        Some(News {
            told: now,
            blocks,
            outs: std::mem::take(&mut self.fresh),
        })
    }

    /// Everything, for a client that has just asked (and so counts as told): every block and the output
    /// of the newest ones that fit in [`REPLAY_MAX`], oldest first.
    pub fn replay(&mut self) -> News {
        let now = self.state();
        self.told = Some(now.clone());
        self.changed.clear();
        self.fresh.clear();
        let mut room = REPLAY_MAX;
        let mut outs = Vec::new();
        for b in self.blocks.iter().rev() {
            if b.out.is_empty() || b.out.len() > room {
                continue;
            }
            room -= b.out.len();
            outs.push((b.i, b.out.clone()));
        }
        outs.reverse();
        News {
            told: now,
            blocks: self.blocks.iter().map(Block::wire).collect(),
            outs,
        }
    }
}

/// zsh prints a reverse `%` and enough spaces to reach the end of the line before the prompt
/// when a command's output did not end with a newline; that is the prompt's, not the block's.
fn trim_eol_mark(out: &mut Vec<u8>) {
    const MARK: &[u8] = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m";
    let tail = out.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    if let Some(at) = find(&out[tail..], MARK) {
        out.truncate(tail + at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        // The few characters the tests use.
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in s.as_bytes().chunks(3) {
            let n =
                chunk.iter().fold(0u32, |a, b| a << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
            for k in 0..chunk.len() + 1 {
                out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            }
            for _ in chunk.len() + 1..4 {
                out.push('=');
            }
        }
        out
    }

    fn start(cmd: &str, cwd: &str) -> Vec<u8> {
        format!("\x1b]7770;start;{};{}\x07", b64(cmd), b64(cwd)).into_bytes()
    }

    fn end(status: i32) -> Vec<u8> {
        format!("\x1b]7770;end;{status}\x07").into_bytes()
    }

    fn line(text: &str) -> Vec<u8> {
        format!("\x1b]7770;line;{}\x07", b64(text)).into_bytes()
    }

    /// What a pane printed, taken through the scanner into the state as the daemon does.
    fn feed(s: &mut ShellState, alt: bool, bytes: &[u8]) {
        for piece in s.scan(bytes) {
            match piece {
                Piece::Data(d) => s.data(&d, alt),
                Piece::Start { cmd, cwd } => s.start(cmd, cwd),
                Piece::End { status } => s.end(status),
                Piece::Line { text } => s.line(text),
            }
        }
    }

    #[test]
    fn base64_is_read_back() {
        for s in ["", "a", "ab", "abc", "git status -sb", "echo \"hi\"; ls"] {
            assert_eq!(decode(&b64(s)).as_deref(), Some(s), "{s:?}");
        }
        assert_eq!(decode("***"), None);
    }

    #[test]
    fn marks_are_found_across_reads_and_output_is_left_whole() {
        let mut sc = Scanner::default();
        let mark = start("ls", "/w");
        // The mark is cut in two reads, in the middle of its start and of its body.
        let mut got = sc.feed(&[b"before".as_slice(), &mark[..4]].concat());
        got.extend(sc.feed(&mark[4..12]));
        got.extend(sc.feed(&[&mark[12..], b"out\x1b[1mbold\x1b[0m".as_slice()].concat()));
        let data: Vec<u8> = got
            .iter()
            .filter_map(|p| {
                if let Piece::Data(d) = p {
                    Some(d.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(data, b"beforeout\x1b[1mbold\x1b[0m");
        assert!(got.contains(&Piece::Start {
            cmd: "ls".into(),
            cwd: "/w".into()
        }));
        // A mark ended by ESC `\`, and one of another program's OSC, are told apart.
        let mut sc = Scanner::default();
        let got = sc.feed(b"\x1b]7770;end;3\x1b\\\x1b]0;title\x07x");
        assert_eq!(got[0], Piece::End { status: 3 });
        assert_eq!(got[1], Piece::Data(b"\x1b]0;title\x07x".to_vec()));
    }

    #[test]
    fn a_command_is_a_block_with_its_output_as_raw_bytes_and_its_status() {
        let mut s = ShellState::default();
        assert!(!s.state().known);
        // What is printed at the prompt is no block's.
        feed(&mut s, false, b"prompt % ");
        feed(&mut s, false, &start("cargo test", "/w"));
        feed(&mut s, false, b"\x1b[32mrunning\x1b[0m 2 tests\r\n");
        let n = s.news().unwrap();
        assert!(n.told.known && n.told.running);
        assert_eq!(n.blocks.len(), 1);
        assert_eq!(
            (n.blocks[0].cmd.as_str(), n.blocks[0].cwd.as_str()),
            ("cargo test", "/w")
        );
        assert!(n.blocks[0].running);
        // Colours and all: the bytes as the program printed them.
        assert_eq!(
            n.outs,
            [(0, b"\x1b[32mrunning\x1b[0m 2 tests\r\n".to_vec())]
        );
        feed(&mut s, false, b"test result: ok\r\n");
        feed(&mut s, false, &end(0));
        let n = s.news().unwrap();
        assert!(!n.told.running);
        assert_eq!((n.blocks[0].code, n.blocks[0].running), (Some(0), false));
        // Only what came since: the bytes are not sent twice.
        assert_eq!(n.outs, [(0, b"test result: ok\r\n".to_vec())]);
        assert_eq!(s.news(), None);
        // A failing one.
        feed(&mut s, false, &start("false", "/w"));
        feed(&mut s, false, &end(1));
        let n = s.news().unwrap();
        assert_eq!(
            n.blocks.iter().map(|b| (b.i, b.code)).collect::<Vec<_>>(),
            [(1, Some(1))]
        );
    }

    #[test]
    fn the_prompts_reverse_percent_that_follows_output_with_no_newline_is_not_the_blocks() {
        let mut s = ShellState::default();
        feed(&mut s, false, &start("printf hi", "/w"));
        // zsh: the mark, 80 spaces to the end of the line and a carriage return, all before the prompt.
        feed(
            &mut s,
            false,
            b"hi\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m                    \r \r",
        );
        feed(&mut s, false, &end(0));
        let n = s.replay();
        assert_eq!(n.outs, [(0, b"hi".to_vec())]);
        // Output that ends with a newline is kept whole.
        feed(&mut s, false, &start("echo", "/w"));
        feed(&mut s, false, b"x\r\n");
        feed(&mut s, false, &end(0));
        assert_eq!(s.replay().outs[1], (1, b"x\r\n".to_vec()));
    }

    #[test]
    fn a_program_that_draws_the_whole_screen_keeps_no_output() {
        let mut s = ShellState::default();
        feed(&mut s, false, &start("nvim a", "/w"));
        feed(&mut s, false, b"before the screen changed");
        feed(&mut s, true, b"\x1b[?1049h\x1b[2J garbage");
        feed(&mut s, true, &end(0));
        let n = s.replay();
        assert!(n.blocks[0].full);
        assert_eq!(n.outs, []);
    }

    #[test]
    fn a_start_with_no_end_closes_the_one_before() {
        let mut s = ShellState::default();
        feed(&mut s, false, &start("one", "/w"));
        feed(&mut s, false, &start("two", "/w"));
        let n = s.replay();
        assert_eq!((n.blocks[0].running, n.blocks[0].code), (false, None));
        assert!(n.blocks[1].running);
    }

    #[test]
    fn the_output_of_a_block_past_its_cap_keeps_its_end_and_says_so() {
        let mut s = ShellState::default();
        feed(&mut s, false, &start("yes", "/w"));
        let chunk = vec![b'y'; 100 << 10];
        for _ in 0..8 {
            feed(&mut s, false, &chunk);
        }
        let n = s.replay();
        assert!(n.blocks[0].truncated);
        assert_eq!(n.outs[0].1.len(), BLOCK_MAX);
        // The bytes held count what is kept, not what went by.
        assert_eq!(s.held, BLOCK_MAX);
    }

    #[test]
    fn old_blocks_are_let_go_past_the_count_and_the_total() {
        let mut s = ShellState::default();
        for n in 0..KEEP + 5 {
            feed(&mut s, false, &start(&format!("c{n}"), "/w"));
            feed(&mut s, false, &end(0));
        }
        let all = s.replay().blocks;
        assert_eq!(all.len(), KEEP);
        assert_eq!(all[0].cmd, "c5");
        // The numbers go on: a block keeps its number.
        assert_eq!(all[0].i, 5);
    }

    #[test]
    fn a_client_that_asks_is_sent_the_newest_output_that_fits_oldest_first() {
        let mut s = ShellState::default();
        for n in 0..3 {
            feed(&mut s, false, &start(&format!("c{n}"), "/w"));
            feed(&mut s, false, format!("out{n}\r\n").as_bytes());
            feed(&mut s, false, &end(0));
        }
        let n = s.replay();
        assert_eq!(n.blocks.len(), 3);
        assert_eq!(
            n.outs,
            [
                (0, b"out0\r\n".to_vec()),
                (1, b"out1\r\n".to_vec()),
                (2, b"out2\r\n".to_vec())
            ]
        );
        // That counts as told.
        assert_eq!(s.news(), None);
    }

    #[test]
    fn the_line_being_typed_and_the_state_are_news_when_they_change() {
        let mut s = ShellState::default();
        assert_eq!(s.news().map(|n| n.told.known), Some(false));
        assert_eq!(s.news(), None);
        feed(&mut s, false, &line("ls"));
        let n = s.news().unwrap();
        assert_eq!((n.told.known, n.told.input.as_str()), (true, "ls"));
        assert!(n.blocks.is_empty() && n.outs.is_empty());
        assert_eq!(s.news(), None);
        // Typing more is news; the same line again is not.
        feed(&mut s, false, &line("ls -l"));
        assert_eq!(s.news().unwrap().told.input, "ls -l");
        feed(&mut s, false, &line("ls -l"));
        assert_eq!(s.news(), None);
        // A command starting is news, and so is the line cleared after.
        feed(&mut s, false, &start("ls -l", "/w"));
        assert!(s.news().unwrap().told.running);
        feed(&mut s, false, &end(0));
        feed(&mut s, false, &line(""));
        let n = s.news().unwrap();
        assert_eq!((n.told.running, n.told.input.as_str()), (false, ""));
    }

    #[test]
    fn only_zsh_is_given_the_integration() {
        let env = prepare("/bin/zsh", Path::new("/i"), None, "/home/u").unwrap();
        assert_eq!(
            env,
            [
                ("ZDOTDIR".to_string(), "/i".to_string()),
                ("JW_USER_ZDOTDIR".to_string(), "/home/u".to_string()),
                // The user has no ZDOTDIR: the shell is not left with one.
                ("JW_NO_ZDOTDIR".to_string(), "1".to_string()),
            ]
        );
        // The user's own ZDOTDIR is kept for the files to read, and kept.
        let env = prepare(
            "-zsh",
            Path::new("/i"),
            Some("/home/u/.config/zsh"),
            "/home/u",
        )
        .unwrap();
        assert_eq!(env[1].1, "/home/u/.config/zsh");
        assert_eq!(env.len(), 2);
        assert_eq!(prepare("/bin/bash", Path::new("/i"), None, "/h"), None);
        assert_eq!(prepare("/usr/bin/fish", Path::new("/i"), None, "/h"), None);
    }

    #[test]
    fn the_files_are_written_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).unwrap();
        for f in [".zshenv", ".zprofile", ".zshrc"] {
            assert!(dir.path().join(f).is_file(), "{f}");
        }
        assert!(
            std::fs::read_to_string(dir.path().join(".zshrc"))
                .unwrap()
                .contains("7770;start")
        );
        install(dir.path()).unwrap();
    }
}
