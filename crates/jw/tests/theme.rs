//! `jw theme` against a config dir of its own (S16, REQ-105–108).

use std::io::Write;
use std::process::{Command, Output, Stdio};

const EXE: &str = env!("CARGO_BIN_EXE_jw");

fn jw(cfg: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(EXE)
        .args(args)
        .env("XDG_CONFIG_HOME", cfg)
        .env_remove("JW_THEME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or("").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

#[test]
fn a_theme_goes_in_through_the_cli_and_gets_picked() {
    let cfg = tempfile::tempdir().unwrap();
    let ls = jw(cfg.path(), &["theme", "ls"], None);
    assert!(ls.status.success());
    assert_eq!(text(&ls).lines().count(), 10, "{}", text(&ls));

    let export = jw(cfg.path(), &["theme", "export", "tokyo-night"], None);
    assert!(export.status.success());
    let json = String::from_utf8(export.stdout).unwrap();
    let install = jw(
        cfg.path(),
        &["theme", "install", "-", "--name", "night"],
        Some(&json),
    );
    assert!(install.status.success(), "{}", text(&install));
    // REQ-106: what was exported is what was installed.
    let back = std::fs::read_to_string(cfg.path().join("jw/themes/night.json")).unwrap();
    assert_eq!(back, json);

    let pick = jw(cfg.path(), &["theme", "use", "night"], None);
    assert!(pick.status.success(), "{}", text(&pick));
    let settings = std::fs::read_to_string(cfg.path().join("jw/settings.json")).unwrap();
    assert!(settings.contains(r#""dark": "night""#), "{settings}");
    let ls = text(&jw(cfg.path(), &["theme", "ls"], None));
    assert!(ls.contains("night") && ls.contains("← dark"), "{ls}");

    // REQ-105: refused, and nothing written.
    let again = jw(
        cfg.path(),
        &["theme", "install", "-", "--name", "night"],
        Some(&json),
    );
    assert!(!again.status.success());
    let junk = jw(
        cfg.path(),
        &["theme", "install", "-", "--name", "junk"],
        Some("{}"),
    );
    assert!(!junk.status.success());
    assert!(!cfg.path().join("jw/themes/junk.json").exists());

    let help = jw(cfg.path(), &["help", "theme"], None);
    assert!(text(&help).contains("jw theme"));
}
