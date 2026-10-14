//! N4: panes are real shells; their screens are nodes; shell marks raise attention.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT};
use contract::{Cmd, ErrorCode};
use wire::Client;

struct World {
    sb: Sandbox,
    ws: String,
    dir: PathBuf,
}

fn world() -> (World, daemon::Daemon, Client) {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = sb.client();
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    (World { sb, ws, dir }, d, c)
}

fn new_pane(c: &mut Client, ws: &str) -> String {
    let r = c
        .cmd(Cmd::NewPane {
            workspace: ws.into(),
            program: None,
        })
        .unwrap()
        .unwrap()
        .unwrap();
    r["pane"].as_str().unwrap().to_owned()
}

fn screen_has(c: &Client, pane: &str, text: &str) -> bool {
    c.nodes.get(pane).is_some_and(|n| {
        n["screen"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains(text))
    })
}

fn type_line(c: &mut Client, pane: &str, line: &str) {
    c.cmd(Cmd::Write {
        pane: pane.into(),
        data: format!("{line}\r"),
    })
    .unwrap()
    .unwrap();
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .output()
        .unwrap()
        .status
        .success()
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_pane_is_a_shell_in_the_workspace_folder() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.open(&pane).unwrap();
    c.open(&w.ws).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)
            && c.nodes.contains_key(&w.ws))
            .unwrap()
    );
    let n = &c.nodes[&pane];
    assert_eq!(
        (
            n["type"].as_str(),
            n["shell"].as_str(),
            n["workspace"].as_str()
        ),
        (Some("pane"), Some("/bin/sh"), Some(w.ws.as_str()))
    );
    assert_eq!(n["screen"]["rows"], 24);
    assert_eq!(n["screen"]["lines"].as_array().unwrap().len(), 24);
    assert_eq!(c.nodes[&w.ws]["panes"][0]["id"], pane.as_str());

    type_line(&mut c, &pane, "pwd");
    let dir = w.dir.to_string_lossy().into_owned();
    assert!(
        c.wait_until(WAIT, |c| screen_has(c, &pane, &dir)).unwrap(),
        "pwd printed {dir}"
    );
    d.stop();
}

#[test]
fn screen_patches_leave_a_watcher_identical_to_a_fresh_snapshot() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    let mut watcher = w.sb.client();
    watcher.open(&pane).unwrap();
    assert!(
        watcher
            .wait_until(WAIT, |c| c.nodes.contains_key(&pane))
            .unwrap()
    );
    for i in 0..5 {
        type_line(&mut c, &pane, &format!("echo line-{i}"));
    }
    assert!(
        watcher
            .wait_until(WAIT, |c| screen_has(c, &pane, "line-4"))
            .unwrap()
    );
    // Let output settle, then compare with a clean read.
    std::thread::sleep(Duration::from_millis(200));
    watcher
        .wait_until(Duration::from_millis(300), |_| false)
        .unwrap();
    let mut fresh = w.sb.client();
    fresh.open(&pane).unwrap();
    assert!(
        fresh
            .wait_until(WAIT, |c| c.nodes.contains_key(&pane))
            .unwrap()
    );
    assert_eq!(watcher.nodes[&pane], fresh.nodes[&pane]);
    d.stop();
}

#[test]
fn resize_changes_the_screen_and_is_bounded() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    c.cmd(Cmd::Resize {
        pane: pane.clone(),
        rows: 10,
        cols: 40,
    })
    .unwrap()
    .unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&pane]["screen"]["rows"] == 10)
            .unwrap()
    );
    assert_eq!(c.nodes[&pane]["screen"]["cols"], 40);
    assert_eq!(
        c.nodes[&pane]["screen"]["lines"].as_array().unwrap().len(),
        10
    );
    for (rows, cols) in [(0, 10), (10, 0), (501, 10)] {
        let e = c
            .cmd(Cmd::Resize {
                pane: pane.clone(),
                rows,
                cols,
            })
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
    }
    d.stop();
    drop(w);
}

