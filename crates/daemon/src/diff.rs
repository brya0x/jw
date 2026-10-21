//! The changes of a workspace, read from git: the three scopes of a `diff` node, the list of
//! commits, and the poll that keeps an `uncommitted` node current while someone watches it.
//! Git does the comparing; this only runs it and shapes what it prints (REQ-91 to REQ-103).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use contract::{CommitInfo, DiffFile, DiffState, FileStatus, Hunk};

use crate::git;
use crate::server::Shared;

/// A diff that prints more than this lists its files by name only.
pub const CAP: usize = 2 * 1024 * 1024;
/// Files listed for a diff over `CAP`.
const MAX_FILES: usize = 200;
/// An untracked file bigger than this is not read.
const UNTRACKED_CAP: u64 = 512 * 1024;
/// A NUL in the first bytes of a file marks it binary, as git decides.
const SNIFF: usize = 8000;
const MAX_COMMITS: usize = 200;
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// How often a watched `uncommitted` node is read again.
const EVERY: Duration = Duration::from_secs(2);

/// What a diff compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Uncommitted,
    General,
    Commit(String),
}

impl Scope {
    /// The key marks of having looked are kept under.
    pub fn key(&self) -> String {
        match self {
            Scope::Uncommitted => "uncommitted".into(),
            Scope::General => "general".into(),
            Scope::Commit(sha) => format!("commit|{sha}"),
        }
    }

    pub fn kind(&self) -> contract::DiffScope {
        match self {
            Scope::Uncommitted => contract::DiffScope::Uncommitted,
            Scope::General => contract::DiffScope::General,
            Scope::Commit(_) => contract::DiffScope::Commit,
        }
    }
}

/// What a read of one scope found. Equal reads mean nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub base: Option<String>,
    pub state: DiffState,
    pub too_large: bool,
    pub omitted: Option<u32>,
    pub files: Vec<DiffFile>,
}

/// The branch's commits, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitList {
    pub base: Option<String>,
    pub more: bool,
    pub commits: Vec<CommitInfo>,
}

pub enum Fail {
    /// The folder is not inside a git repository.
    NotRepo,
    /// The request itself is wrong (a commit id that is not hexadecimal).
    Bad(String),
    /// Git failed; its own message.
    Git(String),
}

impl From<String> for Fail {
    fn from(message: String) -> Fail {
        Fail::Git(message)
    }
}

