//! A stream's changes as data for the diff viewer (REQ-21): what git says
//! changed since the branch left its base, uncommitted work and untracked
//! files included, parsed into files, hunks and lines, plus the pairing and
//! word-level marks a side-by-side view needs. No drawing here.

use std::ffi::CString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

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
    /// Why its content is not here, for a file that could not be read: a
    /// symlink (never followed), something that is not a regular file, one
    /// over the size limit, one the user may not read, a name that is not
    /// UTF-8. Not an empty file: the diff says it could not look.
    pub unreadable: Option<String>,
    /// Not tracked by git: an added file of the Uncommitted view (mark `?`).
    pub untracked: bool,
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
/// The most JSON a reply may carry, well under a frame (16 MiB), so the phone
/// always gets an answer it can take: what does not fit is left out and
/// `truncated` says so.
pub const MAX_REPLY: usize = 8 << 20;
/// Room kept for what is not a file's: the title, `t`, `ws`, `truncated`.
const ENVELOPE: usize = 4096;
/// What a hunk costs besides its lines and its context.
const HUNK_KEYS: usize = 96;
/// A path longer than this is cut, like a line.
const MAX_PATH: usize = 1024;

/// A workspace's diff as the phone gets it.
#[derive(Debug, Serialize)]
pub struct Report {
    /// `<name> vs <base>` for General, `<name> vs HEAD` for Uncommitted, `<name> @ <sha>` for a commit.
    pub title: String,
    pub truncated: bool,
    /// The diff text passed `DIFF_CAP`: `files` only lists names (no hunks, no counts) and this is how many there are.
    pub too_large: Option<usize>,
    /// General with no default branch on origin: there is nothing to compare against (REQ-226).
    pub no_base: bool,
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
    /// Why its content is not here (see `File::unreadable`), else `null`.
    pub unreadable: Option<String>,
    /// Not tracked by git (mark `?` in Uncommitted).
    pub untracked: bool,
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

/// Which view of a workspace's changes a phone asks for (REQ-13): the same three the TUI's diff pane has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    General,
    Uncommitted,
    Commit(String),
}

/// The changes of `entry` in one view, within the phone's limits. The base is the one its stream resolves to
/// (`review_base` included), as the TUI's pane takes it.
pub fn report(
    entry: &crate::core::registry::Entry,
    pick: &Pick,
    cancel: &AtomicBool,
) -> Result<Report> {
    let dir = repo_dir(entry)?;
    let base = crate::stream::Stream::resolve(entry)?.base;
    let (scope, title) = match pick {
        Pick::General if base.is_empty() => (
            Scope::Branch {
                base: String::new(),
            },
            format!("{} vs HEAD", entry.name),
        ),
        Pick::General => (
            Scope::Branch { base: base.clone() },
            format!("{} vs {base}", entry.name),
        ),
        Pick::Uncommitted => (Scope::Uncommitted, format!("{} vs HEAD", entry.name)),
        Pick::Commit(sha) => (
            Scope::Commit(sha.clone()),
            format!(
                "{} @ {}",
                entry.name,
                sha.chars().take(7).collect::<String>()
            ),
        ),
    };
    let got = load_until(dir, &scope, Bounds::PHONE, cancel)?;
    let mut report = match got.loaded {
        Loaded::Files(files) => limited(title, files),
        Loaded::TooLarge { files, total } => Report {
            title,
            truncated: false,
            too_large: Some(total),
            no_base: false,
            files: files
                .into_iter()
                .map(|(status, path)| FileOut {
                    path: clip_to(&path, MAX_PATH),
                    old_path: None,
                    status: status_word(status),
                    binary: false,
                    added: 0,
                    deleted: 0,
                    truncated: false,
                    unreadable: None,
                    untracked: false,
                    hunks: Vec::new(),
                })
                .collect(),
        },
    };
    report.truncated |= got.truncated;
    report.no_base = *pick == Pick::General && base.is_empty();
    Ok(report)
}

fn status_word(s: Status) -> &'static str {
    match s {
        Status::Added => "added",
        Status::Deleted => "deleted",
        Status::Modified => "modified",
        Status::Renamed => "renamed",
    }
}

fn repo_dir(entry: &crate::core::registry::Entry) -> Result<&Path> {
    let dir = Path::new(&entry.path);
    if !dir.join(".git").exists() {
        bail!("{} is not a git repository", entry.name);
    }
    Ok(dir)
}

/// A commit of the branch as the phone is told it.
#[derive(Debug, Serialize)]
pub struct CommitOut {
    pub sha: String,
    pub short: String,
    pub subject: String,
    /// Committer time, seconds since the epoch.
    pub when: i64,
}

/// The answer to `commits{ws}`: the branch's commits, newest first, at most [`COMMITS_LIMIT`].
#[derive(Debug, Serialize)]
pub struct CommitsReport {
    pub commits: Vec<CommitOut>,
    /// Older commits exist beyond the limit.
    pub more: bool,
    /// No default branch on origin: these are the commits of `HEAD`, not of the branch (REQ-226).
    pub no_base: bool,
}

/// The most commits the Commits view lists, as the TUI's.
pub const COMMITS_LIMIT: usize = 200;

/// The commits of `entry`'s branch within the phone's limits.
pub fn commit_report(
    entry: &crate::core::registry::Entry,
    cancel: &AtomicBool,
) -> Result<CommitsReport> {
    let dir = repo_dir(entry)?;
    let base = crate::stream::Stream::resolve(entry)?.base;
    let got = commits_until(dir, &base, COMMITS_LIMIT, Bounds::PHONE, cancel)?;
    Ok(CommitsReport {
        commits: got
            .list
            .into_iter()
            .map(|c| CommitOut {
                sha: c.sha,
                short: c.short,
                subject: clip(&c.subject),
                when: c.when,
            })
            .collect(),
        more: got.more,
        no_base: base.is_empty(),
    })
}

