//! A stream's changes as data for the diff viewer (REQ-21, addendum 17): what
//! git says changed in one scope (the branch's commits, what is uncommitted,
//! or one commit), parsed into files, hunks and lines, plus the pairing and
//! word-level marks a side-by-side view needs. No drawing here.

use std::path::Path;

use anyhow::Result;

use crate::connectors::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    /// The path after the change (before, for a deleted file).
    pub path: String,
    /// The path before a rename.
    pub old_path: Option<String>,
    pub status: Status,
    pub binary: bool,
    /// Not in git yet: an `?` in the Uncommitted view, not an `A`.
    pub untracked: bool,
    pub hunks: Vec<Hunk>,
}

impl File {
    pub fn added(&self) -> usize {
        self.count(Kind::Add)
    }

    pub fn deleted(&self) -> usize {
        self.count(Kind::Del)
    }

    fn count(&self, k: Kind) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == k)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// What follows the second `@@`: usually the enclosing function.
    pub context: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// git's empty tree, the base of a repository with no commit yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Most bytes of diff text read from git for one view; past it the view lists
/// its files and says it is too large (REQ-220).
pub const DIFF_CAP: usize = 2 * 1024 * 1024;

/// Most files a too-large view lists.
pub const FILE_LIST_CAP: usize = 200;

/// Which changes the diff viewer shows (addendum 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Committed work: `origin/<base>` to `HEAD`. An empty base has nothing
    /// to compare against.
    Branch { base: String },
    /// Staged, unstaged and untracked, against `HEAD`.
    Uncommitted,
    /// One commit against its first parent.
    Commit(String),
}

/// What a scope loaded to.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Files(Vec<File>),
    /// The diff text passed [`DIFF_CAP`]: only the first [`FILE_LIST_CAP`]
    /// files, and how many there are.
    TooLarge {
        files: Vec<(Status, String)>,
        total: usize,
    },
}

/// A commit of the branch, for the Commits view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    /// Committer time, seconds since the epoch.
    pub when: i64,
}

/// The commits of a branch, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commits {
    pub list: Vec<Commit>,
    /// Older commits exist beyond the limit.
    pub more: bool,
}

/// The commits `origin/<base>..HEAD`, newest first, at most `limit`. With no
/// `base`, those of `HEAD`. A repository with no commit has none.
pub fn commits(dir: &Path, base: &str, limit: usize) -> Result<Commits> {
    if git::output(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(Commits {
            list: Vec::new(),
            more: false,
        });
    }
    let range = if base.is_empty() {
        "HEAD".to_string()
    } else {
        format!("origin/{base}..HEAD")
    };
    let n = format!("-n{}", limit + 1);
    let out = git::output(dir, &["log", &n, "--format=%H%x1f%h%x1f%ct%x1f%s", &range])?;
    let mut list: Vec<Commit> = out
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\x1f');
            Some(Commit {
                sha: f.next()?.to_string(),
                short: f.next()?.to_string(),
                when: f.next()?.parse().ok()?,
                subject: f.next()?.to_string(),
            })
        })
        .collect();
    let more = list.len() > limit;
    list.truncate(limit);
    Ok(Commits { list, more })
}

