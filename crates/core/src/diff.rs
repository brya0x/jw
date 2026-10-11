//! A stream's changes as data for the diff viewer (REQ-21): what git says
//! changed since the branch left its base, uncommitted work and untracked
//! files included, parsed into files, hunks and lines, plus the pairing and
//! word-level marks a side-by-side view needs. No drawing here.

use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;

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

/// The most a phone is sent of a diff (REQ-13): files, lines a file, and
/// characters a line. A limit hit is said, not hidden.
pub const MAX_FILES: usize = 500;
pub const MAX_LINES: usize = 2000;
pub const MAX_TEXT: usize = 500;

/// A workspace's diff as the phone gets it.
#[derive(Debug, Serialize)]
pub struct Report {
    /// As the TUI titles it: `<name> vs <base>`, or `vs HEAD` with no base.
    pub title: String,
    pub truncated: bool,
    pub files: Vec<FileOut>,
}

#[derive(Debug, Serialize)]
pub struct FileOut {
    pub path: String,
    pub old_path: Option<String>,
    pub status: &'static str,
    pub binary: bool,
    /// Counted before any cut.
    pub added: usize,
    pub deleted: usize,
    pub truncated: bool,
    pub hunks: Vec<HunkOut>,
}

#[derive(Debug, Serialize)]
pub struct HunkOut {
    pub context: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<LineOut>,
}

#[derive(Debug, Serialize)]
pub struct LineOut {
    pub k: &'static str,
    pub text: String,
    pub old: Option<u32>,
    pub new: Option<u32>,
}

/// The diff of `entry`, against the base its stream resolves to, within the
/// phone's limits.
pub fn report(entry: &crate::core::registry::Entry) -> Result<Report> {
    let dir = Path::new(&entry.path);
    if !dir.join(".git").exists() {
        bail!("{} is not a git repository", entry.name);
    }
    let stream = crate::stream::Stream::resolve(entry)?;
    let files = load(dir, &stream.base)?;
    let title = if stream.base.is_empty() {
        format!("{} vs HEAD", entry.name)
    } else {
        format!("{} vs {}", entry.name, stream.base)
    };
    Ok(limited(title, files))
}

/// `files` cut to the limits.
pub fn limited(title: String, files: Vec<File>) -> Report {
    let mut truncated = files.len() > MAX_FILES;
    let files = files
        .into_iter()
        .take(MAX_FILES)
        .map(|f| {
            let (added, deleted) = (f.added(), f.deleted());
            let mut left = MAX_LINES;
            let mut cut = false;
            let mut hunks = Vec::new();
            for h in &f.hunks {
                if left == 0 {
                    cut = true;
                    break;
                }
                let lines: Vec<LineOut> = h
                    .lines
                    .iter()
                    .take(left)
                    .map(|l| LineOut {
                        k: match l.kind {
                            Kind::Ctx => "ctx",
                            Kind::Add => "add",
                            Kind::Del => "del",
                        },
                        text: clip(&l.text),
                        old: l.old_no,
                        new: l.new_no,
                    })
                    .collect();
                cut |= lines.len() < h.lines.len();
                left -= lines.len();
                hunks.push(HunkOut {
                    context: clip(&h.context),
                    old_start: h.old_start,
                    new_start: h.new_start,
                    lines,
                });
            }
            truncated |= cut;
            FileOut {
                path: f.path,
                old_path: f.old_path,
                status: match f.status {
                    Status::Added => "added",
                    Status::Deleted => "deleted",
                    Status::Modified => "modified",
                    Status::Renamed => "renamed",
                },
                binary: f.binary,
                added,
                deleted,
                truncated: cut,
                hunks,
            }
        })
        .collect();
    Report {
        title,
        truncated,
        files,
    }
}

/// A line of at most `MAX_TEXT` characters, cut with `…`.
fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_TEXT {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_TEXT - 1).collect();
    out.push('…');
    out
}

