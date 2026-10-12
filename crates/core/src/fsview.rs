//! The PC's files for a phone (mobile REQ-77 to 81): a directory's entries and a file's text, bounded. A path is any
//! the daemon's user can read; what is bounded is the cost: entries, bytes, time, and what kind of file is opened.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::md;

/// The most entries one listing holds, and the most directory entries looked at to make it.
pub const MAX_ENTRIES: usize = 2000;
const MAX_SCAN: usize = 20_000;
/// The most bytes of a file that are sent.
pub const MAX_READ: usize = 1 << 20;
/// How much of a file is looked at for a NUL, to say it is not text.
const SNIFF: usize = 8 << 10;
/// How long a listing or a read may take: a hung network mount must not hold the ask for ever.
pub const DEADLINE: Duration = Duration::from_secs(5);
/// Work that is still blocked in the kernel after its deadline: past this many, more is refused.
const MAX_STUCK: usize = 8;

static STUCK: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dir,
    File,
    Link,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    /// For a link: what it reaches (`dir`, `file`, `missing`).
    pub target: Option<&'static str>,
    pub md: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    /// How many entries the directory had (up to what was looked at).
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadKind {
    Text,
    Md,
    Binary,
}

#[derive(Debug, Serialize)]
pub struct FileText {
    pub path: String,
    pub kind: ReadKind,
    pub size: u64,
    pub truncated: bool,
    pub text: Option<String>,
    pub blocks: Option<Vec<md::Block>>,
}

/// `path` as an absolute path: `~` is the home; a relative path, one with `..`, or one that is empty is refused.
pub fn resolve(path: &str) -> Result<PathBuf> {
    let p = if path == "~" || path.starts_with("~/") {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
        let Some(home) = home else {
            bail!("the PC has no home folder")
        };
        PathBuf::from(home).join(path.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(path)
    };
    if !p.is_absolute() {
        bail!("not an absolute path");
    }
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => bail!("a path with .. is not accepted"),
            Component::Normal(n) => out.push(n),
            Component::Prefix(_) => bail!("not an absolute path"),
        }
    }
    Ok(out)
}

fn is_md(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".md") || n.ends_with(".markdown")
}

/// Why an I/O error says what it says, in words for a phone.
fn why(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or folder".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => match e.raw_os_error() {
            Some(libc::ENOTDIR) => "not a folder".into(),
            Some(libc::ELOOP) => "too many links".into(),
            _ => "it could not be read".into(),
        },
    }
}

/// Runs `work` on a thread of its own and waits for it until the deadline or `cancel`: the kernel cannot be told to
/// stop a read on a hung mount, but the ask can be let go of. A thread still blocked is counted, and past
/// [`MAX_STUCK`] of them nothing more is started.
fn bounded<T: Send + 'static>(
    cancel: &AtomicBool,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if STUCK.load(Ordering::SeqCst) >= MAX_STUCK {
        bail!("the PC's files are not answering");
    }
    let (tx, rx) = mpsc::channel();
    let until = Instant::now() + DEADLINE;
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(r) => return r,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < until => {}
            Err(_) => {
                // The thread goes on; it is counted until it ends.
                STUCK.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    let _ = rx.recv();
                    STUCK.fetch_sub(1, Ordering::SeqCst);
                });
                bail!("took longer than {} s", DEADLINE.as_secs());
            }
        }
    }
}

