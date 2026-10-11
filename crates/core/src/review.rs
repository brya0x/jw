//! Reviewing a pull request (docs/specs/rust-tui.md, addendum 14): a
//! worktree at the PR's head, the PR's conversation written to a file, and
//! the prompt that points the agent at it. No terminal here: the TUI runs
//! [`review_stream`] off its main thread and opens the workspace.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::actions::{self, Project};
use crate::connectors::git;
use crate::connectors::github::{Client, PrDetail, PrSummary, Remark};
use crate::core::registry::{self, Entry, Registry};

/// The context file's size limit (RISK-49).
pub const CONTEXT_CAP: usize = 200_000;

/// A review ready to open.
#[derive(Debug, Clone)]
pub struct Reviewed {
    pub entry: Entry,
    /// The worktree was there already: refreshed, and its agent may hold a
    /// conversation (REQ-162, REQ-163).
    pub existed: bool,
    /// What the agent is told (REQ-161).
    pub prompt: String,
    pub context: PathBuf,
    /// Run the project's setup: only for a worktree just created.
    pub setup: bool,
}

/// The worktree name of PR `n`.
pub fn name_of(n: u64) -> String {
    format!("review-{n}")
}

/// Where the head of PR `n` is kept after a fetch.
pub fn head_ref(n: u64) -> String {
    format!("refs/jw/pr-{n}")
}

/// The local branch of PR `n`.
pub fn branch_of(n: u64) -> String {
    format!("review/pr-{n}")
}

/// Fetches the PR's head and base, writes its context file, and creates (or
/// refreshes) the worktree `review-<n>` registered with the PR's base.
/// Nothing is created when a fetch or gh fails (REQ-157); `state` is where
/// the context file goes.
pub fn review_stream(
    p: &Project,
    gh: &Client,
    pr: &PrSummary,
    reg_path: &Path,
    state: &Path,
) -> Result<Reviewed> {
    let name = name_of(pr.number);
    let branch = branch_of(pr.number);
    let reg = Registry::load(reg_path)?;
    let existing = if reg.has(&p.name, &name) {
        Some(reg.find(&p.name, &name)?.clone())
    } else {
        None
    };
    if let Some(e) = &existing
        && !Path::new(&e.path).is_dir()
    {
        bail!(
            "{name} is registered but {} is gone; remove it with X",
            e.path
        );
    }
    let path = PathBuf::from(&p.cfg.root).join(&name);
    if existing.is_none() && path.exists() {
        bail!("{} already exists", path.display());
    }

    let head = p
        .repo
        .fetch_pr(pr.number)
        .with_context(|| format!("fetching the head of #{}", pr.number))?;
    p.repo
        .fetch_branch(&pr.base)
        .with_context(|| format!("fetching origin/{}", pr.base))?;
    let detail = gh.detail(&p.repo.root, pr.number)?;
    let context = write_context(state, pr, &detail)?;

    let (entry, existed) = match existing {
        Some(e) => {
            refresh(&e, &head)?;
            (e, true)
        }
        None => (
            create(p, pr, &name, &branch, &path, reg.next_slot(), reg_path)?,
            false,
        ),
    };
    Ok(Reviewed {
        prompt: prompt(pr, &context),
        entry,
        existed,
        context,
        setup: !existed && !p.cfg.setup.is_empty(),
    })
}

/// Moves a review worktree to the PR's new head, unless it holds changes.
fn refresh(e: &Entry, head: &str) -> Result<()> {
    let dir = Path::new(&e.path);
    if git::dirty(dir)? {
        bail!(
            "{} has changes; commit or discard them, then pick the PR again",
            e.name
        );
    }
    git::reset_hard(dir, head)
}

fn create(
    p: &Project,
    pr: &PrSummary,
    name: &str,
    branch: &str,
    path: &Path,
    slot: u32,
    reg_path: &Path,
) -> Result<Entry> {
    let vars = p.cfg.vars(name, &pr.base, slot);
    p.repo
        .add_worktree_reset(path, branch, &head_ref(pr.number))?;
    let entry = Entry {
        id: registry::new_id()?,
        name: name.to_string(),
        project: p.name.clone(),
        branch: branch.to_string(),
        path: path.display().to_string(),
        slot,
        pr: pr.number,
        created: registry::now_rfc3339(),
        root: p.repo.root.display().to_string(),
        session: crate::session::stored(&crate::session::current()),
        review_base: pr.base.clone(),
        ..Entry::default()
    };
    if let Err(e) = actions::provision(p, &entry, &vars, reg_path) {
        let _ = p.repo.remove_worktree(path);
        let _ = p.repo.delete_branch(branch);
        return Err(e.context(format!("rolled back {name}")));
    }
    Ok(entry)
}

