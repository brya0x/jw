//! Git, by running `git`: no library, so behaviour matches what the user's own git does.

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
