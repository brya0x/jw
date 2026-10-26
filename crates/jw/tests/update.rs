//! `update`, `android install` and `version` (N182), run as the built binary against a repository
//! made in a temporary directory (`<PREFIX>_UPDATE_REPO`, REQ-977), a private `TMPDIR` and
//! install root, a fake `adb` and a fake `gradlew`. No network, no `~/.cargo/bin`, no phone and
//! no daemon of the person: the socket is private.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const JW: &str = env!("CARGO_BIN_EXE_jw");

fn write(path: &Path, text: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-C",
        ])
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A gradle that records what it was asked and with which serial, and can fail or wait.
const GRADLEW: &str = "#!/bin/sh
echo \"$ANDROID_SERIAL $@\" > \"$FAKE_OUT\"
if [ -n \"$FAKE_SLEEP\" ]; then echo $$ > \"$FAKE_PID\"; exec sleep 30; fi
exit ${FAKE_RC:-0}
";

const ADB: &str = "#!/bin/sh\nprintf '%b' \"$FAKE_DEVICES\"\n";

struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new(broken: bool) -> Rig {
        let rig = Rig {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir(rig.tmp()).unwrap();
        let repo = rig.repo();
        write(
            &repo.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"9.9.9\"\nedition = \"2024\"\n",
            0o644,
        );
        // the crate and the binary have the identity's name, as in the repository itself
        let name = &identity::get().name;
        write(
            &repo.join("Cargo.lock"),
            &format!("version = 4\n\n[[package]]\nname = \"{name}\"\nversion = \"9.9.9\"\n"),
            0o644,
        );
        write(
            &repo.join(format!("crates/{name}/Cargo.toml")),
            &format!(
                "[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n\n[[bin]]\nname = \"{name}\"\npath = \"src/main.rs\"\n"
            ),
            0o644,
        );
        let main = if broken {
            "fn main( {"
        } else {
            "fn main() { println!(\"installed\"); }"
        };
        write(
            &repo.join(format!("crates/{name}/src/main.rs")),
            main,
            0o644,
        );
        write(&repo.join("android/gradlew"), GRADLEW, 0o755);
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "x"]);
        write(&rig.sdk().join("platform-tools/adb"), ADB, 0o755);
        rig
    }

    fn p(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn tmp(&self) -> PathBuf {
        self.p("tmp")
    }
    fn repo(&self) -> PathBuf {
        self.p("repo")
    }
    fn sdk(&self) -> PathBuf {
        self.p("sdk")
    }

    /// `jw <args>` with everything private; `devices` is what the fake `adb` prints.
    fn jw(&self, args: &[&str], devices: &str) -> Command {
        let mut c = Command::new(JW);
        c.args(args)
            .env("TMPDIR", self.tmp())
            .env(
                identity::env_var("UPDATE_REPO"),
                format!("file://{}", self.repo().display()),
            )
            .env(identity::env_var("STATE_DIR"), self.p("state"))
            .env(identity::env_var("SOCKET"), self.p("s.sock"))
            .env_remove(identity::env_var("PANE_ID"))
            .env("CARGO_INSTALL_ROOT", self.p("root"))
            .env("CARGO_NET_OFFLINE", "true")
            .env_remove("CARGO_TARGET_DIR")
            .env("ANDROID_HOME", self.sdk())
            .env("FAKE_DEVICES", devices)
            .env("FAKE_OUT", self.p("gradle.out"))
            .env("FAKE_PID", self.p("gradle.pid"))
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str], devices: &str) -> Output {
        self.jw(args, devices).output().unwrap()
    }

    fn run_env(&self, args: &[&str], devices: &str, var: (&str, &str)) -> Output {
        self.jw(args, devices).env(var.0, var.1).output().unwrap()
    }

    /// The directories of a command that are still in the private `TMPDIR`.
    fn left(&self) -> Vec<String> {
        let prefix = format!("{}-update-", identity::get().name);
        std::fs::read_dir(self.tmp())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&prefix))
            .collect()
    }

    fn installed(&self) -> bool {
        self.p("root/bin").join(&identity::get().name).exists()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const ONE: &str = "List of devices attached\\nR58M123\\tdevice\\n\\n";
const TWO: &str = "List of devices attached\\nA1\\tdevice\\nB2\\tdevice\\n";

#[test]
fn update_installs_what_the_repository_has_and_deletes_its_directory() {
    let rig = Rig::new(false);
    let out = rig.run(&["update"], "");
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}{stderr}");
    let name = &identity::get().name;
    assert!(
        stdout.contains(&format!("{name} {} → 9.9.9", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    assert!(
        stderr.contains(&format!("from {}", identity::env_var("UPDATE_REPO"))),
        "{stderr}"
    );
    assert!(
        stderr.contains("it runs the code of that repository"),
        "{stderr}"
    );
    assert!(rig.installed());
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn update_of_a_hash_and_of_a_branch_and_of_a_name_that_is_not_there() {
    let rig = Rig::new(false);
    let sha = git(&rig.repo(), &["rev-parse", "HEAD"]);
    for good in [sha.as_str(), "main"] {
        let out = rig.run(&["update", "--ref", good], "");
        assert!(out.status.success(), "{good}: {}", text(&out.stderr));
        assert_eq!(rig.left(), Vec::<String>::new());
    }
    let out = rig.run(&["update", "--ref", "nope"], "");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn a_ref_that_is_an_option_is_refused_before_a_directory_is_made() {
    let rig = Rig::new(false);
    for bad in ["--upload-pack=touch /tmp/never", "-x", "a..b"] {
        let out = rig.run(&["update", "--ref", bad], "");
        assert_eq!(out.status.code(), Some(1), "{bad}");
        assert!(
            text(&out.stderr).contains("not a branch"),
            "{}",
            text(&out.stderr)
        );
        assert!(!rig.installed());
    }
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn update_deletes_its_directory_when_the_build_fails_and_installs_nothing() {
    let rig = Rig::new(true);
    let out = rig.run(&["update"], "");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("cargo failed"));
    assert!(!rig.installed());
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn keep_leaves_the_directory_and_says_where() {
    let rig = Rig::new(false);
    let out = rig.run(&["update", "--keep"], "");
    assert!(out.status.success(), "{}", text(&out.stderr));
    let left = rig.left();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(rig.tmp().join(&left[0]).join("Cargo.toml").exists());
    assert!(text(&out.stderr).contains(&format!("kept {}", rig.tmp().join(&left[0]).display())));
}

#[test]
fn android_install_without_debug_runs_nothing() {
    let rig = Rig::new(false);
    let mut c = rig.jw(&["android", "install"], ONE);
    c.env_remove("ANDROID_HOME").env_remove("ANDROID_SDK_ROOT");
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("no signed release build"),
        "{}",
        text(&out.stderr)
    );
    assert!(!rig.p("gradle.out").exists());
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn android_install_needs_the_sdk() {
    let rig = Rig::new(false);
    let mut c = rig.jw(&["android", "install", "--debug"], ONE);
    c.env_remove("ANDROID_HOME").env_remove("ANDROID_SDK_ROOT");
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("ANDROID_HOME"));
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn android_install_gives_gradle_the_one_device_and_deletes_its_directory() {
    let rig = Rig::new(false);
    let out = rig.run(&["android", "install", "--debug"], ONE);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("device R58M123"));
    assert_eq!(
        std::fs::read_to_string(rig.p("gradle.out")).unwrap().trim(),
        "R58M123 --no-daemon :app:installDebug"
    );
    assert_eq!(rig.left(), Vec::<String>::new());
}

#[test]
fn android_install_with_two_devices_stops_unless_one_is_named() {
    let rig = Rig::new(false);
    let out = rig.run(&["android", "install", "--debug"], TWO);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("--serial") && stderr.contains("A1") && stderr.contains("B2"),
        "{stderr}"
    );
    assert!(!rig.p("gradle.out").exists());
    assert_eq!(rig.left(), Vec::<String>::new());

    let out = rig.run(&["android", "install", "--debug", "--serial", "B2"], TWO);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        std::fs::read_to_string(rig.p("gradle.out"))
            .unwrap()
            .starts_with("B2 ")
    );
}

#[test]
fn a_failed_gradle_is_an_error_that_says_the_app_is_not_uninstalled() {
    let rig = Rig::new(false);
    let out = rig.run_env(&["android", "install", "--debug"], ONE, ("FAKE_RC", "1"));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(rig.left(), Vec::<String>::new());
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_signal_ends_what_runs_deletes_the_directory_and_exits_130() {
    for signal in ["-INT", "-TERM", "-HUP"] {
        let rig = Rig::new(false);
        let mut child = rig
            .jw(&["android", "install", "--debug"], ONE)
            .env("FAKE_SLEEP", "1")
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let end = Instant::now() + Duration::from_secs(30);
        let pid = loop {
            if let Ok(pid) = std::fs::read_to_string(rig.p("gradle.pid"))
                && !pid.trim().is_empty()
            {
                break pid.trim().to_owned();
            }
            assert!(Instant::now() < end, "gradle did not start");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(rig.left().len(), 1, "the directory exists while it runs");
        Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(130), "{signal}");
        assert_eq!(rig.left(), Vec::<String>::new(), "{signal}");
        assert!(!alive(&pid), "{signal}: the program it ran is still there");
    }
}

/// A daemon that only says hello, with `version`, to everyone who connects.
fn fake_daemon(socket: &Path, version: &str) {
    let listener = UnixListener::bind(socket).unwrap();
    let hello = format!(
        "{{\"t\":\"hello\",\"daemon\":{{\"name\":\"{}\",\"version\":\"{version}\"}},\"epoch\":\"e\",\"root\":\"r\",\"accepted\":{{}}}}\n",
        identity::get().name
    );
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let _ = reader.get_mut().write_all(hello.as_bytes());
        }
    });
}

fn warnings(stderr: &str) -> usize {
    stderr.matches("this binary is").count()
}

#[test]
fn version_says_both_and_a_daemon_of_another_version_is_warned_of_once() {
    let rig = Rig::new(false);
    let out = rig.run(&["--version"], "");
    let mine = env!("CARGO_PKG_VERSION");
    let name = &identity::get().name;
    assert!(out.status.success());
    assert_eq!(
        text(&out.stdout),
        format!("{name} {mine}\ndaemon: not running\n")
    );

    fake_daemon(&rig.p("s.sock"), "0.0.1");
    let out = rig.run(&["version"], "");
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), format!("{name} {mine}\ndaemon 0.0.1\n"));
    assert_eq!(warnings(&text(&out.stderr)), 0, "version says it itself");

    // a command that reaches the daemon says it on stderr, once; its failure is its own
    for args in [&["cmd", "{\"cmd\":\"stop\"}"][..], &["ls"][..]] {
        let out = rig.run(args, "");
        let stderr = text(&out.stderr);
        assert_eq!(warnings(&stderr), 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains("the daemon is 0.0.1") && stderr.contains(mine),
            "{stderr}"
        );
        assert!(text(&out.stdout).is_empty(), "{args:?}");
    }
}

#[test]
fn a_daemon_of_this_version_is_not_warned_of() {
    let rig = Rig::new(false);
    fake_daemon(&rig.p("s.sock"), env!("CARGO_PKG_VERSION"));
    let out = rig.run(&["cmd", "{\"cmd\":\"stop\"}"], "");
    assert_eq!(warnings(&text(&out.stderr)), 0, "{}", text(&out.stderr));
}
