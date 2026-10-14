//! N17: the shell integration and the blocks, against real zsh, bash and fish (a shell that
//! is not installed is skipped).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Sandbox, WAIT};
use contract::Cmd;
use daemon::Daemon;
use serde_json::Value;
use wire::Client;

fn find(name: &str) -> Option<String> {
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

struct Rig {
    _sb: Sandbox,
    d: Option<Daemon>,
    c: Client,
    pane: String,
    blocks: String,
    dir: PathBuf,
}

/// A pane running `shell` with an empty home (so the user's own rc files stay out of it),
/// with its blocks node open.
fn rig(shell: &str) -> Rig {
    let sb = Sandbox::new();
    let home = sb.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut config = sb.config();
    config.shell = Some(shell.to_owned());
    config.env = vec![
        ("HOME".into(), home.to_string_lossy().into()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").to_string_lossy().into(),
        ),
    ];
    let d = Daemon::start(config.clone()).unwrap();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
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
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "{shell}: the shell never reported a prompt: {:?}",
        c.nodes.get(&pane)
    );
    let blocks = c.nodes[&pane]["blocks"].as_str().unwrap().to_owned();
    c.open(&blocks).unwrap();
    let mut seen_msgs = Vec::new();
    let end = std::time::Instant::now() + WAIT;
    while !c.nodes.contains_key(&blocks) && std::time::Instant::now() < end {
        if let Some(m) = c.poll(Duration::from_millis(100)).unwrap() {
            seen_msgs.push(format!("{m:?}").chars().take(160).collect::<String>());
        }
    }
    assert!(
        c.nodes.contains_key(&blocks),
        "{shell}: no blocks node {blocks}; got {seen_msgs:#?}"
    );
    Rig {
        _sb: sb,
        d: Some(d),
        c,
        pane,
        blocks,
        dir,
    }
}

impl Rig {
    fn run(&mut self, line: &str) {
        let pane = self.pane.clone();
        self.c
            .cmd(Cmd::Write {
                pane,
                data: format!("{line}\r"),
            })
            .unwrap()
            .unwrap();
    }

    fn blocks(&self) -> Vec<Value> {
        self.c.nodes[&self.blocks]["blocks"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// Wait until the `n`th block (1-based) has ended.
    fn finished(&mut self, n: usize) -> Value {
        let id = self.blocks.clone();
        assert!(
            self.c
                .wait_until(Duration::from_secs(10), |c| c.nodes[&id]["blocks"]
                    .as_array()
                    .is_some_and(|b| b.len() >= n && b[n - 1]["running"] == false))
                .unwrap(),
            "block {n} did not end: {}",
            self.c.nodes[&id]
        );
        self.blocks()[n - 1].clone()
    }

    fn text(block: &Value) -> Vec<String> {
        block["out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["t"].as_str().unwrap())
                    .collect()
            })
            .collect()
    }
}

fn check_shell(shell: &str) {
    let mut r = rig(shell);

    // A command is a block: its words as typed, its folder, its status, its output, no prompt.
    r.run(r#"echo "it's $((1+1))""#);
    let b = r.finished(1);
    assert_eq!(b["cmd"], r#"echo "it's $((1+1))""#, "{shell}");
    assert_eq!(b["cwd"], r.dir.to_string_lossy().as_ref(), "{shell}");
    assert_eq!(
        (b["code"].as_i64(), b["running"].as_bool()),
        (Some(0), Some(false)),
        "{shell}"
    );
    assert!(b["ms"].as_u64().is_some());
    assert_eq!(
        Rig::text(&b),
        ["it's 2"],
        "{shell}: the output is the command's, not the prompt's"
    );

    // The status of a failure.
    r.run("false");
    assert_eq!(r.finished(2)["code"], 1, "{shell}");

    // Colours become spans.
    r.run(r"printf '\033[1;31mred\033[0m plain\n'");
    let b = r.finished(3);
    let row = b["out"][0].as_array().unwrap();
    assert_eq!(
        (
            row[0]["t"].as_str(),
            row[0]["fg"].as_u64(),
            row[0]["b"].as_bool()
        ),
        (Some("red"), Some(1), Some(true)),
        "{shell}: {row:?}"
    );

    // A running command has no status yet, and ends.
    r.run("sleep 1.5");
    let id = r.blocks.clone();
    assert!(
        r.c.wait_until(WAIT, |c| c.nodes[&id]["blocks"]
            .as_array()
            .is_some_and(|b| b.len() == 4 && b[3]["running"] == true))
            .unwrap(),
        "{shell}"
    );
    let running = r.blocks()[3].clone();
    assert!(
        running.get("code").is_none() && running.get("ms").is_none(),
        "{shell}: {running}"
    );
    assert_eq!(r.finished(4)["code"], 0, "{shell}");

    // The folder a command starts in.
    r.run("cd / && pwd");
    let b = r.finished(5);
    assert_eq!(
        b["cwd"],
        r.dir.to_string_lossy().as_ref(),
        "{shell}: it started where it was"
    );
    r.run("true");
    assert_eq!(
        r.finished(6)["cwd"],
        "/",
        "{shell}: the next one starts where the cd left it"
    );

    // A program that takes the whole screen is a block with no output, and the node says so.
    r.run(r"printf '\033[?1049hhello\033[?1049l'");
    let b = r.finished(7);
    assert_eq!(
        (b["full"].as_bool(), b["out"].as_array().map(Vec::len)),
        (Some(true), Some(0)),
        "{shell}"
    );
    assert_eq!(r.c.nodes[&r.blocks]["mode"], "blocks");

    // The pane's own screen is still there, and carries the shell.
    assert_eq!(r.c.nodes[&r.pane]["blocks"], r.blocks.as_str());
    r.d.take().unwrap().stop();
}

#[test]
fn zsh_reports_its_commands_as_blocks() {
    match find("zsh") {
        Some(p) => check_shell(&p),
        None => eprintln!("zsh is not installed; skipped"),
    }
}

#[test]
fn bash_reports_its_commands_as_blocks() {
    match find("bash") {
        Some(p) => check_shell(&p),
        None => eprintln!("bash is not installed; skipped"),
    }
}

#[test]
fn fish_reports_its_commands_as_blocks() {
    match find("fish") {
        Some(p) => check_shell(&p),
        None => eprintln!("fish is not installed; skipped"),
    }
}

#[test]
fn a_shell_without_the_integration_has_no_blocks() {
    let sb = Sandbox::new();
    let d = sb.start(); // /bin/sh
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
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo hi\r".into(),
    })
    .unwrap()
    .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    c.wait_until(Duration::from_millis(200), |_| false).unwrap();
    assert!(
        c.nodes[&pane].get("blocks").is_none(),
        "sh has no integration, so the pane is only a screen"
    );
    d.stop();
}

#[test]
fn a_program_started_by_the_wrapper_leaves_a_shell_with_blocks() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let sb = Sandbox::new();
    let home = sb.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut config = sb.config();
    config.shell = Some(shell);
    config.env = vec![("HOME".into(), home.to_string_lossy().into())];
    let d = Daemon::start(config.clone()).unwrap();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
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
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: Some(vec!["echo".into(), "from the wrapper".into()]),
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    // After the program, `exec $SHELL -i` brings the integration back: a prompt is reported.
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "{:?}",
        c.nodes.get(&pane)
    );
    d.stop();
}
