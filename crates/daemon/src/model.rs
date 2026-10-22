//! What the daemon remembers. Node documents are built from this; `session.json` is this.

use std::collections::BTreeMap;
use std::path::Path;

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
    /// The groups of workspaces, in the order they are shown (REQ-684).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<contract::Group>,
    /// The group of each repository, by `Workspace::group_key` (REQ-685). It outlives the
    /// workspaces on purpose: opening one again finds its group.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grouped: BTreeMap<String, Id>,
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
    /// The diff and the viewers that are leaves of `layout` beside the panes. Not saved: a
    /// restart drops them (REQ-711).
    #[serde(skip)]
    pub views: Vec<View>,
    /// Where each kind of view was last closed, to put the next one back. Not saved.
    #[serde(skip)]
    pub places: std::collections::HashMap<contract::ViewKind, crate::layout::Place>,
    /// The worktree was there before jwn knew of it (N99): its setup is not run (REQ-536).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adopted: bool,
    /// The ports a worktree was given when it was made (REQ-495).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<PortBlock>,
    /// The workspace reviews a pull request (REQ-583).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<contract::Review>,
    /// What was last seen of the branch's pull request, so an attention is raised when it
    /// changes and not again while it does not. Not saved: a restart sees it afresh.
    #[serde(skip)]
    pub github: Option<crate::github::Pr>,
}

/// A leaf of a workspace's layout that is not a terminal (N165).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub id: Id,
    pub kind: contract::ViewKind,
    /// A viewer's tabs, relative paths, the oldest first, at most `MAX_TABS`.
    pub files: Vec<String>,
}

/// The tabs a viewer keeps (REQ-640).
pub const MAX_TABS: usize = 8;

impl Workspace {
    /// The repository the workspace belongs to: a worktree's, or a folder's own path when the
    /// folder is a git repository's root (REQ-222).
    pub fn project_path(&self) -> Option<String> {
        self.project.clone().or_else(|| {
            (self.kind == WorkspaceKind::Folder && crate::browse::is_git(Path::new(&self.path)))
                .then(|| self.path.clone())
        })
    }

    /// What a group is kept under (REQ-685): the repository, or the folder's own path when it
    /// is no repository. The summary's `project` and this are the same logic, so they agree.
    pub fn group_key(&self) -> String {
        key_from(self.project_path(), &self.path)
    }
}

/// The key of a workspace with that project and path (see `Workspace::group_key`).
pub fn key_from(project: Option<String>, path: &str) -> String {
    project.unwrap_or_else(|| path.to_owned())
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

impl Model {
    /// The group kept under `key` (see `Workspace::group_key`), when the session still lists it.
    pub fn group_for(&self, key: &str) -> Option<Id> {
        self.grouped
            .get(key)
            .filter(|g| self.groups.iter().any(|x| x.id == **g))
            .cloned()
    }

    /// The lowest slot no workspace holds, if its block fits below the last port (REQ-495).
    pub fn free_slot(&self) -> Option<u32> {
        let used: Vec<u32> = self
            .workspaces
            .iter()
            .filter_map(|w| w.ports.as_ref().map(|p| p.slot))
            .collect();
        free_slot(&used)
    }
}

/// The lowest slot of `used` that is free, if its block fits below the last port.
fn free_slot(used: &[u32]) -> Option<u32> {
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
    /// Where the shell is: the folder it started in, then the one it says at each prompt (REQ-626).
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
            groups: Vec::new(),
            grouped: BTreeMap::new(),
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
        assert!(!ws.adopted, "an old session has no worktree that existed");
        let mut ws = ws;
        ws.adopted = true;
        let back: Workspace = serde_json::from_str(&serde_json::to_string(&ws).unwrap()).unwrap();
        assert!(
            back.adopted,
            "REQ-536: it is saved, so a restart does not run its setup"
        );
    }

    #[test]
    fn a_session_without_groups_still_loads_and_groups_are_saved_only_when_there_are_some() {
        let old = r#"{"session_id":"s","name":"main","workspaces":[]}"#;
        let m: Model = serde_json::from_str(old).unwrap();
        assert!(m.groups.is_empty() && m.grouped.is_empty(), "REQ-685");
        let text = serde_json::to_string(&m).unwrap();
        assert!(!text.contains("groups") && !text.contains("grouped"));
        let mut m = m;
        m.groups.push(contract::Group {
            id: "g".into(),
            name: "salesassist".into(),
        });
        m.grouped.insert("/r/app".into(), "g".into());
        let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.groups[0].name, "salesassist");
        assert_eq!(back.grouped["/r/app"], "g");
    }
}
