//! The daemon: owns the session, serves the node tree over a Unix socket.

mod agents;
mod blocks;
pub mod browse;
mod changes;
mod commands;
mod conversation;
mod core;
mod diff;
mod engine;
mod files;
mod git;
mod github;
mod grouping;
mod history;
mod integration;
mod launch;
mod layout;
mod markdown;
mod mobile;
mod model;
mod panes;
mod persist;
mod procs;
mod project;
mod pty;
mod rename;
mod replies;
mod saved;
mod server;
mod start;
mod sync;
mod workspaces;

pub use engine::served as conversation_served;
pub use launch::agent_name;
pub use mobile::{LinkConfig, devices_file};
pub use server::{Config, Daemon};
