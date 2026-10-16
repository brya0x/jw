//! N81: a conversation, against a fake engine that speaks the messages recorded in
//! `docs/claude-engine.md` (REQ-371 to REQ-396). The real `claude` is tried by hand.

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT, kinds};
use contract::conversation::{Accent, Mode};
use contract::{Cmd, ErrorCode};
use daemon::{Config, Daemon};
use serde_json::Value;
use wire::Client;

/// A stand-in for the engine (`fake-claude.py`, shared with the binary's tests): it reads the
/// person's lines and answers them the way `claude -p --output-format stream-json` does. What the
/// message says picks what it does.
const ENGINE: &str = include_str!("fake-claude.py");

struct Rig {
    sb: Sandbox,
    daemon: Option<Daemon>,
    client: Client,
    ws: String,
    pane: String,
    log: PathBuf,
}

fn rig_with(sb: Sandbox, extra_env: Vec<(String, String)>) -> Rig {
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("claude");
    std::fs::write(&engine, ENGINE).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = sb.dir.join("engine.log");
    let mut config: Config = sb.config();
    config.env = vec![
        (
            "PATH".into(),
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        ),
        ("FAKE_LOG".into(), log.to_string_lossy().into_owned()),
    ];
    config.env.extend(extra_env);
    let daemon = Daemon::start(config.clone()).unwrap();
    let proj = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let mut client = Client::connect(&config.socket, "test").unwrap();
    let ws = client
        .cmd(Cmd::OpenFolder {
            path: proj.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = client
        .cmd(Cmd::NewPane {
            workspace: ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    client.open(&pane).unwrap();
    Rig {
        sb,
        daemon: Some(daemon),
        client,
        ws,
        pane,
        log,
    }
}

fn rig() -> Rig {
    rig_with(Sandbox::new(), Vec::new())
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("timed out waiting for {what}");
}

fn log_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

impl Rig {
    fn open(&mut self) -> (String, String) {
        self.open_with(None, Vec::new(), None)
    }

    fn open_with(
        &mut self,
        resume: Option<&str>,
        args: Vec<&str>,
        env: Option<BTreeMap<String, String>>,
    ) -> (String, String) {
        let r = self
            .client
            .cmd(Cmd::OpenConversation {
                pane: self.pane.clone(),
                resume: resume.map(str::to_owned),
                args: args.into_iter().map(str::to_owned).collect(),
                env,
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let node = r["node"].as_str().unwrap().to_owned();
        self.client.open(&node).unwrap();
        wait_for("the conversation node", || {
            self.client.poll(Duration::from_millis(20)).is_ok()
                && self.client.nodes.contains_key(&node)
        });
        (node, r["session"].as_str().unwrap().to_owned())
    }

    fn cmd(&mut self, cmd: Cmd) -> Result<Option<Value>, contract::ErrorBody> {
        self.client.cmd(cmd).unwrap()
    }

    fn send(&mut self, node: &str, text: &str) {
        self.cmd(Cmd::ConversationSend {
            node: node.into(),
            text: text.into(),
            images: Vec::new(),
        })
        .unwrap();
    }

    fn conv(&mut self, node: &str) -> Value {
        let _ = self.client.poll(Duration::from_millis(5));
        self.client.nodes[node].clone()
    }

    fn until(&mut self, node: &str, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + WAIT;
        loop {
            let v = self.conv(node);
            if f(&v) {
                return v;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}: {v:#}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn idle_after(&mut self, node: &str, results: usize) -> Value {
        self.until(node, &format!("{results} result(s) and idle"), |v| {
            v["status"] == "idle"
                && v["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|i| i["t"] == "result")
                    .count()
                    >= results
        })
    }

    fn stop(mut self) {
        if let Some(d) = self.daemon.take() {
            d.stop();
        }
        drop(self.sb);
    }
}

fn items<'a>(v: &'a Value, t: &str) -> Vec<&'a Value> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["t"] == t)
        .collect()
}

#[test]
fn a_new_conversation_starts_the_engine_and_a_turn_streams_to_its_end() {
    let mut r = rig();
    let (node, session) = r.open();
    assert_eq!(
        session.len(),
        36,
        "the daemon chose the engine's session id"
    );
    // REQ-376: the pane names its conversation
    wait_for("the pane to name the conversation", || {
        let _ = r.client.poll(Duration::from_millis(20));
        r.client
            .nodes
            .get(&r.pane)
            .is_some_and(|p| p["conversation"] == node.as_str())
    });
    wait_for("the engine to start", || !log_lines(&r.log).is_empty());
    let started = log_lines(&r.log);
    assert!(started[0].contains("\"-p\""), "{started:?}");
    for flag in [
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "--session-id",
        session.as_str(),
        "--permission-mode",
        "manual",
    ] {
        assert!(started[0].contains(flag), "{flag} in {}", started[0]);
    }
    assert!(
        !started[0].contains("no-session-persistence"),
        "resume needs the transcript"
    );

    r.send(&node, "arregla el test que falla en auth::expired");
    let v = r.idle_after(&node, 1);
    assert_eq!(v["model"], "sonnet-5.5", "the engine said it");
    assert_eq!(v["name"], "arregla el test que falla e…", "REQ-388");
    let kinds_of: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["t"].as_str().unwrap())
        .collect();
    assert_eq!(kinds_of, ["user", "text", "result"], "{v:#}");
    assert_eq!(items(&v, "text")[0]["text"], "Voy a mirar.");
    assert!(
        items(&v, "text")[0].get("live").is_none(),
        "no longer arriving"
    );
    assert_eq!(v["cost"], 60_000);
    assert_eq!(items(&v, "result")[0]["cost"], 60_000);
    // the person is told the turn is done
    wait_for("agent_done", || {
        let _ = r.client.poll(Duration::from_millis(20));
        kinds(&r.client, &r.pane) == ["agent_done"]
    });
    r.stop();
}

#[test]
fn the_engine_is_asked_what_it_can_do_and_the_model_and_the_commands_show_before_any_turn() {
    let mut r = rig();
    let (node, _) = r.open();
    // REQ-458: the model is the one the engine runs with, not a placeholder, from the start
    let v = r.until(&node, "the answer to initialize", |v| {
        v["model"] == "sonnet-5.5" && v["commands"].as_array().is_some_and(|c| c.len() > 7)
    });
    assert!(
        log_lines(&r.log).iter().any(|l| l == "control initialize"),
        "{:?}",
        log_lines(&r.log)
    );
    // REQ-417: jwn's own, and what the engine lists tagged by where it is defined
    let find = |name: &str| {
        v["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is not in {:#}", v["commands"]))
    };
    for (name, source, kind) in [
        ("/exit", "jwn", "local"),
        ("/mode", "jwn", "local"),
        ("/context", "claude", "text"),
        ("/init", "claude", "prompt"),
        ("/doctor", "claude", "screen"),
        ("/deploy", "skill", "prompt"),
        ("/vercel:deploy", "plugin", "prompt"),
    ] {
        let c = find(name);
        assert_eq!(
            (c["source"].as_str(), c["kind"].as_str()),
            (Some(source), Some(kind)),
            "{name}"
        );
    }
    assert_eq!(find("/context")["description"], "Show context usage");
    let names: Vec<&str> = v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"/help") && names.contains(&"/permissions") && names.contains(&"/plan"),
        "all of Claude's and jwn's"
    );
    assert_eq!(find("/help")["kind"], "panel");
    assert_eq!(find("/rewind")["kind"], "screen");
    assert_eq!(
        names.iter().filter(|n| **n == "/model").count(),
        1,
        "jwn's own replaces the engine's"
    );
    // the skills the engine's init lists tell a skill from a command of the project
    r.send(&node, "hola");
    let v = r.until(&node, "the skills to be known", |v| {
        v["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "/deploy" && c["source"] == "project")
    });
    let jw = v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "/jw")
        .unwrap();
    assert_eq!(jw["source"], "skill");
    r.stop();
}

#[test]
fn the_model_asked_for_with_the_flag_is_the_one_shown() {
    let mut r = rig();
    let (node, _) = r.open_with(None, vec!["--model", "opus"], None);
    r.until(&node, "the model the engine resolves", |v| {
        v["model"] == "opus-5.5"
    });
    r.stop();
}

#[test]
fn a_command_that_needs_claudes_screen_is_not_sent_and_says_where_to_go() {
    let mut r = rig();
    let (node, _) = r.open();
    let e = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "/doctor".into(),
            images: vec![],
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(e.message.contains("command claude"), "{}", e.message);
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !log_lines(&r.log).iter().any(|l| l.contains("/doctor")),
        "it never reached the engine"
    );
    // so is one that was only listed, not served
    let e = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "/rewind".into(),
            images: vec![],
        })
        .unwrap_err();
    assert!(e.message.contains("command claude"), "{}", e.message);
    // and a list a client draws is not the engine's either
    let e = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "/resume".into(),
            images: vec![],
        })
        .unwrap_err();
    assert!(e.message.contains("lista"), "{}", e.message);
    r.stop();
}

