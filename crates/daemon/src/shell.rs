//! A shell's commands as blocks, a command and its output each (mobile REQ-92).
//!
//! The shell tells the daemon where a command starts and ends: jw gives the
//! shells it starts a few lines of its own (zsh: `install`, `prepare`) that
//! print an OSC 7770 sequence before each command (`start`, with the command
//! and the folder, both in base64) and after it (`end`, with its status). The
//! daemon watches the bytes the pane prints for them (`Scanner`) and keeps the
//! output between the two as plain text (`Plain`). A shell that was not started
//! by jw prints none, so it has no blocks and is drawn as text.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::Instant;

use crate::proto::ShellBlock;

/// The blocks kept per pane; older ones are let go.
const KEEP: usize = 200;
/// The blocks a client is told of when it starts watching.
pub const FIRST: usize = 100;
/// The lines of a block's output that are kept, and the characters of a line.
const LINES: usize = 400;
const LINE_MAX: usize = 400;
/// The most bytes of one mark's body that are waited for.
const MARK_MAX: usize = 8 << 10;

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
fi
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  __jw_b64() { printf %s "$1" | base64 | tr -d '\n'; }
  __jw_preexec() { printf '\e]7770;start;%s;%s\a' "$(__jw_b64 "$1")" "$(__jw_b64 "$PWD")"; }
  __jw_precmd() { local s=$?; printf '\e]7770;end;%d\a' "$s"; }
  add-zsh-hook preexec __jw_preexec
  # First, so the status it reads is the command's.
  precmd_functions=(__jw_precmd ${precmd_functions:#__jw_precmd})
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
    Some(vec![
        ("ZDOTDIR".into(), dir.display().to_string()),
        (
            "JW_USER_ZDOTDIR".into(),
            zdotdir
                .filter(|z| !z.is_empty())
                .unwrap_or(home)
                .to_string(),
        ),
    ])
}

/// One thing a pane printed that a block cares about.
#[derive(Debug, PartialEq)]
pub enum Piece {
    /// Bytes of output (the marks are not in them).
    Data(Vec<u8>),
    Start {
        cmd: String,
        cwd: String,
    },
    End {
        status: i32,
    },
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

/// A command's output as plain text: what a terminal would show of it, a line
/// at a time, with no colours or cursor moves (`\r` goes back to the start of
/// the line, so a progress bar ends as its last state).
#[derive(Default)]
struct Plain {
    lines: VecDeque<Vec<char>>,
    /// The line being written and where in it.
    cur: Vec<char>,
    col: usize,
    /// Inside an escape sequence.
    esc: Esc,
    /// Lines that went by, kept or not.
    total: u32,
    /// Bytes of a character that did not end in the last read.
    pending: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Esc {
    #[default]
    None,
    /// After ESC.
    Seen,
    /// In a CSI, to its last byte.
    Csi,
    /// In an OSC or another string, to BEL or ESC `\`.
    Str,
    /// In a string, after ESC.
    StrEnding,
}

impl Plain {
    fn push(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&data) {
            Ok(_) => data.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(e) => e.valid_up_to() + e.error_len().unwrap_or(1),
        };
        let text = String::from_utf8_lossy(&data[..valid]).into_owned();
        self.pending = data[valid..].to_vec();
        for c in text.chars() {
            self.char(c);
        }
    }

    fn char(&mut self, c: char) {
        match self.esc {
            Esc::Seen => {
                self.esc = match c {
                    '[' => Esc::Csi,
                    ']' | 'P' | '_' | '^' | 'X' => Esc::Str,
                    // A two-byte sequence (ESC 7, ESC =…): over.
                    _ => Esc::None,
                };
                return;
            }
            Esc::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.esc = Esc::None;
                }
                return;
            }
            Esc::Str => {
                match c {
                    '\u{7}' => self.esc = Esc::None,
                    '\u{1b}' => self.esc = Esc::StrEnding,
                    _ => {}
                }
                return;
            }
            Esc::StrEnding => {
                self.esc = if c == '\\' { Esc::None } else { Esc::Str };
                return;
            }
            Esc::None => {}
        }
        match c {
            '\u{1b}' => self.esc = Esc::Seen,
            '\n' => self.newline(),
            '\r' => self.col = 0,
            '\u{8}' => self.col = self.col.saturating_sub(1),
            '\t' => {
                let to = (self.col / 8 + 1) * 8;
                while self.col < to {
                    self.put(' ');
                }
            }
            c if c.is_control() => {}
            c => self.put(c),
        }
    }

    fn put(&mut self, c: char) {
        if self.col >= LINE_MAX {
            return;
        }
        if self.col < self.cur.len() {
            self.cur[self.col] = c;
        } else {
            self.cur.push(c);
        }
        self.col += 1;
    }

    fn newline(&mut self) {
        let line = std::mem::take(&mut self.cur);
        self.lines.push_back(line);
        if self.lines.len() > LINES {
            self.lines.pop_front();
        }
        self.total += 1;
        self.col = 0;
    }

    /// The lines so far, the one being written included, without the blank ones at the end.
    fn text(&self) -> (String, u32) {
        let mut all: Vec<String> = self
            .lines
            .iter()
            .map(|l| l.iter().collect::<String>().trim_end().to_string())
            .collect();
        let cur: String = self.cur.iter().collect::<String>().trim_end().to_string();
        let mut total = self.total;
        if !cur.is_empty() {
            all.push(cur);
            total += 1;
        }
        while all.last().is_some_and(String::is_empty) {
            all.pop();
            total = total.saturating_sub(1);
        }
        (all.join("\n"), total)
    }
}