/// Where PR `n`'s context file lives for the current session.
pub fn context_path(state: &Path, n: u64) -> PathBuf {
    state
        .join("reviews")
        .join(crate::session::current())
        .join(format!("pr-{n}.md"))
}

fn write_context(state: &Path, pr: &PrSummary, d: &PrDetail) -> Result<PathBuf> {
    let path = context_path(state, pr.number);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    std::fs::write(&path, context(pr, d)).with_context(|| path.display().to_string())?;
    Ok(path)
}

/// The Markdown the agent reads (REQ-160). The diff is left to git.
pub fn context(pr: &PrSummary, d: &PrDetail) -> String {
    let mut s = format!(
        "# PR #{} {}\n\n{} wants to merge `{}` into `{}`{}\n{}\n\n## Description\n\n{}\n",
        pr.number,
        or(&d.title, &pr.title),
        or(&d.author, &pr.author),
        or(&d.branch, &pr.branch),
        or(&d.base, &pr.base),
        if pr.is_draft { " (draft)" } else { "" },
        or(&d.url, &pr.url),
        blank(&d.body),
    );
    s.push_str(&format!("\n## Files ({})\n\n", d.files.len()));
    for f in &d.files {
        s.push_str(&format!(
            "- {} (+{} -{})\n",
            f.path, f.additions, f.deletions
        ));
    }
    s.push_str("\n## Conversation\n\n");
    remarks(&mut s, &d.comments);
    s.push_str("\n## Reviews\n\n");
    remarks(&mut s, &d.reviews);
    s.push_str("\n## Inline comments\n\n");
    match &d.inline_error {
        Some(e) => s.push_str(&format!("(not available: {e})\n")),
        None => remarks(&mut s, &d.inline),
    }
    cap(s)
}