/// The changes of one scope, parsed. Reads at most [`DIFF_CAP`] bytes of diff
/// text, and untracked files up to the same total; past that, the file list
/// only.
pub fn load(dir: &Path, scope: &Scope) -> Result<Loaded> {
    if matches!(scope, Scope::Branch { base } if base.is_empty()) {
        return Ok(Loaded::Files(Vec::new()));
    }
    let (sub, rest) = command(dir, scope)?;
    let mut args = vec![sub, "-M", "--no-color", "--no-ext-diff"];
    args.extend(rest.iter().map(String::as_str));
    let Some(out) = git::output_capped(dir, &args, DIFF_CAP)? else {
        let mut listing = vec![sub, "-M", "--name-status", "-z"];
        listing.extend(rest.iter().map(String::as_str));
        let mut all = name_status(&git::output(dir, &listing)?);
        if *scope == Scope::Uncommitted {
            all.extend(untracked(dir)?.into_iter().map(|p| (Status::Added, p)));
        }
        return Ok(too_large(all));
    };
    let mut files = parse(&out);
    if *scope == Scope::Uncommitted {
        // git diff leaves out untracked files; they are new files all the same.
        let mut budget = DIFF_CAP.saturating_sub(out.len());
        let paths = untracked(dir)?;
        for path in &paths {
            let size = std::fs::metadata(dir.join(path)).map_or(0, |m| m.len() as usize);
            if size > budget {
                let mut all: Vec<(Status, String)> =
                    files.iter().map(|f| (f.status, f.path.clone())).collect();
                all.extend(paths.into_iter().map(|p| (Status::Added, p)));
                return Ok(too_large(all));
            }
            budget -= size;
        }
        files.extend(paths.iter().map(|p| untracked_file(dir, p)));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Loaded::Files(files))
}

