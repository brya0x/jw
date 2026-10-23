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
    rig_with_chrome(sb, extra_env, false)
}

fn rig_with_chrome(sb: Sandbox, extra_env: Vec<(String, String)>, chrome: bool) -> Rig {
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("claude");
    std::fs::write(&engine, ENGINE).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = sb.dir.join("engine.log");
    let mut config: Config = sb.config();
    config.chrome = chrome;
    config.env = vec![
        (
            "PATH".into(),
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        ),
        ("FAKE_LOG".into(), log.to_string_lossy().into_owned()),
        (
            "FAKE_TASKS_DIR".into(),
            sb.dir.join("tasks").to_string_lossy().into_owned(),
        ),
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
            folds: Vec::new(),
            private: false,
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

/// REQ-716: what is sent is kept by repository, with its folds, and comes back with the command;
/// what begins with a blank is not kept, and the file is private.
#[test]
fn the_messages_sent_are_kept_for_the_repository_and_come_back_with_their_folds() {
    let mut r = rig();
    let (node, _) = r.open();
    let send = |r: &mut Rig, text: &str, folds: Vec<[usize; 2]>, private: bool| {
        r.cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: text.into(),
            images: Vec::new(),
            folds,
            private,
        })
        .unwrap();
    };
    send(&mut r, "uno", vec![], false);
    send(&mut r, "mira\nl1\nl2\nl3\nl4 y ya", vec![[5, 17]], false);
    send(&mut r, "secreto", vec![], true);
    let reply = r
        .cmd(Cmd::ConversationHistory { node: node.clone() })
        .unwrap()
        .unwrap();
    let list: contract::conversation::SentList = serde_json::from_value(reply).unwrap();
    let texts: Vec<&str> = list.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["uno", "mira\nl1\nl2\nl3\nl4 y ya"]);
    assert_eq!(list.messages[1].folds, vec![[5, 17]]);
    let files: Vec<_> = std::fs::read_dir(r.sb.dir.join("state/sent"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one file for the one repository: {files:?}");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&files[0]), 0o600);
    assert_eq!(mode(&r.sb.dir.join("state/sent")), 0o700);
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
            folds: Vec::new(),
            private: false,
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
            folds: Vec::new(),
            private: false,
        })
        .unwrap_err();
    assert!(e.message.contains("command claude"), "{}", e.message);
    // and a list a client draws is not the engine's either
    let e = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "/resume".into(),
            images: vec![],
            folds: Vec::new(),
            private: false,
        })
        .unwrap_err();
    assert!(e.message.contains("lista"), "{}", e.message);
    r.stop();
}

/// REQ-579: the workspace's row says `working` while a conversation of it thinks, writes or
/// runs a tool, and stops saying it when the turn ends.
#[test]
fn a_workspace_says_it_works_while_its_conversation_runs_a_turn() {
    let mut r = rig();
    let (node, _) = r.open();
    let root = r.client.root.clone();
    r.client.open(&root).unwrap();
    let working = |r: &mut Rig| {
        let _ = r.client.poll(Duration::from_millis(20));
        r.client
            .nodes
            .get(&root)
            .map(|n| n["workspaces"][0]["working"] == true)
    };
    wait_for("the session", || working(&mut r).is_some());
    assert_eq!(
        working(&mut r),
        Some(false),
        "a quiet conversation does not work"
    );
    r.send(&node, "slow one");
    wait_for("the workspace to work", || working(&mut r) == Some(true));
    r.cmd(Cmd::ConversationInterrupt { node: node.clone() })
        .unwrap();
    r.idle_after(&node, 1);
    wait_for("the workspace to rest", || working(&mut r) == Some(false));
    r.stop();
}

fn inits(r: &Rig) -> usize {
    log_lines(&r.log)
        .iter()
        .filter(|l| *l == "control initialize")
        .count()
}

/// The names the list has: a turn's `init` may tell a skill from a command of the project, which
/// changes a source and not the list.
fn names(v: &Value) -> Vec<String> {
    v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect()
}

fn has_command(v: &Value, name: &str) -> bool {
    v["commands"]
        .as_array()
        .is_some_and(|c| c.iter().any(|c| c["name"] == name))
}

/// REQ-756, REQ-758: the engine is asked for its commands again when `/reload-plugins` ends, and
/// the node and `commands.json` show the answer; a conversation that never reloads is not asked.
#[test]
fn a_reload_asks_the_engine_for_its_commands_again_and_the_node_and_the_kept_list_show_them() {
    let mut r = rig();
    let (node, _) = r.open();
    r.until(&node, "the first list", |v| has_command(v, "/deploy"));
    // an ordinary turn asks nothing
    r.send(&node, "hola");
    r.idle_after(&node, 1);
    assert_eq!(inits(&r), 1, "{:?}", log_lines(&r.log));
    assert!(!has_command(&r.conv(&node), "/ponytail:ponytail"));
    r.send(&node, "/reload-plugins");
    let v = r.until(&node, "the list after the reload", |v| {
        has_command(v, "/ponytail:ponytail")
    });
    assert!(has_command(&v, "/ponytail:ponytail-review"));
    assert!(has_command(&v, "/deploy"), "what was there stays");
    assert_eq!(inits(&r), 2, "{:?}", log_lines(&r.log));
    let file = r.sb.dir.join("state").join("commands.json");
    wait_for("the kept list to follow", || {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("ponytail:ponytail-review"))
    });
    r.stop();
}

