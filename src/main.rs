//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. Built in parts (P0…P12); this is P0.

#[cfg(not(unix))]
compile_error!("jw is unix only (macOS and Linux): it needs PTYs and a unix socket");

#[allow(dead_code)] // used by the parts that come next (P2…)
mod core;

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
        Some(cmd) => {
            eprintln!("jw: unknown command {cmd:?}");
            ExitCode::from(2)
        }
    }
}
