//! REQ-132 through the real binary: `jwn hook <event>` as claude runs it (the pane's id in the
//! environment, the event's JSON on stdin) tells a real daemon, and it never gets in the way.

mod common;

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use common::Env;
use serde_json::Value;
use wire::Client;

/// Run `jwn hook <event>` the way an agent's hook would.
fn hook(env: &Env, pane: Option<&str>, event: &str, stdin: &str) -> std::process::ExitStatus {
    let mut cmd = env.jw();
    cmd.args(["hook", event]).stdin(Stdio::piped());
    if let Some(p) = pane {
        cmd.env(identity::env_var("PANE_ID"), p);
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait().unwrap()
}

fn kinds(c: &Client, node: &str) -> Vec<String> {
    c.nodes[node]["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_hook_tells_the_daemon_and_never_fails_the_agent() {
    let env = Env::new("hook");
    let daemon = env.start();
    let ws = env.cmd(&format!(
        r#"{{"cmd":"open_folder","path":"{}"}}"#,
        std::fs::canonicalize(&env.dir).unwrap().display()
    ))["result"]["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = env.cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#))["result"]["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut c = Client::connect(&env.dir.join("s.sock"), "hook-test").unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&pane))
            .unwrap()
    );

    assert!(
        hook(
            &env,
            Some(&pane),
            "notification",
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"x"}"#
        )
        .success()
    );
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane)
            == ["permission"])
            .unwrap(),
        "{:?}",
        c.nodes[&pane]["attention"]
    );

    assert!(
        hook(
            &env,
            Some(&pane),
            "post_tool_use",
            r#"{"tool_name":"Bash"}"#
        )
        .success()
    );
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane).is_empty())
            .unwrap()
    );

    assert!(
        hook(
            &env,
            Some(&pane),
            "pre_tool_use",
            r#"{"tool_name":"ExitPlanMode"}"#
        )
        .success()
    );
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane) == ["plan"])
            .unwrap()
    );
    assert!(hook(&env, Some(&pane), "stop", "not json at all").success());
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane)
            == ["agent_done"])
            .unwrap(),
        "a Stop with unreadable input is still a Stop"
    );

    // Whatever is wrong, the agent is not troubled: no pane, an unknown pane, an event that is
    // not one, and a daemon that is not there.
    assert!(hook(&env, None, "stop", "{}").success());
    assert!(
        hook(
            &env,
            Some("00000000-0000-4000-8000-000000000999"),
            "stop",
            "{}"
        )
        .success()
    );
    assert!(hook(&env, Some(&pane), "sneeze", "{}").success());
    let v: Value = c.nodes[&pane].clone();
    assert_eq!(
        v["attention"].as_array().unwrap().len(),
        1,
        "nothing else changed"
    );
    env.stop(daemon);
    assert!(
        hook(&env, Some(&pane), "stop", "{}").success(),
        "no daemon, no complaint"
    );
}
