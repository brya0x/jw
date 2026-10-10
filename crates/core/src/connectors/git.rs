//! The git commands jw needs. It shells out to the git binary instead of
//! using a library, so behaviour matches what you get typing the same
//! command yourself. Port of internal/connectors/git/git.go.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

/// The main checkout of a repository, even when opened from inside one of
/// its worktrees.
#[derive(Debug, Clone)]
pub struct Repo {
    /// Main checkout directory.
    pub root: PathBuf,
    /// The shared .git directory.
    pub common_dir: PathBuf,
    /// URL of origin.
    pub remote: String,
}

/// Runs git in `dir` and returns trimmed stdout. On failure the error carries
/// git's own stderr, which is usually the most useful message.
fn run(dir: &Path, args: &[&str]) -> Result<String> {
    output(dir, args).map(|o| o.trim().to_string())
}

/// [`run`] with stdout as git wrote it, for output whose leading or trailing
/// whitespace means something (porcelain status, a diff).
pub(crate) fn output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| anyhow!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if msg.is_empty() {
            out.status.to_string()
        } else {
            msg
        };
        bail!("git {}: {msg}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ok(dir: &Path, args: &[&str]) -> bool {
    run(dir, args).is_ok()
}

fn lines(out: String) -> Vec<String> {
    if out.is_empty() {
        return Vec::new();
    }
    out.lines().map(str::to_string).collect()
}

