//! The messages sent from a repository, kept for ↑ and ^R in the composer (spec REQ-716): one file
//! for each repository in `<state>/sent/`, the last hundred, which lasts across conversations as a
//! shell's history does. It holds what the person typed and pasted, so it is private (RISK-89).

use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use contract::conversation::{SentList, SentMessage};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::Done;
use crate::server::Shared;

/// The file's version, the messages it keeps and the most bytes of one.
const VERSION: u32 = 1;
const KEEP: usize = 100;
const MAX_BYTES: usize = 20 * 1024;

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    entries: Vec<SentMessage>,
}

/// Where the files are: `sent`, not `history`, which holds the panes' (`history.rs`).
pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("sent")
}

/// The file of a repository, none for a key that is not a plain name.
fn path(dir: &Path, key: &str) -> Option<PathBuf> {
    let plain = !key.is_empty()
        && !key.starts_with('.')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    plain.then(|| dir.join(format!("{key}.json")))
}

/// Whether a message is kept: not blank, not private and not over 20 KB (REQ-716).
pub fn keeps(text: &str, private: bool) -> bool {
    !private && !text.trim().is_empty() && text.len() <= MAX_BYTES
}

/// The messages of a repository, oldest first. A file that cannot be read is moved to
/// `<key>.json.bad` and not overwritten, as `history::load` does.
pub fn load(dir: &Path, key: &str) -> Vec<SentMessage> {
    let Some(file) = path(dir, key) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    match serde_json::from_str::<File>(&text) {
        Ok(f) if f.v == VERSION => f.entries,
        _ => {
            let bad = file.with_extension("json.bad");
            eprintln!(
                "sent: {} cannot be read; moved to {}",
                file.display(),
                bad.display()
            );
            let _ = std::fs::rename(&file, bad);
            Vec::new()
        }
    }
}

/// The spans that fall inside the text, in order and apart: what a client says of a message it
/// sent is not trusted to be right.
fn spans(text: &str, folds: &[[usize; 2]]) -> Vec<[usize; 2]> {
    let len = text.chars().count();
    let mut at = 0;
    let mut out = Vec::new();
    for &[from, to] in folds {
        if from >= at && from < to && to <= len {
            out.push([from, to]);
            at = to;
        }
    }
    out
}

/// One at a time: two conversations of a repository add to the same file.
static WRITING: Mutex<()> = Mutex::new(());

/// Add a message to the file of `key`, when it is one that is kept and is not the last one kept.
pub fn append(
    dir: &Path,
    key: &str,
    text: &str,
    folds: &[[usize; 2]],
    private: bool,
) -> io::Result<()> {
    let (Some(file), true) = (path(dir, key), keeps(text, private)) else {
        return Ok(());
    };
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load(dir, key);
    if entries.last().is_some_and(|last| last.text == text) {
        return Ok(());
    }
    entries.push(SentMessage {
        text: text.to_owned(),
        folds: spans(text, folds),
    });
    let from = entries.len().saturating_sub(KEEP);
    entries.drain(..from);
    let json = serde_json::to_string(&File {
        v: VERSION,
        entries,
    })
    .map_err(io::Error::other)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = file.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    out.write_all(json.as_bytes())?;
    out.sync_all()?;
    std::fs::rename(tmp, file)
}

/// The repository of a folder as the key of its file: the personal file's (`project::key`), of
/// the main folder when the folder is a worktree. Asked once for each folder: it runs `git`.
fn key_of(folder: &Path) -> String {
    static KEYS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let keys = KEYS.get_or_init(Mutex::default);
    if let Some(key) = keys.lock().unwrap_or_else(|e| e.into_inner()).get(folder) {
        return key.clone();
    }
    let key = crate::project::repo_key(folder);
    keys.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(folder.to_path_buf(), key.clone());
    key
}

/// Where the file of the conversation `node` is: the directory and the key of its folder's
/// repository. None when the daemon has no state directory or there is no such conversation.
fn file_of(shared: &Shared, node: &str) -> Option<(PathBuf, String)> {
    let (state, folder) = {
        let core = shared.lock();
        let conv = core.convs.values().find(|c| c.id == node)?;
        (core.state_dir.clone()?, conv.cwd.clone())
    };
    Some((dir(&state), key_of(&folder)))
}

/// A message was sent in the conversation `node` (REQ-716).
pub fn record(shared: &Shared, node: &str, text: &str, folds: &[[usize; 2]], private: bool) {
    if !keeps(text, private) {
        return;
    }
    if let Some((dir, key)) = file_of(shared, node)
        && let Err(e) = append(&dir, &key, text, folds, private)
    {
        eprintln!("sent: {key} not written: {e}");
    }
}

/// `conversation_history`: the messages of the conversation's repository, oldest first.
pub fn history(shared: &Shared, node: &str) -> Done {
    let messages = file_of(shared, node).map_or_else(Vec::new, |(dir, key)| load(&dir, &key));
    Ok(Some(json!(SentList { messages })))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sent-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d.join("sent")
    }

    fn texts(dir: &Path, key: &str) -> Vec<String> {
        load(dir, key).into_iter().map(|m| m.text).collect()
    }

    #[test]
    fn a_message_is_kept_unless_blank_private_big_or_the_last_one() {
        assert!(keeps("haz un commit", false));
        assert!(!keeps("  \n", false));
        assert!(!keeps("secreto", true));
        assert!(keeps(&"x".repeat(20 * 1024), false));
        assert!(!keeps(&"x".repeat(20 * 1024 + 1), false));
        let d = dir();
        for (text, private) in [
            ("uno", false),
            ("uno", false),
            ("dos", true),
            ("tres", false),
            ("uno", false),
        ] {
            append(&d, "github.com_a_b", text, &[], private).unwrap();
        }
        assert_eq!(texts(&d, "github.com_a_b"), ["uno", "tres", "uno"]);
    }

    #[test]
    fn only_the_last_hundred_stay_oldest_first() {
        let d = dir();
        for n in 0..130 {
            append(&d, "k", &format!("m{n}"), &[], false).unwrap();
        }
        let all = texts(&d, "k");
        assert_eq!(
            (all.len(), all[0].as_str(), all[99].as_str()),
            (100, "m30", "m129")
        );
    }

    #[test]
    fn the_folds_are_kept_when_they_fall_in_the_text_and_apart() {
        let d = dir();
        append(
            &d,
            "k",
            "ab\ncd efg",
            &[[2, 5], [4, 6], [6, 99], [7, 9]],
            false,
        )
        .unwrap();
        assert_eq!(load(&d, "k")[0].folds, vec![[2, 5], [7, 9]]);
    }

    #[test]
    fn the_file_is_private_and_a_bad_one_is_set_aside() {
        let d = dir();
        append(&d, "k", "uno", &[], false).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&d.join("k.json")), 0o600);
        std::fs::write(d.join("k.json"), "{no").unwrap();
        assert!(load(&d, "k").is_empty());
        assert!(d.join("k.json.bad").exists() && !d.join("k.json").exists());
        append(&d, "k", "dos", &[], false).unwrap();
        assert_eq!(texts(&d, "k"), ["dos"]);
    }

    #[test]
    fn a_key_that_is_not_a_plain_name_names_no_file() {
        let d = dir();
        for key in ["", "..", ".x", "a/b", "a b"] {
            append(&d, key, "uno", &[], false).unwrap();
            assert!(load(&d, key).is_empty(), "{key:?}");
        }
        assert!(!d.exists(), "nothing was written");
    }
}
