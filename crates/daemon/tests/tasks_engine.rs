//! N180d2: the task loop in a conversation (REQ-930, REQ-960 to REQ-962, REQ-918), against the fake
//! engine of the conversation tests: its hook file and environment, what a conversation's hooks raise,
//! the notes of a lap, and the turn that ends after the loop stopped. Each test has its own state.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT};
use contract::tasks::TaskOp;
use contract::{AgentEvent, Cmd};
use daemon::{Config, Daemon};
use serde_json::Value;
use wire::Client;

const ENGINE: &str = include_str!("fake-claude.py");
const LIST: &str = "- [ ] t1 first\n- [ ] t2 second (after: t1)\n";

struct Rig {
    sb: Sandbox,
    _daemon: Daemon,
    client: Client,
    config: Config,
}

fn rig() -> Rig {
    let sb = Sandbox::new();
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("claude");
    std::fs::write(&engine, ENGINE).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
    config.hook_exe = Some(PathBuf::from("/bin/jw-test"));
    config.env = vec![
        (
            "PATH".into(),
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        ),
        (
            "FAKE_LOG".into(),
            sb.dir.join("engine.log").to_string_lossy().into_owned(),
        ),
        (
            "CLAUDE_CONFIG_DIR".into(),
            sb.dir.join("claude").to_string_lossy().into_owned(),
        ),
    ];
    let daemon = Daemon::start(config.clone()).unwrap();
    let client = Client::connect(&config.socket, "test").unwrap();
    let mut r = Rig {
        sb,
        _daemon: daemon,
        client,
        config,
    };
    let root = r.client.root.clone();
    r.client.open(&root).unwrap();
    r.wait("the session", |c| c.nodes.contains_key(&root));
    r
}

