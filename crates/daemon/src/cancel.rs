//! What can be stopped while it runs (spec REQ-604): a push, or the review of a pull request
//! being made. The daemon keeps, for each workspace, the token of the operation that runs for it;
//! `cancel` sets it, and the process that operation waits on (`git`, `gh`) is killed.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::commands::{Done, ErrorBody, err};
use crate::server::Shared;

/// The word an operation that was cancelled answers with.
pub const CANCELLED: &str = "cancelado";

/// Whether the person asked to stop an operation; shared between who waits and who asks.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The operation of a workspace that can be stopped, while it runs: it is forgotten when this is
/// dropped, however the operation ends.
pub struct Op<'a> {
    shared: &'a Shared,
    workspace: String,
}

impl Drop for Op<'_> {
    fn drop(&mut self) {
        self.shared.lock().end_op(&self.workspace);
    }
}

/// Note that an operation that can be stopped begins for `workspace`: its token and the guard that
/// forgets it. One that already runs there refuses a second.
pub fn begin<'a>(shared: &'a Shared, workspace: &str) -> Result<(Cancel, Op<'a>), ErrorBody> {
    let token = shared
        .lock()
        .begin_op(workspace)
        .map_err(|(code, why)| err(code, why))?;
    Ok((
        token,
        Op {
            shared,
            workspace: workspace.to_owned(),
        },
    ))
}

/// `cancel`: stop what runs for the workspace, if anything does. It answers the same when
/// nothing does, so a client that asks late has nothing to handle.
pub fn cancel(shared: &Shared, workspace: &str) -> Done {
    shared.lock().cancel_op(workspace);
    Ok(None)
}

/// Kill the child and what it started: its process group (it leads one), then it.
fn stop(child: &mut std::process::Child) {
    let _ = Command::new("kill")
        .args(["-KILL", "--"])
        .arg(format!("-{}", child.id()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

/// Run `cmd` and read what it prints (at most `cap` bytes of stdout), killing it when `within`
/// has passed or `cancel` is set. The message of a failure is its stderr, else `<name> failed`.
pub fn run_child(
    mut cmd: Command,
    name: &str,
    within: Option<Duration>,
    cancel: &Cancel,
    cap: u64,
) -> Result<String, String> {
    // Its own process group, so that what it started (a hook, `ssh`) is stopped with it.
    use std::os::unix::process::CommandExt;
    let mut child = cmd
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {name}: {e}"))?;
    let mut out = child.stdout.take().expect("stdout is piped");
    let mut err = child.stderr.take().expect("stderr is piped");
    // What is past the cap is read and dropped: a pipe nobody reads would kill the command with
    // SIGPIPE, and a command that talks a lot (an update's progress) is not one that failed.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.by_ref().take(cap).read_to_string(&mut text);
        let _ = std::io::copy(&mut out, &mut std::io::sink());
        text
    });
    let message = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.by_ref().take(4096).read_to_string(&mut text);
        let _ = std::io::copy(&mut err, &mut std::io::sink());
        text.trim().to_owned()
    });
    let end = within.map(|w| Instant::now() + w);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancel.is_cancelled() {
            stop(&mut child);
            return Err(CANCELLED.into());
        }
        if end.is_some_and(|end| Instant::now() >= end) {
            stop(&mut child);
            return Err(format!("{name} did not answer in time"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = reader.join().unwrap_or_default();
    let why = message.join().unwrap_or_default();
    if status.success() {
        Ok(text)
    } else if why.is_empty() {
        Err(format!("{name} failed"))
    } else {
        Err(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn a_command_that_prints_past_the_cap_still_ends_well() {
        let c = Cancel::default();
        let talk = "i=0; while [ $i -lt 400 ]; do echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx; echo yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy >&2; i=$((i+1)); done";
        let kept =
            run_child(sh(talk), "sh", None, &c, 100).expect("not killed by a pipe nobody reads");
        assert_eq!(kept.len(), 100, "what is past the cap is dropped");
    }

    #[test]
    fn what_ends_on_its_own_answers_and_a_failure_says_why() {
        let c = Cancel::default();
        assert_eq!(
            run_child(sh("echo hola"), "sh", None, &c, 1024).as_deref(),
            Ok("hola\n")
        );
        assert_eq!(
            run_child(sh("echo malo >&2; exit 3"), "sh", None, &c, 1024),
            Err("malo".into())
        );
        assert_eq!(
            run_child(sh("exit 1"), "sh", None, &c, 1024),
            Err("sh failed".into())
        );
    }

    /// REQ-604: a process that waits is killed when the token is set, and quickly.
    #[test]
    fn a_cancel_kills_what_waits_and_says_so() {
        let c = Cancel::default();
        let later = c.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            later.cancel();
        });
        let began = Instant::now();
        let r = run_child(sh("sleep 30"), "sh", None, &c, 1024);
        t.join().unwrap();
        assert_eq!(r, Err(CANCELLED.into()));
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the sleep was not waited for"
        );
        // one already cancelled does not even wait
        assert_eq!(
            run_child(sh("sleep 30"), "sh", None, &c, 1024),
            Err(CANCELLED.into())
        );
    }

    /// What the process started goes with it: the file the grandchild would write is not written.
    #[test]
    fn what_the_process_started_is_stopped_with_it() {
        let dir = std::env::temp_dir().join(format!("jwn-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("late");
        let c = Cancel::default();
        let later = c.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.cancel();
        });
        let script = format!("(sleep 1.5; echo x > {}) & wait", mark.display());
        let r = run_child(sh(&script), "sh", None, &c, 1024);
        t.join().unwrap();
        assert_eq!(r, Err(CANCELLED.into()));
        std::thread::sleep(Duration::from_millis(2200));
        assert!(!mark.exists(), "the grandchild went on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_takes_longer_than_it_may_is_given_up_on() {
        let c = Cancel::default();
        let r = run_child(
            sh("sleep 30"),
            "sh",
            Some(Duration::from_millis(100)),
            &c,
            1024,
        );
        assert_eq!(r, Err("sh did not answer in time".into()));
    }
}
