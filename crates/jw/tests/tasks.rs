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
