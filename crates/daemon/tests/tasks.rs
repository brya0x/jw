//! N180b: a workspace's task list is read from its file, summed up in the workspace's row, served
//! as the `tasks` node and changed by `tasks_edit` (REQ-890 to REQ-896, REQ-939), in a private daemon.

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
