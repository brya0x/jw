//! The task list of a workspace (N180; REQ-892 to REQ-906, REQ-954, REQ-956): a Markdown file read into
//! tasks and edited one line at a time. Everything but the functions of «the file» is text in, text out, so
//! the daemon and the binary's file commands share it and a list can be edited with no daemon.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use crate::git;

/// The id the final task gets when it is written (REQ-905).
pub const FINAL_ID: &str = "fin";

/// What the final task says (REQ-906): the tests and the CI, the evidence under the line, and the advisor.
pub const FINAL_TEXT: &str = "tests: run the project's tests and CI, note the result and the evidence under this line, and ask the advisor to review before marking it done";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Todo,
    Done,
    Blocked,
}

/// What a task is at the moment (REQ-900); `running` is the loop's and is not in the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Waiting,
    Ready,
    Done,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// The ids it waits for; `*` is every other task (REQ-899).
    pub after: Vec<String>,
    pub status: Status,
    pub why: Option<String>,
    pub notes: Vec<String>,
    pub is_final: bool,
    pub level: u32,
    pub state: State,
    /// The line it is on, from 1.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub tasks: Vec<Task>,
    pub problems: Vec<Problem>,
}

impl List {
    /// A file with no task line is not a list (REQ-904).
    pub fn is_list(&self) -> bool {
        !self.tasks.is_empty()
    }

    pub fn has_final(&self) -> bool {
        self.tasks.iter().any(|t| t.is_final)
    }

    /// The first ready task in the order of the file (REQ-903).
    pub fn next(&self) -> Option<&Task> {
        self.tasks.iter().find(|t| t.state == State::Ready)
    }

    pub fn done(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.status == Status::Done)
            .count()
    }
}

