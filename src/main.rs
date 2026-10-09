//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. Built in parts (P0…P12).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => match jw::tui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("jw: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some("prompt") => report("prompt", jw::cli::prompt(&args[1..])),
        Some("new") => report("new", jw::cli::new(&args[1..])),
        Some("hook") => report("hook", jw::cli::hook(&args[1..])),
        Some("--version" | "-V") => {
            println!("jw {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Hidden: the client starts it (REQ-3); nobody types it.
        Some("daemon") => match jw::daemon::run(&jw::proto::socket_path()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("jw daemon: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some(cmd) => {
            eprintln!("jw: unknown command {cmd:?}");
            ExitCode::from(2)
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
