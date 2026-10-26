//! The task lists the daemon serves (N180; REQ-907 to REQ-914): what it knows of each workspace's
//! list, the `tasks` node, and the two commands that open and change it. The file itself is
//! `tasks.rs`; every read and write here runs with the hub's lock released.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use contract::tasks::{
    LoopInfo, LoopStop, StopKind, TaskItem, TaskOp, TaskPhase, TaskProblem, TaskStatus, Tasks,
    TasksSummary,
};
use contract::{AttentionKind, ErrorCode, Head, Id};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::core::Core;
use crate::server::Shared;
use crate::tasks::{self, Edit, State, Status};

/// How long a list nobody asks for is left alone: the poll goes by every 2 seconds, so this is the
/// second poll after the last look, 6 seconds at most (REQ-910).
const SLOW: Duration = Duration::from_secs(5);

/// The laps of a loop when nobody says (REQ-916).
const DEFAULT_MAX: u32 = 20;

/// What the daemon last read of one workspace's list. `list` has the final task in it even while
/// it is not in the file (`auto` says which task that is), so a row counts it from the start.
pub struct Cached {
    sig: tasks::Sig,
    pub list: tasks::List,
    pub auto: Option<String>,
    looked: Instant,
}

/// A `tasks` node a client asked for.
pub struct NodeEntry {
    pub workspace: Id,
    pub idle_since: Instant,
}

/// A list to look at again, off the lock.
pub struct Job {
    pub workspace: Id,
    pub dir: PathBuf,
    known: Option<tasks::Sig>,
}

/// What a look found.
pub enum Look {
    /// No file, or no task in it.
    Gone,
    /// The file is as the daemon has it.
    Same,
    New(Cached),
}

/// Looks at the list of the folder `dir`: its size and time first, and the file only when they
/// are not what `known` says.
pub fn look(dir: &Path, known: Option<tasks::Sig>) -> Look {
    let path = identity::tasks_path(dir);
    let Some(sig) = tasks::signature(&path) else {
        return Look::Gone;
    };
    if known == Some(sig) {
        return Look::Same;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Look::Gone;
    };
    let whole = tasks::complete(&text);
    let list = tasks::parse(whole.as_deref().unwrap_or(&text));
    if !list.is_list() {
        return Look::Gone;
    }
    let auto = whole.and_then(|_| list.tasks.last().map(|t| t.id.clone()));
    Look::New(Cached {
        sig,
        list,
        auto,
        looked: Instant::now(),
    })
}

fn phase(t: &tasks::Task) -> TaskPhase {
    match t.state {
        State::Waiting => TaskPhase::Waiting,
        State::Ready => TaskPhase::Ready,
        State::Done => TaskPhase::Done,
        State::Blocked => TaskPhase::Blocked,
    }
}

/// A task as the node and `jw tasks ls --json` say it; `auto` names the final task that is not in the file.
pub fn item(t: &tasks::Task, auto: Option<&str>) -> TaskItem {
    TaskItem {
        id: t.id.clone(),
        title: t.title.clone(),
        after: t.after.clone(),
        status: match t.status {
            Status::Todo => TaskStatus::Todo,
            Status::Done => TaskStatus::Done,
            Status::Blocked => TaskStatus::Blocked,
        },
        state: phase(t),
        level: t.level,
        why: t.why.clone(),
        notes: t.notes.clone(),
        is_final: t.is_final,
        auto: auto == Some(t.id.as_str()),
    }
}

/// What is wrong in the file, as the node says it.
pub fn problem(p: &tasks::Problem) -> TaskProblem {
    TaskProblem {
        line: p.line.map(|l| l as u32),
        text: p.text.clone(),
    }
}

/// The loop of one workspace's agent over its list (REQ-915): in the daemon's memory and no further.
#[derive(Clone, Debug)]
pub struct Loop {
    pub on: bool,
    /// Tasks handed out so far.
    pub laps: u32,
    pub max: u32,
    /// The task handed out last, and how many times in a row.
    pub task: Option<String>,
    pub same: u32,
    /// The pane of the agent whose turns the loop answers (REQ-919).
    pub pane: Id,
    pub stop: Option<LoopStop>,
    /// The loop stopped for a reason that needs the person and the engine's turn has not ended yet:
    /// the next `agent_done` of that pane is a `question` (REQ-960), once.
    pub ended: bool,
}