/// The git subcommand and the arguments that pick a scope's revisions.
fn command(dir: &Path, scope: &Scope) -> Result<(&'static str, Vec<String>)> {
    Ok(match scope {
        Scope::Branch { base } => (
            "diff",
            vec![
                "--merge-base".into(),
                format!("origin/{base}"),
                "HEAD".into(),
            ],
        ),
        Scope::Uncommitted => {
            // Before the first commit there is no HEAD: everything staged is new.
            let born = git::output(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
            ("diff", vec![if born { "HEAD" } else { EMPTY_TREE }.into()])
        }
        Scope::Commit(sha) => {
            anyhow::ensure!(
                !sha.is_empty() && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "not a commit id: {sha}"
            );
            // `-m --first-parent` shows a merge against its first parent, which
            // `parse` can read; the combined diff it cannot.
            (
                "show",
                vec![
                    "--format=".into(),
                    "-m".into(),
                    "--first-parent".into(),
                    sha.clone(),
                ],
            )
        }
    })
}

fn untracked(dir: &Path) -> Result<Vec<String>> {
    let out = git::output(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

/// Reads `git diff --name-status -z`: a status, then a path (two for a rename).
fn name_status(text: &str) -> Vec<(Status, String)> {
    let mut out = Vec::new();
    let mut it = text.split('\0').filter(|t| !t.trim().is_empty());
    while let Some(code) = it.next() {
        let status = match code.as_bytes()[0] {
            b'A' => Status::Added,
            b'D' => Status::Deleted,
            b'R' | b'C' => Status::Renamed,
            _ => Status::Modified,
        };
        let renamed = matches!(code.as_bytes()[0], b'R' | b'C');
        let first = it.next();
        let path = if renamed { it.next() } else { first };
        if let Some(p) = path {
            out.push((status, p.to_string()));
        }
    }
    out
}

fn too_large(mut all: Vec<(Status, String)>) -> Loaded {
    all.sort_by(|a, b| a.1.cmp(&b.1));
    let total = all.len();
    all.truncate(FILE_LIST_CAP);
    Loaded::TooLarge { files: all, total }
}

fn untracked_file(dir: &Path, path: &str) -> File {
    let data = std::fs::read(dir.join(path)).unwrap_or_default();
    let binary = data.contains(&0);
    let hunks = if binary || data.is_empty() {
        Vec::new()
    } else {
        let text = String::from_utf8_lossy(&data);
        let lines: Vec<Line> = text
            .lines()
            .enumerate()
            .map(|(i, l)| Line {
                kind: Kind::Add,
                text: l.to_string(),
                old_no: None,
                new_no: Some(i as u32 + 1),
            })
            .collect();
        vec![Hunk {
            context: String::new(),
            old_start: 0,
            new_start: 1,
            lines,
        }]
    };
    File {
        path: path.to_string(),
        old_path: None,
        status: Status::Added,
        binary,
        untracked: true,
        hunks,
    }
}

/// Parses `git diff` output (unified, with git's extended headers).
pub fn parse(text: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            let path = rest.split_once(" b/").map_or(rest, |(_, b)| b).to_string();
            files.push(File {
                path,
                old_path: None,
                status: Status::Modified,
                binary: false,
                untracked: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if file.hunks.is_empty() {
            // Extended headers, before the first hunk.
            if raw.starts_with("new file mode") {
                file.status = Status::Added;
            } else if raw.starts_with("deleted file mode") {
                file.status = Status::Deleted;
            } else if let Some(from) = raw.strip_prefix("rename from ") {
                file.status = Status::Renamed;
                file.old_path = Some(from.to_string());
            } else if let Some(to) = raw.strip_prefix("rename to ") {
                file.path = to.to_string();
            } else if raw.starts_with("Binary files ") {
                file.binary = true;
            } else if let Some(p) = raw.strip_prefix("--- a/")
                && file.status == Status::Deleted
            {
                file.path = p.to_string();
            }
        }
        if let Some(h) = raw.strip_prefix("@@ ") {
            let (ranges, context) = h.split_once(" @@").unwrap_or((h, ""));
            let mut it = ranges.split_whitespace();
            let start = |s: Option<&str>| -> u32 {
                s.map(|s| s.trim_start_matches(['-', '+']))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            let (o, n) = (start(it.next()), start(it.next()));
            (old_no, new_no) = (o, n);
            file.hunks.push(Hunk {
                context: context.trim().to_string(),
                old_start: o,
                new_start: n,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };
        let (kind, body) = match raw.as_bytes().first() {
            Some(b'+') => (Kind::Add, &raw[1..]),
            Some(b'-') => (Kind::Del, &raw[1..]),
            Some(b' ') => (Kind::Ctx, &raw[1..]),
            // "\ No newline at end of file", or the empty line of an empty context.
            Some(b'\\') => continue,
            None => (Kind::Ctx, ""),
            _ => continue,
        };
        let line = Line {
            kind,
            text: body.to_string(),
            old_no: (kind != Kind::Add).then_some(old_no),
            new_no: (kind != Kind::Del).then_some(new_no),
        };
        if kind != Kind::Add {
            old_no += 1;
        }
        if kind != Kind::Del {
            new_no += 1;
        }
        hunk.lines.push(line);
    }
    files
}

/// One row of the side-by-side view: the old line on the left, the new on
/// the right. A run of deletions followed by additions is paired up line by
/// line, as GitHub does, so a changed line sits next to what it became.
#[derive(Debug, Clone, PartialEq)]
pub struct Row<'a> {
    pub left: Option<&'a Line>,
    pub right: Option<&'a Line>,
}

pub fn split_rows(h: &Hunk) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let lines = &h.lines;
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            Kind::Ctx => {
                rows.push(Row {
                    left: Some(&lines[i]),
                    right: Some(&lines[i]),
                });
                i += 1;
            }
            _ => {
                let dels: Vec<&Line> = lines[i..]
                    .iter()
                    .take_while(|l| l.kind == Kind::Del)
                    .collect();
                i += dels.len();
                let adds: Vec<&Line> = lines[i..]
                    .iter()
                    .take_while(|l| l.kind == Kind::Add)
                    .collect();
                i += adds.len();
                for k in 0..dels.len().max(adds.len()) {
                    rows.push(Row {
                        left: dels.get(k).copied(),
                        right: adds.get(k).copied(),
                    });
                }
            }
        }
    }
    rows
}

/// A byte range `start..end` of a line.
pub type Span = (usize, usize);

/// Byte ranges of `a` and of `b` that differ, by words: what a reviewer's
/// eye should land on inside a changed line. Lines too long to compare
/// cheaply count as changed throughout.
pub fn word_changes(a: &str, b: &str) -> (Vec<Span>, Vec<Span>) {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.len() * tb.len() > 40_000 {
        return (vec![(0, a.len())], vec![(0, b.len())]);
    }
    // Longest common subsequence over tokens.
    let (n, m) = (ta.len(), tb.len());
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if ta[i].1 == tb[j].1 {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    while i < n || j < m {
        if i < n && j < m && ta[i].1 == tb[j].1 {
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]) {
            push(&mut rb, tb[j].0, tb[j].0 + tb[j].1.len());
            j += 1;
        } else {
            push(&mut ra, ta[i].0, ta[i].0 + ta[i].1.len());
            i += 1;
        }
    }
    (ra, rb)
}

/// Adds a range, merging it with the previous one when they touch.
fn push(v: &mut Vec<Span>, start: usize, end: usize) {
    match v.last_mut() {
        Some(last) if last.1 == start => last.1 = end,
        _ => v.push((start, end)),
    }
}

/// Words, runs of spaces and single punctuation marks, with their offsets.
fn tokens(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((start, c)) = it.next() {
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                0
            } else if c.is_whitespace() {
                1
            } else {
                2
            }
        };
        let k = class(c);
        let mut end = start + c.len_utf8();
        if k != 2 {
            while let Some(&(i, d)) = it.peek() {
                if class(d) != k {
                    break;
                }
                end = i + d.len_utf8();
                it.next();
            }
        }
        out.push((start, &s[start..end]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/src/a.ts b/src/a.ts
index 1111111..2222222 100644
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,4 +1,5 @@ export function track()
 import x from 'y'
-const limit = 10
+const limit = 20
+const extra = true

 done()
diff --git a/old.md b/new.md
similarity index 90%
rename from old.md
rename to new.md
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/img.png b/img.png
new file mode 100644
Binary files /dev/null and b/img.png differ
";

    #[test]
    fn parses_files_hunks_and_numbers() {
        let files = parse(SAMPLE);
        assert_eq!(files.len(), 4);
        let a = &files[0];
        assert_eq!((a.path.as_str(), a.status), ("src/a.ts", Status::Modified));
        assert_eq!((a.added(), a.deleted()), (2, 1));
        let h = &a.hunks[0];
        assert_eq!(h.context, "export function track()");
        assert_eq!(h.lines.len(), 6);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[3].new_no, Some(3));
        assert_eq!(h.lines[5].old_no, Some(4));
        assert_eq!(h.lines[5].new_no, Some(5));

        assert_eq!(files[1].status, Status::Renamed);
        assert_eq!(files[1].old_path.as_deref(), Some("old.md"));
        assert_eq!(files[1].path, "new.md");
        assert_eq!((files[2].status, files[2].deleted()), (Status::Deleted, 1));
        assert!(files[3].binary && files[3].status == Status::Added);
    }

    #[test]
    fn split_rows_pair_changes() {
        let files = parse(SAMPLE);
        let rows = split_rows(&files[0].hunks[0]);
        let shape: Vec<(Option<&str>, Option<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.left.map(|l| l.text.as_str()),
                    r.right.map(|l| l.text.as_str()),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("import x from 'y'"), Some("import x from 'y'")),
                (Some("const limit = 10"), Some("const limit = 20")),
                (None, Some("const extra = true")),
                (Some(""), Some("")),
                (Some("done()"), Some("done()")),
            ]
        );
    }

    #[test]
    fn word_changes_mark_only_what_changed() {
        let a = "const limit = 10";
        let b = "const limit = 20;";
        let (ra, rb) = word_changes(a, b);
        assert_eq!(
            ra.iter().map(|&(s, e)| &a[s..e]).collect::<Vec<_>>(),
            ["10"]
        );
        assert_eq!(
            rb.iter().map(|&(s, e)| &b[s..e]).collect::<Vec<_>>(),
            ["20;"]
        );
        let (ra, rb) = word_changes("same", "same");
        assert!(ra.is_empty() && rb.is_empty());
    }

    use crate::connectors::git::tests::{must_git, new_test_repo};

    fn commit(work: &Path, msg: &str) {
        must_git(
            work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
    }

    fn files(l: Loaded) -> Vec<File> {
        match l {
            Loaded::Files(f) => f,
            other => panic!("expected files, got {other:?}"),
        }
    }

    fn names(files: &[File]) -> Vec<(&str, Status)> {
        files.iter().map(|f| (f.path.as_str(), f.status)).collect()
    }

    fn branch() -> Scope {
        Scope::Branch {
            base: "trunk".into(),
        }
    }

    /// A feature branch with two commits (a.txt, then b.txt) and, on top,
    /// an edit to a.txt, a staged d.txt and an untracked c.txt.
    fn feature_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let (d, work) = new_test_repo();
        must_git(&work, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(work.join("a.txt"), "one\ntwo\n").unwrap();
        must_git(&work, &["add", "a.txt"]);
        commit(&work, "a");
        std::fs::write(work.join("b.txt"), "b\n").unwrap();
        must_git(&work, &["add", "b.txt"]);
        commit(&work, "b");
        std::fs::write(work.join("a.txt"), "one\n2\n").unwrap();
        std::fs::write(work.join("d.txt"), "d\n").unwrap();
        must_git(&work, &["add", "d.txt"]);
        std::fs::write(work.join("c.txt"), "c\n").unwrap();
        (d, work)
    }

    /// REQ-205: General is what was committed, not what is edited since.
    #[test]
    fn general_is_committed_work_only() {
        let (_d, work) = feature_repo();
        let f = files(load(&work, &branch()).unwrap());
        assert_eq!(
            names(&f),
            [("a.txt", Status::Added), ("b.txt", Status::Added)]
        );
        assert_eq!(f[0].added(), 2, "the committed a.txt, not the edit");
    }

    /// REQ-206: staged, unstaged and untracked, against HEAD.
    #[test]
    fn uncommitted_is_staged_unstaged_and_untracked() {
        let (_d, work) = feature_repo();
        let f = files(load(&work, &Scope::Uncommitted).unwrap());
        assert_eq!(
            names(&f),
            [
                ("a.txt", Status::Modified),
                ("c.txt", Status::Added),
                ("d.txt", Status::Added),
            ]
        );
        assert_eq!(f[1].hunks[0].lines[0].text, "c");
        assert_eq!(
            f.iter().map(|f| f.untracked).collect::<Vec<_>>(),
            [false, true, false],
            "only c.txt is untracked; the staged d.txt is a plain addition"
        );
    }

    /// REQ-206: a repository with no commit yet shows what is staged and
    /// what is untracked, instead of git's "unknown revision HEAD".
    #[test]
    fn uncommitted_works_before_the_first_commit() {
        let d = tempfile::tempdir().unwrap();
        must_git(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
        must_git(d.path(), &["add", "a.txt"]);
        std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
        let f = files(load(d.path(), &Scope::Uncommitted).unwrap());
        assert_eq!(
            names(&f),
            [("a.txt", Status::Added), ("b.txt", Status::Added)]
        );
        assert!(commits(d.path(), "", 10).unwrap().list.is_empty());
    }

    /// REQ-208: no base, nothing to compare General with.
    #[test]
    fn general_with_no_base_lists_nothing() {
        let (_d, work) = feature_repo();
        let none = Scope::Branch {
            base: String::new(),
        };
        assert_eq!(load(&work, &none).unwrap(), Loaded::Files(Vec::new()));
    }

    /// REQ-207, REQ-208: newest first, the limit, and whether more exist.
    #[test]
    fn commits_are_newest_first_and_capped() {
        let (_d, work) = feature_repo();
        let c = commits(&work, "trunk", 10).unwrap();
        let subjects: Vec<&str> = c.list.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["b", "a"]);
        assert!(!c.more);
        assert_eq!(c.list[0].sha.len(), 40);
        assert!(c.list[0].when > 0 && c.list[0].sha.starts_with(&c.list[0].short));

        let c = commits(&work, "trunk", 1).unwrap();
        assert_eq!((c.list.len(), c.more), (1, true));
        assert_eq!(c.list[0].subject, "b");

        // No base: every commit of HEAD, the initial one too.
        let c = commits(&work, "", 10).unwrap();
        assert_eq!(c.list.len(), 3);
    }

    /// One commit against its parent, a root commit, and a merge against its
    /// first parent.
    #[test]
    fn a_commit_shows_only_its_own_changes() {
        let (_d, work) = feature_repo();
        let sha = |n: usize| commits(&work, "trunk", 10).unwrap().list[n].sha.clone();
        let f = files(load(&work, &Scope::Commit(sha(0))).unwrap());
        assert_eq!(names(&f), [("b.txt", Status::Added)]);

        // The repository's first commit has files and no parent.
        let root = tempfile::tempdir().unwrap();
        must_git(root.path(), &["init", "-q", "-b", "trunk"]);
        std::fs::write(root.path().join("r.txt"), "r\n").unwrap();
        must_git(root.path(), &["add", "r.txt"]);
        commit(root.path(), "root");
        let first = commits(root.path(), "", 10).unwrap().list[0].sha.clone();
        let f = files(load(root.path(), &Scope::Commit(first)).unwrap());
        assert_eq!(names(&f), [("r.txt", Status::Added)]);

        // A merge: what the side branch brought in.
        must_git(root.path(), &["checkout", "-q", "-b", "side"]);
        std::fs::write(root.path().join("s.txt"), "s\n").unwrap();
        must_git(root.path(), &["add", "s.txt"]);
        commit(root.path(), "side");
        must_git(root.path(), &["checkout", "-q", "trunk"]);
        std::fs::write(root.path().join("t.txt"), "t\n").unwrap();
        must_git(root.path(), &["add", "t.txt"]);
        commit(root.path(), "trunk");
        must_git(
            root.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "merge",
                "side",
            ],
        );
        let merge = commits(root.path(), "", 10).unwrap().list[0].sha.clone();
        let f = files(load(root.path(), &Scope::Commit(merge)).unwrap());
        assert_eq!(names(&f), [("s.txt", Status::Added)]);
    }

    /// REQ-220: past the cap, the file list and the total, no parsed diff.
    #[test]
    fn a_diff_past_the_cap_lists_its_files() {
        let (_d, work) = new_test_repo();
        must_git(&work, &["checkout", "-q", "-b", "feat"]);
        let big = "x".repeat(99) + "\n";
        std::fs::write(work.join("big.txt"), big.repeat(DIFF_CAP / 100 + 100)).unwrap();
        std::fs::write(work.join("small.txt"), "s\n").unwrap();
        must_git(&work, &["add", "."]);
        commit(&work, "big");
        let l = load(&work, &branch()).unwrap();
        assert_eq!(
            l,
            Loaded::TooLarge {
                files: vec![
                    (Status::Added, "big.txt".into()),
                    (Status::Added, "small.txt".into())
                ],
                total: 2
            }
        );
        // An untracked file that large is not read either.
        std::fs::write(work.join("huge.log"), big.repeat(DIFF_CAP / 100 + 100)).unwrap();
        let l = load(&work, &Scope::Uncommitted).unwrap();
        assert_eq!(
            l,
            Loaded::TooLarge {
                files: vec![(Status::Added, "huge.log".into())],
                total: 1
            }
        );
    }

    /// REQ-228: git's complaint comes back as an error, and an id that is not
    /// hex never reaches git as an option.
    #[test]
    fn errors_name_what_went_wrong() {
        let (_d, work) = feature_repo();
        let missing = Scope::Branch {
            base: "nope".into(),
        };
        let e = load(&work, &missing).unwrap_err().to_string();
        assert!(e.contains("origin/nope"), "{e}");
        let e = load(&work, &Scope::Commit("--output=x".into()))
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a commit id"), "{e}");
    }

    #[test]
    fn name_status_reads_renames_and_blank_leaders() {
        let t = "\nM\0a.txt\0R100\0old.md\0new.md\0D\0gone\0A\0n\0";
        assert_eq!(
            name_status(t),
            [
                (Status::Modified, "a.txt".to_string()),
                (Status::Renamed, "new.md".to_string()),
                (Status::Deleted, "gone".to_string()),
                (Status::Added, "n".to_string()),
            ]
        );
    }
}
