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
const RESERVED: [&str; 15] = [
    "new", "session", "sessions", "ls", "read", "pane", "worktree", "prompt", "hook", "daemon",
    "help", "version", "skill", "server", "theme",
];

static CURRENT: Mutex<String> = Mutex::new(String::new());

/// The session this process works in: the one set, else `$JW_SESSION` (a
/// pane's, followed through renames since the pane started), else `main`.
pub fn current() -> String {
    let set = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !set.is_empty() {
        return set;
    }
    let Some(env) = std::env::var("JW_SESSION")
        .ok()
        .filter(|s| valid(s) || s == MAIN)
    else {
        return MAIN.to_string();
    };
    match registry::state_dir() {
        Ok(state) => follow(&state, &env),
        Err(_) => env,
    }
}

/// Where a renamed session went: `renamed.json` maps old names to new ones.
fn follow(state: &Path, name: &str) -> String {
    let map = renames(state);
    let mut name = name.to_string();
    for _ in 0..16 {
        match map.get(&name) {
            Some(next) => name = next.clone(),
            None => break,
        }
    }
    name
}

fn renames(state: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(state.join("sessions").join("renamed.json"))
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default()
}

/// Renames session `old` to `new`: its folder, its worktrees in the
/// registry, its recent list, the last one used. Returns the workspace ids
/// that changed (its folders'), for the daemon to follow.
pub fn rename(state: &Path, old: &str, new: &str) -> Result<Vec<(String, String)>> {
    if !valid(new) {
        bail!("invalid session name {new:?}: lowercase letters, digits and dashes, at most 24");
    }
    if new == old {
        return Ok(Vec::new());
    }
    if exists(state, new)? {
        bail!("session {new} already exists");
    }
    let from = dir(state, old)?;
    if !from.is_dir() {
        bail!("no session {old}");
    }
    let to = dir(state, new)?;
    std::fs::rename(&from, &to)
        .with_context(|| format!("{} → {}", from.display(), to.display()))?;

    let folders = crate::folders::Folders::load(&to.join("folders.json"))?;
    let ids: Vec<(String, String)> = folders
        .folders
        .iter()
        .map(|f| {
            (
                crate::folders::id_in(old, &f.dir),
                crate::folders::id_in(new, &f.dir),
            )
        })
        .collect();
    let recent: Vec<String> = std::fs::read(to.join("recent.json"))
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    let recent: Vec<String> = recent
        .into_iter()
        .map(|id| {
            ids.iter()
                .find(|(a, _)| *a == id)
                .map_or(id, |(_, b)| b.clone())
        })
        .collect();
    registry::write_atomic(&to.join("recent.json"), &serde_json::to_vec(&recent)?)?;

    let reg_path = state.join("workspaces.json");
    let mut reg = registry::Registry::load(&reg_path)?;
    let mut moved = false;
    for e in reg.entries.iter_mut().filter(|e| owns(old, &e.session)) {
        e.session = stored(new);
        moved = true;
    }
    if moved {
        reg.save(&reg_path)?;
    }

    let was_last =
        std::fs::read_to_string(root(state)?.join("last")).is_ok_and(|l| l.trim() == old);
    if was_last {
        set_last(state, new)?;
    }
    // Panes started before keep `$JW_SESSION=old`: point it here.
    let mut map = renames(state);
    for v in map.values_mut() {
        if v == old {
            *v = new.to_string();
        }
    }
    map.insert(old.to_string(), new.to_string());
    map.remove(new);
    registry::write_atomic(
        &root(state)?.join("renamed.json"),
        &serde_json::to_vec_pretty(&map)?,
    )?;
    Ok(ids)
}

/// What removing a session leaves behind (REQ-273): its worktrees are only
/// forgotten, never deleted, so the plan says which of them hold work that
/// exists nowhere else.
#[derive(Debug, Default)]
pub struct RmPlan {
    /// The ids of the session's folder workspaces, for the daemon to close.
    pub folders: Vec<String>,
    /// Each worktree of the session with what git says about it.
    pub worktrees: Vec<(registry::Entry, crate::actions::RmPlan)>,
}

impl RmPlan {
    /// The workspace ids of the session: its folders' and its worktrees'.
    pub fn streams(&self) -> impl Iterator<Item = &str> {
        self.folders
            .iter()
            .map(String::as_str)
            .chain(self.worktrees.iter().map(|(e, _)| e.id.as_str()))
    }

