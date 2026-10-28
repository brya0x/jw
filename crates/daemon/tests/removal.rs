//! N189: removing a worktree ends what runs in its panes before git, and finishes an orphan
//! (REQ-1065 to REQ-1077), against real git and real processes in a private daemon.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT, git, text};
use contract::Cmd;
use serde_json::Value;
use wire::Client;

/// A worktree `b1` of the repository at `<sandbox>/work/app`, with a shell running in it.
struct World {
    sb: Sandbox,
    top: PathBuf,
    dir: PathBuf,
    ws: String,
    pane: String,
}

fn world() -> (World, daemon::Daemon, Client) {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = sb.dir.join("work/app");
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "-q", "-b", "main"]);
    std::fs::write(top.join("f.txt"), "x").unwrap();
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "init"]);
    let top = std::fs::canonicalize(top).unwrap();
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    let ws = c
        .cmd(Cmd::CreateWorktree {
            repo: text(&top),
            branch: "b1".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    let dir = top.parent().unwrap().join("app-wt/b1");
    (
        World {
            sb,
            top,
            dir,
            ws,
            pane,
        },
        d,
        c,
    )
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .output()
        .unwrap()
        .status
        .success()
}

impl World {
    /// Run a script in the pane that starts two writers in the worktree (one in a process group
    /// of its own that ignores `SIGTERM` and `SIGHUP`) and writes their pids; the pids it wrote.
    fn start_writers(&self, c: &mut Client) -> Vec<String> {
        let pids = self.sb.dir.join("pids");
        let script = self.sb.dir.join("tree.sh");
        let (pids_s, dir_s) = (pids.display(), self.dir.display());
        std::fs::write(
            &script,
            format!(
                "( perl -e 'setpgrp(0,0); exec @ARGV' sh -c 'trap \"\" TERM HUP; echo $$ >> {pids_s}; i=0; while [ $i -lt 600 ]; do touch {dir_s}/w2; sleep 0.1; i=$((i+1)); done' ) &\n\
                 ( sh -c 'echo $$ >> {pids_s}; i=0; while [ $i -lt 600 ]; do touch {dir_s}/w1; sleep 0.1; i=$((i+1)); done' ) &\n\
                 wait\n"
            ),
        )
        .unwrap();
        c.cmd(Cmd::Write {
            pane: self.pane.clone(),
            data: format!("sh {}\r", script.display()),
        })
        .unwrap()
        .unwrap();
        wait_for("the writers' pids", || {
            std::fs::read_to_string(&pids).is_ok_and(|s| s.lines().count() == 2)
        });
        wait_for("the writers to write", || {
            self.dir.join("w1").exists() && self.dir.join("w2").exists()
        });
        std::fs::read_to_string(&pids)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn remove(&self, c: &mut Client, force: bool) -> Value {
        let job = c
            .cmd(Cmd::RemoveWorkspace {
                workspace: self.ws.clone(),
                remove_worktree: true,
                force,
            })
            .unwrap()
            .unwrap()
            .unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = c.root.clone();
        let find = |c: &Client| {
            c.nodes[&root]["jobs"]
                .as_array()
                .and_then(|j| j.iter().find(|j| j["id"] == job.as_str()).cloned())
                .filter(|j| j["state"] != "running")
        };
        assert!(
            c.wait_until(Duration::from_secs(15), |c| find(c).is_some())
                .unwrap(),
            "the job never ended"
        );
        find(c).unwrap()
    }

    fn registered(&self) -> bool {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.top)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains("app-wt/b1")
    }

    fn has_entry(&self, c: &Client) -> bool {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == self.ws.as_str())
    }

    fn entry_goes(&self, c: &mut Client) {
        assert!(c.wait_until(WAIT, |c| !self.has_entry(c)).unwrap());
    }
}

fn all_gone(pids: &[String]) {
    wait_for("the writers to end", || pids.iter().all(|p| !alive(p)));
}

#[test]
fn removing_a_worktree_ends_the_whole_tree_and_then_goes() {
    let (w, d, mut c) = world();
    let pids = w.start_writers(&mut c);
    assert!(pids.iter().all(|p| alive(p)));

    // Without `force` the changes the writers made refuse it, and nothing is ended (REQ-1065).
    let job = w.remove(&mut c, false);
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["detail"].as_str().unwrap().contains("--force"), "{job}");
    assert!(pids.iter().all(|p| alive(p)), "nothing was touched");
    assert!(w.has_entry(&c) && w.dir.exists());

    // By force: the writers, the one that ignores `SIGTERM` in its own group included, end, and the
    // folder, git's record and the entry go (REQ-1066).
    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "done", "{job}");
    all_gone(&pids);
    assert!(!w.dir.exists());
    assert!(!w.registered());
    w.entry_goes(&mut c);
    d.stop();
}

#[test]
fn a_git_failure_says_what_was_done_and_a_retry_goes_on() {
    let (w, d, mut c) = world();
    let pids = w.start_writers(&mut c);
    git(&w.top, &["worktree", "lock", &text(&w.dir)]);

    // A locked worktree is git's refusal, after the processes ended (REQ-1068).
    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "failed", "{job}");
    let why = job["detail"].as_str().unwrap();
    assert!(why.contains("locked"), "git's own message: {why}");
    assert!(why.contains("procesos: "), "what was done: {why}");
    assert!(why.contains("sin hacer: la carpeta y la entrada"), "{why}");
    all_gone(&pids);
    assert!(w.has_entry(&c) && w.dir.exists() && w.registered());

    // Again, with nothing to end any more (REQ-1069).
    git(&w.top, &["worktree", "unlock", &text(&w.dir)]);
    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "done", "{job}");
    assert!(!w.dir.exists() && !w.registered());
    d.stop();
}

