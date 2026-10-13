//! The daemon: owns the session, serves the node tree over a Unix socket.

mod commands;
mod core;
mod git;
mod model;
mod panes;
mod persist;
mod pty;
mod server;
mod workspaces;

pub use server::{Config, Daemon};
