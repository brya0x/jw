//! A conversation the engine already has, read from its own transcript instead of copied
//! (spec REQ-540 to REQ-557): what was said, with the time of each line, and the mode, the model and
//! the effort it ended with. The transcript is `~/.claude/projects/<folder>/<session>.jsonl`, one
//! JSON line for each message or note, and it only grows: a `Source` reads the end of it when a
//! conversation is opened and then the lines that are added, so a session another `claude` is
//! writing is followed. The engine does not send any of it again when it resumes.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use contract::conversation::{DividerKind, Item, Mode, Tone, ToolState};
use serde_json::Value;

use crate::conversation::{
    KEEP, LINE_CHARS, OUTPUT_LINES, clip, content_text, edit_diff, summary_of, tool_arg,
};

/// How much of the end of a transcript is read to begin with, and how much more is tried while it
/// holds fewer items than a node keeps (a long session is mostly requests and attachments, tens of
/// megabytes of lines, with few messages in them).
const WINDOWS: [u64; 3] = [4 << 20, 16 << 20, 64 << 20];

/// The most that is read at once of what has been added, and the longest line that is read: a longer
/// one (a pasted picture is a line of megabytes) is skipped to its end. Small in the tests.
const MOST_ADDED: u64 = if cfg!(test) { 1 << 10 } else { 8 << 20 };
const MAX_LINE: u64 = if cfg!(test) { 8 << 10 } else { 64 << 20 };

/// The longest thinking that is shown: Claude's summaries are a few sentences, and a model that
/// gives its whole thinking would give pages.
const SUMMARY_CHARS: usize = 1200;

/// The efforts `--effort` takes; anything else in a transcript is not passed on.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// An item and the time of the line it comes from, in milliseconds since 1970.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamped {
    pub at: u64,
    pub item: Item,
}

/// What the lines read so far say of a conversation.
#[derive(Debug, Default)]
pub struct Parser {
    pub items: Vec<Stamped>,
    /// The mode it was in last.
    pub mode: Option<Mode>,
    /// The model of Claude's last message, as the engine names it.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Where each tool is in `items`, so a result finds its tool without a search for each one.
    tools: HashMap<String, usize>,
    /// The time of the last line that had one: a line without it is as late as that.
    last_at: u64,
    /// Counts up whenever what the lines say changes: an item, the mode, the model, the effort or what waits.
    rev: u64,
    /// How many tools the turn that runs has used, for the line that ends it.
    tools_in_turn: u32,
    /// The line being read was written by jwn's own engine (`sdk-cli`): the notes, answers and ends
    /// of turn it makes are already jwn's own items (REQ-544), so they are not made again.
    own: bool,
    /// The messages typed while a turn ran, not taken in yet, oldest first, and whether each is
    /// the person's (a task's notice waits too, and is not shown).
    queue: Vec<(String, bool)>,
    /// When the turn that is open began (the time of the person's message that began it): a turn
    /// is open from that message until the line that ends it or an interruption (REQ-565).
    turn: Option<u64>,
    /// What the open turn has written, for each message of Claude, counted once by its id.
    turn_tokens: std::collections::BTreeMap<String, u64>,
    /// The tools of the open turn that wait for a result, in `items`.
    open_tools: Vec<usize>,
    /// The time of the last line of the person's side (a message, a notice of a task, a message of
    /// another agent, a result): what began a turn that Claude takes up with no message of the
    /// person's.
    last_user_at: u64,
}

impl Parser {
    /// The start of the turn that is open, when the last lines say one is.
    pub fn turn_since(&self) -> Option<u64> {
        self.turn
    }

    /// What the open turn has written, in tokens.
    pub fn turn_written(&self) -> u64 {
        self.turn_tokens.values().sum()
    }

    /// The time of the last line that had one.
    pub fn last_at(&self) -> u64 {
        self.last_at
    }

    /// The lines have been read to the end of what was wanted: a turn that is still open is
    /// over, and a tool that waited for a result was interrupted.
    fn finish(&mut self) {
        self.close_turn();
    }

    /// A turn begins with the person's message.
    fn open_turn(&mut self, since: u64) {
        // A turn that nothing ended is over when the person writes again.
        self.close_turn();
        self.turn = (since > 0).then_some(since);
        self.turn_tokens.clear();
        self.open_tools.clear();
        self.rev += 1;
    }

    /// The turn ends: a tool that never answered was interrupted.
    fn close_turn(&mut self) {
        if self.turn.take().is_none() {
            return;
        }
        for at in self.open_tools.drain(..) {
            if let Some(Stamped {
                item: Item::Tool { state, summary, .. },
                ..
            }) = self.items.get_mut(at)
                && *state == ToolState::Run
            {
                *state = ToolState::Failed;
                *summary = Some("sin resultado".into());
            }
        }
        self.rev += 1;
    }

