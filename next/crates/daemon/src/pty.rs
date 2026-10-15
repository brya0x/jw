//! One terminal: a PTY running the user's shell, its screen kept by vt100.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::blocks::{self, BlockStore, Scanner};
use crate::integration::Setup;
use crate::replies::Queries;
use contract::{Block, BlocksMode, Cursor, Id, PaneInput, Screen};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub const ROWS: u16 = 24;
pub const COLS: u16 = 80;
pub const MAX_SIZE: u16 = 500;

/// What happened in a shell, told to the engine.
pub enum Event {
    /// Output arrived (the screen changed), with the shell-integration marks it carried.
    Output {
        pane: Id,
        marks: Vec<Mark>,
    },
    /// The shell exited.
    Exited {
        pane: Id,
    },
    Stop,
}

/// OSC 133 shell-integration marks: a command started, a command finished.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    CommandStart,
    CommandDone(Option<i32>),
}

/// Lock order: the hub's lock, then `parser` or `blocks` (never both at once). The reader
/// thread takes only those, so it can never wait on the hub while the hub waits on it; nor on the
/// input: the answers it owes the program go to a thread of their own, the only one besides a
/// client's `write` that takes `writer`.
pub struct Term {
    parser: Arc<Mutex<vt100::Parser>>,
    blocks: Arc<Mutex<BlockStore>>,
    /// The column of the screen where the shell's prompt ended (OSC 133;B), while it waits for a
    /// line: the reader sets it at that mark and clears it when a command starts or ends.
    input_col: Arc<Mutex<Option<u16>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    shell_pid: Option<u32>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Start `shell` in `cwd`, running `program` inside it when given (see `launch`). Its
/// output is fed to the screen and announced on `events`.
pub fn spawn(
    pane: &Id,
    shell: &str,
    cwd: &Path,
    program: Option<&[String]>,
    env: &[(String, String)],
    integration: &Setup,
    events: Sender<Event>,
) -> Result<Arc<Term>, String> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("cannot open a pty: {e}"))?;
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(&integration.args);
    if let Some(argv) = program.filter(|a| !a.is_empty()) {
        cmd.args(["-i", "-c", &crate::launch::script(shell, argv, integration)]);
    }
    cmd.cwd(cwd);
    for (k, v) in env.iter().chain(&integration.env) {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env(identity::env_var("PANE_ID"), pane);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("cannot start {shell}: {e}"))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let term = Arc::new(Term {
        parser: Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0))),
        blocks: Arc::new(Mutex::new(BlockStore::new(COLS))),
        input_col: Arc::new(Mutex::new(None)),
        writer: Arc::new(Mutex::new(writer)),
        master: Mutex::new(pair.master),
        killer: Mutex::new(child.clone_killer()),
        shell_pid: child.process_id(),
    });
    if !integration.args.is_empty() || !integration.env.is_empty() {
        lock(&term.blocks).integrate();
    }

    // What the program asks its terminal is answered on its input by a thread of its own, so the
    // reader never waits on the input: a program blocked writing cannot hold up the pane (spec REQ-268).
    let (answers, to_answer) = mpsc::channel::<Vec<u8>>();
    let input = term.writer.clone();
    thread::Builder::new()
        .name("pty-answers".into())
        .spawn(move || {
            for answer in to_answer {
                let mut w = lock(&input);
                if w.write_all(&answer).and_then(|()| w.flush()).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;

    let parser = term.parser.clone();
    let store = term.blocks.clone();
    let input_col = term.input_col.clone();
    let pane = pane.clone();
    thread::Builder::new()
        .name("pty".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            let mut scanner = Scanner::default();
            let mut queries = Queries::default();
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut marks = Vec::new();
                for piece in scanner.feed(&buf[..n]) {
                    match piece {
                        blocks::Piece::Data(d) => {
                            // The screen is read where each query stood, for the cursor's place.
                            let mut emulator = lock(&parser);
                            let mut from = 0;
                            for (end, query) in queries.feed(&d) {
                                emulator.process(&d[from..end]);
                                from = end;
                                let _ = answers.send(query.reply(emulator.screen()));
                            }
                            emulator.process(&d[from..]);
                            drop(emulator);
                            lock(&store).data(&d);
                        }
                        blocks::Piece::Mark(m) => {
                            // The screen is exactly at the mark: the cursor is where typing starts.
                            match &m {
                                blocks::Mark::Input => {
                                    let col = lock(&parser).screen().cursor_position().1;
                                    *lock(&input_col) = Some(col);
                                }
                                blocks::Mark::Command { .. } => {
                                    *lock(&input_col) = None;
                                    marks.push(Mark::CommandStart);
                                }
                                blocks::Mark::Done(code) => {
                                    *lock(&input_col) = None;
                                    marks.push(Mark::CommandDone(*code));
                                }
                                blocks::Mark::Prompt => *lock(&input_col) = None,
                            }
                            lock(&store).mark(m);
                        }
                    }
                }
                if events
                    .send(Event::Output {
                        pane: pane.clone(),
                        marks,
                    })
                    .is_err()
                {
                    break;
                }
            }
            let _ = child.wait();
            let _ = events.send(Event::Exited { pane });
        })
        .map_err(|e| e.to_string())?;
    Ok(term)
}

