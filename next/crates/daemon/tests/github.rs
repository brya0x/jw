//! N37: what GitHub says about a worktree's branch becomes attention (REQ-135 to REQ-138),
//! with a stand-in for `gh` so no test goes near the network.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Sandbox, git, kinds, text, wait_kinds};
use contract::Cmd;
use wire::Client;

/// A `gh` that prints what `answer` holds and records how it was run; if `answer` is missing it fails.
fn fake_gh(dir: &Path) -> PathBuf {
    let script = dir.join("gh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> {d}/gh.args\npwd >> {d}/gh.cwd\ncat {d}/answer 2>/dev/null || {{ echo 'gh: not logged in' >&2; exit 1; }}\n",
            d = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn answer(dir: &Path, json: &str) {
    std::fs::write(dir.join("answer"), json).unwrap();
}

fn pr(state: &str, conclusion: &str) -> String {
    format!(
        r#"[{{"number":9,"state":"{state}","statusCheckRollup":[{{"__typename":"CheckRun","conclusion":"{conclusion}","status":"COMPLETED"}}]}}]"#
    )
}

#[test]
fn a_merged_or_failing_pull_request_is_attention_that_comes_back_only_when_it_changes() {
    let sb = Sandbox::new();
    let gh_dir = sb.dir.join("ghbin");
    std::fs::create_dir_all(&gh_dir).unwrap();
    let mut config = sb.config();
    config.gh = Some(fake_gh(&gh_dir));
    let d = daemon::Daemon::start(config.clone()).unwrap();

    let repo = sb.dir.join("work/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let repo = std::fs::canonicalize(repo).unwrap();

    let mut c = Client::connect(&config.socket, "test").unwrap();
    let ws = c
        .cmd(Cmd::CreateWorktree {
            repo: text(&repo),
            branch: "feat/login".into(),
            base: None,
            folder: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = c.root.clone();
    c.open(&root).unwrap();
    c.open(&ws).unwrap();

    // A worktree nobody works in is not looked up.
    answer(&gh_dir, &pr("OPEN", "FAILURE"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!gh_dir.join("gh.args").exists(), "no pane, no lookup");

    c.cmd(Cmd::NewPane {
        workspace: ws.clone(),
        program: None,
    })
    .unwrap()
    .unwrap();
    wait_kinds(&mut c, &ws, &["ci_failed"]);
    wait_kinds(&mut c, &root, &["ci_failed"]);
    let args = std::fs::read_to_string(gh_dir.join("gh.args")).unwrap();
    assert!(
        args.contains(
            "pr list --head feat/login --state all --limit 1 --json number,state,statusCheckRollup"
        ),
        "{args}"
    );
    let cwd = std::fs::read_to_string(gh_dir.join("gh.cwd")).unwrap();
    assert!(
        cwd.trim().ends_with("app-wt/feat-login"),
        "run in the worktree: {cwd}"
    );

    // The checks pass: it is over.
    answer(&gh_dir, &pr("OPEN", "SUCCESS"));
    wait_kinds(&mut c, &ws, &[]);

    // Merged.
    answer(&gh_dir, &pr("MERGED", "SUCCESS"));
    wait_kinds(&mut c, &ws, &["merged"]);

    // Looked at: it goes, and the next lookups, which see the same, do not bring it back.
    let rev = c.nodes[&ws]["attention"][0]["rev"].as_u64().unwrap();
    c.cmd(Cmd::MarkSeen {
        node: ws.clone(),
        rev,
    })
    .unwrap()
    .unwrap();
    wait_kinds(&mut c, &ws, &[]);
    let looked = std::fs::read_to_string(gh_dir.join("gh.args"))
        .unwrap()
        .lines()
        .count();
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        std::fs::read_to_string(gh_dir.join("gh.args"))
            .unwrap()
            .lines()
            .count()
            > looked,
        "it kept looking"
    );
    assert!(kinds(&c, &ws).is_empty(), "and did not raise it again");

    // gh failing keeps what was known and says nothing; no pull request ends it.
    answer(&gh_dir, &pr("OPEN", "FAILURE"));
    wait_kinds(&mut c, &ws, &["ci_failed"]);
    std::fs::remove_file(gh_dir.join("answer")).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        kinds(&c, &ws),
        ["ci_failed"],
        "a failed lookup changes nothing"
    );
    answer(&gh_dir, "[]");
    wait_kinds(&mut c, &ws, &[]);
    d.stop();
}