#[test]
fn plan_enters_plan_mode_and_sends_what_follows_as_the_first_message_of_the_plan() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "/plan");
    r.until(&node, "plan mode", |v| v["mode"] == "plan");
    wait_for("the engine to have been asked", || {
        log_lines(&r.log)
            .iter()
            .any(|l| l == "control set_permission_mode plan")
    });
    assert!(
        !log_lines(&r.log).iter().any(|l| l.starts_with("user")),
        "a bare /plan sends nothing to the model"
    );
    // back to manual, then a plan with its first message
    r.send(&node, "/mode manual");
    r.until(&node, "manual again", |v| v["mode"] == "manual");
    r.send(&node, "/plan cambia el nombre de verify");
    let v = r.idle_after(&node, 1);
    assert_eq!(v["mode"], "plan");
    assert_eq!(items(&v, "user")[0]["text"], "cambia el nombre de verify");
    let log = log_lines(&r.log);
    let mode_at = log
        .iter()
        .rposition(|l| l == "control set_permission_mode plan")
        .unwrap();
    let user_at = log.iter().position(|l| l.starts_with("user")).unwrap();
    assert!(mode_at < user_at, "the mode goes first: {log:?}");
    r.stop();
}

#[test]
fn the_conversations_claude_saved_for_the_folder_are_listed_newest_first() {
    let sb = Sandbox::new();
    let config = sb.dir.join("claude-config");
    let mut r = rig_with(
        sb,
        vec![(
            "CLAUDE_CONFIG_DIR".into(),
            config.to_string_lossy().into_owned(),
        )],
    );
    let proj = std::fs::canonicalize(&r.sb.dir).unwrap().join("proj");
    let encoded: String = proj
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let dir = config.join("projects").join(encoded);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("aaaa.jsonl"),
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"la primera\"}}\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    std::fs::write(
        dir.join("bbbb.jsonl"),
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"la segunda\"}}\n{\"type\":\"ai-title\",\"aiTitle\":\"Título\"}\n",
    )
    .unwrap();
    let v = r
        .cmd(Cmd::ListConversations {
            workspace: r.ws.clone(),
        })
        .unwrap()
        .unwrap();
    let list = v["conversations"].as_array().unwrap();
    let shown: Vec<(&str, &str)> = list
        .iter()
        .map(|c| (c["id"].as_str().unwrap(), c["name"].as_str().unwrap()))
        .collect();
    assert_eq!(shown, [("bbbb", "Título"), ("aaaa", "la primera")]);
    assert!(list[0]["at"].as_u64().unwrap() >= list[1]["at"].as_u64().unwrap());
    let e = r
        .cmd(Cmd::ListConversations {
            workspace: "00000000-0000-4000-8000-00000000dead".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    r.stop();
}

#[test]
fn a_permission_waits_for_whoever_answers_first_and_says_where_it_came_from() {
    let mut r = rig();
    let (node, _) = r.open();
    let mut phone = Client::connect(&r.sb.config().socket, "phone").unwrap();
    phone.open(&node).unwrap();
    r.send(&node, "please edit auth");
    let v = r.until(&node, "the question", |v| v["status"] == "asking");
    assert_eq!(v["ask"]["kind"], "permission");
    assert_eq!(v["ask"]["title"], "Editar src/auth.rs");
    assert_eq!(v["ask"]["diff"].as_array().unwrap().len(), 2);
    let keys: Vec<&str> = v["ask"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["yes", "no"],
        "no answer for the session: the engine suggested none"
    );
    let tool = items(&v, "tool")[0];
    assert_eq!(tool["state"], "ask");
    wait_for("the permission mark", || {
        let _ = r.client.poll(Duration::from_millis(20));
        kinds(&r.client, &r.pane) == ["permission"]
    });
    let ask = v["ask"]["id"].as_str().unwrap().to_owned();
    // a wrong question is refused and leaves everything as it was
    let e = r
        .cmd(Cmd::ConversationAnswer {
            node: node.clone(),
            ask: "nope".into(),
            key: "yes".into(),
            text: None,
            by: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    // the phone answers
    phone
        .cmd(Cmd::ConversationAnswer {
            node: node.clone(),
            ask,
            key: "yes".into(),
            text: None,
            by: Some("el móvil".into()),
        })
        .unwrap()
        .unwrap();
    let v = r.idle_after(&node, 1);
    let tool = items(&v, "tool")[0];
    assert_eq!(tool["state"], "ok");
    assert_eq!(tool["by"], "el móvil");
    assert!(v.get("ask").is_none());
    assert_eq!(items(&v, "text").last().unwrap()["text"], "Hecho.");
    r.stop();
}

#[test]
fn a_refusal_tells_the_engine_and_the_tool_says_it_was_denied() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "edit it");
    let v = r.until(&node, "the question", |v| v["status"] == "asking");
    let ask = v["ask"]["id"].as_str().unwrap().to_owned();
    r.cmd(Cmd::ConversationAnswer {
        node: node.clone(),
        ask,
        key: "no".into(),
        text: Some("mejor no".into()),
        by: None,
    })
    .unwrap();
    let v = r.idle_after(&node, 1);
    let tool = items(&v, "tool")[0];
    assert_eq!(tool["state"], "denied");
    assert_eq!(items(&v, "text").last().unwrap()["text"], "Entendido.");
    r.stop();
}

#[test]
fn an_answer_for_the_session_allows_the_same_kind_of_request_without_asking() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "edit session one");
    let v = r.until(&node, "the question", |v| v["status"] == "asking");
    let keys: Vec<&str> = v["ask"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["yes", "session", "no"]);
    let ask = v["ask"]["id"].as_str().unwrap().to_owned();
    r.cmd(Cmd::ConversationAnswer {
        node: node.clone(),
        ask,
        key: "session".into(),
        text: None,
        by: None,
    })
    .unwrap();
    r.idle_after(&node, 1);
    // the next edit goes through with no question: the node never shows an `ask`
    r.send(&node, "edit session two");
    let v = r.idle_after(&node, 2);
    assert!(
        items(&v, "tool").iter().all(|t| t["state"] == "ok"),
        "{v:#}"
    );
    r.stop();
}