    /// Read lines of a transcript, each whole.
    pub fn feed(&mut self, text: &str) {
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(at) = entry["timestamp"].as_str().and_then(iso_ms) {
                self.last_at = at;
            }
            // What a subagent said inside a tool is not a line of the conversation.
            if entry["isSidechain"] == true {
                continue;
            }
            self.own = entry["entrypoint"] == "sdk-cli";
            if entry["type"] == "user" {
                self.last_user_at = self.last_at;
            }
            match entry["type"].as_str().unwrap_or("") {
                "permission-mode" => {
                    let mode = entry["permissionMode"].as_str().and_then(Mode::from_flag);
                    if mode.is_some() && mode != self.mode {
                        self.mode = mode;
                        self.rev += 1;
                    }
                }
                "queue-operation" => self.queued(&entry),
                "attachment" => self.attached(&entry),
                "assistant" => self.assistant(&entry),
                // The summary Claude Code writes when it compacts is not something the person said.
                "user"
                    if entry["isMeta"] != true
                        && entry["isCompactSummary"] != true
                        && entry["isVisibleInTranscriptOnly"] != true =>
                {
                    self.user(&entry)
                }
                "system" if self.own && entry["subtype"] == "turn_duration" => self.close_turn(),
                "system" if self.own && entry["subtype"] != "compact_boundary" => {}
                "system" if entry["subtype"] == "informational" => {
                    if let Some(text) = entry["content"].as_str().filter(|t| !t.trim().is_empty()) {
                        self.note(clip(text.trim(), LINE_CHARS), None);
                    }
                }
                "system" if entry["subtype"] == "local_command" => {
                    self.command_output(entry["content"].as_str().unwrap_or(""));
                }
                "system" if entry["subtype"] == "compact_boundary" => {
                    self.close_turn();
                    self.push(Item::Divider {
                        text: "conversación compactada".into(),
                        kind: DividerKind::Compact,
                    });
                }
                "system" if entry["subtype"] == "turn_duration" => {
                    let ms = entry["durationMs"].as_u64().unwrap_or(0);
                    let tools = std::mem::take(&mut self.tools_in_turn);
                    self.close_turn();
                    self.push(Item::Result { ms, tools, cost: 0 });
                }
                _ => {}
            }
        }
    }

    fn note(&mut self, text: String, tone: Option<Tone>) {
        self.push(Item::Note { text, tone });
    }

    /// What a command of Claude's own printed (`<local-command-stdout>…`), as lines.
    fn command_output(&mut self, text: &str) {
        let inner = tag(text, "local-command-stdout").unwrap_or(text);
        let lines: Vec<String> = strip_ansi(inner)
            .lines()
            .map(|l| clip(l.trim_end(), LINE_CHARS))
            .filter(|l| !l.is_empty())
            .take(OUTPUT_LINES)
            .collect();
        if !lines.is_empty() {
            self.push(Item::Out { lines });
        }
    }

    /// A message the person typed while a turn ran and Claude took in during it: it is a line of
    /// its own in the transcript (an attachment), not a message of the person's.
    fn attached(&mut self, entry: &Value) {
        let a = &entry["attachment"];
        let human = a["origin"]["kind"].as_str().is_none_or(|k| k == "human");
        if a["type"] != "queued_command" || !human {
            return;
        }
        // A message with a picture has its prompt as blocks.
        let (text, images) = match &a["prompt"] {
            Value::String(s) => (s.clone(), 0),
            Value::Array(blocks) => blocks_text(blocks.iter().collect()),
            _ => return,
        };
        // Whatever it was, it is not waiting any more.
        self.unqueue(&text);
        if is_the_persons(&text) || (text.trim().is_empty() && images > 0) {
            // It does not begin a turn: the one that runs takes it in, and its tools go on counting.
            self.push(Item::User {
                text: text.trim().to_owned(),
                images,
            });
        }
    }

    /// A message that waits for the turn that runs, and the line that says it was taken in (REQ-549).
    fn queued(&mut self, entry: &Value) {
        let content = entry["content"].as_str();
        match (entry["operation"].as_str().unwrap_or(""), content) {
            ("enqueue", Some(text)) => {
                let shown = is_the_persons(text);
                self.queue.push((text.to_owned(), shown));
                self.rev += u64::from(shown);
            }
            // The first one was taken in, whatever it said.
            ("dequeue", _) if !self.queue.is_empty() => {
                let (_, shown) = self.queue.remove(0);
                self.rev += u64::from(shown);
            }
            ("remove", Some(text)) => self.unqueue(text),
            _ => {}
        }
    }

    /// The first message that waits with this text is not waiting any more.
    fn unqueue(&mut self, text: &str) {
        if let Some(at) = self.queue.iter().position(|(t, _)| t.trim() == text.trim()) {
            let (_, shown) = self.queue.remove(at);
            self.rev += u64::from(shown);
        }
    }

    /// The messages that wait, as the person typed them.
    pub fn waiting(&self) -> Vec<String> {
        self.queue
            .iter()
            .filter(|(_, shown)| *shown)
            .map(|(t, _)| t.trim().to_owned())
            .collect()
    }

    fn push(&mut self, item: Item) {
        self.rev += 1;
        self.items.push(Stamped {
            at: self.last_at,
            item,
        });
    }

    fn assistant(&mut self, entry: &Value) {
        let message = &entry["message"];
        // What Claude Code writes itself (an interrupted turn, «no response requested») is not Claude.
        if message["model"] == "<synthetic>" {
            // An error of the API is shown; «no response requested» and the like are not.
            if entry["isApiErrorMessage"] == true {
                self.close_turn();
            }
            if entry["isApiErrorMessage"] == true && !self.own {
                for block in message["content"].as_array().into_iter().flatten() {
                    if let Some(text) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        self.note(clip(text.trim(), LINE_CHARS), Some(Tone::Error));
                    }
                }
            }
            return;
        }
        // Claude takes up a turn with no message of the person's (a task ended, another agent
        // wrote): it began with the line that made it.
        if self.turn.is_none() {
            let began = if self.last_user_at > 0 {
                self.last_user_at
            } else {
                self.last_at
            };
            self.open_turn(began);
        }
        if let (Some(id), Some(n)) = (
            message["id"].as_str(),
            message["usage"]["output_tokens"].as_u64(),
        ) && self.turn.is_some()
        {
            let at = self.turn_tokens.entry(id.to_owned()).or_insert(0);
            if n > *at {
                *at = n;
                self.rev += 1;
            }
        }
        if let Some(model) = message["model"].as_str().filter(|m| !m.is_empty())
            && self.model.as_deref() != Some(model)
        {
            self.model = Some(model.to_owned());
            self.rev += 1;
        }
        if let Some(effort) = entry["effort"].as_str().filter(|e| EFFORTS.contains(e))
            && self.effort.as_deref() != Some(effort)
        {
            self.effort = Some(effort.to_owned());
            self.rev += 1;
        }
        for block in message["content"].as_array().into_iter().flatten() {
            match block["type"].as_str().unwrap_or("") {
                "text" => {
                    let text = block["text"].as_str().unwrap_or("");
                    if !text.trim().is_empty() {
                        self.push(Item::Text {
                            blocks: crate::markdown::conversation_blocks(text),
                            text: text.to_owned(),
                            live: false,
                        });
                    }
                }
                "tool_use" => {
                    let name = block["name"].as_str().unwrap_or("");
                    // The checklist and the tool search are the node's, not lines of the conversation.
                    if matches!(name, "TodoWrite" | "ToolSearch") {
                        continue;
                    }
                    let id = block["id"].as_str().unwrap_or("").to_owned();
                    self.tools_in_turn += 1;
                    self.tools.insert(id.clone(), self.items.len());
                    // In an open turn a tool with no result yet runs; in one that ended, a tool
                    // with none was interrupted.
                    let runs = self.turn.is_some();
                    if runs {
                        self.open_tools.push(self.items.len());
                    }
                    self.push(Item::Tool {
                        id,
                        name: name.to_owned(),
                        arg: tool_arg(name, &block["input"]),
                        state: if runs {
                            ToolState::Run
                        } else {
                            ToolState::Failed
                        },
                        summary: (!runs).then(|| "sin resultado".to_owned()),
                        output: Vec::new(),
                        diff: edit_diff(name, &block["input"]),
                        url: None,
                        by: None,
                    });
                }
                // The advisor is consulted by the API itself: the call and its answer are blocks
                // of Claude's own message, the answer a line later (REQ-570).
                "server_tool_use" if block["name"] == "advisor" => {
                    let id = block["id"].as_str().unwrap_or("");
                    self.tools.insert(id.to_owned(), self.items.len());
                    let mut row = crate::conversation::advisor_row(id);
                    if self.turn.is_some() {
                        self.open_tools.push(self.items.len());
                    } else if let Item::Tool { state, summary, .. } = &mut row {
                        *state = ToolState::Failed;
                        *summary = Some("sin resultado".into());
                    }
                    self.push(row);
                }
                "advisor_tool_result" => {
                    let found = block["tool_use_id"]
                        .as_str()
                        .and_then(|id| self.tools.get(id))
                        .copied();
                    if let Some(Stamped {
                        item: Item::Tool { state, summary, .. },
                        ..
                    }) = found.and_then(|at| self.items.get_mut(at))
                    {
                        let (now, says) = crate::conversation::advisor_answer(block);
                        *state = now;
                        *summary = Some(says);
                        self.rev += 1;
                    }
                }
                // What Claude says between its tools is a summary of its thinking: the lines that
                // are short are shown as the narration they are, and the whole thinking is not.
                "thinking" => {
                    let text = block["thinking"].as_str().unwrap_or("").trim();
                    if !text.is_empty() && text.chars().nth(SUMMARY_CHARS).is_none() {
                        self.push(Item::Text {
                            text: text.to_owned(),
                            live: false,
                            blocks: None,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, entry: &Value) {
        let content = &entry["message"]["content"];
        let blocks: Vec<&Value> = match content {
            Value::Array(a) => a.iter().collect(),
            _ => Vec::new(),
        };
        for block in &blocks {
            if block["type"] == "tool_result" {
                self.result(block, &entry["toolUseResult"]);
            }
        }
        let (text, images) = match content {
            Value::String(s) => (s.clone(), 0),
            _ => blocks_text(
                blocks
                    .iter()
                    .copied()
                    .filter(|b| b["type"] != "tool_result")
                    .collect(),
            ),
        };
        let t = text.trim();
        if self.own
            && (t.starts_with("[Request interrupted")
                || t.starts_with("<command-")
                || t.starts_with("<local-command-"))
        {
            // jwn's engine ran it and jwn made its own item of it.
            if t.starts_with("[Request interrupted") {
                self.close_turn();
            }
        } else if t.starts_with("[Request interrupted") {
            self.close_turn();
            self.note("Interrumpido".into(), Some(Tone::Warn));
        } else if t.starts_with("<task-notification") {
            // A background task ended: the line that says what it was.
            if let Some(summary) = tag(t, "summary").filter(|s| !s.trim().is_empty()) {
                self.note(clip(summary.trim(), LINE_CHARS), None);
            }
        } else if t.starts_with("<command-") && t.contains("<command-name>") {
            let name = tag(t, "command-name").unwrap_or("").trim();
            let args = tag(t, "command-args").unwrap_or("").trim();
            if !name.is_empty() {
                self.tools_in_turn = 0;
                self.push(Item::User {
                    text: format!("{name} {args}").trim().to_owned(),
                    images: 0,
                });
            }
        } else if t.starts_with("<local-command-stdout>") {
            self.command_output(t);
        } else if is_the_persons(&text) || (t.is_empty() && images > 0) {
            self.unqueue(&text);
            self.tools_in_turn = 0;
            self.open_turn(self.last_at);
            self.push(Item::User {
                text: t.to_owned(),
                images,
            });
        }
    }

    /// What a tool answered: its state, a line that says it and the first lines of its output.
    fn result(&mut self, block: &Value, detail: &Value) {
        let Some(at) = block["tool_use_id"]
            .as_str()
            .and_then(|id| self.tools.get(id))
            .copied()
        else {
            return;
        };
        let failed = block["is_error"].as_bool().unwrap_or(false);
        let lines: Vec<String> = content_text(&block["content"])
            .lines()
            .take(OUTPUT_LINES)
            .map(|l| clip(l, LINE_CHARS))
            .collect();
        if let Some(Stamped {
            item:
                Item::Tool {
                    name,
                    state,
                    summary,
                    output,
                    ..
                },
            ..
        }) = self.items.get_mut(at)
        {
            *state = if failed {
                ToolState::Failed
            } else {
                ToolState::Ok
            };
            *summary = Some(summary_of(name, &lines, detail, failed));
            *output = lines;
            self.rev += 1;
        }
    }
}

/// What Claude Code writes in a `user` line that is not the person's: the wrapper of a command and
/// of its output, of a shell line, of the notice of a background task and of a message that
/// another agent sent (a subagent's whole report, REQ-571). A message that merely starts with
/// `<` (a question about `<div>`) is the person's.
const WRAPPERS: [&str; 7] = [
    "<command-",
    "<local-command-",
    "<bash-",
    "<task-notification",
    "<system-reminder",
    "<agent-message",
    "Another Claude session sent a message",
];

/// Whether a text of a `user` line is something the person typed.
fn is_the_persons(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && !WRAPPERS.iter().any(|w| t.starts_with(w))
        && !t.starts_with("[Request interrupted")
}

/// The text of a message made of blocks, and how many pictures it has.
fn blocks_text(blocks: Vec<&Value>) -> (String, u32) {
    let text = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let images = blocks.iter().filter(|b| b["type"] == "image").count();
    (text, u32::try_from(images).unwrap_or(u32::MAX))
}

/// What is between `<name>` and `</name>`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let from = text.find(&open)? + open.len();
    let to = text[from..].find(&format!("</{name}>"))? + from;
    Some(&text[from..to])
}

/// A text without the colour codes of a terminal.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // A control sequence ends at a byte from `@` to `~`.
            Some('[') => {
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            // A command of the operating system ends at a bell or at `ESC \`.
            Some(']') => {
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The lines of `text`, whole, as a conversation.
pub fn parse(text: &str) -> Parser {
    let mut p = Parser::default();
    p.feed(text);
    p
}

/// `2026-10-13T19:52:42.093Z` as milliseconds since 1970.
fn iso_ms(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if s.len() < 19 || s.as_bytes()[10] != b'T' {
        return None;
    }
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let millis = s
        .get(19..)
        .and_then(|r| r.strip_prefix('.'))
        .map(|r| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect::<String>()
        })
        .filter(|f| !f.is_empty())
        .and_then(|f| format!("{f:0<3}").parse::<i64>().ok())
        .unwrap_or(0);
    // The days since 1970 of a civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 24 + h) * 60 + mi) * 60 + sec;
    u64::try_from(ms * 1000 + millis).ok()
}

/// A transcript being read: where it is, how far, and what its lines say.
#[derive(Debug)]
pub struct Source {
    path: PathBuf,
    /// The bytes before this have been read: it is the end of the last whole line.
    offset: u64,
    /// A line that was too long is being skipped: what is read is dropped up to its end.
    skipping: bool,
    pub parser: Parser,
}

impl Source {
    /// The transcript of `session` for the folder `cwd`, when it is there and can be read.
    pub fn open(
        cwd: &Path,
        env: Option<&BTreeMap<String, String>>,
        session: &str,
    ) -> Option<Source> {
        Source::load(Source::path_of(cwd, env, session)?)
    }

    /// Whether the transcript of `session` is there to be read.
    pub fn exists(cwd: &Path, env: Option<&BTreeMap<String, String>>, session: &str) -> bool {
        Source::path_of(cwd, env, session).is_some_and(|p| p.is_file())
    }

    /// The file of the transcript of `session`, for a session id that is a file name.
    pub(crate) fn path_of(
        cwd: &Path,
        env: Option<&BTreeMap<String, String>>,
        session: &str,
    ) -> Option<PathBuf> {
        // A session id is a name of a file: nothing that leaves the folder of transcripts.
        if session.is_empty() || session.contains(['/', '\\', '.']) {
            return None;
        }
        Some(crate::engine::projects_dir(cwd, env)?.join(format!("{session}.jsonl")))
    }

    /// Read the end of the file, as much as it takes for a node's items.
    fn load(path: PathBuf) -> Option<Source> {
        let mut file = std::fs::File::open(&path).ok()?;
        let size = file.metadata().ok()?.len();
        let mut last = None;
        for window in WINDOWS {
            let from = size.saturating_sub(window);
            let (text, end) = read_lines(&mut file, from, size, from > 0)?;
            let parser = parse(&text);
            let enough = from == 0 || parser.items.len() >= KEEP;
            last = Some(Source {
                path: path.clone(),
                offset: end,
                skipping: false,
                parser,
            });
            if enough {
                break;
            }
        }
        last
    }

    /// Read what has been added since: whether what the lines say changed.
    pub fn poll(&mut self) -> bool {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return false;
        };
        let Ok(size) = file.metadata().map(|m| m.len()) else {
            return false;
        };
        // A file that is shorter was written again: read it as new.
        if size < self.offset {
            if let Some(fresh) = Source::load(self.path.clone()) {
                *self = fresh;
                return true;
            }
            return false;
        }
        if size == self.offset {
            return false;
        }
        if self.skipping {
            let to = size.min(self.offset + MOST_ADDED);
            if let Some((next, found)) = skip_line(&mut file, self.offset, to) {
                self.offset = next;
                self.skipping = !found;
            }
            return false;
        }
        // A line that does not end in what is read is read further, until it does or it is too long.
        let mut window = MOST_ADDED;
        loop {
            let to = size.min(self.offset + window);
            let Some((text, end)) = read_lines(&mut file, self.offset, to, false) else {
                return false;
            };
            if end > self.offset {
                self.offset = end;
                let before = self.parser.rev;
                self.parser.feed(&text);
                return self.parser.rev != before;
            }
            // The line is still being written.
            if to == size {
                return false;
            }
            if window >= MAX_LINE {
                self.offset = to;
                self.skipping = true;
                return false;
            }
            window *= 2;
        }
    }
}

/// Where the line that began before `from` ends: the offset after its newline when it is in
/// `from..to`, else `to`, and whether it ended.
fn skip_line(file: &mut std::fs::File, from: u64, to: u64) -> Option<(u64, bool)> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(to - from).read_to_end(&mut bytes).ok()?;
    Some(match bytes.iter().position(|b| *b == b'\n') {
        Some(i) => (from + i as u64 + 1, true),
        None => (from + bytes.len() as u64, false),
    })
}

/// How much is read at a time for a page of older items, the most it grows to, and how far past
/// the cursor it reads so that the results of the page's last tools are found.
const PAGE_CHUNK: u64 = 1 << 20;
const PAGE_MOST: u64 = 64 << 20;
const PAGE_LOOKAHEAD: u64 = 1 << 20;

/// The longest line whose time is looked for when a file is searched by time, and how many lines
/// are tried from one place before it is taken as having none.
const PROBE_LINE: usize = 1 << 20;
const PROBE_LINES: usize = 100_000;
const PROBE_BYTES: u64 = 64 << 20;

/// The bytes of the next line of `r` (at most `max` of them are kept) and how long it was.
fn read_line_capped<R: BufRead>(r: &mut R, max: usize) -> std::io::Result<(Vec<u8>, u64)> {
    let (mut kept, mut total) = (Vec::new(), 0u64);
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok((kept, total));
        }
        let (n, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        let room = max.saturating_sub(kept.len()).min(n);
        kept.extend_from_slice(&buf[..room]);
        total += n as u64;
        r.consume(n);
        if done {
            return Ok((kept, total));
        }
    }
}

/// The start of the first line that begins at or after `mid`, and the time of the first line
/// with one from there.
fn probe(file: &mut std::fs::File, size: u64, mid: u64) -> Option<(u64, u64)> {
    let mut start = mid;
    if mid > 0 {
        file.seek(SeekFrom::Start(mid - 1)).ok()?;
    } else {
        file.seek(SeekFrom::Start(0)).ok()?;
    }
    let mut r = BufReader::new(&mut *file);
    if mid > 0 {
        // Through the end of the line that holds the byte before `mid`.
        let (_, n) = read_line_capped(&mut r, 0).ok()?;
        start = mid - 1 + n;
    }
    for _ in 0..PROBE_LINES {
        if start >= size || start - mid > PROBE_BYTES {
            return None;
        }
        let (bytes, total) = read_line_capped(&mut r, PROBE_LINE).ok()?;
        if total == 0 {
            return None;
        }
        let line_start = start;
        start += total;
        if bytes.len() as u64 == total
            && let Some(at) = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v["timestamp"].as_str().and_then(iso_ms))
        {
            return Some((line_start, at));
        }
    }
    None
}

