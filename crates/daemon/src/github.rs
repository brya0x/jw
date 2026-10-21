//! What GitHub says about a worktree's branch: its newest pull request, whether it was merged
//! and whether its checks failed (REQ-135 to REQ-138). Read with `gh`, so the person's own
//! login and host are used; the daemon holds no token.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cancel::Cancel;
use crate::server::Shared;

/// How long `gh` has to answer before it is given up on.
const WITHIN: Duration = Duration::from_secs(10);
/// The most of `gh`'s answer that is read.
const CAP: u64 = 1024 * 1024;

/// A branch's newest pull request, as far as attention goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub merged: bool,
    /// An open pull request with a check that ended in failure.
    pub failing: bool,
}

/// `gh pr list --head <branch> --state all --limit 1 --json …`: no pull request is `Ok(None)`.
pub fn parse(json: &str) -> Result<Option<Pr>, String> {
    let list: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let Some(pr) = list.as_array().ok_or("not a list")?.first() else {
        return Ok(None);
    };
    // `mergedAt` is not read: older `gh` fills it with a zero time for a pull request never merged.
    let state = pr["state"].as_str().unwrap_or_default();
    let merged = state == "MERGED";
    // A cancelled or skipped run is not a failure; a status that is still running is not one yet.
    let failed = |check: &Value| {
        matches!(
            check["conclusion"].as_str(),
            Some("FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE")
        ) || matches!(check["state"].as_str(), Some("FAILURE" | "ERROR"))
    };
    let failing = state == "OPEN"
        && pr["statusCheckRollup"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(failed));
    Ok(Some(Pr {
        number: pr["number"].as_u64().unwrap_or(0),
        merged,
        failing,
    }))
}

/// Run `gh` in `dir` and read its answer, or give up after `WITHIN`.
fn gh(program: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    run_gh(program, dir, args, WITHIN)
}

/// Run `gh` in `dir` and read its answer, or give up after `within`.
pub(crate) fn run_gh(
    program: &Path,
    dir: &Path,
    args: &[&str],
    within: Duration,
) -> Result<String, String> {
    run_gh_cancel(program, dir, args, within, &Cancel::default())
}

/// The same, and stopped when `cancel` is set (REQ-604).
pub(crate) fn run_gh_cancel(
    program: &Path,
    dir: &Path,
    args: &[&str],
    within: Duration,
    cancel: &Cancel,
) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1");
    crate::cancel::run_child(cmd, "gh", Some(within), cancel, CAP)
}

/// The newest pull request of `branch`, looked up from `dir`.
pub fn lookup(program: &Path, dir: &Path, branch: &str) -> Result<Option<Pr>, String> {
    let text = gh(
        program,
        dir,
        &[
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "all",
            "--limit",
            "1",
            "--json",
            "number,state,statusCheckRollup",
        ],
    )?;
    parse(&text)
}

/// `gh` on the path, or the one `$<PREFIX>_GH` names.
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(identity::env_var("GH")) {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join("gh"))
            .find(|p| p.is_file())
    })
}

/// A worktree that a person is working in, to be looked up.
pub struct Job {
    pub workspace: String,
    pub dir: PathBuf,
    pub branch: String,
}

fn poll(shared: &Shared, program: &Path, said: &mut HashSet<String>) {
    // The lock is let go before `gh` runs: a guard in the `for` head would be held to its end.
    let jobs = shared.lock().github_jobs();
    for job in jobs {
        match lookup(program, &job.dir, &job.branch) {
            Ok(pr) => {
                said.remove(&job.workspace);
                shared.lock().github_apply(&job.workspace, pr);
            }
            // What was known stays, and so does the quiet: it is said once in the log.
            Err(why) => {
                if said.insert(job.workspace.clone()) {
                    eprintln!("github: {}: {why}", job.branch);
                }
            }
        }
    }
}

pub fn run(shared: Shared, stop: Arc<AtomicBool>, program: PathBuf, every: Duration) {
    let mut said = HashSet::new();
    while !stop.load(Ordering::SeqCst) {
        poll(&shared, &program, &mut said);
        let end = Instant::now() + every;
        while Instant::now() < end && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50).min(every));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, checks: &str) -> String {
        format!(r#"[{{"number":7,"state":"{state}","statusCheckRollup":{checks}}}]"#)
    }

    #[test]
    fn a_zero_merge_time_does_not_make_a_pull_request_merged() {
        let json = r#"[{"number":3,"state":"OPEN","mergedAt":"0001-01-01T00:00:00Z","statusCheckRollup":[]}]"#;
        assert!(!parse(json).unwrap().unwrap().merged);
    }

    #[test]
    fn a_branch_without_a_pull_request_has_none() {
        assert_eq!(parse("[]"), Ok(None));
        assert!(parse("nope").is_err());
        assert!(parse("{}").is_err());
    }

    #[test]
    fn a_merged_pull_request_is_merged_and_never_failing() {
        let failed = r#"[{"__typename":"CheckRun","conclusion":"FAILURE","status":"COMPLETED"}]"#;
        let merged = parse(&pr("MERGED", failed)).unwrap().unwrap();
        assert_eq!(
            (merged.number, merged.merged, merged.failing),
            (7, true, false)
        );
        // A closed one with a failed check is not asking for anything either.
        assert!(!parse(&pr("CLOSED", failed)).unwrap().unwrap().failing);
    }

    #[test]
    fn an_open_pull_request_fails_when_a_check_ended_in_failure() {
        let run = |conclusion: &str| {
            format!(
                r#"[{{"__typename":"CheckRun","conclusion":"{conclusion}","status":"COMPLETED"}}]"#
            )
        };
        for c in ["FAILURE", "TIMED_OUT", "STARTUP_FAILURE"] {
            assert!(parse(&pr("OPEN", &run(c))).unwrap().unwrap().failing, "{c}");
        }
        for c in ["SUCCESS", "CANCELLED", "SKIPPED", "NEUTRAL", ""] {
            assert!(
                !parse(&pr("OPEN", &run(c))).unwrap().unwrap().failing,
                "{c}"
            );
        }
        // A commit status (not a check run) says it with `state`; one still running is not a failure.
        let status = |s: &str| format!(r#"[{{"__typename":"StatusContext","state":"{s}"}}]"#);
        assert!(
            parse(&pr("OPEN", &status("ERROR")))
                .unwrap()
                .unwrap()
                .failing
        );
        assert!(
            !parse(&pr("OPEN", &status("PENDING")))
                .unwrap()
                .unwrap()
                .failing
        );
        assert!(!parse(&pr("OPEN", "null")).unwrap().unwrap().failing);
        assert!(!parse(&pr("OPEN", "[]")).unwrap().unwrap().failing);
    }
}
