//! Reviewing a pull request (spec REQ-581 to REQ-586): the open pull requests of a repository,
//! and a worktree that has one's head, with a file that says what the pull request says. GitHub
//! is only read, with `gh`, so the person's own login and host are used; nothing is written to it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use contract::{ErrorCode, OpenKind, PrFile, PrInfo, PrList, Review, WorkspaceKind};
use serde_json::{Value, json};

use crate::commands::{Done, ErrorBody, err};
use crate::git;
use crate::github::run_gh;
use crate::model::{Workspace, new_id};
use crate::server::Shared;

/// `gh` may take its time on a long list or a pull request with many comments.
const WITHIN: Duration = Duration::from_secs(40);
/// The most of the context file that is written: the rest is cut and the cut said.
const CONTEXT_CAP: usize = 200 * 1024;

/// The fields `gh pr list` is asked for.
const LIST_FIELDS: &str = "number,title,body,author,headRefName,baseRefName,isDraft,updatedAt,reviewDecision,additions,deletions,url,files";
/// The fields `gh pr view` is asked for.
const VIEW_FIELDS: &str = "title,body,author,baseRefName,headRefName,comments,reviews,files,url";

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn count(v: &Value) -> u32 {
    u32::try_from(v.as_u64().unwrap_or(0)).unwrap_or(u32::MAX)
}

/// `gh pr list --json …` as pull requests, most recently updated first.
pub fn parse_list(json: &str) -> Result<Vec<PrInfo>, String> {
    let list: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let mut prs: Vec<PrInfo> = list
        .as_array()
        .ok_or("not a list")?
        .iter()
        .map(|pr| PrInfo {
            number: pr["number"].as_u64().unwrap_or(0),
            title: text(&pr["title"]),
            author: text(&pr["author"]["login"]),
            branch: text(&pr["headRefName"]),
            base: text(&pr["baseRefName"]),
            draft: pr["isDraft"].as_bool().unwrap_or(false),
            updated: text(&pr["updatedAt"]),
            decision: Some(text(&pr["reviewDecision"])).filter(|d| !d.is_empty()),
            additions: count(&pr["additions"]),
            deletions: count(&pr["deletions"]),
            body: text(&pr["body"]),
            url: text(&pr["url"]),
            files: pr["files"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| PrFile {
                    path: text(&f["path"]),
                    additions: count(&f["additions"]),
                    deletions: count(&f["deletions"]),
                })
                .collect(),
        })
        .filter(|p| p.number > 0)
        .collect();
    prs.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(prs)
}

/// What `gh` is run with and where: it is a tool of the person's, and the daemon may have none.
fn gh_in(
    shared: &Shared,
    workspace: &str,
) -> Result<(PathBuf, PathBuf, Option<PathBuf>), ErrorBody> {
    let core = shared.lock();
    let ws = core.workspace(workspace)?;
    let program = core
        .gh
        .clone()
        .ok_or_else(|| err(ErrorCode::Unsupported, "gh is not installed"))?;
    Ok((
        program,
        PathBuf::from(&ws.path),
        ws.project.clone().map(PathBuf::from),
    ))
}

/// `list_prs`: the open pull requests of the repository the workspace is in.
pub fn list(shared: &Shared, workspace: &str, requested: bool) -> Done {
    let (program, dir, _) = gh_in(shared, workspace)?;
    let mut args = vec![
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        LIST_FIELDS,
    ];
    if requested {
        args.extend(["--search", "review-requested:@me"]);
    }
    let out = run_gh(&program, &dir, &args, WITHIN).map_err(|m| err(ErrorCode::GitFailed, m))?;
    let prs = parse_list(&out).map_err(|m| err(ErrorCode::GitFailed, format!("gh: {m}")))?;
    Ok(Some(
        serde_json::to_value(PrList { prs }).expect("prs serialise"),
    ))
}

/// Where the file that briefs the review of pull request `pr` of `repo` is kept.
pub fn context_path(state_dir: &Path, repo: &str, pr: u64) -> PathBuf {
    state_dir.join("reviews").join(format!("{repo}-pr-{pr}.md"))
}

/// The ref that holds the head of a pull request in the repository, a name of this product's own so
/// that a review branch that is checked out can be brought up to date.
fn head_ref(pr: u64) -> String {
    format!("refs/{}/pr-{pr}", identity::get().name)
}