/// Make the worktree an orphan the way the incident did: git's record is gone, the folder stays.
fn orphan(w: &World) {
    let admin = std::process::Command::new("git")
        .arg("-C")
        .arg(&w.dir)
        .args(["rev-parse", "--git-dir"])
        .output()
        .unwrap();
    let admin = PathBuf::from(String::from_utf8_lossy(&admin.stdout).trim());
    assert!(admin.ends_with("worktrees/b1"), "{admin:?}");
    std::fs::remove_dir_all(admin).unwrap();
    assert!(!w.registered() && w.dir.exists());
}

fn check(c: &mut Client, ws: &str) -> Value {
    c.cmd(Cmd::CheckWorkspace {
        workspace: ws.into(),
    })
    .unwrap()
    .unwrap()
    .unwrap()
}

#[test]
fn an_orphan_is_said_so_and_is_finished_by_force() {
    let (w, d, mut c) = world();
    let pids = w.start_writers(&mut c);
    orphan(&w);

    // `check_workspace` says so, instead of git's `not a git repository` (REQ-1074).
    let r = check(&mut c, &w.ws);
    assert_eq!(r["orphan"], true, "{r}");
    assert_eq!(r["ahead"], 0);

    // With no `force` nothing is touched, the processes included (REQ-1071).
    let job = w.remove(&mut c, false);
    assert_eq!(job["state"], "failed", "{job}");
    assert!(
        job["detail"]
            .as_str()
            .unwrap()
            .starts_with("huérfano: git ya no conoce"),
        "{job}"
    );
    assert!(pids.iter().all(|p| alive(p)));
    assert!(w.has_entry(&c) && w.dir.exists());

    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "done", "{job}");
    all_gone(&pids);
    assert!(!w.dir.exists());
    w.entry_goes(&mut c);
    d.stop();
}

#[test]
fn an_orphan_outside_the_worktrees_folder_loses_its_entry_and_keeps_its_folder() {
    let (w, d, mut c) = world();
    orphan(&w);
    // The folder where this repository's worktrees go is read afresh: it is not this one any more.
    std::fs::write(
        w.top.join(".jw.toml"),
        format!("root = \"{}\"\n", w.sb.dir.join("elsewhere").display()),
    )
    .unwrap();
    std::fs::create_dir_all(w.sb.dir.join("elsewhere")).unwrap();

    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "failed", "{job}");
    let why = job["detail"].as_str().unwrap();
    assert!(why.contains("sigue en disco"), "{why}");
    assert!(why.contains("la entrada de jw se quitó"), "{why}");
    assert!(w.dir.exists(), "nothing was deleted");
    w.entry_goes(&mut c);
    d.stop();
}

#[test]
fn a_folder_that_is_already_gone_is_finished_too() {
    let (w, d, mut c) = world();
    orphan(&w);
    std::fs::remove_dir_all(&w.dir).unwrap();
    // No `force`, as the phone sends it: there is no folder to lose (REQ-1073).
    let job = w.remove(&mut c, false);
    assert_eq!(job["state"], "done", "{job}");
    w.entry_goes(&mut c);
    d.stop();
}

#[test]
fn a_registered_worktree_whose_folder_is_gone_goes_with_no_force() {
    let (w, d, mut c) = world();
    std::fs::remove_dir_all(&w.dir).unwrap();
    assert!(w.registered());
    let job = w.remove(&mut c, false);
    assert_eq!(job["state"], "done", "{job}");
    assert!(!w.registered());
    w.entry_goes(&mut c);
    d.stop();
}

/// The incident: git deletes what it can and forgets the worktree, then fails on a folder it
/// cannot empty, and the daemon's own deletion fails on it too.
#[test]
fn git_forgetting_the_worktree_before_failing_leaves_an_orphan_that_a_retry_finishes() {
    use std::os::unix::fs::PermissionsExt;
    let (w, d, mut c) = world();
    let ro = w.dir.join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::write(ro.join("f"), "x").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let job = w.remove(&mut c, true);
    let (registered, exists) = (w.registered(), w.dir.exists());
    let seen = check(&mut c, &w.ws);
    // Before any assertion, so a failure leaves no folder that cannot be deleted.
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(job["state"], "failed", "{job}");
    let why = job["detail"].as_str().unwrap();
    assert!(why.contains("lo olvidó"), "{why}");
    assert!(why.contains("sin hacer: la carpeta y la entrada"), "{why}");
    assert!(!registered && exists, "git forgot it and the folder stays");
    assert!(w.has_entry(&c));
    assert_eq!(seen["orphan"], true, "{seen}");

    // It was an orphan before this job: only `force` finishes it.
    let job = w.remove(&mut c, true);
    assert_eq!(job["state"], "done", "{job}");
    assert!(!w.dir.exists());
    w.entry_goes(&mut c);
    d.stop();
}