/// git's empty tree, the base of a repository with no commit yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Everything the stream changed against `origin/<base>`: committed,
/// staged, unstaged and untracked. With no `base` (a repository whose
/// origin has no default branch, or no origin), against `HEAD`: what is
/// not committed yet (REQ-123).
pub fn load(dir: &Path, base: &str) -> Result<Vec<File>> {
    let base_ref = format!("origin/{base}");
    let mut args = vec!["diff", "-M", "--no-color", "--no-ext-diff"];
    if base.is_empty() {
        // Before the first commit there is no HEAD: everything staged is new.
        let born = git::output(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
        args.push(if born { "HEAD" } else { EMPTY_TREE });
    } else {
        args.extend(["--merge-base", &base_ref]);
    }
    let out = git::output(dir, &args)?;
    let mut files = parse(&out);
    // git diff leaves out untracked files; they are new files all the same.
    let untracked = git::output(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        files.push(untracked_file(dir, path));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
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
    fn file(path: &str, lines: usize, text: &str) -> File {
        File {
            path: path.into(),
            old_path: None,
            status: Status::Modified,
            binary: false,
            hunks: vec![Hunk {
                context: "fn main".into(),
                old_start: 1,
                new_start: 1,
                lines: (0..lines)
                    .map(|i| Line {
                        kind: if i % 2 == 0 { Kind::Add } else { Kind::Ctx },
                        text: text.into(),
                        old_no: (i % 2 == 1).then_some(i as u32),
                        new_no: Some(i as u32 + 1),
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn a_report_says_what_it_cut_and_counts_what_it_did_not_send() {
        let r = limited(
            "w vs main".into(),
            vec![file("big.rs", 2500, "x"), file("a.rs", 4, "y")],
        );
        assert!(r.truncated);
        let big = r.files.iter().find(|f| f.path == "big.rs").unwrap();
        assert!(big.truncated);
        assert_eq!(big.hunks[0].lines.len(), MAX_LINES);
        // Counted before the cut.
        assert_eq!(big.added, 1250);
        let small = r.files.iter().find(|f| f.path == "a.rs").unwrap();
        assert!(!small.truncated && small.hunks[0].lines.len() == 4);
        // Files too: more than the limit, and the answer says so.
        let many: Vec<File> = (0..MAX_FILES + 5)
            .map(|i| file(&format!("f{i}"), 1, "z"))
            .collect();
        let r = limited("t".into(), many);
        assert!(r.truncated && r.files.len() == MAX_FILES);
        // No cut, no flag.
        let r = limited("t".into(), vec![file("a", 3, "x")]);
        assert!(!r.truncated && !r.files[0].truncated);
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis_and_the_json_has_the_fixed_keys() {
        let long = "é".repeat(900);
        let r = limited("t".into(), vec![file("a", 2, &long)]);
        let text = &r.files[0].hunks[0].lines[0].text;
        assert_eq!(text.chars().count(), MAX_TEXT);
        assert!(text.ends_with('…'));
        let v = serde_json::to_value(&r).unwrap();
        let f = &v["files"][0];
        assert_eq!(f["status"], "modified");
        assert_eq!(f["old_path"], serde_json::Value::Null);
        assert_eq!(f["hunks"][0]["context"], "fn main");
        let l = &f["hunks"][0]["lines"][0];
        assert_eq!(
            (l["k"].as_str(), l["old"].is_null(), l["new"].as_u64()),
            (Some("add"), true, Some(1))
        );
        assert_eq!(f["hunks"][0]["lines"][1]["k"], "ctx");
    }

    #[test]
    fn a_binary_file_has_no_hunks_and_the_statuses_have_their_words() {
        let mut b = file("img.png", 0, "");
        b.binary = true;
        b.hunks.clear();
        b.status = Status::Renamed;
        b.old_path = Some("old.png".into());
        let r = limited("t".into(), vec![b]);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["files"][0]["status"], "renamed");
        assert_eq!(v["files"][0]["old_path"], "old.png");
        assert_eq!(v["files"][0]["binary"], true);
        assert_eq!(v["files"][0]["hunks"], serde_json::json!([]));
    }

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

    #[test]
    fn load_sees_commits_edits_and_untracked() {
        use crate::connectors::git::tests::{must_git, new_test_repo};
        let (_d, work) = new_test_repo();
        std::fs::write(work.join("a.txt"), "one\ntwo\n").unwrap();
        must_git(&work, &["add", "a.txt"]);
        must_git(
            &work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "a",
            ],
        );
        std::fs::write(work.join("a.txt"), "one\n2\n").unwrap();
        std::fs::write(work.join("new.md"), "# hi\n").unwrap();

        let files = load(&work, "trunk").unwrap();
        let names: Vec<(&str, Status)> =
            files.iter().map(|f| (f.path.as_str(), f.status)).collect();
        assert_eq!(names, [("a.txt", Status::Added), ("new.md", Status::Added)]);
        assert_eq!(
            files[0].added(),
            2,
            "committed and edited, against the base"
        );
        assert_eq!(files[1].hunks[0].lines[0].text, "# hi");

        // REQ-123: with no base, only what isn't committed.
        let files = load(&work, "").unwrap();
        let names: Vec<(&str, Status)> =
            files.iter().map(|f| (f.path.as_str(), f.status)).collect();
        assert_eq!(
            names,
            [("a.txt", Status::Modified), ("new.md", Status::Added)]
        );
        assert_eq!(files[0].added(), 1);
    }

    /// REQ-123: a repository with no commit yet shows what is staged and
    /// what is untracked, instead of git's "unknown revision HEAD".
    #[test]
    fn load_works_before_the_first_commit() {
        use crate::connectors::git::tests::must_git;
        let d = tempfile::tempdir().unwrap();
        must_git(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
        must_git(d.path(), &["add", "a.txt"]);
        std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
        let files = load(d.path(), "").unwrap();
        let names: Vec<(&str, Status)> =
            files.iter().map(|f| (f.path.as_str(), f.status)).collect();
        assert_eq!(names, [("a.txt", Status::Added), ("b.txt", Status::Added)]);
    }
}
