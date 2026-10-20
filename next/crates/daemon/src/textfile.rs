//! A text file of the workspace as lines (spec REQ-610 to REQ-614, RAT-63, RAT-64): read once,
//! at most a MiB of it, and handed to every client as the same `TextDoc`. Nothing here changes a
//! file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use contract::ErrorCode;
use contract::text::TextDoc;

use crate::commands::{Done, ErrorBody, err};
use crate::files;
use crate::highlight;
use crate::server::Shared;

/// The most of a file that is read: a document is one reply.
const MAX_FILE: usize = 1024 * 1024;
/// How much of the head is looked at for a NUL byte, as the diff does.
const SNIFF: usize = 8000;

/// `read_file`: the lines of a text file of the workspace (REQ-610).
pub fn read(shared: &Shared, workspace: &str, path: &str) -> Done {
    let root = shared
        .lock()
        .workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
    let file = files::inside(&root, path)?;
    let (bytes, size, truncated) = load(&file, path)?;
    let text = utf8(bytes, truncated, path)?;
    let title = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let lines = highlight::runs(&text, &title);
    let doc = TextDoc {
        title,
        path: file.to_string_lossy().into_owned(),
        size,
        truncated,
        lines,
    };
    Ok(Some(
        serde_json::to_value(doc).expect("a document serialises"),
    ))
}

/// The head of the file, its size and whether there is more than was read. The handle that is
/// read is the one that was checked, so a file swapped for a link in between is not followed.
fn load(file: &Path, path: &str) -> Result<(Vec<u8>, u64, bool), ErrorBody> {
    let io = |e: std::io::Error| err(ErrorCode::Io, format!("cannot read {path}: {e}"));
    let handle = File::open(file).map_err(io)?;
    let meta = handle.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(err(ErrorCode::BadRequest, format!("{path} is not a file")));
    }
    let mut bytes = Vec::new();
    handle
        .take(MAX_FILE as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.iter().take(SNIFF).any(|b| *b == 0) {
        return Err(binary(path));
    }
    let truncated = meta.len() > MAX_FILE as u64;
    Ok((bytes, meta.len(), truncated))
}

fn binary(path: &str) -> ErrorBody {
    err(
        ErrorCode::Binary,
        format!("{path} is not text: the viewer reads UTF-8 without NUL bytes"),
    )
}

/// The text of the head: a file that was cut ends at its last line end, and one with no line end
/// ends at the last whole character.
fn utf8(mut bytes: Vec<u8>, truncated: bool, path: &str) -> Result<String, ErrorBody> {
    if truncated && let Some(end) = bytes.iter().rposition(|b| *b == b'\n') {
        bytes.truncate(end + 1);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(e) if truncated && e.utf8_error().error_len().is_none() => {
            let end = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(end);
            Ok(String::from_utf8(bytes).expect("up to the last whole character"))
        }
        Err(_) => Err(binary(path)),
    }
}