/// A commit id git cannot mistake for an option or a ref name: 4 to 40 hexadecimal digits.
pub fn valid_commit(sha: &str) -> bool {
    (4..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The top of the repository `dir` is in: every read runs there, so a workspace that is a
/// folder inside a repository shows the repository's changes.
fn repo_top(dir: &Path) -> Result<PathBuf, Fail> {
    git::toplevel(dir)
        .map(PathBuf::from)
        .map_err(|_| Fail::NotRepo)
}

fn has_head(top: &Path) -> bool {
    git::run(top, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok()
}

/// The file in a worktree's own git folder that says what its review compares against (REQ-586).
fn review_base_file(top: &Path) -> Option<PathBuf> {
    let dir = git::run(top, &["rev-parse", "--absolute-git-dir"]).ok()?;
    Some(PathBuf::from(dir).join(format!("{}-review-base", identity::get().name)))
}

/// Make `base` (`origin/<branch>`) what the changes of the worktree at `top` compare against, in
/// place of the repository's default branch: a review is the pull request's change, not the
/// branch's.
pub fn set_review_base(top: &Path, base: &str) -> Result<(), String> {
    let file = review_base_file(top).ok_or("not a git worktree")?;
    std::fs::write(&file, base).map_err(|e| format!("{}: {e}", file.display()))
}

/// What a review compares against, when the worktree is one.
fn review_base(top: &Path) -> Option<String> {
    let text = std::fs::read_to_string(review_base_file(top)?).ok()?;
    Some(text.trim().to_owned()).filter(|b| !b.is_empty())
}

/// What the changes of a worktree are compared with: the base of its review, or else
/// `origin/<default branch>`, when origin says what its default branch is.
fn base_of(top: &Path) -> Option<String> {
    if let Some(base) = review_base(top) {
        return Some(base);
    }
    git::run(
        top,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .filter(|b| !b.is_empty())
}

/// The full id of a commit named by 4 to 40 hexadecimal digits, so one commit is one scope
/// whatever it was called by.
pub fn resolve_commit(dir: &Path, sha: &str) -> Result<String, Fail> {
    if !valid_commit(sha) {
        return Err(Fail::Bad(format!("{sha:?} is not a commit id")));
    }
    let top = repo_top(dir)?;
    git::run(
        &top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ],
    )
    .map_err(|_| Fail::Git(format!("no commit {sha}")))
}

pub fn read(dir: &Path, scope: &Scope) -> Result<Read, Fail> {
    let top = repo_top(dir)?;
    let (base, command): (Option<String>, Vec<String>) = match scope {
        Scope::Uncommitted => {
            let rev = if has_head(&top) { "HEAD" } else { EMPTY_TREE };
            (None, vec!["diff".into(), rev.into()])
        }
        Scope::General => {
            let Some(base) = base_of(&top) else {
                return Ok(Read {
                    base: None,
                    state: DiffState::NoBase,
                    too_large: false,
                    omitted: None,
                    files: Vec::new(),
                });
            };
            let merge_base = git::run(&top, &["merge-base", &base, "HEAD"])?;
            (Some(base), vec!["diff".into(), merge_base, "HEAD".into()])
        }
        Scope::Commit(sha) => {
            if !valid_commit(sha) {
                return Err(Fail::Bad(format!("{sha:?} is not a commit id")));
            }
            let args = ["show", "-m", "--first-parent", "--format=", sha];
            (None, args.map(str::to_owned).to_vec())
        }
    };
    let mut files = tracked(&top, &command)?;
    if matches!(scope, Scope::Uncommitted)
        && let Some(listed) = files.take()
    {
        let (more, over) = untracked(&top, patch_size(&listed))?;
        files = (!over).then(|| [listed, more].concat());
    }
    let (mut files, too_large, omitted) = match files {
        Some(files) => (files, false, None),
        None => names_only(&top, scope, &command)?,
    };
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Read {
        base,
        state: DiffState::Ok,
        too_large,
        omitted,
        files,
    })
}

fn patch_size(files: &[DiffFile]) -> usize {
    files
        .iter()
        .flat_map(|f| &f.hunks)
        .flat_map(|h| &h.lines)
        .map(|l| l.len() + 1)
        .sum()
}

/// The arguments of one git read, with nothing in them that depends on the user's own
/// configuration: no colour, no external diff or text conversion, fixed path prefixes and
/// unquoted non-ASCII paths. `command[0]` is the git command, the rest its operands.
fn diff_args(command: &[String], extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "-c",
        "core.quotepath=false",
        &command[0],
        "-M",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    args.extend(command[1..].iter().cloned());
    args
}

/// Run a diff or show and parse it; `None` when it passed the cap.
fn tracked(top: &Path, command: &[String]) -> Result<Option<Vec<DiffFile>>, Fail> {
    let out = git::read(top, &diff_args(command, &[]), CAP)?;
    Ok((!out.over).then(|| parse(&String::from_utf8_lossy(&out.bytes))))
}

/// The paths git lists as untracked and not ignored.
fn untracked_paths(top: &Path) -> Result<Option<Vec<String>>, Fail> {
    let out = git::read(
        top,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        CAP,
    )?;
    Ok((!out.over).then(|| {
        out.bytes
            .split(|b| *b == 0)
            .filter(|n| !n.is_empty())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect()
    }))
}

/// The untracked files, read from disk. The second value is true when they, with `already`
/// bytes of diff before them, pass the cap.
fn untracked(top: &Path, already: usize) -> Result<(Vec<DiffFile>, bool), Fail> {
    let Some(paths) = untracked_paths(top)? else {
        return Ok((Vec::new(), true));
    };
    let mut files = Vec::new();
    let mut total = already;
    for name in paths {
        let file = read_untracked(&top.join(&name), name);
        total += patch_size(std::slice::from_ref(&file));
        files.push(file);
        if total > CAP {
            return Ok((files, true));
        }
    }
    Ok((files, false))
}

fn read_untracked(path: &Path, name: String) -> DiffFile {
    let mut file = blank(name, FileStatus::Untracked);
    let unread = |mut f: DiffFile, why: &str| {
        f.unreadable = Some(why.into());
        finish(f)
    };
    if file.path.ends_with('/') {
        return unread(file, "repository");
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return unread(file, "gone");
    };
    if meta.file_type().is_symlink() {
        return unread(file, "symlink");
    }
    // A named pipe or a device would block the read for ever.
    if !meta.file_type().is_file() {
        return unread(file, "special");
    }
    let mut bytes = Vec::new();
    let read =
        std::fs::File::open(path).and_then(|f| f.take(UNTRACKED_CAP + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return unread(file, "unreadable");
    }
    if bytes.len() as u64 > UNTRACKED_CAP {
        return unread(file, "too_big");
    }
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        file.binary = true;
        return finish(file);
    }
    let lines: Vec<String> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(|l| format!("+{l}"))
        .collect();
    file.added = lines.len() as u32;
    if !lines.is_empty() {
        file.hunks.push(Hunk {
            context: String::new(),
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: file.added,
            lines,
        });
    }
    finish(file)
}

/// A diff over the cap, listed by name: no hunks, no counts, at most `MAX_FILES`.
fn names_only(
    top: &Path,
    scope: &Scope,
    command: &[String],
) -> Result<(Vec<DiffFile>, bool, Option<u32>), Fail> {
    let out = git::read(top, &diff_args(command, &["--name-status", "-z"]), CAP * 2)?;
    let mut parts = out.bytes.split(|b| *b == 0).map(String::from_utf8_lossy);
    let mut files = Vec::new();
    while let Some(letter) = parts.next().and_then(|s| s.chars().next()) {
        let Some(path) = parts.next() else { break };
        let file = match letter {
            'R' | 'C' => {
                let Some(new) = parts.next() else { break };
                let mut f = blank(new, FileStatus::Renamed);
                f.old_path = Some(path.into_owned());
                f
            }
            'A' => blank(path, FileStatus::Added),
            'D' => blank(path, FileStatus::Deleted),
            _ => blank(path, FileStatus::Modified),
        };
        files.push(finish(file));
    }
    if matches!(scope, Scope::Uncommitted) {
        for name in untracked_paths(top)?.unwrap_or_default() {
            files.push(finish(blank(name, FileStatus::Untracked)));
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let left = files.len().saturating_sub(MAX_FILES) as u32;
    files.truncate(MAX_FILES);
    Ok((files, true, (left > 0).then_some(left)))
}

/// A file of `status` with nothing read from it yet.
fn blank(path: impl Into<String>, status: FileStatus) -> DiffFile {
    DiffFile {
        path: path.into(),
        old_path: None,
        status,
        binary: false,
        unreadable: None,
        added: 0,
        deleted: 0,
        hash: String::new(),
        seen: None,
        hunks: Vec::new(),
    }
}

/// Fill in a file's `hash`: a fingerprint of everything the file says, so a mark of having
/// looked at it stops holding when any of it changes.
fn finish(mut file: DiffFile) -> DiffFile {
    let mut text = format!(
        "{}\0{:?}\0{}\0{}\0{}",
        file.path,
        file.status,
        file.old_path.as_deref().unwrap_or(""),
        file.binary,
        file.unreadable.as_deref().unwrap_or("")
    );
    for h in &file.hunks {
        text.push_str(&format!(
            "\0@@{},{},{},{}",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        ));
        for l in &h.lines {
            text.push('\0');
            text.push_str(l);
        }
    }
    file.hash = web::auth::sha256_hex(&text)[..16].to_owned();
    file
}

/// The commits of the branch: `origin/<base>..HEAD`, or the history of `HEAD` with no base.
pub fn read_commits(dir: &Path) -> Result<CommitList, Fail> {
    let top = repo_top(dir)?;
    let base = base_of(&top);
    if !has_head(&top) {
        return Ok(CommitList {
            base,
            more: false,
            commits: Vec::new(),
        });
    }
    let range = base
        .as_ref()
        .map_or_else(|| "HEAD".to_owned(), |b| format!("{b}..HEAD"));
    let limit = format!("-n{}", MAX_COMMITS + 1);
    let out = git::read(
        &top,
        &[
            "log",
            &limit,
            "--no-color",
            "--format=%H%x1f%h%x1f%ct%x1f%s",
            &range,
        ],
        CAP,
    )?;
    let mut commits: Vec<CommitInfo> = String::from_utf8_lossy(&out.bytes)
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\u{1f}');
            Some(CommitInfo {
                sha: f.next()?.to_owned(),
                short: f.next()?.to_owned(),
                time: f.next()?.parse().ok()?,
                subject: f.next()?.to_owned(),
            })
        })
        .collect();
    let more = commits.len() > MAX_COMMITS;
    commits.truncate(MAX_COMMITS);
    Ok(CommitList {
        base,
        more,
        commits,
    })
}

/// Undo git's C-style quoting of a path (`"a\tb"`, `"\303\251"`); a path that is not
/// quoted is returned as it is.
fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return text.to_owned();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes[i] {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            c @ b'0'..=b'7' => {
                let mut value = u32::from(c - b'0');
                let mut digits = 1;
                while digits < 3 && i + 1 < bytes.len() && (b'0'..=b'7').contains(&bytes[i + 1]) {
                    i += 1;
                    value = value * 8 + u32::from(bytes[i] - b'0');
                    digits += 1;
                }
                out.push(value as u8);
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path as the `---` and `+++` lines print it, without git's `a/` or `b/`; `None` for
/// `/dev/null`.
fn path_of(text: &str, prefix: &str) -> Option<String> {
    let text = unquote(text.strip_suffix('\t').unwrap_or(text));
    if text == "/dev/null" {
        return None;
    }
    Some(text.strip_prefix(prefix).unwrap_or(&text).to_owned())
}

/// Parse the output of `git diff` or `git show`: one `DiffFile` per `diff --git` block.
/// Every line inside a hunk starts with `+`, `-` or a space, so a line that starts with
/// `diff --git` always begins a file.
pub fn parse(patch: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("diff --git ") && !block.is_empty() {
            files.push(parse_block(&block));
            block.clear();
        }
        block.push(line);
    }
    if !block.is_empty() {
        files.push(parse_block(&block));
    }
    files
}

/// The paths a file's header lines name.
#[derive(Default)]
struct Paths {
    from: Option<String>,
    to: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
}

fn parse_block(block: &[&str]) -> DiffFile {
    let mut file = blank("", FileStatus::Modified);
    let mut paths = Paths::default();
    let mut current: Option<Hunk> = None;
    for line in &block[1..] {
        if let Some(h) = current.as_mut() {
            match line.as_bytes().first() {
                Some(b'+') => file.added += 1,
                Some(b'-') => file.deleted += 1,
                Some(b' ') => {}
                // git's note that a file has no newline at its end
                Some(b'\\') => continue,
                // a blank context line whose space was lost
                None => {
                    h.lines.push(" ".into());
                    continue;
                }
                // not part of the hunk: it is over
                Some(_) => {
                    file.hunks.extend(current.take());
                    header_line(line, &mut file, &mut paths, &mut current);
                    continue;
                }
            }
            h.lines.push((*line).to_owned());
        } else {
            header_line(line, &mut file, &mut paths, &mut current);
        }
    }
    file.hunks.extend(current);
    match (paths.rename_from, paths.rename_to) {
        (Some(old), Some(new)) => {
            file.status = FileStatus::Renamed;
            file.old_path = Some(old);
            file.path = new;
        }
        _ => {
            file.path = paths
                .to
                .or(paths.from)
                .unwrap_or_else(|| header_path(block[0]))
        }
    }
    finish(file)
}

/// One line outside a hunk's body: a mode, a rename, a path, or the start of a hunk.
fn header_line(line: &str, file: &mut DiffFile, paths: &mut Paths, current: &mut Option<Hunk>) {
    if let Some(rest) = line.strip_prefix("@@ ") {
        *current = parse_hunk_head(rest);
    } else if line.starts_with("new file mode") {
        file.status = FileStatus::Added;
    } else if line.starts_with("deleted file mode") {
        file.status = FileStatus::Deleted;
    } else if let Some(p) = line.strip_prefix("rename from ") {
        paths.rename_from = Some(unquote(p));
    } else if let Some(p) = line.strip_prefix("rename to ") {
        paths.rename_to = Some(unquote(p));
    } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
        file.binary = true;
    } else if let Some(p) = line.strip_prefix("--- ") {
        paths.from = path_of(p, "a/");
    } else if let Some(p) = line.strip_prefix("+++ ") {
        paths.to = path_of(p, "b/");
    }
}

/// The path from `diff --git a/P b/P`, for a file with no `---`/`+++` lines (a binary file,
/// a change of mode). Both halves are the same path, so its length is known even with spaces.
fn header_path(header: &str) -> String {
    let rest = header.strip_prefix("diff --git ").unwrap_or(header);
    if rest.starts_with('"') {
        let first = rest.find("\" \"b/").map_or(rest, |i| &rest[..=i]);
        let path = unquote(first);
        return path.strip_prefix("a/").unwrap_or(&path).to_owned();
    }
    let half = rest.len().saturating_sub(5) / 2;
    match rest.get(2..2 + half) {
        Some(p) if rest.starts_with("a/") && rest.len() == 2 * half + 5 => p.to_owned(),
        _ => rest.strip_prefix("a/").unwrap_or(rest).to_owned(),
    }
}

/// `-a[,b] +c[,d] @@ context`
fn parse_hunk_head(rest: &str) -> Option<Hunk> {
    let (ranges, context) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let range = |text: &str, sign: char| -> Option<(u32, u32)> {
        let body = text.strip_prefix(sign)?;
        match body.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((body.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old, '-')?;
    let (new_start, new_lines) = range(new, '+')?;
    Some(Hunk {
        context: context.trim_start().to_owned(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

/// One round of reads for the nodes being watched, each off the hub's lock.
fn poll(shared: &Shared) {
    let jobs = shared.lock().diff_jobs();
    for job in jobs {
        match read(&job.dir, &job.scope) {
            Ok(read) => shared.lock().diff_update(&job.node, read),
            Err(Fail::Git(why)) => eprintln!("changes: {why}"),
            Err(_) => {}
        }
    }
}

pub fn run(shared: Shared, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        poll(&shared);
        std::thread::sleep(EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modified_file_has_its_hunks_and_counts() {
        let patch = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@ fn main() {\n a\n-b\n+c\n+d\n e\n@@ -10 +11 @@\n-x\n+y\n";
        let files = parse(patch);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.path, "src/a.rs");
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!((f.added, f.deleted), (3, 2));
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].context, "fn main() {");
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.lines, [" a", "-b", "+c", "+d", " e"]);
        assert_eq!(
            (f.hunks[1].old_lines, f.hunks[1].new_lines),
            (1, 1),
            "a range with no count is one line"
        );
        assert_eq!(f.hash.len(), 16);
    }

    #[test]
    fn added_deleted_and_renamed_files_are_told_apart() {
        let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n\
diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 1..0\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\
diff --git a/old name.rs b/new name.rs\nsimilarity index 90%\nrename from old name.rs\nrename to new name.rs\nindex 1..2 100644\n--- a/old name.rs\t\n+++ b/new name.rs\t\n@@ -1 +1 @@\n-a\n+b\n";
        let files = parse(patch);
        assert_eq!(files.len(), 3);
        assert_eq!(
            (files[0].status, files[0].path.as_str()),
            (FileStatus::Added, "new.txt")
        );
        assert_eq!(
            (files[1].status, files[1].path.as_str()),
            (FileStatus::Deleted, "gone.txt")
        );
        assert_eq!(files[2].status, FileStatus::Renamed);
        assert_eq!(files[2].path, "new name.rs");
        assert_eq!(files[2].old_path.as_deref(), Some("old name.rs"));
    }

    #[test]
    fn a_binary_file_and_a_change_of_mode_have_no_hunks_and_keep_their_path() {
        let patch = "diff --git a/img/my logo.png b/img/my logo.png\nindex 1..2 100644\nBinary files a/img/my logo.png and b/img/my logo.png differ\n\
diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        let files = parse(patch);
        assert_eq!(files[0].path, "img/my logo.png");
        assert!(files[0].binary && files[0].hunks.is_empty());
        assert_eq!(files[1].path, "run.sh");
        assert!(!files[1].binary && files[1].hunks.is_empty());
    }

    #[test]
    fn a_quoted_path_is_unquoted() {
        assert_eq!(unquote("\"a\\tb\\\"c\\\\\""), "a\tb\"c\\");
        assert_eq!(unquote("\"caf\\303\\251\""), "café");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(
            header_path("diff --git \"a/x\\ty\" \"b/x\\ty\""),
            "x\ty",
            "a quoted header still gives the path"
        );
    }

    #[test]
    fn a_line_that_looks_like_a_header_inside_a_hunk_stays_in_its_file() {
        let patch = "diff --git a/a.md b/a.md\n--- a/a.md\n+++ b/a.md\n@@ -1 +1,2 @@\n a\n+diff --git a/x b/x\n";
        let files = parse(patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks[0].lines, [" a", "+diff --git a/x b/x"]);
    }

    #[test]
    fn the_no_newline_note_is_dropped() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n";
        let f = &parse(patch)[0];
        assert_eq!(f.hunks[0].lines, ["-x", "+y"]);
    }

    #[test]
    fn the_hash_follows_the_content() {
        let a = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n");
        let b = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+z\n");
        let again = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n");
        assert_ne!(a[0].hash, b[0].hash);
        assert_eq!(a[0].hash, again[0].hash);
    }

    #[test]
    fn an_untracked_file_is_read_unless_it_is_not_a_plain_file_or_too_big() {
        let dir = std::env::temp_dir().join(format!("jwn-untracked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x\ny\n").unwrap();
        std::fs::write(dir.join("big.txt"), vec![b'a'; UNTRACKED_CAP as usize + 1]).unwrap();
        std::fs::write(dir.join("bin"), [1u8, 0, 2]).unwrap();
        let fifo = std::process::Command::new("mkfifo")
            .arg(dir.join("pipe"))
            .status()
            .unwrap();
        assert!(fifo.success());

        let read = |name: &str| read_untracked(&dir.join(name), name.to_owned());
        assert_eq!(read("a.txt").hunks[0].lines, ["+x", "+y"]);
        assert_eq!(read("big.txt").unreadable.as_deref(), Some("too_big"));
        assert!(read("bin").binary);
        assert_eq!(read("pipe").unreadable.as_deref(), Some("special"));
        assert_eq!(read("missing").unreadable.as_deref(), Some("gone"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_hexadecimal_ids_of_a_sane_length_are_commits() {
        assert!(valid_commit("89abcde"));
        assert!(valid_commit(&"a".repeat(40)));
        assert!(!valid_commit("abc"));
        assert!(!valid_commit(&"a".repeat(41)));
        assert!(!valid_commit("--output=x"));
        assert!(!valid_commit("main"));
    }
}
