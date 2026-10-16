//! The JSON documents shared by the daemon and every client. The schema in
//! `contract/schema` and the fixtures in `contract/fixtures` are the contract;
//! these types are the Rust side of it and `tests/conformance.rs` keeps them honest.
//!
//! Receivers ignore unknown fields (no `deny_unknown_fields`): additive changes never
//! break an old peer. Optional fields are skipped when absent so a round trip through
//! these types reproduces the document exactly.

pub mod conversation;
pub mod link;
pub use link::{BridgeMsg, Denied, PhoneMsg};
pub mod patch;
pub mod themes;

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
    /// Boxed so that the message that is not a document stays small.
    Snapshot { node: Box<Node> },
    Patch {
        node: Id,
        epoch: String,
        from: u64,
        to: u64,
        ops: Vec<PatchOp>,
    },
    /// The node was removed, or never existed.
    Gone { node: Id },
    Reply {
        id: String,
        #[serde(flatten)]
        outcome: Outcome,
    },
    /// A message the daemon could not act on that has no `id` to reply to.
    Error { code: ErrorCode, message: String },
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
    /// Read the subfolders of `path` (the daemon's home when none), for a client choosing a
    /// folder to open. The result is `{path, parent?, home, dirs: [{name, git}], total}`.
    ListDir {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Also list the folders whose names start with a dot.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hidden: bool,
    },
    /// Make a git worktree of `repo` on `branch` and open it as a workspace.
    CreateWorktree {
        repo: String,
        branch: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        /// The name of the worktree's folder inside `<repo>-wt/`; the branch's name with
        /// its slashes made dashes when none is given.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// Close a workspace (a worktree is removed from disk only when `remove_worktree`).
    RemoveWorkspace {
        workspace: Id,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        remove_worktree: bool,
        /// With `remove_worktree`: take the folder away even with changes not committed
        /// (`git worktree remove --force`). The branch stays either way.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Count the commits of a worktree's branch that no remote has. The result is `{ahead}`.
    CheckWorkspace {
        workspace: Id,
    },
    /// Give a workspace a name to be shown by; an empty name brings back the one it had
    /// (the folder's name, or `repo: branch` for a worktree). Its folder is not touched.
    RenameWorkspace {
        workspace: Id,
        name: String,
    },
    /// Push a worktree's branch to its remote (`git push`, never forced, nothing merged). Only a
    /// worktree has a branch of its own to push. The result is `{branch, pushed, new_branch}`:
    /// the commits that went up, and whether the branch was made on the remote.
    SyncWorkspace {
        workspace: Id,
    },
    /// Swap the places of two panes of a workspace in its layout.
    SwapPanes {
        a: Id,
        b: Id,
    },
    /// Make a pane wider (`row`) or taller (`col`) by moving the nearest split of that direction
    /// around it, `by` thousandths of the split's size; a negative `by` makes it narrower or lower.
    GrowPane {
        pane: Id,
        dir: SplitDir,
        by: i16,
    },
    /// Give a pane a name that stays whatever runs in it; an empty name brings back the
    /// automatic one (what runs in it, else its shell).
    RenamePane {
        pane: Id,
        name: String,
    },
    /// List one folder of a workspace: `path` is relative to the workspace's folder (the folder
    /// itself when absent). The result is a `FileListing`.
    ListFiles {
        workspace: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// Find files of a workspace whose path holds `query` (letters in order, any case), the tracked
    /// and the untracked ones that git does not ignore. The result is a `FoundFiles`.
    FindFiles {
        workspace: Id,
        query: String,
    },
    /// Read the settings of the repository `repo` is in (its `.jw.toml`, with the defaults
    /// for what it does not say). The result is a `ProjectInfo`.
    ProjectConfig {
        repo: String,
    },
    /// Start the panes of a workspace that has none: a worktree's from its repository's
    /// `.jw.toml` (the layout and, in a pane of its own, the setup), any other's one shell.
    /// The result is `{panes: [id]}`, in the order of the layout.
    StartWorkspace {
        workspace: Id,
    },
    /// Open a pane: the user's shell, with `program` run inside it when given (the shell
    /// starts it, and is still there when it ends).
    NewPane {
        workspace: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<Vec<String>>,
        /// Split this pane of the workspace to make room for the new one, along `dir` (a row
        /// when absent). Without it the pane goes after the last one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        beside: Option<Id>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir: Option<SplitDir>,
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
    /// An agent (claude) in `pane` reported what it is doing, through one of its hooks; the
    /// daemon turns it into attention (or ends it).
    AgentEvent {
        pane: Id,
        event: AgentEvent,
        /// What a `notification` is about (`permission_prompt`, `elicitation_dialog`, …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notification_type: Option<String>,
        /// The tool a `pre_tool_use` or `post_tool_use` is about.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
    },
    /// Turn the phone link on (bridge, tunnel, private-address listener) and make a pairing
    /// token. The result is `{uri, tunnel?, lan?, pending}`; the phone is paired from `uri`.
    /// With a private address it answers at once and the tunnel opens in the background
    /// (`pending`); `mobile_link` asks how it went.
    MobileStart {},
    /// The link as it is now, with the pairing token `mobile_start` made (a new one only when
    /// that one is gone): the result is `{uri, tunnel?, lan?, pending}`. `pending` is true
    /// while the tunnel is still opening, so a client asks again until it is not.
    MobileLink {},
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
    /// Open the conversation of `pane` (the pane runs `claude`): a new one, or the one of
    /// `resume`. `args` are the flags `claude` was typed with that the engine serves; `env`
    /// is the environment it was typed in, which the engine runs with. The result is
    /// `{node, session}`.
    OpenConversation {
        pane: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<std::collections::BTreeMap<String, String>>,
    },
    /// Say something to Claude, or run a command of the `/` list.
    ConversationSend {
        node: Id,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<conversation::Image>,
    },
    /// Answer the `ask` that waits: `key` is one of its options' keys; `text` goes with a
    /// refusal, as what to do instead; `by` says where the answer comes from (`el móvil`) and
    /// is written on the tool's line.
    ConversationAnswer {
        node: Id,
        ask: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// Stop the turn that runs.
    ConversationInterrupt {
        node: Id,
    },
    /// Change what the conversation is called, its colour, the model, the effort or the mode.
    ConversationSet {
        node: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accent: Option<conversation::Accent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<conversation::Mode>,
    },
    /// Run a shell line of the person's (`!command`) in the pane's folder.
    ConversationShell {
        node: Id,
        command: String,
    },
    /// End the conversation (`/exit`): the engine stops and the pane is the shell again.
    ConversationClose {
        node: Id,
    },
    /// The conversations Claude saved for the workspace's folder, newest first, up to 50 (REQ-426).
    /// The result is a `ConversationList`.
    ListConversations {
        workspace: Id,
    },
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
    /// Boxed: it is by far the largest document and the others are not made larger by it.
    Conversation(Box<conversation::Conversation>),
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
    /// The repository a worktree belongs to, or the folder's own path when it is a repository's root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub panes: Vec<PaneSummary>,
    /// How the panes are laid out: a tree whose leaves are exactly `panes`. Absent from a
    /// daemon that does not keep one: the client lays them out itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
}

/// Which way a split puts its two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDir {
    /// Side by side: `a` on the left, `b` on the right.
    Row,
    /// One over the other: `a` above, `b` below.
    Col,
}

/// What an entry of a folder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Dir,
    File,
    /// A symbolic link, whatever it points at: it is not followed.
    Link,
}

/// What git says of an entry; of a folder, the most serious of what is under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitMark {
    Conflict,
    Modified,
    Deleted,
    Added,
    Untracked,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub kind: FileKind,
    /// Bytes, for a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMark>,
    /// Git ignores it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

/// The result of `list_files`: folders first, then files, each by name; `entries` is cut to the
/// first ones when `total` is more than they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub total: usize,
}

/// The result of `project_config`: what a worktree of the repository will be. `panes` are the
/// programs of the layout's panes (the `setup` pane is not among them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// The `.jw.toml` read; none when the repository has no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The folder worktrees go in.
    pub root: String,
    /// The template of a worktree's branch; clients replace `{name}`.
    pub branch: String,
    /// The setup commands as written.
    pub setup: Vec<String>,
    pub panes: Vec<String>,
}

