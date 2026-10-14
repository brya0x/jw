//! One terminal: a PTY running the user's shell, its screen kept by vt100.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::blocks::{self, BlockStore, Scanner};
use crate::integration::Setup;
use contract::{Block, BlocksMode, Cursor, Id, Screen};
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

/// Lock order: the hub's lock, then `parser`. The reader thread takes only `parser`, so
/// it can never wait on the hub while the hub waits on it.
pub struct Term {
    parser: Arc<Mutex<vt100::Parser>>,
    blocks: Arc<Mutex<BlockStore>>,
    writer: Mutex<Box<dyn Write + Send>>,
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
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
        killer: Mutex::new(child.clone_killer()),
        shell_pid: child.process_id(),
    });

    let parser = term.parser.clone();
    let store = term.blocks.clone();
    let pane = pane.clone();
    thread::Builder::new()
        .name("pty".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            let mut scanner = Scanner::default();
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut marks = Vec::new();
                for piece in scanner.feed(&buf[..n]) {
                    match piece {
                        blocks::Piece::Data(d) => {
                            lock(&parser).process(&d);
                            lock(&store).data(&d);
                        }
                        blocks::Piece::Mark(m) => {
                            match &m {
                                blocks::Mark::Command { .. } => marks.push(Mark::CommandStart),
                                blocks::Mark::Done(code) => marks.push(Mark::CommandDone(*code)),
                                _ => {}
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

    /// The blocks, once the shell has reported a prompt.
    pub fn blocks_view(&self) -> Option<(BlocksMode, Vec<Block>)> {
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

    pub fn screen(&self) -> Screen {
        let parser = lock(&self.parser);
        let screen = parser.screen();
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
        }
    }
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
    }
}
