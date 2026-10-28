//! `tasks`: what an agent does with its workspace's task list (N180c, REQ-936 to REQ-941): `ls`,
//! `next`, `done`, `block`, `reopen` and `note`. They read and write the list's file and nothing else,
//! so they work with no daemon. `loop on|off|status` are the loop's, which the daemon runs (N180d).

use std::path::{Path, PathBuf};

use contract::Cmd;
use contract::WorkspaceSummary;
use daemon::tasks::{self, Edit, Error, List, State};

use super::{Args, Fail, Link, Out, bin, fail, own_pane};

pub(super) fn run(args: &[&str]) -> Out {
    match args {
        ["ls", rest @ ..] => ls(rest),
        ["next", rest @ ..] => next(rest),
        ["done", rest @ ..] => change(rest, "done"),
        ["block", rest @ ..] => change(rest, "block"),
        ["reopen", rest @ ..] => change(rest, "reopen"),
        ["note", rest @ ..] => change(rest, "note"),
        ["loop", "on", rest @ ..] => loop_on(rest),
        ["loop", "off", rest @ ..] => loop_off(rest),
        ["loop", "status", rest @ ..] => loop_status(rest),
        _ => Err(Fail::Usage(String::new())),
    }
}

/// The list from the current folder up to the folder that has a `.git`, a file in a worktree and a
/// folder in a clone (REQ-937), or the file `--file` names.
fn find(a: &Args) -> Out<PathBuf> {
    if let Some(file) = a.value("file") {
        return Ok(PathBuf::from(file));
    }
    let here = std::env::current_dir().map_err(|e| Fail::Error(e.to_string()))?;
    for dir in here.ancestors() {
        let path = identity::tasks_path(dir);
        if path.is_file() {
            tasks::keep_out_of_git(dir);
            return Ok(path);
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    fail("no task list from here up to the root of the repository (--file names one)")
}

fn read(path: &Path) -> Out<String> {
    std::fs::read_to_string(path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))
}

fn parsed(path: &Path, text: &str) -> Out<List> {
    let list = tasks::parse(text);
    if list.is_list() {
        Ok(list)
    } else {
        fail(format!("no task in {}", path.display()))
    }
}

/// Every task in order of level, the final one added to the file when it has none (REQ-905).
fn ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &["file"], &["json"])?;
    a.words::<0>()?;
    let path = find(&a)?;
    tasks::complete_file(&path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let list = parsed(&path, &read(&path)?)?;
    if a.has("json") {
        let items: Vec<_> = list
            .tasks
            .iter()
            .map(|t| daemon::tasklists::item(t, None))
            .collect();
        let problems: Vec<_> = list
            .problems
            .iter()
            .map(daemon::tasklists::problem)
            .collect();
        println!(
            "{}",
            serde_json::json!({ "tasks": items, "problems": problems })
        );
        return Ok(());
    }
    let mut order: Vec<_> = list.tasks.iter().collect();
    order.sort_by_key(|t| (t.level, t.line));
    let word = |s: State| match s {
        State::Waiting => "waiting",
        State::Ready => "ready",
        State::Done => "done",
        State::Blocked => "blocked",
    };
    let width = order
        .iter()
        .map(|t| t.id.chars().count())
        .max()
        .unwrap_or(2)
        .max(2);
    println!("{:<5} {:<width$} {:<8} TASK", "LEVEL", "ID", "STATE");
    for t in order {
        let waits = if t.after.is_empty() {
            String::new()
        } else {
            format!("  (after: {})", t.after.join(", "))
        };
        let why = t
            .why
            .as_deref()
            .map(|w| format!("  [blocked: {w}]"))
            .unwrap_or_default();
        println!(
            "{:<5} {:<width$} {:<8} {}{waits}{why}",
            t.level,
            t.id,
            word(t.state),
            t.title
        );
        for n in &t.notes {
            println!("{:<5} {:<width$} {:<8}   - {n}", "", "", "");
        }
    }
    for p in &list.problems {
        match p.line {
            Some(l) => println!("problem (line {l}): {}", p.text),
            None => println!("problem: {}", p.text),
        }
    }
    println!("{}/{} done", list.done(), list.tasks.len());
    Ok(())
}

/// The next ready task: what the file says, with the final task counted even if it is not written yet.
/// Nothing ready is exit 3, with nothing on stdout (REQ-940).
fn next(args: &[&str]) -> Out {
    let a = Args::parse(args, &["file"], &[])?;
    a.words::<0>()?;
    let path = find(&a)?;
    let text = read(&path)?;
    let whole = tasks::complete(&text);
    let list = parsed(&path, whole.as_deref().unwrap_or(&text))?;
    match list.next() {
        Some(t) => {
            println!("{} {}", t.id, t.title);
            Ok(())
        }
        None => Err(Fail::Idle("nothing is ready".into())),
    }
}

