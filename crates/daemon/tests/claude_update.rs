//! N178a: the daemon keeps `claude` up to date and says what it did (REQ-846 to REQ-853). The
//! `claude` here is a stand-in of `sh` builtins on a PATH of its own: nothing reaches the person's.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT};
use daemon::claude_update::Watch;
use serde_json::Value;

struct Rig {
    sb: Sandbox,
    log: PathBuf,
    config: PathBuf,
}

impl Rig {
    /// A `claude` at `version` whose `update` runs `update` (shell) and logs each call.
    fn new(version: &str, update: &str) -> Rig {
        let sb = Sandbox::new();
        let bin = sb.dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let d = sb.dir.display();
        std::fs::write(sb.dir.join("ver"), format!("{version}\n")).unwrap();
        std::fs::write(sb.dir.join("next"), "2.1.312\n").unwrap();
        let script = format!(
            "#!/bin/sh\nD='{d}'\ncase \"$1\" in\n  --version) echo version >> \"$D/log\"; read v < \"$D/ver\"; echo \"$v (Claude Code)\";;\n  update) echo update >> \"$D/log\"; {update};;\nesac\n"
        );
        let exe = bin.join("claude");
        std::fs::write(&exe, script).unwrap();
        std::process::Command::new("/bin/chmod")
            .args(["+x"])
            .arg(&exe)
            .status()
            .unwrap();
        Rig {
            log: sb.dir.join("log"),
            config: sb.dir.join("config.toml"),
            sb,
        }
    }

    const OK: &'static str = "read n < \"$D/next\"; echo \"$n\" > \"$D/ver\"";

    fn start(&self, within: Duration) -> daemon::Daemon {
        let mut config = self.sb.config();
        // Only the stand-in's folder: a missing one must not fall through to the real `claude`.
        config.env = vec![("PATH".into(), self.sb.dir.join("bin").display().to_string())];
        config.claude_update = Some(Watch {
            file: self.config.clone(),
            wake: Duration::from_millis(50),
            within,
        });
        daemon::Daemon::start(config).expect("daemon starts")
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wait_call(&self, call: &str) {
        let end = Instant::now() + WAIT;
        while !self.calls().iter().any(|c| c == call) {
            assert!(Instant::now() < end, "claude never got `{call}`");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn jobs_of(c: &wire::Client) -> Vec<Value> {
    c.nodes[&c.root]["jobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn open_session(sb: &Sandbox) -> wire::Client {
    let mut c = sb.client();
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());
    c
}

/// The jobs once there is one, or none after `quiet`.
fn jobs_after(c: &mut wire::Client, quiet: Duration) -> Vec<Value> {
    c.wait_until(quiet, |c| !jobs_of(c).is_empty()).unwrap();
    jobs_of(c)
}

#[test]
fn a_new_version_is_a_job_that_went_well_with_no_workspace() {
    let rig = Rig::new("2.1.309", Rig::OK);
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    let jobs = jobs_after(&mut c, WAIT);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["kind"], "update_claude");
    assert_eq!(jobs[0]["state"], "done");
    assert_eq!(jobs[0]["name"], "claude 2.1.309 → 2.1.312");
    assert!(
        jobs[0].get("workspace").is_none(),
        "an update has no workspace"
    );
    assert!(jobs[0]["duration_ms"].is_u64());
    assert_ne!(jobs[0]["cancellable"], true);
    assert_eq!(rig.calls(), ["version", "update", "version"]);
}

#[test]
fn the_same_version_leaves_no_job() {
    let rig = Rig::new("2.1.309", ":");
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    rig.wait_call("update");
    assert!(jobs_after(&mut c, Duration::from_millis(400)).is_empty());
    assert_eq!(rig.calls(), ["version", "update", "version"], "one check");
}

#[test]
fn a_failed_update_is_one_failed_job_with_what_the_command_said() {
    let rig = Rig::new("2.1.309", "echo 'EACCES: permission denied' >&2; exit 1");
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    let jobs = jobs_after(&mut c, WAIT);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["kind"], "update_claude");
    assert_eq!(jobs[0]["state"], "failed");
    assert_eq!(jobs[0]["name"], "claude");
    assert_eq!(jobs[0]["detail"], "EACCES: permission denied");
    // No second run: one `update` however long it waits.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.calls().iter().filter(|c| *c == "update").count(), 1);
}

#[test]
fn an_update_that_never_ends_is_stopped_and_fails() {
    let rig = Rig::new("2.1.309", "while :; do :; done");
    let _daemon = rig.start(Duration::from_millis(300));
    let mut c = open_session(&rig.sb);
    let jobs = jobs_after(&mut c, WAIT);
    assert_eq!(jobs[0]["state"], "failed");
    assert_eq!(jobs[0]["detail"], "se agotó el tiempo");
}

#[test]
fn no_claude_on_the_path_is_a_failed_job() {
    let rig = Rig::new("2.1.309", Rig::OK);
    std::fs::remove_file(rig.sb.dir.join("bin/claude")).unwrap();
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    let jobs = jobs_after(&mut c, WAIT);
    assert_eq!(jobs[0]["state"], "failed");
    assert_eq!(jobs[0]["detail"], "no se encontró claude");
}

#[test]
fn notify_only_reads_the_version_and_never_runs_update() {
    let rig = Rig::new("2.1.309", Rig::OK);
    std::fs::write(&rig.config, "claude_update = \"notify\"\n").unwrap();
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    rig.wait_call("version");
    assert!(jobs_after(&mut c, Duration::from_millis(400)).is_empty());
    assert_eq!(rig.calls(), ["version"]);
}

#[test]
fn off_runs_nothing_and_the_config_is_read_afresh() {
    let rig = Rig::new("2.1.309", Rig::OK);
    std::fs::write(&rig.config, "claude_update = \"off\"\n").unwrap();
    let _daemon = rig.start(Duration::from_secs(5));
    let mut c = open_session(&rig.sb);
    assert!(jobs_after(&mut c, Duration::from_millis(500)).is_empty());
    assert!(
        rig.calls().is_empty(),
        "off runs no process: {:?}",
        rig.calls()
    );
    // Turned on while the daemon runs: the next wake checks, with no restart (REQ-846).
    std::fs::write(&rig.config, "claude_update = \"auto\"\n").unwrap();
    let jobs = jobs_after(&mut c, WAIT);
    assert_eq!(jobs[0]["state"], "done", "{jobs:?}");
}
