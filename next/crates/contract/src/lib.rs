//! The JSON documents shared by the daemon and every client. The schema in
//! `contract/schema` and the fixtures in `contract/fixtures` are the contract;
//! these types are the Rust side of it and `tests/conformance.rs` keeps them honest.
//!
//! Receivers ignore unknown fields (no `deny_unknown_fields`): additive changes never
//! break an old peer. Optional fields are skipped when absent so a round trip through
//! these types reproduces the document exactly.

pub mod link;
pub use link::{BridgeMsg, Denied, PhoneMsg};
pub mod patch;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A node id: a lowercase uuid.
pub type Id = String;

/// What a client sends. One JSON object per line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        client: Peer,
        /// Node type -> the versions this client can read.
        accepts: std::collections::BTreeMap<String, Vec<u32>>,
    },
    Open {
        node: Id,
        /// Resume from a cached copy: honoured only when `epoch` is the daemon's.
        #[serde(skip_serializing_if = "Option::is_none")]
        epoch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
    },
    Close {
        node: Id,
    },
    Cmd {
        id: String,
        cmd: Cmd,
    },
}

/// What the daemon sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DaemonMsg {
    Hello {
        daemon: Peer,
        epoch: String,
        /// The id of the session node, the root of the tree.
        root: Id,
        /// Node type -> the version the daemon will serve this client. A type missing
        /// here shares no version with the client and is not served.
        accepted: std::collections::BTreeMap<String, u32>,
    },
    Snapshot {
        node: Node,
    },
    Patch {
        node: Id,
        epoch: String,
        from: u64,
        to: u64,
        ops: Vec<PatchOp>,
    },
    /// The node was removed, or never existed.
    Gone {
        node: Id,
    },
    Reply {
        id: String,
        #[serde(flatten)]
        outcome: Outcome,
    },
    /// A message the daemon could not act on that has no `id` to reply to.
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Outcome {
    Ok {
        ok: True,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    Err {
        ok: False,
        error: ErrorBody,
    },
}

/// `true` as a type, so `{"ok": true}` and `{"ok": false}` pick the right variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct True;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct False;

impl Serialize for True {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(true)
    }
}
impl Serialize for False {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(false)
    }
}
impl<'de> Deserialize<'de> for True {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match bool::deserialize(d)? {
            true => Ok(True),
            false => Err(serde::de::Error::custom("expected true")),
        }
    }
}
impl<'de> Deserialize<'de> for False {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match bool::deserialize(d)? {
            false => Ok(False),
            true => Err(serde::de::Error::custom("expected false")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Unsupported,
    Conflict,
    GitFailed,
    Io,
}

/// A client action. The daemon answers with a `reply` carrying the same `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Rename the session.
    RenameSession {
        name: String,
    },
    /// Open a folder as a workspace.
    OpenFolder {
        path: String,
    },
    /// Make a git worktree of `repo` on `branch` and open it as a workspace.
    CreateWorktree {
        repo: String,
        branch: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// Close a workspace (a worktree is removed from disk only when `remove_worktree`).
    RemoveWorkspace {
        workspace: Id,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        remove_worktree: bool,
    },
    /// Open a pane: the user's shell, with `program` run inside it when given (the shell
    /// starts it, and is still there when it ends).
    NewPane {
        workspace: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<Vec<String>>,
    },
    ClosePane {
        pane: Id,
    },
    /// Type into a pane: raw text as the terminal would receive it.
    Write {
        pane: Id,
        data: String,
    },
    Resize {
        pane: Id,
        rows: u16,
        cols: u16,
    },
    /// A person looked at `node` at revision `rev`: the daemon ends the attentions it raised
    /// up to that revision, for every client.
    MarkSeen {
        node: Id,
        rev: u64,
    },
    /// Make (or find) the `diff` node of `workspace` for `scope`; `commit` names the commit of
    /// the `commit` scope. The result is `{node}`.
    OpenDiff {
        workspace: Id,
        scope: DiffScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    /// Make (or find) the `commits` node of `workspace`. The result is `{node}`.
    OpenCommits {
        workspace: Id,
    },
    /// Read a `diff` node again now.
    RefreshDiff {
        node: Id,
    },
    /// Record that `path` of a `diff` node was looked at (or forget it), for every client.
    SetViewed {
        node: Id,
        path: String,
        viewed: bool,
    },
    /// Turn the phone link on (bridge, tunnel, private-address listener) and make a pairing
    /// token. The result is `{uri, tunnel?, lan?}`; the phone is paired from `uri`.
    MobileStart {},
    /// Wait up to `seconds` for a phone to ask to pair. The result is `{name, code}`, or
    /// nothing when none asked.
    MobileWait {
        seconds: u32,
    },
    /// Answer the pairing question `mobile_wait` returned.
    MobileAnswer {
        yes: bool,
    },
    /// Turn the phone link off; connected phones are dropped, paired devices are kept.
    MobileStop {},
}

/// A node: one document of the tree, opened on demand and cached by clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Session(Session),
    Workspace(Workspace),
    Pane(Pane),
    Blocks(Blocks),
    Diff(Diff),
    Commits(Commits),
}

/// Fields every node carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub id: Id,
    /// The version of this node type's document.
    pub v: u32,
    /// Changes whenever the daemon starts: a cache from another epoch points at nothing.
    pub epoch: String,
    pub rev: u64,
    /// Everything in this node's subtree that wants a person, computed by the daemon.
    pub attention: Vec<Attention>,
}

/// Level 0: the workspaces of the session, each with a summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(flatten)]
    pub head: Head,
    pub name: String,
    pub workspaces: Vec<WorkspaceSummary>,
}