#[test]
fn a_finished_command_raises_shell_done_that_bubbles_and_clears() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    let root = c.root.clone();
    c.open(&root).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&root)).unwrap());

    // The shell prints the marks a shell integration would: command done, exit code 3.
    type_line(&mut c, &pane, r"printf '\033]133;D;3\007'");
    assert!(
        c.wait_until(WAIT, |c| !c.nodes[&root]["attention"]
            .as_array()
            .unwrap()
            .is_empty())
            .unwrap()
    );
    let a = c.nodes[&root]["attention"][0].clone();
    assert_eq!(
        (
            a["kind"].as_str(),
            a["node"].as_str(),
            a["exit_code"].as_i64()
        ),
        (Some("shell_done"), Some(pane.as_str()), Some(3))
    );
    assert!(a["rev"].as_u64().unwrap() >= 1);
    let summary = &c.nodes[&root]["workspaces"][0]["attention"];
    assert_eq!(
        summary[0]["kind"], "shell_done",
        "the workspace summary carries it"
    );

    // The next command starting ends it.
    type_line(&mut c, &pane, r"printf '\033]133;C\007'");
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&root]["attention"]
            .as_array()
            .unwrap()
            .is_empty())
            .unwrap()
    );
    d.stop();
    drop(w);
}

#[test]
fn closing_a_pane_ends_its_shell_and_its_node() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    let pidfile = w.dir.join("pid");
    type_line(&mut c, &pane, &format!("echo $$ > {}", pidfile.display()));
    wait_for("the pid file", || {
        std::fs::read_to_string(&pidfile).is_ok_and(|s| !s.trim().is_empty())
    });
    let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_owned();
    assert!(alive(&pid));

    c.cmd(Cmd::ClosePane { pane: pane.clone() })
        .unwrap()
        .unwrap();
    assert!(!c.nodes.contains_key(&pane), "gone arrived");
    wait_for("the shell to end", || !alive(&pid));
    let e = c
        .cmd(Cmd::ClosePane { pane: pane.clone() })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = c
        .cmd(Cmd::Write {
            pane,
            data: "x".into(),
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn closing_the_last_pane_leaves_the_workspace() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.cmd(Cmd::ClosePane { pane }).unwrap().unwrap();
    c.open(&w.ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&w.ws)).unwrap());
    assert_eq!(
        c.nodes[&w.ws]["panes"].as_array().map(Vec::len),
        Some(0),
        "the workspace stays, with no panes (REQ-141)"
    );
    d.stop();
}

#[test]
fn a_shell_that_exits_takes_its_pane_with_it() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.open(&pane).unwrap();
    c.open(&w.ws).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)
            && c.nodes.contains_key(&w.ws))
            .unwrap()
    );
    type_line(&mut c, &pane, "exit");
    assert!(
        c.wait_until(WAIT, |c| !c.nodes.contains_key(&pane))
            .unwrap()
    );
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&w.ws]["panes"]
            .as_array()
            .unwrap()
            .is_empty())
            .unwrap()
    );
    d.stop();
}

#[test]
fn removing_a_workspace_ends_its_shells() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    let pidfile = w.dir.join("pid");
    type_line(&mut c, &pane, &format!("echo $$ > {}", pidfile.display()));
    wait_for("the pid file", || {
        std::fs::read_to_string(&pidfile).is_ok_and(|s| !s.trim().is_empty())
    });
    let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_owned();
    c.cmd(Cmd::RemoveWorkspace {
        workspace: w.ws.clone(),
        remove_worktree: false,
    })
    .unwrap()
    .unwrap();
    wait_for("the shell to end", || !alive(&pid));
    d.stop();
}

#[test]
fn panes_come_back_after_a_restart_with_the_same_ids_and_a_live_shell() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    let second = new_pane(&mut c, &w.ws);
    drop(c);
    d.stop();

    let d = w.sb.start();
    let mut c = w.sb.client();
    c.open(&w.ws).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&w.ws)).unwrap());
    let ids: Vec<&str> = c.nodes[&w.ws]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [pane.as_str(), second.as_str()]);
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    type_line(&mut c, &pane, "echo back-again");
    assert!(
        c.wait_until(WAIT, |c| screen_has(c, &pane, "back-again"))
            .unwrap()
    );
    d.stop();
}

