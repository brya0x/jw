//! The diff and the viewer as leaves of a workspace's layout (N163, REQ-684 to REQ-692).
//!
//! A view is not a pane: it has no process, so it lives in `Workspace::views`, which is not
//! saved. Its leaf is in the workspace's layout beside the panes; what it draws a client reads
//! for itself (`open_diff`, `read_file`). The daemon keeps that it is there, where it is, and the
//! files a viewer has open, so that the terminal client and the phone see one layout.

use contract::{ErrorCode, Id, SplitDir, ViewKind};
use serde_json::json;

use crate::commands::{Done, err};
use crate::layout::{self, Side};
use crate::model::{MAX_TABS, View, new_id};
use crate::server::Shared;

/// What a view is called: the diff's name, a viewer's last file.
pub fn title(view: &View) -> String {
    match view.kind {
        ViewKind::Diff => "diff".to_owned(),
        ViewKind::Viewer => view.files.last().map_or_else(
            || "archivo".to_owned(),
            |f| {
                std::path::Path::new(f)
                    .file_name()
                    .map_or_else(|| f.clone(), |n| n.to_string_lossy().into_owned())
            },
        ),
    }
}

fn no_view(view: &str) -> crate::commands::ErrorBody {
    err(ErrorCode::NotFound, format!("no view {view}"))
}

/// Open the diff or a viewer (REQ-685, REQ-686, REQ-687).
pub fn open_view(
    shared: &Shared,
    workspace: &Id,
    kind: ViewKind,
    path: Option<String>,
    beside: Option<Id>,
    dir: Option<SplitDir>,
) -> Done {
    let mut core = shared.lock();
    core.workspace(workspace)?;
    let ws = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.id == *workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
    if kind == ViewKind::Diff && path.is_some() {
        return Err(err(ErrorCode::BadRequest, "the diff takes no path"));
    }
    if kind == ViewKind::Viewer && path.as_deref().is_none_or(str::is_empty) {
        return Err(err(ErrorCode::BadRequest, "a viewer needs a path"));
    }
    let pane_ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
    if pane_ids.is_empty() {
        return Err(err(
            ErrorCode::BadRequest,
            "the workspace has no panes to put a view beside",
        ));
    }
    let known = |ws: &crate::model::Workspace, id: &str| {
        ws.panes.iter().any(|p| p.id == id) || ws.views.iter().any(|v| v.id == id)
    };
    if let Some(b) = beside.as_deref().filter(|b| !known(ws, b)) {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{b} is not a pane or a view of this workspace"),
        ));
    }
    // The diff is one; a viewer is one too and a file is a tab of it.
    if let Some(view) = ws.views.iter_mut().find(|v| v.kind == kind) {
        if let Some(path) = path.filter(|p| !view.files.contains(p)) {
            view.files.push(path);
            if view.files.len() > MAX_TABS {
                view.files.remove(0);
            }
        }
        let id = view.id.clone();
        core.refresh();
        return Ok(Some(json!({ "view": id })));
    }
    let id = new_id();
    let views: Vec<Id> = ws.views.iter().map(|v| v.id.clone()).collect();
    let tree = layout::reconcile_views(ws.layout.take(), &pane_ids, &views)
        .ok_or_else(|| err(ErrorCode::BadRequest, "the workspace has no layout"))?;
    let remembered = ws.places.get(&kind).cloned();
    let placed = match remembered {
        Some(p) => layout::insert(tree, &p.beside, &id, p.side, p.share),
        None => Err(tree),
    };
    let tree = match placed {
        Ok(tree) => tree,
        Err(tree) => match beside.as_deref() {
            Some(b) => {
                let dir = dir.unwrap_or(SplitDir::Row);
                let side = if dir == SplitDir::Row {
                    Side::Right
                } else {
                    Side::Below
                };
                layout::insert(tree, &[b.to_owned()], &id, side, 500)
                    .unwrap_or_else(|tree| layout::split(tree, b, &id, dir))
            }
            None => layout::edge(tree, &id, 500),
        },
    };
    ws.layout = Some(tree);
    ws.views.push(View {
        id: id.clone(),
        kind,
        files: path.into_iter().collect(),
    });
    core.refresh();
    Ok(Some(json!({ "view": id })))
}

/// Close a view, or a tab of a viewer (REQ-688).
pub fn close_view(shared: &Shared, view: &Id, path: Option<String>) -> Done {
    let mut core = shared.lock();
    let ws = core
        .model
        .workspaces
        .iter_mut()
        .find(|w| w.views.iter().any(|v| v.id == *view))
        .ok_or_else(|| no_view(view))?;
    let at = ws
        .views
        .iter()
        .position(|v| v.id == *view)
        .ok_or_else(|| no_view(view))?;
    if let Some(path) = path {
        let v = &mut ws.views[at];
        if v.kind != ViewKind::Viewer {
            return Err(err(ErrorCode::BadRequest, "only a viewer has tabs"));
        }
        let Some(tab) = v.files.iter().position(|f| *f == path) else {
            return Err(err(
                ErrorCode::BadRequest,
                format!("{path} is not open in the viewer"),
            ));
        };
        v.files.remove(tab);
        if !v.files.is_empty() {
            core.refresh();
            return Ok(None);
        }
    }
    let ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
    let views: Vec<Id> = ws.views.iter().map(|v| v.id.clone()).collect();
    ws.layout = layout::reconcile_views(ws.layout.take(), &ids, &views);
    if let Some(place) = ws.layout.as_ref().and_then(|t| layout::place_of(t, view)) {
        let kind = ws.views[at].kind;
        ws.places.insert(kind, place);
    }
    ws.views.remove(at);
    core.refresh();
    Ok(None)
}
