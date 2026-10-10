//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. Built in parts (P0…P12).

use std::process::ExitCode;

mod cli;
mod help;
mod server;
mod skill;

// cli.rs says `crate::stream`, `crate::proto`…, as it did when this was one
// crate.
use jw_core::{actions, core, folders, layout, session, stream};
use jw_proto::{client, proto};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `jw <command> --help`, before the command reads its arguments.
    if let [cmd, flag] = args.as_slice()
        && help::asks(flag)
        && let Some(c) = help::find(cmd)
    {
        print!("{}", help::one(c));
        return ExitCode::SUCCESS;
    }
    match args.first().map(String::as_str) {
        None => tui(None),
        Some("help" | "-h" | "--help") => match args.get(1) {
            None => {
                print!("{}", help::all());
                ExitCode::SUCCESS
            }
            Some(cmd) => match help::find(cmd) {
                Some(c) => {
                    print!("{}", help::one(c));
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("jw help: no command {cmd:?}; jw help lists them");
                    ExitCode::from(2)
                }
            },
        },
        Some("new") => match cli::new_session(&args[1..]) {
            Ok(name) => tui(Some(&name)),
            Err(e) => {
                eprintln!("jw new: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some("sessions") => report("sessions", cli::sessions()),
        Some("prompt") => report("prompt", cli::prompt(&args[1..])),
        Some("worktree") => report("worktree", cli::worktree(&args[1..])),
        Some("ls") => report("ls", cli::ls(&args[1..])),
        Some("read") => report("read", cli::read(&args[1..])),
        Some("hook") => report("hook", cli::hook(&args[1..])),
        Some("skill") => report("skill", skill::run(&args[1..])),
        Some("server") => report("server", server::run(&args[1..])),
        Some("--version" | "-V") => {
            println!("jw {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Hidden: the client starts it (REQ-3); nobody types it.
        Some("daemon") => match jw_daemon::run(&jw_proto::proto::socket_path()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("jw daemon: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some(name) if jw_core::session::valid(name) && args.len() == 1 => tui(Some(name)),
        Some(cmd) => {
            eprintln!("jw: unknown command {cmd:?}; jw help lists them");
            ExitCode::from(2)
        }
    }
}

/// The TUI on session `name`, or on the last one used (REQ-64).
fn tui(name: Option<&str>) -> ExitCode {
    let run = || -> anyhow::Result<()> {
        let state = jw_core::core::registry::state_dir()?;
        let name = match name {
            Some(n) if !jw_core::session::exists(&state, n)? => {
                anyhow::bail!("no session {n}: jw new {n} starts one here")
            }
            Some(n) => n.to_string(),
            None => jw_core::session::last(&state),
        };
        std::fs::create_dir_all(jw_core::session::dir(&state, &name)?)?;
        jw_core::session::set(&name);
        jw_core::session::set_last(&state, &name)?;
        jw_tui::tui::run()
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jw: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn report(cmd: &str, result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jw {cmd}: {e:#}");
            ExitCode::FAILURE
        }
    }
}
