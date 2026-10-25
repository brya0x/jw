//! N180b: a workspace's task list is read from its file, summed up in the workspace's row, served
//! as the `tasks` node and changed by `tasks_edit` (REQ-907 to REQ-913, REQ-956), in a private daemon.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::Sandbox;
use contract::tasks::TaskOp;
use contract::{Cmd, ErrorCode};
use serde_json::Value;
use wire::Client;

const LIST: &str =
    "# plan\n\n- [x] t1 schema\n- [ ] t2 endpoints (after: t1)\n- [ ] t3 docs (after: t2)\n";

fn folder(sb: &Sandbox, name: &str, list: Option<&str>) -> PathBuf {
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(text) = list {
        let path = identity::tasks_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn open_folder(c: &mut Client, dir: &Path) -> String {
    c.cmd(Cmd::OpenFolder {
        path: dir.to_string_lossy().into(),
    })
    .unwrap()
    .unwrap()
    .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn summary(c: &mut Client, ws: &str) -> Value {
    let _ = c.poll(Duration::from_millis(5));
    c.nodes[&c.root.clone()]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == ws)
        .cloned()
        .unwrap()
}

fn until(c: &mut Client, what: &str, f: impl Fn(&Client) -> bool) {
    assert!(
        c.wait_until(Duration::from_secs(8), f).unwrap(),
        "timed out waiting for {what}"
    );
}

fn open_session(c: &mut Client) {
    let root = c.root.clone();
    c.open(&root).unwrap();
    until(c, "the session", |c| c.nodes.contains_key(&c.root));
}

fn node_of(c: &mut Client, ws: &str) -> String {
    let id = c
        .cmd(Cmd::OpenTasks {
            workspace: ws.into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["node"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&id).unwrap();
    until(c, "the tasks node", |c| c.nodes.contains_key(&id));
    id
}

fn states(c: &Client, node: &str) -> Vec<String> {
    c.nodes[node]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            format!(
                "{}:{}",
                t["id"].as_str().unwrap(),
                t["state"].as_str().unwrap()
            )
        })
        .collect()
}

fn edit(
    c: &mut Client,
    ws: &str,
    op: TaskOp,
    id: &str,
    text: Option<&str>,
) -> Result<Option<Value>, ErrorCode> {
    c.cmd(Cmd::TasksEdit {
        workspace: ws.into(),
        op,
        id: id.into(),
        text: text.map(Into::into),
    })
    .unwrap()
    .map_err(|e| e.code)
}

#[test]
fn a_workspace_with_no_list_has_no_summary_and_no_node() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let ws = open_folder(&mut c, &folder(&sb, "plain", None));
    assert!(summary(&mut c, &ws).get("tasks").is_none());
    let err = c
        .cmd(Cmd::OpenTasks {
            workspace: ws.clone(),
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Done, "t1", None),
        Err(ErrorCode::NotFound)
    );
    // a folder with a file that has no task line is not a list either
    let other = folder(&sb, "prose", Some("# nothing to do here\n\njust prose\n"));
    let ws2 = open_folder(&mut c, &other);
    assert_eq!(
        c.cmd(Cmd::OpenTasks { workspace: ws2 })
            .unwrap()
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn a_list_is_in_the_row_with_its_final_task_counted_and_in_the_node() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(&sb, "app", Some(LIST));
    let ws = open_folder(&mut c, &dir);
    let node = node_of(&mut c, &ws);
    // 3 tasks of the file and the final one, which is not in the file yet
    let row = summary(&mut c, &ws);
    assert_eq!(row["tasks"]["done"], 1);
    assert_eq!(row["tasks"]["total"], 4);
    assert_eq!(
        states(&c, &node),
        ["t1:done", "t2:ready", "t3:waiting", "fin:waiting"]
    );
    let fin = &c.nodes[&node]["tasks"][3];
    assert_eq!(fin["final"], true);
    assert_eq!(fin["auto"], true);
    assert_eq!(fin["level"], 4);
    assert_eq!(fin["after"], serde_json::json!(["*"]));
    assert_eq!(
        c.nodes[&node]["loop"],
        serde_json::json!({"on": false, "laps": 0, "max": 20})
    );
    assert_eq!(
        c.nodes[&node]["path"].as_str().unwrap(),
        identity::tasks_path(&dir).to_str().unwrap()
    );
    // looking at a list never writes the final task into the file
    assert_eq!(
        std::fs::read_to_string(identity::tasks_path(&dir)).unwrap(),
        LIST
    );
}

#[test]
fn a_list_that_is_there_when_the_workspace_opens_shows_without_a_command() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let ws = open_folder(&mut c, &folder(&sb, "app", Some(LIST)));
    until(&mut c, "the row to say it has a list", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws.as_str() && w["tasks"]["total"] == 4)
    });
}

