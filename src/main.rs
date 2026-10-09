//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. Built in parts (P0…P12).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => tui(None),
        Some("new") => match jw::cli::new_session(&args[1..]) {
            Ok(name) => tui(Some(&name)),
            Err(e) => {
                eprintln!("jw new: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some("sessions") => report("sessions", jw::cli::sessions()),
        Some("prompt") => report("prompt", jw::cli::prompt(&args[1..])),
        Some("worktree") => report("worktree", jw::cli::worktree(&args[1..])),
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
        Some(name) if jw::session::valid(name) && args.len() == 1 => tui(Some(name)),
        Some(cmd) => {
            eprintln!("jw: unknown command {cmd:?}");
            ExitCode::from(2)
        }
    }
}

/// The TUI on session `name`, or on the last one used (REQ-64).
fn tui(name: Option<&str>) -> ExitCode {
    let run = || -> anyhow::Result<()> {
        let state = jw::core::registry::state_dir()?;
        let name = match name {
            Some(n) if !jw::session::exists(&state, n)? => {
                anyhow::bail!("no session {n}: jw new {n} starts one here")
            }
            Some(n) => n.to_string(),
            None => jw::session::last(&state),
        };
        std::fs::create_dir_all(jw::session::dir(&state, &name)?)?;
        jw::session::set(&name);
        jw::session::set_last(&state, &name)?;
        jw::tui::run()
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