#[test]
fn an_interrupt_ends_the_turn_and_says_so() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "slow one");
    r.until(&node, "the turn to run", |v| {
        v["status"] == "streaming" || v["status"] == "thinking"
    });
    r.cmd(Cmd::ConversationInterrupt { node: node.clone() })
        .unwrap();
    let v = r.idle_after(&node, 1);
    let notes: Vec<&str> = items(&v, "note")
        .iter()
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert!(notes.contains(&"Interrumpido por ti"), "{notes:?}");
    r.stop();
}

#[test]
fn what_is_sent_while_a_turn_runs_waits_in_the_queue_and_goes_when_it_ends() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "slow first");
    r.until(&node, "the turn to run", |v| v["status"] != "idle");
    r.send(&node, "second");
    let v = r.conv(&node);
    assert_eq!(v["queue"], serde_json::json!(["second"]));
    // an interrupt does not empty the queue
    r.cmd(Cmd::ConversationInterrupt { node: node.clone() })
        .unwrap();
    let v = r.idle_after(&node, 2);
    assert_eq!(v["queue"], serde_json::json!([]));
    let sent: Vec<String> = log_lines(&r.log)
        .into_iter()
        .filter(|l| l.starts_with("user"))
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[1].contains("second"));
    r.stop();
}

