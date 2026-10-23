//! Artifacts (N171): files an agent publishes for a person to look at on the phone.
//! A workspace's `artifacts` node lists them, and `artifact_read` replies one as an `ArtifactDoc`.

use serde::{Deserialize, Serialize};

use crate::markdown::MarkdownDoc;
use crate::{Head, Id};

/// The most an artifact may weigh, in bytes: it must fit one message of the phone's link (REQ-759).
pub const MAX_ARTIFACT_BYTES: usize = 512 * 1024;

/// What the file is for. Only drawn on the list; it changes nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Prototype,
    Document,
    Report,
    Spec,
}

/// `html` is drawn in an isolated WebView, `md` with the Markdown viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    Html,
    Md,
}

/// What the person decided about one version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    Changes,
}

/// Whether the verdict reached the agent that published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivered {
    Agent,
    #[serde(rename = "none")]
    Nobody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactVerdict {
    #[serde(rename = "type")]
    pub verdict: Verdict,
    /// What to change; only a `changes` verdict has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Unix seconds.
    pub at: u64,
    /// Absent until the daemon has tried to tell the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<Delivered>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    /// From 1; a new version of the same name is the next number.
    pub n: u32,
    /// SHA-256 of the bytes, in lower-case hex.
    pub hash: String,
    pub size: u64,
    /// Unix seconds.
    pub at: u64,
    /// The pane that published, where the verdict goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<ArtifactVerdict>,
}

/// One artifact of a workspace, its versions oldest first. Its status is its latest version's verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactItem {
    pub id: Id,
    pub name: String,
    pub title: String,
    pub kind: ArtifactKind,
    pub format: ArtifactFormat,
    pub versions: Vec<ArtifactVersion>,
}

/// The `artifacts` node (v1) of a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifacts {
    #[serde(flatten)]
    pub head: Head,
    pub workspace: Id,
    pub items: Vec<ArtifactItem>,
}

/// The result of `artifact_read`: `html` for an `html` artifact, `doc` for a `md` one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactDoc {
    pub id: Id,
    pub n: u32,
    pub hash: String,
    pub title: String,
    pub kind: ArtifactKind,
    pub format: ArtifactFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<MarkdownDoc>,
}
