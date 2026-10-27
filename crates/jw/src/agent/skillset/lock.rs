//! What the skillset installed from outside, kept so that `off` removes only what jw wrote and only
//! when nobody edited it, and so that what another tool installed is never touched (N184b,
//! REQ-1016, REQ-1017).

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

/// What was installed for one entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Locked {
    pub kind: String,
    /// `<owner>/<repo> <path>`, as it was shown.
    pub source: String,
    pub commit: String,
    /// The folder the files are in (a role's: the user's roles folder).
    pub dir: String,
    /// Seconds since the epoch.
    pub installed_at: u64,
    /// Absolute path to the SHA-256 of its bytes.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Lock {
    pub v: u32,
    pub entries: BTreeMap<String, Locked>,
}

/// The SHA-256 of a file's bytes.
pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    web::auth::sha256_bytes_hex(bytes)
}

/// The lock at `path`; one that is not there is empty, one that cannot be read is an error.
pub(super) fn read(path: &Path) -> Result<Lock, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lock {
            v: 1,
            ..Lock::default()
        }),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write it whole, through a rename.
pub(super) fn write(path: &Path, lock: &Lock) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let text = serde_json::to_string_pretty(&Lock {
        v: 1,
        entries: lock.entries.clone(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text + "\n")
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Is `file` strictly inside `dir`, with no `..` on the way? The lock is a file of the person's
/// state directory: whatever it says, a path outside the entry's folder is never touched (REQ-1017).
pub(super) fn inside(file: &Path, dir: &Path) -> bool {
    file.is_absolute()
        && file != dir
        && file.starts_with(dir)
        && file
            .components()
            .all(|c| !matches!(c, Component::ParentDir))
}

/// How the files of an entry are on disk now.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Disk {
    /// Files with the recorded hash.
    pub same: Vec<String>,
    /// Files that are there with another hash, or are links.
    pub edited: Vec<String>,
    /// Files that are not there.
    pub gone: Vec<String>,
}

pub(super) fn disk_of(locked: &Locked) -> Disk {
    let mut disk = Disk::default();
    for (file, hash) in &locked.files {
        let path = Path::new(file);
        match std::fs::symlink_metadata(path) {
            Err(_) => disk.gone.push(file.clone()),
            Ok(m) if m.is_file() && std::fs::read(path).is_ok_and(|b| &hash_bytes(&b) == hash) => {
                disk.same.push(file.clone())
            }
            Ok(_) => disk.edited.push(file.clone()),
        }
    }
    disk
}

/// Remove the empty folders from `file`'s own upward to `limit` included, never further and never a
/// folder with anything in it (`remove_dir` refuses those).
pub(super) fn remove_empty_dirs(files: &[&Path], limit: &Path) {
    let mut dirs: Vec<&Path> = files
        .iter()
        .flat_map(|f| f.ancestors().skip(1))
        .filter(|d| d.starts_with(limit))
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(dir: &Path, files: &[(&str, &[u8])]) -> Locked {
        Locked {
            kind: "skill".into(),
            dir: dir.to_string_lossy().into_owned(),
            files: files
                .iter()
                .map(|(n, b)| (dir.join(n).to_string_lossy().into_owned(), hash_bytes(b)))
                .collect(),
            ..Locked::default()
        }
    }

    #[test]
    fn a_lock_is_written_and_read_back_and_a_missing_one_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/lock");
        assert_eq!(read(&path).unwrap().entries.len(), 0);
        let mut lock = Lock::default();
        lock.entries
            .insert("x".into(), locked(dir.path(), &[("a", b"1")]));
        write(&path, &lock).unwrap();
        assert_eq!(read(&path).unwrap().entries, lock.entries);
        std::fs::write(&path, "not json").unwrap();
        assert!(read(&path).is_err());
    }

    #[test]
    fn only_a_path_strictly_inside_the_folder_without_dots_is_inside() {
        let dir = Path::new("/c/skills/x");
        assert!(inside(Path::new("/c/skills/x/SKILL.md"), dir));
        assert!(inside(Path::new("/c/skills/x/a/b.md"), dir));
        for bad in [
            "/c/skills/x",
            "/c/skills/xy/SKILL.md",
            "/c/skills/x/../y/SKILL.md",
            "/c/skills/y/SKILL.md",
            "/etc/passwd",
            "skills/x/SKILL.md",
        ] {
            assert!(!inside(Path::new(bad), dir), "{bad}");
        }
    }

    #[test]
    fn the_disk_says_which_files_are_the_same_edited_or_gone() {
        let dir = tempfile::tempdir().unwrap();
        let l = locked(
            dir.path(),
            &[
                ("same", b"1"),
                ("edited", b"2"),
                ("gone", b"3"),
                ("link", b"4"),
            ],
        );
        std::fs::write(dir.path().join("same"), "1").unwrap();
        std::fs::write(dir.path().join("edited"), "changed").unwrap();
        // A link with the right bytes behind it is not the file jw wrote.
        std::fs::write(dir.path().join("target"), "4").unwrap();
        std::os::unix::fs::symlink(dir.path().join("target"), dir.path().join("link")).unwrap();
        let disk = disk_of(&l);
        let name = |v: &[String]| {
            v.iter()
                .map(|p| {
                    Path::new(p)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(name(&disk.same), ["same"]);
        assert_eq!(name(&disk.edited), ["edited", "link"]);
        assert_eq!(name(&disk.gone), ["gone"]);
    }

    #[test]
    fn empty_folders_go_from_the_inside_out_and_only_inside_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills/x");
        let file = skill.join("a/b/f.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(skill.join("keep.md"), "mine").unwrap();
        remove_empty_dirs(&[file.as_path()], &skill);
        assert!(!skill.join("a").exists() && skill.join("keep.md").exists());
        std::fs::remove_file(skill.join("keep.md")).unwrap();
        remove_empty_dirs(&[skill.join("keep.md").as_path()], &skill);
        assert!(!skill.exists() && dir.path().join("skills").exists());
    }
}