/// The blocks of one pane.
#[derive(Default)]
pub struct Blocks {
    scanner: Scanner,
    list: VecDeque<Block>,
    /// The number the next block gets.
    next: u32,
    /// The block whose command is running.
    running: Option<Plain>,
    /// A mark has been seen: the shell is one of jw's.
    seen: bool,
    /// What the watchers were told so far.
    told_seen: bool,
    /// The numbers of the blocks changed since they were last told of.
    changed: Vec<u32>,
}

struct Block {
    i: u32,
    cmd: String,
    cwd: String,
    out: String,
    lines: u32,
    code: Option<i32>,
    started: Instant,
    ms: u64,
    running: bool,
    /// The program drew a full screen: no output is kept for it.
    full: bool,
}

impl Block {
    fn wire(&self) -> ShellBlock {
        ShellBlock {
            i: self.i,
            cmd: self.cmd.clone(),
            cwd: self.cwd.clone(),
            out: self.out.clone(),
            lines: self.lines,
            code: self.code,
            ms: if self.running {
                self.started.elapsed().as_millis() as u64
            } else {
                self.ms
            },
            running: self.running,
            full: self.full,
        }
    }
}

impl Blocks {
    /// Whether the shell has marked a command yet.
    pub fn known(&self) -> bool {
        self.seen
    }

    /// What the pane printed; `alt` says its screen is the alternate one now.
    pub fn feed(&mut self, bytes: &[u8], alt: bool) {
        for piece in self.scanner.feed(bytes) {
            match piece {
                Piece::Data(d) => {
                    if let (Some(plain), Some(b)) = (&mut self.running, self.list.back_mut())
                        && b.running
                        && !b.full
                    {
                        if alt {
                            b.full = true;
                            b.out.clear();
                            *plain = Plain::default();
                        } else {
                            plain.push(&d);
                        }
                        let i = b.i;
                        if !self.changed.contains(&i) {
                            self.changed.push(i);
                        }
                    }
                }
                Piece::Start { cmd, cwd } => {
                    self.seen = true;
                    self.finish(None);
                    let i = self.next;
                    self.next += 1;
                    self.list.push_back(Block {
                        i,
                        cmd: cap(&cmd, 2000),
                        cwd,
                        out: String::new(),
                        lines: 0,
                        code: None,
                        started: Instant::now(),
                        ms: 0,
                        running: true,
                        full: false,
                    });
                    if self.list.len() > KEEP {
                        self.list.pop_front();
                    }
                    self.running = Some(Plain::default());
                    self.changed.push(i);
                }
                Piece::End { status } => {
                    self.seen = true;
                    self.finish(Some(status));
                }
            }
        }
    }

    /// Ends the block that runs, with its status (`None`: it never said).
    fn finish(&mut self, status: Option<i32>) {
        let Some(plain) = self.running.take() else {
            return;
        };
        if let Some(b) = self.list.back_mut()
            && b.running
        {
            if !b.full {
                let (out, lines) = plain.text();
                b.out = out;
                b.lines = lines;
            }
            b.running = false;
            b.code = status;
            b.ms = b.started.elapsed().as_millis() as u64;
            if !self.changed.contains(&b.i) {
                self.changed.push(b.i);
            }
        }
    }