/// What the pull request says, as a file Claude can read: its description, conversation, reviews,
/// inline comments by file and line, and the files it changes. The diff stays in git.
pub fn context(pr: u64, view: &Value, inline: &Value) -> String {
    let who = |v: &Value| {
        let login = text(&v["author"]["login"]);
        if login.is_empty() {
            text(&v["user"]["login"])
        } else {
            login
        }
    };
    let mut out = format!(
        "# Pull request #{pr}: {}\n\n{} · {} → {} · {}\n\n## Description\n\n{}\n",
        text(&view["title"]),
        who(view),
        text(&view["headRefName"]),
        text(&view["baseRefName"]),
        text(&view["url"]),
        text(&view["body"]).trim()
    );
    let comments = view["comments"].as_array().cloned().unwrap_or_default();
    if !comments.is_empty() {
        out.push_str("\n## Conversation\n");
        for c in &comments {
            out.push_str(&format!(
                "\n**{}** ({}):\n{}\n",
                who(c),
                text(&c["createdAt"]),
                text(&c["body"]).trim()
            ));
        }
    }
    let reviews = view["reviews"].as_array().cloned().unwrap_or_default();
    if !reviews.is_empty() {
        out.push_str("\n## Reviews\n");
        for r in &reviews {
            out.push_str(&format!(
                "\n**{}** {} ({}):\n{}\n",
                who(r),
                text(&r["state"]),
                text(&r["submittedAt"]),
                text(&r["body"]).trim()
            ));
        }
    }
    let notes = inline.as_array().cloned().unwrap_or_default();
    if !notes.is_empty() {
        out.push_str("\n## Comments on the code\n");
        for n in &notes {
            let line = n["line"]
                .as_u64()
                .or_else(|| n["original_line"].as_u64())
                .unwrap_or(0);
            out.push_str(&format!(
                "\n`{}:{line}` **{}**:\n{}\n",
                text(&n["path"]),
                who(n),
                text(&n["body"]).trim()
            ));
        }
    }
    let files = view["files"].as_array().cloned().unwrap_or_default();
    if !files.is_empty() {
        out.push_str("\n## Files\n\n");
        for f in &files {
            out.push_str(&format!(
                "- {} (+{} −{})\n",
                text(&f["path"]),
                count(&f["additions"]),
                count(&f["deletions"])
            ));
        }
    }
    if out.len() > CONTEXT_CAP {
        let mut cut = CONTEXT_CAP;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n\n(cut: the rest of this file was over 200 KB)\n");
    }
    out
}

/// What the review needs of the workspace it starts from.
struct Origin {
    program: PathBuf,
    dir: PathBuf,
    top: PathBuf,
    state_dir: PathBuf,
}

fn origin(shared: &Shared, workspace: &str) -> Result<Origin, ErrorBody> {
    let (program, dir, project) = gh_in(shared, workspace)?;
    let top = match project {
        Some(p) => p,
        None => PathBuf::from(git::toplevel(&dir).map_err(|m| err(ErrorCode::GitFailed, m))?),
    };
    let state_dir = shared
        .lock()
        .state_dir
        .clone()
        .ok_or_else(|| err(ErrorCode::Io, "the daemon keeps no state folder"))?;
    Ok(Origin {
        program,
        dir,
        top,
        state_dir,
    })
}

