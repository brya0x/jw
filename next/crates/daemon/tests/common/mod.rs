#![allow(dead_code)]
//! Shared helpers: a private socket dir per test (short, for the Unix path limit).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use daemon::{Config, Daemon};
use wire::Client;

pub struct Sandbox {
    pub dir: PathBuf,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = PathBuf::from(format!(
            "/tmp/jwn-t-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Sandbox { dir }
    }

    pub fn config(&self) -> Config {
        Config {
            socket: self.dir.join("s.sock"),
            state_dir: self.dir.join("state"),
            shell: Some("/bin/sh".into()),
            tcp: None,
            env: Vec::new(),
        }
    }

    pub fn start(&self) -> Daemon {
        Daemon::start(self.config()).expect("daemon starts")
    }

    pub fn client(&self) -> Client {
        Client::connect(&self.config().socket, "test").expect("client connects")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub const WAIT: Duration = Duration::from_secs(5);