#[test]
fn a_change_of_the_file_by_hand_reaches_the_open_node_and_the_row() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(&sb, "app", Some(LIST));
    let ws = open_folder(&mut c, &dir);
    let node = node_of(&mut c, &ws);
    let path = identity::tasks_path(&dir);
    std::fs::write(
        &path,
        LIST.replace("- [ ] t2", "- [x] t2")
            .replace("t3 docs (after: t2)", "t3 docs (after: t2, t8)"),
    )
    .unwrap();
    until(&mut c, "the patch", |c| states(c, &node)[1] == "t2:done");
    assert_eq!(states(&c, &node)[2], "t3:waiting");
    assert_eq!(c.nodes[&node]["problems"][0]["line"], 5);
    until(&mut c, "the row", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws.as_str() && w["tasks"]["done"] == 2)
    });
    // when the file goes, so does the node
    std::fs::remove_file(&path).unwrap();
    until(&mut c, "the node to go", |c| !c.nodes.contains_key(&node));
    assert!(summary(&mut c, &ws).get("tasks").is_none());
}

#[test]
fn tasks_edit_changes_the_file_and_the_node_at_once() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(&sb, "app", Some(LIST));
    let ws = open_folder(&mut c, &dir);
    let node = node_of(&mut c, &ws);
    let path = identity::tasks_path(&dir);
    edit(&mut c, &ws, TaskOp::Done, "t2", None).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("- [x] t2 endpoints (after: t1)")
    );
    // no wait for the 2 s poll: the reply came after the node was patched
    until(&mut c, "the patch", |c| states(c, &node)[1] == "t2:done");
    edit(
        &mut c,
        &ws,
        TaskOp::Block,
        "t3",
        Some("needs the key; ask (now)"),
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("- [!] t3 docs (after: t2; blocked: needs the key, ask [now])"),
        "{text}"
    );
    until(&mut c, "blocked", |c| states(c, &node)[2] == "t3:blocked");
    assert_eq!(
        c.nodes[&node]["tasks"][2]["why"],
        "needs the key, ask [now]"
    );
    edit(
        &mut c,
        &ws,
        TaskOp::Note,
        "t1",
        Some("CI #58 green (4 min)"),
    )
    .unwrap();
    until(&mut c, "the note", |c| {
        c.nodes[&node]["tasks"][0]["notes"][0] == "CI #58 green (4 min)"
    });
    edit(&mut c, &ws, TaskOp::Reopen, "t3", None).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("blocked:"));
}

#[test]
fn tasks_edit_answers_with_the_codes_the_spec_gives() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(
        &sb,
        "app",
        Some("- [ ] a one\n- [ ] a two\n- [ ] b three\n"),
    );
    let ws = open_folder(&mut c, &dir);
    let before = std::fs::read_to_string(identity::tasks_path(&dir)).unwrap();
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Done, "zz", None),
        Err(ErrorCode::NotFound)
    );
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Done, "a", None),
        Err(ErrorCode::Conflict)
    );
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Block, "b", Some("  ")),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Block, "b", None),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        edit(&mut c, &ws, TaskOp::Note, "b", Some("\n")),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        edit(&mut c, "no-such-workspace", TaskOp::Done, "a", None),
        Err(ErrorCode::NotFound)
    );
    assert_eq!(
        std::fs::read_to_string(identity::tasks_path(&dir)).unwrap(),
        before
    );
}

#[test]
fn a_list_an_agent_writes_later_shows_when_its_pane_moves() {
    let sb = Sandbox::new();
    let _d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(&sb, "app", None);
    let ws = open_folder(&mut c, &dir);
    let pane = c
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
    assert!(summary(&mut c, &ws).get("tasks").is_none());
    let path = identity::tasks_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, LIST).unwrap();
    c.cmd(Cmd::AgentEvent {
        pane,
        event: contract::AgentEvent::Stop,
        notification_type: None,
        tool: None,
    })
    .unwrap()
    .unwrap();
    until(&mut c, "the row", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws.as_str() && w["tasks"]["total"] == 4)
    });
}

