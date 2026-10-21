//! What each pane's shell blocks were, kept on disk so they come back when the daemon does
//! (spec REQ-297 to REQ-303): `<state>/history/<pane id>.json`, one file for each pane, written
//! whole and atomically, readable by the person alone since a command's output may hold secrets.
//!
//! Only what a client would see is kept: the blocks, with a running one as it was. What a program
//! that had the whole screen drew is not, and the shell's own screen starts afresh.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use contract::Block;
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Saved {
    v: u32,
    /// When it was written, in Unix milliseconds: what a block that was running had run until.
    pub saved_at: u64,
    pub blocks: Vec<Block>,
}

pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("history")
}

/// The file of a pane; `None` for an id that is not one the daemon makes (it names no folder).
fn path(dir: &Path, pane: &str) -> Option<PathBuf> {
    let plain = !pane.is_empty() && pane.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then(|| dir.join(format!("{pane}.json")))
}

pub fn to_text(blocks: &[Block], now: u64) -> String {
    let saved = SavedRef {
        v: VERSION,
        saved_at: now,
        blocks,
    };
    serde_json::to_string(&saved).expect("blocks serialise")
}

#[derive(Serialize)]
struct SavedRef<'a> {
    v: u32,
    saved_at: u64,
    blocks: &'a [Block],
}

/// Write the file of a pane through a temporary one and a rename, so a crash never leaves half a
/// history.
pub fn save(dir: &Path, pane: &str, text: &str) -> io::Result<()> {
    let Some(file) = path(dir, pane) else {
        return Ok(());
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = file.with_extension("json.tmp");
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    out.write_all(text.as_bytes())?;
    drop(out);
    fs::rename(&tmp, &file)
}

/// The saved history of a pane. A file that cannot be read is moved aside to `<pane>.json.bad`
/// and reported, never overwritten silently.
pub fn load(dir: &Path, pane: &str) -> Option<Saved> {
    let file = path(dir, pane)?;
    let text = match fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("cannot read {}: {e}", file.display());
            return None;
        }
    };
    match serde_json::from_str::<Saved>(&text) {
        Ok(saved) if saved.v == VERSION => Some(saved),
        other => {
            let why = match other {
                Ok(saved) => format!("it is version {}, this daemon reads {VERSION}", saved.v),
                Err(e) => e.to_string(),
            };
            let bad = file.with_extension("json.bad");
            eprintln!(
                "{} is unusable ({why}); moved to {}",
                file.display(),
                bad.display()
            );
            let _ = fs::rename(&file, &bad);
            None
        }
    }
}

pub fn remove(dir: &Path, pane: &str) {
    if let Some(file) = path(dir, pane) {
        let _ = fs::remove_file(file);
    }
}

/// Take away the files of panes that are not in `keep`: they were closed while the daemon was
/// not there to say so.
pub fn prune(dir: &Path, keep: &HashSet<&str>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let pane = name.split('.').next().unwrap_or_default();
        if name.ends_with(".json") && !keep.contains(pane) || name.ends_with(".json.tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn block(id: u64) -> Block {
        Block {
            id,
            cmd: "ls".into(),
            cwd: "/w".into(),
            started: 5,
            running: false,
            code: Some(0),
            ms: Some(1),
            truncated: false,
            full: false,
            interrupted: false,
            restart: false,
            out: vec![],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jwn-history-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_history_is_written_for_the_person_alone_and_read_back() {
        let d = tmp("rw");
        save(&d, "p-1", &to_text(&[block(1), block(2)], 99)).unwrap();
        let file = d.join("p-1.json");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let saved = load(&d, "p-1").unwrap();
        assert_eq!((saved.saved_at, saved.blocks.len()), (99, 2));
        assert_eq!(saved.blocks[1], block(2));
        assert!(load(&d, "none").is_none());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_is_set_aside_not_overwritten() {
        let d = tmp("bad");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("p-1.json"), "not json").unwrap();
        assert!(load(&d, "p-1").is_none());
        assert!(!d.join("p-1.json").exists());
        assert_eq!(
            fs::read_to_string(d.join("p-1.json.bad")).unwrap(),
            "not json"
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_id_that_is_not_a_plain_one_names_no_file() {
        let d = tmp("id");
        for pane in ["", "../x", "a/b", "a.b"] {
            save(&d, pane, "{}").unwrap();
            assert!(load(&d, pane).is_none(), "{pane}");
        }
        assert!(!d.exists(), "nothing was made for them");
    }

    #[test]
    fn what_is_not_a_live_panes_is_pruned() {
        let d = tmp("prune");
        for pane in ["a", "b"] {
            save(&d, pane, &to_text(&[], 1)).unwrap();
        }
        fs::write(d.join("c.json.tmp"), "x").unwrap();
        prune(&d, &HashSet::from(["a"]));
        assert!(d.join("a.json").exists());
        assert!(!d.join("b.json").exists() && !d.join("c.json.tmp").exists());
        remove(&d, "a");
        assert!(!d.join("a.json").exists());
        fs::remove_dir_all(&d).unwrap();
    }
}