/// REQ-757: a second answer that fails, or that has no list, leaves the list the conversation had
/// and says nothing.
#[test]
fn a_reload_whose_answer_fails_or_has_no_list_keeps_the_commands_it_had() {
    for line in ["/reload-plugins fail", "/reload-skills nolist"] {
        let mut r = rig();
        let (node, _) = r.open();
        let first = r.until(&node, "the first list", |v| has_command(v, "/deploy"));
        let before = names(&first);
        r.send(&node, line);
        wait_for("the second initialize", || inits(&r) == 2);
        // the engine answers in order: once it has replied to this one, the answer was read
        r.send(&node, "hola");
        let v = r.idle_after(&node, 1);
        assert_eq!(names(&v), before, "{line}");
        assert!(items(&v, "note").is_empty(), "{line}: {v:#}");
        r.stop();
    }
}

/// REQ-758: the file keeps the list of the conversation that reloaded, and one that has not does
/// not put the old one back when it saves.
#[test]
fn a_conversation_that_has_not_reloaded_does_not_put_the_old_list_back_in_the_kept_file() {
    let mut r = rig();
    let (first, _) = r.open();
    let pane = r
        .cmd(Cmd::NewPane {
            workspace: r.ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    r.pane = pane;
    let (second, _) = r.open();
    r.until(&second, "its list", |v| has_command(v, "/deploy"));
    r.send(&first, "/reload-plugins");
    r.until(&first, "the new list", |v| {
        has_command(v, "/ponytail:ponytail")
    });
    let file = r.sb.dir.join("state").join("commands.json");
    wait_for("the new list to be kept", || {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("ponytail:ponytail"))
    });
    // every save looks for the list to keep; the one that has the old one saves three times
    for _ in 0..3 {
        r.send(&second, "hola");
    }
    r.idle_after(&second, 3);
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("ponytail:ponytail"),
        "the old list came back"
    );
    r.stop();
}

