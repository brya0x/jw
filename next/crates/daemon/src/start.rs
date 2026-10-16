//! `project_config` and `start_workspace`: what a worktree of a repository will be, and the one
//! panel it opens with (spec REQ-334, REQ-342 to REQ-345, REQ-500 to REQ-511).

use std::path::{Path, PathBuf};

use contract::{ConfigFrom, ErrorCode, Id, OpenKind, ProjectInfo, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::project::{self, Project};
use crate::server::Shared;
use crate::{git, panes};

/// The settings of the repository `top` is the top level of; a file that cannot be used is a
/// `bad_request` that says why and the line.
pub fn load(shared: &Shared, top: &Path) -> Result<Project, ErrorBody> {
    let personal = shared.lock().projects_dir.clone();
    project::read(top, &personal).map_err(|why| err(ErrorCode::BadRequest, why))
}

/// The settings of a folder opened as a workspace (REQ-501).
fn load_folder(shared: &Shared, folder: &Path) -> Result<Project, ErrorBody> {
    let personal = shared.lock().projects_dir.clone();
    project::read_folder(folder, &personal).map_err(|why| err(ErrorCode::BadRequest, why))
}

/// `project_config`: what the settings of the repository `repo` is in come to.
pub fn project_config(shared: &Shared, repo: &str) -> Done {
    let dir = crate::workspaces::absolute(repo)?;
    let top = git::toplevel(&dir).map_err(|m| err(ErrorCode::GitFailed, m))?;
    let project = load(shared, Path::new(&top))?;
    let info = ProjectInfo {
        source: project
            .source
            .as_ref()
            .map(|s| s.to_string_lossy().into_owned()),
        from: project.from,
        personal: project.personal.to_string_lossy().into_owned(),
        root: project.root.to_string_lossy().into_owned(),
        branch: project.branch.clone(),
        setup: project.setup.clone(),
        open: project.open,
        copy: project.copy.clone(),
        ports: project.ports.clone(),
    };
    Ok(Some(
        serde_json::to_value(info).expect("a ProjectInfo is JSON"),
    ))
}

/// `write_project_config`: the settings of the repository `repo` is in, checked and written to
/// the file `to` says (REQ-486, REQ-487).
pub fn write_project_config(shared: &Shared, repo: &str, to: ConfigFrom, text: &str) -> Done {
    let dir = crate::workspaces::absolute(repo)?;
    let top = git::toplevel(&dir).map_err(|m| err(ErrorCode::GitFailed, m))?;
    let personal = shared.lock().projects_dir.clone();
    let (path, shadowed) = project::write(Path::new(&top), &personal, to, text)
        .map_err(|why| err(ErrorCode::BadRequest, why))?;
    Ok(Some(
        json!({ "path": path.to_string_lossy(), "shadowed": shadowed }),
    ))
}

/// The one panel of a workspace, started with `program` and, when it ends well, `then`.
fn spawn(
    shared: &Shared,
    workspace: &Id,
    program: Option<Vec<String>>,
    then: Option<Vec<String>>,
) -> Result<Id, ErrorBody> {
    let reply = panes::new_pane_then(shared, workspace, program, then, None, None)?;
    reply
        .as_ref()
        .and_then(|r| r["pane"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| err(ErrorCode::Io, "the pane has no id"))
}

/// `start_workspace`: the one panel of a workspace that has none (REQ-506): a shell or `claude`
/// as `open` says or, without it, as the settings do. A worktree's setup runs in that panel first
/// (REQ-507); a folder has no setup (REQ-500).
pub fn start_workspace(shared: &Shared, workspace: &Id, open: Option<OpenKind>) -> Done {
    let (kind, path, top, branch) = {
        let core = shared.lock();
        let ws = core.workspace(workspace)?;
        if !ws.panes.is_empty() {
            return Err(err(
                ErrorCode::Conflict,
                format!("{:?} already has panes", ws.name),
            ));
        }
        (
            ws.kind,
            ws.path.clone(),
            ws.project.clone(),
            ws.branch.clone().unwrap_or_default(),
        )
    };
    let worktree_of = top.filter(|_| kind == WorkspaceKind::Worktree);
    let project = match &worktree_of {
        Some(top) => load(shared, &PathBuf::from(top))?,
        None => load_folder(shared, Path::new(&path))?,
    };
    let claude = vec!["claude".to_owned()];
    let agent = open.unwrap_or(project.open) == OpenKind::Claude;
    let id = if worktree_of.is_some() && !project.setup.is_empty() {
        let name = Path::new(&path)
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let script = project::setup_script(&project.setup, &name, &branch);
        let setup = vec!["/bin/sh".to_owned(), "-c".to_owned(), script];
        spawn(shared, workspace, Some(setup), agent.then_some(claude))?
    } else {
        spawn(shared, workspace, agent.then_some(claude), None)?
    };
    Ok(Some(json!({ "panes": [id] })))
}