/// What the end of an agent's turn comes to (REQ-922).
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Hand(String),
    Stop(StopKind, String),
}

/// The order of REQ-922, from the file as it is now: everything done, a task blocked, nothing ready,
/// the laps used up, the same task handed out three times in a row and not done, else the next.
pub fn decide(list: &tasks::List, laps: u32, max: u32, last: Option<&str>, same: u32) -> Decision {
    let n = list.tasks.len();
    if list.tasks.iter().all(|t| t.status == Status::Done) {
        let plural = if laps == 1 { "" } else { "s" };
        return Decision::Stop(
            StopKind::Done,
            format!("all {n} tasks done in {laps} lap{plural}"),
        );
    }
    if let Some(t) = list.tasks.iter().find(|t| t.status == Status::Blocked) {
        let why = t.why.as_deref().unwrap_or("no reason given");
        return Decision::Stop(StopKind::Blocked, format!("{} blocked: {why}", t.id));
    }
    let Some(next) = list.next() else {
        return Decision::Stop(
            StopKind::Idle,
            "nothing is ready: what is left waits for what cannot be done".into(),
        );
    };
    if laps >= max {
        let left = n - list.done();
        return Decision::Stop(
            StopKind::Max,
            format!("the maximum of {max} laps, with {left} tasks left"),
        );
    }
    if last == Some(next.id.as_str()) && same >= 3 {
        return Decision::Stop(
            StopKind::Stuck,
            format!("{} was handed out 3 times and is not done", next.id),
        );
    }
    Decision::Hand(next.id.clone())
}

/// What the agent is told at a lap (REQ-923): the file, the task, what to do, and not to ask.
pub fn reason(t: &tasks::Task, path: &Path) -> String {
    let b = &identity::get().name;
    let id = &t.id;
    if t.is_final {
        format!(
            "Last task of {}: {}\nNote what you find with `{b} tasks note {id} \"…\"` and, when it is done, run `{b} tasks done {id}`.\nIf it fails, do not ask: run `{b} tasks block {id} \"why\"`.",
            path.display(),
            t.title
        )
    } else {
        let waits = if t.after.is_empty() {
            String::new()
        } else {
            format!(" (after: {})", t.after.join(", "))
        };
        format!(
            "Next ready task in {}: {id} {}{waits}.\nDo it, mark it with `{b} tasks done {id}` and end your turn.\nIf you need the person, do not ask: run `{b} tasks block {id} \"why\"`.",
            path.display(),
            t.title
        )
    }
}

/// The node's document for a workspace (REQ-907).
fn node(
    head: Head,
    workspace: &Id,
    path: &Path,
    c: &Cached,
    lp: Option<&Loop>,
    waiting: bool,
) -> Tasks {
    Tasks {
        head,
        workspace: workspace.clone(),
        path: path.display().to_string(),
        tasks: c
            .list
            .tasks
            .iter()
            .map(|t| {
                let mut it = item(t, c.auto.as_deref());
                // the task the loop handed out and that is not done is the one that runs (REQ-900)
                if lp.is_some_and(|l| l.on && l.task.as_deref() == Some(t.id.as_str()))
                    && t.status == Status::Todo
                {
                    it.state = TaskPhase::Running;
                }
                it
            })
            .collect(),
        problems: c.list.problems.iter().map(problem).collect(),
        loop_: lp.map_or(
            LoopInfo {
                on: false,
                laps: 0,
                max: DEFAULT_MAX,
                task: None,
                pane: None,
                waiting: false,
                stop: None,
            },
            |l| LoopInfo {
                on: l.on,
                laps: l.laps,
                max: l.max,
                task: l.task.clone().filter(|_| l.on),
                pane: Some(l.pane.clone()),
                waiting: l.on && waiting,
                stop: l.stop.clone(),
            },
        ),
    }
}

impl Core {
    /// What a workspace's row says of its list (REQ-908); the final task is counted (REQ-905).
    pub fn task_summary(&self, workspace: &str) -> Option<TasksSummary> {
        let c = self.task_lists.get(workspace)?;
        let lp = self.loops.get(workspace);
        Some(TasksSummary {
            done: c.list.done() as u32,
            total: c.list.tasks.len() as u32,
            on: lp.is_some_and(|l| l.on),
            stop: lp.filter(|l| !l.on).and_then(|l| l.stop.clone()),
        })
    }

