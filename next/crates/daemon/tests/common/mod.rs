#![allow(dead_code)]
//! Shared helpers: a private socket dir per test (short, for the Unix path limit).

use std::path::{Path, PathBuf};
use std::process::Command;
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
            blocks: true,
            link: daemon::LinkConfig::off(),
            hook_exe: None,
            gh: None,
            github_every: Duration::from_millis(100),
            conversation: false,
            projects: self.dir.join("projects"),
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

/// Run `git -C <dir> <args>` as a test author with nothing signed; panics when git fails.
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The kinds of attention a node carries, in the order it lists them.
pub fn kinds(c: &Client, node: &str) -> Vec<String> {
    c.nodes[node]["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap().to_owned())
        .collect()
}

/// Wait until a node carries exactly these kinds of attention.
pub fn wait_kinds(c: &mut Client, node: &str, want: &[&str]) {
    assert!(
        c.wait_until(Duration::from_secs(8), |c| kinds(c, node) == want)
            .unwrap(),
        "wanted {want:?}, found {:?}",
        c.nodes[node]["attention"]
    );
}
