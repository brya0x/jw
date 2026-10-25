//! `tasks`: what an agent does with its workspace's task list (N180c, REQ-919 to REQ-924): `ls`,
//! `next`, `done`, `block`, `reopen` and `note`. They read and write the list's file and nothing else,
//! so they work with no daemon; the loop's commands are N180d.

use std::path::{Path, PathBuf};

use daemon::tasks::{self, Edit, Error, List, State};

use super::{Args, Fail, Out, bin, fail};

pub(super) fn run(args: &[&str]) -> Out {
    match args {
        ["ls", rest @ ..] => ls(rest),
        ["next", rest @ ..] => next(rest),
        ["done", rest @ ..] => change(rest, "done"),
        ["block", rest @ ..] => change(rest, "block"),
        ["reopen", rest @ ..] => change(rest, "reopen"),
        ["note", rest @ ..] => change(rest, "note"),
        _ => Err(Fail::Usage(String::new())),
    }
}

/// The list from the current folder up to the folder that has a `.git`, a file in a worktree and a
/// folder in a clone (REQ-920), or the file `--file` names.
fn find(a: &Args) -> Out<PathBuf> {
    if let Some(file) = a.value("file") {
        return Ok(PathBuf::from(file));
    }
    let here = std::env::current_dir().map_err(|e| Fail::Error(e.to_string()))?;
    for dir in here.ancestors() {
        let path = identity::tasks_path(dir);
        if path.is_file() {
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

/// Every task in order of level, the final one added to the file when it has none (REQ-888).
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
/// Nothing ready is exit 3, with nothing on stdout (REQ-923).
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

/// `done`, `block`, `reopen` and `note`: one change to one task (REQ-881, REQ-924).
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

/// What `jw tasks` says of itself.
pub(super) fn usage() -> String {
    let b = bin();
    format!(
        "usage: {b} tasks ls [--json] [--file <path>]\n       {b} tasks next [--file <path>]\n       {b} tasks done <id>\n       {b} tasks block <id> <reason>\n       {b} tasks reopen <id>\n       {b} tasks note <id> <text>"
    )
}
