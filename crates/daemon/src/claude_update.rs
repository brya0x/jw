//! Keeps `claude` up to date and says what it did (REQ-846 to REQ-853): at its start and every few
//! hours a thread reads `claude --version`, runs `claude update` (the one command that both looks
//! and installs) and reads the version again. The daemon does not parse what `update` prints.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use contract::JobKind;

use crate::cancel::Cancel;
use crate::server::Shared;

/// What `claude_update` of `config.toml` says (REQ-846).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Runs `claude update`.
    Auto,
    /// Only reads the installed version.
    Notify,
    /// Runs nothing.
    Off,
}

/// Where the thread reads its settings and how it waits; tests use short times.
#[derive(Debug, Clone)]
pub struct Watch {
    pub file: PathBuf,
    /// How often the file is read to see whether a check is due: at least once a minute (REQ-846).
    pub wake: Duration,
    /// How long `claude update` has to end (REQ-850).
    pub within: Duration,
}

impl Watch {
    pub fn new(file: PathBuf) -> Watch {
        Watch {
            file,
            wake: Duration::from_secs(30),
            within: Duration::from_secs(600),
        }
    }
}

/// The hours between checks when `config.toml` does not say.
const HOURS: u64 = 6;
/// The most `claude --version` has to answer.
const VERSION_WITHIN: Duration = Duration::from_secs(30);

/// The mode and the hours between checks that `config.toml` says; anything absent or invalid is the
/// default (REQ-846).
pub fn settings_in(config: &str) -> (Mode, u64) {
    let table = config.parse::<toml::Table>().unwrap_or_default();
    let mode = match table.get("claude_update").and_then(toml::Value::as_str) {
        Some("notify") => Mode::Notify,
        Some("off") => Mode::Off,
        _ => Mode::Auto,
    };
    let hours = table
        .get("claude_update_hours")
        .and_then(toml::Value::as_integer)
        .and_then(|h| u64::try_from(h).ok())
        .unwrap_or(HOURS);
    (mode, hours)
}

/// `2.1.309` out of `2.1.309 (Claude Code)`, which is how the engine says it too.
pub fn version_of(printed: &str) -> Option<String> {
    printed.split_whitespace().next().map(str::to_owned)
}

/// Whether a check is due: never when off, never again after `update` failed (REQ-850), at once
/// when none has been made, and then every `hours` (0 is only the first) (REQ-847).
pub fn due(mode: Mode, hours: u64, since_last: Option<Duration>, failed: bool) -> bool {
    match (mode, since_last) {
        (Mode::Off, _) => false,
        (Mode::Auto, _) if failed => false,
        (_, None) => true,
        (_, Some(since)) => hours > 0 && since >= Duration::from_secs(hours * 3600),
    }
}

fn version(exe: &Path) -> Result<String, String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--version");
    let printed = crate::cancel::run_child(
        cmd,
        "claude --version",
        Some(VERSION_WITHIN),
        &Cancel::default(),
        4096,
    )?;
    version_of(&printed).ok_or_else(|| "claude --version printed nothing".into())
}

/// The failure of a check: a job that failed, only when `update` is what runs (REQ-850). True when
/// it was one, so the thread does not run `update` again.
fn failed(shared: &Shared, mode: Mode, took: Duration, why: String) -> bool {
    if mode != Mode::Auto {
        return false;
    }
    let why = if why.ends_with("did not answer in time") {
        "se agotó el tiempo".to_owned()
    } else {
        why
    };
    shared
        .lock()
        .record_job(JobKind::UpdateClaude, "claude".into(), took, Err(why));
    true
}