// ---- the loop (N180d): REQ-915 to REQ-929

const FLOW: &str = "- [ ] t1 first\n- [ ] t2 second (after: t1)\n";

fn new_pane(c: &mut Client, ws: &str) -> String {
    c.cmd(Cmd::NewPane {
        workspace: ws.into(),
        program: None,
        beside: None,
        dir: None,
    })
    .unwrap()
    .unwrap()
    .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn loop_on(
    c: &mut Client,
    ws: &str,
    max: Option<u32>,
    pane: Option<&str>,
) -> Result<Value, ErrorCode> {
    c.cmd(Cmd::TasksLoopOn {
        workspace: ws.into(),
        max,
        pane: pane.map(Into::into),
    })
    .unwrap()
    .map(|v| v.unwrap())
    .map_err(|e| e.code)
}

/// What the agent's `stop` hook gets back: the reason to go on, or nothing.
fn stop(c: &mut Client, pane: &str) -> Option<String> {
    let reply = c
        .cmd(Cmd::AgentEvent {
            pane: pane.into(),
            event: contract::AgentEvent::Stop,
            notification_type: None,
            tool: None,
        })
        .unwrap()
        .unwrap();
    reply.and_then(|v| v["block"]["reason"].as_str().map(str::to_owned))
}

fn attention(c: &mut Client, ws: &str) -> Vec<String> {
    summary(c, ws)["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn until_row(c: &mut Client, ws: &str, what: &str, f: impl Fn(&Value) -> bool) {
    let ws = ws.to_owned();
    until(c, what, move |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws.as_str() && f(w))
    });
}

struct Run {
    sb: Sandbox,
    _d: daemon::Daemon,
    c: Client,
    dir: PathBuf,
    ws: String,
    pane: String,
}

fn run(list: &str) -> Run {
    let sb = Sandbox::new();
    let d = sb.start();
    let mut c = sb.client();
    open_session(&mut c);
    let dir = folder(&sb, "app", Some(list));
    let ws = open_folder(&mut c, &dir);
    let pane = new_pane(&mut c, &ws);
    Run {
        sb,
        _d: d,
        c,
        dir,
        ws,
        pane,
    }
}

#[test]
fn a_loop_hands_out_a_task_at_each_stop_until_the_last_is_done() {
    let mut r = run(FLOW);
    let on = loop_on(&mut r.c, &r.ws, Some(5), Some(&r.pane.clone())).unwrap();
    assert_eq!(on["laps"], 1);
    assert_eq!(on["max"], 5);
    assert_eq!(on["task"]["id"], "t1");
    assert!(
        on["task"]["text"]
            .as_str()
            .unwrap()
            .contains("tasks done t1")
    );
    // the final task was written to the file
    assert!(
        std::fs::read_to_string(identity::tasks_path(&r.dir))
            .unwrap()
            .contains("- [ ] fin tests:")
    );
    let node = node_of(&mut r.c, &r.ws);
    assert_eq!(r.c.nodes[&node]["loop"]["on"], true);
    assert_eq!(r.c.nodes[&node]["loop"]["task"], "t1");
    assert_eq!(states(&r.c, &node)[0], "t1:running");
    until_row(&mut r.c, &r.ws, "the row to say the loop is on", |w| {
        w["tasks"]["on"] == true
    });
    let (ws, pane, node2) = (r.ws.clone(), r.pane.clone(), node.clone());
    edit(&mut r.c, &ws, TaskOp::Done, "t1", None).unwrap();
    // the agent ends its turn: the next task comes back as the reason to go on, and no `agent_done` rises
    let reason = stop(&mut r.c, &pane).expect("a lap");
    assert!(
        reason.contains("t2 second (after: t1)") && reason.contains("end your turn"),
        "{reason}"
    );
    assert!(attention(&mut r.c, &ws).is_empty());
    until(&mut r.c, "lap 2 in the node", |c| {
        c.nodes[&node2]["loop"]["laps"] == 2 && c.nodes[&node2]["loop"]["task"] == "t2"
    });
    edit(&mut r.c, &ws, TaskOp::Done, "t2", None).unwrap();
    let reason = stop(&mut r.c, &pane).expect("the final task");
    assert!(reason.starts_with("Last task of "), "{reason}");
    assert!(attention(&mut r.c, &ws).is_empty());
    edit(
        &mut r.c,
        &ws,
        TaskOp::Note,
        "fin",
        Some("cargo test: 214 ok"),
    )
    .unwrap();
    edit(&mut r.c, &ws, TaskOp::Done, "fin", None).unwrap();
    // everything is done: nothing to go on with, the loop is off and the person is told it ended
    assert_eq!(stop(&mut r.c, &pane), None);
    until_row(&mut r.c, &ws, "the end", |w| {
        w["tasks"]["on"].is_null() && w["tasks"]["stop"]["kind"] == "done"
    });
    assert_eq!(
        summary(&mut r.c, &ws)["tasks"]["stop"]["text"],
        "all 3 tasks done in 3 laps"
    );
    assert_eq!(attention(&mut r.c, &ws), ["agent_done"]);
    let _ = &r.sb;
}