    /// The `tasks` node document of an entry, built from what was last read (no I/O).
    pub(crate) fn tasks_node(&self, id: &Id, e: &NodeEntry, head: Head) -> Option<Tasks> {
        let c = self.task_lists.get(&e.workspace)?;
        let dir = self.workspace_dir(&e.workspace)?;
        let lp = self.loops.get(&e.workspace);
        // the agent waits for the person while it has a permission, a question or a plan raised
        let waiting = lp.is_some_and(|l| {
            self.pane_attention(&l.pane).iter().any(|k| {
                matches!(
                    k,
                    AttentionKind::Permission | AttentionKind::Question | AttentionKind::Plan
                )
            })
        });
        Some(node(
            Head {
                id: id.clone(),
                ..head
            },
            &e.workspace,
            &identity::tasks_path(&dir),
            c,
            lp,
            waiting,
        ))
    }

    /// A look came back: keep what it found and tell the clients if the list is not what it was.
    pub fn tasks_seen(&mut self, workspace: &Id, found: Look) {
        self.task_seen.insert(workspace.clone());
        let changed = match found {
            Look::Same => {
                if let Some(c) = self.task_lists.get_mut(workspace) {
                    c.looked = Instant::now();
                }
                false
            }
            Look::Gone => {
                self.task_nodes.retain(|_, e| e.workspace != *workspace);
                self.task_lists.remove(workspace).is_some()
            }
            Look::New(new) => {
                self.end_blocked_question(workspace, &new.list);
                let changed = self
                    .task_lists
                    .get(workspace)
                    .is_none_or(|old| old.list != new.list || old.auto != new.auto);
                self.task_lists.insert(workspace.clone(), new);
                changed
            }
        };
        if changed {
            self.refresh();
        }
    }

    /// The kinds of attention a pane has raised.
    pub(crate) fn pane_attention(&self, pane: &str) -> Vec<AttentionKind> {
        self.model
            .workspaces
            .iter()
            .flat_map(|w| w.panes.iter())
            .find(|p| p.id == pane)
            .map(|p| p.attention.iter().map(|o| o.kind).collect())
            .unwrap_or_default()
    }

    /// The `question` of a loop that stopped for a blocked task ends when nothing is blocked any
    /// more (REQ-927).
    fn end_blocked_question(&mut self, workspace: &Id, list: &tasks::List) {
        let Some(lp) = self.loops.get(workspace) else {
            return;
        };
        let blocked_stop = !lp.on
            && lp
                .stop
                .as_ref()
                .is_some_and(|s| s.kind == StopKind::Blocked);
        if !blocked_stop || list.tasks.iter().any(|t| t.status == Status::Blocked) {
            return;
        }
        let pane = lp.pane.clone();
        let own = self
            .model
            .workspaces
            .iter_mut()
            .flat_map(|w| w.panes.iter_mut())
            .find(|p| p.id == pane);
        if let Some(p) = own {
            p.attention.retain(|o| o.kind != AttentionKind::Question);
        }
    }

    /// Whether the loop of this pane stopped for a reason that needs the person and its turn has not
    /// ended yet; it is asked once (REQ-960).
    pub(crate) fn take_loop_ended(&mut self, pane: &str) -> bool {
        self.loops
            .values_mut()
            .find(|l| l.pane == pane && l.ended)
            .is_some_and(|l| std::mem::take(&mut l.ended))
    }

    /// A note in the conversation of this pane, when it has one: what the loop did (REQ-962).
    fn conv_note(&mut self, pane: &str, text: String) {
        if let Some(c) = self.convs.get_mut(pane) {
            c.st.loop_note(text);
            c.dirty = true;
        }
    }

    /// A loop whose agent's pane is gone stops (REQ-928), without an attention.
    pub(crate) fn loops_check_panes(&mut self) {
        let panes: HashSet<Id> = self
            .model
            .workspaces
            .iter()
            .flat_map(|w| w.panes.iter().map(|p| p.id.clone()))
            .collect();
        for lp in self.loops.values_mut() {
            if lp.on && !panes.contains(&lp.pane) {
                lp.on = false;
                lp.task = None;
                lp.stop = Some(LoopStop {
                    kind: StopKind::Pane,
                    text: "the agent's pane closed".into(),
                });
            }
        }
    }

