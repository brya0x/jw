//! N78: a repository's `.jw.toml` (REQ-329 to REQ-346), against a real daemon and real git.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, ErrorCode};
use serde_json::Value;
use wire::Client;

/// A repository with one commit, at `<sandbox>/work/app`, and `jw` as its `.jw.toml` when given.
fn repo(sb: &Sandbox, jw: Option<&str>) -> PathBuf {
    let dir = sb.dir.join("work/app");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    if let Some(jw) = jw {
        std::fs::write(dir.join(".jw.toml"), jw).unwrap();
    }
    std::fs::canonicalize(dir).unwrap()
}

fn ok(c: &mut Client, cmd: Cmd) -> Value {
    c.cmd(cmd).unwrap().unwrap().unwrap()
}

fn refused(c: &mut Client, cmd: Cmd) -> (ErrorCode, String) {
    let e = c.cmd(cmd).unwrap().unwrap_err();
    (e.code, e.message)
}

fn worktree(c: &mut Client, repo: &Path, name: &str) -> Result<Value, (ErrorCode, String)> {
    c.cmd(Cmd::CreateWorktree {
        repo: text(repo),
        branch: format!("feat/{name}"),
        base: None,
        folder: Some(name.to_owned()),
    })
    .unwrap()
    .map(|r| r.unwrap())
    .map_err(|e| (e.code, e.message))
}

fn open(c: &mut Client, node: &str) {
    c.open(node).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(node)).unwrap());
}

fn leaf_ids(layout: &Value) -> Vec<String> {
    match layout["t"].as_str() {
        Some("pane") => vec![layout["pane"].as_str().unwrap().to_owned()],
        _ => {
            let mut v = leaf_ids(&layout["a"]);
            v.extend(leaf_ids(&layout["b"]));
            v
        }
    }
}

fn screen(c: &Client, pane: &str) -> String {
    c.nodes.get(pane).map_or_else(String::new, |n| {
        n["screen"]["lines"]
            .as_array()
            .map_or_else(String::new, |l| {
                l.iter()
                    .map(|l| l.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    })
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("timed out waiting for {what}");
}

const FULL: &str = r#"
root = "../app-trees"
branch = "bryan/{name}"
setup = ["echo installing {name}", "echo done-{branch}"]
ports = { web = 1 }

[agent]
default = "claude"

[layout]
split = "right"
ratio = 0.6
a = { run = "cat" }
b = { split = "down", a = { run = "shell" }, b = { run = "editor" } }
editor = "cat -n"
"#;

#[test]
fn project_config_tells_the_defaults_and_what_the_file_says() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, None);
    let mut c = sb.client();

    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert!(r.get("source").is_none());
    assert_eq!(r["root"], text(&top.parent().unwrap().join("app-wt")));
    assert_eq!(r["branch"], "feat/{name}");
    assert_eq!(r["setup"], serde_json::json!([]));
    assert_eq!(r["panes"], serde_json::json!(["sh"]));

    std::fs::write(top.join(".jw.toml"), FULL).unwrap();
    // Any path inside the repository will do, and the file is read afresh.
    let inside = top.join("sub");
    std::fs::create_dir_all(&inside).unwrap();
    let r = ok(
        &mut c,
        Cmd::ProjectConfig {
            repo: text(&inside),
        },
    );
    assert_eq!(r["source"], text(&top.join(".jw.toml")));
    assert_eq!(r["root"], text(&top.parent().unwrap().join("app-trees")));
    assert_eq!(r["branch"], "bryan/{name}");
    assert_eq!(r["setup"].as_array().unwrap().len(), 2);
    assert_eq!(r["panes"], serde_json::json!(["cat", "sh", "cat"]));
    d.stop();
}

#[test]
fn a_file_that_cannot_be_used_is_refused_by_the_three_commands_and_nothing_is_made() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("branch = \"x\"\nsetup = [\"cd {nme}\"]\n"));
    let mut c = sb.client();

    let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(
        why.starts_with(".jw.toml, line 2: «{nme}» is no marker"),
        "{why}"
    );

    let (code, why) = worktree(&mut c, &top, "one").unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains(".jw.toml"), "{why}");
    assert!(!top.parent().unwrap().join("app-wt/one").exists());
    assert!(!git_has_worktree(&top, "one"));

    // A workspace that was made while the file was good, and whose file went bad.
    std::fs::write(top.join(".jw.toml"), "").unwrap();
    let ws = worktree(&mut c, &top, "two").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(top.join(".jw.toml"), "setup = [").unwrap();
    let (code, why) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains(".jw.toml"), "{why}");
    open(&mut c, &ws);
    assert!(c.nodes[&ws]["panes"].as_array().unwrap().is_empty());
    d.stop();
}

