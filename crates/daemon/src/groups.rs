//! Groups of workspaces (REQ-662 to REQ-667): a name that gathers the repositories, worktrees and
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
    let Some(key) = core
        .model
        .workspaces
        .iter()
        .find(|w| w.id == workspace)
        .map(|w| w.group_key())
    else {
        return Err(err(
            ErrorCode::NotFound,
            format!("no workspace {workspace}"),
        ));
    };
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