/// `files` cut to the limits: files, lines a file, characters a line, and the
/// reply's own size, counted as the JSON it will be.
pub fn limited(title: String, files: Vec<File>) -> Report {
    let mut truncated = files.len() > MAX_FILES;
    // The envelope (title, keys, what the bridge adds) is not in the files.
    let mut budget = MAX_REPLY - ENVELOPE - 2 * title.len();
    let mut out: Vec<FileOut> = Vec::new();
    for f in files.into_iter().take(MAX_FILES) {
        let (added, deleted) = (f.added(), f.deleted());
        let path = clip_to(&f.path, MAX_PATH);
        // What a file costs before its lines: its path, twice for a rename,
        // and its keys.
        let overhead = 160 + 2 * path.len() + f.old_path.as_ref().map_or(0, |p| p.len());
        if budget < overhead {
            truncated = true;
            break;
        }
        budget -= overhead;
        let mut left = MAX_LINES;
        let mut cut = false;
        let mut hunks = Vec::new();
        'hunks: for h in &f.hunks {
            if left == 0 {
                cut = true;
                break;
            }
            let hunk_cost = HUNK_KEYS + 2 * clip(&h.context).len();
            if hunk_cost > budget {
                cut = true;
                truncated = true;
                break;
            }
            budget -= hunk_cost;
            let mut lines: Vec<LineOut> = Vec::new();
            for l in &h.lines {
                if left == 0 {
                    cut = true;
                    break;
                }
                let line = LineOut {
                    k: match l.kind {
                        Kind::Ctx => "ctx",
                        Kind::Add => "add",
                        Kind::Del => "del",
                    },
                    text: clip(&l.text),
                    old: l.old_no,
                    new: l.new_no,
                };
                // Counted as it will be sent: escapes make text longer.
                let cost = serde_json::to_string(&line).map_or(MAX_TEXT * 6, |j| j.len()) + 1;
                if cost > budget {
                    cut = true;
                    truncated = true;
                    if !lines.is_empty() {
                        hunks.push(HunkOut {
                            context: clip(&h.context),
                            old_start: h.old_start,
                            new_start: h.new_start,
                            lines,
                        });
                    }
                    break 'hunks;
                }
                budget -= cost;
                left -= 1;
                lines.push(line);
            }
            if lines.len() < h.lines.len() {
                cut = true;
            }
            hunks.push(HunkOut {
                context: clip(&h.context),
                old_start: h.old_start,
                new_start: h.new_start,
                lines,
            });
        }
        truncated |= cut;
        out.push(FileOut {
            path,
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
            unreadable: f.unreadable,
            untracked: f.untracked,
            hunks,
        });
        if budget == 0 {
            truncated = true;
            break;
        }
    }
    Report {
        title,
        truncated,
        too_large: None,
        no_base: false,
        files: out,
    }
}

/// A line of at most `MAX_TEXT` characters, cut with `…`.
fn clip(s: &str) -> String {
    clip_to(s, MAX_TEXT)
}

fn clip_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// git's empty tree, the base of a repository with no commit yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// What a diff may cost, so a huge or hostile repository cannot hold a thread
/// or the memory of the daemon: git's output, how long git may run, and the
/// size of an untracked file that is read.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub max_output: usize,
    pub timeout: Duration,
    pub max_file: u64,
}

impl Bounds {
    /// For the TUI's own viewer.
    pub const LOCAL: Bounds = Bounds {
        max_output: 64 << 20,
        timeout: Duration::from_secs(60),
        max_file: 10 << 20,
    };
    /// For a phone, over a tunnel.
    pub const PHONE: Bounds = Bounds {
        max_output: 16 << 20,
        timeout: Duration::from_secs(20),
        max_file: 1 << 20,
    };
}

/// Most bytes of diff text read from git for one view; past it the view lists
/// its files and says it is too large (REQ-238).
pub const DIFF_CAP: usize = 2 * 1024 * 1024;

/// Most files a too-large view lists.
pub const FILE_LIST_CAP: usize = 200;

/// Which changes the diff viewer shows (addendum 19).
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

/// A scope's changes, and whether a bound (the untracked files read, the
/// deadline) cut them short.
#[derive(Debug)]
pub struct Scoped {
    pub loaded: Loaded,
    pub truncated: bool,
}

/// The commits `origin/<base>..HEAD`, newest first, at most `limit`. With no
/// `base`, those of `HEAD`. A repository with no commit has none.
pub fn commits(dir: &Path, base: &str, limit: usize) -> Result<Commits> {
    commits_until(dir, base, limit, Bounds::LOCAL, &AtomicBool::new(false))
}

