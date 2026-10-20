//! The files of a workspace: one folder at a time, with what git says of each entry, and a search
//! by name over the whole of it (spec REQ-309 to REQ-314). It is what a client draws a file tree
//! from and opens a file by. Nothing here changes a file.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use contract::{ErrorCode, FileEntry, FileKind, FileListing, FoundFiles, GitMark};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::git;
use crate::server::Shared;

/// The most entries of a folder that are sent; the listing says how many there are.
const MAX_ENTRIES: usize = 2000;
/// The most paths a search answers, and the most files it looks at.
const MAX_FOUND: usize = 50;
const MAX_SEEN: usize = 100_000;
/// How much of git's output is read for the marks or the list of files.
const GIT_CAP: usize = 8 * 1024 * 1024;

/// The folder of the workspace, or the answer that it has none.
fn root_of(shared: &Shared, workspace: &str) -> Result<PathBuf, ErrorBody> {
    shared
        .lock()
        .workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))
}

/// `path` as a relative path of plain names with `/` between them, `""` for the folder itself;
/// anything that could leave the folder is refused.
pub(crate) fn clean_rel(path: Option<&str>) -> Result<String, ErrorBody> {
    let path = path.unwrap_or("");
    let mut names = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::Normal(n) => names.push(n.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "a path is relative to the workspace and goes no higher",
                ));
            }
        }
    }
    Ok(names.join("/"))
}

/// The file `path` names, canonical: it must exist and be inside `root` once links are followed.
/// `path` is relative to `root`, or absolute and inside it.
pub(crate) fn inside(root: &Path, path: &str) -> Result<PathBuf, ErrorBody> {
    let root = fs::canonicalize(root).map_err(|e| {
        err(
            ErrorCode::Io,
            format!("cannot read {}: {e}", root.display()),
        )
    })?;
    let file = fs::canonicalize(root.join(path))
        .map_err(|_| err(ErrorCode::NotFound, format!("no file {path}")))?;
    if !file.starts_with(&root) {
        return Err(err(
            ErrorCode::BadRequest,
            "that file is outside the workspace",
        ));
    }
    Ok(file)
}

/// The folder `rel` of `root`, which must exist, be a folder and be inside `root` once links are
/// followed.
fn folder(root: &Path, rel: &str) -> Result<PathBuf, ErrorBody> {
    let root = fs::canonicalize(root).map_err(|e| {
        err(
            ErrorCode::Io,
            format!("cannot read {}: {e}", root.display()),
        )
    })?;
    let target = fs::canonicalize(root.join(rel))
        .map_err(|_| err(ErrorCode::NotFound, format!("no folder {rel}")))?;
    if !target.starts_with(&root) {
        return Err(err(
            ErrorCode::BadRequest,
            "that folder is outside the workspace",
        ));
    }
    if !target.is_dir() {
        return Err(err(ErrorCode::BadRequest, format!("{rel} is not a folder")));
    }
    Ok(target)
}

/// How serious a mark is: a folder shows the most serious of what is under it.
fn weight(m: GitMark) -> u8 {
    match m {
        GitMark::Conflict => 4,
        GitMark::Modified => 3,
        GitMark::Deleted => 2,
        GitMark::Added => 1,
        GitMark::Untracked => 0,
    }
}

/// What a status line `XY` says; `None` for ignored (`!!`) and for what says nothing.
fn mark_of(x: char, y: char) -> Option<GitMark> {
    match (x, y) {
        ('?', '?') => Some(GitMark::Untracked),
        ('!', '!') => None,
        ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => Some(GitMark::Conflict),
        ('M' | 'T' | 'R' | 'C', _) | (_, 'M' | 'T' | 'R' | 'C') => Some(GitMark::Modified),
        ('D', _) | (_, 'D') => Some(GitMark::Deleted),
        ('A', _) => Some(GitMark::Added),
        _ => None,
    }
}