impl Repo {
    /// Finds the repository that contains `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let common = run(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map_err(|_| anyhow!("{} is not inside a git repository", dir.display()))?;
        let common_dir = PathBuf::from(common);
        let root = common_dir
            .parent()
            .ok_or_else(|| anyhow!("{} has no parent", common_dir.display()))?
            .to_path_buf();
        let remote = run(&root, &["remote", "get-url", "origin"])
            .map_err(|_| anyhow!("{} has no origin remote", root.display()))?;
        Ok(Self {
            root,
            common_dir,
            remote,
        })
    }

    /// Reads origin/HEAD. It never assumes main: if the ref is missing
    /// locally it asks the remote once, and fails if that doesn't work.
    pub fn default_branch(&self) -> Result<String> {
        let head = ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"];
        let r = match run(&self.root, &head) {
            Ok(r) => r,
            Err(_) => {
                run(&self.root, &["remote", "set-head", "origin", "--auto"])
                    .map_err(|e| anyhow!("cannot resolve the default branch of origin: {e}"))?;
                run(&self.root, &head)?
            }
        };
        Ok(r.strip_prefix("origin/").unwrap_or(&r).to_string())
    }

    /// Updates the remote-tracking refs from origin.
    pub fn fetch(&self) -> Result<()> {
        run(&self.root, &["fetch", "--quiet", "origin"]).map(drop)
    }

    /// Whether a local branch with that name exists.
    pub fn branch_exists(&self, branch: &str) -> bool {
        let r = format!("refs/heads/{branch}");
        ok(&self.root, &["rev-parse", "--verify", "--quiet", &r])
    }

    /// Whether origin has the branch (as last fetched).
    pub fn remote_branch_exists(&self, branch: &str) -> bool {
        let r = format!("refs/remotes/origin/{branch}");
        ok(&self.root, &["rev-parse", "--verify", "--quiet", &r])
    }

    /// Creates `path` with a new branch starting at `r#ref`.
    pub fn add_worktree(&self, path: &Path, branch: &str, r#ref: &str) -> Result<()> {
        let p = path.to_string_lossy();
        run(
            &self.root,
            &["worktree", "add", "--quiet", "-b", branch, &p, r#ref],
        )
        .map(drop)
    }

    /// Checks out an existing branch at `path`. A branch that only exists on
    /// origin gets a local branch tracking it (git's own DWIM).
    pub fn add_worktree_existing(&self, path: &Path, branch: &str) -> Result<()> {
        let p = path.to_string_lossy();
        run(&self.root, &["worktree", "add", "--quiet", &p, branch]).map(drop)
    }

    /// Deletes the worktree at `path`, discarding local changes.
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        let p = path.to_string_lossy();
        run(&self.root, &["worktree", "remove", "--force", &p]).map(drop)
    }

    /// Moves a worktree's folder; git keeps track of it.
    pub fn move_worktree(&self, from: &Path, to: &Path) -> Result<()> {
        let (f, t) = (from.to_string_lossy(), to.to_string_lossy());
        run(&self.root, &["worktree", "move", &f, &t]).map(drop)
    }

    pub fn rename_branch(&self, from: &str, to: &str) -> Result<()> {
        run(&self.root, &["branch", "-m", from, to]).map(drop)
    }

    /// Forgets worktrees whose directory is gone.
    pub fn prune_worktrees(&self) -> Result<()> {
        run(&self.root, &["worktree", "prune"]).map(drop)
    }

    /// Deletes a local branch only if git sees it merged.
    pub fn delete_merged_branch(&self, branch: &str) -> Result<()> {
        run(&self.root, &["branch", "-d", branch]).map(drop)
    }

    /// Force-deletes a local branch.
    pub fn delete_branch(&self, branch: &str) -> Result<()> {
        run(&self.root, &["branch", "-D", branch]).map(drop)
    }

    /// The files git ignores in the main checkout, relative to it. An ignored
    /// directory (node_modules/, a nested worktree) is left out, so the list
    /// stays short.
    pub fn ignored_files(&self) -> Result<Vec<String>> {
        let out = run(
            &self.root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ],
        )?;
        Ok(lines(out)
            .into_iter()
            .filter(|l| !l.ends_with('/'))
            .collect())
    }

    /// Whether the object database has commit `oid`.
    pub fn has_commit(&self, oid: &str) -> bool {
        ok(
            &self.root,
            &["cat-file", "-e", &format!("{oid}^{{commit}}")],
        )
    }

    /// Downloads one commit by sha (GitHub allows this).
    pub fn fetch_commit(&self, oid: &str) -> Result<()> {
        run(&self.root, &["fetch", "--quiet", "origin", oid]).map(drop)
    }

    /// Whether commit `a` is contained in commit `b`'s history.
    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        ok(&self.root, &["merge-base", "--is-ancestor", a, b])
    }

    /// Adds `pattern` to .git/info/exclude unless it is already there. That
    /// file is shared by every worktree and never committed.
    pub fn exclude(&self, pattern: &str) -> Result<()> {
        let path = self.common_dir.join("info").join("exclude");
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| path.display().to_string()),
        };
        if data.lines().any(|l| l.trim() == pattern) {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .with_context(|| path.display().to_string())?;
        let sep = if !data.is_empty() && !data.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        writeln!(f, "{sep}{pattern}")?;
        Ok(())
    }
}

/// Whether the worktree at `dir` has uncommitted or untracked files.
pub fn dirty(dir: &Path) -> Result<bool> {
    Ok(!run(dir, &["status", "--porcelain"])?.is_empty())
}

/// Uncommitted and untracked paths in the worktree at `dir`, as porcelain
/// status lines (` M src/a.rs`, `?? new.txt`).
pub fn dirty_files(dir: &Path) -> Result<Vec<String>> {
    output(dir, &["status", "--porcelain"]).map(lines)
}

/// The commits in `dir`'s HEAD that no remote branch has: the work that only
/// exists on this machine.
pub fn unpushed(dir: &Path) -> Result<Vec<String>> {
    run(dir, &["log", "--oneline", "HEAD", "--not", "--remotes"]).map(lines)
}

/// A rebase or merge left half-done in the worktree at `dir`.
pub fn operation(dir: &Path) -> Option<&'static str> {
    [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"),
    ]
    .into_iter()
    .find(|(path, _)| {
        run(dir, &["rev-parse", "--git-path", path])
            .map(|p| dir.join(p).exists())
            .unwrap_or(false)
    })
    .map(|(_, name)| name)
}

