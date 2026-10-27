//! `jw update` and `jw android install`: the repository's latest version, fetched into a
//! temporary directory, built there, installed, and the directory deleted (N182, REQ-977 to
//! REQ-986). The only programs the clone makes run are `cargo` and `./gradlew`.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

type Res = Result<ExitCode, String>;

/// What a signal ends the command with, as a shell reports it for `SIGINT`.
const INTERRUPTED: u8 = 130;
/// How long a child that was asked to end has before its group is killed.
const GRACE: Duration = Duration::from_secs(5);

enum Fail {
    Interrupted,
    Error(String),
}

impl From<String> for Fail {
    fn from(e: String) -> Fail {
        Fail::Error(e)
    }
}

type Out<T> = Result<T, Fail>;

#[derive(Default, Debug, PartialEq)]
struct Opts {
    git_ref: Option<String>,
    keep: bool,
    cache: bool,
    restart: bool,
    debug: bool,
    serial: Option<String>,
}

/// A `<ref>` is a branch, a tag or a full hash, and is never an option of `git` (REQ-978).
pub(crate) fn valid_ref(r: &str) -> bool {
    (1..=200).contains(&r.len())
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && !r.starts_with(['-', '/'])
        && !r.ends_with('/')
        && !r.ends_with(".lock")
        && !r.contains("..")
        && !r.contains("//")
}

/// `flags` are the ones the command takes; the two that take a value are `--ref` and `--serial`.
fn parse(args: &[&str], flags: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        if !flags.contains(&a) {
            return Err(format!("unknown argument `{a}`"));
        }
        let mut value = || {
            it.next()
                .map(|v| v.to_string())
                .ok_or(format!("{a} needs a value"))
        };
        match a {
            "--ref" => {
                let r = value()?;
                if !valid_ref(&r) {
                    return Err(format!(
                        "`{r}` is not a branch, a tag or a commit hash (letters, digits and . _ / -, not starting with -)"
                    ));
                }
                o.git_ref = Some(r);
            }
            "--serial" => o.serial = Some(value()?),
            "--keep" => o.keep = true,
            "--cache" => o.cache = true,
            "--restart" => o.restart = true,
            "--debug" => o.debug = true,
            _ => unreachable!("every flag of the list is handled"),
        }
    }
    Ok(o)
}

/// The repository to fetch, and whether the test variable named it (REQ-977).
fn repo() -> (String, bool) {
    match std::env::var(identity::env_var("UPDATE_REPO")) {
        Ok(url) if !url.is_empty() => (url, true),
        _ => (identity::get().repo.clone(), false),
    }
}

/// May `dir` be deleted? Only a directory made here: directly inside the temporary directory
/// and named with the prefix, never the temporary directory itself, `/` or a home (REQ-979).
fn safe_to_remove(dir: &Path, tmp: &Path, prefix: &str) -> bool {
    let (Ok(dir), Ok(tmp)) = (dir.canonicalize(), tmp.canonicalize()) else {
        return false;
    };
    dir.parent() == Some(tmp.as_path())
        && dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.len() > prefix.len() && n.starts_with(prefix))
}

fn prefix() -> String {
    format!("{}-update-", identity::get().name)
}

/// The temporary directory of a command: deleted when this is dropped, unless it is kept.
struct Work {
    path: PathBuf,
    keep: bool,
}

impl Work {
    fn make(keep: bool) -> Result<Work, String> {
        let path = tempfile::Builder::new()
            .prefix(&prefix())
            .tempdir()
            .map_err(|e| format!("cannot make a temporary directory: {e}"))?
            .keep();
        Ok(Work { path, keep })
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if safe_to_remove(&self.path, &std::env::temp_dir(), &prefix()) {
            if let Err(e) = std::fs::remove_dir_all(&self.path) {
                eprintln!("could not delete {}: {e}", self.path.display());
            }
        } else {
            eprintln!("left {}: not a directory made here", self.path.display());
        }
    }
}

/// A command's work: its directory and the flag that a signal sets.
struct Job {
    work: Work,
    stop: Arc<AtomicBool>,
}

impl Job {
    fn start(keep: bool) -> Result<Job, String> {
        let stop = Arc::new(AtomicBool::new(false));
        for signal in [SIGINT, SIGTERM, SIGHUP] {
            signal_hook::flag::register(signal, stop.clone())
                .map_err(|e| format!("cannot catch signals: {e}"))?;
        }
        Ok(Job {
            work: Work::make(keep)?,
            stop,
        })
    }

    fn dir(&self) -> &Path {
        &self.work.path
    }

