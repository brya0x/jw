//! Git, by running `git`: no library, so behaviour matches what the user's own git does.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Run `git -C <dir> <args>`; the trimmed stdout, or git's own message on failure.
pub fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if msg.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&""))
        } else {
            msg
        })
    }
}

/// `run`, stopped when `cancel` is set (REQ-604): for what takes as long as the network does.
pub fn run_cancel(
    dir: &Path,
    args: &[&str],
    cancel: &crate::cancel::Cancel,
) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    crate::cancel::run_child(
        cmd,
        &format!("git {}", args.first().unwrap_or(&"")),
        None,
        cancel,
        1024 * 1024,
    )
    .map(|s| s.trim().to_owned())
}

/// What a bounded read printed.
pub struct Output {
    pub bytes: Vec<u8>,
    /// The output passed the cap: `bytes` holds the first `cap` of it and git was stopped.
    pub over: bool,
}

/// Run `git --no-pager -C <dir> <args>` for a read: no pager, none of git's optional locks (so
/// it never blocks the user's own git) and at most `cap` bytes of output. Git's message when
/// it fails, unless it was stopped for printing too much.
pub fn read<S: AsRef<OsStr>>(dir: &Path, args: &[S], cap: usize) -> Result<Output, String> {
    let mut child = Command::new("git")
        .arg("--no-pager")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // Drained on its own thread so a chatty git cannot fill the pipe while stdout is read.
    let message = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        String::from_utf8_lossy(&text).trim().to_owned()
    });
    let mut bytes = Vec::new();
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let read = stdout
        .by_ref()
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read git's output: {e}"));
    let over = bytes.len() > cap;
    if over {
        bytes.truncate(cap);
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|e| format!("cannot wait for git: {e}"))?;
    let message = message.join().unwrap_or_default();
    read?;
    if over || status.success() {
        return Ok(Output { bytes, over });
    }
    Err(if message.is_empty() {
        "git failed".to_owned()
    } else {
        message
    })
}

/// Which of `names` (entries of the folder `dir`) git ignores. Their names go in on stdin, so
/// any character a name holds is safe; nothing ignored, or not a repository, is the empty set.
pub fn ignored_among(dir: &Path, names: &[&str]) -> HashSet<String> {
    let Ok(mut child) = Command::new("git")
        .arg("--no-pager")
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "-z", "--stdin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut input = Vec::new();
    for name in names {
        input.extend_from_slice(name.as_bytes());
        input.push(0);
    }
    // Written on its own thread so git's answer cannot fill the pipe while the names are still going in.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output();
    let _ = writer.join();
    let Ok(out) = out else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The top of the working tree `dir` is in.
pub fn toplevel(dir: &Path) -> Result<String, String> {
    run(dir, &["rev-parse", "--show-toplevel"])
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    run(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// A branch name git accepts that cannot be mistaken for an option.
pub fn valid_branch(repo: &Path, name: &str) -> bool {
    !name.starts_with('-') && run(repo, &["check-ref-format", "--branch", name]).is_ok()
}

/// A directory name for a branch: path separators and odd characters become `-`.
pub fn slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slug_flattens_separators() {
        assert_eq!(slug("feat/login page"), "feat-login-page");
        assert_eq!(slug("v1.2_x"), "v1.2_x");
    }
}
