//! A Markdown document as blocks (spec *The Markdown viewer*, REQ-465 to REQ-468): what
//! `read_markdown` and `read_plan` reply. The daemon reads the Markdown once, and every client
//! draws these blocks its own way and parses nothing (RAT-45).

use serde::{Deserialize, Serialize};

/// A document: its title, the file it is when it is one, and its blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownDoc {
    /// The first heading, else the name of the file.
    pub title: String,
    /// The file it was read from; for a plan, the plan's file when the engine said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The text was longer than the daemon reads, and ends where the limit was.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
    pub blocks: Vec<MdBlock>,
}

/// A piece of the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum MdBlock {
    Heading {
        level: u8,
        text: Vec<Inline>,
    },
    Para {
        text: Vec<Inline>,
    },
    Quote {
        blocks: Vec<MdBlock>,
    },
    List {
        /// The number of the first item when the list is numbered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<u32>,
        items: Vec<MdItem>,
    },
    Code {
        /// What follows the fence (`rust`), empty when nothing does.
        lang: String,
        text: String,
    },
    Table {
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
}

/// An item of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdItem {
    /// `Some` when it has a checkbox: whether it is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<bool>,
    pub blocks: Vec<MdBlock>,
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inline {
    pub t: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub b: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub i: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub code: bool,
    /// Where it links to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}
