//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. Built in parts (P0…P12).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {
            eprintln!("jw: the TUI is not built yet; see docs/specs/rust-tui.md");
            ExitCode::FAILURE
        }
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