#[test]
fn a_turn_the_engine_starts_by_itself_is_a_turn_like_any_other() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "monitor please");
    // one result for the message, one for what the engine started by itself
    let v = r.idle_after(&node, 2);
    let texts: Vec<&str> = items(&v, "text")
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Voy a mirar.", "Línea 1 del log."], "{v:#}");
    r.stop();
}

#[test]
fn the_model_the_effort_and_the_name_are_set_as_the_engine_says_so() {
    let mut r = rig();
    let (node, _) = r.open();
    r.cmd(Cmd::ConversationSet {
        node: node.clone(),
        name: Some("login".into()),
        accent: Some(Accent::Magenta),
        model: Some("opus".into()),
        effort: Some("max".into()),
        mode: None,
    })
    .unwrap();
    let v = r.until(&node, "the engine to accept them", |v| {
        v["model"] == "opus" && v["effort"] == "max"
    });
    assert_eq!(v["name"], "login");
    assert_eq!(v["accent"], "magenta");
    let sent: Vec<String> = log_lines(&r.log)
        .into_iter()
        .filter(|l| l.starts_with("user"))
        .collect();
    assert_eq!(
        sent,
        [
            "user \"/rename login\"",
            "user \"/model opus\"",
            "user \"/effort max\""
        ]
    );
    // a local command is not a turn
    assert!(items(&v, "result").is_empty());
    r.stop();
}

