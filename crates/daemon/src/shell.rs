//! What a shell tells the daemon about itself (mobile REQ-92): whether it is at its
//! prompt or runs a command, the line being typed, and where its prompt starts on
//! the screen.
//!
//! jw gives the zsh it starts for a pane a few lines of its own (`install`,
//! `prepare`) that print an OSC 7770 sequence: `start` before each command, `end`
//! (with the status) before each prompt, and `line` (the text being typed, in
//! base64) on every redraw of the line. The daemon watches the bytes the pane
//! prints for them (`Scanner`). A shell that was not started by jw prints none,
//! so it is not `known` and is drawn as text as it is.

use std::io;
use std::path::Path;

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
  # The shell goes on as it was: with no ZDOTDIR of its own, none.
  if [[ -n "$JW_NO_ZDOTDIR" ]]; then unset ZDOTDIR JW_NO_ZDOTDIR; fi
fi
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  __jw_b64() { printf %s "$1" | base64 | tr -d '\n'; }
  __jw_preexec() { printf '\e]7770;start\a'; }
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
    /// A command starts.
    Start,
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
        "start" => Some(Piece::Start),
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

/// The state of one pane's shell.
#[derive(Default)]
pub struct ShellState {
    scanner: Scanner,
    seen: bool,
    running: bool,
    input: String,
    /// Where the prompt on screen starts, counted from the first row the pane ever
    /// printed (the rows that scrolled away plus its row on the screen), so it keeps its place as the
    /// screen scrolls; `None` while a command runs.
    prompt: Option<usize>,
    told: Option<Told>,
}

impl ShellState {
    /// The marks and the output in what the pane printed.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<Piece> {
        self.scanner.feed(bytes)
    }

    pub fn start(&mut self) {
        self.seen = true;
        self.running = true;
        self.prompt = None;
    }

    /// A command ended and the prompt starts at the cursor: `scrolled` rows have scrolled
    /// away and the cursor is on screen row `row`.
    pub fn end(&mut self, scrolled: usize, row: u16) {
        self.seen = true;
        self.running = false;
        self.prompt = Some(scrolled + usize::from(row));
    }

    pub fn line(&mut self, text: String) {
        self.seen = true;
        self.input = text.chars().take(4000).collect();
    }

    pub fn state(&self) -> Told {
        Told {
            known: self.seen,
            running: self.running,
            input: self.input.clone(),
        }
    }

    /// The screen row the prompt starts at, while the shell is at its prompt, when `scrolled` rows
    /// have scrolled away: what a phone leaves out of the shell's text.
    pub fn prompt_row(&self, scrolled: usize) -> Option<u16> {
        self.prompt
            .filter(|_| self.seen && !self.running)
            .map(|p| p.saturating_sub(scrolled).min(usize::from(u16::MAX)) as u16)
    }

    /// What changed since the watchers were told, or `None`.
    pub fn news(&mut self) -> Option<Told> {
        let now = self.state();
        (self.told.as_ref() != Some(&now)).then(|| {
            self.told = Some(now.clone());
            now
        })
    }

    /// Takes it as told: what a client that has just asked is sent in full.
    pub fn tell(&mut self) -> Told {
        let now = self.state();
        self.told = Some(now.clone());
        now
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

    fn line(text: &str) -> Vec<u8> {
        format!("\x1b]7770;line;{}\x07", b64(text)).into_bytes()
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
        let mark = line("ls");
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
        assert!(got.contains(&Piece::Line { text: "ls".into() }));
        // A mark ended by ESC `\`, and one of another program's OSC, are told apart.
        let mut sc = Scanner::default();
        let got = sc.feed(b"\x1b]7770;end;3\x1b\\\x1b]0;title\x07x\x1b]7770;start\x07");
        assert_eq!(got[0], Piece::End { status: 3 });
        assert_eq!(got[1], Piece::Data(b"\x1b]0;title\x07x".to_vec()));
        assert_eq!(got[2], Piece::Start);
    }

    #[test]
    fn a_shell_is_known_once_it_marks_something_and_runs_between_start_and_end() {
        let mut s = ShellState::default();
        assert!(!s.state().known);
        s.end(0, 3);
        assert_eq!((s.state().known, s.state().running), (true, false));
        s.start();
        assert!(s.state().running);
        s.end(5, 0);
        assert!(!s.state().running);
    }

    #[test]
    fn the_prompt_row_keeps_its_place_as_the_screen_scrolls() {
        let mut s = ShellState::default();
        // Not a shell of jw's yet: nothing to leave out.
        assert_eq!(s.prompt_row(0), None);
        // The prompt starts at screen row 40 with 10 rows scrolled away.
        s.end(10, 40);
        assert_eq!(s.prompt_row(10), Some(40));
        // The screen scrolled 3 more rows: the same prompt is now 3 rows higher.
        assert_eq!(s.prompt_row(13), Some(37));
        // Scrolled past the top: the first row.
        assert_eq!(s.prompt_row(500), Some(0));
        // A command runs: no prompt is left out, so its prompt and what it prints show.
        s.start();
        assert_eq!(s.prompt_row(10), None);
    }

    #[test]
    fn the_line_being_typed_and_the_state_are_news_when_they_change() {
        let mut s = ShellState::default();
        assert_eq!(s.news().map(|t| t.known), Some(false));
        assert_eq!(s.news(), None);
        s.line("ls".into());
        let n = s.news().unwrap();
        assert_eq!((n.known, n.input.as_str()), (true, "ls"));
        assert_eq!(s.news(), None);
        // Typing more is news; the same line again is not.
        s.line("ls -l".into());
        assert_eq!(s.news().unwrap().input, "ls -l");
        s.line("ls -l".into());
        assert_eq!(s.news(), None);
        // A command starting is news, and so is the line cleared.
        s.start();
        assert!(s.news().unwrap().running);
        s.end(0, 0);
        s.line(String::new());
        let n = s.news().unwrap();
        assert_eq!((n.running, n.input.as_str()), (false, ""));
        // A client that asks is told in full and that counts as told.
        assert_eq!(s.tell().input, "");
        assert_eq!(s.news(), None);
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
