//! The user's settings (docs/specs/rust-tui.md, addendum 3, OPEN-5):
//! `settings.json` in jw's config dir. JSON, so Go's `*.toml` glob over that
//! dir never reads it. Every key is optional; what is missing takes its
//! default. jw reads the file again when it changes (REQ-69).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// `C-Space`, `C-g`…: Ctrl and one key, never Alt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// `system`, `dark` or `light`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// The theme for dark mode and the one for light mode, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<String>,
    /// How long the leader waits before showing every key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub which_delay_ms: Option<u64>,
    /// Action → key, for the actions that can move (`ACTIONS`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
}

/// The actions a key after the leader can be given, by group: `(group,
/// action, what the popup says, default key)`. A key is one character, or
/// `tab`/`space`.
pub const ACTIONS: [(&str, &str, &str, &str); 16] = [
    ("go", "switch", "switch", "space"),
    ("go", "previous", "previous", "tab"),
    ("go", "open", "open folder", "o"),
    ("go", "file", "open file", "/"),
    ("go", "sessions", "sessions", "a"),
    ("go", "settings", "settings", ","),
    ("worktree", "new", "new", "w"),
    ("worktree", "rename", "rename", "r"),
    ("worktree", "sync", "sync", "s"),
    ("worktree", "diff", "changes", "d"),
    ("worktree", "remove", "remove", "X"),
    ("panes", "pane", "new", "t"),
    ("panes", "close", "close", "x"),
    ("panes", "name", "name", "n"),
    ("panes", "full", "full", "f"),
    ("panes", "layout", "save layout", "p"),
];

/// Keys that always mean the same: workspaces, focus, moving panes, help,
/// detach.
pub const FIXED: &str = "123456789hjklHJKL?q";

impl Settings {
    pub fn theme_mode(&self) -> &str {
        self.theme.as_deref().unwrap_or("system")
    }

    pub fn dark_theme(&self) -> &str {
        self.dark.as_deref().unwrap_or("one-dark")
    }

    pub fn light_theme(&self) -> &str {
        self.light.as_deref().unwrap_or("one-light")
    }

    pub fn which_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.which_delay_ms.unwrap_or(600))
    }

    /// The key of `action`: the user's, else the default.
    pub fn key(&self, action: &str) -> String {
        self.keys.get(action).cloned().unwrap_or_else(|| {
            ACTIONS
                .iter()
                .find(|a| a.1 == action)
                .map_or(String::new(), |a| a.3.to_string())
        })
    }

    /// The action `key` runs, if any.
    pub fn action(&self, key: &str) -> Option<&'static str> {
        ACTIONS.iter().map(|a| a.1).find(|a| self.key(a) == key)
    }

    /// Gives `action` the key `key`; an action that had it takes the old
    /// key of `action` (REQ-68). Refuses fixed keys and anything that isn't
    /// one plain key. The action that was swapped, if any.
    pub fn bind(&mut self, action: &str, key: &str) -> Result<Option<&'static str>, String> {
        if !ACTIONS.iter().any(|a| a.1 == action) {
            return Err(format!("no action {action}"));
        }
        let plain = key == "tab" || key == "space" || key.chars().count() == 1;
        if !plain {
            return Err("after the leader it is one plain key, no Ctrl or Alt".into());
        }
        if key.chars().count() == 1 && FIXED.contains(key) {
            return Err(format!("{key} is fixed (1–9, hjkl, HJKL, ?, q)"));
        }
        let old = self.key(action);
        let other = self.action(key).filter(|a| *a != action);
        if let Some(o) = other {
            self.set_key(o, &old);
        }
        self.set_key(action, key);
        Ok(other)
    }

    /// Stores a key, leaving defaults out of the file.
    fn set_key(&mut self, action: &str, key: &str) {
        let default = ACTIONS.iter().find(|a| a.1 == action).map(|a| a.3);
        if default == Some(key) {
            self.keys.remove(action);
        } else {
            self.keys.insert(action.to_string(), key.to_string());
        }
    }
}