    fn interrupted(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Run a program in a process group of its own with this terminal's output; a signal to
    /// this process ends the group before anything else is done (REQ-986).
    fn run(&self, mut cmd: Command) -> Out<()> {
        if self.interrupted() {
            return Err(Fail::Interrupted);
        }
        let program = cmd.get_program().to_string_lossy().into_owned();
        cmd.stdin(Stdio::null()).process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| Fail::Error(format!("cannot run {program}: {e}")))?;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => {
                    return Err(Fail::Error(format!("{program} failed ({status})")));
                }
                Ok(None) => {}
                Err(e) => return Err(Fail::Error(format!("{program}: {e}"))),
            }
            if self.interrupted() {
                end_group(&mut child);
                return Err(Fail::Interrupted);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn git(&self, args: &[&str], allow_file: bool) -> Out<()> {
        let mut cmd = Command::new("git");
        cmd.args(args).env("GIT_TERMINAL_PROMPT", "0").env(
            "GIT_ALLOW_PROTOCOL",
            if allow_file { "https:file" } else { "https" },
        );
        self.run(cmd)
    }

    /// The repository at `git_ref` (its `HEAD` when none) as the files of the directory: `fetch`
    /// takes a hash as `clone --branch` does not, and a fresh `init` has no hook and no submodule (RAT-162).
    fn fetch(&self, url: &str, git_ref: Option<&str>, allow_file: bool) -> Out<()> {
        let dir = self.dir().to_string_lossy().into_owned();
        self.git(&["init", "-q", &dir], allow_file)?;
        let git_ref = git_ref.unwrap_or("HEAD");
        self.git(
            &[
                "-C", &dir, "fetch", "-q", "--depth", "1", "--", url, git_ref,
            ],
            allow_file,
        )?;
        self.git(
            &[
                "-C",
                &dir,
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "FETCH_HEAD",
            ],
            allow_file,
        )
    }
}

/// A repository fetched into a temporary directory that is deleted when this drops, a signal
/// included (N184b, REQ-1014): the means of `update`, for the skillset.
pub(crate) struct Checkout {
    job: Job,
}

impl Checkout {
    pub(crate) fn dir(&self) -> &Path {
        self.job.dir()
    }
}

/// Fetch `url` at `git_ref` (its `HEAD` when none) and the commit that came; `allow_file` lets the
/// `file` protocol through, for the tests of the commands. An interrupt is an error here.
pub(crate) fn checkout(
    url: &str,
    git_ref: Option<&str>,
    allow_file: bool,
) -> Result<(Checkout, String), String> {
    let job = Job::start(false)?;
    let fetched = job.fetch(url, git_ref, allow_file);
    if let Err(f) = fetched {
        return Err(match f {
            Fail::Interrupted => "interrupted".to_owned(),
            Fail::Error(e) => e,
        });
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(job.dir())
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git rev-parse failed ({})", out.status));
    }
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok((Checkout { job }, commit))
}

/// The commit a branch (its `HEAD` when none) of `url` is at now, from `git ls-remote`: it fetches
/// nothing and never asks for a password (N184b, REQ-1010).
pub(crate) fn ls_remote(
    url: &str,
    branch: Option<&str>,
    allow_file: bool,
) -> Result<String, String> {
    let refspec = branch.map_or_else(|| "HEAD".to_owned(), |b| format!("refs/heads/{b}"));
    let out = Command::new("git")
        .args(["ls-remote", "--", url, &refspec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_ALLOW_PROTOCOL",
            if allow_file { "https:file" } else { "https" },
        )
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "15")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-remote {url} failed ({})", out.status));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| format!("{url} has no {refspec}"))
}

/// `SIGINT` to the group, then `SIGKILL` if it is still there after the grace; waits for it
/// either way, so nothing writes into the directory when it is deleted.
fn end_group(child: &mut Child) {
    let group = format!("-{}", child.id());
    let signal = |s: &str| {
        let _ = Command::new("kill").args([s, "--", &group]).status();
    };
    signal("-INT");
    let end = Instant::now() + GRACE;
    while Instant::now() < end {
        if child.try_wait().is_ok_and(|s| s.is_some()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal("-KILL");
    let _ = child.wait();
}

/// A failed or interrupted command: the directory is gone when its `Job` drops.
fn failed(f: Fail) -> Res {
    match f {
        Fail::Interrupted => {
            eprintln!("interrupted");
            Ok(ExitCode::from(INTERRUPTED))
        }
        Fail::Error(e) => Err(e),
    }
}

fn say_repo(url: &str, from_env: bool, git_ref: Option<&str>) {
    let from = if from_env {
        format!(" (from {})", identity::env_var("UPDATE_REPO"))
    } else {
        String::new()
    };
    eprintln!("repo {url}{from}, ref {}", git_ref.unwrap_or("HEAD"));
}

fn keep_note(job: &Job) {
    if job.work.keep {
        eprintln!("kept {}", job.dir().display());
    }
}

/// `[workspace.package] version` of the clone's `Cargo.toml`: read, not run.
fn clone_version(dir: &Path) -> Result<String, String> {
    let path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    doc["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or(format!(
            "{} has no [workspace.package] version",
            path.display()
        ))
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Fetch and build; the version of what was installed.
fn build(job: &Job, o: &Opts, url: &str, from_env: bool) -> Out<String> {
    job.fetch(url, o.git_ref.as_deref(), from_env)?;
    let new = clone_version(job.dir())?;
    eprintln!("building with cargo: it runs the code of that repository");
    let mut cargo = Command::new("cargo");
    // The crate of the binary has the binary's name (a test below checks it).
    cargo
        .args(["install", "--path"])
        .arg(format!("crates/{}", identity::get().name))
        .args(["--locked", "--force"])
        .current_dir(job.dir());
    // Without `--cache` the target directory is inside the one that is deleted (REQ-982).
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(_) => None,
        None if o.cache => Some(identity::state_dir().join("update-target")),
        None => Some(job.dir().join("target")),
    };
    if let Some(target) = &target {
        cargo.env("CARGO_TARGET_DIR", target);
    }
    job.run(cargo)?;
    if let Some(target) = target.filter(|_| o.cache) {
        eprintln!(
            "cache {}: {} MB",
            target.display(),
            dir_size(&target) / 1_000_000
        );
    }
    Ok(new)
}

/// The binary `cargo install` just wrote: `$CARGO_INSTALL_ROOT/bin`, else `$CARGO_HOME/bin`, else
/// `~/.cargo/bin` (REQ-1002).
fn installed_binary(
    install_root: Option<PathBuf>,
    cargo_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let root = install_root
        .or(cargo_home)
        .or_else(|| Some(home?.join(".cargo")))?;
    Some(root.join("bin").join(&identity::get().name))
}

fn installed_binary_here() -> Option<PathBuf> {
    let var = |v: &str| {
        std::env::var_os(v)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    installed_binary(var("CARGO_INSTALL_ROOT"), var("CARGO_HOME"), var("HOME"))
}

/// A person who has the first skill gets the three refreshed by the binary just installed, whose texts
/// are the new ones; with none installed nothing is (REQ-1002). It never fails the update.
fn refresh_skills() {
    if !crate::agent::skill_installed() {
        return;
    }
    let name = &identity::get().name;
    let Some(new) = installed_binary_here().filter(|b| b.is_file()) else {
        println!(
            "skills not refreshed: the installed {name} is not where cargo puts it: run {name} skill install"
        );
        return;
    };
    match Command::new(&new).args(["skill", "install"]).status() {
        Ok(status) if status.success() => {}
        Ok(status) => println!(
            "skills not refreshed: {} skill install failed ({status})",
            new.display()
        ),
        Err(e) => println!("skills not refreshed: {}: {e}", new.display()),
    }
}

pub fn update(args: &[&str]) -> Res {
    let o = parse(args, &["--ref", "--cache", "--keep", "--restart"])?;
    let (url, from_env) = repo();
    say_repo(&url, from_env, o.git_ref.as_deref());
    let job = Job::start(o.keep)?;
    let built = build(&job, &o, &url, from_env);
    keep_note(&job);
    let new = match built {
        Ok(new) => new,
        Err(f) => return failed(f),
    };
    let name = &identity::get().name;
    println!("{name} {} → {new}", crate::version::VERSION);
    refresh_skills();
    let Some(daemon) = crate::version::daemon() else {
        return Ok(ExitCode::SUCCESS);
    };
    if o.restart {
        let own = std::env::var_os(identity::env_var("PANE_ID"))
            .map(|_| ", this command's own pane among them")
            .unwrap_or_default();
        eprintln!("--restart: stopping the daemon; every pane closes{own}");
        drop(job);
        return crate::stop();
    }
    if daemon.version == new {
        println!("the daemon already runs {new}");
    } else {
        println!(
            "the daemon is still {}: {name} stop and open it again",
            daemon.version
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `adb` of the SDK that `ANDROID_HOME` (or `ANDROID_SDK_ROOT`) names, and no other (REQ-983).
fn adb() -> Result<PathBuf, String> {
    let sdk = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .find_map(|v| std::env::var_os(v).filter(|s| !s.is_empty()))
        .ok_or("set ANDROID_HOME to the Android SDK: adb is in its platform-tools")?;
    let adb = Path::new(&sdk).join("platform-tools/adb");
    if adb.is_file() {
        Ok(adb)
    } else {
        Err(format!(
            "{} not found: ANDROID_HOME is not an Android SDK",
            adb.display()
        ))
    }
}

/// The phone to install on, from the output of `adb devices` (REQ-983): the one that is `device`,
/// or the one `serial` names; any other case is said with the list.
fn pick_device(output: &str, serial: Option<&str>) -> Result<String, String> {
    let devices: Vec<(&str, &str)> = output
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?, it.next()?))
        })
        .collect();
    let list = devices
        .iter()
        .map(|(s, state)| format!("  {s}  {state}"))
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(serial) = serial {
        return match devices.iter().find(|(s, _)| *s == serial) {
            Some((s, "device")) => Ok(s.to_string()),
            Some((_, state)) => Err(format!("{serial} is {state}, not ready")),
            None if devices.is_empty() => Err(format!("{serial} is not connected: no device")),
            None => Err(format!(
                "{serial} is not connected; the devices are:\n{list}"
            )),
        };
    }
    let ready: Vec<&str> = devices
        .iter()
        .filter(|(_, state)| *state == "device")
        .map(|(s, _)| *s)
        .collect();
    match ready.as_slice() {
        [one] => Ok(one.to_string()),
        [] if devices.is_empty() => {
            Err("no device: connect the phone with USB debugging on".into())
        }
        [] => Err(format!("no device is ready:\n{list}")),
        _ => Err(format!("several devices; pick one with --serial:\n{list}")),
    }
}

const NEVER_UNINSTALLS: &str = "the app is never uninstalled by this command: INSTALL_FAILED_UPDATE_INCOMPATIBLE means the phone holds the app signed with another key, and uninstalling it would erase its data";

pub fn android(args: &[&str]) -> Res {
    let ["install", rest @ ..] = args else {
        return Err(format!(
            "usage: {0} android install --debug [--serial <id>] [--ref <ref>] [--keep]",
            identity::get().name
        ));
    };
    let o = parse(rest, &["--debug", "--serial", "--ref", "--keep"])?;
    if !o.debug {
        return Err(format!(
            "there is no signed release build yet: use `{} android install --debug`",
            identity::get().name
        ));
    }
    let adb = adb()?;
    let listed = Command::new(&adb)
        .arg("devices")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", adb.display()))?;
    let serial = pick_device(
        &String::from_utf8_lossy(&listed.stdout),
        o.serial.as_deref(),
    )?;
    eprintln!("device {serial}");
    let (url, from_env) = repo();
    say_repo(&url, from_env, o.git_ref.as_deref());
    let job = Job::start(o.keep)?;
    let built = (|| {
        job.fetch(&url, o.git_ref.as_deref(), from_env)?;
        eprintln!("building with gradle: it runs the code of that repository");
        let mut gradle = Command::new(job.dir().join("android/gradlew"));
        gradle
            .args(["--no-daemon", ":app:installDebug"])
            .current_dir(job.dir().join("android"))
            .env("ANDROID_SERIAL", &serial);
        job.run(gradle).map_err(|f| match f {
            Fail::Error(e) => Fail::Error(format!("{e}\n{NEVER_UNINSTALLS}")),
            other => other,
        })
    })();
    keep_note(&job);
    match built {
        Ok(()) => {
            println!("installed on {serial}");
            Ok(ExitCode::SUCCESS)
        }
        Err(f) => failed(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_binary_is_where_cargo_puts_it() {
        let p = |s: &str| Some(PathBuf::from(s));
        let bin = &identity::get().name;
        let at = |root: &str| Some(PathBuf::from(format!("{root}/bin/{bin}")));
        assert_eq!(installed_binary(p("/r"), p("/c"), p("/h")), at("/r"));
        assert_eq!(installed_binary(None, p("/c"), p("/h")), at("/c"));
        assert_eq!(installed_binary(None, None, p("/h")), at("/h/.cargo"));
        assert_eq!(installed_binary(None, None, None), None);
    }

    #[test]
    fn the_crate_of_the_binary_is_in_the_directory_the_identity_names() {
        assert!(Path::new(env!("CARGO_MANIFEST_DIR")).ends_with(&identity::get().name));
    }

    #[test]
    fn a_ref_is_a_name_or_a_hash_and_never_an_option() {
        for ok in [
            "main",
            "v1.2.3",
            "feat/self-update",
            &"a".repeat(40),
            &"0".repeat(64),
        ] {
            assert!(valid_ref(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--upload-pack=touch /tmp/x",
            "/main",
            "main/",
            "a..b",
            "a//b",
            "x.lock",
            "a b",
            "a;b",
            "a$(b)",
            "é",
            &"a".repeat(201),
        ] {
            assert!(!valid_ref(bad), "{bad}");
        }
    }

    #[test]
    fn the_flags_of_each_command_are_its_own() {
        let o = parse(
            &["--ref", "v1", "--cache", "--keep", "--restart"],
            &["--ref", "--cache", "--keep", "--restart"],
        )
        .unwrap();
        assert_eq!(
            o,
            Opts {
                git_ref: Some("v1".into()),
                keep: true,
                cache: true,
                restart: true,
                ..Opts::default()
            }
        );
        assert!(parse(&["--serial", "x"], &["--ref"]).is_err());
        assert!(parse(&["--ref"], &["--ref"]).is_err());
        assert!(parse(&["--ref", "-x"], &["--ref"]).is_err());
        assert!(parse(&["oops"], &["--ref"]).is_err());
    }

    #[test]
    fn only_a_directory_made_here_is_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = "p-update-";
        let made = tmp.path().join("p-update-abc");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(made.join("deep")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        assert!(safe_to_remove(&made, tmp.path(), prefix));
        // the temporary directory itself, a sibling without the prefix, the prefix alone,
        // something deeper, `/`, a path that is not there and an empty one
        assert!(!safe_to_remove(tmp.path(), tmp.path(), prefix));
        assert!(!safe_to_remove(&other, tmp.path(), prefix));
        std::fs::create_dir(tmp.path().join(prefix)).unwrap();
        assert!(!safe_to_remove(
            &tmp.path().join(prefix),
            tmp.path(),
            prefix
        ));
        assert!(!safe_to_remove(&made.join("deep"), tmp.path(), prefix));
        assert!(!safe_to_remove(Path::new("/"), tmp.path(), prefix));
        assert!(!safe_to_remove(
            &tmp.path().join("p-update-gone"),
            tmp.path(),
            prefix
        ));
        assert!(!safe_to_remove(Path::new(""), tmp.path(), prefix));
        // a link out of the temporary directory is judged by where it goes
        let link = tmp.path().join("p-update-link");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(!safe_to_remove(&link, tmp.path(), prefix));
    }

    const ONE: &str = "List of devices attached\nR58M123ABC\tdevice\n\n";

    #[test]
    fn the_only_ready_device_is_chosen() {
        assert_eq!(pick_device(ONE, None).unwrap(), "R58M123ABC");
        // the adb server's start is said before the header
        let started = "* daemon not running; starting now at tcp:5037\n* daemon started successfully\nList of devices attached\nemulator-5554\tdevice\n";
        assert_eq!(pick_device(started, None).unwrap(), "emulator-5554");
        let two = "List of devices attached\nA\tunauthorized\nB\tdevice\nC\toffline\n";
        assert_eq!(pick_device(two, None).unwrap(), "B");
    }

    #[test]
    fn several_none_or_unready_devices_stop_with_the_list() {
        let two = "List of devices attached\nA\tdevice\nB\tdevice\n";
        let e = pick_device(two, None).unwrap_err();
        assert!(e.contains("--serial") && e.contains("A  device") && e.contains("B  device"));
        assert!(
            pick_device("List of devices attached\n\n", None)
                .unwrap_err()
                .contains("no device")
        );
        assert!(
            pick_device("List of devices attached\n", None)
                .unwrap_err()
                .contains("no device")
        );
        assert!(pick_device("", None).unwrap_err().contains("no device"));
        let un = "List of devices attached\nA\tunauthorized\nB\toffline\n";
        let e = pick_device(un, None).unwrap_err();
        assert!(e.contains("no device is ready") && e.contains("A  unauthorized"));
    }

    #[test]
    fn serial_names_a_device_that_is_listed_and_ready() {
        let two = "List of devices attached\nA\tdevice\nB\tdevice\nC\tunauthorized\n";
        assert_eq!(pick_device(two, Some("B")).unwrap(), "B");
        assert!(
            pick_device(two, Some("C"))
                .unwrap_err()
                .contains("unauthorized")
        );
        assert!(
            pick_device(two, Some("Z"))
                .unwrap_err()
                .contains("A  device")
        );
        assert!(pick_device("List of devices attached\n", Some("Z")).is_err());
    }
}
