//! A Claude agent's conversation as blocks, read from the transcript Claude
//! writes as it goes (`~/.claude/projects/<folder>/<session-id>.jsonl`, one
//! JSON entry a line): what a phone draws instead of the agent's screen
//! (mobile REQ-90). The file is Claude's, not jw's: every shape here is read
//! leniently and an entry that is not understood is skipped, never an error.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::proto::ChatBlock;

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
    let named: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let direct = projects.join(named).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
}

/// What a read of the file changed: the conversation's title when it did, and
/// the blocks that are new or changed (a tool's block changes when its result
/// comes).
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub title: Option<String>,
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
        }
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

    /// The last `n` blocks, for a client that starts watching.
    pub fn last(&self, n: usize) -> Vec<ChatBlock> {
        self.blocks[self.blocks.len().saturating_sub(n)..].to_vec()
    }

    /// Reads what was appended since the last read.
    pub fn poll(&mut self) -> Changes {
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
            return self.poll();
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
        let mut touched: Vec<u32> = Vec::new();
        for line in data[..whole].split(|b| *b == b'\n') {
            if let Ok(line) = std::str::from_utf8(line) {
                self.apply(line, &mut touched);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        Changes {
            title: (self.title != before_title)
                .then(|| self.title.clone())
                .flatten(),
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
        match v["type"].as_str() {
            Some("ai-title") => {
                if let Some(t) = v["aiTitle"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.title = Some(cut(t.trim(), ARG_MAX));
                }
            }
            Some(kind @ ("user" | "assistant")) => {
                // A subagent's own conversation, and what Claude wrote for
                // itself (hook output, caveats), is not the person's chat.
                if v["isSidechain"].as_bool() == Some(true)
                    || (kind == "user" && v["isMeta"].as_bool() == Some(true))
                {
                    return;
                }
                let content = &v["message"]["content"];
                if let Some(text) = content.as_str() {
                    self.text_block(kind, text, touched);
                    return;
                }
                let Some(parts) = content.as_array() else {
                    return;
                };
                for part in parts {
                    match part["type"].as_str() {
                        Some("text") => {
                            self.text_block(kind, part["text"].as_str().unwrap_or(""), touched)
                        }
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

    fn text_block(&mut self, role: &str, text: &str, touched: &mut Vec<u32>) {
        if text.trim().is_empty() {
            return;
        }
        let b = ChatBlock {
            kind: if role == "user" { "user" } else { "text" }.into(),
            text: cut(text.trim(), TEXT_MAX),
            ..ChatBlock::default()
        };
        self.push(b, touched);
    }

    fn tool_use(&mut self, part: &Value, touched: &mut Vec<u32>) {
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
    fn the_transcript_is_found_by_the_folder_name_or_by_looking() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the tests of this module are the only ones that set it, and they run one after another here.
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
}