    /// The node of a workspace that has a list, made if it is not there (REQ-907).
    fn find_or_add_tasks(&mut self, workspace: &Id) -> Option<Id> {
        self.task_lists.get(workspace)?;
        let now = Instant::now();
        if let Some((id, e)) = self
            .task_nodes
            .iter_mut()
            .find(|(_, e)| e.workspace == *workspace)
        {
            e.idle_since = now;
            return Some(id.clone());
        }
        let id = crate::model::new_id();
        self.task_nodes.insert(
            id.clone(),
            NodeEntry {
                workspace: workspace.clone(),
                idle_since: now,
            },
        );
        self.refresh();
        Some(id)
    }

    /// Look at this workspace's list at the next poll (REQ-911): an agent moved.
    pub fn task_look_soon(&mut self, workspace: &str) {
        self.task_look.insert(workspace.to_owned());
    }

    /// The same for the workspace that has this pane.
    pub fn task_look_pane(&mut self, pane: &str) {
        if let Some(w) = self
            .model
            .workspaces
            .iter()
            .find(|w| w.panes.iter().any(|p| p.id == pane))
        {
            let id = w.id.clone();
            self.task_look.insert(id);
        }
    }

    /// The lists to look at in this round of the poll (REQ-910, REQ-911): the ones asked for, a
    /// workspace not looked at yet (one that was restored or just opened), the ones a client has
    /// open (every round) and the ones that have a list, once in a while. With no list anywhere and
    /// every workspace seen, it is empty: no periodic work.
    pub fn task_jobs(&mut self) -> Vec<Job> {
        let watched = self.watched_nodes();
        let open: HashSet<Id> = self
            .task_nodes
            .iter()
            .filter(|(id, _)| watched.contains(*id))
            .map(|(_, e)| e.workspace.clone())
            .collect();
        let now = Instant::now();
        for (id, e) in &mut self.task_nodes {
            if watched.contains(id) {
                e.idle_since = now;
            }
        }
        let asked = std::mem::take(&mut self.task_look);
        let due: Vec<Id> = self
            .model
            .workspaces
            .iter()
            .filter(|w| {
                !self.task_seen.contains(&w.id)
                    || asked.contains(&w.id)
                    || open.contains(&w.id)
                    || self
                        .task_lists
                        .get(&w.id)
                        .is_some_and(|c| c.looked.elapsed() >= SLOW)
            })
            .map(|w| w.id.clone())
            .collect();
        due.into_iter()
            .filter_map(|workspace| {
                let known = self.task_lists.get(&workspace).map(|c| c.sig);
                Some(Job {
                    dir: self.workspace_dir(&workspace)?,
                    workspace,
                    known,
                })
            })
            .collect()
    }
}

/// One round of looks, each off the hub's lock.
pub fn poll(shared: &Shared) {
    let jobs = shared.lock().task_jobs();
    for job in jobs {
        let found = look(&job.dir, job.known);
        shared.lock().tasks_seen(&job.workspace, found);
    }
}

fn dir_of(shared: &Shared, workspace: &Id) -> Result<PathBuf, ErrorBody> {
    shared
        .lock()
        .workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))
}

/// `open_tasks`: the node of the workspace's list, made if it is not there (REQ-907).
pub fn open_tasks(shared: &Shared, workspace: &Id) -> Done {
    let dir = dir_of(shared, workspace)?;
    let found = look(&dir, None);
    let mut core = shared.lock();
    core.tasks_seen(workspace, found);
    match core.find_or_add_tasks(workspace) {
        Some(node) => Ok(Some(json!({ "node": node }))),
        None => Err(err(
            ErrorCode::NotFound,
            format!("no task list in {}", identity::tasks_path(&dir).display()),
        )),
    }
}

