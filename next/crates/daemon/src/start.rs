//! `project_config` and `start_workspace`: what a worktree of a repository will be, and the
//! panes it opens with (spec REQ-334, REQ-336 to REQ-341).

use std::path::{Path, PathBuf};

use contract::{ConfigFrom, ErrorCode, Id, Layout, ProjectInfo, SplitDir, WorkspaceKind};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::project::{self, Node, Project, SETUP_SHARE};
use crate::server::Shared;
use crate::{git, panes, rename};

/// The settings of the repository `top` is the top level of; a file that cannot be used is a
/// `bad_request` that says why and the line.
pub fn load(shared: &Shared, top: &Path) -> Result<Project, ErrorBody> {
    let personal = shared.lock().projects_dir.clone();
    project::read(top, &personal).map_err(|why| err(ErrorCode::BadRequest, why))
}

/// `project_config`: what the settings of the repository `repo` is in come to.
pub fn project_config(shared: &Shared, repo: &str) -> Done {
    let dir = crate::workspaces::absolute(repo)?;
    let top = git::toplevel(&dir).map_err(|m| err(ErrorCode::GitFailed, m))?;
    let project = load(shared, Path::new(&top))?;
    let shell = shared.lock().shell.clone();
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
        panes: project.pane_names(&shell),
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

/// One pane, started with `program` (the plain shell when none).
fn spawn(shared: &Shared, workspace: &Id, program: Option<Vec<String>>) -> Result<Id, ErrorBody> {
    let reply = panes::new_pane(shared, workspace, program, None, None)?;
    reply
        .as_ref()
        .and_then(|r| r["pane"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| err(ErrorCode::Io, "the pane has no id"))
}

/// The tree of `node` with the panes that were started for its leaves, in their order.
fn tree(node: &Node, ids: &mut impl Iterator<Item = Id>) -> Option<Layout> {
    Some(match node {
        Node::Leaf(_) => Layout::Pane { pane: ids.next()? },
        Node::Split { dir, ratio, a, b } => Layout::Split {
            dir: *dir,
            ratio: *ratio,
            a: Box::new(tree(a, ids)?),
            b: Box::new(tree(b, ids)?),
        },
    })
}

/// `start_workspace`: the panes of a workspace that has none.
pub fn start_workspace(shared: &Shared, workspace: &Id) -> Done {
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
    let top = top.filter(|_| kind == WorkspaceKind::Worktree);
    let Some(top) = top else {
        return Ok(Some(json!({ "panes": [spawn(shared, workspace, None)?] })));
    };
    let project = load(shared, &PathBuf::from(top))?;

    let mut ids = Vec::new();
    match &project.layout {
        None => ids.push(spawn(shared, workspace, None)?),
        Some(node) => {
            for run in node.leaves() {
                ids.push(spawn(shared, workspace, project.argv(run))?);
            }
        }
    }
    let mut layout = match &project.layout {
        Some(node) => tree(node, &mut ids.clone().into_iter()),
        None => ids.first().map(|id| Layout::Pane { pane: id.clone() }),
    };
    if !project.setup.is_empty() {
        let name = Path::new(&path)
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let script = project::setup_script(&project.setup, &name, &branch);
        let id = spawn(
            shared,
            workspace,
            Some(vec!["/bin/sh".into(), "-c".into(), script]),
        )?;
        rename::pane(shared, &id, "setup")?;
        layout = layout.map(|above| Layout::Split {
            dir: SplitDir::Col,
            ratio: SETUP_SHARE,
            a: Box::new(above),
            b: Box::new(Layout::Pane { pane: id.clone() }),
        });
        ids.push(id);
    }
    let mut core = shared.lock();
    if let Some(ws) = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.id == *workspace)
        // A pane closed while the others started leaves the tree to `reconcile`.
        .filter(|ws| ids.iter().all(|i| ws.panes.iter().any(|p| p.id == *i)))
    {
        ws.layout = layout;
    }
    core.refresh();
    Ok(Some(json!({ "panes": ids })))
}
