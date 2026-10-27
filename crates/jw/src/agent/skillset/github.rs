//! A `github` entry: the repository fetched into a temporary directory (the means of `update`), the
//! files of the skill's folder read from it and written to their place (N184b, REQ-1022 to REQ-1024).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::update::{Checkout, checkout, checkout_cancellable};

const MAX_FILES: usize = 200;
const MAX_BYTES: usize = 5_000_000;

/// The base of every URL: `<PREFIX>_SKILLSET_GITHUB` when it is set (for the tests of the commands,
/// RISK-171, and then the `file` protocol is let through), else `https://github.com` (REQ-1029).
pub(super) fn base() -> (String, bool) {
    match std::env::var(identity::env_var("SKILLSET_GITHUB")) {
        Ok(b) if !b.is_empty() => (b.trim_end_matches('/').to_owned(), true),
        _ => ("https://github.com".to_owned(), false),
    }
}

/// The URL of `<owner>/<repo>` and whether a test's base named it: `place` is the base a `Place`
/// carries (a test's own), else the one of `base`.
pub(super) fn url_of(place: Option<&str>, repo: &str) -> (String, bool) {
    let (base, from_env) = match place {
        Some(b) => (b.trim_end_matches('/').to_owned(), true),
        None => base(),
    };
    (format!("{base}/{repo}"), from_env)
}

/// What was fetched: the commit and the files, each with its path from the skill's folder (a role's
/// one file is its name). The directory is deleted when this drops.
pub(super) struct Fetched {
    _checkout: Checkout,
    pub commit: String,
    pub files: Vec<(String, Vec<u8>)>,
}

impl Fetched {
    pub(super) fn interrupted(&self) -> bool {
        self._checkout.interrupted()
    }
}

pub(super) fn fetch(
    url: &str,
    allow_file: bool,
    branch: Option<&str>,
    path: &str,
    role: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Fetched, String> {
    let (checkout, commit) = match cancel {
        Some(c) => checkout_cancellable(url, branch, allow_file, c)?,
        None => checkout(url, branch, allow_file)?,
    };
    let files = collect(checkout.dir(), path, role)?;
    Ok(Fetched {
        _checkout: checkout,
        commit,
        files,
    })
}

/// The files to copy, read with `symlink_metadata`: a link anywhere on the way is a refusal of the
/// whole install, as are too many files or too many bytes (REQ-1023, RISK-172).
fn collect(root: &Path, path: &str, role: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut at = root.to_path_buf();
    for part in path.split('/') {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return Err(format!("{path}: a link, refused")),
            Ok(_) => {}
            Err(_) => return Err(format!("the repository has no {path}")),
        }
    }
    if role {
        let bytes = std::fs::read(&at).map_err(|e| format!("{path}: {e}"))?;
        let name = at.file_name().unwrap().to_string_lossy().into_owned();
        return Ok(vec![(name, bytes)]);
    }
    let folder = at.parent().unwrap().to_path_buf();
    let mut files = Vec::new();
    let mut bytes = 0;
    walk(&folder, "", &mut files, &mut bytes)?;
    if !files.iter().any(|(rel, _)| rel == "SKILL.md") {
        return Err(format!("the repository has no {path}"));
    }
    Ok(files)
}

fn walk(
    dir: &Path,
    rel: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    bytes: &mut usize,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|n| format!("{n:?}: a name that is not UTF-8"))?;
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| format!("{rel}: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{rel}: a link, refused"));
        } else if meta.is_dir() {
            walk(&entry.path(), &rel, files, bytes)?;
        } else if meta.is_file() {
            *bytes += meta.len() as usize;
            if files.len() >= MAX_FILES || *bytes > MAX_BYTES {
                return Err(format!(
                    "more than {MAX_FILES} files or {} MB: refused",
                    MAX_BYTES / 1_000_000
                ));
            }
            let content = std::fs::read(entry.path()).map_err(|e| format!("{rel}: {e}"))?;
            files.push((rel, content));
        } else {
            return Err(format!("{rel}: not a file or a folder, refused"));
        }
    }
    Ok(())
}

/// The names that are not Markdown: a skill can carry scripts (REQ-1022).
pub(super) fn not_markdown(files: &[(String, Vec<u8>)]) -> Vec<&str> {
    files
        .iter()
        .map(|(rel, _)| rel.as_str())
        .filter(|rel| !rel.to_ascii_lowercase().ends_with(".md"))
        .collect()
}

