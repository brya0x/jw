//! N173a: conversations of their own (REQ-805 to REQ-817), against the fake engine of the
//! conversation tests. Each test has its own state directory, so no chat is made in the person's.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Sandbox, WAIT};
use contract::{Cmd, ErrorCode};
use daemon::{Config, Daemon};
use serde_json::Value;
use wire::Client;

const ENGINE: &str = include_str!("fake-claude.py");
const NEW: &str = "conversación nueva";

struct Rig {
    sb: Sandbox,
    daemon: Option<Daemon>,
    client: Client,
    config: Config,
    session: String,
}

fn rig() -> Rig {
    let sb = Sandbox::new();
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("claude");
    std::fs::write(&engine, ENGINE).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = sb.config();
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
        (
            "FAKE_PIDFILE".into(),
            sb.dir.join("engine.pid").to_string_lossy().into_owned(),
        ),
    ];
    let daemon = Daemon::start(config.clone()).unwrap();
    let client = Client::connect(&config.socket, "test").unwrap();
    let mut rig = Rig {
        daemon: Some(daemon),
        session: client.root.clone(),
        client,
        config,
        sb,
    };
    rig.open_session();
    rig
}

impl Rig {
    fn start(&mut self) {
        self.daemon = Some(Daemon::start(self.config.clone()).unwrap());
        self.client = Client::connect(&self.config.socket, "test").unwrap();
        self.session = self.client.root.clone();
        self.open_session();
    }

    fn open_session(&mut self) {
        let s = self.session.clone();
        self.client.open(&s).unwrap();
        self.wait("the session", |c| c.nodes.contains_key(&s));
    }

    fn stop(&mut self) {
        self.daemon.take().unwrap().stop();
    }

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

    fn cmd(&mut self, cmd: Cmd) -> Result<Option<Value>, contract::ErrorBody> {
        self.client.cmd(cmd).unwrap()
    }

    fn ok(&mut self, cmd: Cmd) -> Value {
        self.cmd(cmd).unwrap().unwrap_or(Value::Null)
    }