    /// The worktrees with dirty or unpushed work.
    pub fn at_risk(&self) -> impl Iterator<Item = &(registry::Entry, crate::actions::RmPlan)> {
        self.worktrees.iter().filter(|(_, p)| p.loses_work())
    }
}

/// Reads what removing session `name` would close and forget. Changes nothing.
pub fn rm_plan(state: &Path, name: &str) -> Result<RmPlan> {
    if !exists(state, name)? {
        bail!("no session {name}");
    }
    let folders = crate::folders::Folders::load(&dir(state, name)?.join("folders.json"))?;
    let reg = registry::Registry::load(&state.join("workspaces.json"))?;
    let mut worktrees = Vec::new();
    for e in reg.entries.into_iter().filter(|e| owns(name, &e.session)) {
        // Nothing here deletes a branch, so whether it exists doesn't matter.
        let plan = match crate::actions::worktree_state(&e) {
            Ok(git) => crate::actions::rm_plan_from(&e, git, false),
            // A checkout git can't read must not block the removal, nor pass
            // as clean: it asks for the confirmation.
            Err(err) => crate::actions::RmPlan {
                on_disk: true,
                dirty: vec![format!("git could not read it: {err}")],
                ..Default::default()
            },
        };
        worktrees.push((e, plan));
    }
    Ok(RmPlan {
        folders: folders
            .folders
            .iter()
            .map(|f| crate::folders::id_in(name, &f.dir))
            .collect(),
        worktrees,
    })
}

/// Removes session `name` from the state dir: its worktrees' registry
/// entries (the checkouts and branches stay), its `sessions/<name>/`, the
/// renames that lead to it and the last-used pointer. `current` is the
/// session the caller works in, which can't go (REQ-271). The caller closes
/// the session's workspaces in the daemon first.
pub fn rm(state: &Path, name: &str, current: &str) -> Result<()> {
    if name == current {
        bail!("cannot remove {name}: it is the session you are in");
    }
    if !valid(name) && name != MAIN {
        bail!("invalid session name {name:?}");
    }
    if !exists(state, name)? {
        bail!("no session {name}");
    }

    let reg_path = state.join("workspaces.json");
    let mut reg = registry::Registry::load(&reg_path)?;
    let before = reg.entries.len();
    reg.entries.retain(|e| !owns(name, &e.session));
    if reg.entries.len() != before {
        reg.save(&reg_path)?;
    }

    let d = dir(state, name)?;
    std::fs::remove_dir_all(&d).with_context(|| d.display().to_string())?;

    // A name that led here, however many renames ago, must not lead a later
    // session of the same name astray.
    let mut map = renames(state);
    let mut gone = vec![name.to_string()];
    while let Some(g) = gone.pop() {
        let from: Vec<String> = map
            .iter()
            .filter(|(k, v)| **v == g || **k == g)
            .map(|(k, _)| k.clone())
            .collect();
        for k in from {
            if map.remove(&k).is_some() {
                gone.push(k);
            }
        }
    }
    let renamed = root(state)?.join("renamed.json");
    if renamed.exists() {
        registry::write_atomic(&renamed, &serde_json::to_vec_pretty(&map)?)?;
    }

    let last = root(state)?.join("last");
    if std::fs::read_to_string(&last).is_ok_and(|l| l.trim() == name) {
        std::fs::remove_file(&last).with_context(|| last.display().to_string())?;
    }
    Ok(())
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
                std::fs::rename(&old, main.join(f)).with_context(|| old.display().to_string())?;
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
    let root = root(state)?;
    let mut out: Vec<String> = std::fs::read_dir(&root)
        .with_context(|| root.display().to_string())?
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
    std::fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
    let path = folder.display().to_string();
    let mut all = crate::folders::Folders::default();
    all.add(&path);
    if let Some(f) = all.folders.last_mut() {
        f.plain = true;
    }
    all.save(&d.join("folders.json"))?;
    let id = crate::folders::id_in(name, &path);
    registry::write_atomic(&d.join("recent.json"), &serde_json::to_vec(&[id])?)?;
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
    registry::write_atomic(&root(state)?.join("last"), format!("{name}\n").as_bytes())
}