/// REQ-593: a conversation that is resumed before its engine starts has the `/` list that an engine
/// gave last, so the person can complete and see the colour of a skill from the first key.
#[test]
fn a_resumed_conversation_has_the_commands_an_engine_gave_before_its_own_starts() {
    let mut r = rig();
    let (node, _) = r.open();
    // the first engine has said its commands; they are kept for the others
    let file = r.sb.dir.join("state").join("commands.json");
    wait_for("the commands to be kept", || file.exists());
    r.until(&node, "the engine's commands", |v| {
        v["commands"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["name"] == "/deploy"))
    });
    let pane = r
        .cmd(Cmd::NewPane {
            workspace: r.ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    r.pane = pane;
    let (_file, env) = old_session(&r, &[FIRST, REPLY]);
    let (second, _) = r.open_with(None, vec!["--resume", "old-session"], Some(env));
    let v = r.conv(&second);
    assert_eq!(v["status"], "dead", "its engine has not started");
    let names: Vec<&str> = v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(
        names.contains(&"/deploy") && names.contains(&"/mode"),
        "{names:?}"
    );
    r.stop();
}

/// REQ-578: a new conversation starts in the mode the person last worked in, and a typed
/// `--permission-mode` still wins.
#[test]
fn a_new_conversation_starts_in_the_mode_the_last_one_was_left_in() {
    let mut r = rig();
    let (node, _) = r.open();
    assert_eq!(r.until(&node, "the first node", |_| true)["mode"], "manual");
    r.send(&node, "/mode acceptEdits");
    r.until(&node, "accept edits", |v| v["mode"] == "acceptEdits");
    let file = r.sb.dir.join("state").join("last-mode");
    wait_for("the mode to be remembered", || {
        std::fs::read_to_string(&file).is_ok_and(|t| t == "acceptEdits")
    });
    let pane = r
        .cmd(Cmd::NewPane {
            workspace: r.ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    r.pane = pane;
    let (second, _) = r.open();
    assert_eq!(
        r.until(&second, "the new conversation", |_| true)["mode"],
        "acceptEdits"
    );
    let pane = r
        .cmd(Cmd::NewPane {
            workspace: r.ws.clone(),
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    r.pane = pane;
    let (third, _) = r.open_with(None, vec!["--permission-mode", "plan"], None);
    assert_eq!(
        r.until(&third, "a typed mode", |_| true)["mode"],
        "plan",
        "what is typed wins over what was left"
    );
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

impl Rig {
    fn answer(
        &mut self,
        node: &str,
        ask: &str,
        key: &str,
        text: Option<&str>,
    ) -> Result<Option<Value>, contract::ErrorBody> {
        self.cmd(Cmd::ConversationAnswer {
            node: node.into(),
            ask: ask.into(),
            key: key.into(),
            text: text.map(str::to_owned),
            by: Some("el móvil".into()),
        })
    }
}

#[test]
fn the_questions_claude_asks_come_one_at_a_time_and_the_engine_gets_all_the_answers_at_the_end() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "una pregunta");
    let v = r.until(&node, "the first question", |v| v["status"] == "asking");
    let ask = &v["ask"];
    assert_eq!(ask["kind"], "question");
    assert_eq!(ask["title"], "¿Qué lenguajes?");
    assert_eq!(ask["multi"], true);
    let labels: Vec<&str> = ask["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Rust", "Kotlin", "Go"]);
    let keys: Vec<&str> = ask["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["0", "1", "2"]);
    assert_eq!(
        ask["plan"],
        serde_json::json!(["Rust: sistemas", "Kotlin: JVM"]),
        "what an option means, for those that say it"
    );
    let first = ask["id"].as_str().unwrap().to_owned();
    wait_for("the question mark", || {
        let _ = r.client.poll(Duration::from_millis(20));
        kinds(&r.client, &r.pane) == ["question"]
    });
    // a key that is no option of it is refused and it stays
    assert!(r.answer(&node, &first, "7", None).is_err());
    assert!(r.answer(&node, &first, "0,x", None).is_err());
    r.answer(&node, &first, "0,1", None).unwrap();
    let v = r.until(&node, "the second question", |v| {
        v["ask"]["title"] == "¿Qué framework?"
    });
    assert!(v["ask"]["multi"].is_null(), "a choice of one: no `multi`");
    let second = v["ask"]["id"].as_str().unwrap().to_owned();
    assert_ne!(first, second);
    // the answer for the first, late, is refused: it is not what waits
    assert!(r.answer(&node, &first, "0", None).is_err());
    // two choices for a question of one is refused
    assert!(r.answer(&node, &second, "0,1", None).is_err());
    r.answer(&node, &second, "1", None).unwrap();
    let v = r.idle_after(&node, 1);
    let tool = items(&v, "tool")[0];
    assert_eq!(tool["name"], "AskUserQuestion");
    assert_eq!(tool["summary"], "Respondida: Rust, Kotlin · B");
    assert_eq!(tool["by"], "el móvil");
    assert!(v.get("ask").is_none());
    wait_for("the engine to have the answers", || {
        log_lines(&r.log).iter().any(|l| {
            l.strip_prefix("answers ")
                .and_then(|j| serde_json::from_str::<Value>(j).ok())
                .is_some_and(|a| {
                    a == serde_json::json!({"¿Qué lenguajes?": "Rust, Kotlin", "¿Qué framework?": "B"})
                })
        })
    });
    r.stop();
}

#[test]
fn the_plan_that_waits_is_read_whole_as_blocks_with_its_file_and_is_refused_once_answered() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "un plan");
    let v = r.until(&node, "the plan", |v| v["status"] == "asking");
    let ask = v["ask"]["id"].as_str().unwrap().to_owned();
    let doc: contract::markdown::MarkdownDoc = serde_json::from_value(
        r.cmd(Cmd::ReadPlan {
            node: node.clone(),
            ask: ask.clone(),
        })
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(doc.title, "Plan");
    assert_eq!(doc.path.as_deref(), Some("/x/plan.md"), "REQ-467");
    assert!(!doc.truncated);
    assert!(matches!(
        doc.blocks[0],
        contract::markdown::MdBlock::Heading { level: 1, .. }
    ));
    assert!(matches!(
        &doc.blocks[1],
        contract::markdown::MdBlock::List { start: Some(1), items } if items.len() == 2
    ));
    // a question that is not this one waits for nothing
    let err = r
        .cmd(Cmd::ReadPlan {
            node: node.clone(),
            ask: "otra".into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    r.answer(&node, &ask, "no", None).unwrap();
    r.idle_after(&node, 1);
    let err = r.cmd(Cmd::ReadPlan { node, ask }).unwrap_err();
    assert!(err.message.contains("not waiting"), "{}", err.message);
}

#[test]
fn a_plan_longer_than_the_box_keeps_comes_whole_to_the_viewer() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "un plan largo");
    let v = r.until(&node, "the plan", |v| v["status"] == "asking");
    assert_eq!(
        v["ask"]["plan"].as_array().unwrap().len(),
        200,
        "the box keeps 200 lines"
    );
    let doc: contract::markdown::MarkdownDoc = serde_json::from_value(
        r.cmd(Cmd::ReadPlan {
            node,
            ask: v["ask"]["id"].as_str().unwrap().to_owned(),
        })
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    let contract::markdown::MdBlock::List { items, .. } = &doc.blocks[1] else {
        panic!("a list: {:?}", doc.blocks[1]);
    };
    assert_eq!(items.len(), 300, "REQ-467: the whole plan");
}

#[test]
fn a_plan_is_approved_with_edits_allowed_or_asking_or_refused_with_what_to_do_instead() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "un plan");
    let v = r.until(&node, "the plan", |v| v["status"] == "asking");
    assert_eq!(v["ask"]["kind"], "plan");
    assert_eq!(
        v["ask"]["plan"],
        serde_json::json!(["# Plan", "1. mirar", "2. editar"])
    );
    let keys: Vec<&str> = v["ask"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["accept", "manual", "no"]);
    wait_for("the plan mark", || {
        let _ = r.client.poll(Duration::from_millis(20));
        kinds(&r.client, &r.pane) == ["plan"]
    });
    let ask = v["ask"]["id"].as_str().unwrap().to_owned();
    r.answer(&node, &ask, "accept", None).unwrap();
    r.idle_after(&node, 1);
    let v = r.until(&node, "the mode the engine says it runs with", |v| {
        v["mode"] == "acceptEdits"
    });
    assert_eq!(items(&v, "tool")[0]["summary"], "Plan aprobado");
    assert!(log_lines(&r.log).iter().any(|l| l == "plan allowed"));
    assert!(
        log_lines(&r.log)
            .iter()
            .any(|l| l == "control set_permission_mode acceptEdits")
    );
    // approved asking for each edit: the mode is the engine's own, and no mode is asked of it
    r.send(&node, "otro plan");
    let v = r.until(&node, "the second plan", |v| v["status"] == "asking");
    let asked_before = log_lines(&r.log)
        .iter()
        .filter(|l| l.starts_with("control set_permission_mode"))
        .count();
    r.answer(&node, v["ask"]["id"].as_str().unwrap(), "manual", None)
        .unwrap();
    let v = r.idle_after(&node, 2);
    assert_eq!(v["mode"], "manual");
    assert_eq!(
        log_lines(&r.log)
            .iter()
            .filter(|l| l.starts_with("control set_permission_mode"))
            .count(),
        asked_before
    );
    // refused, with what to do instead
    r.send(&node, "un plan más");
    let v = r.until(&node, "the third plan", |v| v["status"] == "asking");
    r.answer(
        &node,
        v["ask"]["id"].as_str().unwrap(),
        "no",
        Some("añade una verificación"),
    )
    .unwrap();
    let v = r.idle_after(&node, 3);
    assert_eq!(items(&v, "tool")[2]["state"], "denied");
    assert!(
        log_lines(&r.log).iter().any(|l| l == "plan denied El usuario no aprobó el plan y dice: añade una verificación"),
        "{:?}",
        log_lines(&r.log)
    );
    r.stop();
}

#[test]
fn a_subagent_is_a_card_with_its_steps_progress_and_totals_and_a_task_that_ran_in_front_leaves_no_note()
 {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "usa un agente");
    let v = r.idle_after(&node, 1);
    let agent = items(&v, "agent")[0];
    assert_eq!(agent["name"], "Explore");
    assert_eq!(agent["description"], "buscar los usos");
    assert_eq!(agent["state"], "ok");
    assert_eq!(
        (
            agent["tools"].as_u64(),
            agent["tokens"].as_u64(),
            agent["ms"].as_u64()
        ),
        (Some(3), Some(22024), Some(6000))
    );
    assert_eq!(agent["last"], "Read");
    assert_eq!(
        agent["steps"],
        serde_json::json!(["Grep verify"]),
        "what it did with its own tools"
    );
    assert!(items(&v, "tool").is_empty(), "the agent is not a tool line");
    let tasks = v["tasks"].as_array().unwrap();
    assert_eq!(
        (
            tasks[0]["id"].as_str(),
            tasks[0]["kind"].as_str(),
            tasks[0]["state"].as_str()
        ),
        (Some("tsk1"), Some("agent"), Some("done"))
    );
    assert!(
        items(&v, "note").is_empty(),
        "it ran in front and its card says it"
    );
    r.stop();
}

#[test]
fn a_task_in_the_background_is_listed_leaves_a_note_when_it_ends_and_can_be_stopped() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "algo de fondo");
    let v = r.idle_after(&node, 1);
    let task = &v["tasks"].as_array().unwrap()[0];
    assert_eq!(
        (
            task["id"].as_str(),
            task["kind"].as_str(),
            task["state"].as_str(),
            task["label"].as_str()
        ),
        (Some("bg1"), Some("bash"), Some("run"), Some("dormir 45 s"))
    );
    // the tool's own answer is only that it started
    assert_eq!(items(&v, "tool")[0]["state"], "ok");
    let e = r
        .cmd(Cmd::ConversationStopTask {
            node: node.clone(),
            task: "nope".into(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    r.cmd(Cmd::ConversationStopTask {
        node: node.clone(),
        task: "bg1".into(),
    })
    .unwrap();
    let v = r.until(&node, "the task stopped and told", |v| {
        v["tasks"][0]["state"] == "stopped" && !items(v, "note").is_empty()
    });
    assert!(
        log_lines(&r.log)
            .iter()
            .any(|l| l == "control stop_task bg1")
    );
    let notes: Vec<&str> = items(&v, "note")
        .iter()
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        notes,
        ["Tarea se detuvo: dormir 45 s"],
        "told once, without repeating what it was called: {v:#}"
    );
    // one that is not running cannot be stopped again
    assert!(
        r.cmd(Cmd::ConversationStopTask {
            node,
            task: "bg1".into()
        })
        .is_err()
    );
    r.stop();
}

#[test]
fn a_shell_and_a_monitor_show_the_last_lines_they_print_and_keep_them_when_they_end() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "algo de fondo");
    // The plain lines of the file the engine wrote: the colour codes are gone.
    let v = r.until(&node, "the tail of the shell", |v| {
        v["tasks"][0]["tail"] == serde_json::json!(["uno", "dos"])
    });
    assert_eq!(v["tasks"][0]["kind"], "bash");
    // A monitor is `local_bash` for the engine too; the tool says what it is, and its result does
    // not say where its output is: it is beside the one of the shell.
    r.send(&node, "vigila el log");
    let v = r.until(&node, "the tail of the monitor", |v| {
        v["tasks"].as_array().is_some_and(|t| {
            t.iter()
                .any(|t| t["id"] == "mon1" && t["tail"] == serde_json::json!(["ERROR uno"]))
        })
    });
    let kinds: Vec<_> = v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["id"].as_str().unwrap(), t["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, [("bg1", "bash"), ("mon1", "monitor")]);
    // What it printed as it ended is read once more, without the line the engine adds.
    r.cmd(Cmd::ConversationStopTask {
        node: node.clone(),
        task: "bg1".into(),
    })
    .unwrap();
    r.until(&node, "the last read of the shell that ended", |v| {
        v["tasks"][0]["state"] == "stopped"
            && v["tasks"][0]["tail"] == serde_json::json!(["uno", "dos", "tres"])
    });
    r.stop();
}

