//! The task lists the daemon serves (N180; REQ-890 to REQ-897): what it knows of each workspace's
//! list, the `tasks` node, and the two commands that open and change it. The file itself is
//! `tasks.rs`; every read and write here runs with the hub's lock released.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use contract::tasks::{
    LoopInfo, TaskItem, TaskOp, TaskPhase, TaskProblem, TaskStatus, Tasks, TasksSummary,
};
use contract::{ErrorCode, Head, Id};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::core::Core;
use crate::server::Shared;
use crate::tasks::{self, Edit, State, Status};

/// How long a list nobody asks for is left alone: the poll goes by every 2 seconds, so this is the
/// second poll after the last look, 6 seconds at most (REQ-893).
const SLOW: Duration = Duration::from_secs(5);

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

/// The node's document for a workspace (REQ-890).
fn node(head: Head, workspace: &Id, path: &Path, c: &Cached) -> Tasks {
    Tasks {
        head,
        workspace: workspace.clone(),
        path: path.display().to_string(),
        tasks: c
            .list
            .tasks
            .iter()
            .map(|t| item(t, c.auto.as_deref()))
            .collect(),
        problems: c.list.problems.iter().map(problem).collect(),
        loop_: LoopInfo {
            on: false,
            laps: 0,
            max: 20,
            task: None,
            pane: None,
            waiting: false,
            stop: None,
        },
    }
}

impl Core {
    /// What a workspace's row says of its list (REQ-891); the final task is counted (REQ-888).
    pub fn task_summary(&self, workspace: &str) -> Option<TasksSummary> {
        let c = self.task_lists.get(workspace)?;
        Some(TasksSummary {
            done: c.list.done() as u32,
            total: c.list.tasks.len() as u32,
            on: false,
            stop: None,
        })
    }

    /// The `tasks` node document of an entry, built from what was last read (no I/O).
    pub(crate) fn tasks_node(&self, id: &Id, e: &NodeEntry, head: Head) -> Option<Tasks> {
        let c = self.task_lists.get(&e.workspace)?;
        let dir = self.workspace_dir(&e.workspace)?;
        Some(node(
            Head {
                id: id.clone(),
                ..head
            },
            &e.workspace,
            &identity::tasks_path(&dir),
            c,
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

    /// The node of a workspace that has a list, made if it is not there (REQ-890).
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

    /// Look at this workspace's list at the next poll (REQ-894): an agent moved.
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

    /// The lists to look at in this round of the poll (REQ-893, REQ-894): the ones asked for, a
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

/// `open_tasks`: the node of the workspace's list, made if it is not there (REQ-890).
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

/// `tasks_edit`: the same change `jw tasks` makes (REQ-895, REQ-896), and the clients are told at once.
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
