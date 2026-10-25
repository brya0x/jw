//! N180c: `jw tasks ls | next | done | block | reopen | note` read and write a list's file and work with no
//! daemon (REQ-919, REQ-920, REQ-922 to REQ-924, REQ-888). The private socket of `Env` answers nothing.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::Env;

const LIST: &str =
    "# plan\n\n- [x] t1 schema\n- [ ] t2 endpoints (after: t1)\n- [ ] t3 docs (after: t2)\n";

/// A folder that is the root of a worktree (its `.git` is a file) with the list written in it.
fn repo(env: &Env, name: &str, list: Option<&str>) -> PathBuf {
    let dir = env.dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".git"), "gitdir: /elsewhere\n").unwrap();
    if let Some(text) = list {
        let path = identity::tasks_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn run(env: &Env, dir: &Path, args: &[&str]) -> Output {
    let mut all = vec!["tasks"];
    all.extend(args);
    env.jw().args(all).current_dir(dir).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn file(dir: &Path) -> String {
    std::fs::read_to_string(identity::tasks_path(dir)).unwrap()
}

#[test]
fn ls_lists_the_tasks_by_level_and_adds_the_final_one_once() {
    let env = Env::new("tk-ls");
    let dir = repo(&env, "app", Some(LIST));
    let sub = dir.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let o = run(&env, &sub, &["ls"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("LEVEL"), "{text}");
    assert!(
        lines[1].contains("t1") && lines[1].contains("done"),
        "{text}"
    );
    assert!(
        lines[2].contains("t2") && lines[2].contains("ready") && lines[2].contains("(after: t1)"),
        "{text}"
    );
    assert!(
        lines[4].contains("fin") && lines[4].contains("waiting") && lines[4].contains("(after: *)"),
        "{text}"
    );
    assert!(text.trim_end().ends_with("1/4 done"), "{text}");
    // REQ-888: the final task was written, once
    let written = file(&dir);
    assert!(
        written.starts_with(LIST) && written.contains("- [ ] fin tests:"),
        "{written}"
    );
    let again = run(&env, &sub, &["ls"]);
    assert_eq!(out(&again), text);
    assert_eq!(file(&dir), written, "a second ls changes nothing");
}

#[test]
fn ls_json_has_the_nodes_tasks_and_problems() {
    let env = Env::new("tk-json");
    let dir = repo(&env, "app", Some("- [ ] a one (after: zz)\n"));
    let o = run(&env, &dir, &["ls", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["tasks"][0]["id"], "a");
    assert_eq!(v["tasks"][0]["state"], "waiting");
    assert_eq!(v["tasks"][1]["final"], true);
    assert!(v["tasks"][1].get("auto").is_none(), "it is in the file now");
    assert_eq!(v["problems"][0]["line"], 1);
    assert!(v["problems"][0]["text"].as_str().unwrap().contains("zz"));
}

#[test]
fn the_list_is_looked_for_up_to_the_root_of_the_repository_and_no_further() {
    let env = Env::new("tk-find");
    let outer = repo(&env, "outer", Some(LIST));
    // an inner repository (a `.git` file) with no list: the outer one is not its list
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join(".git"), "gitdir: /x\n").unwrap();
    let o = run(&env, &inner, &["ls"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("no task list"), "{}", err(&o));
    assert!(err(&o).starts_with(&format!("{} tasks:", identity::get().name)));
    // --file names one anywhere
    let o = run(
        &env,
        &inner,
        &[
            "ls",
            "--file",
            identity::tasks_path(&outer).to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    // a folder with no list and no repository above it
    let lone = env.dir.join("lone");
    std::fs::create_dir_all(&lone).unwrap();
    assert_eq!(run(&env, &lone, &["next"]).status.code(), Some(1));
}

#[test]
fn next_prints_the_first_ready_task_and_exits_3_with_nothing_on_stdout_when_none_is() {
    let env = Env::new("tk-next");
    let dir = repo(&env, "app", Some(LIST));
    let o = run(&env, &dir, &["next"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(out(&o), "t2 endpoints\n");
    assert_eq!(file(&dir), LIST, "next writes nothing");
    // with every task done the final one, which is not in the file yet, is the next
    std::fs::write(identity::tasks_path(&dir), "- [x] a one\n- [x] b two\n").unwrap();
    let o = run(&env, &dir, &["next"]);
    assert!(out(&o).starts_with("fin tests:"), "{}", out(&o));
    std::fs::write(
        identity::tasks_path(&dir),
        "- [x] a one\n- [x] fin all of it (after: *)\n",
    )
    .unwrap();
    let o = run(&env, &dir, &["next"]);
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(out(&o), "");
    assert!(err(&o).contains("nothing is ready"));
}

#[test]
fn done_block_reopen_and_note_change_only_their_lines() {
    let env = Env::new("tk-change");
    let dir = repo(&env, "app", Some(LIST));
    let o = run(&env, &dir, &["done", "t2"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(out(&o), "t2 done\n");
    assert_eq!(file(&dir), LIST.replace("- [ ] t2", "- [x] t2"));
    // the reason is every word that follows, and a `;` or a parenthesis in it is made safe
    let o = run(
        &env,
        &dir,
        &["block", "t3", "needs", "the", "key;", "ask", "(now)"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        file(&dir).contains("- [!] t3 docs (after: t2; blocked: needs the key, ask [now])"),
        "{}",
        file(&dir)
    );
    let o = run(&env, &dir, &["reopen", "t3"]);
    assert_eq!(out(&o), "t3 reopened\n");
    assert_eq!(file(&dir), LIST.replace("- [ ] t2", "- [x] t2"));
    let o = run(&env, &dir, &["note", "t1", "CI #58 green", "(4 min)"]);
    assert_eq!(out(&o), "t1 noted\n");
    assert!(
        file(&dir).contains("- [x] t1 schema\n  - CI #58 green (4 min)\n"),
        "{}",
        file(&dir)
    );
}

#[test]
fn a_task_that_is_not_there_a_duplicate_and_a_use_that_is_wrong_are_said_and_change_nothing() {
    let env = Env::new("tk-errors");
    let text = "- [ ] a one\n- [ ] a two\n- [ ] b three\n";
    let dir = repo(&env, "app", Some(text));
    let name = &identity::get().name;
    let o = run(&env, &dir, &["done", "zz"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err(&o).starts_with(&format!("{name} tasks: no task zz")),
        "{}",
        err(&o)
    );
    let o = run(&env, &dir, &["done", "a"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("more than one task"), "{}", err(&o));
    for args in [
        &["block", "b"][..],
        &["block", "b", " "],
        &["note", "b"],
        &["done"],
        &["done", "b", "extra"],
        &["frobnicate"],
        &["ls", "--nope"],
    ] {
        let o = run(&env, &dir, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", err(&o));
        assert!(err(&o).contains("usage:"), "{args:?}");
    }
    assert_eq!(file(&dir), text);
}

// ---- the loop through the binary (N180d): `jw tasks loop …` and `jw hook stop`, against a private daemon

/// The private daemon of a test, stopped when the test ends, even by a failed assertion: `jw stop`
/// signals only the pid it recorded in that test's own state directory.
struct Daemon<'a>(&'a Env, Option<std::process::Child>);

impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        if let Some(mut child) = self.1.take() {
            let _ = self.0.jw().arg("stop").status();
            let _ = child.wait();
        }
    }
}

/// A private daemon with a workspace that has a list and a pane in it: the ids.
fn live(env: &Env) -> (Daemon<'_>, String, String) {
    let dir = repo(
        env,
        "app",
        Some("- [ ] t1 first\n- [ ] t2 second (after: t1)\n"),
    );
    let daemon = env.start();
    let ws = env.cmd(&format!(
        r#"{{"cmd":"open_folder","path":"{}"}}"#,
        std::fs::canonicalize(&dir).unwrap().display()
    ))["result"]["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = env.cmd(&format!(r#"{{"cmd":"new_pane","workspace":"{ws}"}}"#))["result"]["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    (Daemon(env, Some(daemon)), ws, pane)
}

/// `jw hook stop` as an agent's hook runs it in that pane.
fn stop_hook(env: &Env, pane: &str) -> Output {
    env.jw()
        .args(["hook", "stop"])
        .env(identity::env_var("PANE_ID"), pane)
        .output()
        .unwrap()
}

fn in_pane(env: &Env, pane: &str, args: &[&str]) -> Output {
    let mut all = vec!["tasks"];
    all.extend(args);
    env.jw()
        .args(all)
        .env(identity::env_var("PANE_ID"), pane)
        .output()
        .unwrap()
}

#[test]
fn loop_on_status_and_off_run_from_the_agents_own_pane() {
    let env = Env::new("tk-loop");
    let (_daemon, _ws, pane) = live(&env);
    let o = in_pane(&env, &pane, &["loop", "on", "--max", "5"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(out(&o), "t1 first\n", "the first task, as `next` prints it");
    let o = in_pane(&env, &pane, &["loop", "status"]);
    assert_eq!(out(&o), "loop on: lap 1/5, on t1\n", "{}", err(&o));
    // the workspace can be named instead of the pane
    let o = env
        .jw()
        .args(["tasks", "loop", "status", "--in", "app"])
        .output()
        .unwrap();
    assert_eq!(out(&o), "loop on: lap 1/5, on t1\n", "{}", err(&o));
    let o = in_pane(&env, &pane, &["loop", "off"]);
    assert_eq!(out(&o), "loop off\n");
    let o = in_pane(&env, &pane, &["loop", "status"]);
    assert_eq!(out(&o), "loop off: stopped by the person in lap 1\n");
    // a laps number that is not one, and no pane and no --in, are a wrong use
    assert_eq!(
        in_pane(&env, &pane, &["loop", "on", "--max", "many"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        env.jw()
            .args(["tasks", "loop", "on"])
            .env_remove(identity::env_var("PANE_ID"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    // from outside the workspace's folder the loop still finds it by the pane
    assert_eq!(
        in_pane(&env, &pane, &["loop", "on", "--max", "100"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn the_stop_hook_says_to_go_on_only_while_a_loop_is_on() {
    let env = Env::new("tk-hook");
    let (_daemon, _ws, pane) = live(&env);
    // no loop: the hook says nothing and the agent stops
    let o = stop_hook(&env, &pane);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(out(&o), "");
    let on = in_pane(&env, &pane, &["loop", "on"]);
    assert_eq!(on.status.code(), Some(0), "{}", err(&on));
    assert_eq!(
        run(&env, &env.dir.join("app"), &["done", "t1"])
            .status
            .code(),
        Some(0)
    );
    let o = stop_hook(&env, &pane);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value =
        serde_json::from_str(&out(&o)).unwrap_or_else(|e| panic!("{e}: {}", out(&o)));
    assert_eq!(v["decision"], "block");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("t2 second (after: t1)") && reason.contains("tasks done t2"),
        "{reason}"
    );
    assert_eq!(v.as_object().unwrap().len(), 2, "nothing else is said");
    // off again: nothing
    in_pane(&env, &pane, &["loop", "off"]);
    assert_eq!(out(&stop_hook(&env, &pane)), "");
    // a hook outside a pane of the daemon's says nothing either
    let o = env
        .jw()
        .args(["hook", "stop"])
        .env_remove(identity::env_var("PANE_ID"))
        .output()
        .unwrap();
    assert_eq!((o.status.code(), out(&o)), (Some(0), String::new()));
}

#[test]
fn with_no_daemon_the_stop_hook_says_nothing_and_does_not_fail() {
    let env = Env::new("tk-hook-none");
    let o = stop_hook(&env, "00000000-0000-4000-8000-0000000000c1");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!((out(&o), err(&o)), (String::new(), String::new()));
}