/// What git says of the entries of a folder: for each name, the mark (a folder's is the most
/// serious of what is under it) and whether git ignores it. `base` is the folder's path from the
/// repository's top, with a `/` at the end, and `status` is `git status --porcelain=v1 -z`.
pub fn marks(base: &str, status: &[u8]) -> HashMap<String, (Option<GitMark>, bool)> {
    let mut out: HashMap<String, (Option<GitMark>, bool)> = HashMap::new();
    let text = String::from_utf8_lossy(status);
    let mut records = text.split('\0');
    while let Some(rec) = records.next() {
        let mut chars = rec.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let path = chars.as_str();
        // A rename or a copy is followed by the path it came from.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            records.next();
        }
        let Some(rest) = path.strip_prefix(base) else {
            continue;
        };
        let (name, below) = match rest.split_once('/') {
            Some((name, below)) => (name, !below.is_empty()),
            None => (rest, false),
        };
        if name.is_empty() {
            continue;
        }
        let entry = out.entry(name.to_owned()).or_default();
        if (x, y) == ('!', '!') {
            // Only the entry itself being ignored says it is; an ignored file inside does not.
            entry.1 |= !below;
        } else if let Some(m) = mark_of(x, y) {
            entry.0 = Some(
                entry
                    .0
                    .map_or(m, |old| if weight(m) > weight(old) { m } else { old }),
            );
        }
    }
    out
}

/// The marks of the entries of `rel` in the workspace folder `root`, when it is in a repository.
fn git_marks(root: &Path, rel: &str) -> HashMap<String, (Option<GitMark>, bool)> {
    let Ok(prefix) = git::run(root, &["rev-parse", "--show-prefix"]) else {
        return HashMap::new();
    };
    let spec = if rel.is_empty() { "." } else { rel };
    let args = [
        "status",
        "--porcelain=v1",
        "-z",
        "--ignored=traditional",
        "--untracked-files=normal",
        "--",
        spec,
    ];
    let Ok(out) = git::read(root, &args, GIT_CAP) else {
        return HashMap::new();
    };
    let base = if rel.is_empty() {
        prefix
    } else {
        format!("{prefix}{rel}/")
    };
    marks(&base, &out.bytes)
}

/// `list_files`: the entries of one folder, folders first and then the rest, each by name.
pub fn list(shared: &Shared, workspace: &str, path: Option<&str>) -> Done {
    let root = root_of(shared, workspace)?;
    let rel = clean_rel(path)?;
    let dir = folder(&root, &rel)?;
    let marks = git_marks(&root, &rel);
    let read =
        fs::read_dir(&dir).map_err(|e| err(ErrorCode::Io, format!("cannot read {rel}: {e}")))?;
    let mut entries: Vec<FileEntry> = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let Ok(kind) = item.file_type() else {
            continue;
        };
        let (kind, size) = if kind.is_symlink() {
            (FileKind::Link, None)
        } else if kind.is_dir() {
            (FileKind::Dir, None)
        } else {
            (FileKind::File, item.metadata().ok().map(|m| m.len()))
        };
        let (git, ignored) = marks.get(&name).copied().unwrap_or_default();
        entries.push(FileEntry {
            name,
            kind,
            size,
            git,
            ignored,
        });
    }
    entries.sort_by_key(|e| (e.kind != FileKind::Dir, e.name.to_lowercase()));
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);
    let listing = FileListing {
        path: rel,
        entries,
        total,
    };
    Ok(Some(
        serde_json::to_value(listing).expect("a listing serialises"),
    ))
}

/// Whether the letters of `query` are in `path` in order, whatever their case.
pub fn matches(query: &str, path: &str) -> bool {
    let mut letters = path.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|q| letters.any(|p| p == q))
}

/// The best of the paths that match: those whose name holds the query whole first, then the
/// shorter, then by path.
pub fn best(query: &str, paths: impl Iterator<Item = String>) -> (Vec<String>, bool) {
    let q = query.to_lowercase();
    let mut found: Vec<String> = paths.filter(|p| matches(query, p)).collect();
    // The key is made once for each path: a one-letter query matches nearly every file of a big repository.
    found.sort_by_cached_key(|p| {
        let name = p.rsplit('/').next().unwrap_or(p).to_lowercase();
        (!name.contains(&q), p.chars().count(), p.clone())
    });
    let cut = found.len() > MAX_FOUND;
    found.truncate(MAX_FOUND);
    (found, cut)
}

