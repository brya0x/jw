//! The worktrees git already has: listed from `git worktree list`, and opened as workspaces
//! without making, copying or running anything (N99).

use std::path::{Path, PathBuf};

use contract::{ErrorCode, Id, WorkspaceKind, WorktreeInfo};
use serde_json::json;

use crate::commands::{Done, ErrorBody, err};
use crate::git;
use crate::model::{PortBlock, Workspace, new_id};
use crate::server::Shared;
use crate::workspaces::{added, directory};

/// What `git worktree list --porcelain` says of one worktree.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    path: String,
    head: String,
    branch: Option<String>,
    bare: bool,
}

/// The entries of `git worktree list --porcelain`: blocks of `key value` lines apart by a blank
/// line, the first being the repository's main worktree. A block without a path is skipped.
fn parse(text: &str) -> Vec<Entry> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut entry = Entry {
                path: String::new(),
                head: String::new(),
                branch: None,
                bare: false,
            };
            for line in block.lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "worktree" => entry.path = value.to_owned(),
                    "HEAD" => entry.head = value.chars().take(7).collect(),
                    "branch" => {
                        entry.branch = Some(
                            value
                                .strip_prefix("refs/heads/")
                                .unwrap_or(value)
                                .to_owned(),
                        );
                    }
                    "bare" => entry.bare = true,
                    _ => {}
                }
            }
            (!entry.path.is_empty()).then_some(entry)
        })
        .collect()
}

/// The main worktree of the repository `repo` is in, and the other worktrees git lists.
fn read(repo: &str) -> Result<(PathBuf, Vec<Entry>), ErrorBody> {
    let dir = directory(repo)?;
    let mut entries = parse(
        &git::run(&dir, &["worktree", "list", "--porcelain"])
            .map_err(|m| err(ErrorCode::GitFailed, m))?,
    );
    if entries.is_empty() {
        return Err(err(
            ErrorCode::GitFailed,
            format!("git lists no worktree for {repo}"),
        ));
    }
    let main = PathBuf::from(entries.remove(0).path);
    entries.retain(|e| !e.bare);
    Ok((main, entries))
}

/// How many files a worktree has changed, when git says.
fn changes(dir: &Path) -> Option<u32> {
    let out = git::read(dir, &["status", "--porcelain"], 1 << 20).ok()?;
    Some(
        out.bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count() as u32,
    )
}

fn same(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// `list_worktrees`: the worktrees of the repository `repo` is in, but its main one, each with
/// its branch, whether its folder is there, how many files it has changed and the workspace that
/// has it open (REQ-533).
pub fn list(shared: &Shared, repo: &str) -> Done {
    let (_, entries) = read(repo)?;
    let open: Vec<(PathBuf, Id)> = shared
        .lock()
        .model
        .workspaces
        .iter()
        .map(|w| (PathBuf::from(&w.path), w.id.clone()))
        .collect();
    // The status of each is read on a thread of its own: git is slow in a big checkout and the
    // worktrees are many.
    let worktrees: Vec<WorktreeInfo> = std::thread::scope(|scope| {
        let jobs: Vec<_> = entries
            .iter()
            .map(|e| {
                let path = Path::new(&e.path);
                let missing = !path.is_dir();
                let changed = scope.spawn(move || if missing { None } else { changes(path) });
                (e, missing, changed)
            })
            .collect();
        jobs.into_iter()
            .map(|(e, missing, changed)| WorktreeInfo {
                path: e.path.clone(),
                branch: e.branch.clone(),
                head: e.head.clone(),
                missing,
                changes: changed.join().ok().flatten(),
                workspace: open
                    .iter()
                    .find(|(p, _)| same(p, Path::new(&e.path)))
                    .map(|(_, id)| id.clone()),
            })
            .collect()
    });
    Ok(Some(json!({ "worktrees": worktrees })))
}

/// `open_worktree`: a workspace for a worktree `list_worktrees` lists, with no panel and with
/// nothing made, copied or run (REQ-534, REQ-536). Its ports are given as any worktree's are.
pub fn open(shared: &Shared, repo: &str, path: &str) -> Done {
    let (main, entries) = read(repo)?;
    let Some(entry) = entries
        .iter()
        .find(|e| same(Path::new(&e.path), Path::new(path)))
    else {
        return Err(err(
            ErrorCode::NotFound,
            format!("{path} is not a worktree of {repo}"),
        ));
    };
    let dir = Path::new(&entry.path);
    if !dir.is_dir() {
        return Err(err(
            ErrorCode::NotFound,
            format!("the folder {} is not there", entry.path),
        ));
    }
    let dir = directory(&entry.path)?;
    let dir_text = dir.to_string_lossy().into_owned();
    // The workspace of a worktree made by jwn keeps the path as it was joined, not as the disk
    // spells it, so the paths are compared as the disk does.
    if let Some(w) = shared
        .lock()
        .model
        .workspaces
        .iter()
        .find(|w| same(Path::new(&w.path), &dir))
    {
        return Err(err(
            ErrorCode::Conflict,
            format!("{dir_text} is already open as {:?}", w.name),
        ));
    }
    let project = crate::start::load(shared, &main)?;
    if !project.ports.is_empty() && shared.lock().model.free_slot().is_none() {
        return Err(err(ErrorCode::Conflict, "no block of ports is free"));
    }
    let repo_name = main
        .file_name()
        .map_or_else(|| "repo".into(), |n| n.to_string_lossy().into_owned());
    let name = match &entry.branch {
        Some(branch) => format!("{repo_name}: {branch}"),
        None => format!("{repo_name}: ({})", entry.head),
    };
    added(
        shared,
        Workspace {
            id: new_id(),
            name,
            kind: WorkspaceKind::Worktree,
            path: dir_text,
            project: Some(main.to_string_lossy().into_owned()),
            branch: entry.branch.clone(),
            panes: Vec::new(),
            layout: None,
            attention: Vec::new(),
            adopted: true,
            ports: (!project.ports.is_empty()).then(|| PortBlock {
                slot: 0,
                names: project.ports.clone(),
            }),
            github: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: &str = "worktree /code/app\nHEAD 4f2a9c1d00000000000000000000000000000000\nbranch refs/heads/main\n\n\
        worktree /code/app-wt/login\nHEAD 9a8b7c6e00000000000000000000000000000000\nbranch refs/heads/fix/login\n\n\
        worktree /code/app-wt/bisect\nHEAD 1d2e3f4a00000000000000000000000000000000\ndetached\nlocked testing\n\n\
        worktree /code/app.git\nbare\n";

    #[test]
    fn what_git_lists_is_read_with_its_branches_detached_heads_and_bare_repositories() {
        let list = parse(LISTED);
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].path, "/code/app");
        assert_eq!(
            list[1].branch.as_deref(),
            Some("fix/login"),
            "a prefix is kept"
        );
        assert_eq!(list[1].head, "9a8b7c6", "the first seven digits");
        assert_eq!(list[2].branch, None, "a detached head has no branch");
        assert!(list[3].bare);
    }

    #[test]
    fn a_block_with_no_path_is_skipped_and_nothing_is_nothing() {
        assert!(parse("").is_empty());
        assert_eq!(parse("HEAD abc\n\nworktree /x\nHEAD def1234\n").len(), 1);
    }
}
