//! The skill files `skill install` writes, with a mark at the end so that a later install refreshes
//! the file jw wrote and leaves the one the person edited (N184, REQ-1007 to REQ-1009).

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::Path;

use super::{Fail, Out, bin};
use crate::version::VERSION;

/// What to do with the file that is already there.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Plan {
    Write,
    Unchanged,
    Keep,
}

/// The first 16 hex digits of the SHA-256: it tells an edit from jw's own text, nothing more (RISK-168).
fn hash(body: &str) -> String {
    web::auth::sha256_hex(body)[..16].to_owned()
}

/// The text with its final newline and the mark under it.
fn marked(text: &str) -> String {
    let body = body_of(text);
    format!("{body}<!-- {} {VERSION} {} -->\n", bin(), hash(&body))
}

fn body_of(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// What is above the last line and the hash that line carries, when that line is a mark.
fn split_mark(file: &str) -> Option<(&str, &str)> {
    let trimmed = file.strip_suffix('\n')?;
    let at = trimmed.rfind('\n')? + 1;
    let line = trimmed[at..]
        .strip_prefix("<!-- ")?
        .strip_prefix(bin())?
        .strip_prefix(' ')?
        .strip_suffix(" -->")?;
    let hash = line.rsplit(' ').next()?;
    Some((&file[..at], hash))
}

pub(super) fn decide(existing: Option<&str>, text: &str) -> Plan {
    let Some(on_disk) = existing else {
        return Plan::Write;
    };
    if on_disk == marked(text) {
        return Plan::Unchanged;
    }
    match split_mark(on_disk) {
        Some((body, h)) if hash(body) == h => Plan::Write,
        None if on_disk == body_of(text) => Plan::Write,
        _ => Plan::Keep,
    }
}

/// Write a file whole, through a rename.
fn put(path: &Path, text: &str) -> Out {
    let dir = path
        .parent()
        .ok_or_else(|| Fail::Error("no folder".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| Fail::Error(format!("{}: {e}", dir.display())))?;
    let mut tmp: OsString = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| Fail::Error(format!("{}: {e}", path.display())))
}

/// What is on disk at `path`: nothing, or its text. A file that cannot be read (not UTF-8, no
/// permission) is not ours to replace, so it reads as text that is not a mark.
pub(super) fn read(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(_) => Some(String::new()),
    }
}

/// Install a skill file by the table of the spec and say what was done.
pub(super) fn install(path: &Path, text: &str, say: &mut dyn FnMut(String)) -> Out {
    let existing = read(path);
    match decide(existing.as_deref(), text) {
        Plan::Write => {
            put(path, &marked(text))?;
            say(format!("installed {}", path.display()));
        }
        Plan::Unchanged => say(format!("unchanged {}", path.display())),
        Plan::Keep => {
            let mut new: OsString = path.as_os_str().to_owned();
            new.push(".new");
            let new = Path::new(&new);
            put(new, &marked(text))?;
            say(format!(
                "kept {}: not written by {} or edited; the new one is {}",
                path.display(),
                bin(),
                new.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "---\nname: x\n---\nbody\n";

    #[test]
    fn the_mark_is_the_last_line_and_carries_the_hash_of_what_is_above() {
        let file = marked(TEXT);
        let (body, h) = split_mark(&file).expect("a mark");
        assert_eq!(body, TEXT);
        assert_eq!(h, hash(TEXT));
        assert!(file.contains(&format!("<!-- {} {VERSION} ", bin())));
        // A text with no final newline gets one before the mark.
        assert_eq!(marked("a"), marked("a\n"));
    }

    #[test]
    fn what_is_not_a_mark_is_not_read_as_one() {
        for file in ["", "a", "a\n", "a\n<!-- other 1 abc -->\n", "<!-- x -->\n"] {
            assert_eq!(split_mark(file), None, "{file:?}");
        }
    }

    #[test]
    fn a_file_jw_wrote_is_refreshed_and_a_file_that_was_edited_is_kept() {
        let mine = marked(TEXT);
        assert_eq!(decide(None, TEXT), Plan::Write);
        assert_eq!(decide(Some(&mine), TEXT), Plan::Unchanged);
        // Written by an older version: same body, another version in the mark.
        let older = mine.replace(VERSION, "0.0.1");
        assert_eq!(decide(Some(&older), TEXT), Plan::Write);
        // Written by an older version with another text and not edited: refreshed.
        assert_eq!(decide(Some(&marked("old\n")), TEXT), Plan::Write);
        // Edited under its mark.
        let edited = mine.replace("body", "my body");
        assert_eq!(decide(Some(&edited), TEXT), Plan::Keep);
        // Edited with the mark cut off.
        assert_eq!(decide(Some("my own skill\n"), TEXT), Plan::Keep);
        assert_eq!(decide(Some(""), TEXT), Plan::Keep);
    }

    #[test]
    fn a_file_from_before_the_mark_is_marked_only_when_its_text_is_the_new_one() {
        assert_eq!(decide(Some(TEXT), TEXT), Plan::Write);
        assert_eq!(decide(Some("old text\n"), TEXT), Plan::Keep);
    }

    #[test]
    fn install_writes_refreshes_keeps_and_leaves_a_new_file_beside_the_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s/SKILL.md");
        install(&path, TEXT, &mut |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), marked(TEXT));
        install(&path, "---\nname: x\n---\nnewer\n", &mut |_| {}).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("newer"));
        std::fs::write(&path, "mine\n").unwrap();
        install(&path, TEXT, &mut |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
        let new = dir.path().join("s/SKILL.md.new");
        assert_eq!(std::fs::read_to_string(new).unwrap(), marked(TEXT));
    }
}