    /// The summaries of the session, in the order it lists them.
    fn list(&mut self) -> Vec<Value> {
        let _ = self.client.poll(Duration::from_millis(5));
        let s = self.session.clone();
        self.client.nodes[&s]["workspaces"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn summary(&mut self, id: &str) -> Option<Value> {
        self.list().into_iter().find(|w| w["id"] == id)
    }

    fn until_summary(&mut self, what: &str, id: &str, f: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + WAIT;
        loop {
            if let Some(w) = self.summary(id)
                && f(&w)
            {
                return w;
            }
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}: {:?}",
                self.summary(id)
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn new_chat(&mut self) -> String {
        let id = self.ok(Cmd::ChatNew {})["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        let at = id.clone();
        self.until_summary("the chat", &at, |w| w["pane_count"] == 1);
        id
    }

    fn pane_of(&mut self, ws: &str) -> String {
        self.client.open(ws).unwrap();
        self.wait("the workspace node", |c| c.nodes.contains_key(ws));
        self.client.nodes[ws]["panes"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Open the conversation of the chat's panel (the shell integration does it for a `claude` typed).
    fn conversation(&mut self, ws: &str) -> String {
        let pane = self.pane_of(ws);
        let r = self.ok(Cmd::OpenConversation {
            pane,
            resume: None,
            args: Vec::new(),
            env: None,
        });
        let node = r["node"].as_str().unwrap().to_owned();
        self.client.open(&node).unwrap();
        let n = node.clone();
        self.wait("the conversation", |c| c.nodes.contains_key(&n));
        node
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

    /// Wait until `n` lines of the engine's log that say a start match.
    fn log_count(&mut self, what: &str, n: usize, f: impl Fn(&str) -> bool) {
        let end = Instant::now() + WAIT;
        loop {
            let log = std::fs::read_to_string(self.sb.dir.join("engine.log")).unwrap_or_default();
            if log
                .lines()
                .filter(|l| l.starts_with("start") && f(l))
                .count()
                >= n
            {
                return;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}: {log}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn log_has(&mut self, what: &str, f: impl Fn(&str) -> bool) {
        self.log_count(what, 1, f);
    }

    fn chats_dir(&self) -> PathBuf {
        identity::chats_dir(&self.config.state_dir)
    }

    fn saved(&self) -> Value {
        let text = std::fs::read_to_string(self.config.state_dir.join("session.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

fn code(r: Result<Option<Value>, contract::ErrorBody>) -> ErrorCode {
    r.unwrap_err().code
}

fn gone(pid: &str) -> bool {
    !std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

#[test]
fn a_new_chat_has_an_empty_folder_of_its_own_and_one_panel() {
    let mut r = rig();
    let id = r.new_chat();
    let w = r.summary(&id).unwrap();
    assert_eq!(w["name"], NEW);
    assert_eq!(w["kind"], "folder");
    assert!(w.get("project").is_none(), "{w}");
    assert!(w["chat"]["active"].as_u64().is_some());
    assert!(w["chat"].get("archived").is_none() && w["chat"].get("last").is_none());
    let dir = r.chats_dir().join(&id);
    assert_eq!(w["path"], dir.to_string_lossy().as_ref());
    assert!(dir.is_dir());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn the_first_message_names_the_chat_and_a_reply_is_its_last_line() {
    let mut r = rig();
    let id = r.new_chat();
    let node = r.conversation(&id);
    // the engine's own `/commands` are not something said: they name nothing
    r.say(&node, "/compact");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.summary(&id).unwrap()["name"], NEW);
    r.say(&node, "  una   cosa\n larga  ");
    let w = r.until_summary("the name", &id, |w| w["name"] != NEW);
    assert_eq!(w["name"], "una cosa larga");
    // the panel's engine starts in the chat's folder with mode `manual` (REQ-807)
    r.log_has("an engine in manual mode", |l| {
        l.contains("--permission-mode\", \"manual") && l.contains("/chats/")
    });
    let w = r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    assert!(w["chat"]["active"].as_u64().unwrap() > 0);
    // a later message does not name it again
    r.say(&node, "otra");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.summary(&id).unwrap()["name"], "una cosa larga");
    // a long first message is cut at 40 characters, with an ellipsis
    let id2 = r.new_chat();
    let node2 = r.conversation(&id2);
    r.say(&node2, &"x".repeat(60));
    let w = r.until_summary("the long name", &id2, |w| w["name"] != NEW);
    assert_eq!(w["name"].as_str().unwrap().chars().count(), 40);
    assert!(w["name"].as_str().unwrap().ends_with('…'));
}

#[test]
fn a_rename_before_the_first_message_is_kept() {
    let mut r = rig();
    let id = r.new_chat();
    r.ok(Cmd::RenameWorkspace {
        workspace: id.clone(),
        name: "mi charla".into(),
    });
    let node = r.conversation(&id);
    r.say(&node, "hola");
    r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    assert_eq!(r.summary(&id).unwrap()["name"], "mi charla");
    // and an empty name brings back «conversación nueva»
    r.ok(Cmd::RenameWorkspace {
        workspace: id.clone(),
        name: "".into(),
    });
    assert_eq!(
        r.until_summary("the name", &id, |w| w["name"] == NEW)["name"],
        NEW
    );
}

#[test]
fn what_a_chat_cannot_do_is_refused() {
    let mut r = rig();
    let id = r.new_chat();
    let path = r.summary(&id).unwrap()["path"].as_str().unwrap().to_owned();
    for cmd in [
        Cmd::OpenDiff {
            workspace: id.clone(),
            scope: contract::DiffScope::Uncommitted,
            commit: None,
        },
        Cmd::SyncWorkspace {
            workspace: id.clone(),
        },
        Cmd::StartWorkspace {
            workspace: id.clone(),
            open: None,
        },
        Cmd::MoveToGroup {
            workspace: id.clone(),
            group: None,
        },
        Cmd::MoveRepo {
            workspace: id.clone(),
            delta: 1,
        },
        Cmd::RemoveWorkspace {
            workspace: id.clone(),
            remove_worktree: false,
            force: false,
        },
        Cmd::CreateWorktree {
            repo: path.clone(),
            branch: "b".into(),
            base: None,
            folder: None,
        },
        Cmd::OpenWorktree {
            repo: path.clone(),
            path: "/x".into(),
        },
    ] {
        assert_eq!(code(r.cmd(cmd)), ErrorCode::BadRequest);
    }
    // nothing was removed
    assert!(r.summary(&id).is_some());
    // and a workspace that is no chat is not a chat for these commands
    let folder = r.sb.dir.join("proj");
    std::fs::create_dir_all(&folder).unwrap();
    let ws = r.ok(Cmd::OpenFolder {
        path: folder.to_string_lossy().into(),
    })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    for cmd in [
        Cmd::ArchiveChat {
            workspace: ws.clone(),
        },
        Cmd::ResumeChat {
            workspace: ws.clone(),
        },
        Cmd::DeleteChat {
            workspace: ws.clone(),
        },
    ] {
        assert_eq!(code(r.cmd(cmd)), ErrorCode::BadRequest);
    }
    assert!(r.summary(&ws).is_some());
}

#[test]
fn chats_come_last_live_ones_by_activity_then_the_archived() {
    let mut r = rig();
    let folder = r.sb.dir.join("proj");
    std::fs::create_dir_all(&folder).unwrap();
    let ws = r.ok(Cmd::OpenFolder {
        path: folder.to_string_lossy().into(),
    })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let a = r.new_chat();
    std::thread::sleep(Duration::from_millis(1100));
    let b = r.new_chat();
    let ids = |r: &mut Rig| -> Vec<String> {
        r.list()
            .iter()
            .map(|w| w["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&mut r), [ws.clone(), b.clone(), a.clone()]);
    // a message moves a chat up
    std::thread::sleep(Duration::from_millis(1100));
    let node = r.conversation(&a);
    r.say(&node, "hola");
    r.until_summary("the reply", &a, |w| w["chat"]["last"] == "Voy a mirar.");
    assert_eq!(ids(&mut r), [ws.clone(), a.clone(), b.clone()]);
    // an archived chat goes after the live ones
    r.ok(Cmd::ArchiveChat {
        workspace: a.clone(),
    });
    r.until_summary("archived", &a, |w| w["chat"]["archived"] == true);
    assert_eq!(ids(&mut r), [ws, b, a]);
}

#[test]
fn an_empty_chat_is_discarded_when_it_is_archived() {
    let mut r = rig();
    let id = r.new_chat();
    let dir = r.chats_dir().join(&id);
    let reply = r.ok(Cmd::ArchiveChat {
        workspace: id.clone(),
    });
    assert_eq!(reply["discarded"], true);
    r.wait("the chat to go", |c| {
        !c.nodes[&c
            .nodes
            .keys()
            .find(|k| c.nodes[*k]["type"] == "session")
            .unwrap()
            .clone()]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == id.as_str())
    });
    assert!(!exists(&dir));
}

#[test]
fn archiving_keeps_the_folder_and_the_session_and_resuming_opens_it_again() {
    let mut r = rig();
    let id = r.new_chat();
    let node = r.conversation(&id);
    r.say(&node, "hola");
    r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    let pid = std::fs::read_to_string(r.sb.dir.join("engine.pid")).unwrap();
    let dir = r.chats_dir().join(&id);
    assert_eq!(
        r.ok(Cmd::ArchiveChat {
            workspace: id.clone()
        }),
        Value::Null
    );
    let w = r.until_summary("archived", &id, |w| w["chat"]["archived"] == true);
    assert_eq!(w["pane_count"], 0);
    assert!(dir.is_dir());
    // the engine of the panel ended with it
    let end = Instant::now() + WAIT;
    while !gone(&pid) {
        assert!(Instant::now() < end, "the engine {pid} is still running");
        std::thread::sleep(Duration::from_millis(50));
    }
    // the saved session says which conversation it was
    let saved = r.saved();
    let ws = saved["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id.as_str())
        .unwrap()
        .clone();
    let session = ws["chat"]["session"]
        .as_str()
        .expect("a session id is kept")
        .to_owned();
    assert_eq!(ws["chat"]["archived"], true);
    // resuming opens a panel on that session, in the same folder
    let pane = r.ok(Cmd::ResumeChat {
        workspace: id.clone(),
    })["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    let w = r.until_summary("resumed", &id, |w| w["pane_count"] == 1);
    assert!(w["chat"].get("archived").is_none(), "{w}");
    let cwd = format!("cwd={}", std::fs::canonicalize(&dir).unwrap().display());
    r.log_has("an engine started on the session", |l| {
        l.contains("--resume") && l.contains(&session) && l.contains(&cwd)
    });
    assert!(!pane.is_empty());
    // a chat that is not archived cannot be resumed
    assert_eq!(
        code(r.cmd(Cmd::ResumeChat { workspace: id })),
        ErrorCode::BadRequest
    );
}

#[test]
fn deleting_removes_the_folder_and_the_chat_and_only_inside_the_chats() {
    let mut r = rig();
    let id = r.new_chat();
    let node = r.conversation(&id);
    r.say(&node, "hola");
    r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    r.ok(Cmd::ArchiveChat {
        workspace: id.clone(),
    });
    r.until_summary("archived", &id, |w| w["chat"]["archived"] == true);
    // an archived chat takes no panel: `resume_chat` is what opens one
    assert_eq!(
        code(r.cmd(Cmd::NewPane {
            workspace: id.clone(),
            program: None,
            beside: None,
            dir: None,
        })),
        ErrorCode::BadRequest
    );
    let dir = r.chats_dir().join(&id);
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    r.ok(Cmd::DeleteChat {
        workspace: id.clone(),
    });
    r.wait("the chat to go", |_| true);
    let end = Instant::now() + WAIT;
    while r.summary(&id).is_some() {
        assert!(Instant::now() < end, "the chat is still listed");
    }
    assert!(!exists(&dir));

    // a record whose folder points outside the chats removes nothing
    let other = r.new_chat();
    let outside = r.sb.dir.join("precious");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), "x").unwrap();
    let node = r.conversation(&other);
    r.say(&node, "hola");
    r.until_summary("the reply", &other, |w| w["chat"]["last"] == "Voy a mirar.");
    r.ok(Cmd::ArchiveChat {
        workspace: other.clone(),
    });
    r.until_summary("archived", &other, |w| w["chat"]["archived"] == true);
    r.stop();
    let file = r.config.state_dir.join("session.json");
    let mut saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for w in saved["workspaces"].as_array_mut().unwrap() {
        if w["id"] == other.as_str() {
            w["path"] = outside.to_string_lossy().as_ref().into();
        }
    }
    std::fs::write(&file, saved.to_string()).unwrap();
    r.start();
    assert_eq!(
        code(r.cmd(Cmd::DeleteChat {
            workspace: other.clone()
        })),
        ErrorCode::BadRequest
    );
    assert!(outside.join("keep").exists());
    assert!(r.summary(&other).is_some());
}

#[test]
fn a_restart_keeps_the_chats_as_they_were() {
    let mut r = rig();
    let live = r.new_chat();
    let said = r.new_chat();
    let node = r.conversation(&said);
    r.say(&node, "hola");
    r.until_summary("the reply", &said, |w| w["chat"]["last"] == "Voy a mirar.");
    r.ok(Cmd::ArchiveChat {
        workspace: said.clone(),
    });
    r.until_summary("archived", &said, |w| w["chat"]["archived"] == true);
    r.stop();
    r.start();
    let w = r.summary(&said).expect("the archived chat is back");
    assert_eq!(w["name"], "hola");
    assert_eq!(w["chat"]["archived"], true);
    assert_eq!(w["chat"]["last"], "Voy a mirar.");
    assert_eq!(w["pane_count"], 0);
    // the live chat that said nothing comes back with a panel, a conversation and not a shell
    let w = r.until_summary("the panel", &live, |w| w["pane_count"] == 1);
    assert_eq!(w["name"], NEW);
    let live_dir = r.chats_dir().join(&live);
    let at = format!(
        "cwd={}",
        std::fs::canonicalize(&live_dir).unwrap().display()
    );
    r.log_count("the panel started again as `claude`", 2, |l| {
        l.contains(&at)
    });
}

#[test]
fn taking_a_chat_to_a_project_starts_a_conversation_there_and_leaves_the_chat() {
    let mut r = rig();
    let folder = r.sb.dir.join("proj");
    std::fs::create_dir_all(&folder).unwrap();
    let ws = r.ok(Cmd::OpenFolder {
        path: folder.to_string_lossy().into(),
    })["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = r.new_chat();
    // an empty chat has nothing to take
    assert_eq!(
        code(r.cmd(Cmd::PromoteChat {
            workspace: id.clone(),
            to: ws.clone(),
            summary: None
        })),
        ErrorCode::BadRequest
    );
    let node = r.conversation(&id);
    r.say(&node, "cuántos bytes tiene un kilobyte");
    r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    let reply = r.ok(Cmd::PromoteChat {
        workspace: id.clone(),
        to: ws.clone(),
        summary: None,
    });
    let text = reply["text"].as_str().unwrap();
    assert!(
        text.contains("tú: cuántos bytes tiene un kilobyte"),
        "{text}"
    );
    assert!(text.contains("claude: Voy a mirar."), "{text}");
    assert!(reply["pane"].as_str().is_some());
    r.until_summary("the pane", &ws, |w| w["pane_count"] == 1);
    let w = r.summary(&id).unwrap();
    assert_eq!(w["pane_count"], 1);
    assert_eq!(w["name"], "cuántos bytes tiene un kilobyte");
    // without a summary the text is empty
    let reply = r.ok(Cmd::PromoteChat {
        workspace: id.clone(),
        to: ws.clone(),
        summary: Some(false),
    });
    assert_eq!(reply["text"], "");
    // a chat is not taken to another chat, and an archived one has to be resumed first
    let other = r.new_chat();
    assert_eq!(
        code(r.cmd(Cmd::PromoteChat {
            workspace: id.clone(),
            to: other,
            summary: None
        })),
        ErrorCode::BadRequest
    );
    r.ok(Cmd::ArchiveChat {
        workspace: id.clone(),
    });
    r.until_summary("archived", &id, |w| w["chat"]["archived"] == true);
    assert_eq!(
        code(r.cmd(Cmd::PromoteChat {
            workspace: id,
            to: ws,
            summary: None
        })),
        ErrorCode::BadRequest
    );
}

#[test]
fn a_chat_whose_conversation_has_a_transcript_is_kept_when_archived_and_not_discarded() {
    let mut r = rig();
    let id = r.new_chat();
    // the daemon never saw a message (the conversation ran apart), but Claude has the session
    let dir = r.chats_dir().join(&id);
    let end = Instant::now() + WAIT;
    let session = loop {
        let pane = r.saved()["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id.as_str())
            .and_then(|w| w["panes"][0]["resume"][2].as_str().map(str::to_owned));
        if let Some(s) = pane {
            break s;
        }
        assert!(Instant::now() < end, "the panel has no session id");
        std::thread::sleep(Duration::from_millis(30));
    };
    let projects = r.sb.dir.join("claude").join("projects").join("x");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join(format!("{session}.jsonl")), "{}\n").unwrap();
    assert_eq!(
        r.ok(Cmd::ArchiveChat {
            workspace: id.clone()
        }),
        Value::Null
    );
    let w = r.until_summary("archived", &id, |w| w["chat"]["archived"] == true);
    assert_eq!(w["pane_count"], 0);
    assert!(dir.is_dir());
}

#[test]
fn a_turn_with_no_text_does_not_bring_back_an_older_reply_as_the_last_line() {
    let mut r = rig();
    let id = r.new_chat();
    let node = r.conversation(&id);
    r.say(&node, "hola");
    r.until_summary("the first reply", &id, |w| {
        w["chat"]["last"] == "Voy a mirar."
    });
    // the engine answers this one with a tool and no text
    r.say(&node, "push");
    r.until_summary("the turn", &id, |w| w["chat"]["last"] == "push");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.summary(&id).unwrap()["chat"]["last"], "push");
}

#[test]
fn closing_the_last_panel_of_a_chat_archives_it_with_its_session_and_an_empty_one_goes() {
    let mut r = rig();
    let id = r.new_chat();
    let node = r.conversation(&id);
    r.say(&node, "hola");
    r.until_summary("the reply", &id, |w| w["chat"]["last"] == "Voy a mirar.");
    let dir = r.chats_dir().join(&id);
    let pane = r.pane_of(&id);
    r.ok(Cmd::ClosePane { pane });
    // not a chat without a panel: it is archived, with the id of the conversation it had
    let w = r.until_summary("archived", &id, |w| w["chat"]["archived"] == true);
    assert_eq!(w["pane_count"], 0);
    assert!(dir.is_dir());
    let saved = r.saved();
    let session = saved["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id.as_str())
        .and_then(|w| w["chat"]["session"].as_str().map(str::to_owned))
        .expect("the session id was kept");
    r.ok(Cmd::ResumeChat {
        workspace: id.clone(),
    });
    r.log_has("an engine on the session", |l| {
        l.contains("--resume") && l.contains(&session)
    });
    // a chat that said nothing goes with its last panel
    let empty = r.new_chat();
    let dir = r.chats_dir().join(&empty);
    let pane = r.pane_of(&empty);
    r.ok(Cmd::ClosePane { pane });
    let end = Instant::now() + WAIT;
    while r.summary(&empty).is_some() {
        assert!(Instant::now() < end, "the empty chat is still listed");
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(!exists(&dir));
}