/// [`commits`] within `bounds`, stopping at once when `cancel` is set.
pub fn commits_until(
    dir: &Path,
    base: &str,
    limit: usize,
    bounds: Bounds,
    cancel: &AtomicBool,
) -> Result<Commits> {
    let run = Run {
        until: Instant::now() + bounds.timeout,
        bounds,
        cancel,
    };
    if !has_head(dir, &run) {
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
    let (out, _) = git_bounded(
        "git",
        dir,
        &[],
        &["log", &n, "--format=%H%x1f%h%x1f%ct%x1f%s", &range],
        &run,
    )?;
    let mut list: Vec<Commit> = String::from_utf8_lossy(&out)
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

fn has_head(dir: &Path, run: &Run) -> bool {
    git_bounded(
        "git",
        dir,
        &[],
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        run,
    )
    .is_ok()
}

/// The changes of one scope, parsed, for the TUI's own viewer.
pub fn load(dir: &Path, scope: &Scope) -> Result<Loaded> {
    Ok(load_with(dir, scope, Bounds::LOCAL)?.loaded)
}

/// [`load`] within `bounds`.
pub fn load_with(dir: &Path, scope: &Scope, bounds: Bounds) -> Result<Scoped> {
    load_until(dir, scope, bounds, &AtomicBool::new(false))
}

/// [`load_with`], stopping at once (and killing git) when `cancel` is set:
/// the phone that asked went away. Reads at most [`DIFF_CAP`] bytes of diff
/// text, and untracked files up to the same total; past that, the file list
/// only.
pub fn load_until(
    dir: &Path,
    scope: &Scope,
    bounds: Bounds,
    cancel: &AtomicBool,
) -> Result<Scoped> {
    // One deadline for the whole load, not for each git call.
    let run = Run {
        until: Instant::now() + bounds.timeout,
        bounds,
        cancel,
    };
    if matches!(scope, Scope::Branch { base } if base.is_empty()) {
        return Ok(Scoped {
            loaded: Loaded::Files(Vec::new()),
            truncated: false,
        });
    }
    let overrides = filter_overrides(dir, &run)?;
    let (sub, rest) = command(dir, scope, &run)?;
    let mut args = vec![sub, "-M", "--no-color", "--no-ext-diff", "--no-textconv"];
    args.extend(rest.iter().map(String::as_str));
    let (out, over) = git_capped(dir, &overrides, &args, DIFF_CAP, &run)?;
    let uncommitted = *scope == Scope::Uncommitted;
    let (untracked, mut truncated) = if uncommitted {
        let (names, cut) = git_bounded(
            "git",
            dir,
            &overrides,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            &run,
        )?;
        // Names are bytes, not text: one that is not UTF-8 is said to be
        // unreadable, not changed into another name and read as an empty file.
        let paths: Vec<Vec<u8>> = names
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        (paths, cut)
    } else {
        (Vec::new(), false)
    };
    // What the untracked files would add (a link counts as its own length: it is never read through).
    let weight: usize = untracked
        .iter()
        .take(MAX_UNTRACKED)
        .map(|raw| {
            std::fs::symlink_metadata(dir.join(std::ffi::OsStr::from_bytes(raw)))
                .map_or(0, |m| m.len() as usize)
        })
        .sum();
    if over || weight > DIFF_CAP.saturating_sub(out.len()) {
        let mut listing = vec![sub, "-M", "--name-status", "-z", "--no-ext-diff"];
        listing.extend(rest.iter().map(String::as_str));
        let (names, cut) = git_bounded("git", dir, &overrides, &listing, &run)?;
        truncated |= cut;
        let mut all = name_status(&String::from_utf8_lossy(&names));
        all.extend(
            untracked
                .iter()
                .map(|p| (Status::Added, String::from_utf8_lossy(p).into_owned())),
        );
        return Ok(Scoped {
            loaded: too_large(all),
            truncated,
        });
    }
    let mut files = parse(&String::from_utf8_lossy(&out));
    // Only so many are read, and none after the deadline: the reads are not
    // git's, so git's own deadline does not cover them.
    for (n, raw) in untracked.iter().enumerate() {
        if cancelled(&run)? || n >= MAX_UNTRACKED || Instant::now() >= run.until {
            truncated = true;
            break;
        }
        files.push(untracked_file(dir, raw, &run.bounds));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Scoped {
        loaded: Loaded::Files(files),
        truncated,
    })
}

/// The git subcommand and the arguments that pick a scope's revisions.
fn command(dir: &Path, scope: &Scope, run: &Run) -> Result<(&'static str, Vec<String>)> {
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
            let born = has_head(dir, run);
            ("diff", vec![if born { "HEAD" } else { EMPTY_TREE }.into()])
        }
        Scope::Commit(sha) => {
            anyhow::ensure!(
                !sha.is_empty() && sha.len() <= 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "not a commit id"
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

/// How many untracked files one load reads; past it the diff says it is cut.
const MAX_UNTRACKED: usize = 2000;

/// What one load is held to: its bounds, its single deadline and whether to
/// stop.
struct Run<'a> {
    bounds: Bounds,
    until: Instant,
    cancel: &'a AtomicBool,
}

fn cancelled(run: &Run) -> Result<bool> {
    if run.cancel.load(Ordering::Relaxed) {
        bail!("the diff was cancelled");
    }
    Ok(false)
}

/// `-c` arguments that switch off every filter driver the configuration
/// defines. A repository's `.gitattributes` can name a driver and its own
/// config can give it a command, which `git diff` would run on every file it
/// compares; textconv and the external diff are closed by flags, this is the
/// other way in.
fn filter_overrides(dir: &Path, run: &Run) -> Result<Vec<String>> {
    let (out, _) = git_run(
        "git",
        dir,
        &[],
        &["config", "-z", "--get-regexp", r"^filter\."],
        run.bounds.max_output,
        run,
    )?
    .ok_or_exit(&[1])?;
    let mut names: Vec<String> = Vec::new();
    for entry in out.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        // `key\nvalue`, or `key` alone for a key without a value.
        let key = entry.split(|b| *b == b'\n').next().unwrap_or_default();
        let Ok(key) = std::str::from_utf8(key) else {
            bail!("a filter in the git configuration has a name that is not UTF-8");
        };
        let Some(rest) = key.strip_prefix("filter.") else {
            continue;
        };
        // The driver's name can hold dots: the setting is the last part.
        let Some((name, _)) = rest.rsplit_once('.') else {
            continue;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    let mut args = Vec::new();
    for n in names {
        for setting in ["clean", "smudge", "process"] {
            args.push("-c".to_string());
            args.push(format!("filter.{n}.{setting}="));
        }
        args.push("-c".to_string());
        args.push(format!("filter.{n}.required=false"));
    }
    Ok(args)
}

/// What a finished git run left: its output, and whether it exited 0.
struct Ran {
    out: Vec<u8>,
    cut: bool,
    ok: bool,
    err: String,
    status: String,
}

impl Ran {
    /// The output if it exited 0 (or one of `fine`, which is not an error:
    /// `git config` exits 1 for no match).
    fn ok_or_exit(self, fine: &[i32]) -> Result<(Vec<u8>, bool)> {
        let code = self
            .status
            .strip_prefix("exit status: ")
            .and_then(|c| c.parse().ok());
        if self.ok || code.is_some_and(|c| fine.contains(&c)) {
            return Ok((self.out, self.cut));
        }
        bail!(
            "git: {}",
            if self.err.is_empty() {
                self.status
            } else {
                self.err
            }
        )
    }
}

/// Runs git (or another `program`, in tests) in `dir` until the load's
/// deadline, reading at most `bounds.max_output` bytes of stdout; the child
/// and everything it started (its process group) is killed and reaped when a
/// bound is hit or the load is cancelled, so nothing outlives the call. `true`
/// says the output was cut. `config` are `-c` pairs for git.
///
/// git runs with a clean environment and without what a repository can set to
/// run a command of its own on a read: `core.fsmonitor`, and (in
/// `load_until`'s arguments) textconv. It never asks a question.
fn git_bounded(
    program: &str,
    dir: &Path,
    config: &[String],
    args: &[&str],
    run: &Run,
) -> Result<(Vec<u8>, bool)> {
    let ran = git_run(program, dir, config, args, run.bounds.max_output, run)?;
    if !ran.ok {
        bail!(
            "{program} {}: {}",
            args.join(" "),
            if ran.err.is_empty() {
                ran.status
            } else {
                ran.err
            }
        );
    }
    Ok((ran.out, ran.cut))
}

/// [`git_bounded`] reading at most `cap` bytes: `true` says git had more, which is not an error here.
fn git_capped(
    dir: &Path,
    config: &[String],
    args: &[&str],
    cap: usize,
    run: &Run,
) -> Result<(Vec<u8>, bool)> {
    let ran = git_run("git", dir, config, args, cap, run)?;
    if !ran.ok {
        bail!(
            "git {}: {}",
            args.join(" "),
            if ran.err.is_empty() {
                ran.status
            } else {
                ran.err
            }
        );
    }
    Ok((ran.out, ran.cut))
}

fn kill_group(child: &mut std::process::Child) {
    // SAFETY: a signal to the group this child leads; the group is ours.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn git_run(
    program: &str,
    dir: &Path,
    config: &[String],
    args: &[&str],
    cap: usize,
    run: &Run,
) -> Result<Ran> {
    let mut cmd = Command::new(program);
    if program == "git" {
        cmd.args(["-c", "core.fsmonitor=false"]).args(config);
    }
    cmd.args(args)
        .current_dir(dir)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so a grandchild (a filter, a hook) dies with it.
        .process_group(0);
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    cancelled(run)?;
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("{program} {}: {e}", args.join(" ")))?;
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        kill_group(&mut child);
        bail!("{program} has no output");
    };
    let (tx, done) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 32 << 10];
        let mut cut = false;
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            if out.len() + n > cap {
                out.extend_from_slice(&buf[..cap - out.len()]);
                cut = true;
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        let _ = tx.send((out, cut));
    });
    let err = thread::spawn(move || {
        let mut msg = Vec::new();
        let _ = (&mut stderr).take(8 << 10).read_to_end(&mut msg);
        String::from_utf8_lossy(&msg).trim().to_string()
    });
    let (out, cut) = loop {
        if run.cancel.load(Ordering::Relaxed) {
            kill_group(&mut child);
            bail!("the diff was cancelled");
        }
        match done.recv_timeout(Duration::from_millis(25)) {
            Ok(got) => break got,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < run.until => {}
            Err(_) => {
                kill_group(&mut child);
                bail!(
                    "{program} {} took longer than {:?}",
                    args.join(" "),
                    run.bounds.timeout
                );
            }
        }
    };
    if cut {
        // More than is wanted: stop it, and say so.
        kill_group(&mut child);
        return Ok(Ran {
            out,
            cut: true,
            ok: true,
            err: String::new(),
            status: String::new(),
        });
    }
    let status = child.wait()?;
    Ok(Ran {
        out,
        cut: false,
        ok: status.success(),
        err: if status.success() {
            String::new()
        } else {
            err.join().unwrap_or_default()
        },
        status: status.to_string(),
    })
}