#[test]
fn the_mode_is_changed_in_the_engine_that_runs_and_never_by_restarting_it() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    r.cmd(Cmd::ConversationSet {
        node: node.clone(),
        name: None,
        accent: None,
        model: None,
        effort: None,
        mode: Some(Mode::Plan),
    })
    .unwrap();
    let v = r.until(&node, "the new mode", |v| v["mode"] == "plan");
    assert_eq!(v["status"], "idle");
    wait_for("the engine to have been asked", || {
        log_lines(&r.log)
            .iter()
            .any(|l| l == "control set_permission_mode plan")
    });
    let starts = log_lines(&r.log)
        .iter()
        .filter(|l| l.starts_with("start"))
        .count();
    assert_eq!(
        starts, 1,
        "the same engine, with its cache and its conversation"
    );
    assert!(
        items(&v, "note").iter().any(|n| n["text"] == "Modo: plan"),
        "{v:#}"
    );
    // a mode that bypasses permissions is refused when it is asked as a flag
    let e = r
        .cmd(Cmd::OpenConversation {
            pane: r.pane.clone(),
            resume: None,
            args: vec!["--permission-mode".into(), "bypassPermissions".into()],
            env: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    r.stop();
}

#[test]
fn a_turn_that_ended_is_on_disk_at_once() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    // no wait for a tick: a daemon stopped by a signal has no last save
    let file =
        r.sb.dir
            .join("state/conversations")
            .join(format!("{}.json", r.pane));
    wait_for("the file", || {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("\"result\""))
    });
    r.stop();
}