/// One check (REQ-848, REQ-849, REQ-850); true when it failed. No lock is held while a command runs.
pub fn check(shared: &Shared, mode: Mode, within: Duration) -> bool {
    let env = shared.lock().env.clone();
    let Some(exe) = crate::engine::resolve_claude(None, &env) else {
        return failed(shared, mode, Duration::ZERO, "no se encontró claude".into());
    };
    let before = match version(&exe) {
        Ok(v) => v,
        Err(why) => return failed(shared, mode, Duration::ZERO, why),
    };
    shared.lock().set_installed_claude(Some(before.clone()));
    if mode != Mode::Auto {
        return false;
    }
    let began = Instant::now();
    let mut cmd = Command::new(&exe);
    cmd.arg("update").env("NO_COLOR", "1");
    let ran =
        crate::cancel::run_child(cmd, "claude update", Some(within), &Cancel::default(), 4096);
    let took = began.elapsed();
    if let Err(why) = ran {
        return failed(shared, mode, took, why);
    }
    match version(&exe) {
        Err(why) => failed(shared, mode, took, why),
        Ok(after) => {
            let mut core = shared.lock();
            if after != before {
                // One refresh for the new version and the job (REQ-853).
                core.remember_installed_claude(Some(after.clone()));
                core.record_job(
                    JobKind::UpdateClaude,
                    format!("claude {before} → {after}"),
                    took,
                    Ok(()),
                );
            }
            false
        }
    }
}

/// The thread: it reads the settings afresh at each wake, so a change needs no restart (REQ-846).
pub fn run(shared: Shared, stop: Arc<AtomicBool>, watch: Watch) {
    let mut last: Option<Instant> = None;
    let mut broke = false;
    while !stop.load(Ordering::SeqCst) {
        let config = std::fs::read_to_string(&watch.file).unwrap_or_default();
        let (mode, hours) = settings_in(&config);
        if mode == Mode::Off {
            shared.lock().set_installed_claude(None);
            last = None;
        } else if due(mode, hours, last.map(|t| t.elapsed()), broke) {
            last = Some(Instant::now());
            broke |= check(&shared, mode, watch.within);
        }
        let end = Instant::now() + watch.wake;
        while Instant::now() < end && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50).min(watch.wake));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_says_the_mode_and_the_hours_and_anything_else_is_the_default() {
        assert_eq!(settings_in(""), (Mode::Auto, 6));
        assert_eq!(settings_in("not toml {"), (Mode::Auto, 6));
        assert_eq!(settings_in("claude_update = \"off\""), (Mode::Off, 6));
        assert_eq!(
            settings_in("claude_update = \"notify\"\nclaude_update_hours = 0"),
            (Mode::Notify, 0)
        );
        assert_eq!(settings_in("claude_update = \"quiet\""), (Mode::Auto, 6));
        assert_eq!(settings_in("claude_update = false"), (Mode::Auto, 6));
        assert_eq!(settings_in("claude_update_hours = -2"), (Mode::Auto, 6));
        assert_eq!(settings_in("claude_update_hours = \"3\""), (Mode::Auto, 6));
        assert_eq!(settings_in("claude_update_hours = 12"), (Mode::Auto, 12));
    }

    #[test]
    fn the_version_is_the_first_word_of_what_claude_prints() {
        assert_eq!(
            version_of("2.1.309 (Claude Code)\n").as_deref(),
            Some("2.1.309")
        );
        assert_eq!(version_of("2.1.312").as_deref(), Some("2.1.312"));
        assert_eq!(version_of("  \n"), None);
    }

    #[test]
    fn a_check_is_due_at_the_start_then_every_so_many_hours_and_never_after_a_failure() {
        let h = |n: u64| Some(Duration::from_secs(n * 3600));
        assert!(due(Mode::Auto, 6, None, false));
        assert!(due(Mode::Notify, 6, None, false));
        assert!(!due(Mode::Auto, 6, h(5), false));
        assert!(due(Mode::Auto, 6, h(6), false));
        assert!(!due(Mode::Auto, 0, h(100), false), "0 is only at the start");
        assert!(!due(Mode::Off, 6, None, false));
        assert!(
            !due(Mode::Auto, 6, h(100), true),
            "no second run after a failure"
        );
        assert!(!due(Mode::Auto, 6, None, true));
        assert!(due(Mode::Notify, 6, h(7), true), "notify never runs update");
    }
}
