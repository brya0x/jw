//! A text file as lines of runs (spec *The file viewer*, REQ-610 to REQ-624): what `read_file`
//! replies. A run is a piece of a line with, when the daemon knows the language, the syntax class
//! it has; a client draws the class with its theme's accent and parses nothing (RAT-65, RAT-67).

use serde::{Deserialize, Serialize};

/// A file: its name, where it is, how big it is and its lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDoc {
    /// The name of the file.
    pub title: String,
    /// The canonical path of the file, for a client that hands it to an editor.
    pub path: String,
    /// What the file weighs, in bytes.
    pub size: u64,
    /// The file was longer than the daemon reads, and the lines end where it cut, at a line end.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
    pub lines: Vec<Vec<Run>>,
}

/// A piece of a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub t: String,
    /// The syntax class: `kw`, `str`, `num`, `ty`, `fn`, `cm` or `attr`. A client shows a run
    /// with no class, or with one it does not know, as plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}