/// `tasks_edit`: the same change `jw tasks` makes (REQ-912, REQ-913), and the clients are told at once.
pub fn edit(shared: &Shared, workspace: &Id, op: TaskOp, id: &str, text: Option<&str>) -> Done {
    let dir = dir_of(shared, workspace)?;
    let path = identity::tasks_path(&dir);
    let change = match op {
        TaskOp::Done => Edit::Done,
        TaskOp::Reopen => Edit::Reopen,
        TaskOp::Block => Edit::Block(text.unwrap_or("")),
        TaskOp::Note => Edit::Note(text.unwrap_or("")),
    };
    let done = tasks::apply(&path, id, change);
    let found = look(&dir, None);
    shared.lock().tasks_seen(workspace, found);
    match done {
        Ok(()) => Ok(None),
        Err(tasks::Error::NotFound) => Err(err(
            ErrorCode::NotFound,
            format!("no task {id} in the list"),
        )),
        Err(tasks::Error::Duplicate) => Err(err(
            ErrorCode::Conflict,
            format!("the id {id} is on more than one task"),
        )),
        Err(tasks::Error::Empty) => Err(err(
            ErrorCode::BadRequest,
            "a reason or a note needs a text",
        )),
        Err(tasks::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(err(
            ErrorCode::NotFound,
            format!("no task list in {}", path.display()),
        )),
        Err(tasks::Error::Io(e)) => Err(err(ErrorCode::Io, e.to_string())),
    }
}

/// The agent pane of a workspace: the one `pane` names (it must be the workspace's), else the first
/// that runs a conversation or an agent (REQ-919).
fn agent_pane(core: &Core, workspace: &Id, pane: Option<&Id>) -> Result<Id, ErrorBody> {
    let ws = core
        .model
        .workspaces
        .iter()
        .find(|w| w.id == *workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
    match pane {
        Some(p) if ws.panes.iter().any(|q| q.id == *p) => Ok(p.clone()),
        Some(p) => Err(err(
            ErrorCode::NotFound,
            format!("no pane {p} in the workspace"),
        )),
        None => ws
            .panes
            .iter()
            .find(|p| {
                core.convs.contains_key(&p.id)
                    || p.running.as_deref().and_then(crate::agent_name).is_some()
            })
            .map(|p| p.id.clone())
            .ok_or_else(|| {
                err(
                    ErrorCode::Conflict,
                    "the workspace has no agent to run the loop",
                )
            }),
    }
}

/// Tell the agent of `pane` the first task, as `jw prompt` does: a conversation by its own message
/// and a program in the pane by a paste and an Enter (REQ-918). The lock is not held.
fn deliver(shared: &Shared, pane: &Id, text: &str) {
    let conv = shared.lock().convs.get(pane).map(|c| c.id.clone());
    if let Some(node) = conv {
        let _ = crate::engine::send(shared, &node, text, Vec::new(), false);
        return;
    }
    let Some(term) = shared.lock().term(pane) else {
        return;
    };
    let _ = term.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
    // the input box takes the paste as text first, and Enter later
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let _ = term.write(b"\r");
    });
}

/// `tasks_loop_on` (REQ-916 to REQ-918): the loop starts at lap 1 with the first ready task.
pub fn loop_on(shared: &Shared, workspace: &Id, max: Option<u32>, pane: Option<&Id>) -> Done {
    let max = max.unwrap_or(DEFAULT_MAX);
    if !(1..=99).contains(&max) {
        return Err(err(ErrorCode::BadRequest, "the laps are 1 to 99"));
    }
    let dir = dir_of(shared, workspace)?;
    let path = identity::tasks_path(&dir);
    let no_list = || {
        err(
            ErrorCode::NotFound,
            format!("no task list in {}", path.display()),
        )
    };
    let io = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::NotFound {
            no_list()
        } else {
            err(ErrorCode::Io, e.to_string())
        }
    };
    tasks::complete_file(&path).map_err(io)?;
    let list = tasks::parse(&std::fs::read_to_string(&path).map_err(io)?);
    if !list.is_list() {
        return Err(no_list());
    }
    let found = look(&dir, None);
    let agent = agent_pane(&shared.lock(), workspace, pane)?;
    let id = match decide(&list, 0, max, None, 0) {
        Decision::Hand(id) => id,
        Decision::Stop(_, why) => {
            shared.lock().tasks_seen(workspace, found);
            return Err(err(ErrorCode::Conflict, why));
        }
    };
    let task = list
        .tasks
        .iter()
        .find(|t| t.id == id)
        .expect("the task is in the list");
    let text = reason(task, &path);
    {
        let mut core = shared.lock();
        core.loops.insert(
            workspace.clone(),
            Loop {
                on: true,
                laps: 1,
                max,
                task: Some(id.clone()),
                same: 1,
                pane: agent.clone(),
                stop: None,
                ended: false,
            },
        );
        crate::agents::settle(&mut core, &agent, None);
        core.tasks_seen(workspace, found);
        core.refresh();
    }
    if pane.is_none() {
        deliver(shared, &agent, &text);
    }
    Ok(Some(
        json!({ "laps": 1, "max": max, "task": { "id": id, "title": task.title, "text": text } }),
    ))
}

