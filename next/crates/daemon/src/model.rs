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
    /// How the panes are laid out; made to agree with `panes` whenever the model changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<contract::Layout>,
    #[serde(skip)]
    pub attention: Vec<Own>,
    /// The ports a worktree was given when it was made (REQ-495).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<PortBlock>,
    /// What was last seen of the branch's pull request, so an attention is raised when it
    /// changes and not again while it does not. Not saved: a restart sees it afresh.
    #[serde(skip)]
    pub github: Option<crate::github::Pr>,
}

/// A worktree's block of 100 ports: its slot and the names the settings gave to offsets in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBlock {
    pub slot: u32,
    pub names: std::collections::BTreeMap<String, u32>,
}

/// Where the blocks start, how many ports each has, and the last port there is.
const FIRST_PORT: u32 = 20_000;
const BLOCK: u32 = 100;
const LAST_PORT: u32 = 65_535;

impl PortBlock {
    /// The first port of the block.
    pub fn base(&self) -> u32 {
        FIRST_PORT + BLOCK * self.slot
    }

    /// Each name's port.
    pub fn ports(&self) -> std::collections::BTreeMap<String, u32> {
        let base = self.base();
        self.names
            .iter()
            .map(|(n, o)| (n.clone(), base + o))
            .collect()
    }

    /// What every pane of the workspace starts with (REQ-496).
    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![(identity::env_var("PORT_BASE"), self.base().to_string())];
        env.extend(self.ports().into_iter().map(|(name, port)| {
            (
                identity::env_var(&format!("PORT_{}", name.to_uppercase())),
                port.to_string(),
            )
        }));
        env
    }
}

/// The lowest slot of `used` that is free, if its block fits below the last port (REQ-495).
pub fn free_slot(used: &[u32]) -> Option<u32> {
    (0..)
        .find(|s| !used.contains(s))
        .filter(|s| FIRST_PORT + BLOCK * s + (BLOCK - 1) <= LAST_PORT)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: Id,
    /// What the pane is called: the person's name for it, else what runs in it.
    pub title: String,
    /// The name the person gave it; the title stays this whatever runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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

#[cfg(test)]
mod port_tests {
    use super::*;

    fn block(slot: u32) -> PortBlock {
        PortBlock {
            slot,
            names: [("web".to_owned(), 0), ("api".to_owned(), 7)].into(),
        }
    }

    #[test]
    fn a_block_starts_at_20000_plus_100_a_slot_and_each_name_is_an_offset_in_it() {
        let b = block(3);
        assert_eq!(b.base(), 20_300);
        assert_eq!(
            b.ports().into_iter().collect::<Vec<_>>(),
            [("api".to_owned(), 20_307), ("web".to_owned(), 20_300)]
        );
        let env: Vec<(String, String)> = b.env();
        assert_eq!(env[0], (identity::env_var("PORT_BASE"), "20300".to_owned()));
        assert!(env.contains(&(identity::env_var("PORT_WEB"), "20300".to_owned())));
        assert!(env.contains(&(identity::env_var("PORT_API"), "20307".to_owned())));
    }

    #[test]
    fn the_lowest_free_slot_is_given_and_none_when_the_block_would_pass_the_last_port() {
        assert_eq!(free_slot(&[]), Some(0));
        assert_eq!(free_slot(&[0, 1, 3]), Some(2), "a hole is filled first");
        assert_eq!(free_slot(&[1]), Some(0));
        let all: Vec<u32> = (0..454).collect();
        assert_eq!(
            free_slot(&all),
            Some(454),
            "20000 + 454 × 100 + 99 is 65499"
        );
        let all: Vec<u32> = (0..=454).collect();
        assert_eq!(free_slot(&all), None, "the next block would end at 65599");
    }

    #[test]
    fn the_block_is_saved_with_the_workspace_and_an_old_session_without_one_still_loads() {
        let b = block(2);
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<PortBlock>(&json).unwrap(), b);
        let ws: Workspace =
            serde_json::from_str(r#"{"id":"w","name":"n","kind":"folder","path":"/p","panes":[]}"#)
                .unwrap();
        assert!(ws.ports.is_none());
    }
}
