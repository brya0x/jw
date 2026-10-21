//! Shared by the end-to-end tests of the binary: a private socket and state dir, and a `jw`
//! command that uses them. Only the pid it started is ever signalled.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const JW: &str = env!("CARGO_BIN_EXE_jw");

pub struct Env {
    pub dir: PathBuf,
}

impl Env {
    pub fn new(name: &str) -> Env {
        let dir = PathBuf::from(format!("/tmp/jwn-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Env { dir }
    }

    pub fn jw(&self) -> Command {
        let mut c = Command::new(JW);
        c.env(identity::env_var("STATE_DIR"), self.dir.join("state"))
            .env(identity::env_var("SOCKET"), self.dir.join("s.sock"))
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null());
        c
    }

    pub fn start(&self) -> Child {
        self.start_with(&[])
    }

    /// The daemon, with `env` added to its own environment (what its panes run with).
    pub fn start_with(&self, env: &[(&str, String)]) -> Child {
        let mut cmd = self.jw();
        cmd.arg("daemon");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if wire::Conn::connect(&self.dir.join("s.sock")).is_ok() {
                return child;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("the daemon did not come up");
    }

    /// `jwn cmd '<json>'`: its reply line as JSON.
    pub fn cmd(&self, json: &str) -> Value {
        let out = self.jw().args(["cmd", json]).output().unwrap();
        let line = String::from_utf8(out.stdout).unwrap();
        assert!(
            out.status.success(),
            "cmd {json} failed: {line} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&line).unwrap()
    }

    pub fn stop(&self, mut child: Child) {
        assert!(self.jw().arg("stop").status().unwrap().success());
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.kill().unwrap();
        panic!("the daemon ignored `jwn stop`");
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}
