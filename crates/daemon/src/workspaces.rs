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
            chat: None,
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
            chat: None,
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
        core.forget_artifacts(workspace);
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
            let result = remove_steps(&removal.shared, &removal.workspace, &project, &path, force);
            removal.end(result);
        });
    if let Err(e) = spawned {
        // The closure went with the error, and its `Removal` ended the job as failed.
        return Err(err(ErrorCode::Io, e.to_string()));
    }
    Ok(Some(serde_json::json!({ "job": job })))
}

/// The steps of REQ-1065 for a worktree, on the thread of the job: when one fails the message says
/// what was done and what was not (REQ-1068), and a retry goes on from where the folder is (REQ-1069).
fn remove_steps(
    shared: &Shared,
    workspace: &Id,
    project: &str,
    path: &str,
    force: bool,
) -> Result<(), String> {
    let (top, dir) = (Path::new(project), Path::new(path));
    let mut done: Vec<String> = Vec::new();
    let said = |done: &[String], why: String, left: &str| {
        let did = if done.is_empty() {
            "nada".to_owned()
        } else {
            done.join(", ")
        };
        format!("{why} (hecho: {did}; sin hacer: {left})")
    };
    let known = git::knows(top, dir).map_err(|e| said(&done, e, "todo"))?;
    // A folder that is gone has no changes to refuse and nothing to say is on disk (REQ-1073).
    let exists = std::fs::symlink_metadata(dir).is_ok();
    if known && !force && exists {
        let changes =
            git::run(dir, &["status", "--porcelain"]).map_err(|e| said(&done, e, "todo"))?;
        if !changes.is_empty() {
            return Err("el worktree tiene cambios y no hay --force (nada se tocó: ni los procesos, ni git, ni la carpeta)".into());
        }
    }
    if !known && !force && exists {
        return Err(format!(
            "huérfano: git ya no conoce {path} (nada se tocó: ni los procesos, ni la carpeta, ni la entrada); la carpeta sigue en disco: quítalo con force, o cierra el workspace sin quitar el worktree"
        ));
    }
    let roots = shared.lock().removal_roots(workspace);
    let ended =
        crate::reap::end(&roots).map_err(|e| said(&done, e, "git, la carpeta y la entrada"))?;
    done.push(format!(
        "procesos: {} terminados, {} con SIGKILL",
        ended.reached, ended.killed
    ));
    if known {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path);
        match git::run(top, &args) {
            Ok(_) => return Ok(()),
            // Git forgot the worktree and failed on the last folder: what is left is what the
            // processes wrote, and the job finishes it (REQ-1071).
            Err(e) => match git::knows(top, dir) {
                Ok(false) => done.push(format!("git: lo olvidó pero falló ({e})")),
                _ => return Err(said(&done, e, "la carpeta y la entrada")),
            },
        }
    }
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(());
    }
    let allowed = crate::start::load(shared, top)
        .map_err(|e| e.message)
        .and_then(|p| deletable(dir, &p.root, top, home().as_deref()));
    match allowed {
        Ok(real) => std::fs::remove_dir_all(&real).map_err(|e| {
            said(
                &done,
                format!("no se pudo borrar {path}: {e}"),
                "la carpeta y la entrada",
            )
        }),
        Err(why) => {
            shared.lock().drop_workspace(workspace);
            Err(format!(
                "la carpeta {path} sigue en disco ({why}); la entrada de {} se quitó (hecho: {})",
                identity::get().name,
                done.join(", ")
            ))
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").and_then(|h| Path::new(&h).canonicalize().ok())
}

/// The folder of an orphan, canonical, when it may be deleted (REQ-1072): not a link, strictly
/// inside the folder where the repository's worktrees go, which is not `/` nor home, not home
/// itself, with a name, and not holding the repository.
fn deletable(dir: &Path, root: &Path, top: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("no es una carpeta, o es un enlace".into());
    }
    let real = dir.canonicalize().map_err(|e| e.to_string())?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("la carpeta de worktrees {}: {e}", root.display()))?;
    if root == Path::new("/") || Some(root.as_path()) == home {
        return Err(format!("la carpeta de worktrees es {}", root.display()));
    }
    if real == root || !real.starts_with(&root) {
        return Err(format!("no está dentro de {}", root.display()));
    }
    if Some(real.as_path()) == home || real.file_name().is_none() {
        return Err("es el home o no tiene nombre".into());
    }
    if top.canonicalize().is_ok_and(|t| t.starts_with(&real)) {
        return Err("contiene el repositorio".into());
    }
    Ok(real)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jw-del-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn only_a_folder_strictly_inside_the_worktrees_folder_may_be_deleted() {
        let base = temp("guard");
        let (root, top) = (base.join("wt"), base.join("repo"));
        let (inside, outside) = (root.join("feat"), base.join("other"));
        for d in [&inside, &outside, &top] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(deletable(&inside, &root, &top, None).unwrap(), inside);
        for (dir, root) in [(&root, &root), (&outside, &root), (&top, &root)] {
            assert!(deletable(dir, root, &top, None).is_err(), "{dir:?}");
        }
        // The worktrees folder is `/` or home; a folder that holds the repository.
        assert!(deletable(&inside, Path::new("/"), &top, None).is_err());
        assert!(deletable(&inside, &root, &top, Some(&root)).is_err());
        assert!(deletable(&base, &base, &inside, Some(&base)).is_err());
        let holder = root.join("holds");
        std::fs::create_dir_all(holder.join("repo")).unwrap();
        assert!(deletable(&holder, &root, &holder.join("repo"), None).is_err());
        // A link, even to a folder inside the right one.
        let link = root.join("link");
        std::os::unix::fs::symlink(&inside, &link).unwrap();
        assert!(deletable(&link, &root, &top, None).is_err());
        // A path that is gone.
        assert!(deletable(&root.join("gone"), &root, &top, None).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