/// The entries of the directory `path`, directories first, then by name; at most [`MAX_ENTRIES`].
pub fn list(path: &str, cancel: &AtomicBool) -> Result<Listing> {
    let dir = resolve(path)?;
    let shown = dir.to_string_lossy().into_owned();
    let parent = dir.parent().map(|p| p.to_string_lossy().into_owned());
    let work = move || scan(&dir);
    let (mut entries, total, more) = bounded(cancel, work)?;
    entries.sort_by(|a, b| {
        let dir_first =
            |e: &Entry| e.kind == Kind::Dir || (e.kind == Kind::Link && e.target == Some("dir"));
        dir_first(b)
            .cmp(&dir_first(a))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let truncated = more || entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(Listing {
        path: shown,
        parent,
        entries,
        total,
        truncated,
    })
}

type Scanned = (Vec<Entry>, usize, bool);

fn scan(dir: &Path) -> Result<Scanned> {
    let md = fs::metadata(dir).map_err(|e| anyhow::anyhow!(why(&e)))?;
    if !md.is_dir() {
        bail!("not a folder");
    }
    let mut entries = Vec::new();
    let mut total = 0;
    for item in fs::read_dir(dir).map_err(|e| anyhow::anyhow!(why(&e)))? {
        if total >= MAX_SCAN {
            return Ok((entries, total, true));
        }
        let Ok(item) = item else { continue };
        total += 1;
        // A name that is not UTF-8 cannot be asked for again by a phone: it is counted and not listed.
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        let Ok(ft) = item.file_type() else { continue };
        let lmeta = item.metadata().ok();
        let (kind, target) = if ft.is_symlink() {
            let target = match fs::metadata(item.path()) {
                Ok(m) if m.is_dir() => "dir",
                Ok(_) => "file",
                Err(_) => "missing",
            };
            (Kind::Link, Some(target))
        } else if ft.is_dir() {
            (Kind::Dir, None)
        } else {
            (Kind::File, None)
        };
        entries.push(Entry {
            md: kind != Kind::Dir && is_md(&name),
            size: (kind == Kind::File)
                .then(|| lmeta.as_ref().map(|m| m.len()))
                .flatten(),
            mtime: lmeta.as_ref().map(|m| m.mtime()),
            name,
            kind,
            target,
        });
    }
    Ok((entries, total, false))
}

/// The text of the file `path`, at most [`MAX_READ`] bytes; Markdown also as blocks. A file that is not a regular
/// file, or that has a NUL near its start, is `Binary` and its bytes are not read past that.
pub fn read(path: &str, cancel: &AtomicBool) -> Result<FileText> {
    let file = resolve(path)?;
    let shown = file.to_string_lossy().into_owned();
    let name_md = file
        .file_name()
        .is_some_and(|n| is_md(&n.to_string_lossy()));
    let work = move || read_file(&file);
    let (size, truncated, bytes) = bounded(cancel, work)?;
    let Some(bytes) = bytes else {
        return Ok(FileText {
            path: shown,
            kind: ReadKind::Binary,
            size,
            truncated: false,
            text: None,
            blocks: None,
        });
    };
    let text = text_of(&bytes, truncated);
    let blocks = name_md.then(|| md::blocks(&text));
    Ok(FileText {
        path: shown,
        kind: if name_md {
            ReadKind::Md
        } else {
            ReadKind::Text
        },
        size,
        truncated,
        text: Some(text),
        blocks,
    })
}

type Raw = (u64, bool, Option<Vec<u8>>);

fn read_file(path: &Path) -> Result<Raw> {
    // Opened without blocking, so a FIFO does not hold the open; what was opened is then checked, not what was named.
    let mut f: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| anyhow::anyhow!(why(&e)))?;
    let meta = f.metadata().map_err(|e| anyhow::anyhow!(why(&e)))?;
    if meta.is_dir() {
        bail!("it is a folder");
    }
    if !meta.is_file() {
        return Ok((0, false, None));
    }
    let size = meta.len();
    let mut buf = Vec::new();
    (&mut f)
        .take(MAX_READ as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| anyhow::anyhow!(why(&e)))?;
    if buf[..buf.len().min(SNIFF)].contains(&0) {
        return Ok((size, false, None));
    }
    let truncated = buf.len() > MAX_READ;
    buf.truncate(MAX_READ);
    Ok((size, truncated, Some(buf)))
}

/// The bytes as text: cut at a character when they were cut, replacement characters for what is not UTF-8.
fn text_of(bytes: &[u8], cut: bool) -> String {
    if cut && let Err(e) = std::str::from_utf8(bytes) {
        // A character split by the cut is dropped, not turned into a replacement.
        if e.error_len().is_none() {
            return String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn no() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_path_must_be_absolute_and_plain() {
        assert!(resolve("relative/dir").is_err());
        assert!(resolve("/a/../b").is_err());
        assert!(resolve("").is_err());
        assert_eq!(resolve("/a//b/./c").unwrap(), PathBuf::from("/a/b/c"));
        let home = std::env::var("HOME").unwrap();
        assert_eq!(resolve("~").unwrap(), PathBuf::from(&home));
        assert_eq!(resolve("~/x").unwrap(), PathBuf::from(&home).join("x"));
    }

    #[test]
    fn a_listing_puts_folders_first_and_says_what_each_is() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir(p.join("zdir")).unwrap();
        fs::create_dir(p.join("Adir")).unwrap();
        fs::write(p.join("b.txt"), "hi").unwrap();
        fs::write(p.join("README.md"), "# x").unwrap();
        fs::write(p.join(".hidden"), "").unwrap();
        symlink(p.join("zdir"), p.join("to-dir")).unwrap();
        symlink(p.join("gone"), p.join("dangling")).unwrap();
        let l = list(p.to_str().unwrap(), &no()).unwrap();
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Adir",
                "to-dir",
                "zdir",
                ".hidden",
                "b.txt",
                "dangling",
                "README.md"
            ]
        );
        let by = |n: &str| l.entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(by("to-dir").kind, Kind::Link);
        assert_eq!(by("to-dir").target, Some("dir"));
        assert_eq!(by("dangling").target, Some("missing"));
        assert_eq!(by("b.txt").size, Some(2));
        assert!(by("README.md").md && !by("b.txt").md);
        assert_eq!(l.total, 7);
        assert!(!l.truncated);
        assert_eq!(l.parent.as_deref(), p.parent().unwrap().to_str());
    }

    #[test]
    fn a_big_directory_is_cut_and_says_how_many() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..MAX_ENTRIES + 30 {
            fs::write(d.path().join(format!("f{i:05}")), "").unwrap();
        }
        let l = list(d.path().to_str().unwrap(), &no()).unwrap();
        assert_eq!(l.entries.len(), MAX_ENTRIES);
        assert_eq!(l.total, MAX_ENTRIES + 30);
        assert!(l.truncated);
    }

    #[test]
    fn what_cannot_be_listed_says_why() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("file");
        fs::write(&f, "x").unwrap();
        assert_eq!(
            list(f.to_str().unwrap(), &no()).unwrap_err().to_string(),
            "not a folder"
        );
        let gone = d.path().join("gone");
        assert_eq!(
            list(gone.to_str().unwrap(), &no()).unwrap_err().to_string(),
            "no such file or folder"
        );
        let locked = d.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        // Running as root reads anything: the check is only for a normal user.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(
                list(locked.to_str().unwrap(), &no())
                    .unwrap_err()
                    .to_string(),
                "permission denied"
            );
        }
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_text_file_is_read_and_markdown_comes_with_its_blocks() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(d.path().join("r.MD"), "# Hi\n\ntext\n").unwrap();
        let t = read(d.path().join("a.txt").to_str().unwrap(), &no()).unwrap();
        assert_eq!(
            (t.kind, t.text.as_deref(), t.blocks.is_none()),
            (ReadKind::Text, Some("one\ntwo\n"), true)
        );
        let m = read(d.path().join("r.MD").to_str().unwrap(), &no()).unwrap();
        assert_eq!(m.kind, ReadKind::Md);
        assert_eq!(m.blocks.unwrap().len(), 2);
    }

    #[test]
    fn a_link_is_followed_when_it_is_named() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("real.txt"), "payload").unwrap();
        symlink(d.path().join("real.txt"), d.path().join("link.txt")).unwrap();
        let r = read(d.path().join("link.txt").to_str().unwrap(), &no()).unwrap();
        assert_eq!(r.text.as_deref(), Some("payload"));
    }

    #[test]
    fn a_file_over_the_limit_is_cut_at_a_character() {
        let d = tempfile::tempdir().unwrap();
        // 'é' is two bytes: a cut at an odd offset would split one.
        let big: String = "é".repeat(MAX_READ);
        fs::write(d.path().join("big.txt"), &big).unwrap();
        let r = read(d.path().join("big.txt").to_str().unwrap(), &no()).unwrap();
        assert!(r.truncated);
        assert_eq!(r.size, big.len() as u64);
        let t = r.text.unwrap();
        assert!(t.len() <= MAX_READ && !t.contains('\u{FFFD}'));
        assert!(t.chars().all(|c| c == 'é'));
    }

    #[test]
    fn binary_files_fifos_and_folders_are_not_text() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("blob"), b"abc\0def").unwrap();
        let b = read(d.path().join("blob").to_str().unwrap(), &no()).unwrap();
        assert_eq!((b.kind, b.text.is_none()), (ReadKind::Binary, true));
        let fifo = d.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t = Instant::now();
        let f = read(fifo.to_str().unwrap(), &no()).unwrap();
        assert_eq!(f.kind, ReadKind::Binary);
        assert!(t.elapsed() < Duration::from_secs(2), "a FIFO held the read");
        assert_eq!(
            read(d.path().to_str().unwrap(), &no())
                .unwrap_err()
                .to_string(),
            "it is a folder"
        );
    }

    #[test]
    fn invalid_utf8_becomes_replacement_characters_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("l1.txt"), b"caf\xe9").unwrap();
        let r = read(d.path().join("l1.txt").to_str().unwrap(), &no()).unwrap();
        assert_eq!(r.text.as_deref(), Some("caf\u{FFFD}"));
    }

    #[test]
    fn a_cancelled_ask_returns_at_once() {
        let d = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        let t = Instant::now();
        assert!(list(d.path().to_str().unwrap(), &cancel).is_err());
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    /// The Android app parses these very files (`android/proto/.../PcFilesTest`), so a field renamed here breaks a test
    /// there and the other way round. `UPDATE_FIXTURES=1 cargo test -p jw-core the_answers` rewrites them.
    #[test]
    fn the_answers_the_phone_parses_are_what_this_serialises() {
        fn check(name: &str, mut got: serde_json::Value, extra: &[(&str, &str)]) {
            for (k, v) in extra {
                got[*k] = (*v).into();
            }
            let path = format!(
                "{}/../../android/proto/src/test/resources/{name}",
                env!("CARGO_MANIFEST_DIR")
            );
            if std::env::var_os("UPDATE_FIXTURES").is_some() {
                std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
            }
            let want: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(got, want, "{name}");
        }
        let entry = |name: &str, kind, size, mtime, target, md| Entry {
            name: name.into(),
            kind,
            size,
            mtime: Some(mtime),
            target,
            md,
        };
        let listing = Listing {
            path: "/Users/me/ws".into(),
            parent: Some("/Users/me".into()),
            entries: vec![
                entry("jw", Kind::Dir, None, 1_760_000_000, None, false),
                entry("to-jw", Kind::Link, None, 1_760_000_001, Some("dir"), false),
                entry(
                    "README.md",
                    Kind::File,
                    Some(1234),
                    1_760_000_002,
                    None,
                    true,
                ),
            ],
            total: 3,
            truncated: false,
        };
        check(
            "phone_ls.json",
            serde_json::to_value(&listing).unwrap(),
            &[("t", "ls"), ("req", "~/ws")],
        );
        let src = "# Title\n\nSome **bold** and `code`, [link](http://x).\n\n- one\n- two\n\n```sh\njw mobile\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let file = FileText {
            path: "/Users/me/ws/README.md".into(),
            kind: ReadKind::Md,
            size: src.len() as u64,
            truncated: false,
            text: Some(src.into()),
            blocks: Some(md::blocks(src)),
        };
        check(
            "phone_read_md.json",
            serde_json::to_value(&file).unwrap(),
            &[("t", "read"), ("req", "~/ws/README.md")],
        );
        let bin = FileText {
            path: "/Users/me/ws/logo.png".into(),
            kind: ReadKind::Binary,
            size: 18_000,
            truncated: false,
            text: None,
            blocks: None,
        };
        check(
            "phone_read_binary.json",
            serde_json::to_value(&bin).unwrap(),
            &[("t", "read"), ("req", "~/ws/logo.png")],
        );
    }
}
