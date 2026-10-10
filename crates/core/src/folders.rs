//! The folders the user opened as workspaces (docs/specs/rust-tui.md,
//! Interfaces → Folders): project roots and plain folders, kept in
//! `sessions/<session>/folders.json` in the state dir. A worktree is not a folder here: those
//! live in the registry and show under their project's folder.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::connectors::git::{self, Repo};
use crate::core::config::Config;
use crate::core::registry::{self, Entry};

/// A folder's workspace id is its path behind this prefix: the same folder
/// is the same workspace, whether it came from `folders.json` or from the
/// worktrees of its project. Outside `main` the session follows, so one
/// folder open in two sessions is two workspaces (RISK-22).
pub const ID_PREFIX: &str = "dir:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub dir: String,
    /// An agent ran here before: resume it.
    #[serde(default)]
    pub opened: bool,
    pub created: String,
    /// The id of the claude conversation in its agent pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Opens with one shell, whatever its layout: the folder a session
    /// started in (REQ-61).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plain: bool,
    /// The name jw shows for it; the folder on disk keeps its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Folders {
    #[serde(default)]
    pub folders: Vec<Folder>,
}

/// `folders.json` of the current session.
pub fn default_path() -> Result<PathBuf> {
    path(&registry::state_dir()?)
}

fn path(state: &Path) -> Result<PathBuf> {
    Ok(crate::session::dir(state, &crate::session::current())?.join("folders.json"))
}

/// The workspace id of `dir` in the current session.
pub fn id(dir: &str) -> String {
    id_in(&crate::session::current(), dir)
}

pub fn id_in(session: &str, dir: &str) -> String {
    if session == crate::session::MAIN {
        format!("{ID_PREFIX}{dir}")
    } else {
        format!("{ID_PREFIX}{dir}#{session}")
    }
}

pub fn is_folder(e: &Entry) -> bool {
    e.id.starts_with(ID_PREFIX)
}

impl Folders {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(d) => {
                serde_json::from_slice(&d).with_context(|| format!("parse {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| path.display().to_string()),
        }
    }

    /// Temp file and rename, like the registry.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Adds `dir` at the end unless it is there; true when it was added.
    pub fn add(&mut self, dir: &str) -> bool {
        if self.folders.iter().any(|f| f.dir == dir) {
            return false;
        }
        self.folders.push(Folder {
            dir: dir.to_string(),
            opened: false,
            created: registry::now_rfc3339(),
            agent: None,
            plain: false,
            name: None,
        });
        true
    }
}

/// `folders.json`, after moving any free sessions of an older jw into it
/// (REQ-56): each one's directory becomes a folder and `free.json` is
/// renamed `free.json.migrated`.
pub fn load(state: &Path) -> Result<Folders> {
    let path = path(state)?;
    let mut all = Folders::load(&path)?;
    let free = state.join("free.json");
    if let Ok(data) = std::fs::read(&free) {
        #[derive(Deserialize)]
        struct Old {
            #[serde(default)]
            sessions: Vec<OldSession>,
        }
        #[derive(Deserialize)]
        struct OldSession {
            dir: String,
            #[serde(default)]
            opened: bool,
        }
        let old: Old =
            serde_json::from_slice(&data).with_context(|| format!("parse {}", free.display()))?;
        for s in old.sessions {
            let dir = s.dir.trim_end_matches('/').to_string();
            if all.add(&dir)
                && s.opened
                && let Some(f) = all.folders.last_mut()
            {
                f.opened = true;
            }
        }
        all.save(&path)?;
        std::fs::rename(&free, state.join("free.json.migrated"))?;
    }
    Ok(all)
}

/// Changes the current session's `folders.json`.
pub fn edit(f: impl FnOnce(&mut Folders)) -> Result<()> {
    let state = registry::state_dir()?;
    let mut all = load(&state)?;
    f(&mut all);
    all.save(&path(&state)?)
}

/// The repository whose main checkout is exactly `dir`, if any: a folder
/// inside a repository, or one without an origin remote, is a plain folder.
pub fn repo(dir: &Path) -> Option<Repo> {
    let repo = Repo::open(dir).ok()?;
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    same(&repo.root, dir).then_some(repo)
}

/// A folder as the sidebar and the actions see it. A repository's folder
/// carries its project name and the branch checked out; a plain folder has
/// no branch.
/// The workspace of a folder of session `session`, with its name and its
/// agent's conversation.
pub fn entry_for(f: &Folder, session: &str) -> Entry {
    let mut e = entry(&f.dir, f.opened);
    e.id = id_in(session, &f.dir);
    e.agent = f.agent.clone().unwrap_or_default();
    if let Some(n) = &f.name {
        e.name = n.clone();
    }
    e
}

pub fn entry(dir: &str, opened: bool) -> Entry {
    let path = Path::new(dir);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string());
    let (project, branch) = match repo(path) {
        Some(r) => (
            git::project_name(&r.remote),
            git::current_branch(path).unwrap_or_default(),
        ),
        None => (name.clone(), String::new()),
    };
    Entry {
        id: id(dir),
        name,
        project,
        branch,
        path: dir.to_string(),
        opened,
        ..Entry::default()
    }
}

/// The config a folder without its own runs with: jw's defaults.
pub fn config() -> Config {
    let mut cfg = Config::default();
    cfg.agent.default = "claude".into();
    cfg.agent.claude.start = "claude".into();
    cfg.agent.claude.resume = "claude --continue".into();
    cfg.sync = "rebase".into();
    cfg
}

/// `~` and `~/…` to the home directory.
pub fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix('~'), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_sessions_become_folders_once() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path();
        std::fs::write(
            state.join("free.json"),
            r#"{"sessions":[
                {"id":"a","name":"x","dir":"/tmp/one/","layout":"shell-only","opened":true,"created":"t"},
                {"id":"b","name":"y","dir":"/tmp/two","created":"t"}
            ]}"#,
        )
        .unwrap();
        let all = load(state).unwrap();
        let dirs: Vec<(&str, bool)> = all
            .folders
            .iter()
            .map(|f| (f.dir.as_str(), f.opened))
            .collect();
        assert_eq!(dirs, [("/tmp/one", true), ("/tmp/two", false)]);
        assert!(!state.join("free.json").exists());
        assert!(state.join("free.json.migrated").exists());
        // Already moved: a second load reads folders.json as it is.
        assert_eq!(load(state).unwrap(), all);
    }

    #[test]
    fn add_keeps_one_of_each() {
        let mut all = Folders::default();
        assert!(all.add("/a"));
        assert!(!all.add("/a"));
        assert!(all.add("/b"));
        assert_eq!(all.folders.len(), 2);
    }

    #[test]
    fn a_plain_folder_has_no_branch() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_str().unwrap();
        let e = entry(dir, false);
        assert!(is_folder(&e));
        // The id is in the current session, which `$JW_SESSION` sets when
        // the tests run inside a jw pane.
        assert_eq!(e.id, id(dir));
        assert_eq!(id_in(crate::session::MAIN, dir), format!("dir:{dir}"));
        assert_eq!(id_in("other", dir), format!("dir:{dir}#other"));
        assert_eq!(e.project, e.name);
        assert!(e.branch.is_empty());
    }
}