#[test]
fn bad_pane_requests_are_refused() {
    let (_w, d, mut c) = world();
    let nowhere = "00000000-0000-4000-8000-0000000000aa".to_owned();
    let e = c
        .cmd(Cmd::NewPane {
            workspace: nowhere.clone(),
            program: None,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = c
        .cmd(Cmd::Resize {
            pane: nowhere,
            rows: 5,
            cols: 5,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn mark_seen_ends_the_attention_for_every_client_and_a_newer_one_survives() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    let mut other = w.sb.client();
    let root = c.root.clone();
    for client in [&mut c, &mut other] {
        client.open(&root).unwrap();
        assert!(
            client
                .wait_until(WAIT, |c| c.nodes.contains_key(&root))
                .unwrap()
        );
    }
    let waiting = |c: &Client| c.nodes[&root]["attention"].as_array().unwrap().len();

    type_line(&mut c, &pane, r"printf '\033]133;D;3\007'");
    assert!(other.wait_until(WAIT, |c| waiting(c) == 1).unwrap());
    let rev = other.nodes[&root]["attention"][0]["rev"].as_u64().unwrap();

    // One client looks; the other stops showing it.
    c.cmd(Cmd::MarkSeen {
        node: pane.clone(),
        rev,
    })
    .unwrap()
    .unwrap();
    assert!(
        other.wait_until(WAIT, |c| waiting(c) == 0).unwrap(),
        "the other client cleared it too"
    );

    // A later signal is a new attention; looking at the old revision does not end it.
    type_line(&mut c, &pane, r"printf '\033]133;D;4\007'");
    assert!(other.wait_until(WAIT, |c| waiting(c) == 1).unwrap());
    c.cmd(Cmd::MarkSeen {
        node: pane.clone(),
        rev,
    })
    .unwrap()
    .unwrap();
    assert!(
        !other
            .wait_until(Duration::from_millis(300), |c| waiting(c) == 0)
            .unwrap()
    );
    assert_eq!(waiting(&other), 1);

    let nowhere = "00000000-0000-4000-8000-0000000000aa".to_owned();
    let e = c
        .cmd(Cmd::MarkSeen {
            node: nowhere,
            rev: 1,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

#[test]
fn the_program_running_in_a_pane_is_detected_and_remembered_when_it_ends() {
    let (w, d, mut c) = world();
    let pane = new_pane(&mut c, &w.ws);
    c.open(&pane).unwrap();
    c.open(&w.ws).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)
            && c.nodes.contains_key(&w.ws))
            .unwrap()
    );
    assert_eq!(c.nodes[&pane]["title"], "sh");
    assert!(
        c.nodes[&pane].get("running").is_none(),
        "at the prompt nothing runs"
    );

    // A program starts inside the shell: the pane stays a shell, and says what runs in it.
    type_line(&mut c, &pane, "sleep 30");
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&pane]["running"]["argv"][0] == "sleep")
            .unwrap(),
        "{}",
        c.nodes[&pane]
    );
    assert_eq!(
        c.nodes[&pane]["shell"], "/bin/sh",
        "the pane is still the shell"
    );
    assert_eq!(c.nodes[&pane]["title"], "sleep");
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&w.ws]["panes"][0]["title"] == "sleep")
            .unwrap(),
        "the workspace's pane row follows"
    );

    // It ends (Ctrl-C): back to the prompt, and the program is remembered.
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "\u{3}".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&pane].get("running").is_none())
            .unwrap(),
        "{}",
        c.nodes[&pane]
    );
    assert_eq!(c.nodes[&pane]["last_program"]["argv"][0], "sleep");
    assert_eq!(c.nodes[&pane]["title"], "sh");
    d.stop();
}
