//! The daemon: owns the session, serves the node tree over a Unix socket.

mod agents;
mod blocks;
pub mod browse;
mod changes;
mod commands;
mod core;
mod diff;
mod git;
mod github;
mod grouping;
mod history;
mod integration;
mod launch;
mod layout;
mod mobile;
mod model;
mod panes;
mod persist;
mod procs;
mod pty;
mod rename;
mod replies;
mod server;
mod sync;
mod workspaces;

pub use mobile::{LinkConfig, devices_file};
pub use server::{Config, Daemon};
