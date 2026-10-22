//! Groups of workspaces (REQ-684 to REQ-689): a name that gathers the repositories, worktrees and
//! folders of one product. A group is organization only: it closes, moves and changes nothing on
//! disk. The group of a workspace is kept under `Workspace::group_key`, so a repository's root and
//! its worktrees share it and a workspace that is opened again finds it.

use contract::{ErrorCode, Group};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::model::new_id;
use crate::server::Shared;

/// The most characters a group's name may have.
const MAX: usize = 40;

/// The name as kept: trimmed, with no control characters, not empty, not over `MAX` characters,
/// and not another group's without regard to case (`except` is the group being renamed).
fn clean(name: &str, groups: &[Group], except: Option<&str>) -> Result<String, ErrorBody> {
    let name = name.trim();
    if name.is_empty() {
        return Err(err(ErrorCode::BadRequest, "a group needs a name"));
    }
    if name.chars().any(char::is_control) {
        return Err(err(
            ErrorCode::BadRequest,
            "a name has no control characters",
        ));
    }
    if name.chars().count() > MAX {
        return Err(err(
            ErrorCode::BadRequest,
            format!("a group's name has at most {MAX} characters"),
        ));
    }
    let lower = name.to_lowercase();
    if groups
        .iter()
        .any(|g| Some(g.id.as_str()) != except && g.name.to_lowercase() == lower)
    {
        return Err(err(
            ErrorCode::Conflict,
            format!("there is a group «{name}» already"),
        ));
    }
    Ok(name.to_owned())
}

fn no_group(group: &str) -> ErrorBody {
    err(ErrorCode::NotFound, format!("no group {group}"))
}

pub fn create(shared: &Shared, name: &str) -> Done {
    let mut core = shared.lock();
    let name = clean(name, &core.model.groups, None)?;
    let id = new_id();
    core.model.groups.push(Group {
        id: id.clone(),
        name,
    });
    core.refresh();
    Ok(Some(json!({ "group": id })))
}

pub fn rename(shared: &Shared, group: &str, name: &str) -> Done {
    let mut core = shared.lock();
    if !core.model.groups.iter().any(|g| g.id == group) {
        return Err(no_group(group));
    }
    let name = clean(name, &core.model.groups, Some(group))?;
    if let Some(g) = core.model.groups.iter_mut().find(|g| g.id == group) {
        g.name = name;
    }
    core.refresh();
    Ok(None)
}

/// The workspaces of the group stay open and belong to none; what named the group goes with it.
pub fn delete(shared: &Shared, group: &str) -> Done {
    let mut core = shared.lock();
    let Some(at) = core.model.groups.iter().position(|g| g.id == group) else {
        return Err(no_group(group));
    };
    core.model.groups.remove(at);
    core.model.grouped.retain(|_, g| g != group);
    core.refresh();
    Ok(None)
}

/// Put the workspace's repository (every workspace with its key) in `group`, or in none.
pub fn move_to(shared: &Shared, workspace: &str, group: Option<&str>) -> Done {
    let mut core = shared.lock();
    // a workspace that is being removed is `conflict`, as for every command that looks one up (REQ-651)
    let key = core.workspace(workspace)?.group_key();
    match group {
        Some(g) if core.model.groups.iter().any(|x| x.id == g) => {
            core.model.grouped.insert(key, g.to_owned());
        }
        Some(g) => return Err(no_group(g)),
        None => {
            core.model.grouped.remove(&key);
        }
    }
    core.refresh();
    Ok(None)
}

/// Swap a group with its neighbour; at either end nothing changes.
pub fn move_group(shared: &Shared, group: &str, delta: i32) -> Done {
    if delta != -1 && delta != 1 {
        return Err(err(ErrorCode::BadRequest, "delta is -1 or 1"));
    }
    let mut core = shared.lock();
    let Some(at) = core.model.groups.iter().position(|g| g.id == group) else {
        return Err(no_group(group));
    };
    let to = at as i64 + i64::from(delta);
    if to < 0 || to >= core.model.groups.len() as i64 {
        return Ok(None);
    }
    core.model.groups.swap(at, to as usize);
    core.refresh();
    Ok(None)
}

/// Swap the repository of a workspace with its neighbour among those of the same group, or of no
/// group (REQ-702): the later repository's workspaces move, in their order, to just before the
/// earlier one's first, which is the order `grouping::group` reads and what `session.json` keeps.
pub fn move_repo(shared: &Shared, workspace: &str, delta: i32) -> Done {
    if delta != -1 && delta != 1 {
        return Err(err(ErrorCode::BadRequest, "delta is -1 or 1"));
    }
    let mut core = shared.lock();
    let ws = core.workspace(workspace)?;
    let Some(project) = ws.project_path() else {
        return Err(err(
            ErrorCode::BadRequest,
            "a folder that is not a repository goes last in its group and has no place to move to",
        ));
    };
    let group = core.model.group_for(&ws.group_key());
    // each workspace's project and group, read once (a folder's is a stat)
    let kept: Vec<(Option<String>, Option<contract::Id>)> = core
        .model
        .workspaces
        .iter()
        .map(|w| (w.project_path(), core.model.group_for(&w.group_key())))
        .collect();
    let mut blocks: Vec<&str> = Vec::new();
    for (p, g) in &kept {
        if let Some(p) = p
            && *g == group
            && !blocks.contains(&p.as_str())
        {
            blocks.push(p);
        }
    }
    let at = blocks.iter().position(|b| *b == project).unwrap_or(0);
    let to = at as i64 + i64::from(delta);
    if to < 0 || to >= blocks.len() as i64 {
        return Ok(None);
    }
    let (earlier, later) = (
        blocks[at.min(to as usize)].to_owned(),
        blocks[at.max(to as usize)].to_owned(),
    );
    let mut moved = Vec::new();
    let mut rest = Vec::new();
    for (w, (p, _)) in core.model.workspaces.drain(..).zip(kept) {
        if p.as_deref() == Some(later.as_str()) {
            moved.push(w);
        } else {
            rest.push((w, p));
        }
    }
    let first = rest
        .iter()
        .position(|(_, p)| p.as_deref() == Some(earlier.as_str()))
        .unwrap_or(rest.len());
    let tail = rest.split_off(first);
    core.model.workspaces = rest
        .into_iter()
        .map(|(w, _)| w)
        .chain(moved)
        .chain(tail.into_iter().map(|(w, _)| w))
        .collect();
    core.refresh();
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, name: &str) -> Group {
        Group {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn a_name_is_trimmed_and_refused_when_empty_long_a_control_or_taken_without_regard_to_case() {
        let groups = [g("1", "salesassist"), g("2", "Jw")];
        assert_eq!(clean("  ideas ", &groups, None).unwrap(), "ideas");
        assert_eq!(
            clean("   ", &groups, None).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            clean("a\nb", &groups, None).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert!(clean(&"x".repeat(40), &groups, None).is_ok());
        assert_eq!(
            clean(&"x".repeat(41), &groups, None).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            clean("SALESASSIST", &groups, None).unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(
            clean("jw", &groups, Some("2")).unwrap(),
            "jw",
            "its own name"
        );
        assert_eq!(
            clean("jw", &groups, Some("1")).unwrap_err().code,
            ErrorCode::Conflict
        );
    }
}
