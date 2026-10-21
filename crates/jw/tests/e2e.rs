//! The whole thing through the real `jw` binary, JSON only: start a daemon in a private
//! socket dir, make a session's worth of state with `jwn cmd`, stop it with `jwn stop`
//! (only its own pid), start it again and find the state back.

mod common;

use std::time::Duration;

use common::{Env, git};
use serde_json::Value;
use wire::Client;

#[test]
fn a_session_is_built_with_json_and_comes_back_after_a_restart() {
    let env = Env::new("e2e");
    let work = std::fs::canonicalize(&env.dir).unwrap().join("work");
    let repo = work.join("app");
    let notes = work.join("notes");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&notes).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    let daemon = env.start();
    let ok = |v: &Value| assert_eq!(v["ok"], true, "{v}");
    let r = env.cmd(r#"{"cmd":"rename_session","name":"e2e"}"#);
    ok(&r);
    let folder = env.cmd(&format!(
        r#"{{"cmd":"open_folder","path":"{}"}}"#,
        notes.display()
    ));
    ok(&folder);
    let wt = env.cmd(&format!(
        r#"{{"cmd":"create_worktree","repo":"{}","branch":"feat/x"}}"#,
        repo.display()
    ));
    ok(&wt);
    let ws = wt["result"]["workspace"].as_str().unwrap().to_owned();
    let pane = env.cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#));
    ok(&pane);
    let pane = pane["result"]["pane"].as_str().unwrap().to_owned();
    ok(&env.cmd(&format!(
        r#"{{"cmd":"write","pane":"{pane}","data":"echo e2e-marker\r"}}"#
    )));

    let sock = env.dir.join("s.sock");
    let mut c = Client::connect(&sock, "e2e").unwrap();
    let root = c.root.clone();
    c.open(&root).unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| {
            c.nodes.get(&pane).is_some_and(|p| {
                p["screen"]["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|l| l.as_str().unwrap().contains("e2e-marker"))
            }) && c.nodes.contains_key(&root)
        })
        .unwrap()
    );
    assert_eq!(c.nodes[&root]["name"], "e2e");
    assert_eq!(c.nodes[&root]["workspaces"].as_array().unwrap().len(), 2);
    drop(c);

    env.stop(daemon);

    let daemon = env.start();
    let mut c = Client::connect(&sock, "e2e").unwrap();
    assert_eq!(c.root, root, "same session");
    c.open(&root).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&root))
            .unwrap()
    );
    assert_eq!(c.nodes[&root]["name"], "e2e");
    // A repository's workspaces come before the folders that are not repositories (REQ-219).
    assert_eq!(c.nodes[&root]["workspaces"][0]["branch"], "feat/x");
    assert_eq!(c.nodes[&root]["workspaces"][0]["pane_count"], 1);
    assert_eq!(c.nodes[&root]["workspaces"][1]["name"], "notes");
    c.open(&ws).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&ws))
            .unwrap()
    );
    assert_eq!(c.nodes[&ws]["panes"][0]["id"], pane.as_str());
    drop(c);
    env.stop(daemon);
}
