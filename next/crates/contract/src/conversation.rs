//! The `conversation` node: what a pane that runs `claude` is, as a document every client draws
//! its own way (spec: *The conversation*, REQ-371 and following; what the engine says is in
//! `docs/claude-engine.md`). Money is whole micro-dollars and never a float, so that a document
//! round-trips byte for byte in every language.

use serde::{Deserialize, Serialize};

use crate::{Head, Id};

/// A pane's conversation with the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    #[serde(flatten)]
    pub head: Head,
    pub pane: Id,
    /// The engine's id for this conversation; what `--resume` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub name: String,
    pub accent: Accent,
    pub model: String,
    pub effort: String,
    pub mode: Mode,
    pub status: ConvStatus,
    /// Milliseconds since the epoch at which the turn that runs began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// How many items came before `items[0]`: the node keeps the last 300.
    pub first: u32,
    pub items: Vec<Item>,
    /// What waits for the person, at most one at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The messages that wait for the turn that runs to end.
    pub queue: Vec<String>,
    pub tasks: Vec<Task>,
    pub todos: Vec<Todo>,
    pub artifacts: Vec<Artifact>,
    pub commands: Vec<ConvCommand>,
    /// What the conversation has cost, in millionths of a dollar.
    pub cost: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// The tools that run, and when each began (REQ-562): a client counts the time itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<Running>,
    /// The time of the first item of `items` when the conversation has a transcript that may hold
    /// older ones: the `before` of `read_older` (REQ-567).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older: Option<u64>,
    /// Another `claude` that has this session open, while the conversation has no engine of its
    /// own (REQ-575): the clients read it and do not write in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
    /// What Claude proposes the person writes next, while the engine is stopped and nothing waits
    /// (REQ-572): the clients draw it grey in the empty field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// When the turn began that another `claude` is running in the transcript this conversation
    /// follows, while its lines say one is open (REQ-565).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub following: Option<u64>,
    /// What Claude has written in the turn that runs, in tokens; 0 when the engine has not said
    /// (REQ-563).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub tokens: u64,
}

/// A process of Claude that has the session open: its pid, the folder it runs in and whether it
/// works now or waits for a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub pid: u32,
    pub cwd: String,
    pub busy: bool,
}

/// A tool that runs: its item's id and the milliseconds since the epoch at which it began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub id: String,
    pub since: u64,
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

/// The conversation's colour: one of the theme's six accents, never a free colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Blue,
    Green,
    Yellow,
    Red,
    Magenta,
    Cyan,
}

/// How much the engine may do without asking: Claude's own `--permission-mode` values that the
/// conversation offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "auto")]
    Auto,
}

impl Mode {
    /// The value `--permission-mode` takes.
    pub fn flag(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Plan => "plan",
            Mode::Auto => "auto",
        }
    }

    pub fn from_flag(s: &str) -> Option<Mode> {
        Some(match s {
            "manual" | "default" => Mode::Manual,
            "acceptEdits" => Mode::AcceptEdits,
            "plan" => Mode::Plan,
            "auto" => Mode::Auto,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvStatus {
    Idle,
    Resuming,
    Thinking,
    Streaming,
    Tool,
    Asking,
    Compacting,
    Dead,
}

/// One thing in the conversation, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Item {
    /// What the person sent; `images` counts the pictures that went with it.
    User {
        text: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        images: u32,
    },
    /// What Claude wrote; `live` while it is still arriving.
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        live: bool,
        /// `text` as Markdown blocks, once it is finished and holds more than plain paragraphs
        /// (REQ-560): the daemon parses and the clients draw. A client without it draws `text`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<crate::markdown::MdBlock>>,
    },
    Tool {
        id: String,
        name: String,
        /// The path or the command it is about, cut short.
        arg: String,
        state: ToolState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diff: Vec<DiffLine>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Where the answer to its permission came from (`el móvil`, `modo accept edits`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// A subagent: a card with its progress and its totals.
    Agent {
        id: String,
        name: String,
        description: String,
        state: ToolState,
        tools: u32,
        tokens: u64,
        ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<String>,
        steps: Vec<String>,
        /// What the subagent was asked, clipped (REQ-600).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// The model the subagent runs with, as its own messages say it (short, `haiku-5.5`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A shell line of the person's (`!command`) and what it printed.
    Shell {
        command: String,
        state: ToolState,
        output: Vec<String>,
    },
    /// What a command answered, as lines.
    Out {
        lines: Vec<String>,
    },
    Note {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
    },
    /// The end of a turn: how long it took, how many tools it used and what it cost.
    Result {
        ms: u64,
        tools: u32,
        cost: u64,
    },
    Divider {
        text: String,
        kind: DividerKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Run,
    Ask,
    Ok,
    Denied,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DividerKind {
    Restart,
    Compact,
    Clear,
    Resume,
}

/// A line of a diff: `n` is its number in the file when it is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    pub sign: DiffSign,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffSign {
    #[serde(rename = "+")]
    Add,
    #[serde(rename = "-")]
    Del,
    #[serde(rename = " ")]
    Same,
}

/// What waits for the person: a permission to use a tool, a choice Claude asks for, or a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// What an answer names; the engine's own request id.
    pub id: String,
    pub kind: AskKind,
    pub tool: String,
    pub title: String,
    /// The command a `Bash` is about to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff: Vec<DiffLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<String>,
    pub options: Vec<AskOption>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub multi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    Permission,
    Question,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskOption {
    /// What an answer sends back: `yes`, `session` or `no` for a permission.
    pub key: String,
    pub label: String,
}

/// What the engine runs in the background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub kind: TaskKind,
    pub label: String,
    pub state: TaskState,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    /// The `id` of the item of the tool call that started it, when there is one: for a subagent
    /// that is its card (its tools, tokens, steps and prompt are there), for a shell or a monitor
    /// the `Bash` or `Monitor` call, whose `arg` is the command (REQ-600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Agent,
    Bash,
    Monitor,
    Loop,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Run,
    Done,
    Stopped,
    Failed,
}

/// One line of the checklist Claude keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub text: String,
    pub done: bool,
}

/// A page Claude published.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub title: String,
    pub url: String,
}

/// A command of the `/` list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvCommand {
    pub name: String,
    pub source: CommandSource,
    pub kind: CommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    Jwn,
    Claude,
    Project,
    Skill,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Local,
    Panel,
    Text,
    Prompt,
    Screen,
}

/// How much of the subscription's windows is used, in whole percent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<u32>,
}

/// What `open_conversation` answers: the conversation's node and the engine's session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub node: Id,
    pub session: String,
}

/// One conversation Claude saved for a folder: what `/resume` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConversation {
    /// The engine's id: what `open_conversation {resume}` takes.
    pub id: String,
    /// What the person (or Claude) called it, else the first thing said in it.
    pub name: String,
    /// Milliseconds since the epoch at which it last changed.
    pub at: u64,
}

/// What `list_conversations` answers, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationList {
    pub conversations: Vec<SavedConversation>,
}

/// What `read_older` answers: the items, oldest first, and the `before` that asks for the ones
/// before them (REQ-567); `older` is left out when there are no more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OlderItems {
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older: Option<u64>,
}

/// A picture sent with a message: the client has already made it small (a JPEG of at most 1568
/// pixels on its long side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub media_type: String,
    /// Base64.
    pub data: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}
