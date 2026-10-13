//! The whole thing through the real `jwn` binary, JSON only: start a daemon in a private
//! socket dir, make a session's worth of state with `jwn cmd`, stop it with `jwn stop`
//! (only its own pid), start it again and find the state back.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use wire::Client;

const JWN: &str = env!("CARGO_BIN_EXE_jwn");

struct Env {
    dir: PathBuf,
}

impl Env {
    fn jwn(&self) -> Command {
        let mut c = Command::new(JWN);
        c.env("JWN_STATE_DIR", self.dir.join("state"))
            .env("JWN_SOCKET", self.dir.join("s.sock"))
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null());
        c
    }

    fn start(&self) -> Child {
        let mut child = self.jwn().arg("daemon").spawn().unwrap();
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
    fn cmd(&self, json: &str) -> Value {
        let out = self.jwn().args(["cmd", json]).output().unwrap();
        let line = String::from_utf8(out.stdout).unwrap();
        assert!(
            out.status.success(),
            "cmd {json} failed: {line} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&line).unwrap()
    }

    fn stop(&self, mut child: Child) {
        assert!(self.jwn().arg("stop").status().unwrap().success());
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

fn git(dir: &Path, args: &[&str]) {
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

#[test]
fn a_session_is_built_with_json_and_comes_back_after_a_restart() {
    let env = Env {
        dir: PathBuf::from(format!("/tmp/jwn-e2e-{}", std::process::id())),
    };
    std::fs::create_dir_all(&env.dir).unwrap();
    let work = std::fs::canonicalize(&env.dir).unwrap().join("work");
    let repo = work.join("app");
    let notes = work.join("notes");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&notes).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    let daemon = env.start();
    let ok = |v: &Value| assert_eq!(v["ok"], true, "{v}");
    let r = env.cmd(r#"{"cmd":"rename_session","name":"e2e"}"#);
    ok(&r);
    let folder = env.cmd(&format!(
        r#"{{"cmd":"open_folder","path":"{}"}}"#,
        notes.display()
    ));
    ok(&folder);
    let wt = env.cmd(&format!(
        r#"{{"cmd":"create_worktree","repo":"{}","branch":"feat/x"}}"#,
        repo.display()
    ));
    ok(&wt);
    let ws = wt["result"]["workspace"].as_str().unwrap().to_owned();
    let pane = env.cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#));
    ok(&pane);
    let pane = pane["result"]["pane"].as_str().unwrap().to_owned();
    ok(&env.cmd(&format!(
        r#"{{"cmd":"write","pane":"{pane}","data":"echo e2e-marker\r"}}"#
    )));

    let sock = env.dir.join("s.sock");
    let mut c = Client::connect(&sock, "e2e").unwrap();
    let root = c.root.clone();
    c.open(&root).unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| {
            c.nodes.get(&pane).is_some_and(|p| {
                p["screen"]["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|l| l.as_str().unwrap().contains("e2e-marker"))
            }) && c.nodes.contains_key(&root)
        })
        .unwrap()
    );
    assert_eq!(c.nodes[&root]["name"], "e2e");
    assert_eq!(c.nodes[&root]["workspaces"].as_array().unwrap().len(), 2);
    drop(c);

    env.stop(daemon);

    let daemon = env.start();
    let mut c = Client::connect(&sock, "e2e").unwrap();
    assert_eq!(c.root, root, "same session");
    c.open(&root).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&root))
            .unwrap()
    );
    assert_eq!(c.nodes[&root]["name"], "e2e");
    assert_eq!(c.nodes[&root]["workspaces"][1]["branch"], "feat/x");
    assert_eq!(c.nodes[&root]["workspaces"][1]["pane_count"], 1);
    c.open(&ws).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&ws))
            .unwrap()
    );
    assert_eq!(c.nodes[&ws]["panes"][0]["id"], pane.as_str());
    drop(c);
    env.stop(daemon);
}