/// The result of `find_files`: paths relative to the workspace's folder, cut when `cut`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundFiles {
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

/// The panes of a workspace as a tree of splits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Layout {
    Pane {
        pane: Id,
    },
    /// `ratio` is the share of `a`, in thousandths of the split's width or height.
    Split {
        dir: SplitDir,
        ratio: u16,
        a: Box<Layout>,
        b: Box<Layout>,
    },
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
    /// What the person has typed at the shell's prompt; absent while a command runs and for a
    /// shell started without the integration.
    /// Boxed so the node stays small: a snapshot is the largest message and is as large as this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Box<PaneInput>>,
    /// The node of this pane's conversation while one is open: the pane runs `claude` and the
    /// clients draw what it says in place of the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Id>,
}

/// The line the shell is holding: its text, wrapped rows joined, and where the cursor is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInput {
    pub text: String,
    /// The cursor's place in `text`, counted in characters.
    pub cursor: u32,
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
    /// The daemon restarted while it ran: its end was never seen, and `ms` is how long it had
    /// run when the daemon last saved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// Not a command: the place where the daemon restarted, `started` being when. What is before
    /// it was kept from before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart: bool,
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

/// The hooks of an agent that the daemon listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    Notification,
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Stop,
    SessionEnd,
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
    /// The repository a worktree belongs to, or the folder's own path when it is a repository's root: where a new worktree of it is made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
    /// The program is on the alternate screen (nvim, htop, claude…); absent otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
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
        Node::Conversation(n) => &n.head.id,
    }
}