    /// The blocks to tell the watchers of: (`reset`, blocks). All of them when
    /// the shell has just shown itself to be jw's, else the ones that changed;
    /// `None` when there is nothing.
    pub fn news(&mut self) -> Option<(bool, Vec<ShellBlock>)> {
        if self.seen && !self.told_seen {
            self.told_seen = true;
            self.changed.clear();
            self.refresh();
            return Some((true, self.all(FIRST)));
        }
        if self.changed.is_empty() {
            return None;
        }
        self.refresh();
        let changed = std::mem::take(&mut self.changed);
        let blocks: Vec<ShellBlock> = self
            .list
            .iter()
            .filter(|b| changed.contains(&b.i))
            .map(Block::wire)
            .collect();
        (!blocks.is_empty()).then_some((false, blocks))
    }

    /// The running block's output so far.
    fn refresh(&mut self) {
        if let (Some(plain), Some(b)) = (&self.running, self.list.back_mut())
            && b.running
            && !b.full
        {
            let (out, lines) = plain.text();
            b.out = out;
            b.lines = lines;
        }
    }

    /// The last `n` blocks, for a client that starts watching.
    pub fn all(&mut self, n: usize) -> Vec<ShellBlock> {
        self.refresh();
        self.list
            .iter()
            .skip(self.list.len().saturating_sub(n))
            .map(Block::wire)
            .collect()
    }
}

fn cap(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
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
    fn plain_text_has_no_colours_and_a_carriage_return_overwrites() {
        let mut p = Plain::default();
        p.push(b"\x1b[32mok\x1b[0m line\r\nprogress 10%\rprogress 100%\r\n\x1b]0;t\x07done");
        let (text, lines) = p.text();
        assert_eq!(text, "ok line\nprogress 100%\ndone");
        assert_eq!(lines, 3);
        // A character cut between two reads is whole.
        let mut p = Plain::default();
        let bytes = "漢".as_bytes();
        p.push(&bytes[..1]);
        p.push(&bytes[1..]);
        assert_eq!(p.text().0, "漢");
    }

    #[test]
    fn a_command_is_a_block_with_its_output_and_status() {
        let mut b = Blocks::default();
        assert!(!b.known());
        b.feed(b"prompt % ", false);
        b.feed(&start("cargo test", "/w"), false);
        b.feed(b"running 2 tests\r\ntest result: ok\r\n", false);
        // Running: the output so far.
        let (reset, blocks) = b.news().unwrap();
        assert!(reset, "the first mark shows the shell is jw's");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].running);
        assert_eq!(blocks[0].out, "running 2 tests\ntest result: ok");
        b.feed(&end(0), false);
        let (reset, blocks) = b.news().unwrap();
        assert!(!reset);
        assert_eq!((blocks[0].code, blocks[0].running), (Some(0), false));
        assert_eq!(blocks[0].cwd, "/w");
        assert_eq!(b.news(), None);
        // A failing one, then the list.
        b.feed(&start("false", "/w"), false);
        b.feed(&end(1), false);
        let all = b.all(10);
        assert_eq!(all.iter().map(|b| b.i).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(all[1].code, Some(1));
    }

    #[test]
    fn a_program_that_draws_the_whole_screen_keeps_no_output() {
        let mut b = Blocks::default();
        b.feed(&start("nvim a", "/w"), false);
        b.feed(b"\x1b[?1049h\x1b[2J garbage", true);
        b.feed(&end(0), true);
        let blocks = b.all(1);
        assert!(blocks[0].full);
        assert_eq!(blocks[0].out, "");
    }

    #[test]
    fn a_start_with_no_end_closes_the_one_before() {
        let mut b = Blocks::default();
        b.feed(&start("one", "/w"), false);
        b.feed(&start("two", "/w"), false);
        let all = b.all(10);
        assert_eq!((all[0].running, all[0].code), (false, None));
        assert!(all[1].running);
    }

    #[test]
    fn only_zsh_is_given_the_integration() {
        let env = prepare("/bin/zsh", Path::new("/i"), None, "/home/u").unwrap();
        assert_eq!(
            env,
            [
                ("ZDOTDIR".to_string(), "/i".to_string()),
                ("JW_USER_ZDOTDIR".to_string(), "/home/u".to_string())
            ]
        );
        // The user's own ZDOTDIR is kept for the files to read.
        let env = prepare(
            "-zsh",
            Path::new("/i"),
            Some("/home/u/.config/zsh"),
            "/home/u",
        )
        .unwrap();
        assert_eq!(env[1].1, "/home/u/.config/zsh");
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