/// Where the first line made at or after the time `before` starts, or the end of the file: the
/// lines are written in time order, so the file is searched by halves.
fn start_of_time(file: &mut std::fs::File, size: u64, before: u64) -> u64 {
    let (mut lo, mut hi) = (0, size);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match probe(file, size, mid) {
            Some((_, at)) if at < before => lo = mid + 1,
            _ => hi = mid,
        }
    }
    probe(file, size, lo).map_or(size, |(start, _)| start)
}

/// A page of the items made before the time `before`, oldest first, at most `count` of them, and
/// the time to ask with for the page before it (always older than `before`; none when the file
/// holds no more). The lines are read from a chunk of the file before that time and a little past
/// it, so a tool of the page finds its result, and what the lines did not close is closed.
pub(crate) fn older_page(
    path: &Path,
    before: u64,
    count: usize,
) -> Option<(Vec<Stamped>, Option<u64>)> {
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let end = start_of_time(&mut file, size, before);
    let to = (end + PAGE_LOOKAHEAD).min(size);
    let mut chunk = PAGE_CHUNK;
    loop {
        let from = end.saturating_sub(chunk);
        let (text, _) = read_lines(&mut file, from, to, from > 0)?;
        let mut p = parse(&text);
        p.finish();
        let mut items: Vec<Stamped> = p.items.into_iter().filter(|s| s.at < before).collect();
        let whole = from == 0;
        if items.len() >= count || whole || chunk >= PAGE_MOST {
            let more = !whole || items.len() > count;
            let skip = items.len().saturating_sub(count);
            let page: Vec<Stamped> = items.drain(skip..).collect();
            let cursor = if !more {
                None
            } else if let Some(first) = page.first() {
                Some(first.at)
            } else {
                // Nothing in all that was read, and the file goes on back: the next try starts
                // from what was read, which is earlier than `before` since it was read from.
                probe(&mut file, size, from)
                    .map(|(_, at)| at)
                    .filter(|at| *at < before)
            };
            return Some((page, cursor));
        }
        chunk *= 4;
    }
}