impl Rig {
    fn wait(&mut self, what: &str, f: impl Fn(&Client) -> bool) {
        let end = Instant::now() + WAIT;
        while Instant::now() < end {
            let _ = self.client.poll(Duration::from_millis(20));
            if f(&self.client) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn ok(&mut self, cmd: Cmd) -> Value {
        self.client
            .cmd(cmd)
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null)
    }

    fn summary(&mut self, ws: &str) -> Value {
        let _ = self.client.poll(Duration::from_millis(5));
        let root = self.client.root.clone();
        self.client.nodes[&root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == ws)
            .cloned()
            .unwrap()
    }

    fn attention(&mut self, ws: &str) -> Vec<String> {
        self.summary(ws)["attention"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["kind"].as_str().unwrap().to_owned())
            .collect()
    }

    /// A chat with a list in its folder and a conversation open in its panel, `args` the flags `claude` was typed with.
    fn chat(&mut self, list: &str, args: &[&str]) -> (String, String, String) {
        self.chat_env(list, args, None)
    }

    /// The same, opened as a `claude` typed in the panel is: with the environment it was typed in.
    fn chat_env(
        &mut self,
        list: &str,
        args: &[&str],
        env: Option<std::collections::BTreeMap<String, String>>,
    ) -> (String, String, String) {
        let ws = self.ok(Cmd::ChatNew {})["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        let at = ws.clone();
        self.wait("the chat", |c| {
            let root = &c.root;
            c.nodes[root]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["id"] == at.as_str() && w["pane_count"] == 1)
        });
        let path = self.summary(&ws)["path"].as_str().unwrap().to_owned();
        let file = identity::tasks_path(Path::new(&path));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, list).unwrap();
        self.client.open(&ws).unwrap();
        let w = ws.clone();
        self.wait("the workspace node", |c| c.nodes.contains_key(&w));
        let pane = self.client.nodes[&ws]["panes"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = self.ok(Cmd::OpenConversation {
            pane: pane.clone(),
            resume: None,
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env,
        });
        let node = r["node"].as_str().unwrap().to_owned();
        self.client.open(&node).unwrap();
        let n = node.clone();
        self.wait("the conversation", |c| c.nodes.contains_key(&n));
        (ws, pane, node)
    }

    fn say(&mut self, node: &str, text: &str) {
        self.ok(Cmd::ConversationSend {
            node: node.into(),
            text: text.into(),
            images: Vec::new(),
            folds: Vec::new(),
            private: false,
            now: false,
        });
    }

    fn log_has(&mut self, what: &str, f: impl Fn(&str) -> bool) -> String {
        let end = Instant::now() + WAIT;
        loop {
            let log = std::fs::read_to_string(self.sb.dir.join("engine.log")).unwrap_or_default();
            if let Some(l) = log.lines().find(|l| f(l)) {
                return l.to_owned();
            }
            assert!(Instant::now() < end, "timed out waiting for {what}: {log}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn stop(&mut self, pane: &str) -> Option<String> {
        let reply = self.ok(Cmd::AgentEvent {
            pane: pane.into(),
            event: AgentEvent::Stop,
            notification_type: None,
            tool: None,
        });
        reply["block"]["reason"].as_str().map(str::to_owned)
    }

    fn notes(&self, node: &str) -> Vec<String> {
        self.client.nodes[node]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["t"] == "note")
            .map(|i| i["text"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[test]
fn the_engine_is_started_with_the_stop_hook_alone_and_its_panes_id() {
    let mut r = rig();
    let (_ws, pane, node) = r.chat(LIST, &[]);
    r.say(&node, "hola");
    let line = r.log_has("the engine's start", |l| {
        l.starts_with("start") && l.contains("--input-format")
    });
    let file = r.config.state_dir.join("claude-loop-hooks.json");
    assert!(
        line.contains(&format!("\"--settings\", \"{}\"", file.display())),
        "{line}"
    );
    assert!(line.ends_with(&format!(" pane={pane}")), "{line}");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        hooks["hooks"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["Stop", "UserPromptSubmit"]
    );
}

#[test]
fn an_engine_started_with_the_environment_it_was_typed_in_has_its_panes_id_too() {
    let mut r = rig();
    // what a shell hands the binary's `claude`: its own environment, which has no pane id of ours here
    let mut env: std::collections::BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| k == "PATH" || k == "HOME")
        .collect();
    env.insert("MY_VAR".into(), "typed".into());
    // the engine is started with this environment alone: the fake's own settings travel in it
    env.insert(
        "FAKE_LOG".into(),
        r.sb.dir.join("engine.log").to_string_lossy().into_owned(),
    );
    env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        r.sb.dir.join("claude").to_string_lossy().into_owned(),
    );
    let bin = r.sb.dir.join("bin");
    let path = format!("{}:{}", bin.display(), env["PATH"]);
    env.insert("PATH".into(), path);
    let (_ws, pane, node) = r.chat_env(LIST, &[], Some(env));
    r.say(&node, "hola");
    let line = r.log_has("the engine's start", |l| {
        l.starts_with("start") && l.contains("--input-format")
    });
    assert!(
        line.contains(" var=typed "),
        "it ran with the environment it was typed in: {line}"
    );
    assert!(line.ends_with(&format!(" pane={pane}")), "{line}");
    assert!(line.contains("claude-loop-hooks.json"), "{line}");
}

#[test]
fn a_person_who_started_the_engine_with_settings_of_their_own_gets_none_of_ours() {
    let mut r = rig();
    let own = r.sb.dir.join("own.json");
    std::fs::write(&own, "{}").unwrap();
    let (_ws, _pane, node) = r.chat(LIST, &["--settings", own.to_str().unwrap()]);
    r.say(&node, "hola");
    let line = r.log_has("the engine's start", |l| {
        l.starts_with("start") && l.contains("--input-format")
    });
    assert_eq!(line.matches("--settings").count(), 1, "{line}");
    assert!(
        line.contains(own.to_str().unwrap()) && !line.contains("claude-loop-hooks"),
        "{line}"
    );
}

#[test]
fn a_stop_from_a_conversation_raises_nothing() {
    let mut r = rig();
    let (ws, pane, _node) = r.chat(LIST, &[]);
    assert_eq!(r.stop(&pane), None);
    assert_eq!(
        r.attention(&ws),
        Vec::<String>::new(),
        "the conversation's attention is the engine's own"
    );
}

#[test]
fn a_loop_in_a_conversation_tells_the_agent_notes_each_lap_and_ends_with_a_question() {
    let mut r = rig();
    let (ws, pane, node) = r.chat(LIST, &[]);
    // no pane named: the workspace's agent is the conversation, and it is told the first task
    let on = r.ok(Cmd::TasksLoopOn {
        workspace: ws.clone(),
        max: None,
        pane: None,
    });
    assert_eq!(on["task"]["id"], "t1");
    r.log_has("the first task sent to the engine", |l| {
        l.starts_with("user ") && l.contains("Next ready task in") && l.contains("t1 first")
    });
    // the agent's hook asks at the end of the turn: the next task, and a note of the lap
    r.ok(Cmd::TasksEdit {
        workspace: ws.clone(),
        op: TaskOp::Done,
        id: "t1".into(),
        text: None,
    });
    let reason = r.stop(&pane).expect("a lap");
    assert!(reason.contains("t2 second (after: t1)"), "{reason}");
    let n = node.clone();
    r.wait("the note of the lap", |c| {
        c.nodes[&n]["items"].as_array().unwrap().iter().any(|i| {
            i["t"] == "note"
                && i["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Task loop, lap 2/20: t2 second")
        })
    });
    // the person is needed: the loop stops, says so in the conversation, and the turn that ends after is a question
    r.ok(Cmd::TasksEdit {
        workspace: ws.clone(),
        op: TaskOp::Block,
        id: "t2".into(),
        text: Some("needs the key".into()),
    });
    assert_eq!(r.stop(&pane), None);
    let n = node.clone();
    r.wait("the note of the stop", |c| {
        c.nodes[&n]["items"].as_array().unwrap().iter().any(|i| {
            i["t"] == "note" && i["text"] == "Task loop stopped: t2 blocked: needs the key"
        })
    });
    assert_eq!(r.notes(&node).len(), 2);
    r.say(&node, "sigue");
    let w = ws.clone();
    r.wait("a question and not a turn done", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| {
                x["id"] == w.as_str()
                    && x["attention"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a["kind"] == "question")
            })
    });
    assert_eq!(r.attention(&ws), ["question"]);
    // and only that one turn: the next ends as any other
    r.say(&node, "otra");
    let w = ws.clone();
    r.wait("an ordinary turn done", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| {
                x["id"] == w.as_str()
                    && x["attention"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a["kind"] == "agent_done")
            })
    });
    assert_eq!(r.attention(&ws), ["agent_done"]);
}

#[test]
fn what_a_loop_hands_out_is_never_a_message_of_the_person() {
    let mut r = rig();
    let (ws, pane, node) = r.chat(LIST, &[]);
    r.ok(Cmd::TasksLoopOn {
        workspace: ws.clone(),
        max: None,
        pane: Some(pane.clone()),
    });
    r.ok(Cmd::TasksEdit {
        workspace: ws.clone(),
        op: TaskOp::Done,
        id: "t1".into(),
        text: None,
    });
    r.stop(&pane).expect("a lap");
    let users = r.client.nodes[&node]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["t"] == "user")
        .count();
    assert_eq!(
        users, 0,
        "the reason of a lap is the hook's, not an item of the person's"
    );
    assert!(
        r.summary(&ws)
            .get("chat")
            .is_some_and(|c| c.get("last").is_none()),
        "{}",
        r.summary(&ws)
    );
}