/// `tasks_loop_off` (REQ-928).
pub fn loop_off(shared: &Shared, workspace: &Id) -> Done {
    let mut core = shared.lock();
    core.workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
    if let Some(lp) = core.loops.get_mut(workspace).filter(|l| l.on) {
        lp.on = false;
        lp.task = None;
        lp.stop = Some(LoopStop {
            kind: StopKind::Person,
            text: format!("stopped by the person in lap {}", lp.laps),
        });
        core.refresh();
    }
    Ok(None)
}

/// The end of an agent's turn, for the pane of a loop that is on (REQ-919 to REQ-926): `None` when
/// there is no such loop, and the ordinary attention follows. The file is read with the lock released.
pub fn on_stop(shared: &Shared, pane: &Id) -> Option<Done> {
    let (workspace, dir) = {
        let core = shared.lock();
        let w = core
            .model
            .workspaces
            .iter()
            .find(|w| w.panes.iter().any(|p| p.id == *pane))?;
        core.loops.get(&w.id).filter(|l| l.on && l.pane == *pane)?;
        (w.id.clone(), PathBuf::from(&w.path))
    };
    let path = identity::tasks_path(&dir);
    let list = std::fs::read_to_string(&path)
        .map(|t| tasks::parse(&t))
        .unwrap_or_default();
    let mut core = shared.lock();
    let lp = core
        .loops
        .get_mut(&workspace)
        .filter(|l| l.on && l.pane == *pane)?;
    let (laps, max, last, same) = (lp.laps, lp.max, lp.task.clone(), lp.same);
    let decision = if list.is_list() {
        decide(&list, laps, max, last.as_deref(), same)
    } else {
        Decision::Stop(StopKind::Idle, "the task list is gone".into())
    };
    core.task_look_soon(&workspace);
    match decision {
        Decision::Hand(id) => {
            let lp = core.loops.get_mut(&workspace)?;
            lp.same = if last.as_deref() == Some(id.as_str()) {
                same + 1
            } else {
                1
            };
            lp.laps += 1;
            lp.task = Some(id.clone());
            let note = format!("Task loop, lap {}/{}: {id}", lp.laps, lp.max);
            crate::agents::settle(&mut core, pane, None);
            let task = list.tasks.iter().find(|t| t.id == id)?;
            core.conv_note(pane, format!("{note} {}", task.title));
            core.refresh();
            Some(Ok(Some(
                json!({ "block": { "reason": reason(task, &path) } }),
            )))
        }
        Decision::Stop(kind, text) => {
            let lp = core.loops.get_mut(&workspace)?;
            lp.on = false;
            lp.task = None;
            lp.ended = kind != StopKind::Done;
            core.conv_note(pane, format!("Task loop stopped: {text}"));
            let lp = core.loops.get_mut(&workspace)?;
            lp.stop = Some(LoopStop { kind, text });
            let raise = if kind == StopKind::Done {
                AttentionKind::AgentDone
            } else {
                AttentionKind::Question
            };
            crate::agents::settle(&mut core, pane, Some(raise));
            core.refresh();
            Some(Ok(None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(text: &str) -> tasks::List {
        tasks::parse(text)
    }

    fn stop(kind: StopKind, starts: &str, d: Decision) {
        match d {
            Decision::Stop(k, text) => {
                assert_eq!(k, kind, "{text}");
                assert!(text.starts_with(starts), "{text}");
            }
            other => panic!("wanted a stop for {kind:?}, got {other:?}"),
        }
    }

    /// REQ-922: the order of the decision, one case for each step and the ones that beat each other.
    #[test]
    fn the_end_of_a_turn_is_decided_in_the_order_of_the_spec() {
        let open = list("- [x] t1 one\n- [ ] t2 two (after: t1)\n- [ ] t3 three (after: t2)\n");
        assert_eq!(
            decide(&open, 1, 20, Some("t1"), 1),
            Decision::Hand("t2".into())
        );
        // everything done, and in what lap
        let all = list("- [x] a x\n- [x] b y\n- [x] c z\n");
        stop(
            StopKind::Done,
            "all 3 tasks done in 2 laps",
            decide(&all, 2, 20, Some("c"), 1),
        );
        stop(
            StopKind::Done,
            "all 3 tasks done in 1 lap",
            decide(&all, 1, 20, Some("c"), 1),
        );
        // done beats the maximum
        stop(StopKind::Done, "all 3", decide(&all, 20, 20, Some("c"), 1));
        // a blocked task stops it, with its reason, even when another task is ready, and beats the maximum
        let blocked = list("- [ ] a x\n- [!] b y (blocked: no key)\n");
        stop(
            StopKind::Blocked,
            "b blocked: no key",
            decide(&blocked, 1, 20, None, 0),
        );
        stop(
            StopKind::Blocked,
            "b blocked",
            decide(&blocked, 20, 20, None, 0),
        );
        stop(
            StopKind::Blocked,
            "b blocked: no reason given",
            decide(&list("- [!] b y\n"), 1, 20, None, 0),
        );
        // nothing ready: a cycle, or a wait for what is not in the list
        stop(
            StopKind::Idle,
            "nothing is ready",
            decide(
                &list("- [ ] a x (after: b)\n- [ ] b y (after: a)\n"),
                1,
                20,
                None,
                0,
            ),
        );
        stop(
            StopKind::Idle,
            "nothing is ready",
            decide(&list("- [ ] a x (after: zz)\n"), 1, 20, None, 0),
        );
        // the laps used up, with what is left
        stop(
            StopKind::Max,
            "the maximum of 5 laps, with 2 tasks left",
            decide(&open, 5, 5, Some("t1"), 1),
        );
        assert_eq!(
            decide(&open, 4, 5, Some("t1"), 1),
            Decision::Hand("t2".into())
        );
        // the same task handed out three times in a row and not done: the fourth lap is not given
        assert_eq!(
            decide(&open, 1, 20, Some("t2"), 1),
            Decision::Hand("t2".into())
        );
        assert_eq!(
            decide(&open, 2, 20, Some("t2"), 2),
            Decision::Hand("t2".into())
        );
        stop(
            StopKind::Stuck,
            "t2 was handed out 3 times",
            decide(&open, 3, 20, Some("t2"), 3),
        );
        // another task is never stuck, whatever the count was
        assert_eq!(
            decide(&open, 3, 20, Some("t1"), 3),
            Decision::Hand("t2".into())
        );
        // the first ready task of the file is the one handed out
        assert_eq!(
            decide(&list("- [ ] z last\n- [ ] a first\n"), 0, 20, None, 0),
            Decision::Hand("z".into())
        );
    }

    #[test]
    fn what_the_agent_is_told_names_the_file_the_task_and_never_to_ask() {
        let l = list(
            "- [x] t1 one\n- [ ] t2 endpoints (after: t1)\n- [ ] fin run the tests (after: *)\n",
        );
        let path = Path::new("/w/list-file");
        let b = &identity::get().name;
        let text = reason(&l.tasks[1], path);
        assert!(
            text.contains("/w/list-file") && text.contains("t2 endpoints (after: t1)"),
            "{text}"
        );
        assert!(
            text.contains(&format!("`{b} tasks done t2`")) && text.contains("end your turn"),
            "{text}"
        );
        assert!(
            text.contains(&format!("do not ask: run `{b} tasks block t2")),
            "{text}"
        );
        let last = reason(&l.tasks[2], path);
        assert!(
            last.starts_with("Last task of /w/list-file: run the tests"),
            "{last}"
        );
        assert!(
            last.contains(&format!("`{b} tasks note fin"))
                && last.contains(&format!("`{b} tasks done fin`")),
            "{last}"
        );
        assert!(
            last.contains(&format!("do not ask: run `{b} tasks block fin")),
            "{last}"
        );
    }
}
