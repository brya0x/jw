//! A conversation the engine already has, read from its own transcript instead of copied
//! (spec REQ-540 to REQ-546): what was said, with the time of each line, and the mode, the model and
//! the effort it ended with. The transcript is `~/.claude/projects/<folder>/<session>.jsonl`, one
//! JSON line for each message or note, and it only grows: a `Source` reads the end of it when a
//! conversation is opened and then the lines that are added, so a session another `claude` is
//! writing is followed. The engine does not send any of it again when it resumes.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
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
}

impl Parser {
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
                "system" if self.own && entry["subtype"] != "compact_boundary" => {}
                "system" if entry["subtype"] == "informational" => {
                    if let Some(text) = entry["content"].as_str().filter(|t| !t.trim().is_empty()) {
                        self.note(clip(text.trim(), LINE_CHARS), None);
                    }
                }
                "system" if entry["subtype"] == "local_command" => {
                    self.command_output(entry["content"].as_str().unwrap_or(""));
                }
                "system" if entry["subtype"] == "compact_boundary" => self.push(Item::Divider {
                    text: "conversación compactada".into(),
                    kind: DividerKind::Compact,
                }),
                "system" if entry["subtype"] == "turn_duration" => {
                    let ms = entry["durationMs"].as_u64().unwrap_or(0);
                    let tools = std::mem::take(&mut self.tools_in_turn);
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
        if is_the_persons(&text) || (text.trim().is_empty() && images > 0) {
            // It does not begin a turn: the one that runs takes it in, and its tools go on counting.
            self.unqueue(&text);
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
                self.queue.push((text.to_owned(), is_the_persons(text)));
                self.rev += 1;
            }
            // The first one was taken in, whatever it said.
            ("dequeue", _) if !self.queue.is_empty() => {
                self.queue.remove(0);
                self.rev += 1;
            }
            ("remove", Some(text)) => self.unqueue(text),
            _ => {}
        }
    }

    /// The first message that waits with this text is not waiting any more.
    fn unqueue(&mut self, text: &str) {
        if let Some(at) = self.queue.iter().position(|(t, _)| t.trim() == text.trim()) {
            self.queue.remove(at);
            self.rev += 1;
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
            if entry["isApiErrorMessage"] == true && !self.own {
                for block in message["content"].as_array().into_iter().flatten() {
                    if let Some(text) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        self.note(clip(text.trim(), LINE_CHARS), Some(Tone::Error));
                    }
                }
            }
            return;
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
                    self.push(Item::Tool {
                        id,
                        name: name.to_owned(),
                        arg: tool_arg(name, &block["input"]),
                        // Until its result is read: a tool with none was interrupted.
                        state: ToolState::Failed,
                        summary: Some("sin resultado".into()),
                        output: Vec::new(),
                        diff: edit_diff(name, &block["input"]),
                        url: None,
                        by: None,
                    });
                }
                // What Claude says between its tools is a summary of its thinking: the lines that
                // are short are shown as the narration they are, and the whole thinking is not.
                "thinking" => {
                    let text = block["thinking"].as_str().unwrap_or("").trim();
                    if !text.is_empty() && text.chars().count() <= SUMMARY_CHARS {
                        self.push(Item::Text {
                            text: text.to_owned(),
                            live: false,
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
        } else if t.starts_with("[Request interrupted") {
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
/// of its output, of a shell line and of the notice of a background task. A message that merely
/// starts with `<` (a question about `<div>`) is the person's.
const WRAPPERS: [&str; 5] = [
    "<command-",
    "<local-command-",
    "<bash-",
    "<task-notification",
    "<system-reminder",
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
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // `ESC [` and what follows up to a letter.
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
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
    fn path_of(
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
}
