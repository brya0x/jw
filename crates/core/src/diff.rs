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
    /// Why its content is not here (see `File::unreadable`), else `null`.
    pub unreadable: Option<String>,
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
pub fn report(entry: &crate::core::registry::Entry, cancel: &AtomicBool) -> Result<Report> {
    let dir = Path::new(&entry.path);
    if !dir.join(".git").exists() {
        bail!("{} is not a git repository", entry.name);
    }
    let stream = crate::stream::Stream::resolve(entry)?;
    let loaded = load_until(dir, &stream.base, Bounds::PHONE, cancel)?;
    let title = if stream.base.is_empty() {
        format!("{} vs HEAD", entry.name)
    } else {
        format!("{} vs {}", entry.name, stream.base)
    };
    let mut report = limited(title, loaded.files);
    report.truncated |= loaded.truncated;
    Ok(report)
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

/// A diff, and whether a bound cut it short.
#[derive(Debug)]
pub struct Loaded {
    pub files: Vec<File>,
    pub truncated: bool,
}

/// Everything the stream changed against `origin/<base>`: committed,
/// staged, unstaged and untracked. With no `base` (a repository whose
/// origin has no default branch, or no origin), against `HEAD`: what is
/// not committed yet (REQ-123).
pub fn load(dir: &Path, base: &str) -> Result<Vec<File>> {
    Ok(load_with(dir, base, Bounds::LOCAL)?.files)
}

/// [`load`] within `bounds`.
pub fn load_with(dir: &Path, base: &str, bounds: Bounds) -> Result<Loaded> {
    load_until(dir, base, bounds, &AtomicBool::new(false))
}

/// [`load_with`], stopping at once (and killing git) when `cancel` is set:
/// the phone that asked went away.
pub fn load_until(dir: &Path, base: &str, bounds: Bounds, cancel: &AtomicBool) -> Result<Loaded> {
    // One deadline for the whole load, not for each git call.
    let run = Run {
        until: Instant::now() + bounds.timeout,
        bounds,
        cancel,
    };
    let overrides = filter_overrides(dir, &run)?;
    let base_ref = format!("origin/{base}");
    let mut args = vec!["diff", "-M", "--no-color", "--no-ext-diff", "--no-textconv"];
    if base.is_empty() {
        // Before the first commit there is no HEAD: everything staged is new.
        let born = git_bounded(
            "git",
            dir,
            &[],
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            &run,
        )
        .is_ok();
        args.push(if born { "HEAD" } else { EMPTY_TREE });
    } else {
        args.extend(["--merge-base", &base_ref]);
    }
    let (out, mut truncated) = git_bounded("git", dir, &overrides, &args, &run)?;
    let mut files = parse(&String::from_utf8_lossy(&out));
    // git diff leaves out untracked files; they are new files all the same.
    let (names, cut) = git_bounded(
        "git",
        dir,
        &overrides,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &run,
    )?;
    truncated |= cut;
    // Names are bytes, not text: one that is not UTF-8 is said to be
    // unreadable, not changed into another name and read as an empty file.
    // Only so many are read, and none after the deadline: the reads are not
    // git's, so git's own deadline does not cover them.
    for (n, raw) in names
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .enumerate()
    {
        if cancelled(&run)? || n >= MAX_UNTRACKED || Instant::now() >= run.until {
            truncated = true;
            break;
        }
        files.push(untracked_file(dir, raw, &run.bounds));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Loaded { files, truncated })
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
    let ran = git_run(program, dir, config, args, run)?;
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

fn kill_group(child: &mut std::process::Child) {
    // SAFETY: a signal to the group this child leads; the group is ours.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn git_run(program: &str, dir: &Path, config: &[String], args: &[&str], run: &Run) -> Result<Ran> {
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
    let cap = run.bounds.max_output;
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
            LineOut { k, text: text.into(), old, new }
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
            files: vec![a, renamed, gone, img, link, big],
        };
        // The bridge adds `t` and `ws`.
        let mut got = serde_json::to_value(&report).unwrap();
        got["t"] = "diff".into();
        got["ws"] = "w1".into();
        let want: serde_json::Value = serde_json::from_str(include_str!(
            "../../../android/proto/src/test/resources/phone_diff.json"
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
        let loaded = load_with(&work, "trunk", Bounds::PHONE).unwrap();
        for name in ["leak", "zero"] {
            let f = by_name(&loaded.files, name);
            assert_eq!(f.unreadable.as_deref(), Some("symlink"), "{name}");
            assert!(f.hunks.is_empty());
        }
        let json = serde_json::to_string(&limited("t".into(), loaded.files)).unwrap();
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
            let _ = tx.send(load_with(&w, "trunk", Bounds::PHONE));
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
        let files = load_with(&work, "trunk", b).unwrap().files;
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
        let files = load_with(&work, "trunk", Bounds::PHONE).unwrap().files;
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
        let files = load_with(&work, "trunk", Bounds::PHONE).unwrap().files;
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
            let loaded = load_with(&work, "trunk", Bounds::PHONE).unwrap();
            assert!(
                !marker.exists(),
                "diffing ran the repository's {setting} filter of {driver}"
            );
            assert!(loaded.files.iter().any(|f| f.path == "a.txt"));
            // And a filter that is required does not make the diff fail.
            must_git(
                &work,
                &["config", &format!("filter.{driver}.required"), "true"],
            );
            load_with(&work, "trunk", Bounds::PHONE).unwrap();
            assert!(!marker.exists(), "a required filter ran");
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
        let err = load_until(&dir, "trunk", Bounds::PHONE, &cancel).unwrap_err();
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
        let loaded = load_with(&work, "trunk", Bounds::PHONE).unwrap();
        assert!(loaded.truncated);
        // `a.txt` is tracked and changed by nothing here: only untracked count.
        let untracked = loaded
            .files
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
        load_with(&work, "trunk", Bounds::PHONE).unwrap();
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
        load_with(&work, "trunk", Bounds::PHONE).unwrap();
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
