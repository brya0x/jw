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
    // The hook leaves without reading stdin for an event it does not know or outside a pane, so
    // the pipe may be closed already.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
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

// ---- the nudge (N180e, REQ-917): one line that names the skill of the task lists, when it is installed

/// `jw hook user_prompt_submit` in a pane, with `config` as the Claude configuration; what it printed.
fn prompt_hook(env: &Env, pane: Option<&str>, config: &std::path::Path) -> std::process::Output {
    let mut cmd = env.jw();
    cmd.args(["hook", "user_prompt_submit"])
        .env("CLAUDE_CONFIG_DIR", config);
    match pane {
        Some(p) => cmd.env(identity::env_var("PANE_ID"), p),
        None => cmd.env_remove(identity::env_var("PANE_ID")),
    };
    cmd.output().unwrap()
}

#[test]
fn the_prompt_hook_names_the_task_skill_only_when_it_is_installed() {
    let env = Env::new("hook-nudge");
    let config = env.dir.join("cfg");
    let pane = "00000000-0000-4000-8000-0000000000c1";
    // with no daemon, and no skill: nothing
    let o = prompt_hook(&env, Some(pane), &config);
    assert_eq!((o.status.code(), o.stdout.len()), (Some(0), 0));
    // the skill is installed (by the binary itself, into this private configuration)
    let name = &identity::get().name;
    assert!(
        env.jw()
            .args(["skill", "install"])
            .env("CLAUDE_CONFIG_DIR", &config)
            .output()
            .unwrap()
            .status
            .success()
    );
    // still no daemon: the nudge needs none, and it is the one JSON object and nothing else
    let o = prompt_hook(&env, Some(pane), &config);
    assert_eq!(o.status.code(), Some(0));
    let text = String::from_utf8(o.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    let line = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        line.contains(&format!("{name}-tasks"))
            && line.contains("make a plan")
            && !line.contains('\n'),
        "{line}"
    );
    assert_eq!(v.as_object().unwrap().len(), 1);
    assert_eq!(text.lines().count(), 1);
    // outside a pane of the daemon's: nothing, with the skill there or not
    let o = prompt_hook(&env, None, &config);
    assert_eq!((o.status.code(), o.stdout.len()), (Some(0), 0));
    // the other events never print it
    let o = env
        .jw()
        .args(["hook", "stop"])
        .env("CLAUDE_CONFIG_DIR", &config)
        .env(identity::env_var("PANE_ID"), pane)
        .output()
        .unwrap();
    assert_eq!(o.stdout.len(), 0);
    // a skill removed: nothing again
    std::fs::remove_dir_all(config.join("skills")).unwrap();
    assert_eq!(prompt_hook(&env, Some(pane), &config).stdout.len(), 0);
}

#[test]
fn the_prompt_hook_still_tells_a_daemon_that_is_there() {
    let env = Env::new("hook-nudge-live");
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
    let config = env.dir.join("cfg");
    assert!(
        env.jw()
            .args(["skill", "install"])
            .env("CLAUDE_CONFIG_DIR", &config)
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut c = Client::connect(&env.dir.join("s.sock"), "nudge").unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(&pane))
            .unwrap()
    );
    // a permission waits; the person's prompt ends it, and the nudge is printed beside
    assert!(
        hook(
            &env,
            Some(&pane),
            "notification",
            r#"{"notification_type":"permission_prompt"}"#
        )
        .success()
    );
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane)
            == ["permission"])
            .unwrap()
    );
    let o = prompt_hook(&env, Some(&pane), &config);
    assert!(String::from_utf8_lossy(&o.stdout).contains("hookSpecificOutput"));
    assert!(
        c.wait_until(Duration::from_secs(5), |c| kinds(c, &pane).is_empty())
            .unwrap(),
        "{:?}",
        c.nodes[&pane]["attention"]
    );
    env.stop(daemon);
}