/// "1 workspace", "3 workspaces".
pub fn workspaces(n: usize) -> String {
    if n == 1 {
        "1 workspace".into()
    } else {
        format!("{n} workspaces")
    }
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
    fn a_renamed_session_keeps_its_workspaces() {
        let state = tempfile::tempdir().unwrap();
        let s = state.path();
        let work = tempfile::tempdir().unwrap();
        create(s, "brain", work.path()).unwrap();
        let mut reg = registry::Registry::default();
        reg.add(registry::Entry {
            id: "wt".into(),
            session: "brain".into(),
            ..Default::default()
        });
        reg.save(&s.join("workspaces.json")).unwrap();

        let dir = work.path().display().to_string();
        let ids = rename(s, "brain", "ops").unwrap();
        assert_eq!(
            ids,
            [(
                crate::folders::id_in("brain", &dir),
                crate::folders::id_in("ops", &dir)
            )]
        );
        assert_eq!(list(s).unwrap(), [MAIN, "ops"]);
        assert_eq!(last(s), "ops");
        let reg = registry::Registry::load(&s.join("workspaces.json")).unwrap();
        assert_eq!(reg.entries[0].session, "ops");
        let recent: Vec<String> =
            serde_json::from_slice(&std::fs::read(s.join("sessions/ops/recent.json")).unwrap())
                .unwrap();
        assert_eq!(recent, [crate::folders::id_in("ops", &dir)]);
        assert_eq!(follow(s, "brain"), "ops");
        assert!(rename(s, "ops", "main").is_err());
        assert!(rename(s, "ops", "Bad").is_err());

        // main itself can be renamed; its worktrees go with it.
        rename(s, MAIN, "home").unwrap();
        assert_eq!(follow(s, "main"), "home");
        assert_eq!(follow(s, "brain"), "ops");
    }

    fn entry(id: &str, session: &str, path: &str) -> registry::Entry {
        registry::Entry {
            id: id.into(),
            session: session.into(),
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn rm_forgets_the_session_and_nothing_else() {
        let state = tempfile::tempdir().unwrap();
        let s = state.path();
        let work = tempfile::tempdir().unwrap();
        create(s, "keep", work.path()).unwrap();
        create(s, "dev", work.path()).unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let mut reg = registry::Registry::default();
        reg.add(entry("a", "dev", &checkout.path().display().to_string()));
        reg.add(entry("b", "keep", ""));
        reg.add(entry("c", "", ""));
        reg.save(&s.join("workspaces.json")).unwrap();

        let plan = rm_plan(s, "dev").unwrap();
        assert_eq!(plan.worktrees.len(), 1);
        assert_eq!(plan.folders.len(), 1);
        assert_eq!(plan.streams().count(), 2);
        // Not a git checkout: unreadable counts as at risk, not as clean.
        assert_eq!(plan.at_risk().count(), 1);

        assert!(rm(s, "dev", "keep").is_ok());
        assert_eq!(list(s).unwrap(), ["keep", MAIN]);
        let reg = registry::Registry::load(&s.join("workspaces.json")).unwrap();
        let ids: Vec<&str> = reg.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        // The checkout is still there (REQ-273).
        assert!(checkout.path().is_dir());
        // `last` pointed at dev, the latest created.
        assert_eq!(last(s), MAIN);
        assert!(rm(s, "dev", "keep").is_err());
    }

    #[test]
    fn rm_refuses_the_current_session_and_takes_main_like_any_other() {
        let state = tempfile::tempdir().unwrap();
        let s = state.path();
        let work = tempfile::tempdir().unwrap();
        create(s, "ops", work.path()).unwrap();
        assert!(rm(s, "ops", "ops").is_err());
        assert!(rm(s, "../x", "ops").is_err());
        assert!(list(s).unwrap().contains(&"ops".to_string()));
        // `main` was made by the migration; nothing keeps it alive.
        std::fs::create_dir_all(s.join("sessions/main")).unwrap();
        assert!(rm(s, MAIN, "ops").is_ok());
        assert_eq!(list(s).unwrap(), ["ops"]);
    }

    #[test]
    fn rm_drops_the_renames_that_lead_to_it() {
        let state = tempfile::tempdir().unwrap();
        let s = state.path();
        let work = tempfile::tempdir().unwrap();
        create(s, "one", work.path()).unwrap();
        create(s, "keep", work.path()).unwrap();
        rename(s, "one", "two").unwrap();
        rename(s, "two", "three").unwrap();
        assert_eq!(follow(s, "one"), "three");
        rm(s, "three", "keep").unwrap();
        assert_eq!(follow(s, "one"), "one");
        assert_eq!(follow(s, "two"), "two");
        assert_eq!(follow(s, "three"), "three");
    }

    #[test]
    fn main_is_the_empty_session_in_the_registry() {
        assert!(owns(MAIN, ""));
        assert!(owns("testing", "testing"));
        assert!(!owns(MAIN, "testing"));
    }
}
