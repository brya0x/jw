//! A Claude agent's conversation as blocks, read from the transcript Claude
//! writes as it goes (`~/.claude/projects/<folder>/<session-id>.jsonl`, one
//! JSON entry a line): what a phone draws instead of the agent's screen
//! (mobile REQ-93). The file is Claude's, not jw's: every shape here is read
//! leniently and an entry that is not understood is skipped, never an error.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::proto::{ChatBlock, ChatTurn};

/// The blocks kept per conversation; older ones are let go.
const KEEP: usize = 400;
/// The blocks a client is told of when it starts watching.
pub const FIRST: usize = 150;
/// A transcript bigger than this is read from its last bytes.
const MAX_READ: u64 = 16 << 20;
/// The most characters of a message, of a tool's output and of the one line
/// that sums a tool call up.
const TEXT_MAX: usize = 16 << 10;
const BODY_MAX: usize = 4 << 10;
const ARG_MAX: usize = 200;
/// The most bytes of a plan file that are read.
const PLAN_MAX: u64 = 64 << 10;

/// The session a pane's agent runs, from the command that started it
/// (`claude --session-id <uuid> …`, or `--resume <uuid>`).
pub fn session_id(cmd: &str) -> Option<String> {
    for flag in ["--session-id ", "--resume "] {
        if let Some(at) = cmd.find(flag) {
            let id: String = cmd[at + flag.len()..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
                .collect();
            if id.len() == 36 {
                return Some(id);
            }
        }
    }
    None
}

/// Claude's config folder: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude")),
    }
}

/// Where the transcript of `id` is: the folder named for `cwd` first (every
/// character that is not a letter or a digit becomes `-`), else any project
/// folder that has it (Claude may name them another way).
pub fn find(id: &str, cwd: &Path) -> Option<PathBuf> {
    let projects = config_dir()?.join("projects");
    let file = format!("{id}.jsonl");
    let direct = projects.join(folder_name(cwd)).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
}

/// The session a Claude started by hand in `cwd` is on, when it was not told
/// (`--session-id`, `--resume`): the transcript of that folder written to
/// since `since`, the newest of them. Two Claudes started in one folder at
/// once cannot be told apart this way, and the newest wins.
pub fn latest_since(cwd: &Path, since: std::time::SystemTime) -> Option<String> {
    let dir = config_dir()?.join("projects").join(folder_name(cwd));
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let id = path.file_stem()?.to_str()?.to_string();
            let mtime = e.metadata().ok()?.modified().ok()?;
            (path.extension()? == "jsonl" && id.len() == 36 && mtime >= since)
                .then_some((mtime, id))
        })
        .max()
        .map(|(_, id)| id)
}

/// The folder Claude names for work in `cwd`: every character that is not a
/// letter or a digit becomes `-`.
fn folder_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What a read of the file changed: the conversation's title when it did, and
/// the blocks that are new or changed (a tool's block changes when its result
/// comes).
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub name: Option<String>,
    pub title: Option<String>,
    pub turn: Option<ChatTurn>,
    pub bg: Option<u32>,
    pub plan: Option<String>,
    pub blocks: Vec<ChatBlock>,
}

/// A transcript being followed: where it was read to, and what it holds.
pub struct Tail {
    path: Option<PathBuf>,
    offset: u64,
    /// The end of the last read, when it was not a whole line yet.
    partial: Vec<u8>,
    blocks: Vec<ChatBlock>,
    /// The `i` of `blocks[0]`.
    base: u32,
    tools: HashMap<String, u32>,
    title: Option<String>,
    /// What the person named the conversation (`/rename`), and the name its
    /// agent has: the first wins when both are there.
    custom_name: Option<String>,
    agent_name: Option<String>,
    /// The turn in progress (REQ-99): from the person's last message, with the output tokens of each
    /// message of Claude's since (a message is written once per block it has, with the same count).
    turn: Option<ChatTurn>,
    msg_tokens: HashMap<String, u64>,
    /// The blocks of messages the person sent while Claude worked and the transcript has not yet.
    queued: Vec<u32>,
    /// The calls that started a background task or subagent and whose end was not told yet (REQ-100).
    bg_open: HashSet<String>,
    notices: HashSet<String>,
    /// The name Claude gives the conversation's plan file (`<config>/plans/<slug>.md`), and what that file held when last read.
    slug: Option<String>,
    plan: Option<(std::time::SystemTime, String)>,
}

