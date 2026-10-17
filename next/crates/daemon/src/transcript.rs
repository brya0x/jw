//! A conversation the engine already had, read from its own transcript when the person resumes it
//! (`claude --resume <id>`, `-c`, `/resume`): what was said and the mode, the model and the effort
//! it ended with (spec REQ-540 to REQ-543). The transcript is
//! `~/.claude/projects/<folder>/<session>.jsonl`, one JSON line for each message or note; the
//! engine does not send any of it again when it resumes.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use contract::conversation::{DividerKind, Item, Mode, ToolState};
use serde_json::Value;

use crate::conversation::{LINE_CHARS, OUTPUT_LINES, clip, content_text, summary_of, tool_arg};

/// The most of the end of a transcript that is read: a long session is megabytes of lines, and the
/// node keeps its last items only (REQ-392).
const TAIL: u64 = 4 * 1024 * 1024;

/// The efforts `--effort` takes; anything else in a transcript is not passed on.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// What a transcript says of a conversation.
#[derive(Debug, Default, PartialEq)]
pub struct Transcript {
    pub items: Vec<Item>,
    /// The mode it was in last.
    pub mode: Option<Mode>,
    /// The model of Claude's last message, as the engine names it.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The transcript of `session` for the folder `cwd`, when it is there and can be read.
pub fn read(
    cwd: &Path,
    env: Option<&BTreeMap<String, String>>,
    session: &str,
) -> Option<Transcript> {
    // A session id is a name of a file: nothing that leaves the folder of transcripts.
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return None;
    }
    let path = crate::engine::projects_dir(cwd, env)?.join(format!("{session}.jsonl"));
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let cut = size > TAIL;
    if cut {
        file.seek(SeekFrom::Start(size - TAIL)).ok()?;
    }
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // A read that began in the middle of a line starts at the next one.
    let text = match (cut, text.split_once('\n')) {
        (true, Some((_, rest))) => rest,
        _ => &text,
    };
    Some(parse(text))
}

/// The lines of a transcript as a conversation: the person's messages, Claude's text, its tools
/// with what they answered, the compactions, and the last mode, model and effort.
pub fn parse(text: &str) -> Transcript {
    let mut out = Transcript::default();
    // Where each tool is in `items`, so a result finds its tool without a search for each one.
    let mut tools: HashMap<String, usize> = HashMap::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // What a subagent said inside a tool is not a line of the conversation.
        if entry["isSidechain"] == true {
            continue;
        }
        match entry["type"].as_str().unwrap_or("") {
            "permission-mode" => {
                if let Some(mode) = entry["permissionMode"].as_str().and_then(Mode::from_flag) {
                    out.mode = Some(mode);
                }
            }
            "assistant" => assistant(&entry, &mut out, &mut tools),
            "user" if entry["isMeta"] != true => user(&entry, &mut out, &tools),
            "system" if entry["subtype"] == "compact_boundary" => out.items.push(Item::Divider {
                text: "conversación compactada".into(),
                kind: DividerKind::Compact,
            }),
            _ => {}
        }
    }
    out
}