#[test]
fn a_blocked_task_stops_the_loop_with_a_question_that_ends_when_it_is_unblocked() {
    let mut r = run(FLOW);
    let (ws, pane) = (r.ws.clone(), r.pane.clone());
    loop_on(&mut r.c, &ws, None, Some(&pane)).unwrap();
    edit(&mut r.c, &ws, TaskOp::Block, "t1", Some("needs the key")).unwrap();
    assert_eq!(stop(&mut r.c, &pane), None);
    until_row(&mut r.c, &ws, "the stop", |w| {
        w["tasks"]["stop"]["kind"] == "blocked"
    });
    assert_eq!(
        summary(&mut r.c, &ws)["tasks"]["stop"]["text"],
        "t1 blocked: needs the key"
    );
    assert_eq!(
        attention(&mut r.c, &ws),
        ["question"],
        "a question, and not agent_done"
    );
    // the person unblocks it in the file: the question goes with its cause
    edit(&mut r.c, &ws, TaskOp::Reopen, "t1", None).unwrap();
    until(&mut r.c, "the question to end", |c| {
        c.nodes[&c.root]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == ws.as_str() && w["attention"].as_array().unwrap().is_empty())
    });
    // and the loop turned on again starts anew
    let on = loop_on(&mut r.c, &ws, None, Some(&pane)).unwrap();
    assert_eq!(on["laps"], 1);
    until_row(&mut r.c, &ws, "on", |w| {
        w["tasks"]["on"] == true && w["tasks"].get("stop").is_none()
    });
}

#[test]
fn the_laps_and_a_task_that_never_gets_done_end_the_loop_with_a_question() {
    let mut r = run(FLOW);
    let (ws, pane) = (r.ws.clone(), r.pane.clone());
    loop_on(&mut r.c, &ws, Some(1), Some(&pane)).unwrap();
    assert_eq!(stop(&mut r.c, &pane), None, "lap 1 of 1 was the last");
    until_row(&mut r.c, &ws, "max", |w| {
        w["tasks"]["stop"]["kind"] == "max"
    });
    assert_eq!(attention(&mut r.c, &ws), ["question"]);
    // three hand-outs of the same task, the fourth is refused
    loop_on(&mut r.c, &ws, Some(20), Some(&pane)).unwrap();
    assert!(stop(&mut r.c, &pane).is_some());
    assert!(stop(&mut r.c, &pane).is_some());
    assert_eq!(stop(&mut r.c, &pane), None);
    until_row(&mut r.c, &ws, "stuck", |w| {
        w["tasks"]["stop"]["kind"] == "stuck"
    });
    assert!(
        summary(&mut r.c, &ws)["tasks"]["stop"]["text"]
            .as_str()
            .unwrap()
            .starts_with("t1 was handed out 3 times")
    );
}