#[test]
fn the_checklist_is_the_nodes_and_a_notification_of_claudes_is_a_note_that_asks_for_attention() {
    let mut r = rig();
    let (node, _) = r.open();
    r.send(&node, "haz una lista de todo");
    let v = r.idle_after(&node, 1);
    assert_eq!(
        v["todos"],
        serde_json::json!([{"text": "Buscar", "done": true}, {"text": "Compilar", "done": false}])
    );
    assert!(
        items(&v, "tool").is_empty(),
        "the checklist is not a line of the conversation"
    );
    r.send(&node, "avisa con un push");
    let v = r.idle_after(&node, 2);
    assert!(
        items(&v, "note")
            .iter()
            .any(|n| n["text"] == "tests listos"),
        "{v:#}"
    );
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
        items(&v, "note").is_empty(),
        "the mode is in the footer, not a line: {v:#}"
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

/// REQ-630, OPEN-20: a conversation opened from a pane with no environment (what the phone's
/// button and a resume send) starts in the pane's folder, which is the workspace's here, and the
/// node says so.
#[test]
fn a_conversation_opened_with_no_environment_starts_in_the_panes_folder() {
    let mut r = rig();
    let (node, _) = r.open_with(None, vec!["hola"], None);
    let cwd = r.conv(&node)["cwd"].as_str().unwrap().to_owned();
    assert!(std::path::Path::new(&cwd).is_absolute(), "{cwd}");
    wait_for("the engine to start", || {
        log_lines(&r.log).iter().any(|l| l.starts_with("start"))
    });
    let start = log_lines(&r.log)
        .into_iter()
        .find(|l| l.starts_with("start"))
        .unwrap();
    assert!(
        start.contains(&format!("cwd={cwd}")),
        "{start} (node says {cwd})"
    );
}

/// REQ-634: the engine gets the Claude in Chrome tools when `config.toml` leaves them on, and not
/// when it turns them off, and what the person typed wins; a conversation opened with no
/// arguments (a resume, the phone's button) gets the default.
#[test]
fn the_engine_is_given_chrome_when_the_config_says_so_and_what_is_typed_wins() {
    let start_of = |r: &Rig| {
        wait_for("the engine to start", || {
            log_lines(&r.log).iter().any(|l| l.starts_with("start"))
        });
        log_lines(&r.log)
            .into_iter()
            .find(|l| l.starts_with("start"))
            .unwrap()
    };
    // on by default: a plain open has `--chrome`
    let mut on = rig_with_chrome(Sandbox::new(), Vec::new(), true);
    on.open_with(None, vec!["hola"], None);
    let s = start_of(&on);
    assert!(
        s.contains("\"--chrome\"") && !s.contains("--no-chrome"),
        "{s}"
    );
    on.daemon.take().unwrap().stop();
    // typed `--no-chrome` wins over the config
    let mut off_typed = rig_with_chrome(Sandbox::new(), Vec::new(), true);
    off_typed.open_with(None, vec!["--no-chrome", "hola"], None);
    let s = start_of(&off_typed);
    assert!(
        s.contains("--no-chrome") && !s.contains("\"--chrome\""),
        "{s}"
    );
    off_typed.daemon.take().unwrap().stop();
    // off in the config: nothing is passed, and a typed `--chrome` wins
    let mut off = rig_with_chrome(Sandbox::new(), Vec::new(), false);
    off.open_with(None, vec!["hola"], None);
    let s = start_of(&off);
    assert!(!s.contains("chrome"), "{s}");
    off.daemon.take().unwrap().stop();
    let mut typed = rig_with_chrome(Sandbox::new(), Vec::new(), false);
    typed.open_with(None, vec!["--chrome", "hola"], None);
    assert!(start_of(&typed).contains("\"--chrome\""));
    typed.daemon.take().unwrap().stop();
}

/// REQ-572: `jwn claude` adds `--prompt-suggestions false` when `config.toml` turns the
/// suggestions off, and the daemon must serve it, or the person would silently get the original claude.
#[test]
fn a_prompt_suggestions_argument_is_served_and_reaches_the_engine() {
    let mut r = rig();
    r.open_with(None, vec!["--prompt-suggestions", "false", "hola"], None);
    wait_for("the engine to start", || {
        log_lines(&r.log).iter().any(|l| l.starts_with("start"))
    });
    let start = log_lines(&r.log)
        .into_iter()
        .find(|l| l.starts_with("start"))
        .unwrap();
    assert!(
        start.contains("\"--prompt-suggestions\", \"false\"") && !start.contains("\"true\""),
        "{start}"
    );
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
    let (node, _) = r.open_with(
        None,
        vec!["--model", "opus", "--name", "login", "hola"],
        Some(env),
    );
    // REQ-630: the node says the folder the engine runs in.
    assert_eq!(r.conv(&node)["cwd"], here.to_string_lossy().as_ref());
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

/// REQ-540 to REQ-542: a session that Claude itself wrote is resumed with what was said, and with
/// the mode, the model and the effort it ended with, which the engine is started with.
#[test]
fn resuming_a_session_of_claudes_own_brings_what_was_said_and_the_state_it_ended_with() {
    let mut r = rig();
    let here = std::fs::canonicalize(&r.sb.dir).unwrap();
    let proj = here.join("proj");
    let config = here.join("claude-config");
    let encoded: String = proj
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let dir = config.join("projects").join(encoded);
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        r#"{"type":"permission-mode","permissionMode":"acceptEdits"}"#,
        r#"{"type":"user","message":{"role":"user","content":"arregla el login"}}"#,
        r#"{"type":"assistant","effort":"xhigh","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Lo miro."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a/login.rs"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn login() {}"}]}}"#,
        r#"{"type":"assistant","effort":"xhigh","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Ya está."}]}}"#,
    ];
    std::fs::write(dir.join("old-session.jsonl"), lines.join("\n") + "\n").unwrap();
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
        ("PWD".to_owned(), proj.to_string_lossy().into_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_owned(),
            config.to_string_lossy().into_owned(),
        ),
    ]
    .into();

    let (node, session) = r.open_with(None, vec!["--resume", "old-session"], Some(env));
    assert_eq!(session, "old-session");
    let v = r.conv(&node);
    let kinds: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["t"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["user", "text", "tool", "text"], "{v}");
    assert_eq!(v["items"][0]["text"], "arregla el login");
    assert_eq!(v["items"][2]["state"], "ok");
    // The line that says it was resumed waits for the first message sent (REQ-564).
    assert_eq!(
        (
            v["mode"].as_str(),
            v["model"].as_str(),
            v["effort"].as_str()
        ),
        (Some("acceptEdits"), Some("opus-5.5"), Some("xhigh")),
        "the node shows the state the session ended with: {v}"
    );
    // The engine is started with them when the next message comes.
    r.send(&node, "sigue");
    wait_for("the engine to start", || {
        log_lines(&r.log).iter().any(|l| l.starts_with("start"))
    });
    let start = log_lines(&r.log)
        .into_iter()
        .find(|l| l.starts_with("start"))
        .unwrap();
    for want in [
        "\"--resume\", \"old-session\"",
        "\"--permission-mode\", \"acceptEdits\"",
        "\"--model\", \"claude-opus-5-5\"",
        "\"--effort\", \"xhigh\"",
    ] {
        assert!(start.contains(want), "{want}: {start}");
    }
    r.stop();
}