impl Term {
    /// Changes whenever the blocks a client would see change.
    pub fn blocks_version(&self) -> u64 {
        lock(&self.blocks).version()
    }

    /// Give the blocks a daemon saved before it stopped to this shell's store.
    pub fn restore_blocks(&self, blocks: Vec<contract::Block>, saved_at: u64, now: u64) {
        lock(&self.blocks).restore(blocks, saved_at, now);
    }

    /// The blocks of a shell started with the integration.
    pub fn blocks_view(&self) -> Option<(u64, BlocksMode, Vec<Block>)> {
        lock(&self.blocks).view()
    }

    /// The shell's own process id.
    pub fn shell_pid(&self) -> Option<u32> {
        self.shell_pid
    }

    /// The process group in the foreground of the terminal: the shell at its prompt, else the
    /// program it is running.
    pub fn foreground(&self) -> Option<u32> {
        lock(&self.master)
            .process_group_leader()
            .and_then(|p| u32::try_from(p).ok())
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        let mut w = lock(&self.writer);
        w.write_all(data)?;
        w.flush()
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        lock(&self.master)
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        lock(&self.parser).screen_mut().set_size(rows, cols);
        lock(&self.blocks).set_cols(cols);
        Ok(())
    }

    /// End the shell (SIGHUP); the reader thread then reaps it.
    pub fn kill(&self) {
        let _ = lock(&self.killer).kill();
    }

    /// What the person has typed at the shell's prompt; `None` while a command runs, and for a
    /// shell that has not said where its prompt ends.
    pub fn input(&self) -> Option<PaneInput> {
        let col = (*lock(&self.input_col))?;
        Some(input_of(lock(&self.parser).screen(), col))
    }

    pub fn screen(&self) -> Screen {
        screen_of(lock(&self.parser).screen())
    }
}

/// A terminal's screen as the pane node carries it.
pub fn screen_of(screen: &vt100::Screen) -> Screen {
    let (rows, cols) = screen.size();
    let (row, col) = screen.cursor_position();
    Screen {
        rows,
        cols,
        lines: screen
            .rows(0, cols)
            .map(|l| l.trim_end().to_owned())
            .collect(),
        cursor: Cursor {
            row,
            col,
            visible: !screen.hide_cursor(),
        },
        alt: screen.alternate_screen(),
    }
}