fn git_has_worktree(top: &Path, name: &str) -> bool {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(name)
}

#[test]
fn a_worktree_goes_in_the_root_of_the_file_and_without_one_in_the_default() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some("root = \"../app-trees\"\n"));
    let mut c = sb.client();

    let r = worktree(&mut c, &top, "one").unwrap();
    assert_eq!(
        r["path"],
        text(&top.parent().unwrap().join("app-trees/one"))
    );
    assert!(top.parent().unwrap().join("app-trees/one/f.txt").exists());

    std::fs::remove_file(top.join(".jw.toml")).unwrap();
    let r = worktree(&mut c, &top, "two").unwrap();
    assert_eq!(r["path"], text(&top.parent().unwrap().join("app-wt/two")));

    std::fs::write(
        top.join(".jw.toml"),
        "root = \"~/jwn-trees-that-do-not-exist\"\n",
    )
    .unwrap();
    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert!(
        r["root"]
            .as_str()
            .unwrap()
            .ends_with("/jwn-trees-that-do-not-exist")
    );
    assert!(!r["root"].as_str().unwrap().contains('~'));
    d.stop();
}

#[test]
fn start_workspace_starts_the_layout_in_order_with_its_shares_and_the_setup_under_it() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, Some(FULL));
    let mut c = sb.client();
    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();

    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    let ids: Vec<String> = r["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 4, "cat, a shell, the editor and the setup");

    open(&mut c, &ws);
    wait_for("the layout", || {
        c.wait_until(Duration::from_millis(50), |c| {
            leaf_ids(&c.nodes[&ws]["layout"]).len() == 4
        })
        .unwrap_or(false)
    });
    let l = c.nodes[&ws]["layout"].clone();
    // Under everything, the setup keeps about a third.
    assert_eq!(
        (l["t"].as_str(), l["dir"].as_str(), l["ratio"].as_u64()),
        (Some("split"), Some("col"), Some(680))
    );
    assert_eq!(l["b"]["pane"], ids[3].as_str());
    // The layout above it: a on the left with 60%, b down.
    let above = &l["a"];
    assert_eq!(
        (above["dir"].as_str(), above["ratio"].as_u64()),
        (Some("row"), Some(600))
    );
    assert_eq!(above["a"]["pane"], ids[0].as_str());
    assert_eq!(
        (above["b"]["dir"].as_str(), above["b"]["ratio"].as_u64()),
        (Some("col"), Some(500))
    );
    assert_eq!(leaf_ids(&l), ids);

    let title = |c: &Client, id: &str| {
        c.nodes[&ws]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p["title"].as_str().unwrap().to_owned())
    };
    assert_eq!(title(&c, &ids[3]).as_deref(), Some("setup"), "named");
    assert!(
        ids[..3]
            .iter()
            .all(|i| title(&c, i).as_deref() != Some("setup"))
    );

    // The programs: the first leaf runs `cat`, the third is the editor, `cat -n`.
    for (i, argv) in [
        (0, serde_json::json!(["cat"])),
        (2, serde_json::json!(["cat", "-n"])),
    ] {
        let pane = &ids[i];
        open(&mut c, pane);
        assert!(
            c.wait_until(WAIT, |c| c.nodes[pane]["running"]["argv"] == argv)
                .unwrap(),
            "pane {i}"
        );
    }

    // The setup: each line after `❯ ` and then run, with the markers quoted.
    let setup = &ids[3];
    open(&mut c, setup);
    wait_for("the setup's output", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, setup).contains("done-feat/ws-1")
    });
    let s = screen(&c, setup);
    assert!(s.contains("❯ echo installing {name}"), "{s}");
    assert!(s.contains("installing ws-1"), "{s}");
    assert!(s.contains("❯ echo done-{branch}"), "{s}");
    d.stop();
}

