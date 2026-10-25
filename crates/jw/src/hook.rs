//! `jwn hook <event>`: the command an agent's hooks run (REQ-132). It tells the daemon, by
//! the pane's id in the environment, what the agent is doing, and never gets in the agent's
//! way: whatever goes wrong it says nothing and exits 0. The one thing it ever says is the daemon's
//! answer to a `stop` while a task loop is on: keep going, with the next task as the reason.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::time::Duration;

use contract::{AgentEvent, Cmd};
use serde_json::Value;

/// The most of the hook's input that is read.
const INPUT_CAP: u64 = 256 * 1024;
const REPLY_WAIT: Duration = Duration::from_secs(3);

/// What the agent put on stdin: JSON with the event's fields. Only two are used.
fn payload() -> Value {
    let mut text = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().take(INPUT_CAP).read_to_string(&mut text);
    }
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

pub fn run(event: &str) -> ExitCode {
    let Ok(event) = serde_json::from_value::<AgentEvent>(Value::from(event)) else {
        return ExitCode::SUCCESS;
    };
    let Ok(pane) = std::env::var(identity::env_var("PANE_ID")) else {
        // Not in a pane of ours: nothing to tell.
        return ExitCode::SUCCESS;
    };
    let body = payload();
    let field = |name: &str| body[name].as_str().map(str::to_owned);
    let Ok(mut client) = wire::Client::connect(&identity::socket_path(), &identity::peer("hook"))
    else {
        return ExitCode::SUCCESS;
    };
    let reply = client.cmd_within(
        Cmd::AgentEvent {
            pane,
            event,
            notification_type: field("notification_type"),
            tool: field("tool_name"),
        },
        REPLY_WAIT,
    );
    // The daemon answers a `stop` with a reason when a task loop goes on (REQ-903): claude keeps going.
    if let Ok(Ok(Some(result))) = reply
        && let Some(reason) = result["block"]["reason"].as_str()
    {
        println!(
            "{}",
            serde_json::json!({ "decision": "block", "reason": reason })
        );
    }
    ExitCode::SUCCESS
}