#[derive(Debug)]
pub enum Edit<'a> {
    Done,
    Reopen,
    Block(&'a str),
    Note(&'a str),
}

#[derive(Debug)]
pub enum Error {
    /// No task has that id.
    NotFound,
    /// More than one task has it (REQ-956).
    Duplicate,
    /// A reason or a note with nothing in it.
    Empty,
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

// ---- the lines

struct Line<'a> {
    text: &'a str,
    end: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    text.split_inclusive('\n')
        .map(|chunk| match chunk.strip_suffix("\r\n") {
            Some(t) => Line {
                text: t,
                end: "\r\n",
            },
            None => match chunk.strip_suffix('\n') {
                Some(t) => Line { text: t, end: "\n" },
                None => Line {
                    text: chunk,
                    end: "",
                },
            },
        })
        .collect()
}

/// A list item indented two columns or more (REQ-896).
fn note_of(s: &str) -> Option<&str> {
    let indent = s.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    let rest = &s[indent..];
    (indent >= 2 && (rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ")))
        .then(|| rest[2..].trim())
}

/// A parenthesis group at the end of a task line whose segments name `after:` or `blocked:` (REQ-894).
struct Group {
    open: usize,
    close: usize,
}

struct TaskLine {
    box_at: usize,
    status: Status,
    id: (usize, usize),
    groups: Vec<Group>,
}

fn key_of(segment: &str) -> Option<(&'static str, &str)> {
    let t = segment.trim_start();
    for key in ["after", "blocked"] {
        let n = key.len() + 1;
        if t.get(..n).is_some_and(|h| {
            h.as_bytes()[..key.len()].eq_ignore_ascii_case(key.as_bytes()) && h.ends_with(':')
        }) {
            return Some((key, t[n..].trim()));
        }
    }
    None
}

fn task_line(s: &str) -> Option<TaskLine> {
    let b = s.as_bytes();
    let indent = b.iter().take_while(|c| **c == b' ').count();
    if indent > 3
        || !matches!(b.get(indent), Some(b'-' | b'*'))
        || b.get(indent + 1) != Some(&b' ')
        || b.get(indent + 2) != Some(&b'[')
    {
        return None;
    }
    let status = match b.get(indent + 3) {
        Some(b' ') => Status::Todo,
        Some(b'x' | b'X') => Status::Done,
        Some(b'!') => Status::Blocked,
        _ => return None,
    };
    if b.get(indent + 4) != Some(&b']') || b.get(indent + 5) != Some(&b' ') {
        return None;
    }
    let box_at = indent + 3;
    let mut i = indent + 5;
    while b.get(i) == Some(&b' ') {
        i += 1;
    }
    let start = i;
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
    {
        i += 1;
    }
    if i == start || !matches!(b.get(i), None | Some(b' ')) {
        return None;
    }
    let rest = i;
    let mut groups = Vec::new();
    let mut end = s.trim_end().len();
    while end > rest && b[end - 1] == b')' {
        let (mut depth, mut open) = (0, None);
        for k in (rest..end).rev() {
            match b[k] {
                b')' => depth += 1,
                b'(' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(open) = open else { break };
        if s[open + 1..end - 1]
            .split(';')
            .any(|seg| key_of(seg).is_some())
        {
            groups.push(Group {
                open,
                close: end - 1,
            });
        }
        end = s[..open].trim_end().len();
    }
    groups.reverse();
    Some(TaskLine {
        box_at,
        status,
        id: (start, rest),
        groups,
    })
}

// ---- reading

pub fn parse(text: &str) -> List {
    let ls = lines(text);
    let mut tasks: Vec<Task> = Vec::new();
    let mut open = false;
    let mut fenced = false;
    for (n, l) in ls.iter().enumerate() {
        let t = l.text.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            open = false;
            continue;
        }
        if fenced {
            continue;
        }
        if let (true, Some(note)) = (open, note_of(l.text)) {
            tasks
                .last_mut()
                .expect("a task is open")
                .notes
                .push(note.to_string());
            continue;
        }
        open = false;
        let Some(tl) = task_line(l.text) else {
            continue;
        };
        let s = l.text;
        let mut after = Vec::new();
        let mut why = None;
        for g in &tl.groups {
            for seg in s[g.open + 1..g.close].split(';') {
                match key_of(seg) {
                    Some(("after", v)) => after.extend(
                        v.split(',')
                            .map(str::trim)
                            .filter(|x| {
                                !x.is_empty() && !x.eq_ignore_ascii_case("none") && *x != "-"
                            })
                            .map(String::from),
                    ),
                    Some((_, v)) if why.is_none() && !v.is_empty() => why = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        let mut title = String::new();
        let mut at = tl.id.1;
        for g in &tl.groups {
            title.push_str(&s[at..g.open]);
            at = g.close + 1;
        }
        title.push_str(&s[at..]);
        tasks.push(Task {
            id: s[tl.id.0..tl.id.1].to_string(),
            title: title.split_whitespace().collect::<Vec<_>>().join(" "),
            is_final: after.iter().any(|a| a == "*"),
            after,
            status: tl.status,
            why,
            notes: Vec::new(),
            level: 1,
            state: State::Waiting,
            line: n + 1,
        });
        open = true;
    }
    let mut problems = Vec::new();
    analyse(&mut tasks, &mut problems);
    List { tasks, problems }
}

/// Dependencies, levels, cycles and states (REQ-895, REQ-899 to REQ-902, REQ-956).
fn analyse(tasks: &mut [Task], problems: &mut Vec<Problem>) {
    let n = tasks.len();
    let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
    for (k, t) in tasks.iter().enumerate() {
        by_id.entry(t.id.as_str()).or_default().push(k);
    }
    let mut dup: Vec<(&str, &Vec<usize>)> = by_id
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(id, v)| (*id, v))
        .collect();
    dup.sort_by_key(|(_, v)| v[0]);
    for (id, v) in dup {
        let at: Vec<String> = v.iter().map(|k| tasks[*k].line.to_string()).collect();
        problems.push(Problem {
            line: Some(tasks[v[0]].line),
            text: format!("the id `{id}` is on lines {}", at.join(" and ")),
        });
    }
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut unresolved = vec![false; n];
    for k in 0..n {
        for a in &tasks[k].after {
            if a == "*" {
                deps[k].extend((0..n).filter(|u| *u != k && (!tasks[*u].is_final || *u < k)));
            } else {
                match by_id.get(a.as_str()).map(Vec::as_slice) {
                    Some([only]) => deps[k].push(*only),
                    Some(_) => unresolved[k] = true,
                    None => {
                        unresolved[k] = true;
                        problems.push(Problem {
                            line: Some(tasks[k].line),
                            text: format!(
                                "{} waits for {a}, which is not in the list",
                                tasks[k].id
                            ),
                        });
                    }
                }
            }
        }
    }
    let cyclic = cycles(&deps);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (k, c) in cyclic.iter().enumerate() {
        if let Some(g) = c {
            if *g >= groups.len() {
                groups.resize(*g + 1, Vec::new());
            }
            groups[*g].push(k);
        }
    }
    for g in groups.into_iter().filter(|g| !g.is_empty()) {
        let ids: Vec<&str> = g.iter().map(|k| tasks[*k].id.as_str()).collect();
        problems.push(Problem {
            line: Some(tasks[g[0]].line),
            text: format!("{} wait for each other", ids.join(", ")),
        });
    }
    let mut level = vec![0u32; n];
    for k in 0..n {
        level_of(k, &deps, &mut level, &mut Vec::new());
    }
    let done: Vec<bool> = tasks.iter().map(|t| t.status == Status::Done).collect();
    for k in 0..n {
        tasks[k].level = level[k];
        tasks[k].state = match tasks[k].status {
            Status::Done => State::Done,
            Status::Blocked => State::Blocked,
            Status::Todo
                if cyclic[k].is_none() && !unresolved[k] && deps[k].iter().all(|d| done[*d]) =>
            {
                State::Ready
            }
            Status::Todo => State::Waiting,
        };
    }
}

/// 1 plus the greatest level of what it waits for; an edge back into the path is a cycle and is left out.
fn level_of(k: usize, deps: &[Vec<usize>], level: &mut [u32], path: &mut Vec<usize>) -> u32 {
    if level[k] > 0 {
        return level[k];
    }
    path.push(k);
    let mut top = 0;
    for d in &deps[k] {
        if !path.contains(d) {
            top = top.max(level_of(*d, deps, level, path));
        }
    }
    path.pop();
    level[k] = top + 1;
    level[k]
}

/// For each task, the number of its strongly connected group when that group is a cycle (Tarjan).
fn cycles(deps: &[Vec<usize>]) -> Vec<Option<usize>> {
    struct T<'a> {
        deps: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Option<usize>>,
        groups: usize,
    }
    fn go(t: &mut T, v: usize) {
        t.index[v] = Some(t.next);
        t.low[v] = t.next;
        t.next += 1;
        t.stack.push(v);
        t.on[v] = true;
        for i in 0..t.deps[v].len() {
            let w = t.deps[v][i];
            match t.index[w] {
                None => {
                    go(t, w);
                    t.low[v] = t.low[v].min(t.low[w]);
                }
                Some(iw) if t.on[w] => t.low[v] = t.low[v].min(iw),
                Some(_) => {}
            }
        }
        if Some(t.low[v]) == t.index[v] {
            let mut members = Vec::new();
            loop {
                let w = t.stack.pop().expect("the stack holds v");
                t.on[w] = false;
                members.push(w);
                if w == v {
                    break;
                }
            }
            if members.len() > 1 || t.deps[v].contains(&v) {
                for w in members {
                    t.out[w] = Some(t.groups);
                }
                t.groups += 1;
            }
        }
    }
    let n = deps.len();
    let mut t = T {
        deps,
        index: vec![None; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: vec![None; n],
        groups: 0,
    };
    for v in 0..n {
        if t.index[v].is_none() {
            go(&mut t, v);
        }
    }
    t.out
}

// ---- writing

fn clean(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| match c {
            ';' => ',',
            '(' => '[',
            ')' => ']',
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect();
    one.trim().to_string()
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// The inner segments of a group, as byte ranges of the line.
fn segments(s: &str, g: &Group) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = g.open + 1;
    for seg in s[g.open + 1..g.close].split(';') {
        out.push((at, at + seg.len()));
        at += seg.len() + 1;
    }
    out
}

/// The line with the `blocked:` key set to `why`, or taken away when `why` is `None` (REQ-898).
fn set_blocked(s: &str, tl: &TaskLine, why: Option<&str>) -> String {
    let found = tl.groups.iter().find_map(|g| {
        segments(s, g)
            .into_iter()
            .enumerate()
            .find(|(_, (a, b))| matches!(key_of(&s[*a..*b]), Some(("blocked", _))))
            .map(|(i, r)| (g, i, r))
    });
    match (found, why) {
        (Some((_, _, (a, b))), Some(w)) => {
            let lead = s[a..b].len() - s[a..b].trim_start().len();
            format!("{}blocked: {w}{}", &s[..a + lead], &s[b..])
        }
        (Some((g, i, (a, b))), None) => {
            let segs = segments(s, g);
            if segs.len() == 1 {
                format!("{}{}", s[..g.open].trim_end(), &s[g.close + 1..])
            } else if i > 0 {
                format!("{}{}", &s[..segs[i - 1].1], &s[b..])
            } else {
                let next = segs[1].0
                    + (s[segs[1].0..segs[1].1].len() - s[segs[1].0..segs[1].1].trim_start().len());
                format!("{}{}", &s[..a], &s[next..])
            }
        }
        (None, Some(w)) => match tl.groups.last() {
            Some(g) => format!("{}; blocked: {w}{}", &s[..g.close], &s[g.close..]),
            None => format!("{} (blocked: {w})", s.trim_end()),
        },
        (None, None) => s.to_string(),
    }
}

fn set_box(s: &str, tl: &TaskLine, c: char) -> String {
    format!("{}{c}{}", &s[..tl.box_at], &s[tl.box_at + 1..])
}

/// The new text of the file with one change, found again by id in `text` itself (REQ-898): every other
/// line is as it was, with its line ending (REQ-897).
pub fn edit(text: &str, id: &str, change: Edit) -> Result<String, Error> {
    let ls = lines(text);
    let list = parse(text);
    let mut hits = list.tasks.iter().filter(|t| t.id == id);
    let at = hits.next().ok_or(Error::NotFound)?.line - 1;
    if hits.next().is_some() {
        return Err(Error::Duplicate);
    }
    let s = ls[at].text;
    let end = ls[at].end;
    // the box, then the blocked key, on the line as it is after the box
    let line = |c: char, why: Option<&str>| {
        let boxed = set_box(s, &task_line(s).expect("a task line"), c);
        set_blocked(&boxed, &task_line(&boxed).expect("still a task line"), why)
    };
    let mut out: Vec<String> = ls.iter().map(|l| format!("{}{}", l.text, l.end)).collect();
    match change {
        Edit::Done => out[at] = format!("{}{end}", line('x', None)),
        Edit::Reopen => out[at] = format!("{}{end}", line(' ', None)),
        Edit::Block(why) => {
            let why = clean(why);
            if why.is_empty() {
                return Err(Error::Empty);
            }
            out[at] = format!("{}{end}", line('!', Some(&why)));
        }
        Edit::Note(note) => {
            let note = one_line(note);
            if note.is_empty() {
                return Err(Error::Empty);
            }
            let mut last = at;
            while ls.get(last + 1).is_some_and(|l| note_of(l.text).is_some()) {
                last += 1;
            }
            let eol = if ls[last].end.is_empty() {
                line_ending(text)
            } else {
                ls[last].end
            };
            let tail = ls[last].end;
            out[last] = format!("{}{eol}", ls[last].text);
            out.insert(last + 1, format!("  - {note}{tail}"));
        }
    }
    Ok(out.concat())
}

/// The file with the final task appended when it has tasks and none is final (REQ-905); `None` when nothing
/// is to be added.
pub fn complete(text: &str) -> Option<String> {
    let list = parse(text);
    if !list.is_list() || list.has_final() {
        return None;
    }
    let mut id = FINAL_ID.to_string();
    let mut n = 1;
    while list.tasks.iter().any(|t| t.id == id) {
        n += 1;
        id = format!("{FINAL_ID}{n}");
    }
    let eol = line_ending(text);
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&format!("- [ ] {id} {FINAL_TEXT} (after: *){eol}"));
    Some(out)
}

// ---- the file

/// Replaces the file atomically, with the mode it had (REQ-898): a temporary file beside it, named for this
/// write alone so that the daemon and the binary writing at once never share one, then a rename.
pub fn write_keeping_mode(path: &Path, text: &str) -> io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("list");
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::write(&tmp, text)
        .and_then(|()| match fs::metadata(path) {
            Ok(meta) => fs::set_permissions(&tmp, meta.permissions()),
            Err(_) => Ok(()),
        })
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Makes git ignore the list of the folder `dir` on this machine alone (N188, REQ-1061 to REQ-1063): the
/// lines of `identity::tasks_exclude` go in the repository's `info/exclude`, common to its worktrees and
/// never versioned, so the list is no untracked file that `git worktree remove` refuses. Nothing is done
/// with no list or no repository; a failure is said once and the list works as before.
pub fn keep_out_of_git(dir: &Path) {
    static SAID: AtomicBool = AtomicBool::new(false);
    if !identity::tasks_path(dir).is_file() {
        return;
    }
    if let Err(why) = exclude(dir)
        && !SAID.swap(true, Ordering::Relaxed)
    {
        eprintln!("{}: not kept out of git: {why}", dir.display());
    }
}

fn exclude(dir: &Path) -> Result<(), String> {
    // one at a time in this process; with two processes the line may be twice, which git reads as one (RISK-182)
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let file = match git::run(dir, &["rev-parse", "--git-path", "info/exclude"]) {
        Ok(file) => dir.join(file),
        Err(why) if why.starts_with("cannot run git") => return Err(why),
        Err(_) => return Ok(()), // no repository
    };
    let list = identity::tasks_path(Path::new(""));
    if git::run(dir, &["check-ignore", "-q", &list.to_string_lossy()]).is_ok() {
        return Ok(());
    }
    let io = |e: io::Error| format!("{}: {e}", file.display());
    let before = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io(e)),
    };
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let lines = identity::tasks_exclude();
    let text = if before.is_empty() || before.ends_with(b"\n") {
        lines
    } else {
        format!("\n{lines}")
    };
    let mut out = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file)
        .map_err(io)?;
    io::Write::write_all(&mut out, text.as_bytes()).map_err(io)
}

/// What the daemon looks at to know a list changed without reading it: its size and its time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sig {
    len: u64,
    modified: Option<SystemTime>,
}

/// The size and time of the file; `None` when it is not there.
pub fn signature(path: &Path) -> Option<Sig> {
    let meta = fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some(Sig {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Reads the file again, makes one change and writes it back (REQ-898).
pub fn apply(path: &Path, id: &str, change: Edit) -> Result<(), Error> {
    let text = fs::read_to_string(path)?;
    let new = edit(&text, id, change)?;
    Ok(write_keeping_mode(path, &new)?)
}

/// Adds the final task to the file when it has none; whether it did.
pub fn complete_file(path: &Path) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    match complete(&text) {
        Some(new) => write_keeping_mode(path, &new).map(|()| true),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ids(l: &List) -> Vec<&str> {
        l.tasks.iter().map(|t| t.id.as_str()).collect()
    }

    fn task<'a>(l: &'a List, id: &str) -> &'a Task {
        l.tasks.iter().find(|t| t.id == id).unwrap()
    }

    /// The lines of two texts that differ, by number.
    fn changed(a: &str, b: &str) -> Vec<usize> {
        let (x, y): (Vec<&str>, Vec<&str>) = (
            a.split_inclusive('\n').collect(),
            b.split_inclusive('\n').collect(),
        );
        (0..x.len().max(y.len()))
            .filter(|i| x.get(*i) != y.get(*i))
            .collect()
    }

    #[test]
    fn a_task_line_has_a_box_an_id_a_title_and_after() {
        let l = parse(
            "# plan\n\n- [ ] t1 write the model\n- [x] t2 migrate (after: t1)\n- [!] t3 deploy (after: t1, t2; blocked: no key)\n",
        );
        assert_eq!(ids(&l), ["t1", "t2", "t3"]);
        assert_eq!(task(&l, "t2").title, "migrate");
        assert_eq!(task(&l, "t2").after, ["t1"]);
        assert_eq!(task(&l, "t2").status, Status::Done);
        assert_eq!(task(&l, "t3").after, ["t1", "t2"]);
        assert_eq!(task(&l, "t3").why.as_deref(), Some("no key"));
        assert_eq!(task(&l, "t3").line, 5);
    }

    #[test]
    fn every_trailing_group_is_read_and_a_group_without_a_key_is_title() {
        // the lines the agents really wrote (docs/claude-engine.md)
        let l = parse(concat!(
            "- [ ] t2 subir express a 5 (revisar los cambios) (range ^5.0.0, no verificado)\n",
            "- [ ] fin run the tests (after: *) (bloqueado por t4)\n",
            "- [ ] t4 correr los tests (bloqueado: npm requiere aprobación)\n",
        ));
        assert_eq!(
            task(&l, "t2").title,
            "subir express a 5 (revisar los cambios) (range ^5.0.0, no verificado)"
        );
        assert!(task(&l, "t2").after.is_empty());
        assert_eq!(task(&l, "fin").after, ["*"]);
        assert!(task(&l, "fin").is_final);
        assert_eq!(task(&l, "fin").title, "run the tests (bloqueado por t4)");
        // `bloqueado:` is the agent's own word, not the key: it stays in the title
        assert_eq!(task(&l, "t4").why, None);
        assert_eq!(
            task(&l, "t4").title,
            "correr los tests (bloqueado: npm requiere aprobación)"
        );
        assert!(l.has_final());
    }

    #[test]
    fn after_none_or_a_dash_or_nothing_is_no_dependency() {
        let l = parse(
            "- [ ] a x (after: none)\n- [ ] b x (after: -)\n- [ ] c x (after: )\n- [ ] d x (after: a)\n",
        );
        for id in ["a", "b", "c"] {
            assert!(task(&l, id).after.is_empty(), "{id}");
            assert_eq!(task(&l, id).state, State::Ready);
        }
        assert_eq!(task(&l, "d").state, State::Waiting);
        assert!(l.problems.is_empty());
    }

    #[test]
    fn a_missing_id_is_a_problem_and_the_task_waits() {
        let l = parse("- [ ] a one (after: zz)\n");
        assert_eq!(task(&l, "a").state, State::Waiting);
        assert_eq!(l.problems.len(), 1);
        assert!(l.problems[0].text.contains("zz"));
        assert_eq!(l.problems[0].line, Some(1));
    }

    #[test]
    fn items_indented_under_a_task_are_its_notes_even_with_a_box() {
        let l = parse(
            "- [x] a one\n  - cargo test: 214 ok\n  - [ ] sub item\n- [ ] b two\n   - [ ] c not a task\n",
        );
        assert_eq!(task(&l, "a").notes, ["cargo test: 214 ok", "[ ] sub item"]);
        assert_eq!(ids(&l), ["a", "b"]);
        assert_eq!(task(&l, "b").notes, ["[ ] c not a task"]);
    }

    #[test]
    fn a_line_inside_a_fenced_code_block_is_not_a_task() {
        let l = parse(
            "- [ ] a one\n```md\n- [ ] b in a snippet\n```\n- [ ] c two\n~~~\n- [ ] d\n~~~\n",
        );
        assert_eq!(ids(&l), ["a", "c"]);
        assert_eq!(task(&l, "c").line, 5);
        assert!(matches!(
            edit("```\n- [ ] x y\n```\n", "x", Edit::Done),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn a_file_with_no_task_line_is_not_a_list() {
        assert!(!parse("# just a title\n\nsome prose\n- a plain item\n").is_list());
        assert!(!parse("").is_list());
    }

    #[test]
    fn a_list_with_no_after_is_worked_in_the_order_of_the_file() {
        let l = parse("- [ ] c3 last\n- [ ] a1 first\n- [ ] b2 second\n");
        assert_eq!(l.next().unwrap().id, "c3");
        assert!(l.tasks.iter().all(|t| t.level == 1));
    }

    #[test]
    fn levels_are_one_plus_the_greatest_of_what_a_task_waits_for() {
        let l = parse(
            "- [x] t1 a\n- [ ] t2 b (after: t1)\n- [ ] t3 c (after: t1)\n- [ ] t4 d (after: t2, t3)\n- [ ] t5 e (after: t4)\n",
        );
        let levels: Vec<u32> = l.tasks.iter().map(|t| t.level).collect();
        assert_eq!(levels, [1, 2, 2, 3, 4]);
        assert_eq!(l.next().unwrap().id, "t2");
        assert_eq!(task(&l, "t4").state, State::Waiting);
    }

    #[test]
    fn a_cycle_is_a_problem_and_its_tasks_wait_even_when_one_is_done() {
        let l = parse("- [ ] a x (after: b)\n- [ ] b x (after: a)\n- [ ] c x\n");
        assert_eq!(task(&l, "a").state, State::Waiting);
        assert_eq!(task(&l, "b").state, State::Waiting);
        assert_eq!(task(&l, "c").state, State::Ready);
        assert!(
            l.problems
                .iter()
                .any(|p| p.text.contains("wait for each other"))
        );
        let l = parse("- [x] a x (after: b)\n- [ ] b x (after: a)\n");
        assert_eq!(task(&l, "b").state, State::Waiting);
        let l = parse("- [ ] a x (after: a)\n");
        assert_eq!(task(&l, "a").state, State::Waiting);
    }

    #[test]
    fn a_star_waits_for_every_task_that_is_not_final_and_for_the_finals_above_it() {
        let l = parse(
            "- [x] a one\n- [ ] f1 tests (after: *)\n- [ ] b two\n- [ ] f2 more tests (after: *)\n",
        );
        assert!(task(&l, "f1").is_final && task(&l, "f2").is_final);
        assert_eq!(task(&l, "f1").state, State::Waiting); // b is not done
        assert_eq!(task(&l, "b").state, State::Ready);
        let l = parse(
            "- [x] a one\n- [x] b two\n- [ ] f1 tests (after: *)\n- [ ] f2 more tests (after: *)\n",
        );
        assert_eq!(task(&l, "f1").state, State::Ready);
        assert_eq!(task(&l, "f2").state, State::Waiting); // f1 is above it
        assert_eq!(task(&l, "f2").level, 3);
        assert!(l.problems.is_empty());
    }

    #[test]
    fn a_duplicate_id_is_a_problem_and_what_waits_for_it_waits() {
        let l = parse("- [ ] subir lodash\n- [ ] subir express\n- [ ] t9 x (after: subir)\n");
        assert_eq!(ids(&l), ["subir", "subir", "t9"]);
        assert!(
            l.problems
                .iter()
                .any(|p| p.text.contains("`subir`") && p.text.contains("1 and 2"))
        );
        assert_eq!(task(&l, "t9").state, State::Waiting);
    }

    #[test]
    fn done_blocked_and_reopen_change_only_the_box_and_the_blocked_key() {
        let text = "# plan\n\n- [ ] t1 one (after: t0; see the doc)\nprose between\n- [ ] t2 two\n";
        let blocked = edit(text, "t1", Edit::Block("needs the key")).unwrap();
        assert_eq!(
            blocked,
            "# plan\n\n- [!] t1 one (after: t0; see the doc; blocked: needs the key)\nprose between\n- [ ] t2 two\n"
        );
        assert_eq!(changed(text, &blocked), [2]);
        let again = edit(&blocked, "t1", Edit::Block("other")).unwrap();
        assert!(again.contains("(after: t0; see the doc; blocked: other)"));
        let done = edit(&blocked, "t1", Edit::Done).unwrap();
        assert_eq!(
            done,
            "# plan\n\n- [x] t1 one (after: t0; see the doc)\nprose between\n- [ ] t2 two\n"
        );
        let reopened = edit(&done, "t1", Edit::Reopen).unwrap();
        assert_eq!(reopened, text);
        let only = edit("- [ ] a x\n", "a", Edit::Block("why")).unwrap();
        assert_eq!(only, "- [!] a x (blocked: why)\n");
        assert_eq!(edit(&only, "a", Edit::Reopen).unwrap(), "- [ ] a x\n");
        let first = edit(
            "- [!] a x (blocked: why; after: b)\n- [ ] b y\n",
            "a",
            Edit::Done,
        )
        .unwrap();
        assert_eq!(first, "- [x] a x (after: b)\n- [ ] b y\n");
    }

    #[test]
    fn a_reason_with_a_semicolon_parentheses_or_a_line_break_cannot_split_the_group() {
        let out = edit("- [ ] a x\n", "a", Edit::Block("no (key); ask\nnow")).unwrap();
        assert_eq!(out, "- [!] a x (blocked: no [key], ask now)\n");
        let l = parse(&out);
        assert_eq!(task(&l, "a").why.as_deref(), Some("no [key], ask now"));
    }

    #[test]
    fn a_note_goes_under_the_task_after_its_other_notes() {
        let text = "- [ ] a x\n  - one\n- [ ] b y";
        let out = edit(text, "a", Edit::Note("two")).unwrap();
        assert_eq!(out, "- [ ] a x\n  - one\n  - two\n- [ ] b y");
        let last = edit(text, "b", Edit::Note("fin\nline")).unwrap();
        assert_eq!(last, "- [ ] a x\n  - one\n- [ ] b y\n  - fin line");
        assert_eq!(parse(&last).tasks[1].notes, ["fin line"]);
    }

    #[test]
    fn line_endings_and_a_missing_final_newline_are_kept() {
        let crlf = "# t\r\n- [ ] a x\r\n- [ ] b y\r\n";
        let out = edit(crlf, "a", Edit::Done).unwrap();
        assert_eq!(out, "# t\r\n- [x] a x\r\n- [ ] b y\r\n");
        let out = edit(crlf, "b", Edit::Note("n")).unwrap();
        assert_eq!(out, "# t\r\n- [ ] a x\r\n- [ ] b y\r\n  - n\r\n");
        let bare = "- [ ] a x\n- [ ] b y";
        assert_eq!(edit(bare, "b", Edit::Done).unwrap(), "- [ ] a x\n- [x] b y");
        let mixed = "- [ ] a x\r\n- [ ] b y\n";
        assert_eq!(
            changed(mixed, &edit(mixed, "b", Edit::Block("w")).unwrap()),
            [1]
        );
    }

    #[test]
    fn an_unknown_id_a_duplicate_id_and_an_empty_text_change_nothing() {
        let text = "- [ ] a x\n- [ ] a y\n- [ ] b z\n  - [ ] sub\n";
        assert!(matches!(
            edit(text, "nope", Edit::Done),
            Err(Error::NotFound)
        ));
        assert!(matches!(edit(text, "a", Edit::Done), Err(Error::Duplicate)));
        assert!(matches!(
            edit(text, "b", Edit::Block("  ")),
            Err(Error::Empty)
        ));
        assert!(matches!(
            edit(text, "b", Edit::Note("\n")),
            Err(Error::Empty)
        ));
        assert!(matches!(
            edit(text, "sub", Edit::Done),
            Err(Error::NotFound)
        )); // a note is not a task
    }

    #[test]
    fn the_task_is_found_again_in_the_text_it_is_given_and_not_by_an_old_line_number() {
        let before = "- [ ] a x\n- [ ] b y\n";
        let _old = parse(before); // b was on line 2
        let after = "# a heading added by hand\n\n- [ ] a x\n- [ ] b y\n";
        assert_eq!(
            edit(after, "b", Edit::Done).unwrap(),
            "# a heading added by hand\n\n- [ ] a x\n- [x] b y\n"
        );
    }

    #[test]
    fn the_final_task_is_added_once_after_the_others_and_with_a_free_id() {
        let text = "- [ ] a one\n- [ ] b two\n";
        let out = complete(text).unwrap();
        assert_eq!(out, format!("{text}- [ ] fin {FINAL_TEXT} (after: *)\n"));
        let l = parse(&out);
        assert!(l.tasks.last().unwrap().is_final);
        assert_eq!(l.tasks.last().unwrap().level, 2);
        assert!(complete(&out).is_none());
        let taken = complete("- [ ] fin write the final report\n- [ ] fin2 other\n").unwrap();
        assert!(taken.contains("- [ ] fin3 tests:"));
        assert_eq!(complete("# title only\n"), None);
        assert_eq!(
            complete("- [ ] a x").unwrap(),
            format!("- [ ] a x\n- [ ] fin {FINAL_TEXT} (after: *)\n")
        );
        assert!(
            complete("- [ ] a x\r\n")
                .unwrap()
                .ends_with("(after: *)\r\n")
        );
        assert!(
            FINAL_TEXT.contains("tests")
                && FINAL_TEXT.contains("CI")
                && FINAL_TEXT.contains("evidence")
                && FINAL_TEXT.contains("advisor")
        );
        assert!(!FINAL_TEXT.contains(['(', ')', ';']));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jw-tasks-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_file_is_replaced_whole_with_the_mode_it_had_and_no_temporary_is_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("list.md");
        fs::write(&path, "- [ ] a x\n- [ ] b y\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        apply(&path, "a", Edit::Done).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "- [x] a x\n- [ ] b y\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(complete_file(&path).unwrap());
        assert!(!complete_file(&path).unwrap());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // a neighbour that looks like a temporary of another writer is not touched
        fs::write(dir.join("list.tmp"), "other writer").unwrap();
        apply(&path, "b", Edit::Done).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("list.tmp")).unwrap(),
            "other writer"
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        assert!(matches!(
            apply(&path, "zz", Edit::Done),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            apply(&dir.join("absent.md"), "a", Edit::Done),
            Err(Error::Io(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_line_a_change_does_not_name_is_byte_for_byte_as_it_was() {
        let text = concat!(
            "# plan  \r\n",
            "\r\n",
            "- [ ] t1 one (after: x; y)\r\n",
            "  - a note\r\n",
            "* [x] t2  two   words \r\n",
            "plain *prose* with (parentheses)\r\n",
            "- [ ] t3 three (blocked: because)\r\n",
        );
        for (id, change) in [
            ("t1", Edit::Done),
            ("t2", Edit::Reopen),
            ("t3", Edit::Block("again")),
            ("t3", Edit::Done),
        ] {
            let new = edit(text, id, change).unwrap();
            assert_eq!(changed(text, &new).len(), 1, "{id}");
        }
        let with_note = edit(text, "t1", Edit::Note("n (4 min); ok")).unwrap();
        assert_eq!(with_note.replace("  - n (4 min); ok\r\n", ""), text);
    }

    // ---- out of git (N188)

    /// A repository with a list in it, in a folder of its own.
    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jw-exclude-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git::run(&dir, &["init", "-q"]).unwrap();
        let list = identity::tasks_path(&dir);
        fs::create_dir_all(list.parent().unwrap()).unwrap();
        fs::write(list, "- [ ] a x\n").unwrap();
        dir
    }

    fn exclude_file(dir: &Path) -> PathBuf {
        dir.join(".git/info/exclude")
    }

    fn ignored(dir: &Path) -> bool {
        let list = identity::tasks_path(Path::new(""));
        git::run(dir, &["check-ignore", "-q", &list.to_string_lossy()]).is_ok()
    }

    /// REQ-1061, REQ-1062: the lines go in, once, after what was there, with a line break when it had none.
    #[test]
    fn the_lines_are_added_after_what_was_there_and_never_twice() {
        let dir = repo("add");
        let file = exclude_file(&dir);
        let mut before = fs::read_to_string(&file).unwrap_or_default();
        before.push_str("# mine\n*.log");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, &before).unwrap();
        assert!(!ignored(&dir));
        keep_out_of_git(&dir);
        keep_out_of_git(&dir);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("{before}\n{}", identity::tasks_exclude())
        );
        assert!(ignored(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-1062: no file and no folder is made, and an empty file gets the lines alone.
    #[test]
    fn the_exclude_file_and_its_folder_are_made_when_they_are_not_there() {
        let dir = repo("make");
        fs::remove_dir_all(dir.join(".git/info")).unwrap();
        keep_out_of_git(&dir);
        assert_eq!(
            fs::read_to_string(exclude_file(&dir)).unwrap(),
            identity::tasks_exclude()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-1062: what the repository already ignores is not added, and the lists of the roles are not hidden.
    #[test]
    fn a_list_the_repository_already_ignores_adds_nothing_and_the_roles_are_not_hidden() {
        let dir = repo("known");
        let folder = identity::tasks_path(Path::new(""));
        let folder = folder.parent().unwrap().display();
        fs::write(dir.join(".gitignore"), format!("{folder}/\n")).unwrap();
        let file = exclude_file(&dir);
        let before = fs::read_to_string(&file).ok();
        keep_out_of_git(&dir);
        assert_eq!(fs::read_to_string(&file).ok(), before);
        fs::remove_dir_all(&dir).unwrap();

        let dir = repo("roles");
        keep_out_of_git(&dir);
        let roles = identity::roles_dir(&dir).join("brain.md");
        fs::create_dir_all(roles.parent().unwrap()).unwrap();
        fs::write(&roles, "x").unwrap();
        let status = git::run(&dir, &["status", "--porcelain", "-uall"]).unwrap();
        assert!(status.contains("brain.md"), "{status}");
        assert!(!status.contains("tasks"), "{status}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-1062: a link is written through and kept.
    #[test]
    fn an_exclude_file_that_is_a_link_is_written_through_and_kept() {
        let dir = repo("link");
        let target = dir.join("elsewhere");
        fs::write(&target, "# mine\n").unwrap();
        let file = exclude_file(&dir);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let _ = fs::remove_file(&file);
        std::os::unix::fs::symlink(&target, &file).unwrap();
        keep_out_of_git(&dir);
        assert!(fs::symlink_metadata(&file).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            format!("# mine\n{}", identity::tasks_exclude())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-1063: no list, no repository and a file that cannot be written leave everything as it was.
    #[test]
    fn with_no_list_no_repository_or_no_way_to_write_nothing_changes() {
        let dir = repo("none");
        let list = identity::tasks_path(&dir);
        fs::remove_file(&list).unwrap();
        keep_out_of_git(&dir);
        assert!(!exclude_file(&dir).exists() || !ignored(&dir));
        fs::write(&list, "- [ ] a x\n").unwrap();

        let plain = std::env::temp_dir().join(format!("jw-exclude-plain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&plain);
        let plain_list = identity::tasks_path(&plain);
        fs::create_dir_all(plain_list.parent().unwrap()).unwrap();
        fs::write(&plain_list, "- [ ] a x\n").unwrap();
        assert_eq!(exclude(&plain), Ok(()), "no repository is not a failure");
        assert!(!plain.join(".git").exists());
        fs::remove_dir_all(&plain).unwrap();

        let file = exclude_file(&dir);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "# mine\n").unwrap();
        let mut mode = fs::metadata(&file).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o444);
        fs::set_permissions(&file, mode).unwrap();
        // a process that may write anyway (root) is not the case this is about
        if fs::OpenOptions::new().append(true).open(&file).is_err() {
            assert!(exclude(&dir).is_err());
            keep_out_of_git(&dir);
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "# mine\n");
        assert_eq!(fs::read_to_string(&list).unwrap(), "- [ ] a x\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-1064: a list added with `git add -f` stays tracked.
    #[test]
    fn a_list_added_to_git_by_the_person_stays_tracked() {
        let dir = repo("tracked");
        let list = identity::tasks_path(Path::new(""))
            .to_string_lossy()
            .into_owned();
        keep_out_of_git(&dir);
        git::run(&dir, &["add", "-f", &list]).unwrap();
        keep_out_of_git(&dir);
        assert_eq!(git::run(&dir, &["ls-files"]).unwrap(), list);
        fs::remove_dir_all(&dir).unwrap();
    }
}