#[test]
fn an_engine_that_dies_is_dead_in_the_node_and_the_next_message_resumes_it() {
    let mut r = rig();
    let (node, session) = r.open();
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    r.send(&node, "die now");
    let v = r.until(&node, "the engine to be dead", |v| v["status"] == "dead");
    let notes: Vec<&str> = items(&v, "note")
        .iter()
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("código 3") && n.contains("boom")),
        "{notes:?}"
    );
    r.send(&node, "otra vez");
    let v = r.idle_after(&node, 2);
    assert_eq!(v["status"], "idle");
    let starts: Vec<String> = log_lines(&r.log)
        .into_iter()
        .filter(|l| l.starts_with("start"))
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(
        starts[1].contains("--resume") && starts[1].contains(&session),
        "{}",
        starts[1]
    );
    r.stop();
}

#[test]
fn shell_lines_run_in_the_folder_and_go_to_the_engine_with_the_next_message() {
    let mut r = rig();
    let (node, _) = r.open();
    r.cmd(Cmd::ConversationShell {
        node: node.clone(),
        command: "echo hola; pwd".into(),
    })
    .unwrap();
    let v = r.until(&node, "the line to end", |v| {
        items(v, "shell")
            .first()
            .is_some_and(|s| s["state"] != "run")
    });
    let shell = items(&v, "shell")[0];
    assert_eq!(shell["state"], "ok");
    assert_eq!(shell["output"][0], "hola");
    assert!(
        shell["output"][1].as_str().unwrap().ends_with("proj"),
        "{shell:#}"
    );
    r.send(&node, "qué ves");
    r.idle_after(&node, 1);
    let sent: Vec<String> = log_lines(&r.log)
        .into_iter()
        .filter(|l| l.starts_with("user"))
        .collect();
    assert!(
        sent[0].contains("! echo hola; pwd")
            && sent[0].contains("hola")
            && sent[0].contains("qué ves"),
        "{sent:?}"
    );
    r.stop();
}

#[test]
fn what_the_daemon_answers_itself_never_reaches_the_engine() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    r.send(&node, "/cost");
    let v = r.conv(&node);
    let out = items(&v, "out");
    assert_eq!(out.len(), 1);
    assert!(
        out[0]["lines"][0].as_str().unwrap().starts_with("$0,06"),
        "{out:?}"
    );
    r.send(&node, "/color red");
    r.send(&node, "/mode accept edits");
    let v = r.until(&node, "the colour and the mode", |v| {
        v["accent"] == "red" && v["mode"] == "acceptEdits"
    });
    assert_eq!(v["status"], "idle");
    let sent: Vec<String> = log_lines(&r.log)
        .into_iter()
        .filter(|l| l.starts_with("user"))
        .collect();
    assert_eq!(sent.len(), 1, "only the message went: {sent:?}");
    r.stop();
}

