//! The free space (REQ-23, REQ-24): sessions with no repository, branch or
//! ports, just a shell (and maybe an agent) in any directory.
//!
//! They live in their own file, free.json next to the registry, so
//! registry.json stays exactly what the Go binary reads. In the TUI a
//! session travels as a registry [`Entry`] whose project is [`PROJECT`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::core::config::Config;
use crate::core::registry::{self, Entry};
use crate::layout::{Dir, Node};

/// The project name free sessions carry as entries. Not a name a repository
/// can produce (project names come from the remote URL's last segment).
pub const PROJECT: &str = "free/";

/// What the sidebar calls the space.
pub const LABEL: &str = "free";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// Shell and agent side by side.
    #[default]
    ShellAgent,
    /// The agent on top, a full-width shell below.
    WideShell,
    ShellOnly,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::ShellAgent, Layout::WideShell, Layout::ShellOnly];

    pub fn label(self) -> &'static str {
        match self {
            Layout::ShellAgent => "shell + agent",
            Layout::WideShell => "agent over a wide shell",
            Layout::ShellOnly => "shell only",
        }
    }

    pub fn tree(self) -> Node {
        match self {
            Layout::ShellAgent => {
                Node::split(Dir::Right, 0.5, Node::leaf("shell"), Node::leaf("agent"))
            }
            Layout::WideShell => {
                Node::split(Dir::Down, 0.6, Node::leaf("agent"), Node::leaf("shell"))
            }
            Layout::ShellOnly => Node::leaf("shell"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub dir: String,
    #[serde(default)]
    pub layout: Layout,
    /// An agent ran here before: resume it.
    #[serde(default)]
    pub opened: bool,
    pub created: String,
}

impl Session {
    pub fn entry(&self) -> Entry {
        Entry {
            id: self.id.clone(),
            name: self.name.clone(),
            project: PROJECT.into(),
            path: self.dir.clone(),
            opened: self.opened,
            created: self.created.clone(),
            ..Entry::default()
        }
    }
}

pub fn is_free(e: &Entry) -> bool {
    e.project == PROJECT
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sessions {
    #[serde(default)]
    pub sessions: Vec<Session>,
}

pub fn default_path() -> Result<PathBuf> {
    Ok(registry::state_dir()?.join("free.json"))
}

impl Sessions {
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

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
}

/// Creates a session in `dir` (`~` expanded); the directory must exist.
pub fn create(path: &Path, name: &str, dir: &str, layout: Layout) -> Result<Session> {
    if !crate::actions::valid_name(name) {
        bail!("invalid name {name:?}: use lowercase letters, digits and dashes");
    }
    let dir = expand_home(dir.trim());
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut all = Sessions::load(path)?;
    if all.sessions.iter().any(|s| s.name == name) {
        bail!("free/{name} already exists");
    }
    let s = Session {
        id: registry::new_id()?,
        name: name.into(),
        dir: dir.display().to_string(),
        layout,
        opened: false,
        created: registry::now_rfc3339(),
    };
    all.sessions.push(s.clone());
    all.save(path)?;
    Ok(s)
}

/// Forgets a session; its directory is never touched.
pub fn remove(path: &Path, id: &str) -> Result<()> {
    let mut all = Sessions::load(path)?;
    all.sessions.retain(|s| s.id != id);
    all.save(path)
}

pub fn mark_opened(path: &Path, id: &str) -> Result<()> {
    let mut all = Sessions::load(path)?;
    if let Some(s) = all.sessions.iter_mut().find(|s| s.id == id)
        && !s.opened
    {
        s.opened = true;
        all.save(path)?;
    }
    Ok(())
}

/// The config a free session runs with: jw's defaults, no repo file.
pub fn config() -> Config {
    let mut cfg = Config::default();
    cfg.agent.default = "claude".into();
    cfg.agent.claude.start = "claude".into();
    cfg.agent.claude.resume = "claude --continue".into();
    cfg.sync = "rebase".into();
    cfg
}

fn expand_home(p: &str) -> PathBuf {
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
    fn create_list_remove() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("free.json");
        let s = create(
            &path,
            "scratch",
            d.path().to_str().unwrap(),
            Layout::WideShell,
        )
        .unwrap();
        assert!(
            create(&path, "scratch", "/", Layout::ShellOnly).is_err(),
            "names are unique"
        );
        assert!(create(&path, "x", "/nope/not/here", Layout::ShellOnly).is_err());
        assert!(create(&path, "Bad", "/", Layout::ShellOnly).is_err());

        mark_opened(&path, &s.id).unwrap();
        let all = Sessions::load(&path).unwrap();
        let got = all.get(&s.id).unwrap();
        assert!(got.opened);
        let e = got.entry();
        assert!(is_free(&e));
        assert_eq!(e.path, s.dir);

        remove(&path, &s.id).unwrap();
        assert!(Sessions::load(&path).unwrap().sessions.is_empty());
        assert!(d.path().exists(), "the directory stays");
    }

    #[test]
    fn layouts() {
        let roles =
            |l: Layout| -> Vec<String> { l.tree().leaves().into_iter().map(|x| x.role).collect() };
        assert_eq!(roles(Layout::ShellAgent), ["shell", "agent"]);
        assert_eq!(roles(Layout::WideShell), ["agent", "shell"]);
        assert_eq!(roles(Layout::ShellOnly), ["shell"]);
    }
}