/// A transcript of Claude's own for the folder of the rig's pane, and the environment that finds it.
fn old_session(r: &Rig, lines: &[&str]) -> (PathBuf, BTreeMap<String, String>) {
    let here = std::fs::canonicalize(&r.sb.dir).unwrap();
    let proj = here.join("proj");
    let config = here.join("claude-config");
    let encoded: String = proj
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let dir = config.join("projects").join(encoded);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("old-session.jsonl");
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
    let env = [
        (
            "PATH".to_owned(),
            format!(
                "{}:{}",
                here.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        ),
        ("FAKE_LOG".to_owned(), r.log.to_string_lossy().into_owned()),
        ("PWD".to_owned(), proj.to_string_lossy().into_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_owned(),
            config.to_string_lossy().into_owned(),
        ),
    ]
    .into();
    (file, env)
}

const FIRST: &str = r#"{"type":"user","timestamp":"2026-10-13T19:00:00.000Z","message":{"role":"user","content":"hola"}}"#;
const REPLY: &str = r#"{"type":"assistant","timestamp":"2026-10-13T19:00:01.000Z","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"buenas"}]}}"#;

/// REQ-567: the node of a resumed conversation says the time of its first item, and `read_older`
/// answers with the items before it, page by page, until the transcript is out; a conversation
/// with no transcript has nothing to read.
#[test]
fn read_older_pages_back_from_the_node_to_the_start_of_the_transcript() {
    let sb = Sandbox::new();
    let mut r = rig_with(sb, Vec::new());
    let mut lines: Vec<String> = Vec::new();
    for i in 0..220u64 {
        let (m, s) = (i / 60, i % 60);
        lines.push(format!(r#"{{"type":"user","timestamp":"2026-10-13T19:{m:02}:{s:02}.000Z","message":{{"role":"user","content":"pregunta {i}"}}}}"#));
        lines.push(format!(r#"{{"type":"assistant","timestamp":"2026-10-13T19:{m:02}:{s:02}.500Z","message":{{"model":"claude-opus-5-5","content":[{{"type":"text","text":"respuesta {i}"}}]}}}}"#));
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (_file, env) = old_session(&r, &refs);
    let (node, _) = r.open_with(None, vec!["--resume", "old-session"], Some(env));
    let v = r.conv(&node);
    assert_eq!(v["items"].as_array().unwrap().len(), 300);
    assert_eq!(v["first"], 140);
    let mut before = v["older"]
        .as_u64()
        .expect("the node says where to ask from");
    let mut older: Vec<Value> = Vec::new();
    for _ in 0..20 {
        let page = r
            .cmd(Cmd::ReadOlder {
                node: node.clone(),
                before,
                count: 100,
            })
            .unwrap()
            .unwrap();
        let items = page["items"].as_array().unwrap().clone();
        assert!(items.len() <= 100);
        older.splice(0..0, items);
        match page["older"].as_u64() {
            Some(at) => {
                assert!(at < before);
                before = at;
            }
            None => break,
        }
    }
    assert_eq!(
        older.len(),
        140,
        "what the node did not hold, and nothing twice"
    );
    assert_eq!(older[0]["text"], "pregunta 0");
    assert_eq!(older[139]["text"], "respuesta 69");
    assert_eq!(v["items"][0]["text"], "pregunta 70");
    r.stop();
}

/// A live process whose command says `claude`, which is killed when the test ends, however it
/// ends: how a `claude` of another terminal looks to the daemon.
struct StrayClaude(std::process::Child);

impl StrayClaude {
    fn start() -> StrayClaude {
        StrayClaude(
            std::process::Command::new("bash")
                .args(["-c", "exec -a claude sleep 60"])
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn end(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for StrayClaude {
    fn drop(&mut self) {
        self.end();
    }
}

/// REQ-575: a session that another `claude` has open is read and not written: the node says who
/// has it, a message is refused while that process is alive, and the node is free again within a
/// moment of its end.
#[test]
fn a_session_that_another_claude_has_open_is_read_and_not_written() {
    let sb = Sandbox::new();
    let mut r = rig_with(sb, Vec::new());
    let (_file, env) = old_session(&r, &[FIRST, REPLY]);
    let mut stray = StrayClaude::start();
    let pid = stray.pid();
    let sessions = std::path::Path::new(&env["CLAUDE_CONFIG_DIR"]).join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        format!(
            r#"{{"pid":{pid},"sessionId":"old-session","cwd":"/otra/carpeta","status":"idle"}}"#
        ),
    )
    .unwrap();
    let (node, _) = r.open_with(None, vec!["--resume", "old-session"], Some(env));
    let v = r.conv(&node);
    assert_eq!(v["held"]["pid"], pid, "{v}");
    assert_eq!(v["held"]["busy"], false);
    let refused = r
        .cmd(Cmd::ConversationSend {
            node: node.clone(),
            text: "hola".into(),
            images: Vec::new(),
            folds: Vec::new(),
            private: false,
        })
        .unwrap_err();
    assert_eq!(refused.code, contract::ErrorCode::Conflict);
    assert!(
        refused.message.contains(&pid.to_string()),
        "{}",
        refused.message
    );
    // it ends: the node is free again and a message goes
    stray.end();
    r.until(&node, "the session to be free", |v| v.get("held").is_none());
    r.cmd(Cmd::ConversationSend {
        node: node.clone(),
        text: "hola".into(),
        images: Vec::new(),
        folds: Vec::new(),
        private: false,
    })
    .unwrap();
    r.stop();
}

/// REQ-545: while the conversation has no engine of its own, what another `claude` writes to the
/// session shows in it.
#[test]
fn a_session_another_claude_is_writing_is_followed_while_the_conversation_has_no_engine() {
    let mut r = rig();
    let (file, env) = old_session(&r, &[FIRST, REPLY]);
    let (node, _) = r.open_with(None, vec!["--resume", "old-session"], Some(env));
    let v = r.conv(&node);
    assert_eq!(items(&v, "text").len(), 1, "{v}");
    let mut open = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write;
    writeln!(open, r#"{{"type":"assistant","timestamp":"2026-10-13T19:00:09.000Z","message":{{"model":"claude-opus-5-5","content":[{{"type":"text","text":"desde otro claude"}}]}}}}"#).unwrap();
    writeln!(
        open,
        r#"{{"type":"permission-mode","permissionMode":"plan"}}"#
    )
    .unwrap();
    let v = r.until(&node, "the line another claude wrote", |v| {
        items(v, "text")
            .iter()
            .any(|t| t["text"] == "desde otro claude")
    });
    // The note of the resume is jwn's own: it was made later than the lines, and stays after them.
    let kinds: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["t"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["user", "text", "text"], "{v}");
    assert_eq!(v["mode"], "plan", "the mode it ends with follows too");
    // REQ-549: a message typed in the other `claude` while it works waits, and stops waiting when it is taken in.
    writeln!(open, r#"{{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-13T19:00:10.000Z","content":"y luego corre los tests"}}"#).unwrap();
    let v = r.until(&node, "the message that waits", |v| {
        v["queue"] == serde_json::json!(["y luego corre los tests"])
    });
    writeln!(open, r#"{{"type":"queue-operation","operation":"remove","timestamp":"2026-10-13T19:00:11.000Z","content":"y luego corre los tests","reason":"absorbed_mid_turn"}}"#).unwrap();
    r.until(&node, "the queue to empty", |v| {
        v["queue"] == serde_json::json!([])
    });
    let _ = v;
    r.stop();
}

/// REQ-544: what is jwn's own is saved beside the transcript, with its time, and the chat is not
/// copied: it is read again from the transcript when the daemon comes back.
#[test]
fn what_is_jwns_own_is_kept_beside_the_transcript_and_the_chat_is_not_copied() {
    let sb = Sandbox::new();
    let mut r = rig_with(sb, Vec::new());
    let (_file, env) = old_session(&r, &[FIRST, REPLY]);
    let (node, session) = r.open_with(None, vec!["--resume", "old-session"], Some(env.clone()));
    r.send(&node, "/cost");
    r.until(&node, "the answer to /cost", |v| {
        !items(v, "out").is_empty()
    });
    let (dir, log) = (r.sb.dir.clone(), r.log.clone());
    let config = r.sb.config();
    r.daemon.take().unwrap().stop();
    std::mem::forget(r.sb);
    let saved = std::fs::read_dir(dir.join("state/conversations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(saved).unwrap()).unwrap();
    assert_eq!(
        saved["items"].as_array().map(Vec::len),
        Some(0),
        "the chat is not copied: {saved}"
    );
    let kept: Vec<&str> = saved["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["item"]["t"].as_str().unwrap())
        .collect();
    assert_eq!(
        kept,
        ["out"],
        "a command is jwn's, and no message was sent yet"
    );

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
    let node2 = c
        .cmd(Cmd::OpenConversation {
            pane: r.pane.clone(),
            resume: Some(session),
            args: Vec::new(),
            env: Some(env),
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
    let kinds: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["t"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["user", "text", "out", "divider"],
        "the lines of the transcript, then what is jwn's: {v}"
    );
    daemon.stop();
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
            folds: Vec::new(),
            private: false,
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
        folds: Vec::new(),
        private: false,
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

/// Whether the process `pid` is there (a zombie that nobody has waited for is not).
fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.trim().starts_with('Z')
}

fn wait_gone(pid: u32, within: Duration, what: &str) {
    let end = Instant::now() + within;
    while alive(pid) {
        assert!(Instant::now() < end, "{what}: process {pid} is still there");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// REQ-638: a daemon that stops does not leave an engine that ignores the end of its input.
#[test]
fn a_daemon_that_stops_leaves_no_engine_that_ignores_the_end_of_its_input() {
    let sb = Sandbox::new();
    let pidfile = sb.dir.join("engine.pid");
    let mut r = rig_with(
        sb,
        vec![
            ("FAKE_STUBBORN".into(), "1".into()),
            (
                "FAKE_PIDFILE".into(),
                pidfile.to_string_lossy().into_owned(),
            ),
        ],
    );
    r.open();
    wait_for("the engine's pid", || pidfile.exists());
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    let group = r.sb.dir.join("state/conversations");
    assert!(
        std::fs::read_dir(&group).unwrap().flatten().any(|f| f
            .file_name()
            .to_string_lossy()
            .ends_with(&format!(".{pid}.pgid"))),
        "its group is written down while it runs"
    );
    r.stop();
    // Not later: the daemon's process may be gone by then, and the engine with nobody to end it.
    wait_gone(pid, Duration::from_secs(1), "as the daemon stopped");
}

/// REQ-638: a daemon that starts ends the engines a daemon before it left, by the groups it wrote
/// down, and only if the number still is an engine.
#[test]
fn a_daemon_that_starts_ends_the_engines_an_earlier_one_left_and_nothing_else() {
    use std::os::unix::process::CommandExt;
    let sb = Sandbox::new();
    let dir = sb.dir.join("state/conversations");
    std::fs::create_dir_all(&dir).unwrap();
    // looks like an engine to `ps`: `claude`, `--input-format`, `stream-json`
    let engine = std::process::Command::new("sh")
        .args([
            "-c",
            "sleep 300; true",
            "claude",
            "--input-format",
            "stream-json",
        ])
        .process_group(0)
        .spawn()
        .unwrap();
    // a number that is somebody else's now
    let other = std::process::Command::new("sleep")
        .arg("300")
        .process_group(0)
        .spawn()
        .unwrap();
    let (engine_pid, other_pid) = (engine.id(), other.id());
    std::fs::write(
        dir.join(format!("p1.{engine_pid}.pgid")),
        engine_pid.to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("p2.{other_pid}.pgid")),
        other_pid.to_string(),
    )
    .unwrap();
    let mut engine = engine;
    let mut other = other;
    let d = Daemon::start(sb.config()).unwrap();
    wait_for("the leftover engine to end", || {
        engine.try_wait().unwrap().is_some()
    });
    assert!(
        other.try_wait().unwrap().is_none(),
        "what is not an engine is left alone"
    );
    assert!(
        std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|f| f.path().extension().is_none_or(|e| e != "pgid")),
        "the notes are gone"
    );
    let _ = other.kill();
    let _ = other.wait();
    d.stop();
}