/// `review_pr`: fetch the head of the pull request and its base, make the worktree `review-<n>` on
/// the branch `review/pr-<n>` at the head (or bring the one that is there up to date), write the
/// file that says what the pull request says, and open the workspace with Claude. The network and
/// git run with no lock held.
pub fn review(shared: &Shared, workspace: &str, pr: u64) -> Done {
    if pr == 0 {
        return Err(err(ErrorCode::BadRequest, "a pull request has a number"));
    }
    let from = origin(shared, workspace)?;
    let git_err = |m: String| err(ErrorCode::GitFailed, m);
    let n = pr.to_string();
    let gh = |args: &[&str]| run_gh(&from.program, &from.dir, args, WITHIN);
    let view: Value =
        serde_json::from_str(&gh(&["pr", "view", &n, "--json", VIEW_FIELDS]).map_err(git_err)?)
            .map_err(|e| err(ErrorCode::GitFailed, format!("gh: not JSON: {e}")))?;
    let base = text(&view["baseRefName"]);
    if base.is_empty() || !git::valid_branch(&from.top, &base) {
        return Err(err(
            ErrorCode::GitFailed,
            format!("pull request {pr} has no usable base branch"),
        ));
    }
    // Comments on the code are one more read; a pull request that has none is no failure.
    let inline = gh(&[
        "api",
        &format!("repos/{{owner}}/{{repo}}/pulls/{n}/comments"),
    ])
    .ok()
    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    .unwrap_or(Value::Null);
    let head = head_ref(pr);
    git::run(
        &from.top,
        &["fetch", "origin", &format!("+pull/{n}/head:{head}")],
    )
    .map_err(git_err)?;
    git::run(&from.top, &["fetch", "origin", &base]).map_err(git_err)?;

    let repo = from
        .top
        .file_name()
        .map_or_else(|| "repo".into(), |s| s.to_string_lossy().into_owned());
    let file = context_path(&from.state_dir, &repo, pr);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| err(ErrorCode::Io, format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(&file, context(pr, &view, &inline))
        .map_err(|e| err(ErrorCode::Io, format!("{}: {e}", file.display())))?;

    let project = crate::start::load(shared, &from.top)?;
    let dest = project.root.join(format!("review-{pr}"));
    let dest_text = dest.to_string_lossy().into_owned();
    let review = Review {
        pr,
        base: base.clone(),
    };
    let branch = format!("review/pr-{pr}");
    let open = shared
        .lock()
        .model
        .workspaces
        .iter()
        .find(|w| w.path == dest_text)
        .map(|w| w.id.clone());
    if let Some(id) = open {
        // A review that is there is brought to the new head, unless it has changes of the person's.
        let dirty = !git::run(&dest, &["status", "--porcelain"])
            .map_err(git_err)?
            .is_empty();
        if !dirty {
            git::run(&dest, &["reset", "--hard", &head]).map_err(git_err)?;
        }
        crate::diff::set_review_base(&dest, &format!("origin/{base}")).map_err(git_err)?;
        let mut core = shared.lock();
        if let Some(ws) = core.model.workspaces.iter_mut().find(|w| w.id == id) {
            ws.review = Some(review);
        }
        core.refresh();
        return Ok(Some(
            json!({ "workspace": id, "existed": true, "dirty": dirty }),
        ));
    }
    if dest.exists() {
        return Err(err(
            ErrorCode::Conflict,
            format!("{dest_text} already exists"),
        ));
    }
    git::run(
        &from.top,
        &["worktree", "add", "-B", &branch, &dest_text, &head],
    )
    .map_err(git_err)?;
    crate::provision::copy_files(&from.top, &dest, &project.copy);
    crate::diff::set_review_base(&dest, &format!("origin/{base}")).map_err(git_err)?;
    let id = new_id();
    crate::workspaces::added(
        shared,
        Workspace {
            id: id.clone(),
            name: format!("{repo}: {branch}"),
            kind: WorkspaceKind::Worktree,
            path: dest_text,
            project: Some(from.top.to_string_lossy().into_owned()),
            branch: Some(branch),
            panes: Vec::new(),
            layout: None,
            attention: Vec::new(),
            adopted: false,
            ports: None,
            review: Some(review),
            github: None,
        },
    )?;
    crate::start::start_workspace(shared, &id, Some(OpenKind::Claude))?;
    Ok(Some(
        json!({ "workspace": id, "existed": false, "dirty": false }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_from_gh_is_read_newest_first_and_a_missing_decision_is_none() {
        let json = r#"[
          {"number":3,"title":"a","author":{"login":"ana"},"headRefName":"ana/a","baseRefName":"main","isDraft":false,"updatedAt":"2026-10-18T10:00:00Z","reviewDecision":"","additions":2,"deletions":1,"url":"u3","body":"b","files":[{"path":"x.rs","additions":2,"deletions":1}]},
          {"number":7,"title":"b","author":{"login":"luis"},"headRefName":"luis/b","baseRefName":"main","isDraft":true,"updatedAt":"2026-10-19T10:00:00Z","reviewDecision":"APPROVED","additions":0,"deletions":0,"url":"u7","body":"","files":[]}
        ]"#;
        let prs = parse_list(json).unwrap();
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 3]);
        assert_eq!(prs[0].decision.as_deref(), Some("APPROVED"));
        assert!(prs[0].draft);
        assert_eq!(prs[1].decision, None);
        assert_eq!(prs[1].files[0].path, "x.rs");
        assert!(parse_list("not json").is_err());
        assert!(parse_list("{}").is_err());
    }

    #[test]
    fn the_context_says_what_the_pull_request_says_and_is_cut_when_it_is_too_long() {
        let view = json!({
            "title": "Limit", "body": "Adds a bucket.", "author": {"login": "maria"},
            "headRefName": "maria/limit", "baseRefName": "main", "url": "https://x/pull/7",
            "comments": [{"author": {"login": "luis"}, "createdAt": "t1", "body": "why?"}],
            "reviews": [{"author": {"login": "ana"}, "state": "CHANGES_REQUESTED", "submittedAt": "t2", "body": "see line 9"}],
            "files": [{"path": "src/limit.rs", "additions": 188, "deletions": 0}]
        });
        let inline = json!([{"path": "src/limit.rs", "line": 9, "user": {"login": "ana"}, "body": "off by one"}]);
        let text = context(7, &view, &inline);
        for want in [
            "# Pull request #7: Limit",
            "maria · maria/limit → main",
            "Adds a bucket.",
            "**luis** (t1)",
            "**ana** CHANGES_REQUESTED",
            "`src/limit.rs:9` **ana**",
            "- src/limit.rs (+188 −0)",
        ] {
            assert!(text.contains(want), "{want:?} is not in\n{text}");
        }
        let long = json!({"title": "t", "body": "x".repeat(CONTEXT_CAP * 2)});
        let cut = context(1, &long, &Value::Null);
        assert!(cut.len() < CONTEXT_CAP + 200 && cut.contains("(cut:"));
    }
}
