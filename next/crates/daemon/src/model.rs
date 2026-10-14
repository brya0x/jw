//! What the daemon remembers. Node documents are built from this; `session.json` is this.

use std::collections::BTreeMap;

use contract::{AttentionKind, Id, WorkspaceKind};
use serde::{Deserialize, Serialize};

pub fn new_id() -> Id {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub session_id: Id,
    pub name: String,
    pub workspaces: Vec<Workspace>,
    /// Files of a diff that a person looked at, by `<workspace id>|<scope key>`: each path
    /// with the file's `hash` at that moment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub viewed: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Id,
    pub name: String,
    pub kind: WorkspaceKind,
    pub path: String,
    /// The repository a worktree belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub panes: Vec<Pane>,
    #[serde(skip)]
    pub attention: Vec<Own>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: Id,
    pub title: String,
    /// The user's shell, always the pane's process.
    pub shell: String,
    /// Where the shell was started.
    pub cwd: String,
    #[serde(skip)]
    pub attention: Vec<Own>,
    /// The program running in the shell now. Not saved: it is looked at again after a restart.
    #[serde(skip)]
    pub running: Option<Vec<String>>,
    /// The program that ran last, kept so a restart can bring it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_program: Option<Vec<String>>,
    /// How to bring back the agent that was running when the daemon last saved: set while it
    /// runs, cleared when it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Vec<String>>,
}

/// An attention a node raised itself. Not saved: it describes what is happening now.
#[derive(Debug, Clone)]
pub struct Own {
    pub kind: AttentionKind,
    pub exit_code: Option<i32>,
    /// The revision of the node that first showed it; the hub fills this in.
    pub rev: Option<u64>,
}

impl Model {
    pub fn new() -> Model {
        Model {
            session_id: new_id(),
            name: "main".into(),
            workspaces: Vec::new(),
            viewed: BTreeMap::new(),
        }
    }
}
