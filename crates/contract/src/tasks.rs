//! Task lists (N180): a workspace's file of tasks, served as the `tasks` node (v1) and summed up in
//! the workspace's summary. What a task is and how it is written is `docs/spec.md`; the daemon reads
//! the file and the clients only draw this.

use serde::{Deserialize, Serialize};

use crate::{Head, Id};

/// What the file says of a task (`[ ]`, `[x]`, `[!]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    Done,
    Blocked,
}

/// What a task is at the moment; `running` is the loop's and is never in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Waiting,
    Ready,
    Running,
    Done,
    Blocked,
}

/// One task. `after` is the ids it waits for, or `["*"]` for every other task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskItem {
    pub id: String,
    pub title: String,
    pub after: Vec<String>,
    pub status: TaskStatus,
    pub state: TaskPhase,
    pub level: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The last task, which waits for every other: the tests and their evidence.
    #[serde(rename = "final", default, skip_serializing_if = "std::ops::Not::not")]
    pub is_final: bool,
    /// The final task is not in the file yet (it is added when the loop is turned on).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

/// Something wrong in the file that the daemon reports and does not mend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProblem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub text: String,
}

/// Why a loop is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Done,
    Blocked,
    Idle,
    Max,
    Stuck,
    Person,
    Pane,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopStop {
    pub kind: StopKind,
    pub text: String,
}

/// The loop of a workspace's agent over the list; it lives in the daemon's memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopInfo {
    pub on: bool,
    pub laps: u32,
    pub max: u32,
    /// The id of the task handed out now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The pane of the agent the loop is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<Id>,
    /// The agent waits for the person (a permission, a question or a plan).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<LoopStop>,
}

/// The `tasks` node (v1) of a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tasks {
    #[serde(flatten)]
    pub head: Head,
    pub workspace: Id,
    /// Where the list is, as the daemon names it.
    pub path: String,
    pub tasks: Vec<TaskItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<TaskProblem>,
    #[serde(rename = "loop")]
    pub loop_: LoopInfo,
}

/// What a workspace's row says of its list (`WorkspaceSummary.tasks`). `total` counts the final
/// task even while it is not in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksSummary {
    pub done: u32,
    pub total: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<LoopStop>,
}

/// What `tasks_edit` changes: a task's box, its blocked reason, or a note under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOp {
    Done,
    Block,
    Reopen,
    Note,
}