/// An untracked file as an added one, read only if it is a regular file within
/// the bounds: never through a symlink (it could point at anything the daemon
/// can read), never a FIFO or a device (it could block or never end).
fn untracked_file(dir: &Path, raw: &[u8], bounds: &Bounds) -> File {
    let mut file = File {
        path: String::from_utf8_lossy(raw).into_owned(),
        old_path: None,
        status: Status::Added,
        binary: false,
        hunks: Vec::new(),
        unreadable: None,
        untracked: true,
    };
    if std::str::from_utf8(raw).is_err() {
        file.unreadable = Some("the name is not UTF-8".into());
        return file;
    }
    let data = match read_beneath(dir, raw, bounds.max_file) {
        Ok(Ok(data)) => data,
        Ok(Err(why)) => {
            file.unreadable = Some(why.into());
            return file;
        }
        Err(e) => {
            file.unreadable = Some(
                match e.kind() {
                    std::io::ErrorKind::PermissionDenied => "permission denied",
                    std::io::ErrorKind::NotFound => "it went away",
                    _ => "it could not be read",
                }
                .into(),
            );
            return file;
        }
    };
    file.binary = data.contains(&0);
    if !file.binary && !data.is_empty() {
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
        file.hunks = vec![Hunk {
            context: String::new(),
            old_start: 0,
            new_start: 1,
            lines,
        }];
    }
    file
}

fn c_name(part: &[u8]) -> std::io::Result<CString> {
    CString::new(part).map_err(|_| std::io::ErrorKind::InvalidInput.into())
}