/// Level 1: one workspace and its panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(flatten)]
    pub head: Head,
    pub name: String,
    pub kind: WorkspaceKind,
    pub path: String,
    /// The repository a worktree belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub panes: Vec<PaneSummary>,
}

/// Level 2: one pane, a terminal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    #[serde(flatten)]
    pub head: Head,
    pub workspace: Id,
    pub title: String,
    /// The user's shell, always the pane's process.
    pub shell: String,
    pub cwd: String,
    /// The program running inside the shell now, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<Program>,
    /// The program that ran last, kept so a restart can bring it back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_program: Option<Program>,
    pub screen: Screen,
    /// The node of this pane's shell blocks; present for a shell started with the integration
    /// (zsh, bash, fish), from the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Id>,
}

/// Level 2 as command blocks: what a pane's shell ran, newest last, each with its output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blocks {
    #[serde(flatten)]
    pub head: Head,
    pub pane: Id,
    pub mode: BlocksMode,
    pub blocks: Vec<Block>,
}

/// `full` while a program has the whole screen (nvim): clients draw the pane's screen instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlocksMode {
    Blocks,
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    /// Counts up per pane and is never reused.
    pub id: u64,
    pub cmd: String,
    /// Where the command started.
    pub cwd: String,
    /// Unix milliseconds.
    pub started: u64,
    pub running: bool,
    /// The exit status; absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// How long it took, in milliseconds; absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// The start of the output was dropped to stay within the limits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The command used the whole screen; its output is not kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full: bool,
    /// The output as rows of styled spans: the daemon resolves colours and attributes.
    pub out: Vec<Vec<Span>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub b: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub i: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub u: bool,
}

/// A palette index (0 to 255) or a `#rrggbb` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color {
    Index(u8),
    Rgb(String),
}

/// What a workspace's changes are: its files, hunk by hunk, against a scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    #[serde(flatten)]
    pub head: Head,
    pub workspace: Id,
    pub scope: DiffScope,
    /// The commit of the `commit` scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// What `general` compares with, such as `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub state: DiffState,
    /// Over 2 MiB: the files are listed with no hunks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub too_large: bool,
    /// Files left out of a too large diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<u32>,
    pub files: Vec<DiffFile>,
}

/// The commits of a workspace's branch, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Commits {
    #[serde(flatten)]
    pub head: Head,
    pub workspace: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Older commits exist than the ones listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
    pub commits: Vec<CommitInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffScope {
    Uncommitted,
    General,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffState {
    Ok,
    /// `origin` has no default branch: a `general` diff has nothing to compare with.
    NoBase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seen {
    /// Looked at, and unchanged since.
    Yes,
    /// Looked at, but the file differs now.
    Changed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: FileStatus,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    /// Why the daemon did not read the file (`symlink`, `too_big`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
    pub added: u32,
    pub deleted: u32,
    /// Fingerprint of the path, status and lines: a mark of having looked at the file is
    /// only good while this stays the same.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<Seen>,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hunk {
    /// What git prints after the second `@@`, usually the enclosing function.
    pub context: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Each line prefixed with `+` (added), `-` (deleted) or a space (unchanged).
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub short: String,
    pub subject: String,
    /// Commit time in Unix seconds.
    pub time: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Worktree,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: Id,
    pub name: String,
    pub kind: WorkspaceKind,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub pane_count: u32,
    pub attention: Vec<Attention>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSummary {
    pub id: Id,
    pub title: String,
    pub attention: Vec<Attention>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub argv: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub rows: u16,
    pub cols: u16,
    /// Exactly `rows` lines of text, trailing blanks trimmed.
    pub lines: Vec<String>,
    pub cursor: Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// Something that wants a person. A type on a node that bubbles to its parents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attention {
    pub kind: AttentionKind,
    /// The node that opens when this is tapped.
    pub node: Id,
    /// The revision of `node` that raised it. A client that has seen `node` at this
    /// revision or later treats the attention as seen; the daemon never learns that.
    pub rev: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Question,
    Plan,
    Permission,
    ShellDone,
    AgentDone,
    Screen,
    Merged,
    CiFailed,
}

/// One edit of a node document: the RFC 6902 subset `add`, `remove`, `replace`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp {
    Add { path: String, value: Value },
    Remove { path: String },
    Replace { path: String, value: Value },
}

pub fn node_id(node: &Node) -> &Id {
    match node {
        Node::Session(n) => &n.head.id,
        Node::Workspace(n) => &n.head.id,
        Node::Pane(n) => &n.head.id,
        Node::Blocks(n) => &n.head.id,
        Node::Diff(n) => &n.head.id,
        Node::Commits(n) => &n.head.id,
    }
}