#[test]
fn the_setup_stops_at_the_first_line_that_fails_and_the_pane_stays_a_shell() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(
        &sb,
        Some("setup = [\"echo first\", \"false\", \"echo never\"]\n"),
    );
    let mut c = sb.client();
    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();

    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    let ids = r["panes"].as_array().unwrap();
    assert_eq!(ids.len(), 2, "the shell and the setup");
    let setup = ids[1].as_str().unwrap().to_owned();
    open(&mut c, &setup);
    wait_for("the failing line", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, &setup).contains("❯ false")
    });
    // Its end is reported as a shell would, with the exit status of the line that failed.
    let root = c.root.clone();
    open(&mut c, &root);
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&root]["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "shell_done"
                && a["node"] == setup.as_str()
                && a["exit_code"] == 1))
            .unwrap(),
        "{}",
        c.nodes[&root]["attention"]
    );
    let s = screen(&c, &setup);
    assert!(s.contains("❯ echo first") && s.contains("first"), "{s}");
    assert!(!s.contains("never"), "{s}");

    // Still a shell: it takes input.
    c.cmd(Cmd::Write {
        pane: setup.clone(),
        data: "echo still-here\r".into(),
    })
    .unwrap()
    .unwrap();
    wait_for("the shell", || {
        c.wait_until(Duration::from_millis(50), |_| false).ok();
        screen(&c, &setup).contains("still-here")
    });
    d.stop();
}

#[test]
fn design_studios_file_names_one_agent_pane() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(
        &sb,
        Some(
            "setup = [\"true\"]\n[agent]\ndefault = \"claude\"\n[layout]\neditor = \"nvim\"\nrun = \"agent\"\n",
        ),
    );
    let mut c = sb.client();
    let r = ok(&mut c, Cmd::ProjectConfig { repo: text(&top) });
    assert_eq!(r["panes"], serde_json::json!(["claude"]));
    d.stop();
}

#[test]
fn without_a_file_start_workspace_opens_one_shell_and_a_workspace_with_panes_conflicts() {
    let sb = Sandbox::new();
    let d = sb.start();
    let top = repo(&sb, None);
    let mut c = sb.client();

    let ws = worktree(&mut c, &top, "ws-1").unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(r["panes"].as_array().unwrap().len(), 1);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(code, ErrorCode::Conflict);
    let (code, _) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: "nope".into(),
        },
    );
    assert_eq!(code, ErrorCode::NotFound);
    open(&mut c, &ws);
    assert_eq!(c.nodes[&ws]["panes"].as_array().unwrap().len(), 1);

    // The top level of the repository, opened as a folder, has the layout's three panes and not
    // the setup (REQ-500): FULL's setup lines are for worktrees.
    std::fs::write(top.join(".jw.toml"), FULL).unwrap();
    let folder = ok(&mut c, Cmd::OpenFolder { path: text(&top) })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ok(&mut c, Cmd::StartWorkspace { workspace: folder });
    assert_eq!(r["panes"].as_array().unwrap().len(), 3);
    d.stop();
}

fn config(c: &mut Client, repo: &Path) -> Value {
    ok(c, Cmd::ProjectConfig { repo: text(repo) })
}

fn write(
    c: &mut Client,
    repo: &Path,
    to: contract::ConfigFrom,
    body: &str,
) -> Result<Value, (ErrorCode, String)> {
    c.cmd(Cmd::WriteProjectConfig {
        repo: text(repo),
        to,
        text: body.to_owned(),
    })
    .unwrap()
    .map(|r| r.unwrap())
    .map_err(|e| (e.code, e.message))
}

