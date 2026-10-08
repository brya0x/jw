//! jw: worktree streams in a TUI that is its own terminal multiplexer.
//! Spec: docs/specs/rust-tui.md. The binary is a thin shell over this crate,
//! which is also what the integration tests drive.

#[cfg(not(unix))]
compile_error!("jw is unix only (macOS and Linux): it needs PTYs and a unix socket");

pub mod actions;
pub mod cli;
pub mod client;
pub mod connectors;
pub mod core;
pub mod daemon;
pub mod free;
pub mod init;
pub mod layout;
pub mod proto;
pub mod stream;
pub mod tui;