impl Tail {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
            blocks: Vec::new(),
            base: 0,
            tools: HashMap::new(),
            title: None,
            custom_name: None,
            agent_name: None,
            turn: None,
            msg_tokens: HashMap::new(),
            queued: Vec::new(),
            bg_open: HashSet::new(),
            notices: HashSet::new(),
            slug: None,
            plan: None,
        }
    }

    /// The turn Claude is in, once the person has said something.
    pub fn turn(&self) -> Option<ChatTurn> {
        self.turn.clone()
    }

    /// The plan Claude drafted for this conversation, read from its plan file.
    pub fn plan(&self) -> Option<String> {
        self.plan.as_ref().map(|(_, t)| t.clone())
    }

    /// Reads the plan file again when it changed; the text is news when it did.
    fn read_plan(&mut self) -> Option<String> {
        let file = config_dir()?
            .join("plans")
            .join(format!("{}.md", self.slug.as_deref()?));
        let at = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
        if self.plan.as_ref().is_some_and(|(seen, _)| *seen == at) {
            return None;
        }
        let mut text = String::new();
        File::open(&file)
            .ok()?
            .take(PLAN_MAX)
            .read_to_string(&mut text)
            .ok()?;
        self.plan = Some((at, text.clone()));
        Some(text)
    }

    /// Background tasks and subagents started that have not ended.
    pub fn bg(&self) -> u32 {
        self.bg_open.len() as u32
    }

    /// The transcript's path, once it exists: a conversation that has not
    /// said anything yet has no file.
    pub fn locate(&mut self, id: &str, cwd: &Path) {
        if self.path.is_none() {
            self.path = find(id, cwd);
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.custom_name.as_deref().or(self.agent_name.as_deref())
    }

    /// The last `n` blocks, for a client that starts watching.
    pub fn last(&self, n: usize) -> Vec<ChatBlock> {
        self.blocks[self.blocks.len().saturating_sub(n)..].to_vec()
    }

    /// Reads what was appended since the last read, and the plan file when it changed.
    pub fn poll(&mut self) -> Changes {
        let mut changes = self.poll_blocks();
        changes.plan = self.read_plan();
        changes
    }

    fn poll_blocks(&mut self) -> Changes {
        let Some(path) = self.path.clone() else {
            return Changes::default();
        };
        let Ok(mut f) = File::open(&path) else {
            return Changes::default();
        };
        let Ok(len) = f.metadata().map(|m| m.len()) else {
            return Changes::default();
        };
        if len < self.offset {
            // Rewritten from the start (a `/clear`, a compaction): begin again.
            *self = Self::new(Some(path));
            return self.poll_blocks();
        }
        if self.offset == 0 && len > MAX_READ {
            self.offset = len - MAX_READ;
            // Into the middle of a line: drop it.
            self.partial = b"\0".to_vec();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Changes::default();
        }
        let mut fresh = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut fresh).is_err() {
            return Changes::default();
        }
        self.offset += fresh.len() as u64;
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(&fresh);
        let whole = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        self.partial = data[whole..].to_vec();
        let before_title = self.title.clone();
        let before_name = self.name().map(str::to_string);
        let before_turn = self.turn.clone();
        let before_bg = self.bg();
        let mut touched: Vec<u32> = Vec::new();
        for line in data[..whole].split(|b| *b == b'\n') {
            if let Ok(line) = std::str::from_utf8(line) {
                self.apply(line, &mut touched);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        let name = self.name().map(str::to_string);
        Changes {
            name: if name != before_name { name } else { None },
            title: (self.title != before_title)
                .then(|| self.title.clone())
                .flatten(),
            turn: if self.turn != before_turn {
                self.turn.clone()
            } else {
                None
            },
            bg: (self.bg() != before_bg).then(|| self.bg()),
            plan: None,
            blocks: touched
                .iter()
                .filter_map(|i| i.checked_sub(self.base))
                .filter_map(|at| self.blocks.get(at as usize).cloned())
                .collect(),
        }
    }

    fn push(&mut self, mut b: ChatBlock, touched: &mut Vec<u32>) -> u32 {
        b.i = self.base + self.blocks.len() as u32;
        let i = b.i;
        self.blocks.push(b);
        touched.push(i);
        if self.blocks.len() > KEEP {
            let drop = self.blocks.len() - KEEP;
            self.blocks.drain(..drop);
            self.base += drop as u32;
            self.tools.retain(|_, i| *i >= self.base);
        }
        i
    }

    /// One line of the transcript.
    fn apply(&mut self, line: &str, touched: &mut Vec<u32>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let slug = v["slug"]
            .as_str()
            .filter(|s| !s.is_empty() && !s.contains(['/', '\\', '.']));
        if let Some(slug) = slug.filter(|s| self.slug.as_deref() != Some(*s)) {
            self.slug = Some(slug.to_string());
            self.plan = None;
        }
        match v["type"].as_str() {
            Some("custom-title") => {
                if let Some(t) = v["customTitle"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.custom_name = Some(cut(t.trim(), ARG_MAX));
                }
            }
            Some("agent-name") => {
                if let Some(t) = v["agentName"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.agent_name = Some(cut(t.trim(), ARG_MAX));
                }
            }
            Some("ai-title") => {
                if let Some(t) = v["aiTitle"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.title = Some(cut(t.trim(), ARG_MAX));
                }
            }
            Some("queue-operation") => {
                // A message sent while Claude works waits in a queue until the turn takes it.
                let text = v["content"].as_str().unwrap_or("").trim();
                if v["operation"].as_str() == Some("enqueue")
                    && !text.is_empty()
                    && !text.starts_with(NOTICE)
                {
                    let b = ChatBlock {
                        kind: "queued".into(),
                        text: cut(text, TEXT_MAX),
                        ..ChatBlock::default()
                    };
                    let i = self.push(b, touched);
                    self.queued.push(i);
                }
            }
            Some("attachment") => {
                // What reached Claude in the middle of a turn: a notice, or a message of the person's.
                let a = &v["attachment"];
                if a["type"].as_str() != Some("queued_command") {
                    return;
                }
                let prompt = a["prompt"].as_str().unwrap_or("");
                if prompt.trim_start().starts_with(NOTICE) {
                    self.notice(prompt, touched);
                } else if a["origin"]["kind"].as_str().unwrap_or("human") == "human" {
                    self.text_block("user", prompt, epoch_ms(&v), touched);
                }
            }
            Some(kind @ ("user" | "assistant")) => {
                if kind == "assistant" {
                    self.count_tokens(&v);
                }
                if kind == "user" {
                    let text = v["message"]["content"].as_str().unwrap_or("");
                    if v["origin"]["kind"].as_str() == Some("task-notification")
                        || (v["origin"].is_null() && text.trim_start().starts_with(NOTICE))
                    {
                        let xml = text.to_string();
                        self.notice(&xml, touched);
                        return;
                    }
                    // A slash command the person ran, or the output of a local one: not the person's own words.
                    if let Some(b) = command_block(text) {
                        self.push(b, touched);
                        return;
                    }
                }
                // A subagent's own conversation, and what Claude wrote for
                // itself (hook output, caveats), is not the person's chat.
                if v["isSidechain"].as_bool() == Some(true)
                    || (kind == "user" && v["isMeta"].as_bool() == Some(true))
                {
                    return;
                }
                let content = &v["message"]["content"];
                if let Some(text) = content.as_str() {
                    self.text_block(kind, text, epoch_ms(&v), touched);
                    return;
                }
                let Some(parts) = content.as_array() else {
                    return;
                };
                for part in parts {
                    match part["type"].as_str() {
                        Some("text") => self.text_block(
                            kind,
                            part["text"].as_str().unwrap_or(""),
                            epoch_ms(&v),
                            touched,
                        ),
                        Some("thinking") => {
                            let secs = v["thinkingDurationMs"].as_u64().map(|ms| ms.div_ceil(1000));
                            let b = ChatBlock {
                                kind: "think".into(),
                                result: secs.map(|s| format!("{s} s")),
                                ..ChatBlock::default()
                            };
                            self.push(b, touched);
                        }
                        Some("tool_use") => self.tool_use(part, touched),
                        Some("tool_result") => self.tool_result(part, touched),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn text_block(&mut self, role: &str, text: &str, at: Option<u64>, touched: &mut Vec<u32>) {
        if text.trim().is_empty() {
            return;
        }
        if role == "user" {
            // The message the person sent while Claude worked is its queued block, taken by the turn.
            let queued = self.queued.iter().position(|i| {
                i.checked_sub(self.base)
                    .and_then(|at| self.blocks.get(at as usize))
                    .is_some_and(|b| b.text == cut(text.trim(), TEXT_MAX))
            });
            if let Some(n) = queued {
                let i = self.queued.remove(n);
                if let Some(b) = i
                    .checked_sub(self.base)
                    .and_then(|at| self.blocks.get_mut(at as usize))
                {
                    b.kind = "user".into();
                    touched.push(i);
                    return;
                }
            }
            if let Some(since) = at {
                self.turn = Some(ChatTurn { since, tokens: 0 });
                self.msg_tokens.clear();
            }
        }
        let b = ChatBlock {
            kind: if role == "user" { "user" } else { "text" }.into(),
            text: cut(text.trim(), TEXT_MAX),
            ..ChatBlock::default()
        };
        self.push(b, touched);
    }

    /// The output tokens of a message of Claude's, counted once however many lines it was written in.
    fn count_tokens(&mut self, v: &Value) {
        let (Some(id), Some(n)) = (
            v["message"]["id"].as_str(),
            v["message"]["usage"]["output_tokens"].as_u64(),
        ) else {
            return;
        };
        self.msg_tokens.insert(id.to_string(), n);
        if let Some(t) = self.turn.as_mut() {
            t.tokens = self.msg_tokens.values().sum();
        }
    }

    /// The PC telling Claude that a subagent or a background task ended.
    fn notice(&mut self, xml: &str, touched: &mut Vec<u32>) {
        let id = tag(xml, "task-id").unwrap_or_default();
        if !id.is_empty() && !self.notices.insert(id) {
            return;
        }
        if let Some(id) = tag(xml, "tool-use-id") {
            self.bg_open.remove(&id);
        }
        let summary = tag(xml, "summary").unwrap_or_default();
        let title = match (summary.find('"'), summary.rfind('"')) {
            (Some(a), Some(b)) if b > a => summary[a + 1..b].to_string(),
            _ => summary.clone(),
        };
        let status = match tag(xml, "status").as_deref() {
            Some("completed") => "done",
            Some("stopped" | "killed" | "cancelled") => "stopped",
            _ => "failed",
        };
        let secs = tag(xml, "duration_ms")
            .and_then(|ms| ms.trim().parse::<u64>().ok())
            .map(|ms| format!("{} s", ms.div_ceil(1000)));
        self.push(
            ChatBlock {
                kind: "notice".into(),
                name: status.into(),
                text: cut(title.trim(), ARG_MAX),
                arg: if summary.trim_start().starts_with("Agent") {
                    "agent"
                } else {
                    "task"
                }
                .into(),
                result: secs,
                ..ChatBlock::default()
            },
            touched,
        );
    }

    fn tool_use(&mut self, part: &Value, touched: &mut Vec<u32>) {
        if part["input"]["run_in_background"].as_bool() == Some(true)
            && let Some(id) = part["id"].as_str()
        {
            self.bg_open.insert(id.to_string());
        }
        let name = part["name"].as_str().unwrap_or("").to_string();
        let (arg, body, result) = summarise(&name, &part["input"]);
        let id = part["id"].as_str().unwrap_or("").to_string();
        let i = self.push(
            ChatBlock {
                kind: "tool".into(),
                name,
                arg,
                body,
                result,
                ..ChatBlock::default()
            },
            touched,
        );
        if !id.is_empty() {
            self.tools.insert(id, i);
        }
    }

    fn tool_result(&mut self, part: &Value, touched: &mut Vec<u32>) {
        let Some(i) = part["tool_use_id"]
            .as_str()
            .and_then(|id| self.tools.get(id))
            .copied()
        else {
            return;
        };
        let Some(b) = i
            .checked_sub(self.base)
            .and_then(|at| self.blocks.get_mut(at as usize))
        else {
            return;
        };
        let out = match &part["content"] {
            Value::String(s) => s.clone(),
            Value::Array(a) => a
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let error = part["is_error"].as_bool() == Some(true);
        b.error = error;
        // An edit already says what it changed; its result is only news when it failed.
        if b.result.is_none() || error {
            b.result = Some(sum_up(&out));
        }
        if b.body.is_none() || error {
            let t = out.trim();
            b.body = (!t.is_empty()).then(|| cut(t, BODY_MAX));
        }
        touched.push(i);
    }
}

/// A user entry that is a slash command or a local command's output, as the block it is drawn as (REQ-91): `command` (`text` is
/// what was run, `/commit` or `/model opus`, or `!ls` for a shell line) and `commandout` (`text` is what it printed).
fn command_block(text: &str) -> Option<ChatBlock> {
    let t = text.trim_start();
    let block = |kind: &str, text: String| ChatBlock {
        kind: kind.into(),
        text: cut(text.trim(), TEXT_MAX),
        ..ChatBlock::default()
    };
    if t.starts_with("<command-name>") || t.starts_with("<command-message>") {
        let name = tag(t, "command-name")
            .or_else(|| tag(t, "command-message").map(|m| format!("/{m}")))?;
        let args = tag(t, "command-args").unwrap_or_default();
        return Some(block("command", format!("{name} {args}")));
    }
    if t.starts_with("<bash-input>") {
        return Some(block("command", format!("!{}", tag(t, "bash-input")?)));
    }
    if t.starts_with("<local-command-stdout>") || t.starts_with("<bash-stdout>") {
        let out = tag(t, "local-command-stdout")
            .or_else(|| tag(t, "bash-stdout"))
            .unwrap_or_default();
        let err = tag(t, "bash-stderr").unwrap_or_default();
        let all = format!("{out}\n{err}");
        return (!all.trim().is_empty()).then(|| block("commandout", all));
    }
    None
}

/// How an entry that is the PC's notice of an ended task begins.
const NOTICE: &str = "<task-notification>";

/// What is between `<name>` and `</name>`, the first time.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let from = xml.find(&open)? + open.len();
    let to = xml[from..].find(&format!("</{name}>"))? + from;
    Some(xml[from..to].trim().to_string())
}

/// When an entry was written, in ms since the epoch, from its `timestamp` (`2026-10-12T18:25:23.853Z`).
fn epoch_ms(v: &Value) -> Option<u64> {
    let t = v["timestamp"].as_str()?;
    let n = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (
        n(0, 4)?,
        n(5, 7)?,
        n(8, 10)?,
        n(11, 13)?,
        n(14, 16)?,
        n(17, 19)?,
    );
    let ms = t
        .get(20..23)
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    // Days since 1970-01-01 of a civil date.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000 + ms).ok()
}

/// What a tool call is called, shown on one line, and its body: the command or
/// the diff, and for an edit the count of lines it changed.
fn summarise(name: &str, input: &Value) -> (String, Option<String>, Option<String>) {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    match name {
        "Bash" => {
            let cmd = s("command");
            (
                cut(cmd.lines().next().unwrap_or("").trim(), ARG_MAX),
                (cmd.lines().count() > 1).then(|| cut(cmd, BODY_MAX)),
                None,
            )
        }
        "Edit" => {
            let (body, add, del) = diff(s("old_string"), s("new_string"));
            (
                cut(s("file_path"), ARG_MAX),
                Some(body),
                Some(format!("+{add} −{del}")),
            )
        }
        "MultiEdit" => {
            let (mut body, mut add, mut del) = (String::new(), 0, 0);
            for e in input["edits"].as_array().into_iter().flatten() {
                let old = e["old_string"].as_str().unwrap_or("");
                let new = e["new_string"].as_str().unwrap_or("");
                let (b, a, d) = diff(old, new);
                body.push_str(&b);
                body.push('\n');
                add += a;
                del += d;
            }
            (
                cut(s("file_path"), ARG_MAX),
                Some(cut(&body, BODY_MAX)),
                Some(format!("+{add} −{del}")),
            )
        }
        "Read" | "Write" | "NotebookEdit" => (cut(s("file_path"), ARG_MAX), None, None),
        "Grep" | "Glob" => (cut(s("pattern"), ARG_MAX), None, None),
        "WebFetch" => (cut(s("url"), ARG_MAX), None, None),
        "WebSearch" => (cut(s("query"), ARG_MAX), None, None),
        "Task" | "Agent" => (cut(s("description"), ARG_MAX), None, None),
        // The plan Claude asks to go ahead with: its text is what a phone shows full screen (REQ-98).
        "ExitPlanMode" => ("plan".into(), Some(cut(s("plan"), TEXT_MAX)), None),
        _ => {
            let first = input
                .as_object()
                .and_then(|o| o.values().find_map(|v| v.as_str()))
                .unwrap_or("");
            (cut(first.lines().next().unwrap_or(""), ARG_MAX), None, None)
        }
    }
}

/// A diff of two texts as the lines taken out and put in, and their counts.
fn diff(old: &str, new: &str) -> (String, usize, usize) {
    let mut out = String::new();
    let (mut del, mut add) = (0, 0);
    for l in old.lines().take(40) {
        out.push_str(&format!("- {l}\n"));
        del += 1;
    }
    for l in new.lines().take(40) {
        out.push_str(&format!("+ {l}\n"));
        add += 1;
    }
    (cut(out.trim_end(), BODY_MAX), add, del)
}

/// A tool's output as one line: its first line and how many it has.
fn sum_up(out: &str) -> String {
    let lines = out.lines().filter(|l| !l.trim().is_empty()).count();
    match out.lines().find(|l| !l.trim().is_empty()) {
        None => "sin salida".into(),
        Some(first) if lines > 1 => format!("{} · {lines} líneas", cut(first.trim(), 60)),
        Some(first) => cut(first.trim(), 80),
    }
}

/// `s` cut to `max` characters, with `…` when it was.
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_of(lines: &[&str]) -> (Tail, Vec<ChatBlock>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let mut t = Tail::new(Some(path));
        let c = t.poll();
        (t, c.blocks, dir)
    }

    const UUID: &str = "0f6bd1c2-1111-4222-8333-444455556666";

    /// The tests that point `CLAUDE_CONFIG_DIR` at a folder of their own take turns.
    static CONFIG: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_session_is_in_the_command_that_started_the_agent() {
        assert_eq!(
            session_id(&format!("claude --session-id {UUID} --settings '{{}}'")).as_deref(),
            Some(UUID)
        );
        assert_eq!(
            session_id(&format!("claude --resume {UUID}")).as_deref(),
            Some(UUID)
        );
        assert_eq!(session_id("claude --session-id nope"), None);
        assert_eq!(session_id("sleep 30"), None);
    }

    #[test]
    fn messages_thoughts_and_tools_become_blocks_and_a_result_finds_its_call() {
        let (t, blocks, _d) = tail_of(&[
            r#"{"type":"ai-title","aiTitle":"Arreglar el socket"}"#,
            r#"{"type":"user","message":{"role":"user","content":"hola"}}"#,
            r#"{"type":"assistant","thinkingDurationMs":5200,"message":{"content":[{"type":"thinking","thinking":"..."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Miro el socket."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test -p jw-daemon"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"running 2 tests\ntest result: ok"}]}}"#,
        ]);
        assert_eq!(t.title(), Some("Arreglar el socket"));
        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(kinds, ["user", "think", "text", "tool"]);
        assert_eq!(blocks[0].text, "hola");
        assert_eq!(blocks[1].result.as_deref(), Some("6 s"));
        let tool = &blocks[3];
        assert_eq!(
            (tool.name.as_str(), tool.arg.as_str()),
            ("Bash", "cargo test -p jw-daemon")
        );
        assert_eq!(tool.result.as_deref(), Some("running 2 tests · 2 líneas"));
        assert!(tool.body.as_deref().unwrap().contains("test result: ok"));
        assert_eq!(blocks.iter().map(|b| b.i).collect::<Vec<_>>(), [0, 1, 2, 3]);
    }

    #[test]
    fn the_name_the_person_gave_the_chat_wins_over_the_agents_and_the_title_stays_apart() {
        let (t, _b, _d) = tail_of(&[
            r#"{"type":"ai-title","aiTitle":"Arreglar el socket"}"#,
            r#"{"type":"agent-name","agentName":"agente"}"#,
        ]);
        assert_eq!(
            (t.name(), t.title()),
            (Some("agente"), Some("Arreglar el socket"))
        );
        let (t, _b, _d) = tail_of(&[
            r#"{"type":"agent-name","agentName":"agente"}"#,
            r#"{"type":"custom-title","customTitle":"mobile-fixes"}"#,
            r#"{"type":"ai-title","aiTitle":"Arreglar el socket"}"#,
        ]);
        assert_eq!(
            (t.name(), t.title()),
            (Some("mobile-fixes"), Some("Arreglar el socket"))
        );
        // A rename that comes later is news.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"custom-title\",\"customTitle\":\"uno\"}\n",
        )
        .unwrap();
        let mut t = Tail::new(Some(path.clone()));
        assert_eq!(t.poll().name.as_deref(), Some("uno"));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"type\":\"custom-title\",\"customTitle\":\"dos\"}\n",
        )
        .unwrap();
        assert_eq!(t.poll().name.as_deref(), Some("dos"));
    }

    #[test]
    fn an_edit_says_what_it_changed_and_a_failed_tool_says_it_failed() {
        let (_t, blocks, _d) = tail_of(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"a.rs","old_string":"x\ny","new_string":"z"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e1","content":"updated"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"false"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"exit 1"}]}}"#,
        ]);
        assert_eq!(blocks[0].result.as_deref(), Some("+1 −2"));
        assert_eq!(blocks[0].body.as_deref(), Some("- x\n- y\n+ z"));
        assert!(!blocks[0].error);
        assert!(blocks[1].error);
        assert_eq!(blocks[1].result.as_deref(), Some("exit 1"));
    }

    #[test]
    fn what_is_not_the_persons_chat_or_not_understood_is_skipped() {
        let (_t, blocks, _d) = tail_of(&[
            r#"{"type":"user","isMeta":true,"message":{"content":"<caveat>"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"attachment","attachment":{}}"#,
            "not json",
            r#"{"type":"assistant","message":{"content":[{"type":"mystery"}]}}"#,
            r#"{"type":"user","message":{"content":"real"}}"#,
        ]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "real");
    }

    #[test]
    fn a_read_gives_only_what_was_appended_and_waits_for_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let line = |t: &str| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{t}\"}}}}\n");
        std::fs::write(&path, line("uno")).unwrap();
        let mut t = Tail::new(Some(path.clone()));
        assert_eq!(t.poll().blocks.len(), 1);
        assert_eq!(t.poll(), Changes::default());
        // Half a line is not read yet.
        let whole = line("dos");
        let (a, b) = whole.split_at(10);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, a.as_bytes()).unwrap();
        assert_eq!(t.poll(), Changes::default());
        std::io::Write::write_all(&mut f, b.as_bytes()).unwrap();
        let c = t.poll();
        assert_eq!(c.blocks.len(), 1);
        assert_eq!((c.blocks[0].i, c.blocks[0].text.as_str()), (1, "dos"));
    }

    #[test]
    fn a_result_that_arrives_later_changes_its_block_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"Read\",\"input\":{\"file_path\":\"a\"}}]}}\n",
        )
        .unwrap();
        let mut t = Tail::new(Some(path.clone()));
        let first = t.poll();
        assert_eq!(first.blocks[0].result, None);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t\",\"content\":\"ok\"}]}}\n",
        )
        .unwrap();
        let c = t.poll();
        assert_eq!(c.blocks.len(), 1);
        assert_eq!(
            (c.blocks[0].i, c.blocks[0].result.as_deref()),
            (0, Some("ok"))
        );
    }

    #[test]
    fn a_long_conversation_keeps_its_last_blocks_and_numbers_stay_whole() {
        let lines: Vec<String> = (0..KEEP + 20)
            .map(|n| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"m{n}\"}}}}"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (t, _all, _d) = tail_of(&refs);
        let last = t.last(3);
        assert_eq!(
            last.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            ["m417", "m418", "m419"]
        );
        assert_eq!(last[2].i, (KEEP + 19) as u32);
    }

    #[test]
    fn a_claude_started_by_hand_is_on_the_newest_transcript_written_since_it_started() {
        let _turn = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the tests that set it hold `CONFIG`.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir.path()) };
        let folder = dir.path().join("projects/-w-x");
        std::fs::create_dir_all(&folder).unwrap();
        let old = "11111111-1111-4111-8111-111111111111";
        let new = "22222222-2222-4222-8222-222222222222";
        std::fs::write(folder.join(format!("{old}.jsonl")), "").unwrap();
        let started = std::time::SystemTime::now() + std::time::Duration::from_millis(50);
        // Nothing has been written since it started: no session yet.
        assert_eq!(latest_since(Path::new("/w/x"), started), None);
        std::thread::sleep(std::time::Duration::from_millis(120));
        std::fs::write(folder.join(format!("{new}.jsonl")), "").unwrap();
        std::fs::write(folder.join("notes.txt"), "").unwrap();
        assert_eq!(
            latest_since(Path::new("/w/x"), started).as_deref(),
            Some(new)
        );
        assert_eq!(latest_since(Path::new("/other"), started), None);
    }

    #[test]
    fn the_transcript_is_found_by_the_folder_name_or_by_looking() {
        let _turn = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the tests that set it hold `CONFIG`.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir.path()) };
        let named = dir.path().join("projects/-a-b-c.d");
        std::fs::create_dir_all(&named).unwrap();
        std::fs::write(named.join(format!("{UUID}.jsonl")), "").unwrap();
        assert_eq!(
            find(UUID, Path::new("/a/b/c.d")),
            Some(named.join(format!("{UUID}.jsonl")))
        );
        // Another folder name still finds it.
        assert_eq!(
            find(UUID, Path::new("/somewhere/else")),
            Some(named.join(format!("{UUID}.jsonl")))
        );
        assert_eq!(
            find("00000000-0000-0000-0000-000000000000", Path::new("/a")),
            None
        );
    }

    #[test]
    fn a_task_notice_is_a_notice_and_not_a_message_of_the_person_and_ends_a_background_task() {
        let note = "<task-notification>\n<task-id>abc</task-id>\n<tool-use-id>a</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Scout ask UI\" finished</summary>\n<usage><duration_ms>18488</duration_ms></usage>\n</task-notification>";
        let note_line = serde_json::json!({"type":"user","origin":{"kind":"task-notification"},"message":{"content":note}}).to_string();
        // The same notice also comes as a queued command: it counts once.
        let queued_line = serde_json::json!({"type":"attachment","attachment":{"type":"queued_command","prompt":note}}).to_string();
        let failed = "<task-notification>\n<task-id>def</task-id>\n<tool-use-id>b</tool-use-id>\n<status>failed</status>\n<summary>Background command \"cargo test\" failed</summary>\n</task-notification>";
        let failed_line =
            serde_json::json!({"type":"user","message":{"content":failed}}).to_string();
        let (t, blocks, _d) = tail_of(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Agent","input":{"run_in_background":true}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"b","name":"Bash","input":{"command":"cargo test","run_in_background":true}}]}}"#,
            &note_line,
            &queued_line,
            &failed_line,
        ]);
        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(kinds, ["tool", "tool", "notice", "notice"]);
        let n = &blocks[2];
        assert_eq!(
            (
                n.name.as_str(),
                n.text.as_str(),
                n.arg.as_str(),
                n.result.as_deref()
            ),
            ("done", "Scout ask UI", "agent", Some("19 s"))
        );
        let f = &blocks[3];
        assert_eq!(
            (f.name.as_str(), f.text.as_str(), f.arg.as_str()),
            ("failed", "cargo test", "task")
        );
        assert_eq!(t.bg(), 0);
        // A notice of a task started before this transcript's start does not end one that runs.
        let old = "<task-notification>\n<task-id>z</task-id>\n<tool-use-id>gone</tool-use-id>\n<status>stopped</status>\n<summary>Background command \"x\" stopped</summary>\n</task-notification>";
        let old_line = serde_json::json!({"type":"user","message":{"content":old}}).to_string();
        let (t, _b, _d) = tail_of(&[
            &old_line,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c","name":"Bash","input":{"command":"sleep 30","run_in_background":true}}]}}"#,
        ]);
        assert_eq!(t.bg(), 1);
    }

    #[test]
    fn a_message_sent_while_claude_works_is_queued_until_the_turn_takes_it() {
        let (mut t, blocks, d) = tail_of(&[
            r#"{"type":"user","timestamp":"2026-10-12T18:00:00.000Z","message":{"content":"hola"}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","content":"y también esto"}"#,
            r#"{"type":"queue-operation","operation":"enqueue","content":"<task-notification><task-id>x</task-id></task-notification>"}"#,
        ]);
        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(kinds, ["user", "queued"]);
        // The turn takes it: the same block, now the person's message, and no second one.
        let line = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"y también esto","origin":{"kind":"human"}}}"#;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(d.path().join("s.jsonl"))
            .unwrap();
        std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()).unwrap();
        let c = t.poll();
        assert_eq!(c.blocks.len(), 1);
        assert_eq!((c.blocks[0].i, c.blocks[0].kind.as_str()), (1, "user"));
    }

    #[test]
    fn the_turn_starts_at_the_persons_message_and_counts_each_message_of_claude_once() {
        let (t, _b, _d) = tail_of(&[
            r#"{"type":"user","timestamp":"2026-10-12T18:00:00.500Z","message":{"content":"hola"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","usage":{"output_tokens":10},"content":[{"type":"thinking","thinking":""}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","usage":{"output_tokens":12},"content":[{"type":"text","text":"a"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m2","usage":{"output_tokens":30},"content":[{"type":"text","text":"b"}]}}"#,
        ]);
        // 2026-10-12T18:00:00.500Z
        assert_eq!(
            t.turn(),
            Some(ChatTurn {
                since: 1_791_828_000_500,
                tokens: 42
            })
        );
    }

    #[test]
    fn the_plan_of_exit_plan_mode_is_the_body_of_its_call() {
        let (_t, blocks, _d) = tail_of(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"p","name":"ExitPlanMode","input":{"plan":"1. Daemon\n2. App"}}]}}"#,
        ]);
        assert_eq!(blocks[0].name, "ExitPlanMode");
        assert_eq!(blocks[0].body.as_deref(), Some("1. Daemon\n2. App"));
        assert_eq!(blocks[0].result, None);
    }

    #[test]
    fn the_plan_file_named_by_the_conversations_slug_is_news_when_it_changes() {
        let _g = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cfg.path().join("plans")).unwrap();
        std::fs::write(cfg.path().join("plans/valiant-owl.md"), "# Plan\n1. Daemon").unwrap();
        // SAFETY: the tests that set this variable take turns on CONFIG.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", cfg.path()) };
        let (mut t, _b, _d) =
            tail_of(&[r#"{"type":"user","slug":"valiant-owl","message":{"content":"hola"}}"#]);
        // `tail_of` polled once: the plan is held, and it is not news again until the file changes.
        assert_eq!(t.plan().as_deref(), Some("# Plan\n1. Daemon"));
        assert_eq!(t.poll().plan, None);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            cfg.path().join("plans/valiant-owl.md"),
            "# Plan\n1. Daemon\n2. App",
        )
        .unwrap();
        assert_eq!(t.poll().plan.as_deref(), Some("# Plan\n1. Daemon\n2. App"));
        // A slug that is a path is not followed.
        let (t2, _b, _d) =
            tail_of(&[r#"{"type":"user","slug":"../x","message":{"content":"hola"}}"#]);
        assert_eq!(t2.plan(), None);
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    }

    #[test]
    fn a_slash_command_and_a_local_commands_output_are_not_the_persons_words() {
        let (_t, blocks, _d) = tail_of(&[
            &serde_json::json!({"type":"user","message":{"content":"<command-message>commit</command-message>\n<command-name>/commit</command-name>"}}).to_string(),
            &serde_json::json!({"type":"user","message":{"content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"}}).to_string(),
            &serde_json::json!({"type":"user","message":{"content":"<local-command-stdout>Set model to opus</local-command-stdout>"}}).to_string(),
            &serde_json::json!({"type":"user","message":{"content":"<bash-input>ls -a</bash-input>"}}).to_string(),
            &serde_json::json!({"type":"user","message":{"content":"<bash-stdout>.git\nsrc</bash-stdout><bash-stderr></bash-stderr>"}}).to_string(),
            // The person's own words that only mention a tag stay their words.
            &serde_json::json!({"type":"user","message":{"content":"un caso como los <task-notifications> y <command-name>"}}).to_string(),
        ]);
        let seen: Vec<(&str, &str)> = blocks
            .iter()
            .map(|b| (b.kind.as_str(), b.text.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("command", "/commit"),
                ("command", "/model opus"),
                ("commandout", "Set model to opus"),
                ("command", "!ls -a"),
                ("commandout", ".git\nsrc"),
                (
                    "user",
                    "un caso como los <task-notifications> y <command-name>"
                ),
            ]
        );
    }
}
