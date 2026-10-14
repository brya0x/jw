//! Folders and git worktrees as workspaces.

use std::path::{Path, PathBuf};

use contract::{ErrorCode, Id, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, err};
use crate::git;
use crate::model::{Workspace, new_id};
use crate::server::Shared;

fn absolute(path: &str) -> Result<PathBuf, crate::commands::ErrorBody> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{path:?} is not an absolute path"),
        ));
    }
    std::fs::canonicalize(p).map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::NotFound
        } else {
            ErrorCode::Io
        };
        err(code, format!("{path}: {e}"))
    })
}

/// An existing folder, by its canonical path.
pub fn directory(path: &str) -> Result<PathBuf, crate::commands::ErrorBody> {
    let dir = absolute(path)?;
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(err(
            ErrorCode::BadRequest,
            format!("{} is not a directory", dir.display()),
        ))
    }
}

fn already_open(shared: &Shared, path: &str) -> Result<(), crate::commands::ErrorBody> {
    match shared
        .lock()
        .model
        .workspaces
        .iter()
        .find(|w| w.path == path)
    {
        Some(w) => Err(err(
            ErrorCode::Conflict,
            format!("{path} is already open as {:?}", w.name),
        )),
        None => Ok(()),
    }
}

fn added(shared: &Shared, ws: Workspace) -> Done {
    let id = ws.id.clone();
    let path = ws.path.clone();
    let mut core = shared.lock();
    // Another client may have opened the same path while git ran.
    if core.model.workspaces.iter().any(|w| w.path == path) {
        return Err(err(ErrorCode::Conflict, format!("{path} is already open")));
    }
    core.model.workspaces.push(ws);
    core.refresh();
    Ok(Some(json!({ "workspace": id, "path": path })))
}

pub fn open_folder(shared: &Shared, path: &str) -> Done {
    let dir = directory(path)?;
    let path = dir.to_string_lossy().into_owned();
    already_open(shared, &path)?;
    let name = dir
        .file_name()
        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    added(
        shared,
        Workspace {
            id: new_id(),
            name,
            kind: WorkspaceKind::Folder,
            path,
            project: None,
            branch: None,
            panes: Vec::new(),
            attention: Vec::new(),
            github: None,
        },
    )
}

/// A folder name a person gave: one path component, nothing that moves out of `<repo>-wt/`.
fn valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 80
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// The worktree goes beside the repository, in `<repo>-wt/<folder>`, the folder being the
/// given name or else the branch with its slashes made dashes.
pub fn create_worktree(
    shared: &Shared,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    folder: Option<&str>,
) -> Done {
    if let Some(f) = folder
        && !valid_folder(f)
    {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{f:?} is not a usable folder name"),
        ));
    }
    let repo_dir = absolute(repo)?;
    let git_err = |m: String| err(ErrorCode::GitFailed, m);
    let top = PathBuf::from(git::toplevel(&repo_dir).map_err(git_err)?);
    if !git::valid_branch(&top, branch) {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{branch:?} is not a usable branch name"),
        ));
    }
    if let Some(base) = base
        && !git::valid_branch(&top, base)
    {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{base:?} is not a usable base"),
        ));
    }
    let repo_name = top
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let parent = top.parent().ok_or_else(|| {
        err(
            ErrorCode::BadRequest,
            "the repository has no parent directory",
        )
    })?;
    let dest = parent
        .join(format!("{repo_name}-wt"))
        .join(folder.map_or_else(|| git::slug(branch), str::to_owned));
    let dest_text = dest.to_string_lossy().into_owned();
    already_open(shared, &dest_text)?;
    if dest.exists() {
        return Err(err(
            ErrorCode::Conflict,
            format!("{dest_text} already exists"),
        ));
    }
    let existing = git::branch_exists(&top, branch);
    let mut args = vec!["worktree", "add"];
    match (existing, base) {
        (true, Some(_)) => {
            return Err(err(
                ErrorCode::BadRequest,
                format!("branch {branch} exists, so a base makes no sense"),
            ));
        }
        (true, None) => args.extend([dest_text.as_str(), branch]),
        (false, base) => {
            args.extend(["-b", branch, dest_text.as_str()]);
            args.extend(base);
        }
    }
    git::run(&top, &args).map_err(git_err)?;
    added(
        shared,
        Workspace {
            id: new_id(),
            name: format!("{repo_name}: {branch}"),
            kind: WorkspaceKind::Worktree,
            path: dest_text,
            project: Some(top.to_string_lossy().into_owned()),
            branch: Some(branch.to_owned()),
            panes: Vec::new(),
            attention: Vec::new(),
            github: None,
        },
    )
}

pub fn remove_workspace(shared: &Shared, workspace: &Id, remove_worktree: bool) -> Done {
    let (kind, path, project) = {
        let core = shared.lock();
        let ws = core
            .model
            .workspaces
            .iter()
            .find(|w| w.id == *workspace)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
        (ws.kind, ws.path.clone(), ws.project.clone())
    };
    if remove_worktree {
        let project = match (kind, project) {
            (WorkspaceKind::Worktree, Some(p)) => p,
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "only a worktree workspace has a worktree to remove",
                ));
            }
        };
        // No --force: a worktree with changes stays, and so does its workspace.
        git::run(Path::new(&project), &["worktree", "remove", &path])
            .map_err(|m| err(ErrorCode::GitFailed, m))?;
    }
    let mut core = shared.lock();
    core.model.workspaces.retain(|w| w.id != *workspace);
    core.refresh();
    Ok(None)
}