fn assistant(entry: &Value, out: &mut Transcript, tools: &mut HashMap<String, usize>) {
    let message = &entry["message"];
    // What Claude Code writes itself (an interrupted turn, «no response requested») is not Claude.
    if message["model"] == "<synthetic>" {
        return;
    }
    if let Some(model) = message["model"].as_str().filter(|m| !m.is_empty()) {
        out.model = Some(model.to_owned());
    }
    if let Some(effort) = entry["effort"].as_str().filter(|e| EFFORTS.contains(e)) {
        out.effort = Some(effort.to_owned());
    }
    for block in message["content"].as_array().into_iter().flatten() {
        match block["type"].as_str().unwrap_or("") {
            "text" => {
                let text = block["text"].as_str().unwrap_or("");
                if !text.trim().is_empty() {
                    out.items.push(Item::Text {
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
                tools.insert(id.clone(), out.items.len());
                out.items.push(Item::Tool {
                    id,
                    name: name.to_owned(),
                    arg: tool_arg(name, &block["input"]),
                    // Until its result is read: a tool with none was interrupted.
                    state: ToolState::Failed,
                    summary: Some("sin resultado".into()),
                    output: Vec::new(),
                    diff: Vec::new(),
                    url: None,
                    by: None,
                });
            }
            _ => {}
        }
    }
}

fn user(entry: &Value, out: &mut Transcript, tools: &HashMap<String, usize>) {
    let content = &entry["message"]["content"];
    let blocks: Vec<&Value> = match content {
        Value::Array(a) => a.iter().collect(),
        _ => Vec::new(),
    };
    for block in &blocks {
        if block["type"] == "tool_result" {
            result(block, &entry["toolUseResult"], out, tools);
        }
    }
    let (text, images) = match content {
        Value::String(s) => (s.clone(), 0),
        _ => (
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            u32::try_from(blocks.iter().filter(|b| b["type"] == "image").count())
                .unwrap_or(u32::MAX),
        ),
    };
    if is_the_persons(&text) || (text.trim().is_empty() && images > 0) {
        out.items.push(Item::User {
            text: text.trim().to_owned(),
            images,
        });
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

/// What a tool answered: its state, a line that says it and the first lines of its output.
fn result(block: &Value, detail: &Value, out: &mut Transcript, tools: &HashMap<String, usize>) {
    let Some(at) = block["tool_use_id"]
        .as_str()
        .and_then(|id| tools.get(id))
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
    if let Some(Item::Tool {
        name,
        state,
        summary,
        output,
        ..
    }) = out.items.get_mut(at)
    {
        *state = if failed {
            ToolState::Failed
        } else {
            ToolState::Ok
        };
        *summary = Some(summary_of(name, &lines, detail, failed));
        *output = lines;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string() + "\n"
    }

    fn sample() -> String {
        [
            line(serde_json::json!({"type":"permission-mode","permissionMode":"acceptEdits"})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"arregla el login"}})),
            line(serde_json::json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}})),
            line(serde_json::json!({"type":"assistant","effort":"high","message":{"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"hm"}]}})),
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
        let kinds: Vec<&str> = t
            .items
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
            ["user", "text", "tool", "tool", "divider", "user"],
            "no reminder, thinking, checklist, subagent line, synthetic text or command wrapper"
        );
        assert!(
            matches!(&t.items[0], Item::User { text, images: 0 } if text == "arregla el login")
        );
        assert!(matches!(&t.items[5], Item::User { text, .. } if text == "gracias"));
    }

    #[test]
    fn a_tool_takes_the_state_and_the_summary_of_its_result_and_one_without_is_interrupted() {
        let mut t = parse(&sample());
        let read = t.items.remove(2);
        assert!(matches!(
            read,
            Item::Tool { ref name, state: ToolState::Ok, ref summary, ref output, ref arg, .. }
                if name == "Read" && summary.as_deref() == Some("Leídas 42 líneas")
                    && output.len() == 2 && arg.contains("login.rs")
        ));
        assert!(matches!(
            &t.items[2],
            Item::Tool { name, state: ToolState::Failed, summary, .. }
                if name == "Bash" && summary.as_deref() == Some("error: boom")
        ));
        let lone = parse(&line(
            serde_json::json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":"x","name":"Bash","input":{"command":"sleep 9"}}]}}),
        ));
        assert!(matches!(
            &lone.items[0],
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
        assert_eq!(t.items.len(), 1);
        assert!(matches!(&t.items[0], Item::User { text, .. } if text == "<div> no se ve"));
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
                &t.items[0],
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
        while text.len() < TAIL as usize {
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
        assert_eq!(t.items.len(), n);
        assert!(matches!(
            &t.items[n - 1],
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
            let t = read(Path::new(cwd), Some(&env), "abc").unwrap_or_else(|| panic!("{folder}"));
            assert_eq!(t.mode, Some(Mode::Plan));
            std::fs::remove_dir_all(config.join("projects")).unwrap();
        }
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn a_session_id_cannot_name_a_file_outside_the_folder_of_transcripts() {
        for bad in ["", "../x", "a/b", "a\\b", "..", "x.jsonl"] {
            assert!(read(Path::new("/tmp"), None, bad).is_none(), "{bad:?}");
        }
    }
}