/// `done`, `block`, `reopen` and `note`: one change to one task (REQ-898, REQ-941).
fn change(args: &[&str], verb: &str) -> Out {
    let a = Args::parse(args, &["file"], &[])?;
    let Some((id, words)) = a.pos.split_first() else {
        return Err(Fail::Usage("a task id is needed".into()));
    };
    let text = words
        .iter()
        .chain(&a.rest)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let edit = match verb {
        "done" => Edit::Done,
        "reopen" => Edit::Reopen,
        "block" => Edit::Block(&text),
        _ => Edit::Note(&text),
    };
    if matches!(verb, "done" | "reopen") && !text.is_empty() {
        return Err(Fail::Usage(format!("{verb} takes only an id")));
    }
    let path = find(&a)?;
    match tasks::apply(&path, id, edit) {
        Ok(()) => {
            println!(
                "{id} {}",
                match verb {
                    "done" => "done",
                    "block" => "blocked",
                    "reopen" => "reopened",
                    _ => "noted",
                }
            );
            Ok(())
        }
        Err(Error::NotFound) => fail(format!("no task {id} in {}", path.display())),
        Err(Error::Duplicate) => fail(format!(
            "the id {id} is on more than one task: nothing was changed"
        )),
        Err(Error::Empty) => Err(Fail::Usage(format!("{verb} needs a text"))),
        Err(Error::Io(e)) => fail(format!("{}: {e}", path.display())),
    }
}

/// The workspace a loop command is about: the one `--in` names, else the one the agent's own pane is in.
fn target(a: &Args, s: &mut Link) -> Out<WorkspaceSummary> {
    if let Some(word) = a.value("in") {
        return s.workspace(word);
    }
    let Some(pane) = own_pane() else {
        return Err(Fail::Usage(
            "run it in a pane of the daemon's, or name the workspace with --in".into(),
        ));
    };
    match s.workspace_of(&pane)? {
        Some(ws) => Ok(ws),
        None => fail("this pane is in no workspace"),
    }
}

/// `loop on [--max N]`: the first task is printed as `next` prints it (REQ-938).
fn loop_on(args: &[&str]) -> Out {
    let a = Args::parse(args, &["max", "in"], &[])?;
    a.words::<0>()?;
    let max = match a.value("max") {
        None => None,
        Some(n) => Some(
            n.parse::<u32>()
                .map_err(|_| Fail::Usage("--max is a number of laps".into()))?,
        ),
    };
    let mut s = Link::connect()?;
    let ws = target(&a, &mut s)?;
    // an agent that asks for the loop on its own workspace is the loop's agent and is sent nothing
    let pane = if a.value("in").is_none() {
        own_pane()
    } else {
        None
    };
    let reply = s.cmd(Cmd::TasksLoopOn {
        workspace: ws.id,
        max,
        pane,
    })?;
    let task = &reply["task"];
    println!(
        "{} {}",
        task["id"].as_str().unwrap_or(""),
        task["title"].as_str().unwrap_or("")
    );
    Ok(())
}

fn loop_off(args: &[&str]) -> Out {
    let a = Args::parse(args, &["in"], &[])?;
    a.words::<0>()?;
    let mut s = Link::connect()?;
    let ws = target(&a, &mut s)?;
    s.cmd(Cmd::TasksLoopOff { workspace: ws.id })?;
    println!("loop off");
    Ok(())
}

/// `loop status`: whether it is on, and in which lap, or why it is not.
fn loop_status(args: &[&str]) -> Out {
    let a = Args::parse(args, &["in"], &[])?;
    a.words::<0>()?;
    let mut s = Link::connect()?;
    let ws = target(&a, &mut s)?;
    let node = s.cmd(Cmd::OpenTasks { workspace: ws.id })?;
    let node = node["node"].as_str().unwrap_or("").to_owned();
    let doc: serde_json::Value = s.node(&node)?;
    let lp = &doc["loop"];
    if lp["on"] == true {
        let at = lp["task"]
            .as_str()
            .map(|t| format!(", on {t}"))
            .unwrap_or_default();
        println!("loop on: lap {}/{}{at}", lp["laps"], lp["max"]);
    } else if let Some(text) = lp["stop"]["text"].as_str() {
        println!("loop off: {text}");
    } else {
        println!("loop off");
    }
    Ok(())
}

/// What `jw tasks` says of itself.
pub(super) fn usage() -> String {
    let b = bin();
    format!(
        "usage: {b} tasks ls [--json] [--file <path>]\n       {b} tasks next [--file <path>]\n       {b} tasks done <id>\n       {b} tasks block <id> <reason>\n       {b} tasks reopen <id>\n       {b} tasks note <id> <text>\n       {b} tasks loop on [--max N] [--in <workspace>]\n       {b} tasks loop off [--in <workspace>]\n       {b} tasks loop status [--in <workspace>]"
    )
}
