//! jw's model (docs/specs/rust-tui.md): the project config and its layouts,
//! the registry of worktrees, sessions and their folders, git and GitHub,
//! and the actions on worktrees (new, rename, sync, remove). No terminal,
//! no daemon, no UI: everything else builds on this crate.

#[cfg(not(unix))]
compile_error!("jw is unix only (macOS and Linux): it needs PTYs and a unix socket");

pub mod actions;
pub mod connectors;
pub mod core;
pub mod diff;
pub mod folders;
pub mod init;
pub mod layout;
pub mod session;
pub mod stream;
