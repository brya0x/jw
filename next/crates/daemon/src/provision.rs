//! What a new worktree gets besides its files (spec REQ-490 to REQ-492): the files of the
//! settings' `copy`, taken from the repository's checkout. A file that cannot be copied is said,
//! never a reason for the worktree to fail, and nothing is ever copied over a file or written
//! outside the worktree.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The most a copied file may weigh.
const MAX_FILE: u64 = 1024 * 1024;

/// What was done with the files of `copy`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Copied {
    pub copied: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

impl Copied {
    /// The fields of the reply of `create_worktree` that say it, when there is something to say.
    pub fn say(&self, reply: &mut Value) {
        if !self.copied.is_empty() {
            reply["copied"] = json!(self.copied);
        }
        if !self.skipped.is_empty() {
            reply["skipped"] = self
                .skipped
                .iter()
                .map(|(path, why)| json!({ "path": path, "why": why }))
                .collect();
        }
    }
}

/// Copy each of `files` (paths relative to `top`) from the checkout `top` into the worktree `dest`.
pub fn copy_files(top: &Path, dest: &Path, files: &[String]) -> Copied {
    let mut out = Copied::default();
    let (Ok(top), Ok(dest)) = (fs::canonicalize(top), fs::canonicalize(dest)) else {
        out.skipped = files
            .iter()
            .map(|f| (f.clone(), "the folders cannot be read".to_owned()))
            .collect();
        return out;
    };
    for file in files {
        match copy_one(&top, &dest, file) {
            Ok(()) => out.copied.push(file.clone()),
            Err(why) => out.skipped.push((file.clone(), why)),
        }
    }
    out
}

fn copy_one(top: &Path, dest: &Path, file: &str) -> Result<(), String> {
    let rel = clean(file).ok_or("it is not a path inside the repository")?;
    let from = top.join(&rel);
    let meta = fs::symlink_metadata(&from).map_err(|_| "it is not in the checkout".to_owned())?;
    if !meta.is_file() {
        return Err("it is a link or not a regular file".into());
    }
    // A folder of the path may be a link that leads out of the checkout.
    let real = fs::canonicalize(&from).map_err(|e| e.to_string())?;
    if !real.starts_with(top) {
        return Err("it leads outside the checkout".into());
    }
    // What is read is what was opened, however the file changes after this: its size is the
    // handle's, and a file that grows is cut at the limit instead of read whole.
    let source = fs::File::open(&real).map_err(|e| e.to_string())?;
    let meta = source.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(format!(
            "it weighs {} KiB: at most {} KiB",
            meta.len() / 1024,
            MAX_FILE / 1024
        ));
    }
    let to = dest.join(&rel);
    let folder = to.parent().ok_or("it has no folder")?;
    // The worktree's own checkout may hold a link where a folder is, and `create_dir_all` would
    // make folders on the other side of it: the deepest folder that exists is looked at first.
    let mut ancestor = folder;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("it has no folder")?;
    }
    if !fs::canonicalize(ancestor).is_ok_and(|f| f.starts_with(dest)) {
        return Err("its folder in the worktree leads outside it".into());
    }
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    // Only the owner's bits: a copy of a secret is not more public than it was.
    let mode = meta.permissions().mode() & 0o700;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode | 0o600)
        .open(&to)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => "it is already in the worktree".to_owned(),
            _ => format!("{}: {e}", to.display()),
        })?;
    let mut bytes = Vec::new();
    source
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE {
        let _ = fs::remove_file(&to);
        return Err(format!(
            "it grew past {} KiB while it was read",
            MAX_FILE / 1024
        ));
    }
    target
        .write_all(&bytes)
        .map_err(|e| format!("{}: {e}", to.display()))
}

/// `file` as a path of plain names, or `None` when it is empty, absolute or goes up.
fn clean(file: &str) -> Option<PathBuf> {
    crate::files::clean_rel(Some(file))
        .ok()
        .filter(|rel| !rel.is_empty())
        .map(PathBuf::from)
}