/// The line the shell holds: from `start_col` of the first row of the wrapped line the cursor is
/// in, to its end, with the rows a wrap split joined (a typed space at the end of a row stays).
/// A line with a hard break (a multi-line zsh buffer) is read from its last row.
pub fn input_of(screen: &vt100::Screen, start_col: u16) -> PaneInput {
    let (row, col) = screen.cursor_position();
    let (_, cols) = screen.size();
    let mut first = row;
    while first > 0 && screen.row_wrapped(first - 1) {
        first -= 1;
    }
    let mut text = String::new();
    let mut cursor = 0u32;
    for r in first..=row {
        let from = if r == first { start_col.min(cols) } else { 0 };
        let mut line = String::new();
        let mut before = 0u32;
        for c in from..cols {
            let Some(cell) = screen.cell(r, c) else { break };
            if cell.is_wide_continuation() {
                continue;
            }
            if r == row && c < col {
                before += 1;
            }
            let s = cell.contents();
            line.push_str(if s.is_empty() { " " } else { s });
        }
        if r == row {
            // Trailing blanks are not typed, but those before the cursor are.
            let kept = line.trim_end().chars().count().max(before as usize);
            line = line.chars().take(kept).collect();
            cursor = text.chars().count() as u32 + before;
        }
        text.push_str(&line);
    }
    PaneInput { text, cursor }
}

pub fn blank_screen() -> Screen {
    Screen {
        rows: ROWS,
        cols: COLS,
        lines: vec![String::new(); ROWS as usize],
        cursor: Cursor {
            row: 0,
            col: 0,
            visible: true,
        },
        alt: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen of 20 columns after `bytes`, and what the shell holds from column `from`.
    fn input(bytes: &str, from: u16) -> PaneInput {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(bytes.as_bytes());
        input_of(p.screen(), from)
    }

    #[test]
    fn what_is_typed_after_the_prompt_is_the_input_and_the_cursor_is_in_characters() {
        let held = input("old output\r\n❯ git status", 2);
        assert_eq!((held.text.as_str(), held.cursor), ("git status", 10));
        let empty = input("❯ ", 2);
        assert_eq!((empty.text.as_str(), empty.cursor), ("", 0));
    }

    #[test]
    fn a_trailing_space_before_the_cursor_is_typed_and_text_after_it_is_kept() {
        let held = input("❯ ls ", 2);
        assert_eq!((held.text.as_str(), held.cursor), ("ls ", 3));
        // The cursor moved back two characters, as with ^B: the text stays whole.
        let held = input("❯ git status\x1b[2D", 2);
        assert_eq!((held.text.as_str(), held.cursor), ("git status", 8));
    }

    #[test]
    fn a_line_the_screen_wrapped_is_one_line() {
        // 20 columns: the prompt takes two, so 18 characters fit in the first row.
        let held = input("❯ abcdefghijklmnopqrstuvwxy", 2);
        assert_eq!(held.text, "abcdefghijklmnopqrstuvwxy");
        assert_eq!(held.cursor, 25);
    }

    #[test]
    fn a_line_that_scrolled_the_screen_is_still_read_from_its_first_row() {
        // The prompt is on the last row, so the wrap scrolls everything up by one.
        let held = input("1\r\n2\r\n3\r\n4\r\n5\r\n❯ abcdefghijklmnopqrstuvwxy", 2);
        assert_eq!(held.text, "abcdefghijklmnopqrstuvwxy");
        assert_eq!(held.cursor, 25);
    }

    #[test]
    fn the_screen_says_whether_the_program_is_on_the_alternate_screen() {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(b"$ nvim");
        assert!(!screen_of(p.screen()).alt);
        p.process(b"\x1b[?1049h~");
        assert!(screen_of(p.screen()).alt, "entered");
        p.process(b"\x1b[?1049l");
        assert!(
            !screen_of(p.screen()).alt,
            "left: the shell's own screen is back"
        );
    }

    #[test]
    fn wide_characters_count_once() {
        let held = input("❯ 日本語", 2);
        assert_eq!((held.text.as_str(), held.cursor), ("日本語", 3));
    }
}