/// The bytes of `rel` (a path inside `dir`) if it is a regular file of at
/// most `max` bytes; else why not. Every component is opened from the one
/// before it without following a link, so no directory of the path can be
/// swapped for a link to somewhere else between git's listing and this read;
/// and the leaf is looked at, then opened without blocking, then its type
/// checked on what was opened.
fn read_beneath(
    dir: &Path,
    rel: &[u8],
    max: u64,
) -> std::io::Result<Result<Vec<u8>, &'static str>> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let parts: Vec<&[u8]> = rel.split(|b| *b == b'/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == b"." || *p == b"..")
    {
        return Ok(Err("not a path inside the workspace"));
    }
    let open =
        |from: libc::c_int, name: &CString, flags: libc::c_int| -> std::io::Result<OwnedFd> {
            // SAFETY: `name` is a NUL-terminated string that outlives the call.
            let fd = unsafe { libc::openat(from, name.as_ptr(), flags | libc::O_CLOEXEC) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: openat returned a new descriptor that nothing else owns.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        };
    let mut cur = {
        let root = c_name(dir.as_os_str().as_bytes())?;
        open(libc::AT_FDCWD, &root, libc::O_RDONLY | libc::O_DIRECTORY)?
    };
    let (leaf, dirs) = parts.split_last().expect("a split has a part");
    for part in dirs {
        let name = c_name(part)?;
        cur = match open(
            cur.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        ) {
            Ok(fd) => fd,
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(Err("symlink")),
            Err(e) if e.raw_os_error() == Some(libc::ENOTDIR) => {
                return Ok(Err("not a regular file"));
            }
            Err(e) => return Err(e),
        };
    }
    let name = c_name(leaf)?;
    // SAFETY: a zeroed stat is a valid out-buffer for fstatat.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is NUL-terminated and `st` is a stat.
    if unsafe {
        libc::fstatat(
            cur.as_raw_fd(),
            name.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    match st.st_mode & libc::S_IFMT {
        libc::S_IFLNK => return Ok(Err("symlink")),
        libc::S_IFREG => {}
        _ => return Ok(Err("not a regular file")),
    }
    let fd = match open(
        cur.as_raw_fd(),
        &name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
    ) {
        Ok(fd) => fd,
        // A link put there after the look: ELOOP.
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(Err("symlink")),
        Err(e) => return Err(e),
    };
    let file = std::fs::File::from(fd);
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Ok(Err("not a regular file"));
    }
    if opened.len() > max {
        return Ok(Err("too large"));
    }
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max {
        return Ok(Err("too large"));
    }
    Ok(Ok(data))
}

/// Parses `git diff` output (unified, with git's extended headers).
pub fn parse(text: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            let path = rest.split_once(" b/").map_or(rest, |(_, b)| b).to_string();
            files.push(File {
                unreadable: None,
                untracked: false,
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
            unreadable: None,
            untracked: false,
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

    /// The phone app parses this very file (`android/proto/.../PhoneDiffTest`), so a field
    /// renamed here breaks a test there and the other way round.
    #[test]
    fn the_answer_the_phone_parses_is_what_this_serialises() {
        fn l(k: &'static str, text: &str, old: Option<u32>, new: Option<u32>) -> LineOut {
            LineOut {
                k,
                text: text.into(),
                old,
                new,
            }
        }
        fn f(path: &str, status: &'static str) -> FileOut {
            FileOut {
                path: path.into(),
                old_path: None,
                status,
                binary: false,
                added: 0,
                deleted: 0,
                truncated: false,
                unreadable: None,
                untracked: false,
                hunks: vec![],
            }
        }
        let mut a = f("src/a.ts", "modified");
        a.added = 2;
        a.deleted = 1;
        a.hunks = vec![HunkOut {
            context: "export function track()".into(),
            old_start: 1,
            new_start: 1,
            lines: vec![
                l("ctx", "import x from 'y'", Some(1), Some(1)),
                l("del", "const limit = 10", Some(2), None),
                l("add", "const limit = 20", None, Some(2)),
                l("add", "const extra = true", None, Some(3)),
            ],
        }];
        let mut renamed = f("new.md", "renamed");
        renamed.old_path = Some("old.md".into());
        let mut gone = f("gone.txt", "deleted");
        gone.deleted = 1;
        gone.hunks = vec![HunkOut {
            context: String::new(),
            old_start: 1,
            new_start: 0,
            lines: vec![l("del", "bye", Some(1), None)],
        }];
        let mut img = f("img.png", "modified");
        img.binary = true;
        let mut link = f("link", "added");
        link.unreadable = Some("symlink".into());
        link.untracked = true;
        let mut big = f("big.rs", "added");
        big.added = 5000;
        big.truncated = true;
        big.hunks = vec![HunkOut {
            context: String::new(),
            old_start: 0,
            new_start: 1,
            lines: vec![l("add", "fn main() {}…", None, Some(1))],
        }];
        let report = Report {
            title: "demo vs origin/main".into(),
            truncated: true,
            too_large: None,
            no_base: false,
            files: vec![a, renamed, gone, img, link, big],
        };
        // The bridge adds `t` and `ws`.
        let mut got = serde_json::to_value(&report).unwrap();
        got["t"] = "diff".into();
        got["ws"] = "w1".into();
        got["scope"] = "general".into();
        let want: serde_json::Value = serde_json::from_str(include_str!(
            "../../../android/proto/src/test/resources/phone_diff.json"
        ))
        .unwrap();
        assert_eq!(got, want);

        // A view past `DIFF_CAP`: names only, and how many there were.
        let mut a = f("a/b.rs", "modified");
        a.untracked = false;
        let large = Report {
            title: "demo vs HEAD".into(),
            truncated: false,
            too_large: Some(1234),
            no_base: false,
            files: vec![a, f("c.txt", "added")],
        };
        let mut got = serde_json::to_value(&large).unwrap();
        got["t"] = "diff".into();
        got["ws"] = "w1".into();
        got["scope"] = "uncommitted".into();
        let want: serde_json::Value = serde_json::from_str(include_str!(
            "../../../android/proto/src/test/resources/phone_diff_large.json"
        ))
        .unwrap();
        assert_eq!(got, want);

        let commits = CommitsReport {
            commits: vec![
                CommitOut {
                    sha: "0123456789abcdef0123456789abcdef01234567".into(),
                    short: "0123456".into(),
                    subject: "fix the thing".into(),
                    when: 1_760_000_000,
                },
                CommitOut {
                    sha: "89abcdef0123456789abcdef0123456789abcdef".into(),
                    short: "89abcde".into(),
                    subject: "add the thing".into(),
                    when: 1_759_990_000,
                },
            ],
            more: true,
            no_base: false,
        };
        let mut got = serde_json::to_value(&commits).unwrap();
        got["t"] = "commits".into();
        got["ws"] = "w1".into();
        let want: serde_json::Value = serde_json::from_str(include_str!(
            "../../../android/proto/src/test/resources/phone_commits.json"
        ))
        .unwrap();
        assert_eq!(got, want);
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

    // --- bounds, symlinks and hostile repositories (the W3 review) ---

    /// The files of a load that was not too large to read.
    fn all(s: &Scoped) -> Vec<File> {
        match &s.loaded {
            Loaded::Files(f) => f.clone(),
            other => panic!("too large: {other:?}"),
        }
    }

    /// A repository with a committed `a.txt`, the way the other tests make one.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::connectors::git::tests::{must_git, new_test_repo};
        let (d, work) = new_test_repo();
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
        (d, work)
    }

    fn by_name<'a>(files: &'a [File], name: &str) -> &'a File {
        files
            .iter()
            .find(|f| f.path == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn a_symlink_is_never_followed_so_what_it_points_at_never_leaves() {
        let (_d, work) = repo();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("id_secret");
        std::fs::write(&secret, "TOP-SECRET-KEY\n").unwrap();
        std::os::unix::fs::symlink(&secret, work.join("leak")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", work.join("zero")).unwrap();
        let loaded = load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
        let files = all(&loaded);
        for name in ["leak", "zero"] {
            let f = by_name(&files, name);
            assert_eq!(f.unreadable.as_deref(), Some("symlink"), "{name}");
            assert!(f.hunks.is_empty());
        }
        let json = serde_json::to_string(&limited("t".into(), all(&loaded))).unwrap();
        assert!(!json.contains("TOP-SECRET"), "the secret was sent");
        assert!(json.contains("\"unreadable\":\"symlink\""), "{json}");
    }

    #[test]
    fn a_fifo_does_not_hang_the_diff_or_the_read() {
        let (_d, work) = repo();
        let pipe = work.join("pipe");
        let work2 = work.clone();
        let path = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        // SAFETY: a path we made, and a mode.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // git itself does not list a FIFO as untracked, so the diff goes by.
        let (tx, rx) = mpsc::channel();
        let w = work.clone();
        thread::spawn(move || {
            let _ = tx.send(load_with(&w, &Scope::Uncommitted, Bounds::PHONE));
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the diff hung on a FIFO")
            .unwrap();
        // And the read refuses one on its own, should git ever name it or the
        // file be swapped for one after the listing; without blocking.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(read_beneath(&work2, b"pipe", 1000));
        });
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the read hung on a FIFO");
        assert_eq!(got.unwrap().unwrap_err(), "not a regular file");
    }

    #[test]
    fn a_file_over_the_size_limit_is_not_read_and_one_at_it_is() {
        let (_d, work) = repo();
        std::fs::write(work.join("big.txt"), "x".repeat(101)).unwrap();
        std::fs::write(work.join("fits.txt"), "y".repeat(100)).unwrap();
        let b = Bounds {
            max_file: 100,
            ..Bounds::PHONE
        };
        let files = all(&load_with(&work, &Scope::Uncommitted, b).unwrap());
        assert_eq!(
            by_name(&files, "big.txt").unreadable.as_deref(),
            Some("too large")
        );
        assert!(by_name(&files, "big.txt").hunks.is_empty());
        let fits = by_name(&files, "fits.txt");
        assert!(fits.unreadable.is_none() && fits.hunks[0].lines.len() == 1);
    }

    #[test]
    fn a_file_that_cannot_be_read_says_why_not_that_it_is_empty() {
        // SAFETY: only asks who we are.
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads anything
        }
        let (_d, work) = repo();
        let path = work.join("locked.txt");
        std::fs::write(&path, "secret\n").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        let files = all(&load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap());
        let f = by_name(&files, "locked.txt");
        assert_eq!(f.unreadable.as_deref(), Some("permission denied"));
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn a_name_that_is_not_utf8_is_unreadable_not_another_name() {
        let (_d, work) = repo();
        let name = std::ffi::OsStr::from_bytes(b"bad\xffname.txt");
        if std::fs::write(work.join(name), "x\n").is_err() {
            return; // this file system keeps only valid names
        }
        let files = all(&load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap());
        let f = files
            .iter()
            .find(|f| f.path.starts_with("bad"))
            .expect("listed");
        assert_eq!(f.unreadable.as_deref(), Some("the name is not UTF-8"));
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn git_gets_a_deadline_and_a_cap_and_nothing_outlives_it() {
        let dir = std::env::temp_dir();
        // A command that never ends: killed at the deadline, and reaped.
        let pidfile = dir.join(format!("jw-bounded-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let script = format!("echo $$ > {}; exec sleep 30", pidfile.display());
        let b = Bounds {
            timeout: Duration::from_millis(300),
            ..Bounds::PHONE
        };
        let t = Instant::now();
        let err = git_bounded("sh", &dir, &[], &["-c", &script], &run(b)).unwrap_err();
        assert!(err.to_string().contains("took longer"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_file(&pidfile);
        // SAFETY: signal 0 only checks the process exists.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the child is still there");

        // Output without end: cut at the cap, and said so.
        let b = Bounds {
            max_output: 1000,
            ..Bounds::PHONE
        };
        let t = Instant::now();
        let (out, cut) = git_bounded("yes", &dir, &[], &[], &run(b)).unwrap();
        assert!(cut && out.len() == 1000, "{} bytes, cut {cut}", out.len());
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    /// Runs `script` and tells a marker file it ran; false if it never did.
    fn run(bounds: Bounds) -> Run<'static> {
        static NEVER: AtomicBool = AtomicBool::new(false);
        Run {
            until: Instant::now() + bounds.timeout,
            bounds,
            cancel: &NEVER,
        }
    }

    #[test]
    fn a_repository_cannot_run_a_filter_of_its_own_by_being_diffed() {
        use crate::connectors::git::tests::must_git;
        // The setting that names the command, and the driver's own name: dots
        // in it must not fool the parsing of the key.
        for (driver, setting, value) in [
            ("evil", "clean", "{} %f"),
            ("my.evil", "clean", "{} %f"),
            ("evil", "smudge", "{} %f"),
            ("evil", "process", "{}"),
        ] {
            let (d, work) = repo();
            let (script, marker) = marker_script(d.path(), "filter");
            std::fs::write(
                work.join(".gitattributes"),
                format!("*.txt filter={driver}\n"),
            )
            .unwrap();
            let cmd = value.replace("{}", script.to_str().unwrap());
            must_git(
                &work,
                &["config", &format!("filter.{driver}.{setting}"), &cmd],
            );
            std::fs::write(work.join("a.txt"), "one\n2\n").unwrap();
            std::fs::write(work.join("new.txt"), "x\n").unwrap();
            if setting == "clean" {
                // The control: git itself does run it on a diff.
                must_git(&work, &["diff", "--no-ext-diff"]);
                assert!(
                    marker.exists(),
                    "the control did not run the {driver} filter: the test proves nothing"
                );
                std::fs::remove_file(&marker).unwrap();
            }
            let loaded = load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
            assert!(
                !marker.exists(),
                "diffing ran the repository's {setting} filter of {driver}"
            );
            assert!(all(&loaded).iter().any(|f| f.path == "a.txt"));
            // And a filter that is required does not make the diff fail.
            must_git(
                &work,
                &["config", &format!("filter.{driver}.required"), "true"],
            );
            load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
            assert!(!marker.exists(), "a required filter ran");
            // The same for a commit and for the commit list: what the phone can ask besides the working tree.
            // The setup commits with the filters off: it is loading that is under test.
            let off: Vec<String> = ["clean", "smudge", "process"]
                .iter()
                .flat_map(|k| ["-c".to_string(), format!("filter.{driver}.{k}=")])
                .chain(["-c".to_string(), format!("filter.{driver}.required=false")])
                .collect();
            let git_off = |rest: &[&str]| {
                let mut args: Vec<&str> = off.iter().map(String::as_str).collect();
                args.extend(["-c", "user.name=t", "-c", "user.email=t@t"]);
                args.extend(rest);
                must_git(&work, &args);
            };
            git_off(&["add", "-A"]);
            git_off(&["commit", "-q", "-m", "b"]);
            // What is checked is what loading does, not the setup.
            let _ = std::fs::remove_file(&marker);
            let sha = crate::connectors::git::output(&work, &["rev-parse", "HEAD"]).unwrap();
            let shown = load_with(&work, &Scope::Commit(sha.trim().into()), Bounds::PHONE).unwrap();
            assert!(!all(&shown).is_empty());
            let cancel = AtomicBool::new(false);
            assert!(
                !commits_until(&work, "", 10, Bounds::PHONE, &cancel)
                    .unwrap()
                    .list
                    .is_empty()
            );
            assert!(
                !marker.exists(),
                "showing a commit or listing commits ran a filter"
            );
        }
    }

    #[test]
    fn what_git_started_dies_with_it() {
        let dir = std::env::temp_dir();
        let pidfile = dir.join(format!("jw-grand-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        // The grandchild holds the pipe; only a group kill ends it.
        let script = format!("sleep 30 & echo $! > {}; wait", pidfile.display());
        let b = Bounds {
            timeout: Duration::from_millis(400),
            ..Bounds::PHONE
        };
        let err = git_bounded("sh", &dir, &[], &["-c", &script], &run(b)).unwrap_err();
        assert!(err.to_string().contains("took longer"), "{err}");
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_file(&pidfile);
        for _ in 0..40 {
            // SAFETY: signal 0 only checks the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the grandchild outlived the deadline");
    }

    #[test]
    fn a_cancelled_load_kills_git_at_once() {
        let dir = std::env::temp_dir();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            c.store(true, Ordering::SeqCst);
        });
        let b = Bounds::PHONE;
        let r = Run {
            until: Instant::now() + b.timeout,
            bounds: b,
            cancel: &cancel,
        };
        let t = Instant::now();
        let err = git_bounded("sleep", &dir, &[], &["30"], &r).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        // A load that starts cancelled does not even start git.
        let err = load_until(&dir, &Scope::Uncommitted, Bounds::PHONE, &cancel).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
    }

    #[test]
    fn a_directory_swapped_for_a_link_does_not_leak_what_is_outside() {
        let (d, work) = repo();
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), "top secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, work.join("sub")).unwrap();
        // git listed `sub/secret`, then `sub` became a link (or was one).
        let f = untracked_file(&work, b"sub/secret", &Bounds::PHONE);
        // (ELOOP or ENOTDIR, by the system: either way it is refused.)
        assert!(f.unreadable.is_some() && f.hunks.is_empty(), "{f:?}");
        for bad in [
            &b"../outside/secret"[..],
            b"/etc/passwd",
            b"a//b",
            b"./a.txt",
        ] {
            let f = untracked_file(&work, bad, &Bounds::PHONE);
            assert!(f.unreadable.is_some(), "{}", String::from_utf8_lossy(bad));
        }
        // A real nested file still reads.
        std::fs::create_dir(work.join("real")).unwrap();
        std::fs::write(work.join("real/in.txt"), "hi\n").unwrap();
        let f = untracked_file(&work, b"real/in.txt", &Bounds::PHONE);
        assert!(f.unreadable.is_none() && f.hunks.len() == 1);
    }

    #[test]
    fn only_so_many_untracked_files_are_read() {
        let (_d, work) = repo();
        for i in 0..MAX_UNTRACKED + 5 {
            std::fs::write(work.join(format!("u{i:05}.txt")), "x\n").unwrap();
        }
        let loaded = load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
        assert!(loaded.truncated);
        // `a.txt` is tracked and changed by nothing here: only untracked count.
        let untracked = all(&loaded)
            .iter()
            .filter(|f| f.path.starts_with('u'))
            .count();
        assert_eq!(untracked, MAX_UNTRACKED);
    }

    fn marker_script(
        dir: &std::path::Path,
        name: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let marker = dir.join(format!("{name}.ran"));
        let script = dir.join(format!("{name}.sh"));
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch {}\ncat \"$1\" 2>/dev/null\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        (script, marker)
    }

    #[test]
    fn a_repository_cannot_run_a_command_of_its_own_by_being_diffed() {
        use crate::connectors::git::tests::must_git;
        let (d, work) = repo();
        // `.gitattributes` names a driver, `.git/config` gives it a textconv.
        let (script, marker) = marker_script(d.path(), "textconv");
        std::fs::write(work.join(".gitattributes"), "*.txt diff=evil\n").unwrap();
        must_git(
            &work,
            &["config", "diff.evil.textconv", script.to_str().unwrap()],
        );
        std::fs::write(work.join("a.txt"), "one\n2\n").unwrap();
        // The control: without our flags the repository does run it.
        must_git(&work, &["diff", "--no-ext-diff"]);
        assert!(
            marker.exists(),
            "the control did not run the textconv: the test proves nothing"
        );
        std::fs::remove_file(&marker).unwrap();
        load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
        assert!(!marker.exists(), "diffing ran the repository's textconv");

        // And core.fsmonitor, which runs on a read of the index.
        let (script, fs_marker) = marker_script(d.path(), "fsmonitor");
        must_git(
            &work,
            &["config", "core.fsmonitor", script.to_str().unwrap()],
        );
        let _ = std::process::Command::new("git")
            .args(["status", "--short"])
            .current_dir(&work)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output();
        assert!(
            fs_marker.exists(),
            "the control did not run the fsmonitor: the test proves nothing"
        );
        std::fs::remove_file(&fs_marker).unwrap();
        load_with(&work, &Scope::Uncommitted, Bounds::PHONE).unwrap();
        assert!(
            !fs_marker.exists(),
            "diffing ran the repository's fsmonitor"
        );
    }

    #[test]
    fn a_reply_stays_under_its_budget_and_says_it_was_cut() {
        // About 20 MB of lines: 100 files of 2000 lines of 100 characters.
        let files: Vec<File> = (0..100)
            .map(|i| file(&format!("f{i}.rs"), 2000, &"x".repeat(100)))
            .collect();
        let report = limited("t".into(), files);
        let json = serde_json::to_string(&report).unwrap();
        assert!(report.truncated);
        assert!(json.len() <= MAX_REPLY, "{} bytes", json.len());
        assert!(
            json.len() > MAX_REPLY / 2,
            "the budget cut far too early: {}",
            json.len()
        );
        // Escapes cost more than they look: control characters become \u00XX.
        let files: Vec<File> = (0..20)
            .map(|i| file(&format!("g{i}.rs"), 2000, &"\u{1}".repeat(400)))
            .collect();
        let json = serde_json::to_string(&limited("t".into(), files)).unwrap();
        assert!(json.len() <= MAX_REPLY, "{} bytes", json.len());
    }

    #[test]
    fn the_cuts_land_on_the_character_boundary_and_paths_are_cut_too() {
        let exact = "a".repeat(MAX_TEXT);
        assert_eq!(clip(&exact), exact);
        let over = "a".repeat(MAX_TEXT + 1);
        let cut = clip(&over);
        assert_eq!(cut.chars().count(), MAX_TEXT);
        assert!(cut.ends_with('…'));
        // Multi-byte: 501 two-byte characters cut without splitting one.
        let wide = "é".repeat(MAX_TEXT + 1);
        let cut = clip(&wide);
        assert_eq!(cut.chars().count(), MAX_TEXT);
        assert!(cut.starts_with("éé") && cut.ends_with('…'));
        let report = limited("t".into(), vec![file(&"p".repeat(5000), 1, "x")]);
        assert_eq!(report.files[0].path.chars().count(), 1024);
    }
}

#[cfg(test)]
mod scope_tests {
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

    /// REQ-223: General is what was committed, not what is edited since.
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

    /// REQ-224: staged, unstaged and untracked, against HEAD.
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

    /// REQ-224: a repository with no commit yet shows what is staged and
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

    /// REQ-226: no base, nothing to compare General with.
    #[test]
    fn general_with_no_base_lists_nothing() {
        let (_d, work) = feature_repo();
        let none = Scope::Branch {
            base: String::new(),
        };
        assert_eq!(load(&work, &none).unwrap(), Loaded::Files(Vec::new()));
    }

    /// REQ-225, REQ-226: newest first, the limit, and whether more exist.
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

    /// REQ-238: past the cap, the file list and the total, no parsed diff.
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

    /// REQ-246: git's complaint comes back as an error, and an id that is not
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
