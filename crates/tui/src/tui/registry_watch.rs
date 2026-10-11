//! The files the sidebar is built from (addendum 14): `workspaces.json` and
//! the session's `folders.json`. Anyone can rewrite them (the CLI, another
//! TUI); a `stat` per file on the 2 s tick says when to read them again.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::core::registry;
use crate::folders;

type Stamp = (PathBuf, Option<SystemTime>);

pub struct RegistryWatch {
    seen: Vec<Stamp>,
}

fn stamp(p: PathBuf) -> Stamp {
    let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    (p, t)
}

fn paths() -> Vec<PathBuf> {
    [registry::default_path(), folders::default_path()]
        .into_iter()
        .flatten()
        .collect()
}

impl RegistryWatch {
    pub fn new() -> Self {
        Self::over(paths())
    }

    pub fn over(files: Vec<PathBuf>) -> Self {
        Self {
            seen: files.into_iter().map(stamp).collect(),
        }
    }

    /// Looks at the files again, for another session too, and takes their
    /// stamps as the ones seen. Called before the TUI reads them itself, so
    /// its own write is not taken for someone else's (RISK-48: the stamp
    /// goes before the read, never after).
    pub fn sync(&mut self) {
        *self = Self::new();
    }

    /// `stat` only: whether a file changed since the last look, and that
    /// look is now the one seen.
    pub fn changed(&mut self) -> bool {
        let now: Vec<Stamp> = self.seen.iter().map(|(p, _)| stamp(p.clone())).collect();
        let changed = now != self.seen;
        self.seen = now;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(p: &std::path::Path, secs: u64) {
        std::fs::write(p, b"{}").unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn quiet_until_a_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.json"), dir.path().join("b.json"));
        touch(&a, 100);
        let mut w = RegistryWatch::over(vec![a.clone(), b.clone()]);
        assert!(!w.changed());
        touch(&a, 200);
        assert!(w.changed());
        assert!(!w.changed(), "the change is reported once");
    }

    #[test]
    fn a_file_that_appears_or_goes_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.json");
        let mut w = RegistryWatch::over(vec![a.clone()]);
        touch(&a, 100);
        assert!(w.changed());
        std::fs::remove_file(&a).unwrap();
        assert!(w.changed());
        assert!(!w.changed());
    }
}
