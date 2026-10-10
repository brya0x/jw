//! A stream's changes as data for the diff viewer (REQ-21): what git says
//! changed since the branch left its base, uncommitted work and untracked
//! files included, parsed into files, hunks and lines, plus the pairing and
//! word-level marks a side-by-side view needs. No drawing here.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

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

/// Everything the stream changed against `origin/<base>`: committed,
/// staged, unstaged and untracked.
pub fn load(dir: &Path, base: &str) -> Result<Vec<File>> {
    let base_ref = format!("origin/{base}");
    let out = git(
        dir,
        &[
            "diff",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--merge-base",
            &base_ref,
        ],
    )?;
    let mut files = parse(&out);
    // git diff leaves out untracked files; they are new files all the same.
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        files.push(untracked_file(dir, path));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
    }
}
