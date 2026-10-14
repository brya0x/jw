//! N36: what an agent's hooks make of a pane's attention (REQ-129 to REQ-133), with JSON alone
//! and with a stand-in for `claude`.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Sandbox, WAIT, wait_kinds};
use contract::{AgentEvent, Cmd, ErrorCode};
use wire::Client;

fn world(sb: &Sandbox, config: daemon::Config) -> (daemon::Daemon, Client, String, String) {
    let d = daemon::Daemon::start(config.clone()).unwrap();
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
    let root = c.root.clone();
    c.open(&root).unwrap();
    c.open(&ws).unwrap();
    (d, c, root, ws)
}

fn new_pane(c: &mut Client, ws: &str, program: Option<Vec<&str>>) -> String {
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws.into(),
            program: program.map(|p| p.into_iter().map(str::to_owned).collect()),
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    pane
}

fn event(
    c: &mut Client,
    pane: &str,
    event: AgentEvent,
    notification_type: Option<&str>,
    tool: Option<&str>,
) {
    c.cmd(Cmd::AgentEvent {
        pane: pane.into(),
        event,
        notification_type: notification_type.map(str::to_owned),
        tool: tool.map(str::to_owned),
    })
    .unwrap()
    .unwrap();
}

#[test]
fn hooks_raise_and_end_attention_that_bubbles_up_the_tree() {
    let sb = Sandbox::new();
    let (d, mut c, root, ws) = world(&sb, sb.config());
    let pane = new_pane(&mut c, &ws, None);

    event(
        &mut c,
        &pane,
        AgentEvent::Notification,
        Some("permission_prompt"),
        None,
    );
    wait_kinds(&mut c, &pane, &["permission"]);
    wait_kinds(&mut c, &ws, &["permission"]);
    wait_kinds(&mut c, &root, &["permission"]);

    // The agent ran the tool: the person answered.
    event(&mut c, &pane, AgentEvent::PostToolUse, None, Some("Bash"));
    wait_kinds(&mut c, &pane, &[]);
    wait_kinds(&mut c, &root, &[]);

    // A plan is also a request for permission; only the plan is said.
    event(
        &mut c,
        &pane,
        AgentEvent::PreToolUse,
        None,
        Some("ExitPlanMode"),
    );
    event(
        &mut c,
        &pane,
        AgentEvent::Notification,
        Some("permission_prompt"),
        None,
    );
    wait_kinds(&mut c, &pane, &["plan"]);

    event(
        &mut c,
        &pane,
        AgentEvent::PreToolUse,
        None,
        Some("AskUserQuestion"),
    );
    wait_kinds(&mut c, &pane, &["question"]);

    // Done, and the next prompt ends it.
    event(&mut c, &pane, AgentEvent::Stop, None, None);
    wait_kinds(&mut c, &pane, &["agent_done"]);
    event(&mut c, &pane, AgentEvent::UserPromptSubmit, None, None);
    wait_kinds(&mut c, &pane, &[]);

    // An agent idle for a minute is not news.
    event(
        &mut c,
        &pane,
        AgentEvent::Notification,
        Some("idle_prompt"),
        None,
    );
    event(&mut c, &pane, AgentEvent::PreToolUse, None, Some("Bash"));
    event(&mut c, &pane, AgentEvent::Stop, None, None);
    event(&mut c, &pane, AgentEvent::SessionEnd, None, None);
    wait_kinds(&mut c, &pane, &[]);

    let e = c
        .cmd(Cmd::AgentEvent {
            pane: "00000000-0000-4000-8000-000000000999".into(),
            event: AgentEvent::Stop,
            notification_type: None,
            tool: None,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    d.stop();
}

/// A stand-in for `claude` that records its arguments and waits to be interrupted.
fn fake_claude(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> {}/claude.args\nsleep 600\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn started_with(args: &Path, n: usize) -> String {
    let end = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < end {
        if let Some(l) = std::fs::read_to_string(args)
            .ok()
            .and_then(|s| s.lines().nth(n).map(str::to_owned))
        {
            return l;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("the agent did not start");
}

#[test]
fn an_agent_is_started_with_the_hooks_and_a_gone_agent_leaves_no_attention() {
    let sb = Sandbox::new();
    let bin = fake_claude(&sb.dir);
    let mut config = sb.config();
    config.hook_exe = Some(PathBuf::from("/opt/my jwn/jwn"));
    config.env = vec![(
        "PATH".into(),
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    )];
    let (d, mut c, _, ws) = world(&sb, config.clone());
    let args = sb.dir.join("claude.args");
    let hooks = config.state_dir.join("claude-hooks.json");

    let pane = new_pane(&mut c, &ws, Some(vec!["claude"]));
    let first = started_with(&args, 0);
    assert!(
        first.contains(&format!("--settings {}", hooks.display())),
        "{first}"
    );
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(
        file["hooks"]["Stop"][0]["hooks"][0]["command"],
        "'/opt/my jwn/jwn' hook stop"
    );

    // An agent started with settings of its own keeps them and gets no second set.
    new_pane(
        &mut c,
        &ws,
        Some(vec!["claude", "--settings", "/mine.json"]),
    );
    let second = started_with(&args, 1);
    assert!(second.contains("--settings /mine.json"), "{second}");
    assert_eq!(second.matches("--settings").count(), 1, "{second}");

    // The agent is detected, finishes a turn, and then is interrupted: what it raised goes.
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&pane]["title"] == "claude")
            .unwrap()
    );
    event(&mut c, &pane, AgentEvent::Stop, None, None);
    wait_kinds(&mut c, &pane, &["agent_done"]);
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "\u{3}".into(),
    })
    .unwrap()
    .unwrap();
    wait_kinds(&mut c, &pane, &[]);
    d.stop();
}
