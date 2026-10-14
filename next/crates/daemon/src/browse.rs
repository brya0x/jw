//! Reading a folder's subfolders, for a client that is choosing a folder to open.

use std::path::{Path, PathBuf};

use contract::ErrorCode;
use serde_json::json;

use crate::commands::{Done, err};
use crate::workspaces::absolute;

/// The most folders one `list_dir` returns; `total` says how many there were.
const LIMIT: usize = 500;

/// The folders of `dir`, sorted without regard to case; those starting with a dot only when
/// `hidden`.
fn read_subdirs(dir: &Path, hidden: bool) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| hidden || !n.starts_with('.'))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

/// The folders of `dir` a person would open: no hidden ones; none when it cannot be read.
pub fn subdirs(dir: &Path) -> Vec<String> {
    read_subdirs(dir, false).unwrap_or_default()
}

pub fn is_git(path: &Path) -> bool {
    path.join(".git").exists()
}

/// This machine's home folder (`/` when `$HOME` is not set).
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// `list_dir`: where it is, where up is, the daemon's home, and its folders (at most `LIMIT`,
/// each with whether it is a git repository).
pub fn list_dir(path: Option<&str>, hidden: bool) -> Done {
    let home = home();
    let dir = match path {
        Some(p) => absolute(p)?,
        None => absolute(&home.to_string_lossy())?,
    };
    if !dir.is_dir() {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{} is not a directory", dir.display()),
        ));
    }
    let names = read_subdirs(&dir, hidden).map_err(|e| {
        let code = match e.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            _ => ErrorCode::Io,
        };
        err(code, format!("{}: {e}", dir.display()))
    })?;
    let total = names.len();
    let dirs: Vec<_> = names
        .into_iter()
        .take(LIMIT)
        .map(|name| {
            let git = is_git(&dir.join(&name));
            json!({ "name": name, "git": git })
        })
        .collect();
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let mut result = json!({
        "path": text(&dir),
        "home": text(&home),
        "dirs": dirs,
        "total": total,
    });
    if let Some(parent) = dir.parent() {
        result["parent"] = json!(text(parent));
    }
    Ok(Some(result))
}
