//! The workspace the person was in, in `tui-last.json` under the state directory (spec REQ-662
//! to REQ-664). Nothing here fails: a missing, empty or unreadable file is no workspace, and a
//! write that cannot be made is dropped.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A workspace, by its id and by its folder: the id is the daemon's own and a session that was
/// lost makes new ones, the folder is what the person made.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Last {
    pub workspace: String,
    pub path: String,
}

pub fn path() -> PathBuf {
    identity::state_dir().join("tui-last.json")
}

/// The file's workspace, or none.
pub fn read(path: &Path) -> Option<Last> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write the file whole, through a rename, so a reader never sees half of it; a failure is
/// dropped, for the screen must not stop for it.
pub fn write(path: &Path, last: &Last) {
    let _ = try_write(path, last);
}

fn try_write(path: &Path, last: &Last) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(last)? + "\n")?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last() -> Last {
        Last {
            workspace: "w1".into(),
            path: "/tmp/w1".into(),
        }
    }

    #[test]
    fn what_is_written_is_read_back_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("jw-last-{}", std::process::id()));
        let file = dir.join("nested").join("tui-last.json");
        write(&file, &last());
        assert_eq!(read(&file), Some(last()));
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["tui-last.json"], "the rename left nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_empty_or_broken_file_is_no_workspace() {
        let dir = std::env::temp_dir().join(format!("jw-last-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir.join("none.json")), None);
        for (name, text) in [
            ("empty", ""),
            ("garbage", "{{ nope"),
            ("shape", r#"{"a":1}"#),
        ] {
            let file = dir.join(name);
            std::fs::write(&file, text).unwrap();
            assert_eq!(read(&file), None, "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_that_cannot_be_made_is_dropped() {
        let dir = std::env::temp_dir().join(format!("jw-last-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the directory should be.
        let blocker = dir.join("file");
        std::fs::write(&blocker, "").unwrap();
        write(&blocker.join("tui-last.json"), &last());
        assert_eq!(read(&blocker.join("tui-last.json")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