#[test]
fn a_loop_answers_the_stops_of_its_own_pane_only_and_stops_when_the_person_says_so() {
    let mut r = run(FLOW);
    let (ws, pane) = (r.ws.clone(), r.pane.clone());
    let other = new_pane(&mut r.c, &ws);
    loop_on(&mut r.c, &ws, None, Some(&pane)).unwrap();
    // another pane of the workspace: its stop is the ordinary one
    assert_eq!(stop(&mut r.c, &other), None);
    assert_eq!(attention(&mut r.c, &ws), ["agent_done"]);
    until_row(&mut r.c, &ws, "still on", |w| w["tasks"]["on"] == true);
    // the person turns it off: no attention, and the next stop is ordinary too
    r.c.cmd(Cmd::TasksLoopOff {
        workspace: ws.clone(),
    })
    .unwrap()
    .unwrap();
    until_row(&mut r.c, &ws, "off", |w| {
        w["tasks"]["stop"]["kind"] == "person"
    });
    assert!(!attention(&mut r.c, &ws).contains(&"question".to_owned()));
    assert_eq!(stop(&mut r.c, &pane), None);
    // turning off what is off is not an error
    r.c.cmd(Cmd::TasksLoopOff {
        workspace: ws.clone(),
    })
    .unwrap()
    .unwrap();
}

#[test]
fn closing_the_agents_pane_stops_the_loop_and_a_restart_leaves_it_off() {
    let mut r = run(FLOW);
    let (ws, pane) = (r.ws.clone(), r.pane.clone());
    let keep = new_pane(&mut r.c, &ws);
    loop_on(&mut r.c, &ws, None, Some(&pane)).unwrap();
    r.c.cmd(Cmd::ClosePane { pane: pane.clone() })
        .unwrap()
        .unwrap();
    until_row(&mut r.c, &ws, "the pane to end the loop", |w| {
        w["tasks"]["stop"]["kind"] == "pane"
    });
    assert!(!attention(&mut r.c, &ws).contains(&"question".to_owned()));
    // a restart: the list is as it was and the loop is off (REQ-915)
    loop_on(&mut r.c, &ws, None, Some(&keep)).unwrap();
    let Run { sb, _d, c, .. } = r;
    drop(c);
    _d.stop();
    let d2 = sb.start();
    let mut c2 = sb.client();
    open_session(&mut c2);
    until_row(&mut c2, &ws, "the list after the restart", |w| {
        w["tasks"]["total"] == 3
    });
    assert!(summary(&mut c2, &ws)["tasks"].get("on").is_none());
    d2.stop();
}

#[test]
fn turning_the_loop_on_says_why_it_cannot() {
    let mut r = run(FLOW);
    let (ws, pane) = (r.ws.clone(), r.pane.clone());
    assert_eq!(
        loop_on(&mut r.c, &ws, Some(0), Some(&pane)),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        loop_on(&mut r.c, &ws, Some(100), Some(&pane)),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        loop_on(&mut r.c, &ws, None, Some("no-such-pane")),
        Err(ErrorCode::NotFound)
    );
    assert_eq!(
        loop_on(&mut r.c, "no-such-workspace", None, None),
        Err(ErrorCode::NotFound)
    );
    // from a client, with no pane named: the workspace has no agent running
    assert_eq!(loop_on(&mut r.c, &ws, None, None), Err(ErrorCode::Conflict));
    // nothing ready, a task blocked, everything done
    std::fs::write(
        identity::tasks_path(&r.dir),
        "- [ ] a x (after: b)\n- [ ] b y (after: a)\n",
    )
    .unwrap();
    assert_eq!(
        loop_on(&mut r.c, &ws, None, Some(&pane)),
        Err(ErrorCode::Conflict)
    );
    std::fs::write(identity::tasks_path(&r.dir), "- [!] a x (blocked: why)\n").unwrap();
    assert_eq!(
        loop_on(&mut r.c, &ws, None, Some(&pane)),
        Err(ErrorCode::Conflict)
    );
    std::fs::write(
        identity::tasks_path(&r.dir),
        "- [x] a x\n- [x] fin all (after: *)\n",
    )
    .unwrap();
    assert_eq!(
        loop_on(&mut r.c, &ws, None, Some(&pane)),
        Err(ErrorCode::Conflict)
    );
    // a workspace with no list
    let plain = open_folder(&mut r.c, &folder(&r.sb, "plain", None));
    let p2 = new_pane(&mut r.c, &plain);
    assert_eq!(
        loop_on(&mut r.c, &plain, None, Some(&p2)),
        Err(ErrorCode::NotFound)
    );
    // two asks for the node give the same one (REQ-907)
    let a = node_of(&mut r.c, &ws);
    let b = node_of(&mut r.c, &ws);
    assert_eq!(a, b);
}