/// How a key is shown: `␣`, `tab`, or itself.
pub fn label(key: &str) -> String {
    match key {
        "space" => "␣".into(),
        k => k.into(),
    }
}

pub fn path() -> Result<PathBuf> {
    Ok(crate::core::config::dir()?.join("settings.json"))
}

/// Reads `path`; a missing file is the defaults.
pub fn read(path: &std::path::Path) -> Result<Settings> {
    match std::fs::read(path) {
        Ok(d) => serde_json::from_slice(&d).with_context(|| format!("{}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}

/// Writes the settings (temp file and rename) and makes them current.
pub fn save(s: Settings) -> Result<()> {
    let path = path()?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&s)?)?;
    std::fs::rename(&tmp, &path)?;
    set(s);
    Ok(())
}

static CURRENT: RwLock<Option<Arc<Settings>>> = RwLock::new(None);

/// The settings in use.
pub fn get() -> Arc<Settings> {
    if let Some(s) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return Arc::clone(s);
    }
    let s = Arc::new(path().and_then(|p| read(&p)).unwrap_or_default());
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&s));
    s
}

pub fn set(s: Settings) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(s));
}

/// The files whose changes reload the settings: `settings.json` and the
/// themes it names.
pub struct Watch {
    seen: Vec<(PathBuf, Option<SystemTime>)>,
}

fn stamp(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Watch {
    pub fn new() -> Self {
        let mut w = Self { seen: Vec::new() };
        w.seen = w.files();
        w
    }

    fn files(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        let s = get();
        let mut files: Vec<PathBuf> = path().into_iter().collect();
        for name in [s.dark_theme(), s.light_theme()] {
            if let Ok(p) = crate::theme::file(name) {
                files.push(p);
            }
        }
        files
            .into_iter()
            .map(|p| {
                let t = stamp(&p);
                (p, t)
            })
            .collect()
    }

    /// Reads everything again when a file changed: `Ok(true)` when it did
    /// and applied, `Err` when it doesn't parse (the last good values stay).
    pub fn check(&mut self) -> Result<bool> {
        if self.seen.iter().all(|(p, t)| stamp(p) == *t) {
            return Ok(false);
        }
        let result = path().and_then(|p| read(&p));
        // Seen even when broken: the error shows once, not every tick.
        let s = match result {
            Ok(s) => s,
            Err(e) => {
                self.seen = self.files();
                return Err(e);
            }
        };
        let themes = crate::theme::load(&s);
        set(s);
        self.seen = self.files();
        themes?;
        Ok(true)
    }
}

impl Default for Watch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.key("switch"), "space");
        assert_eq!(s.theme_mode(), "system");
        assert_eq!(s.which_delay().as_millis(), 600);
        let s: Settings =
            serde_json::from_str(r#"{"keys": {"sessions": "g"}, "dark": "tokyo"}"#).unwrap();
        assert_eq!(s.action("g"), Some("sessions"));
        assert_eq!(s.action("a"), None);
        assert_eq!(s.dark_theme(), "tokyo");
        assert!(serde_json::from_str::<Settings>(r#"{"lead": "C-a"}"#).is_err());
    }

    #[test]
    fn rebinding_swaps_and_refuses() {
        let mut s = Settings::default();
        assert_eq!(s.bind("sessions", "o"), Ok(Some("open")));
        assert_eq!(s.key("sessions"), "o");
        assert_eq!(s.key("open"), "a");
        assert!(s.bind("sessions", "j").is_err());
        assert!(s.bind("sessions", "C-a").is_err());
        // Back to the defaults: nothing left in the file.
        s.bind("sessions", "a").unwrap();
        assert!(s.keys.is_empty(), "{:?}", s.keys);
        assert_eq!(s.bind("full", "space"), Ok(Some("switch")));
        assert_eq!(s.key("switch"), "f");
    }
}