/// Write each file through a rename; if one fails, remove what this run wrote (REQ-1024).
pub(super) fn write_all(files: &[(PathBuf, Vec<u8>)], limit: Option<&Path>) -> Result<(), String> {
    let mut written: Vec<&Path> = Vec::new();
    for (path, bytes) in files {
        let put = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(".tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = put() {
            for w in &written {
                let _ = std::fs::remove_file(w);
            }
            if let Some(limit) = limit {
                super::lock::remove_empty_dirs(&written, limit);
            }
            return Err(format!("{}: {e}", path.display()));
        }
        written.push(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let p = dir.path().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn the_skills_folder_is_collected_with_its_paths_and_the_rest_of_the_repository_is_not() {
        let r = repo(&[
            ("skills/x/SKILL.md", "---\nname: x\n---\n"),
            ("skills/x/refs/a.md", "a"),
            ("skills/x/run.sh", "echo"),
            ("skills/other/SKILL.md", "no"),
            ("README.md", "no"),
        ]);
        let files = collect(r.path(), "skills/x/SKILL.md", false).unwrap();
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["SKILL.md", "refs/a.md", "run.sh"]);
        assert_eq!(not_markdown(&files), ["run.sh"]);
    }

    #[test]
    fn a_role_is_one_file_and_a_missing_path_is_an_error() {
        let r = repo(&[("roles/voice.md", "v"), ("skills/x/other.md", "o")]);
        assert_eq!(
            collect(r.path(), "roles/voice.md", true).unwrap(),
            [("voice.md".to_owned(), b"v".to_vec())]
        );
        assert!(collect(r.path(), "roles/none.md", true).is_err());
        // A skill folder with no SKILL.md is not a skill.
        let e = collect(r.path(), "skills/x/SKILL.md", false).unwrap_err();
        assert!(e.contains("no skills/x/SKILL.md"), "{e}");
    }

    #[test]
    fn a_link_anywhere_refuses_the_whole_install() {
        let r = repo(&[("skills/x/SKILL.md", "s"), ("secret", "top")]);
        let x = r.path().join("skills/x");
        std::os::unix::fs::symlink(r.path().join("secret"), x.join("leak.md")).unwrap();
        let e = collect(r.path(), "skills/x/SKILL.md", false).unwrap_err();
        assert!(e.contains("leak.md") && e.contains("link"), "{e}");
        // The link is a folder.
        std::fs::remove_file(x.join("leak.md")).unwrap();
        std::os::unix::fs::symlink(r.path(), x.join("up")).unwrap();
        assert!(collect(r.path(), "skills/x/SKILL.md", false).is_err());
        // The skill's own folder, or its SKILL.md, is the link.
        let r = repo(&[("real/SKILL.md", "s")]);
        std::fs::create_dir_all(r.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(r.path().join("real"), r.path().join("skills/x")).unwrap();
        assert!(collect(r.path(), "skills/x/SKILL.md", false).is_err());
        let r = repo(&[("secret", "s"), ("skills/x/keep", "k")]);
        std::os::unix::fs::symlink(r.path().join("secret"), r.path().join("skills/x/SKILL.md"))
            .unwrap();
        assert!(collect(r.path(), "skills/x/SKILL.md", false).is_err());
    }

    #[test]
    fn too_many_files_or_too_many_bytes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let x = dir.path().join("s/x");
        std::fs::create_dir_all(&x).unwrap();
        std::fs::write(x.join("SKILL.md"), "s").unwrap();
        for i in 0..MAX_FILES {
            std::fs::write(x.join(format!("f{i}.md")), "f").unwrap();
        }
        let e = collect(dir.path(), "s/x/SKILL.md", false).unwrap_err();
        assert!(e.contains("more than"), "{e}");
        let dir = tempfile::tempdir().unwrap();
        let x = dir.path().join("s/x");
        std::fs::create_dir_all(&x).unwrap();
        std::fs::write(x.join("SKILL.md"), "s").unwrap();
        std::fs::write(x.join("big.bin"), vec![0u8; MAX_BYTES + 1]).unwrap();
        assert!(collect(dir.path(), "s/x/SKILL.md", false).is_err());
    }

    #[test]
    fn a_write_that_fails_removes_what_the_run_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills/x");
        let blocked = skill.join("sub");
        std::fs::create_dir_all(&skill).unwrap();
        // `sub` is a file, so a file below it cannot be made.
        std::fs::write(&blocked, "in the way").unwrap();
        let files = vec![
            (skill.join("SKILL.md"), b"s".to_vec()),
            (skill.join("a/b.md"), b"b".to_vec()),
            (blocked.join("c.md"), b"c".to_vec()),
        ];
        assert!(write_all(&files, Some(&skill)).is_err());
        assert!(!skill.join("SKILL.md").exists() && !skill.join("a").exists());
        assert!(blocked.exists(), "what was there is left");
        let ok = vec![(skill.join("n/SKILL.md"), b"s".to_vec())];
        write_all(&ok, Some(&skill)).unwrap();
        assert_eq!(std::fs::read(skill.join("n/SKILL.md")).unwrap(), b"s");
    }
}
