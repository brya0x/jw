//! Named sessions (docs/specs/rust-tui.md, addendum 3): each one is its own
//! list of workspaces. Its folders and its recent list live in
//! `sessions/<name>/` in the state dir; its worktrees are the registry
//! entries with its name. The daemon doesn't know about sessions: a
//! workspace's id is unique across them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};

use crate::core::registry;

/// The session every jw had before sessions: today's state moves into it.
pub const MAIN: &str = "main";

/// Words `jw <name>` can't take, since they are commands.
const RESERVED: [&str; 10] = [
    "new", "sessions", "ls", "read", "worktree", "prompt", "hook", "daemon", "help", "version",
];

static CURRENT: Mutex<String> = Mutex::new(String::new());

/// The session this process works in: the one set, else `$JW_SESSION` (a
/// pane's), else `main`.
pub fn current() -> String {
    let set = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !set.is_empty() {
        return set;
    }
    std::env::var("JW_SESSION")
        .ok()
        .filter(|s| valid(s))
        .unwrap_or_else(|| MAIN.to_string())
}

pub fn set(name: &str) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = name.to_string();
}

/// How the registry records a session: `main` is the empty string, so
/// entries from before sessions belong to it.
pub fn stored(name: &str) -> String {
    if name == MAIN {
        String::new()
    } else {
        name.to_string()
    }
}

/// Whether a registry entry's session is `name`.
pub fn owns(name: &str, entry_session: &str) -> bool {
    stored(name) == entry_session
}

pub fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 24
        && crate::actions::valid_name(name)
        && !RESERVED.contains(&name)
}

/// `sessions/` in the state dir, made on first use: today's
/// `folders.json` and `recent.json` move into `sessions/main/` (REQ-65).
pub fn root(state: &Path) -> Result<PathBuf> {
    let root = state.join("sessions");
    if !root.exists() {
        let main = root.join(MAIN);
        std::fs::create_dir_all(&main).with_context(|| main.display().to_string())?;
        for f in ["folders.json", "recent.json"] {
            let old = state.join(f);
            if old.exists() {
                std::fs::rename(&old, main.join(f))?;
            }
        }
    }
    Ok(root)
}

/// The directory of session `name`.
pub fn dir(state: &Path, name: &str) -> Result<PathBuf> {
    Ok(root(state)?.join(name))
}

/// Every session, by name.
pub fn list(state: &Path) -> Result<Vec<String>> {
    let mut out: Vec<String> = std::fs::read_dir(root(state)?)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid(n) || n == MAIN)
        .collect();
    out.sort();
    Ok(out)
}

pub fn exists(state: &Path, name: &str) -> Result<bool> {
    Ok(dir(state, name)?.is_dir())
}

/// Creates session `name` with one folder, `dir`, which opens with one
/// shell, and makes it the last one used (REQ-61).
pub fn create(state: &Path, name: &str, folder: &Path) -> Result<()> {
    if !valid(name) {
        bail!("invalid session name {name:?}: lowercase letters, digits and dashes, at most 24");
    }
    if exists(state, name)? {
        bail!("session {name} already exists: jw {name} opens it");
    }
    let d = dir(state, name)?;
    std::fs::create_dir_all(&d)?;
    let path = folder.display().to_string();
    let mut all = crate::folders::Folders::default();
    all.add(&path);
    if let Some(f) = all.folders.last_mut() {
        f.plain = true;
    }
    all.save(&d.join("folders.json"))?;
    let id = crate::folders::id_in(name, &path);
    std::fs::write(d.join("recent.json"), serde_json::to_vec(&[id])?)?;
    set_last(state, name)
}

/// The session `jw` with no name opens.
pub fn last(state: &Path) -> String {
    std::fs::read_to_string(root(state).map(|r| r.join("last")).unwrap_or_default())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| exists(state, s).unwrap_or(false))
        .unwrap_or_else(|| MAIN.to_string())
}

pub fn set_last(state: &Path, name: &str) -> Result<()> {
    std::fs::write(root(state)?.join("last"), format!("{name}\n"))?;
    Ok(())
}

/// The state dir of this jw.
pub fn state() -> Result<PathBuf> {
    registry::state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid("salesassist") && valid("hello-world"));
        assert!(!valid("") && !valid("New") && !valid("new") && !valid("ls"));
        assert!(!valid(&"a".repeat(25)));
    }

    #[test]
    fn main_takes_todays_state_then_sessions_are_separate() {
        let state = tempfile::tempdir().unwrap();
        let s = state.path();
        std::fs::write(s.join("folders.json"), r#"{"folders":[]}"#).unwrap();
        std::fs::write(s.join("recent.json"), "[]").unwrap();

        assert_eq!(list(s).unwrap(), [MAIN]);
        assert!(s.join("sessions/main/folders.json").exists());
        assert!(!s.join("folders.json").exists());
        assert_eq!(last(s), MAIN);

        let work = tempfile::tempdir().unwrap();
        create(s, "testing", work.path()).unwrap();
        assert_eq!(list(s).unwrap(), [MAIN, "testing"]);
        assert_eq!(last(s), "testing");
        assert!(create(s, "testing", work.path()).is_err());
        let f = crate::folders::Folders::load(&s.join("sessions/testing/folders.json")).unwrap();
        assert_eq!(f.folders.len(), 1);
        assert!(f.folders[0].plain);
    }

    #[test]
    fn main_is_the_empty_session_in_the_registry() {
        assert!(owns(MAIN, ""));
        assert!(owns("testing", "testing"));
        assert!(!owns(MAIN, "testing"));
    }
}