/// Whether `file` is a path the settings may name for `copy` (REQ-489).
pub fn is_inside(file: &str) -> bool {
    clean(file).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("prov-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (top, dest) = (base.join("top"), base.join("dest"));
        fs::create_dir_all(&top).unwrap();
        fs::create_dir_all(&dest).unwrap();
        (top, dest)
    }

    #[test]
    fn a_file_is_copied_with_its_folders_and_only_its_owners_bits_and_never_over_another() {
        let (top, dest) = scratch("a");
        fs::create_dir_all(top.join("apps/web")).unwrap();
        fs::write(top.join(".env"), "A=1\n").unwrap();
        fs::set_permissions(top.join(".env"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(top.join("apps/web/.env.local"), "B=2\n").unwrap();
        fs::write(top.join("exists"), "new").unwrap();
        fs::write(dest.join("exists"), "old").unwrap();
        let got = copy_files(
            &top,
            &dest,
            &[
                ".env".into(),
                "apps/web/.env.local".into(),
                "exists".into(),
                "nada".into(),
            ],
        );
        assert_eq!(got.copied, [".env", "apps/web/.env.local"]);
        assert_eq!(
            got.skipped,
            [
                (
                    "exists".to_owned(),
                    "it is already in the worktree".to_owned()
                ),
                ("nada".to_owned(), "it is not in the checkout".to_owned()),
            ]
        );
        assert_eq!(fs::read_to_string(dest.join(".env")).unwrap(), "A=1\n");
        assert_eq!(
            fs::read_to_string(dest.join("apps/web/.env.local")).unwrap(),
            "B=2\n"
        );
        assert_eq!(
            fs::read_to_string(dest.join("exists")).unwrap(),
            "old",
            "REQ-491"
        );
        let mode = fs::metadata(dest.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the group and the others lose what they had");
    }

    #[test]
    fn links_big_files_and_paths_that_leave_are_skipped_and_say_why() {
        let (top, dest) = scratch("b");
        let outside = top.parent().unwrap().join("secret");
        fs::write(&outside, "s").unwrap();
        std::os::unix::fs::symlink(&outside, top.join("link")).unwrap();
        fs::create_dir_all(top.join("d")).unwrap();
        std::os::unix::fs::symlink(top.parent().unwrap(), top.join("out")).unwrap();
        fs::write(top.join("big"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        fs::write(top.join("d/ok"), "ok").unwrap();
        fs::create_dir_all(top.join("d/new/sub")).unwrap();
        fs::write(top.join("d/new/sub/f"), "f").unwrap();
        // the worktree's own checkout holds a link where a folder should be
        std::os::unix::fs::symlink(top.parent().unwrap(), dest.join("d")).unwrap();
        let got = copy_files(
            &top,
            &dest,
            &[
                "link".into(),
                "big".into(),
                "../secret".into(),
                "/etc/passwd".into(),
                "out/secret".into(),
                "d/ok".into(),
                "d/new/sub/f".into(),
                "".into(),
            ],
        );
        assert!(got.copied.is_empty(), "{got:?}");
        let why: Vec<&str> = got.skipped.iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(why[0], "it is a link or not a regular file");
        assert!(why[1].starts_with("it weighs 1024 KiB"), "{}", why[1]);
        assert_eq!(why[2], "it is not a path inside the repository");
        assert_eq!(why[3], "it is not a path inside the repository");
        assert_eq!(why[4], "it leads outside the checkout");
        assert_eq!(why[5], "its folder in the worktree leads outside it");
        assert_eq!(why[6], "its folder in the worktree leads outside it");
        assert!(
            !top.parent().unwrap().join("new").exists(),
            "not even the folders of a path that leads out are made on the other side of the link"
        );
        assert!(
            !top.parent().unwrap().join("ok").exists(),
            "nothing was written outside the worktree"
        );
    }

    #[test]
    fn the_reply_says_only_what_there_is_to_say() {
        let mut r = json!({ "workspace": "w" });
        Copied::default().say(&mut r);
        assert_eq!(r, json!({ "workspace": "w" }));
        Copied {
            copied: vec![".env".into()],
            skipped: vec![("x".into(), "why".into())],
        }
        .say(&mut r);
        assert_eq!(r["copied"], json!([".env"]));
        assert_eq!(r["skipped"], json!([{ "path": "x", "why": "why" }]));
        assert!(
            is_inside(".env")
                && is_inside("a/./b")
                && !is_inside("../a")
                && !is_inside("/a")
                && !is_inside("")
        );
    }
}