/// N91: where the settings are (REQ-482 to REQ-487).
#[test]
fn the_settings_come_from_the_repository_then_from_the_personal_file_then_from_the_defaults() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();

    // neither file: the defaults, and where the personal one would be
    let none = config(&mut c, &app);
    assert_eq!(none["branch"], "feat/{name}");
    assert!(none.get("source").is_none() && none.get("from").is_none());
    let personal = PathBuf::from(none["personal"].as_str().unwrap());
    assert!(
        personal.starts_with(sb.dir.join("projects")),
        "{personal:?}"
    );
    assert!(
        personal
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("local_"),
        "a repository with no origin is named by its path: {personal:?}"
    );
    assert!(!personal.exists(), "asking writes nothing");

    // the personal file, read as a `.jw.toml` is
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, "branch = \"mio/{name}\"\nsetup = [\"true\"]\n").unwrap();
    let mine = config(&mut c, &app);
    assert_eq!(mine["branch"], "mio/{name}");
    assert_eq!(
        (mine["from"].as_str(), mine["source"].as_str()),
        (Some("personal"), personal.to_str())
    );
    assert_eq!(mine["setup"], serde_json::json!(["true"]));

    // the repository's own file wins, and the personal one is not read
    std::fs::write(app.join(".jw.toml"), "branch = \"equipo/{name}\"\n").unwrap();
    let theirs = config(&mut c, &app);
    assert_eq!(theirs["branch"], "equipo/{name}");
    assert_eq!(theirs["from"], "repo");
    assert_eq!(theirs["setup"], serde_json::json!([]));
    assert_eq!(
        theirs["personal"], mine["personal"],
        "it still says where the personal one is"
    );

    // a worktree of the repository has the same personal file
    std::fs::remove_file(app.join(".jw.toml")).unwrap();
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let wt = PathBuf::from(made["path"].as_str().unwrap());
    let from_worktree = config(&mut c, &wt);
    assert_eq!(from_worktree["personal"], mine["personal"], "REQ-483");
    assert_eq!(from_worktree["from"], "personal");

    // an error in the personal file names it and its line
    std::fs::write(&personal, "branch = \"a\"\nsetup = nope\n").unwrap();
    let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&app) });
    assert_eq!(code, ErrorCode::BadRequest);
    let name = personal.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        why.starts_with(&format!("{name}, line 2")),
        "REQ-484: {why}"
    );
    d.stop();
}

#[test]
fn a_worktree_starts_with_the_personal_settings_and_a_project_that_needs_nothing_needs_no_file() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();
    // no file anywhere: one shell and no setup pane, and nothing is written
    let plain = worktree(&mut c, &app, "ws-plain").unwrap();
    let ws = plain["workspace"].as_str().unwrap().to_owned();
    let started = ok(&mut c, Cmd::StartWorkspace { workspace: ws });
    assert_eq!(started["panes"].as_array().unwrap().len(), 1);
    assert!(!app.join(".jw.toml").exists());
    assert!(
        !sb.dir.join("projects").exists(),
        "REQ-488: nothing is asked or made"
    );
    // a personal file with a setup gives the next worktree its setup pane
    let personal = PathBuf::from(config(&mut c, &app)["personal"].as_str().unwrap());
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, "setup = [\"true\"]\n").unwrap();
    let with = worktree(&mut c, &app, "ws-setup").unwrap();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: with["workspace"].as_str().unwrap().to_owned(),
        },
    );
    assert_eq!(
        started["panes"].as_array().unwrap().len(),
        2,
        "a shell and the setup pane"
    );
    d.stop();
}

#[test]
fn writing_the_settings_checks_them_first_keeps_the_personal_file_private_and_never_touches_the_repository_unasked()
 {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let mut c = sb.client();
    let personal = PathBuf::from(config(&mut c, &app)["personal"].as_str().unwrap());

    // REQ-486: it is written whole, to the personal file, readable by its owner only
    let wrote = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "branch = \"x/{name}\"\n",
    )
    .unwrap();
    assert_eq!(wrote["path"], text(&personal));
    assert_eq!(wrote["shadowed"], false);
    assert_eq!(
        std::fs::read_to_string(&personal).unwrap(),
        "branch = \"x/{name}\"\n"
    );
    assert_eq!(
        std::fs::metadata(&personal).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !app.join(".jw.toml").exists(),
        "REQ-487: the repository is not touched"
    );
    assert_eq!(
        config(&mut c, &app)["branch"],
        "x/{name}",
        "and the next read has it"
    );
    // no temporary file is left behind
    let left: Vec<_> = std::fs::read_dir(personal.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");

    // a text that does not read is refused, says where, and changes nothing
    let (code, why) = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "branch = \"{nombre}\"\n",
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains("line 1"), "{why}");
    assert_eq!(
        std::fs::read_to_string(&personal).unwrap(),
        "branch = \"x/{name}\"\n"
    );

    // only when asked does it write into the repository, and then the personal one is hidden by it
    let repo_file = write(
        &mut c,
        &app,
        contract::ConfigFrom::Repo,
        "branch = \"equipo/{name}\"\n",
    )
    .unwrap();
    assert_eq!(repo_file["path"], text(&app.join(".jw.toml")));
    assert_eq!(
        std::fs::read_to_string(app.join(".jw.toml")).unwrap(),
        "branch = \"equipo/{name}\"\n"
    );
    let shadowed = write(
        &mut c,
        &app,
        contract::ConfigFrom::Personal,
        "branch = \"y/{name}\"\n",
    )
    .unwrap();
    assert_eq!(shadowed["shadowed"], true, "REQ-487");
    assert_eq!(config(&mut c, &app)["branch"], "equipo/{name}");
    // a folder that is not a repository has nothing to write for
    let plain = sb.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(
        write(&mut c, &plain, contract::ConfigFrom::Personal, "")
            .unwrap_err()
            .0,
        ErrorCode::GitFailed
    );
    d.stop();
}

