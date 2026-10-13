//! The daemon: owns the session, serves the node tree over a Unix socket.

mod commands;
mod core;
mod model;
mod persist;
mod server;

pub use server::{Config, Daemon};