fn remarks(s: &mut String, list: &[Remark]) {
    if list.is_empty() {
        s.push_str("(none)\n");
    }
    for r in list {
        let at = match (r.path.as_str(), r.line) {
            ("", _) => String::new(),
            (p, Some(l)) => format!(" on {p}:{l}"),
            (p, None) => format!(" on {p}"),
        };
        let state = if r.state.is_empty() {
            String::new()
        } else {
            format!(" [{}]", r.state)
        };
        s.push_str(&format!(
            "### {}{state}{at}\n\n{}\n\n",
            r.author,
            blank(&r.body)
        ));
    }
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

fn blank(s: &str) -> &str {
    if s.trim().is_empty() {
        "(empty)"
    } else {
        s.trim()
    }
}

/// Cuts `s` to [`CONTEXT_CAP`] at a character boundary and says so.
fn cap(mut s: String) -> String {
    if s.len() <= CONTEXT_CAP {
        return s;
    }
    let mut end = CONTEXT_CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str("\n\n[cut here: the context passed 200 KB; `gh pr view` has the rest]\n");
    s
}

/// What the agent is told (REQ-161): one line, so a program that did not
/// turn bracketed paste on does not read it as several messages.
pub fn prompt(pr: &PrSummary, context: &Path) -> String {
    format!(
        "Review pull request #{} \"{}\" ({}, {} into {}). Its description, conversation, reviews, inline comments and files are in {}. See the change with: git diff origin/{}...HEAD. Review it for correctness, missing tests and risks, and report findings by file:line, most serious first. Do not edit files, commit or push.",
        pr.number,
        pr.title,
        pr.author,
        pr.branch,
        pr.base,
        context.display(),
        pr.base,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::git::tests::{must_git, new_test_repo};
    use crate::connectors::github::Changed;
    use crate::core::config::Config;
    use crate::stream::{Stream, review_tree};
    use std::os::unix::fs::PermissionsExt;

    fn pr() -> PrSummary {
        PrSummary {
            number: 7,
            title: "Add login".into(),
            author: "ana".into(),
            branch: "feat/login".into(),
            base: "main".into(),
            ..PrSummary::default()
        }
    }

    #[test]
    fn prompt_is_one_line_naming_the_pr_the_file_and_the_diff() {
        let t = prompt(&pr(), Path::new("/s/pr-7.md"));
        assert!(!t.contains('\n'));
        for part in [
            "#7",
            "Add login",
            "/s/pr-7.md",
            "git diff origin/main...HEAD",
            "file:line",
            "Do not edit files, commit or push",
        ] {
            assert!(t.contains(part), "{part} missing from {t}");
        }
    }

    #[test]
    fn context_lists_files_and_every_kind_of_remark() {
        let d = PrDetail {
            body: "Replaces the form.".into(),
            files: vec![Changed {
                path: "a.rs".into(),
                additions: 3,
                deletions: 1,
            }],
            comments: vec![Remark {
                author: "bo".into(),
                body: "why?".into(),
                ..Remark::default()
            }],
            reviews: vec![Remark {
                author: "cy".into(),
                state: "CHANGES_REQUESTED".into(),
                ..Remark::default()
            }],
            inline: vec![Remark {
                author: "bo".into(),
                body: "nit".into(),
                path: "a.rs".into(),
                line: Some(4),
                ..Remark::default()
            }],
            ..PrDetail::default()
        };
        let c = context(&pr(), &d);
        for part in [
            "# PR #7 Add login",
            "ana wants to merge `feat/login` into `main`",
            "Replaces the form.",
            "- a.rs (+3 -1)",
            "### bo\n\nwhy?",
            "### cy [CHANGES_REQUESTED]",
            "### bo on a.rs:4\n\nnit",
        ] {
            assert!(c.contains(part), "{part} missing from {c}");
        }
    }

    #[test]
    fn context_says_when_inline_comments_are_missing() {
        let d = PrDetail {
            inline_error: Some("gh api: 404".into()),
            ..PrDetail::default()
        };
        assert!(context(&pr(), &d).contains("(not available: gh api: 404)"));
    }

    #[test]
    fn context_is_capped_on_a_character_boundary_and_says_so() {
        let d = PrDetail {
            body: "é".repeat(CONTEXT_CAP),
            ..PrDetail::default()
        };
        let c = context(&pr(), &d);
        assert!(c.len() < CONTEXT_CAP + 200);
        assert!(c.ends_with("`gh pr view` has the rest]\n"));
    }

    /// A gh that answers `pr view` and `api` with fixed JSON.
    fn fake_gh(dir: &Path) -> Client {
        let bin = dir.join("gh");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
case "$1" in
pr) echo '{"title":"Add login","body":"Replaces the form.","author":{"login":"ana"},"headRefName":"feat/login","baseRefName":"trunk","url":"https://github.com/o/r/pull/7","comments":[{"author":{"login":"bo"},"body":"why?"}],"reviews":[],"files":[{"path":"a.txt","additions":1,"deletions":0}]}' ;;
api) echo '[{"user":{"login":"bo"},"path":"a.txt","line":1,"body":"nit"}]' ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Client { bin: Some(bin) }
    }

    fn project(work: &Path, root: &Path) -> Project {
        let mut cfg: Config = toml::from_str("").unwrap();
        cfg.root = root.display().to_string();
        Project {
            name: "myapp".into(),
            repo: git::Repo::open(work).unwrap(),
            cfg,
        }
    }

    /// Puts a commit on origin's `refs/pull/<n>/head`, as GitHub does, and
    /// returns its sha.
    fn publish_pr(work: &Path, n: u64, file: &str) -> String {
        let commit = |msg: &str| {
            std::fs::write(work.join(file), msg).unwrap();
            must_git(work, &["add", file]);
            must_git(work, &["commit", "-q", "-m", msg]);
        };
        must_git(work, &["checkout", "-q", "-B", "pr-src", "trunk"]);
        commit("change");
        must_git(
            work,
            &[
                "push",
                "-q",
                "-f",
                "origin",
                &format!("pr-src:refs/pull/{n}/head"),
            ],
        );
        let sha = git::head(work).unwrap();
        must_git(work, &["checkout", "-q", "trunk"]);
        sha
    }

    fn summary(n: u64) -> PrSummary {
        PrSummary {
            number: n,
            base: "trunk".into(),
            ..pr()
        }
    }

    /// REQ-155, REQ-156, REQ-159, REQ-160: the worktree is at the PR's head,
    /// registered with its base, and the context file is there.
    #[test]
    fn review_stream_creates_the_worktree_at_the_prs_head() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        let state = dir.path().join("state");
        let sha = publish_pr(&work, 7, "a.txt");
        let p = project(&work, &dir.path().join("wt"));
        let gh = fake_gh(dir.path());

        let r = review_stream(&p, &gh, &summary(7), &reg_path, &state).unwrap();
        assert!(!r.existed);
        assert_eq!(r.entry.name, "review-7");
        assert_eq!(
            (
                r.entry.branch.as_str(),
                r.entry.pr,
                r.entry.review_base.as_str()
            ),
            ("review/pr-7", 7, "trunk")
        );
        assert_eq!(git::head(Path::new(&r.entry.path)).unwrap(), sha);
        assert_eq!(
            git::current_branch(Path::new(&r.entry.path)).unwrap(),
            "review/pr-7"
        );
        assert!(p.repo.remote_branch_exists("trunk"));

        let ctx = std::fs::read_to_string(&r.context).unwrap();
        assert!(ctx.contains("Replaces the form.") && ctx.contains("### bo on a.txt:1"));
        assert!(r.prompt.contains(&r.context.display().to_string()));

        let reg = Registry::load(&reg_path).unwrap();
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].review_base, "trunk");
    }

    /// REQ-158, REQ-159: a review resolves to the diff beside the agent,
    /// against the PR's base, and the diff leaf has no process.
    #[test]
    fn a_review_entry_resolves_to_the_diff_and_the_agent() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        publish_pr(&work, 7, "a.txt");
        let p = project(&work, &dir.path().join("wt"));
        let r =
            review_stream(&p, &fake_gh(dir.path()), &summary(7), &reg_path, dir.path()).unwrap();

        let s = Stream::resolve(&r.entry).unwrap();
        assert_eq!(s.base, "trunk");
        assert_eq!(s.tree, review_tree());
        let specs = s.panes().unwrap();
        let roles: Vec<_> = specs.iter().map(|s| s.role.as_str()).collect();
        assert_eq!(roles, ["view:diff", "agent"]);
        assert!(specs[0].cmd.is_none() && specs[1].cmd.is_some());
    }

    /// REQ-162: picking the PR again reuses the worktree and moves it to the
    /// new head.
    #[test]
    fn a_second_review_refreshes_the_same_worktree() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        publish_pr(&work, 7, "a.txt");
        let p = project(&work, &dir.path().join("wt"));
        let gh = fake_gh(dir.path());
        let first = review_stream(&p, &gh, &summary(7), &reg_path, dir.path()).unwrap();

        let newer = publish_pr(&work, 7, "b.txt");
        let again = review_stream(&p, &gh, &summary(7), &reg_path, dir.path()).unwrap();
        assert!(again.existed && !again.setup);
        assert_eq!(again.entry.id, first.entry.id);
        assert_eq!(git::head(Path::new(&again.entry.path)).unwrap(), newer);
        assert_eq!(Registry::load(&reg_path).unwrap().entries.len(), 1);
    }

    /// A review with local changes is not overwritten.
    #[test]
    fn a_dirty_review_is_not_reset() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        publish_pr(&work, 7, "a.txt");
        let p = project(&work, &dir.path().join("wt"));
        let gh = fake_gh(dir.path());
        let r = review_stream(&p, &gh, &summary(7), &reg_path, dir.path()).unwrap();
        std::fs::write(Path::new(&r.entry.path).join("a.txt"), "mine").unwrap();
        publish_pr(&work, 7, "b.txt");
        let err = review_stream(&p, &gh, &summary(7), &reg_path, dir.path()).unwrap_err();
        assert!(format!("{err}").contains("has changes"), "{err}");
    }

    /// REQ-157: a head that cannot be fetched creates nothing.
    #[test]
    fn a_failed_fetch_creates_nothing() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        let state = dir.path().join("state");
        let p = project(&work, &dir.path().join("wt"));
        let err =
            review_stream(&p, &fake_gh(dir.path()), &summary(8), &reg_path, &state).unwrap_err();
        assert!(
            format!("{err:#}").contains("fetching the head of #8"),
            "{err:#}"
        );
        assert!(!dir.path().join("wt/review-8").exists());
        assert!(Registry::load(&reg_path).unwrap().entries.is_empty());
        assert!(!context_path(&state, 8).exists());
    }

    /// REQ-165: removing a review asks nothing about the PR's own commits,
    /// but does about one the reviewer added.
    #[test]
    fn removing_a_review_counts_only_the_reviewers_own_commits() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        publish_pr(&work, 7, "a.txt");
        let p = project(&work, &dir.path().join("wt"));
        let r =
            review_stream(&p, &fake_gh(dir.path()), &summary(7), &reg_path, dir.path()).unwrap();

        let plan = actions::rm_plan(&p, &r.entry).unwrap();
        assert!(
            plan.unpushed.is_empty() && plan.dirty.is_empty(),
            "{plan:?}"
        );
        assert!(plan.deletes.iter().any(|d| d.contains("review/pr-7")));

        let wt = Path::new(&r.entry.path);
        std::fs::write(wt.join("notes.txt"), "mine").unwrap();
        must_git(wt, &["add", "notes.txt"]);
        must_git(wt, &["commit", "-q", "-m", "my note"]);
        assert_eq!(actions::rm_plan(&p, &r.entry).unwrap().unpushed.len(), 1);
    }
}
