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
    Ok((main, entries))
}

/// How many entries `git status` lists for a worktree, when git says.
fn changes(dir: &Path) -> Option<u32> {
    let out = git::read(dir, &["status", "--porcelain"], 1 << 20).ok()?;
    let lines = out
        .bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .count();
    Some(u32::try_from(lines).unwrap_or(u32::MAX))
}

/// A path as the disk spells it, or as it was given when the disk cannot say.
fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The workspaces' folders and ids, copied so the paths are compared with the lock dropped.
fn open_folders(shared: &Shared) -> Vec<(PathBuf, Id, String)> {
    shared
        .lock()
        .model
        .workspaces
        .iter()
        .map(|w| (PathBuf::from(&w.path), w.id.clone(), w.name.clone()))
        .collect()
}

/// Worktrees whose status is read together: git is slow in a big checkout, and a repository can
/// have tens of them.
const AT_ONCE: usize = 8;

/// Whether each worktree's folder is missing and, when it is not, how many files it has changed.
fn statuses(entries: &[Entry]) -> Vec<(bool, Option<u32>)> {
    let mut out = Vec::with_capacity(entries.len());
    for group in entries.chunks(AT_ONCE) {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = group
                .iter()
                .map(|e| {
                    let path = Path::new(&e.path);
                    let missing = !path.is_dir();
                    (
                        missing,
                        scope.spawn(move || if missing { None } else { changes(path) }),
                    )
                })
                .collect();
            out.extend(
                jobs.into_iter()
                    .map(|(missing, job)| (missing, job.join().ok().flatten())),
            );
        });
    }
    out
}

/// `list_worktrees`: the worktrees of the repository `repo` is in, but its main one, each with
/// its branch, whether its folder is there, how many files it has changed and the workspace that
/// has it open (REQ-533).
pub fn list(shared: &Shared, repo: &str) -> Done {
    let (_, entries) = read(repo)?;
    let open: Vec<(PathBuf, Id)> = open_folders(shared)
        .into_iter()
        .map(|(path, id, _)| (canon(&path), id))
        .collect();
    let worktrees: Vec<WorktreeInfo> = entries
        .iter()
        .zip(statuses(&entries))
        .map(|(e, (missing, changes))| {
            let at = canon(Path::new(&e.path));
            WorktreeInfo {
                path: e.path.clone(),
                branch: e.branch.clone(),
                head: e.head.clone(),
                missing,
                changes,
                workspace: open
                    .iter()
                    .find(|(p, _)| *p == at)
                    .map(|(_, id)| id.clone()),
            }
        })
        .collect();
    Ok(Some(json!({ "worktrees": worktrees })))
}

/// `open_worktree`: a workspace for a worktree `list_worktrees` lists, with no panel and with
/// nothing made, copied or run (REQ-534, REQ-536). Its ports are given as any worktree's are.
pub fn open(shared: &Shared, repo: &str, path: &str) -> Done {
    let (main, entries) = read(repo)?;
    let wanted = canon(Path::new(path));
    let Some(entry) = entries.iter().find(|e| canon(Path::new(&e.path)) == wanted) else {
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
    if let Some((_, _, name)) = open_folders(shared)
        .into_iter()
        .find(|(p, _, _)| canon(p) == dir)
    {
        return Err(err(
            ErrorCode::Conflict,
            format!("{dir_text} is already open as {name:?}"),
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
            views: Vec::new(),
            places: Default::default(),
            adopted: true,
            ports: (!project.ports.is_empty()).then(|| PortBlock {
                slot: 0,
                names: project.ports.clone(),
            }),
            review: None,
            github: None,
            chat: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: &str = "worktree /code/app\nHEAD 4f2a9c1d00000000000000000000000000000000\nbranch refs/heads/main\n\n\
        worktree /code/app-wt/login\nHEAD 9a8b7c6e00000000000000000000000000000000\nbranch refs/heads/fix/login\n\n\
        worktree /code/app-wt/bisect\nHEAD 1d2e3f4a00000000000000000000000000000000\ndetached\nlocked testing\n\n\
        worktree /code/my worktrees/old\nHEAD aa00bb1100000000000000000000000000000000\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n";

    #[test]
    fn what_git_lists_is_read_with_its_branches_detached_heads_spaces_and_the_notes_it_adds() {
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
        assert_eq!(list[3].path, "/code/my worktrees/old", "a path with spaces");
        assert_eq!(
            list[3].branch.as_deref(),
            Some("old"),
            "a prunable note is ignored"
        );
    }

    #[test]
    fn a_block_with_no_path_is_skipped_and_nothing_is_nothing() {
        assert!(parse("").is_empty());
        assert_eq!(parse("HEAD abc\n\nworktree /x\nHEAD def1234\n").len(), 1);
    }
}