/// The files of a folder that has no repository: a walk of bounded size that leaves out `.git`,
/// `node_modules`, `target` and folders that begin with a dot.
fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![(root.to_path_buf(), String::new(), 0usize)];
    while let Some((dir, rel, depth)) = todo.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            if out.len() >= MAX_SEEN {
                return out;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(kind) = item.file_type() else {
                continue;
            };
            if kind.is_dir() {
                let skip = name.starts_with('.') || name == "node_modules" || name == "target";
                if !skip && depth < 8 {
                    todo.push((item.path(), path, depth + 1));
                }
            } else if kind.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// `find_files`: the paths of the workspace that match `query`.
pub fn find(shared: &Shared, workspace: &str, query: &str) -> Done {
    let root = root_of(shared, workspace)?;
    if query.chars().count() > 200 {
        return Err(err(
            ErrorCode::BadRequest,
            "a query has at most 200 characters",
        ));
    }
    if query.trim().is_empty() {
        return Ok(Some(json!(FoundFiles {
            paths: Vec::new(),
            cut: false
        })));
    }
    let listed = git::read(
        &root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        GIT_CAP,
    );
    let paths: Vec<String> = match listed {
        Ok(out) => String::from_utf8_lossy(&out.bytes)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => walk(&root),
    };
    let (paths, cut) = best(query.trim(), paths.into_iter());
    Ok(Some(json!(FoundFiles { paths, cut })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(lines: &[&str]) -> Vec<u8> {
        lines.join("\0").into_bytes()
    }

    #[test]
    fn a_folder_shows_the_most_serious_of_what_is_under_it_and_an_ignored_entry_says_so() {
        let s = status(&[
            " M src/auth/login.rs",
            "?? src/auth/new.rs",
            "A  src/lib.rs",
            "?? notes.md",
            "!! target/",
            "!! src/auth/out.log",
            "UU src/conflict.rs",
            "",
        ]);
        let top = marks("", &s);
        assert_eq!(top["src"], (Some(GitMark::Conflict), false));
        assert_eq!(top["notes.md"], (Some(GitMark::Untracked), false));
        assert_eq!(top["target"], (None, true));
        let src = marks("src/", &s);
        assert_eq!(
            src["auth"],
            (Some(GitMark::Modified), false),
            "modified before untracked"
        );
        assert_eq!(src["lib.rs"], (Some(GitMark::Added), false));
        assert_eq!(src["conflict.rs"], (Some(GitMark::Conflict), false));
        let auth = marks("src/auth/", &s);
        assert_eq!(auth["login.rs"], (Some(GitMark::Modified), false));
        assert_eq!(auth["out.log"], (None, true));
        assert!(
            !marks("src/", &s).contains_key("notes.md"),
            "what is not in the folder is not there"
        );
    }

    #[test]
    fn an_ignored_file_inside_a_folder_does_not_make_the_folder_ignored() {
        let s = status(&["!! src/auth/out.log", ""]);
        assert_eq!(marks("", &s)["src"], (None, false));
    }

    #[test]
    fn a_rename_is_a_modification_and_its_old_path_is_not_read_as_a_status() {
        let s = status(&["R  new.rs", "old.rs", "?? other.rs", ""]);
        let m = marks("", &s);
        assert_eq!(m["new.rs"], (Some(GitMark::Modified), false));
        assert!(!m.contains_key("old.rs"));
        assert_eq!(m["other.rs"], (Some(GitMark::Untracked), false));
    }

    #[test]
    fn the_letters_of_a_query_are_looked_for_in_order_whatever_their_case() {
        assert!(matches("tok", "src/auth/token.rs"));
        assert!(matches("SAT", "src/auth/token.rs"));
        assert!(!matches("tkz", "src/auth/rok.rs"));
        assert!(matches("", "anything"));
    }

    #[test]
    fn the_best_paths_are_those_whose_name_holds_the_query_then_the_shorter() {
        let paths = [
            "docs/token-notes.md",
            "src/auth/token.rs",
            "a/t/o/k/e/n.rs",
            "src/t.rs",
        ];
        let (found, cut) = best("token", paths.iter().map(|p| (*p).to_owned()));
        assert_eq!(
            found,
            ["src/auth/token.rs", "docs/token-notes.md", "a/t/o/k/e/n.rs"],
            "the name that holds it whole first; letters in order still match"
        );
        assert!(!cut);
        let many = (0..80).map(|i| format!("f{i}.rs"));
        let (found, cut) = best("f", many);
        assert_eq!((found.len(), cut), (MAX_FOUND, true));
    }

    #[test]
    fn a_path_that_could_leave_the_workspace_is_refused() {
        assert_eq!(clean_rel(None).unwrap(), "");
        assert_eq!(clean_rel(Some("./src//auth/")).unwrap(), "src/auth");
        for bad in ["..", "src/../..", "/etc", "a/../../b"] {
            assert!(clean_rel(Some(bad)).is_err(), "{bad}");
        }
    }
}