/// The whole lines of `file` from `from` to `to`, and the offset where the next one starts. A read
/// that begins in the middle of a line (`skip_first`) starts at the next one, and a last line that
/// is not finished is left for the next read.
fn read_lines(
    file: &mut std::fs::File,
    from: u64,
    to: u64,
    skip_first: bool,
) -> Option<(String, u64)> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(to - from).read_to_end(&mut bytes).ok()?;
    let start = if skip_first {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1)
    } else {
        0
    };
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(start, |i| (i + 1).max(start));
    let text = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    Some((text, from + end as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The items of what was read, without their times.
    fn it(p: &Parser) -> Vec<Item> {
        p.items.iter().map(|s| s.item.clone()).collect()
    }

    fn line(v: Value) -> String {
        v.to_string() + "\n"
    }

    fn sample() -> String {
        [
            line(serde_json::json!({"type":"permission-mode","permissionMode":"acceptEdits"})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"arregla el login"}})),
            line(serde_json::json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}})),
            line(serde_json::json!({"type":"assistant","effort":"high","message":{"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":""}]}})),
            line(serde_json::json!({"type":"assistant","effort":"high","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Voy a leerlo."}]}})),
            line(serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a/login.rs"}}]}})),
            line(serde_json::json!({"type":"user","toolUseResult":{"file":{"numLines":42}},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn login() {}\nfn out() {}"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test"}},{"type":"tool_use","id":"t3","name":"TodoWrite","input":{"todos":[]}}]}})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"error: boom"}]}})),
            line(serde_json::json!({"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5","content":[{"type":"text","text":"de un subagente"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}})),
            line(serde_json::json!({"type":"system","subtype":"compact_boundary"})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"gracias"}]}})),
            "not json\n".to_owned(),
            line(serde_json::json!({"type":"permission-mode","permissionMode":"plan"})),
        ]
        .concat()
    }

    #[test]
    fn the_text_of_claude_in_the_transcript_carries_its_blocks_when_it_is_more_than_paragraphs() {
        let t = parse(
            &[
                line(serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Plano."}]}})),
                line(serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"| a | b |\n|---|---|\n| 1 | 2 |"}]}})),
            ]
            .concat(),
        );
        let got: Vec<bool> = it(&t)
            .iter()
            .filter_map(|i| match i {
                Item::Text { blocks, .. } => Some(blocks.is_some()),
                _ => None,
            })
            .collect();
        assert_eq!(got, [false, true]);
    }

    #[test]
    fn the_person_claude_and_its_tools_come_back_in_order_and_what_is_not_theirs_does_not() {
        let t = parse(&sample());
        let kinds: Vec<&str> = it(&t)
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Text { .. } => "text",
                Item::Tool { .. } => "tool",
                Item::Divider { .. } => "divider",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["user", "text", "tool", "tool", "user", "divider", "user"],
            "no reminder, redacted thinking, checklist, subagent line or synthetic text; the command `/clear` is a line of the person's"
        );
        assert!(matches!(&it(&t)[0], Item::User { text, images: 0 } if text == "arregla el login"));
        assert!(matches!(&it(&t)[6], Item::User { text, .. } if text == "gracias"));
    }

    #[test]
    fn a_tool_takes_the_state_and_the_summary_of_its_result_and_one_without_is_interrupted() {
        let t = parse(&sample());
        assert!(matches!(
            &it(&t)[2],
            Item::Tool { name, state: ToolState::Ok, summary, output, arg, .. }
                if name == "Read" && summary.as_deref() == Some("Leídas 42 líneas")
                    && output.len() == 2 && arg.contains("login.rs")
        ));
        assert!(matches!(
            &it(&t)[3],
            Item::Tool { name, state: ToolState::Failed, summary, .. }
                if name == "Bash" && summary.as_deref() == Some("error: boom")
        ));
        let lone = parse(&line(
            serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":"x","name":"Bash","input":{"command":"sleep 9"}}]}}),
        ));
        assert!(matches!(
            &it(&lone)[0],
            Item::Tool { state: ToolState::Failed, summary, .. } if summary.as_deref() == Some("sin resultado")
        ));
    }

    #[test]
    fn the_last_mode_model_and_effort_are_the_ones_it_ends_with() {
        let t = parse(&sample());
        assert_eq!(t.mode, Some(Mode::Plan), "the last permission-mode line");
        assert_eq!(
            t.model.as_deref(),
            Some("claude-opus-5-5"),
            "not a subagent's model, not <synthetic>"
        );
        assert_eq!(t.effort.as_deref(), Some("high"));
        let odd = parse(&line(
            serde_json::json!({"type":"assistant","effort":"turbo","message":{"model":"m","content":[]}}),
        ));
        assert_eq!(
            odd.effort, None,
            "an effort that --effort does not take is not passed on"
        );
        assert_eq!(parse("").mode, None);
    }

    #[test]
    fn a_message_that_starts_with_an_angle_bracket_is_the_persons_unless_it_is_a_wrapper() {
        let user = |text: &str| {
            line(serde_json::json!({"type":"user","message":{"role":"user","content":text}}))
        };
        let t = parse(
            &[
                user("<div> no se ve"),
                user("<command-name>/x</command-name>"),
                user("<local-command-stdout>ok</local-command-stdout>"),
                user("<bash-input>ls</bash-input>"),
            ]
            .concat(),
        );
        // The command and what it printed are shown; the line of a shell is not.
        assert_eq!(it(&t).len(), 3);
        assert!(matches!(&it(&t)[0], Item::User { text, .. } if text == "<div> no se ve"));
        assert!(matches!(&it(&t)[1], Item::User { text, .. } if text == "/x"));
        assert!(matches!(&it(&t)[2], Item::Out { lines } if lines == &["ok"]));
    }

    #[test]
    fn a_result_finds_its_tool_wherever_it_is_and_one_that_arrives_first_is_dropped() {
        let before = line(
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"late","content":"x"}]}}),
        );
        let tool = line(
            serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":"late","name":"Bash","input":{"command":"ls"}}]}}),
        );
        let t = parse(&(before + &tool));
        assert!(
            matches!(
                &it(&t)[0],
                Item::Tool {
                    state: ToolState::Failed,
                    ..
                }
            ),
            "a result before its tool says nothing of it"
        );
    }

    /// A transcript as long as what is read, with thousands of tools: it is read at once.
    #[test]
    fn a_transcript_of_four_mebibytes_is_read_in_a_blink() {
        let mut text = String::new();
        let mut n = 0;
        while text.len() < WINDOWS[0] as usize {
            n += 1;
            text += &line(
                serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":format!("t{n}"),"name":"Bash","input":{"command":"echo hola"}}]}}),
            );
            text += &line(
                serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{n}"),"content":"hola"}]}}),
            );
        }
        let started = std::time::Instant::now();
        let t = parse(&text);
        assert_eq!(it(&t).len(), n);
        assert!(matches!(
            &it(&t)[n - 1],
            Item::Tool {
                state: ToolState::Ok,
                ..
            }
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_folder_with_an_underscore_or_a_space_finds_its_transcripts_by_either_naming() {
        let config = std::env::temp_dir().join(format!("jwn-transcript-{}", std::process::id()));
        let env: BTreeMap<String, String> = [(
            "CLAUDE_CONFIG_DIR".to_owned(),
            config.to_string_lossy().into_owned(),
        )]
        .into();
        for (cwd, folder) in [
            ("/tmp/jwn_x y", "-tmp-jwn-x-y"),
            ("/tmp/jwn_x y", "-tmp-jwn_x y"),
        ] {
            let dir = config.join("projects").join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("abc.jsonl"),
                line(serde_json::json!({"type":"permission-mode","permissionMode":"plan"})),
            )
            .unwrap();
            let t = Source::open(Path::new(cwd), Some(&env), "abc")
                .unwrap_or_else(|| panic!("{folder}"));
            assert_eq!(t.parser.mode, Some(Mode::Plan));
            std::fs::remove_dir_all(config.join("projects")).unwrap();
        }
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn a_line_has_the_time_it_was_written_at_and_one_without_is_as_late_as_the_last() {
        assert_eq!(iso_ms("2026-10-13T19:52:42.093Z"), Some(1_791_921_162_093));
        assert_eq!(iso_ms("2026-10-13T19:52:42Z"), Some(1_791_921_162_000));
        assert_eq!(iso_ms("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(iso_ms("not a time"), None);
        let t = parse(
            &[
                line(serde_json::json!({"type":"user","timestamp":"2026-10-13T19:52:42.093Z","message":{"role":"user","content":"uno"}})),
                line(serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"text","text":"dos"}]}})),
            ]
            .concat(),
        );
        assert_eq!(
            t.items.iter().map(|i| i.at).collect::<Vec<_>>(),
            [1_791_921_162_093; 2]
        );
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jwn-source-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("s.jsonl")
    }

    fn say(who: &str, text: &str) -> String {
        line(
            serde_json::json!({"type":who,"timestamp":"2026-10-13T19:52:42.093Z","message":{"role":who,"content":[{"type":"text","text":text}],"model":"m"}}),
        )
    }

    /// REQ-545: what is added to the transcript is read when it is whole, and only that.
    #[test]
    fn what_is_added_to_a_transcript_is_read_when_the_line_is_whole() {
        let path = temp("grow");
        std::fs::write(&path, say("assistant", "uno")).unwrap();
        let mut src = Source::load(path.clone()).unwrap();
        assert_eq!(src.parser.items.len(), 1);
        assert!(!src.poll(), "nothing was added");
        // A line that is being written is left until it ends.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let next = say("assistant", "dos");
        std::io::Write::write_all(&mut file, &next.as_bytes()[..20]).unwrap();
        assert!(!src.poll() && src.parser.items.len() == 1);
        std::io::Write::write_all(&mut file, &next.as_bytes()[20..]).unwrap();
        assert!(src.poll());
        assert_eq!(src.parser.items.len(), 2);
        assert!(matches!(&src.parser.items[1].item, Item::Text { text, .. } if text == "dos"));
        // A file that was written again from the start is read again.
        std::fs::write(&path, say("assistant", "otra")).unwrap();
        assert!(src.poll());
        assert_eq!(src.parser.items.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// REQ-546: a line longer than what is read at once is read when it ends, and one that is too long is skipped.
    #[test]
    fn a_line_longer_than_what_is_read_at_once_is_read_whole_and_one_too_long_is_skipped() {
        let path = temp("long");
        std::fs::write(&path, say("assistant", "uno")).unwrap();
        let mut src = Source::load(path.clone()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        // Two kibibytes: more than the 1 KiB read at once in the tests, less than what is too long.
        let long = say("assistant", &"x".repeat(2 << 10));
        std::io::Write::write_all(&mut file, long.as_bytes()).unwrap();
        assert!(src.poll(), "the line was read whole, in a longer read");
        assert_eq!(src.parser.items.len(), 2);
        // Twenty kibibytes: it is skipped, and the line after it is read.
        let huge = say("assistant", &"y".repeat(20 << 10));
        std::io::Write::write_all(&mut file, huge.as_bytes()).unwrap();
        std::io::Write::write_all(&mut file, say("assistant", "después").as_bytes()).unwrap();
        let mut changed = false;
        for _ in 0..40 {
            changed |= src.poll();
        }
        assert!(changed);
        let texts: Vec<String> = src
            .parser
            .items
            .iter()
            .map(|i| match &i.item {
                Item::Text { text, .. } => text.chars().take(7).collect(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            texts,
            ["uno", "xxxxxxx", "después"],
            "the long line is not there and the next one is"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// REQ-549: what another `claude` has typed and not taken in yet is what waits.
    #[test]
    fn a_message_typed_while_a_turn_runs_waits_until_it_is_taken_in() {
        let op = |op: &str, text: Option<&str>| {
            line(serde_json::json!({"type":"queue-operation","operation":op,"content":text}))
        };
        let mut t = parse(
            &[
                op("enqueue", Some("uno")),
                op("enqueue", Some("<task-notification>x</task-notification>")),
                op("enqueue", Some("dos")),
            ]
            .concat(),
        );
        assert_eq!(
            t.waiting(),
            ["uno", "dos"],
            "a task's notice waits and is not shown"
        );
        t.feed(&op("remove", Some("dos")));
        assert_eq!(t.waiting(), ["uno"], "absorbed in the turn that ran");
        t.feed(&op("dequeue", None));
        assert_eq!(
            t.waiting(),
            Vec::<String>::new(),
            "`uno` was first in line and was taken in; the notice that stays is not shown"
        );
        t.feed(&op("dequeue", None));
        assert!(t.waiting().is_empty());
        // The message arriving as a line of the person's is not waiting any more.
        t.feed(&op("enqueue", Some("tres")));
        t.feed(&line(
            serde_json::json!({"type":"user","message":{"role":"user","content":"tres"}}),
        ));
        assert!(t.waiting().is_empty());
        // Nothing waits is a change worth showing.
        let before = t.rev;
        t.feed(&op("enqueue", Some("cuatro")));
        assert_ne!(t.rev, before);
    }

    /// What the other `claude` shows that the plain lines do not carry: the narration between its tools,
    /// a message typed while it worked and the end of a turn.
    #[test]
    fn the_narration_a_message_typed_mid_turn_and_the_end_of_a_turn_are_read() {
        let t = parse(
            &[
                line(serde_json::json!({"type":"user","timestamp":"2026-10-17T11:55:00.000Z","message":{"role":"user","content":"haz esto"}})),
                line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:01.000Z","message":{"model":"m","content":[{"type":"thinking","thinking":"Voy a leer el archivo primero."}]}})),
                line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:02.000Z","message":{"model":"m","content":[{"type":"thinking","thinking":""}]}})),
                line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:03.000Z","message":{"model":"m","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}})),
                line(serde_json::json!({"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-17T11:55:04.000Z","content":"y también esto"})),
                line(serde_json::json!({"type":"attachment","timestamp":"2026-10-17T11:55:05.000Z","attachment":{"type":"queued_command","prompt":"y también esto","origin":{"kind":"human"}}})),
                line(serde_json::json!({"type":"attachment","timestamp":"2026-10-17T11:55:05.500Z","attachment":{"type":"queued_command","prompt":"<task-notification>x</task-notification>","origin":{"kind":"task"}}})),
                line(serde_json::json!({"type":"attachment","timestamp":"2026-10-17T11:55:06.000Z","attachment":{"type":"deferred_tools_record"}})),
                line(serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":"2026-10-17T11:55:09.000Z","durationMs":9000})),
            ]
            .concat(),
        );
        let kinds: Vec<String> = it(&t)
            .iter()
            .map(|i| match i {
                Item::User { text, .. } => format!("user:{text}"),
                Item::Text { text, .. } => format!("text:{text}"),
                Item::Tool { name, .. } => format!("tool:{name}"),
                Item::Result { ms, tools, cost } => format!("result:{ms}:{tools}:{cost}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user:haz esto",
                "text:Voy a leer el archivo primero.",
                "tool:Bash",
                "user:y también esto",
                "result:9000:1:0"
            ],
            "the summary of the thinking is the narration; a message absorbed in the turn is the person's; the end of a turn says how long and how many tools"
        );
        assert!(
            t.waiting().is_empty(),
            "the message that was absorbed is not waiting"
        );
        // The whole thinking of a model that gives it is not shown.
        let long = parse(&line(
            serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"thinking","thinking":"x".repeat(5000)}]}}),
        ));
        assert!(long.items.is_empty());
    }

    /// REQ-553 to REQ-556: what Claude Code shows of its own, that the reader dropped.
    #[test]
    fn what_claude_code_writes_of_its_own_is_hidden_or_shown_as_the_note_it_is() {
        let t = parse(
            &[
                line(serde_json::json!({"type":"user","isCompactSummary":true,"isVisibleInTranscriptOnly":true,"message":{"role":"user","content":"This session is being continued from a previous conversation that ran out of context…"}})),
                line(serde_json::json!({"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: Your computer went to sleep mid-response."}]}})),
                line(serde_json::json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"[Request interrupted by user for tool use]"}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Background command \"cargo test\" completed (exit code 0)</summary>\n</task-notification>"}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"<command-message>commit</command-message>\n<command-name>/commit</command-name>"}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>\n   <command-message>model</command-message>\n   <command-args>sonnet</command-args>"}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"<local-command-caveat>Caveat: the messages below…</local-command-caveat>"}})),
                line(serde_json::json!({"type":"user","message":{"role":"user","content":"<local-command-stdout>\u{1b}[2mCompacted (ctrl+o to see full summary)\u{1b}[22m</local-command-stdout>"}})),
                line(serde_json::json!({"type":"system","subtype":"informational","content":"Automatic replies were not resumed for an artifact.","level":"notice"})),
                line(serde_json::json!({"type":"system","subtype":"local_command","content":"<local-command-stdout>Set model to `Sonnet 5.5` for this session only</local-command-stdout>"})),
            ]
            .concat(),
        );
        let shown: Vec<String> = it(&t)
            .iter()
            .map(|i| match i {
                Item::Note { text, tone } => format!("note[{tone:?}]:{text}"),
                Item::User { text, .. } => format!("user:{text}"),
                Item::Out { lines } => format!("out:{}", lines.join("/")),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "note[Some(Error)]:API Error: Your computer went to sleep mid-response.",
                "note[Some(Warn)]:Interrumpido",
                "note[None]:Background command \"cargo test\" completed (exit code 0)",
                "user:/commit",
                "user:/model sonnet",
                "out:Compacted (ctrl+o to see full summary)",
                "note[None]:Automatic replies were not resumed for an artifact.",
                "out:Set model to `Sonnet 5.5` for this session only",
            ],
            "no compaction summary, no «no response requested», no caveat; the rest as the notes and lines they are"
        );
    }

    #[test]
    fn a_message_with_a_picture_typed_mid_turn_and_the_diff_of_an_edit_are_read() {
        let t = parse(
            &[
                line(serde_json::json!({"type":"attachment","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"[Image #6] mira esto"},{"type":"image","source":{}}],"origin":{"kind":"human"}}})),
                line(serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","old_string":"uno\ndos","new_string":"tres"}}]}})),
            ]
            .concat(),
        );
        assert!(
            matches!(&it(&t)[0], Item::User { text, images: 1 } if text == "[Image #6] mira esto")
        );
        let Item::Tool { diff, .. } = &it(&t)[1] else {
            panic!("the edit");
        };
        assert_eq!(
            diff.len(),
            3,
            "two lines out and one in, as the engine shows an edit: {diff:?}"
        );
    }

    /// REQ-557: what jwn's own engine writes is not made again from the transcript.
    #[test]
    fn what_jwns_own_engine_wrote_is_not_made_again_as_an_item_of_the_transcript() {
        let lines = |entry: &str| {
            [
                line(serde_json::json!({"type":"user","entrypoint":entry,"message":{"role":"user","content":"[Request interrupted by user]"}})),
                line(serde_json::json!({"type":"user","entrypoint":entry,"message":{"role":"user","content":"<command-name>/model</command-name>"}})),
                line(serde_json::json!({"type":"user","entrypoint":entry,"message":{"role":"user","content":"<local-command-stdout>ok</local-command-stdout>"}})),
                line(serde_json::json!({"type":"system","entrypoint":entry,"subtype":"turn_duration","durationMs":5})),
                line(serde_json::json!({"type":"assistant","entrypoint":entry,"isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: x"}]}})),
                line(serde_json::json!({"type":"user","entrypoint":entry,"message":{"role":"user","content":"hola"}})),
            ]
            .concat()
        };
        assert_eq!(
            it(&parse(&lines("cli"))).len(),
            6,
            "Claude Code's own screen wrote them: all are shown"
        );
        let own = it(&parse(&lines("sdk-cli")));
        assert_eq!(
            own.len(),
            1,
            "jwn's engine wrote them: only what the person said is read: {own:?}"
        );
        assert!(matches!(&own[0], Item::User { text, .. } if text == "hola"));
    }

    #[test]
    fn the_colour_codes_and_the_commands_of_a_terminal_are_taken_out_of_what_a_command_printed() {
        assert_eq!(
            strip_ansi("\u{1b}[2mCompacted\u{1b}[22m ok"),
            "Compacted ok"
        );
        assert_eq!(
            strip_ansi("a\u{1b}[3~b"),
            "ab",
            "a sequence that ends with `~` ends there"
        );
        assert_eq!(
            strip_ansi("a\u{1b}]0;titulo\u{7}b"),
            "ab",
            "a command of the system, ended by a bell"
        );
        assert_eq!(
            strip_ansi("a\u{1b}]8;;http://x\u{1b}\\b"),
            "ab",
            "or by ESC and a backslash"
        );
        assert_eq!(strip_ansi("fin\u{1b}"), "fin", "a lone ESC at the end");
    }

    #[test]
    fn lines_that_say_nothing_new_do_not_count_as_a_change() {
        let path = temp("quiet");
        std::fs::write(&path, say("assistant", "uno")).unwrap();
        let mut src = Source::load(path.clone()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let blob = line(serde_json::json!({"type":"api-request","blob":"x"}));
        std::io::Write::write_all(&mut file, blob.as_bytes()).unwrap();
        assert!(!src.poll(), "a line that makes no item is no change");
        std::io::Write::write_all(&mut file, say("assistant", "dos").as_bytes()).unwrap();
        assert!(src.poll());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_transcript_with_few_messages_in_much_text_is_read_further_back_until_the_node_has_its_items()
     {
        let path = temp("window");
        // 100 messages, 5 MiB of lines that say nothing, 100 more: the first window of 4 MiB holds
        // only the last hundred, which are fewer than a node keeps, so the read goes further back.
        let said = |from: usize| -> String {
            (from..from + 100)
                .map(|i| say("assistant", &format!("m{i}")))
                .collect()
        };
        let filler =
            line(serde_json::json!({"type":"api-request","blob":"x".repeat(1 << 20)})).repeat(5);
        std::fs::write(&path, said(0) + &filler + &said(100)).unwrap();
        let src = Source::load(path.clone()).unwrap();
        assert_eq!(src.parser.items.len(), 200);
        assert!(matches!(&src.parser.items[0].item, Item::Text { text, .. } if text == "m0"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_session_id_cannot_name_a_file_outside_the_folder_of_transcripts() {
        for bad in ["", "../x", "a/b", "a\\b", "..", "x.jsonl"] {
            assert!(
                Source::open(Path::new("/tmp"), None, bad).is_none(),
                "{bad:?}"
            );
        }
    }

    fn turn_lines() -> Vec<String> {
        vec![
            line(
                serde_json::json!({"type":"user","timestamp":"2026-10-17T11:55:00.000Z","message":{"role":"user","content":"corre los tests"}}),
            ),
            line(
                serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:02.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"output_tokens":30},"content":[{"type":"text","text":"Voy."}]}}),
            ),
            line(
                serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:03.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"output_tokens":30},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}),
            ),
            line(
                serde_json::json!({"type":"assistant","timestamp":"2026-10-17T11:55:04.000Z","message":{"id":"m2","model":"claude-opus-5-5","usage":{"output_tokens":12},"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/a"}}]}}),
            ),
        ]
    }

    fn states(p: &Parser) -> Vec<(ToolState, Option<String>)> {
        it(p)
            .iter()
            .filter_map(|i| match i {
                Item::Tool { state, summary, .. } => Some((*state, summary.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_turn_that_is_open_runs_its_tools_and_counts_its_tokens_once_for_each_message() {
        let mut p = Parser::default();
        p.feed(&turn_lines().concat());
        assert_eq!(p.turn_since(), iso_ms("2026-10-17T11:55:00.000Z"));
        assert_eq!(p.turn_written(), 42, "m1 once, m2 once");
        assert_eq!(states(&p), [(ToolState::Run, None), (ToolState::Run, None)]);
        p.feed(&line(serde_json::json!({"type":"user","timestamp":"2026-10-17T11:55:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}})));
        assert_eq!(states(&p)[0].0, ToolState::Ok);
        assert_eq!(states(&p)[1].0, ToolState::Run);
    }

    #[test]
    fn the_end_of_the_turn_or_an_interruption_closes_it_and_a_tool_without_result_was_interrupted()
    {
        for end in [
            serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":"2026-10-17T11:56:00.000Z","durationMs":60000}),
            serde_json::json!({"type":"user","timestamp":"2026-10-17T11:56:00.000Z","message":{"role":"user","content":"[Request interrupted by user]"}}),
            serde_json::json!({"type":"system","entrypoint":"sdk-cli","subtype":"turn_duration","timestamp":"2026-10-17T11:56:00.000Z","durationMs":60000}),
        ] {
            let mut p = Parser::default();
            p.feed(&turn_lines().concat());
            p.feed(&line(end));
            assert_eq!(p.turn_since(), None);
            assert_eq!(
                states(&p),
                [
                    (ToolState::Failed, Some("sin resultado".into())),
                    (ToolState::Failed, Some("sin resultado".into()))
                ]
            );
        }
    }

    #[test]
    fn a_new_message_a_compaction_and_an_error_of_the_api_end_a_turn_that_was_left_open() {
        for end in [
            serde_json::json!({"type":"user","timestamp":"2026-10-17T12:00:00.000Z","message":{"role":"user","content":"otra cosa"}}),
            serde_json::json!({"type":"system","subtype":"compact_boundary","timestamp":"2026-10-17T12:00:00.000Z"}),
            serde_json::json!({"type":"assistant","isApiErrorMessage":true,"timestamp":"2026-10-17T12:00:00.000Z","message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: x"}]}}),
        ] {
            let mut p = Parser::default();
            p.feed(&turn_lines().concat());
            p.feed(&line(end));
            assert!(
                states(&p).iter().all(|(s, _)| *s == ToolState::Failed),
                "{:?}",
                states(&p)
            );
        }
    }

    #[test]
    fn a_command_of_claude_does_not_open_a_turn() {
        let mut p = Parser::default();
        p.feed(&line(serde_json::json!({"type":"user","timestamp":"2026-10-17T12:00:00.000Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}})));
        assert_eq!(p.turn_since(), None);
    }

    #[test]
    fn a_turn_that_claude_takes_up_with_no_message_of_the_person_begins_with_the_line_that_made_it()
    {
        let mut p = Parser::default();
        p.feed(&line(serde_json::json!({"type":"user","isMeta":true,"timestamp":"2026-10-17T12:00:00.000Z","origin":{"kind":"peer"},"message":{"role":"user","content":"Another Claude session sent a message: <agent-message from=\"a1\">hola</agent-message>"}})));
        assert!(
            it(&p).iter().all(|i| !matches!(i, Item::User { .. })),
            "a message of another agent is not the person's"
        );
        assert_eq!(p.turn_since(), None, "nothing answered yet");
        p.feed(&line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T12:00:05.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"output_tokens":9},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}})));
        assert_eq!(p.turn_since(), iso_ms("2026-10-17T12:00:00.000Z"));
        assert_eq!(states(&p), [(ToolState::Run, None)]);
        assert_eq!(p.turn_written(), 9);
        p.feed(&line(serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":"2026-10-17T12:00:09.000Z","durationMs":9000})));
        assert_eq!(p.turn_since(), None);
    }

    /// A transcript of `turns` turns, each a message of the person, a text of Claude, a tool with
    /// its result and a text, a second apart; with a line of `fat` bytes of no time in the middle.
    fn paged_file(name: &str, turns: u64, fat: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jwn-older-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let mut out = String::new();
        let at = |n: u64| format!("2026-10-17T10:{:02}:{:02}.000Z", (n / 60) % 60, n % 60);
        for t in 0..turns {
            let b = t * 10;
            out += &line(
                serde_json::json!({"type":"user","timestamp":at(b),"message":{"role":"user","content":format!("pregunta {t}")}}),
            );
            out += &line(
                serde_json::json!({"type":"assistant","timestamp":at(b + 1),"message":{"id":format!("m{t}a"),"model":"claude-opus-5-5","content":[{"type":"text","text":format!("respuesta {t}")}]}}),
            );
            out += &line(
                serde_json::json!({"type":"assistant","timestamp":at(b + 2),"message":{"id":format!("m{t}b"),"model":"claude-opus-5-5","content":[{"type":"tool_use","id":format!("t{t}"),"name":"Bash","input":{"command":"ls"}}]}}),
            );
            if t == turns / 2 && fat > 0 {
                out += &line(serde_json::json!({"type":"api-request-blob","blob":"x".repeat(fat)}));
                // a long run of lines that have no time
                for k in 0..400 {
                    out += &line(serde_json::json!({"type":"progress","n":k}));
                }
            }
            out += &line(
                serde_json::json!({"type":"user","timestamp":at(b + 3),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{t}"),"content":"ok"}]}}),
            );
            out += &line(
                serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":at(b + 4),"durationMs":4000}),
            );
        }
        std::fs::write(&path, out).unwrap();
        path
    }

    fn texts(items: &[Stamped]) -> Vec<String> {
        items
            .iter()
            .filter_map(|s| match &s.item {
                Item::User { text, .. } | Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// REQ-567: the pages, asked for one after the other with the cursor each gave, add up to
    /// the items of the file in order, with none twice and none missing, and the cursor always
    /// goes back in time until the file is out.
    #[test]
    fn the_pages_of_older_items_add_up_to_the_file_and_the_cursor_always_goes_back() {
        for fat in [0, 3 << 20] {
            let path = paged_file(&format!("pages{fat}"), 40, fat);
            let whole = {
                let mut p = parse(&std::fs::read_to_string(&path).unwrap());
                p.finish();
                p.items
            };
            assert!(whole.len() > 100);
            let mut before = u64::MAX;
            let mut got: Vec<Stamped> = Vec::new();
            for _ in 0..200 {
                let (page, cursor) = older_page(&path, before, 17).expect("it reads");
                assert!(page.len() <= 17);
                let mut next = page.clone();
                next.extend(std::mem::take(&mut got));
                got = next;
                match cursor {
                    Some(at) => {
                        assert!(at < before, "the cursor goes back: {at} < {before}");
                        before = at;
                    }
                    None => break,
                }
            }
            assert_eq!(
                got.iter().map(|s| s.item.clone()).collect::<Vec<_>>(),
                whole.iter().map(|s| s.item.clone()).collect::<Vec<_>>(),
                "fat {fat}"
            );
            std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    /// A page cut between a tool and its result still has the tool answered, and a tool that
    /// no line answers was interrupted and does not run.
    #[test]
    fn a_page_cut_inside_a_turn_finds_the_result_of_its_last_tool() {
        let path = paged_file("cut", 6, 0);
        // before the result of the tool of turn 3 (at 33 s): that tool is the last of the page
        let before = iso_ms("2026-10-17T10:00:33.000Z").unwrap();
        let (page, _) = older_page(&path, before, 100).unwrap();
        let tool = page
            .iter()
            .rev()
            .find_map(|s| match &s.item {
                Item::Tool { state, .. } => Some(*state),
                _ => None,
            })
            .expect("a tool");
        assert_eq!(
            tool,
            ToolState::Ok,
            "its result is a few lines past the cursor"
        );
        assert!(texts(&page).iter().all(|t| !t.contains("pregunta 4")));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn nothing_older_than_the_first_line_or_in_a_file_that_is_not_there_is_empty_or_none() {
        let path = paged_file("none", 3, 0);
        let (page, cursor) = older_page(&path, 1, 10).unwrap();
        assert!(page.is_empty() && cursor.is_none());
        assert!(older_page(Path::new("/no/such/file.jsonl"), 10, 10).is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// REQ-570: a consultation of the advisor is a row that runs until its answer, which is a line
    /// of its own a while later, and then says what the advisor did.
    #[test]
    fn a_consultation_of_the_advisor_is_a_row_that_runs_until_the_answer() {
        let mut p = Parser::default();
        p.feed(&line(serde_json::json!({"type":"user","timestamp":"2026-10-17T13:16:00.000Z","message":{"role":"user","content":"revisa esto"}})));
        p.feed(&line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T13:16:49.000Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"server_tool_use","id":"srvtoolu_1","name":"advisor","input":{}}]}})));
        assert_eq!(states(&p), [(ToolState::Run, None)]);
        p.feed(&line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T13:18:17.000Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"advisor_tool_result","tool_use_id":"srvtoolu_1","content":{"type":"advisor_redacted_result","encrypted_content":"x"}}]}})));
        assert_eq!(
            states(&p),
            [(ToolState::Ok, Some(crate::conversation::ADVISED.to_owned()))]
        );
        let Item::Tool { name, arg, .. } = &it(&p)[1] else {
            panic!("a row");
        };
        assert_eq!((name.as_str(), arg.as_str()), ("Advising", ""));
        // an advisor that ran when the turn ended and never answered was interrupted
        let mut q = Parser::default();
        q.feed(&line(serde_json::json!({"type":"user","timestamp":"2026-10-17T13:16:00.000Z","message":{"role":"user","content":"revisa"}})));
        q.feed(&line(serde_json::json!({"type":"assistant","timestamp":"2026-10-17T13:16:49.000Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"server_tool_use","id":"srvtoolu_2","name":"advisor","input":{}}]}})));
        q.feed(&line(serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":"2026-10-17T13:17:00.000Z","durationMs":1000})));
        assert_eq!(states(&q)[0].0, ToolState::Failed);
    }

    /// REQ-571: what another agent sent is neither an item of the conversation nor a message that
    /// waits, in the queue or taken in during a turn; what the person pastes of it is theirs.
    #[test]
    fn a_message_of_another_agent_is_not_shown_and_what_the_person_pastes_of_it_is() {
        let report =
            r#"<agent-message from="a1">[Subagent hand-back] ## Findings …</agent-message>"#;
        let mut p = Parser::default();
        p.feed(&line(serde_json::json!({"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-17T12:00:00.000Z","content":report})));
        assert!(
            p.waiting().is_empty(),
            "it does not wait in the queue the person sees"
        );
        p.feed(&line(serde_json::json!({"type":"attachment","timestamp":"2026-10-17T12:00:01.000Z","attachment":{"type":"queued_command","prompt":report}})));
        assert!(p.queue.is_empty(), "taken in, it does not wait any more");
        p.feed(&line(serde_json::json!({"type":"user","timestamp":"2026-10-17T12:00:02.000Z","message":{"role":"user","content":format!("Another Claude session sent a message: {report}")}})));
        assert!(
            it(&p).iter().all(|i| !matches!(i, Item::User { .. })),
            "{:?}",
            it(&p)
        );
        let pasted = format!(
            "<pasted_content id=\"1\">{report}</pasted_content> más texto que no debería salir"
        );
        p.feed(&line(serde_json::json!({"type":"attachment","timestamp":"2026-10-17T12:00:03.000Z","attachment":{"type":"queued_command","prompt":pasted,"origin":{"kind":"human"}}})));
        assert!(
            it(&p)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text.contains("más texto")))
        );
    }
}
