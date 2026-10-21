//! N14: programs start inside the shell through a wrapper, and an agent comes back after a
//! restart with its own conversation.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT};
use contract::Cmd;
use daemon::{Config, Daemon};
use wire::Client;

fn type_line(c: &mut Client, pane: &str, line: &str) {
    c.cmd(Cmd::Write {
        pane: pane.into(),
        data: format!("{line}\r"),
    })
    .unwrap()
    .unwrap();
}

fn world(config: Config) -> (Daemon, Client, String, PathBuf) {
    let d = Daemon::start(config.clone()).unwrap();
    let dir = std::fs::canonicalize(config.socket.parent().unwrap())
        .unwrap()
        .join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    let r = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap();
    let ws = match r {
        Ok(v) => v.unwrap()["workspace"].as_str().unwrap().to_owned(),
        Err(_) => c
            .nodes
            .values()
            .find_map(|n| (n["type"] == "workspace").then(|| n["id"].as_str().unwrap().to_owned()))
            .unwrap_or_default(),
    };
    (d, c, ws, dir)
}

fn new_pane(c: &mut Client, ws: &str, program: Option<Vec<&str>>) -> String {
    let program = program.map(|p| p.into_iter().map(str::to_owned).collect());
    let r = c
        .cmd(Cmd::NewPane {
            workspace: ws.into(),
            program,
            beside: None,
            dir: None,
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

#[test]
fn a_program_starts_inside_the_shell_with_its_words_intact_and_the_pane_outlives_it() {
    let sb = Sandbox::new();
    let (d, mut c, ws, _) = world(sb.config());
    // Quotes and a variable name must reach the program as they are, not be read by the shell.
    let pane = new_pane(&mut c, &ws, Some(vec!["echo", "it's a $HOME test"]));
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(WAIT, |c| screen_has(c, &pane, "it's a $HOME test"))
            .unwrap(),
        "{:?}",
        c.nodes.get(&pane)
    );
    // The program ended; the pane is the shell it left behind.
    type_line(&mut c, &pane, "echo still-here");
    assert!(
        c.wait_until(WAIT, |c| screen_has(c, &pane, "still-here"))
            .unwrap()
    );

    // A program that keeps running is detected, and ending it leaves the prompt, not a dead pane.
    let long = new_pane(&mut c, &ws, Some(vec!["sleep", "30"]));
    c.open(&long).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c
            .nodes
            .get(&long)
            .is_some_and(|n| n["running"]["argv"][0] == "sleep"))
            .unwrap()
    );
    assert_eq!(c.nodes[&long]["shell"], "/bin/sh");
    c.cmd(Cmd::Write {
        pane: long.clone(),
        data: "\u{3}".into(),
    })
    .unwrap()
    .unwrap();
    type_line(&mut c, &long, "echo back-at-the-prompt");
    assert!(
        c.wait_until(WAIT, |c| screen_has(c, &long, "back-at-the-prompt"))
            .unwrap()
    );
    d.stop();
}

#[test]
fn an_empty_program_is_refused() {
    let sb = Sandbox::new();
    let (d, mut c, ws, _) = world(sb.config());
    let e = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: Some(vec![]),
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, contract::ErrorCode::BadRequest);
    d.stop();
}

/// A stand-in for `claude` that records how it was started and then waits.
fn fake_claude(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("claude");
    // Like the real one, it saves a transcript named after the conversation id it was given.
    let body = format!(
        "#!/bin/sh\necho \"$@\" >> {d}/claude.args\nmkdir -p {d}/cfg/projects/p && : > {d}/cfg/projects/p/\"$2\".jsonl\nsleep 600\n",
        d = dir.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

#[test]
fn an_agent_gets_its_own_conversation_and_comes_back_after_a_restart_until_it_ends() {
    let sb = Sandbox::new();
    let bin = fake_claude(&sb.dir);
    let args = sb.dir.join("claude.args");
    let mut config = sb.config();
    config.env = vec![
        (
            "PATH".into(),
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        ),
        (
            "CLAUDE_CONFIG_DIR".into(),
            format!("{}/cfg", sb.dir.display()),
        ),
    ];

    // 1. Started by jwn: it is given a conversation id of its own.
    let (d, mut c, ws, _) = world(config.clone());
    let pane = new_pane(&mut c, &ws, Some(vec!["claude"]));
    wait_for("the agent to start", || !lines(&args).is_empty());
    let first = lines(&args)[0].clone();
    let id = first
        .strip_prefix("--session-id ")
        .unwrap_or_else(|| panic!("started as {first:?}"))
        .to_owned();
    assert_eq!(id.len(), 36, "a uuid: {id}");
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n["title"] == "claude"))
            .unwrap(),
        "{:?}",
        c.nodes.get(&pane)
    );
    drop(c);
    d.stop();

    // 2. The daemon restarts: the same pane comes back running `claude --resume <that id>`.
    let (d, mut c, _, _) = world(config.clone());
    wait_for("the agent to be resumed", || lines(&args).len() >= 2);
    assert_eq!(lines(&args)[1], format!("--resume {id}"));
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n["title"] == "claude"))
            .unwrap()
    );

    // 3. The agent ends; the pane is a shell, and the next restart does not bring it back.
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "\u{3}".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&pane].get("running").is_none()
            && c.nodes[&pane]["title"] == "sh")
            .unwrap(),
        "{}",
        c.nodes[&pane]
    );
    drop(c);
    d.stop();
    let (d, mut c, _, _) = world(config);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        lines(&args).len(),
        2,
        "an agent that ended is not started again: {:?}",
        lines(&args)
    );
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    assert_eq!(c.nodes[&pane]["title"], "sh");
    d.stop();
}
