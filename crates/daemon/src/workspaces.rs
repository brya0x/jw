//! Folders and git worktrees as workspaces.

use std::path::{Path, PathBuf};

use contract::{ErrorCode, Id, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, err};
use crate::git;
use crate::model::{Workspace, new_id};
use crate::server::Shared;

pub fn absolute(path: &str) -> Result<PathBuf, crate::commands::ErrorBody> {
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

pub(crate) fn added(shared: &Shared, ws: Workspace) -> Done {
    let id = ws.id.clone();
    let path = ws.path.clone();
    let mut core = shared.lock();
    // Another client may have opened the same path while git ran.
    if core.model.workspaces.iter().any(|w| w.path == path) {
        return Err(err(ErrorCode::Conflict, format!("{path} is already open")));
    }
    // The ports are the lowest block no other workspace holds, chosen here where nobody else can
    // take it first (REQ-495).
    let mut ws = ws;
    if let Some(block) = ws.ports.as_mut() {
        block.slot = core
            .model
            .free_slot()
            .ok_or_else(|| err(ErrorCode::Conflict, "no block of ports is free"))?;
    }
    let mut reply = json!({ "workspace": id, "path": path });
    if let Some(block) = &ws.ports {
        reply["ports"] = json!(block.ports());
    }
    core.model.workspaces.push(ws);
    core.refresh();
    Ok(Some(reply))
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
            layout: None,
            attention: Vec::new(),
            views: Vec::new(),
            places: Default::default(),
            adopted: false,
            ports: None,
            review: None,
            github: None,
        },
    )
}

/// A folder name a person gave: one path component, nothing that moves out of the worktrees' folder.
fn valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 80
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// The worktree goes in `<root>/<folder>`, the root being the one of the repository's `.jw.toml`
/// (`<repo>-wt`, beside it, by default) and the folder the given name or else the branch with its
/// slashes made dashes.
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
    let project = crate::start::load(shared, &top)?;
    let dest = project
        .root
        .join(folder.map_or_else(|| git::slug(branch), str::to_owned));
    let dest_text = dest.to_string_lossy().into_owned();
    already_open(shared, &dest_text)?;
    if dest.exists() {
        return Err(err(
            ErrorCode::Conflict,
            format!("{dest_text} already exists"),
        ));
    }
    if !project.ports.is_empty() && shared.lock().model.free_slot().is_none() {
        return Err(err(ErrorCode::Conflict, "no block of ports is free"));
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
    let copied = crate::provision::copy_files(&top, &dest, &project.copy);
    let reply = added(
        shared,
        Workspace {
            id: new_id(),
            name: format!("{repo_name}: {branch}"),
            kind: WorkspaceKind::Worktree,
            path: dest_text,
            project: Some(top.to_string_lossy().into_owned()),
            branch: Some(branch.to_owned()),
            panes: Vec::new(),
            layout: None,
            attention: Vec::new(),
            views: Vec::new(),
            places: Default::default(),
            adopted: false,
            ports: (!project.ports.is_empty()).then(|| crate::model::PortBlock {
                slot: 0,
                names: project.ports.clone(),
            }),
            review: None,
            github: None,
        },
    )?;
    Ok(reply.map(|mut r| {
        copied.say(&mut r);
        r
    }))
}

/// `remove_workspace`. Closing a workspace is at once. Removing its worktree takes as long as the
/// disk does (a build folder is gigabytes), so it is a job: the request is checked, the reply is
/// `{job}` and git runs in a thread of its own, with no lock held (REQ-648, REQ-649).
pub fn remove_workspace(
    shared: &Shared,
    workspace: &Id,
    remove_worktree: bool,
    force: bool,
) -> Done {
    if force && !remove_worktree {
        return Err(err(
            ErrorCode::BadRequest,
            "force is for removing a worktree",
        ));
    }
    if !remove_worktree {
        let mut core = shared.lock();
        core.workspace(workspace)?;
        core.model.workspaces.retain(|w| w.id != *workspace);
        core.refresh();
        return Ok(None);
    }
    let (path, project, job) = {
        let mut core = shared.lock();
        let ws = core.workspace(workspace)?;
        let (path, project) = match (ws.kind, ws.project.clone()) {
            (WorkspaceKind::Worktree, Some(p)) => (ws.path.clone(), p),
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "only a worktree workspace has a worktree to remove",
                ));
            }
        };
        (path, project, core.begin_removal(workspace)?)
    };
    let removal = Removal {
        shared: shared.clone(),
        workspace: workspace.clone(),
        job: job.clone(),
        done: false,
    };
    // Without `force` a worktree with changes stays, and so does its workspace; the client asks
    // for it only after the person has typed the branch's name.
    let spawned = std::thread::Builder::new()
        .name("remove-worktree".into())
        .spawn(move || {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(&path);
            let result = git::run(Path::new(&project), &args).map(|_| ());
            removal.end(result);
        });
    if let Err(e) = spawned {
        // The closure went with the error, and its `Removal` ended the job as failed.
        return Err(err(ErrorCode::Io, e.to_string()));
    }
    Ok(Some(serde_json::json!({ "job": job })))
}

/// The end of a removal: whatever happens to the thread, the job ends and the workspace is
/// unmarked.
struct Removal {
    shared: Shared,
    workspace: Id,
    job: Id,
    done: bool,
}

impl Removal {
    fn end(mut self, result: Result<(), String>) {
        self.done = true;
        self.shared
            .lock()
            .end_removal(&self.workspace, &self.job, result);
    }
}

impl Drop for Removal {
    fn drop(&mut self) {
        if !self.done {
            self.shared.lock().end_removal(
                &self.workspace,
                &self.job,
                Err("se interrumpió antes de terminar".into()),
            );
        }
    }
}