/// Counts the commits HEAD lacks from `r#ref` (behind) and has on top of it
/// (ahead).
pub fn divergence(dir: &Path, r#ref: &str) -> Result<(u32, u32)> {
    let out = run(
        dir,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{ref}...HEAD"),
        ],
    )?;
    let mut n = out.split_whitespace().map(str::parse::<u32>);
    match (n.next(), n.next()) {
        (Some(Ok(behind)), Some(Ok(ahead))) => Ok((behind, ahead)),
        _ => bail!("git rev-list: unexpected output {out:?}"),
    }
}

/// Replays HEAD's own commits onto `r#ref`. On conflict git stops half-way
/// and the error is returned; [`conflicts`] says which files.
pub fn rebase(dir: &Path, r#ref: &str) -> Result<()> {
    run(dir, &["rebase", r#ref]).map(drop)
}

/// Merges `r#ref` into HEAD with git's default message.
pub fn merge(dir: &Path, r#ref: &str) -> Result<()> {
    run(dir, &["merge", "--no-edit", r#ref]).map(drop)
}

/// The files left unmerged in the worktree at `dir`.
pub fn conflicts(dir: &Path) -> Result<Vec<String>> {
    run(dir, &["diff", "--name-only", "--diff-filter=U"]).map(lines)
}

/// The branch checked out in `dir`, empty on a detached HEAD.
pub fn current_branch(dir: &Path) -> Result<String> {
    run(dir, &["branch", "--show-current"])
}

/// The repository `dir` is in and what it has checked out, read from
/// `.git` without running git, so it is cheap enough for every refresh:
/// the checkout's root and its branch (or a short commit when detached).
pub fn head_of(dir: &Path) -> Option<(PathBuf, String)> {
    let root = dir.ancestors().find(|d| d.join(".git").exists())?;
    let dot = root.join(".git");
    // A worktree's .git is a file pointing at its git dir.
    let git_dir = if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if p.is_absolute() { p } else { root.join(p) }
    } else {
        dot
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let branch = match head.strip_prefix("ref: refs/heads/") {
        Some(b) => b.to_string(),
        None => head.chars().take(7).collect(),
    };
    Some((root.to_path_buf(), branch))
}

/// Brings the branch checked out in `dir` up to its upstream, refusing
/// anything but a fast-forward.
pub fn pull_ff(dir: &Path) -> Result<()> {
    run(dir, &["pull", "--ff-only"]).map(drop)
}

/// The commit checked out in `dir`.
pub fn head(dir: &Path) -> Result<String> {
    run(dir, &["rev-parse", "HEAD"])
}

/// A short project name from a remote URL: git@github.com:acme/myapp.git and
/// https://github.com/acme/myapp both give "myapp".
pub fn project_name(remote: &str) -> String {
    let name = remote.trim_end_matches('/');
    let name = name.strip_suffix(".git").unwrap_or(name);
    match name.rfind(['/', ':']) {
        Some(i) => name[i + 1..].to_string(),
        None => name.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds origin (bare) + a clone with one commit, on a default branch
    /// deliberately not called main. Returns the tempdir and the clone.
    pub(crate) fn new_test_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("myapp.git");
        let work = dir.path().join("myapp");
        let (o, w) = (origin.to_str().unwrap(), work.to_str().unwrap());
        must_git(dir.path(), &["init", "-q", "--bare", "-b", "trunk", o]);
        must_git(dir.path(), &["init", "-q", "-b", "trunk", w]);
        must_git(
            &work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        must_git(&work, &["remote", "add", "origin", o]);
        must_git(&work, &["push", "-q", "-u", "origin", "trunk"]);
        // What jw itself runs (a rebase in sync) commits too, and a CI
        // runner has no identity of its own. Worktrees share this config.
        must_git(&work, &["config", "user.name", "t"]);
        must_git(&work, &["config", "user.email", "t@t"]);
        (dir, work)
    }

    #[test]
    fn head_of_reads_the_checkout_and_its_worktrees() {
        let (dir, work) = new_test_repo();
        std::fs::create_dir_all(work.join("src/deep")).unwrap();
        let (root, branch) = head_of(&work.join("src/deep")).unwrap();
        assert_eq!((root, branch.as_str()), (work.clone(), "trunk"));

        let wt = dir.path().join("wt");
        must_git(
            &work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/x",
                wt.to_str().unwrap(),
            ],
        );
        assert_eq!(head_of(&wt).unwrap().1, "feat/x");
        must_git(&work, &["checkout", "-q", "--detach"]);
        assert_eq!(head_of(&work).unwrap().1.len(), 7);
    }

    /// Ignores the developer's global git config (signing, hooks, templates).
    pub(crate) fn must_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Resolves macOS's /var → /private/var so paths compare equal.
    fn real(p: &Path) -> PathBuf {
        p.canonicalize().unwrap()
    }

    #[test]
    fn default_branch_is_not_assumed() {
        let (_d, work) = new_test_repo();
        assert_eq!(
            Repo::open(&work).unwrap().default_branch().unwrap(),
            "trunk"
        );
    }

    #[test]
    fn open_from_worktree_finds_main_checkout() {
        let (_d, work) = new_test_repo();
        let repo = Repo::open(&work).unwrap();
        let wt = work.parent().unwrap().join("myapp-wt").join("feature");
        repo.add_worktree(&wt, "feat/feature", "origin/trunk")
            .unwrap();
        assert!(repo.branch_exists("feat/feature"));
        assert!(repo.remote_branch_exists("trunk"));
        assert!(!repo.remote_branch_exists("feat/feature"));

        let from_wt = Repo::open(&wt).unwrap();
        assert_eq!(real(&from_wt.root), real(&work));
        assert_eq!(current_branch(&wt).unwrap(), "feat/feature");
        assert_eq!(divergence(&wt, "origin/trunk").unwrap(), (0, 0));
        assert!(!dirty(&wt).unwrap());
        std::fs::write(wt.join("new.txt"), "x").unwrap();
        assert_eq!(dirty_files(&wt).unwrap(), ["?? new.txt"]);
        for f in ["a.txt", "b.txt"] {
            std::fs::write(wt.join(f), "1").unwrap();
        }
        must_git(&wt, &["add", "a.txt", "b.txt"]);
        must_git(&wt, &["commit", "-q", "-m", "ab"]);
        for f in ["a.txt", "b.txt"] {
            std::fs::write(wt.join(f), "2").unwrap();
        }
        // Every line keeps its status columns, the first one too.
        assert_eq!(
            dirty_files(&wt).unwrap(),
            [" M a.txt", " M b.txt", "?? new.txt"]
        );
        assert_eq!(operation(&wt), None);

        repo.remove_worktree(&wt).unwrap();
        repo.delete_branch("feat/feature").unwrap();
        assert!(!repo.branch_exists("feat/feature"));
    }

    #[test]
    fn unpushed_lists_local_commits() {
        let (_d, work) = new_test_repo();
        assert!(unpushed(&work).unwrap().is_empty());
        must_git(
            &work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "local",
            ],
        );
        let got = unpushed(&work).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with(" local"), "{got:?}");
        assert_eq!(divergence(&work, "origin/trunk").unwrap(), (0, 1));
    }

    #[test]
    fn exclude_is_idempotent() {
        let (_d, work) = new_test_repo();
        let repo = Repo::open(&work).unwrap();
        for _ in 0..2 {
            repo.exclude(".jw.env").unwrap();
        }
        let data = std::fs::read_to_string(repo.common_dir.join("info/exclude")).unwrap();
        assert_eq!(data.matches(".jw.env").count(), 1, "{data}");
    }

    #[test]
    fn project_names() {
        for (input, want) in [
            ("git@github.com:acme/myapp.git", "myapp"),
            ("https://github.com/acme/myapp", "myapp"),
            ("https://github.com/acme/myapp/", "myapp"),
            ("/tmp/origins/myapp.git", "myapp"),
        ] {
            assert_eq!(project_name(input), want, "{input}");
        }
    }
}