#[test]
fn writing_into_the_repository_never_goes_through_a_link_planted_where_the_temporary_file_goes() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, None);
    let victim = sb.dir.join("victim.txt");
    std::fs::write(&victim, "intacto").unwrap();
    std::os::unix::fs::symlink(&victim, app.join(".jw.toml.tmp")).unwrap();
    let mut c = sb.client();
    write(
        &mut c,
        &app,
        contract::ConfigFrom::Repo,
        "branch = \"a/{name}\"\n",
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "intacto");
    assert_eq!(
        std::fs::read_to_string(app.join(".jw.toml")).unwrap(),
        "branch = \"a/{name}\"\n"
    );
    assert!(!app.join(".jw.toml.tmp").exists());
    // a text that is too big is refused whole
    let big = "# ".repeat(40 * 1024);
    let (code, why) = write(&mut c, &app, contract::ConfigFrom::Personal, &big).unwrap_err();
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.contains("KiB"), "{why}");
    // the personal folder is made for its owner only
    use std::os::unix::fs::PermissionsExt;
    write(&mut c, &app, contract::ConfigFrom::Personal, "").unwrap();
    let mode = std::fs::metadata(sb.dir.join("projects"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
    d.stop();
}

/// N92: `copy` (REQ-489 to REQ-492).
#[test]
fn copy_puts_the_files_of_the_checkout_in_a_new_worktree_with_their_owners_bits_and_says_what_it_skipped()
 {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(
        &sb,
        Some("copy = [\".env\", \"apps/.env.local\", \"nada\", \"f.txt\"]\n"),
    );
    std::fs::write(app.join(".env"), "SECRETO=1\n").unwrap();
    std::fs::set_permissions(app.join(".env"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::create_dir_all(app.join("apps")).unwrap();
    std::fs::write(app.join("apps/.env.local"), "LOCAL=2\n").unwrap();
    let mut c = sb.client();

    assert_eq!(
        config(&mut c, &app)["copy"],
        serde_json::json!([".env", "apps/.env.local", "nada", "f.txt"]),
        "REQ-489: the settings list them"
    );
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let wt = PathBuf::from(made["path"].as_str().unwrap());
    assert_eq!(
        made["copied"],
        serde_json::json!([".env", "apps/.env.local"])
    );
    assert_eq!(
        made["skipped"],
        serde_json::json!([
            { "path": "nada", "why": "it is not in the checkout" },
            { "path": "f.txt", "why": "it is already in the worktree" },
        ])
    );
    assert_eq!(
        std::fs::read_to_string(wt.join(".env")).unwrap(),
        "SECRETO=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("apps/.env.local")).unwrap(),
        "LOCAL=2\n"
    );
    let mode = std::fs::metadata(wt.join(".env"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "REQ-490");
    assert_eq!(
        std::fs::read_to_string(app.join(".env")).unwrap(),
        "SECRETO=1\n",
        "the checkout is as it was"
    );
    // a worktree of a repository with nothing to copy says nothing of it
    std::fs::write(app.join(".jw.toml"), "").unwrap();
    let plain = worktree(&mut c, &app, "ws-2").unwrap();
    assert!(
        plain.get("copied").is_none() && plain.get("skipped").is_none(),
        "{plain}"
    );
    d.stop();
}

#[test]
fn a_copy_that_leaves_the_repository_is_refused_naming_the_path_and_the_line() {
    let sb = Sandbox::new();
    let d = sb.start();
    for bad in ["../fuera", "/etc/passwd", ""] {
        let app = repo(
            &sb,
            Some(&format!("branch = \"a/{{name}}\"\ncopy = [{bad:?}]\n")),
        );
        let mut c = sb.client();
        let (code, why) = refused(&mut c, Cmd::ProjectConfig { repo: text(&app) });
        assert_eq!(code, ErrorCode::BadRequest);
        assert!(why.starts_with(".jw.toml, line 2"), "{bad:?}: {why}");
        assert!(why.contains("not a path inside the repository"), "{why}");
        std::fs::remove_dir_all(&app).unwrap();
    }
    d.stop();
}

/// N93: ports per worktree (REQ-494 to REQ-498).
#[test]
fn a_worktree_gets_a_block_of_ports_its_panes_see_them_and_a_removed_one_gives_its_block_back() {
    let sb = Sandbox::new();
    let d = sb.start();
    let base = identity::env_var("PORT_BASE");
    let web = identity::env_var("PORT_WEB");
    let app = repo(
        &sb,
        Some(&format!(
            "ports = {{ web = 0, api = 5 }}\nsetup = [\"echo seen-${base}-${web}-end\"]\n"
        )),
    );
    let mut c = sb.client();
    assert_eq!(
        config(&mut c, &app)["ports"],
        serde_json::json!({ "api": 5, "web": 0 }),
        "REQ-494"
    );

    let one = worktree(&mut c, &app, "ws-1").unwrap();
    assert_eq!(
        one["ports"],
        serde_json::json!({ "api": 20005, "web": 20000 }),
        "REQ-497"
    );
    let two = worktree(&mut c, &app, "ws-2").unwrap();
    assert_eq!(
        two["ports"],
        serde_json::json!({ "api": 20105, "web": 20100 }),
        "the next slot"
    );

    // the panes of the worktree start with its ports, the setup pane among them (REQ-496)
    let ws = two["workspace"].as_str().unwrap().to_owned();
    let started = ok(&mut c, Cmd::StartWorkspace { workspace: ws });
    let setup = started["panes"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    open(&mut c, &setup);
    wait_for("the setup to say its ports", || {
        let _ = c.poll(Duration::from_millis(20));
        screen(&c, &setup).contains("seen-20100-20100-end")
    });

    // a change of the settings is for the next worktree, not for this one (REQ-498)
    std::fs::write(app.join(".jw.toml"), "ports = { web = 9 }\n").unwrap();
    let three = worktree(&mut c, &app, "ws-3").unwrap();
    assert_eq!(three["ports"], serde_json::json!({ "web": 20209 }));

    // a removed worktree gives its block back, and the lowest free one is the next to be given (REQ-495)
    c.cmd(Cmd::RemoveWorkspace {
        workspace: one["workspace"].as_str().unwrap().to_owned(),
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();
    let four = worktree(&mut c, &app, "ws-4").unwrap();
    assert_eq!(
        four["ports"],
        serde_json::json!({ "web": 20009 }),
        "the first block again"
    );
    d.stop();
}

#[test]
fn a_project_without_ports_gives_none_and_the_blocks_survive_a_restart_of_the_daemon() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, Some("ports = { web = 0 }\n"));
    let mut c = sb.client();
    let plain = repo_two(&sb);
    let none = worktree(&mut c, &plain, "ws-n").unwrap();
    assert!(none.get("ports").is_none(), "{none}");
    let one = worktree(&mut c, &app, "ws-1").unwrap();
    assert_eq!(one["ports"], serde_json::json!({ "web": 20000 }));
    d.stop();
    let d = sb.start();
    let mut c = sb.client();
    let two = worktree(&mut c, &app, "ws-2").unwrap();
    assert_eq!(
        two["ports"],
        serde_json::json!({ "web": 20100 }),
        "the first block is still held"
    );
    d.stop();
}

/// A second repository, at `<sandbox>/work/other`, with no settings.
fn repo_two(sb: &Sandbox) -> PathBuf {
    let dir = sb.dir.join("work/other");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::canonicalize(dir).unwrap()
}

/// The personal file of a folder that is not a repository: named by its path (REQ-483).
fn folder_personal(sb: &Sandbox, folder: &Path) -> PathBuf {
    daemon::personal_file(&sb.dir.join("projects"), None, folder)
}

fn open_folder(c: &mut Client, folder: &Path) -> String {
    ok(c, Cmd::OpenFolder { path: text(folder) })["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

const TWO: &str = "setup = [\"true\"]\nports = { web = 0 }\n[layout]\nsplit = \"right\"\na = { run = \"cat\" }\nb = { run = \"shell\" }\n";

/// N94: a folder's layout (REQ-500 to REQ-502).
#[test]
fn a_folder_opened_gets_the_layout_of_its_personal_file_and_never_its_setup_or_ports() {
    let sb = Sandbox::new();
    let d = sb.start();
    let folder = sb.dir.join("work/plain");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = std::fs::canonicalize(folder).unwrap();
    let mut c = sb.client();

    // no settings: one shell, as always
    let ws = open_folder(&mut c, &folder);
    assert_eq!(
        ok(
            &mut c,
            Cmd::StartWorkspace {
                workspace: ws.clone()
            }
        )["panes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    c.cmd(Cmd::RemoveWorkspace {
        workspace: ws,
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();

    // the personal file named by its path: the layout's two panes and no setup pane
    let personal = folder_personal(&sb, &folder);
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, TWO).unwrap();
    let ws = open_folder(&mut c, &folder);
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(
        started["panes"].as_array().unwrap().len(),
        2,
        "REQ-500: the layout and no setup pane"
    );
    open(&mut c, &ws);
    assert_eq!(leaf_ids(&c.nodes[&ws]["layout"]).len(), 2);
    let pane = started["panes"][0].as_str().unwrap();
    open(&mut c, pane);
    assert!(
        c.nodes[pane]["running"].is_null() || c.nodes[pane]["running"]["argv"][0] == "cat",
        "the first leaf runs `cat`"
    );
    // a folder has no ports: nothing was given to its panes
    assert!(sb.dir.join("projects").exists());
    c.cmd(Cmd::RemoveWorkspace {
        workspace: ws,
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();

    // its own `.jw.toml` is read before the personal file
    std::fs::write(folder.join(".jw.toml"), "[layout]\nrun = \"shell\"\n").unwrap();
    let ws = open_folder(&mut c, &folder);
    assert_eq!(
        ok(
            &mut c,
            Cmd::StartWorkspace {
                workspace: ws.clone()
            }
        )["panes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    c.cmd(Cmd::RemoveWorkspace {
        workspace: ws,
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();

    // settings that cannot be used: refused naming the file and the line, and nothing started
    std::fs::write(folder.join(".jw.toml"), "[layout]\nrun = \n").unwrap();
    let ws = open_folder(&mut c, &folder);
    let (code, why) = refused(
        &mut c,
        Cmd::StartWorkspace {
            workspace: ws.clone(),
        },
    );
    assert_eq!(code, ErrorCode::BadRequest);
    assert!(why.starts_with(".jw.toml, line"), "REQ-502: {why}");
    open(&mut c, &ws);
    assert_eq!(c.nodes[&ws]["panes"].as_array().map_or(0, Vec::len), 0);
    d.stop();
}

#[test]
fn the_top_of_a_repository_has_its_settings_and_a_folder_inside_it_does_not() {
    let sb = Sandbox::new();
    let d = sb.start();
    let app = repo(&sb, Some(TWO));
    std::fs::create_dir_all(app.join("apps/web")).unwrap();
    let mut c = sb.client();
    // the top level: the repository's layout, but not its setup (REQ-501)
    let ws = open_folder(&mut c, &app);
    let started = ok(&mut c, Cmd::StartWorkspace { workspace: ws });
    assert_eq!(
        started["panes"].as_array().unwrap().len(),
        2,
        "two panes of the layout, no setup pane"
    );
    // a folder inside it is not the repository: one shell, whatever the repository says
    let inner = open_folder(&mut c, &app.join("apps/web"));
    let started = ok(&mut c, Cmd::StartWorkspace { workspace: inner });
    assert_eq!(started["panes"].as_array().unwrap().len(), 1);
    // while a worktree of the repository has the setup too
    let made = worktree(&mut c, &app, "ws-1").unwrap();
    let started = ok(
        &mut c,
        Cmd::StartWorkspace {
            workspace: made["workspace"].as_str().unwrap().to_owned(),
        },
    );
    assert_eq!(
        started["panes"].as_array().unwrap().len(),
        3,
        "the layout's two and the setup"
    );
    d.stop();
}