#[test]
fn the_engine_runs_with_the_environment_and_the_folder_it_was_typed_in() {
    let mut r = rig();
    let here = std::fs::canonicalize(&r.sb.dir).unwrap();
    let env: BTreeMap<String, String> = [
        (
            "PATH".to_owned(),
            format!(
                "{}:{}",
                here.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        ),
        ("FAKE_LOG".to_owned(), r.log.to_string_lossy().into_owned()),
        ("MY_VAR".to_owned(), "typed-here".to_owned()),
        ("PWD".to_owned(), here.to_string_lossy().into_owned()),
    ]
    .into();
    r.open_with(
        None,
        vec!["--model", "opus", "--name", "login", "hola"],
        Some(env),
    );
    wait_for("the engine to start", || {
        log_lines(&r.log).iter().any(|l| l.starts_with("start"))
    });
    let start = log_lines(&r.log)
        .into_iter()
        .find(|l| l.starts_with("start"))
        .unwrap();
    assert!(start.contains("var=typed-here"), "{start}");
    assert!(
        start.contains(&format!("cwd={}", here.display())),
        "{start}"
    );
    assert!(
        start.contains("\"--model\", \"opus\"") && start.contains("\"--name\", \"login\""),
        "{start}"
    );
    // the first message typed after the flags goes at once
    wait_for("the first message", || {
        log_lines(&r.log)
            .iter()
            .any(|l| l.starts_with("user") && l.contains("hola"))
    });
    r.stop();
}

#[test]
fn a_command_the_engine_does_not_serve_is_refused_so_the_original_runs() {
    let mut r = rig();
    for args in [
        vec!["-p", "hola"],
        vec!["--output-format", "json"],
        vec!["mcp"],
        vec!["--resume"],
    ] {
        let e = r
            .cmd(Cmd::OpenConversation {
                pane: r.pane.clone(),
                resume: None,
                args: args.iter().map(|s| s.to_string()).collect(),
                env: None,
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Unsupported, "{args:?}");
    }
    assert!(!daemon::conversation_served(&["--print".to_owned()]));
    assert!(daemon::conversation_served(&[
        "--model".to_owned(),
        "opus".to_owned(),
        "hola".to_owned()
    ]));
    r.stop();
}

#[test]
fn closing_ends_the_engine_and_the_pane_stops_naming_the_conversation() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    r.send(&node, "/exit");
    wait_for("the pane to let go of it", || {
        let _ = r.client.poll(Duration::from_millis(20));
        r.client
            .nodes
            .get(&r.pane)
            .is_some_and(|p| p.get("conversation").is_none())
    });
    let e = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "x".into(),
            images: Vec::new(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    r.stop();
}

#[test]
fn a_daemon_that_restarts_brings_the_conversation_back_and_the_next_message_resumes_it() {
    let sb = Sandbox::new();
    let mut r = rig_with(sb, Vec::new());
    let (node, session) = r.open_with(None, vec!["--model", "opus"], None);
    r.send(&node, "hola mundo");
    r.idle_after(&node, 1);
    // the pane brings the agent back by its conversation id (REQ-393)
    wait_for("session.json to say how to resume", || {
        std::fs::read_to_string(r.sb.dir.join("state/session.json"))
            .is_ok_and(|s| s.contains("--resume") && s.contains(&session))
    });
    let dir = r.sb.dir.clone();
    let log = r.log.clone();
    // stop the daemon but keep the sandbox's directory
    let config = r.sb.config();
    r.daemon.take().unwrap().stop();
    std::mem::forget(r.sb);
    let daemon = Daemon::start(Config {
        env: vec![
            (
                "PATH".into(),
                format!(
                    "{}:{}",
                    dir.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            ),
            ("FAKE_LOG".into(), log.to_string_lossy().into_owned()),
        ],
        ..config.clone()
    })
    .unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    // the pane is still there; its conversation is opened again by what ran in it
    let node2 = c
        .cmd(Cmd::OpenConversation {
            pane: r.pane.clone(),
            resume: Some(session.clone()),
            args: Vec::new(),
            env: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["node"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&node2).unwrap();
    wait_for("the node", || {
        c.poll(Duration::from_millis(20)).is_ok() && c.nodes.contains_key(&node2)
    });
    let v = c.nodes[&node2].clone();
    assert_eq!(v["status"], "dead");
    assert_eq!(v["name"], "hola mundo");
    let t: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["t"].as_str().unwrap())
        .collect();
    assert_eq!(t, ["user", "text", "result", "divider"], "{v:#}");
    assert_eq!(v["items"][3]["kind"], "restart");
    c.cmd(Cmd::ConversationSend {
        node: node2.clone(),
        text: "sigo".into(),
        images: Vec::new(),
    })
    .unwrap()
    .unwrap();
    wait_for("the second turn", || {
        let _ = c.poll(Duration::from_millis(20));
        c.nodes[&node2]["status"] == "idle"
            && c.nodes[&node2]["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["t"] == "result")
                .count()
                == 2
    });
    let starts: Vec<String> = log_lines(&log)
        .into_iter()
        .filter(|l| l.starts_with("start"))
        .collect();
    assert!(
        starts.last().unwrap().contains("--resume") && starts.last().unwrap().contains(&session)
    );
    // what it was asked to run with comes back with it
    assert!(
        starts.last().unwrap().contains("\"--model\", \"opus\""),
        "{}",
        starts.last().unwrap()
    );
    daemon.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
