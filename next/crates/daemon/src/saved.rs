//! The conversations Claude saved for a folder, for `/resume` (spec REQ-426): the transcripts in
//! its `projects/` folder, newest first, each with the name it has or the first thing said in it.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use contract::conversation::{ConversationList, SavedConversation};
use contract::{ErrorCode, Id};
use serde_json::{Value, json};

use crate::commands::{Done, err};
use crate::server::Shared;

/// How many conversations the list holds.
const MOST: usize = 50;
/// How much of a transcript is read from its start (the first message) and from its end (the
/// titles Claude appends), so that a transcript of many megabytes costs what a small one does.
const HEAD: u64 = 64 * 1024;
const TAIL: u64 = 128 * 1024;
const NAME_CHARS: usize = 80;

pub fn list(shared: &Shared, workspace: &Id) -> Done {
    // Claude's folder is where the daemon was told it is (`env`), as for the engine it runs.
    let (dir, env) = {
        let core = shared.lock();
        let dir = core
            .workspace_dir(workspace)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
        (dir, core.env.iter().cloned().collect::<BTreeMap<_, _>>())
    };
    let conversations = saved_conversations(&dir, Some(&env));
    Ok(Some(json!(ConversationList { conversations })))
}

/// The conversations saved for `cwd`, newest first.
pub fn saved_conversations(
    cwd: &Path,
    env: Option<&BTreeMap<String, String>>,
) -> Vec<SavedConversation> {
    let Some(dir) = crate::engine::projects_dir(cwd, env) else {
        return Vec::new();
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .filter_map(|(modified, path)| {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let name = describe(&path, &id)?;
            let at = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            Some(SavedConversation { id, name, at })
        })
        .take(MOST)
        .collect()
}

/// A transcript's name: the one the person gave it, else Claude's title for it, else the first
/// thing the person said; none when nothing was said in it.
fn describe(path: &Path, id: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let head = read_at(&mut file, 0, HEAD.min(len))?;
    let tail = if len > HEAD {
        read_at(
            &mut file,
            len.saturating_sub(TAIL).max(HEAD),
            TAIL.min(len - HEAD),
        )?
    } else {
        String::new()
    };
    let (mut first, mut custom, mut ai) = (None, None, None);
    for line in head.lines().chain(tail.lines()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(str::to_owned),
            Some("ai-title") => ai = v["aiTitle"].as_str().map(str::to_owned),
            Some("user") if first.is_none() && v["isSidechain"] != true => {
                first = said(&v["message"]["content"]);
            }
            _ => {}
        }
    }
    let name = custom.or(ai).or(first)?;
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Some(id.chars().take(8).collect());
    }
    Some(clip(&name))
}

/// What the person typed in a `user` message: not the commands and reminders Claude's own
/// machinery writes in the same place.
fn said(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().find_map(|b| {
            (b["type"] == "text")
                .then(|| b["text"].as_str().map(str::to_owned))
                .flatten()
        })?,
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")).then(|| t.to_owned())
}

fn read_at(file: &mut std::fs::File, from: u64, n: u64) -> Option<String> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.take(n).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    // A read that starts in the middle of a line starts at the next one.
    Some(if from > 0 {
        text.split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_owned())
    } else {
        text
    })
}

fn clip(s: &str) -> String {
    if s.chars().count() <= NAME_CHARS {
        return s.to_owned();
    }
    s.chars().take(NAME_CHARS - 1).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(dir: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        )])
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "{}-saved-{}-{name}",
            identity::get().name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(config: &Path, cwd: &Path, id: &str, lines: &[Value], age: u64) {
        let dir = crate::engine::projects_dir(cwd, Some(&env_of(config))).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, body).unwrap();
        let time = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    fn user(text: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": text}})
    }

    #[test]
    fn the_conversations_of_a_folder_come_newest_first_each_with_its_name() {
        let config = scratch("list");
        let cwd = Path::new("/work/app.one");
        put(&config, cwd, "old", &[user("arregla el login")], 3600);
        put(
            &config,
            cwd,
            "new",
            &[
                json!({"type": "user", "message": {"content": [{"type": "text", "text": "hola"}]}}),
                json!({"type": "ai-title", "aiTitle": "Saludo"}),
            ],
            10,
        );
        put(
            &config,
            cwd,
            "named",
            &[
                user("x"),
                json!({"type": "ai-title", "aiTitle": "Título de Claude"}),
                json!({"type": "custom-title", "customTitle": "el mío"}),
            ],
            600,
        );
        let list = saved_conversations(cwd, Some(&env_of(&config)));
        let shown: Vec<(&str, &str)> = list
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("new", "Saludo"),
                ("named", "el mío"),
                ("old", "arregla el login")
            ],
            "the name the person gave wins, then Claude's title, then the first message"
        );
        assert!(list[0].at > list[1].at && list[1].at > list[2].at);
        // another folder has none
        assert!(saved_conversations(Path::new("/work/other"), Some(&env_of(&config))).is_empty());
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn what_claudes_own_machinery_wrote_is_not_the_first_message_and_an_empty_transcript_is_not_listed()
     {
        let config = scratch("noise");
        let cwd = Path::new("/work/noise");
        put(
            &config,
            cwd,
            "cmds",
            &[
                user("<command-name>/init</command-name>"),
                user("Caveat: the messages below were generated by the user"),
                user("  lo   que\n sí  dije  "),
            ],
            5,
        );
        put(
            &config,
            cwd,
            "empty",
            &[json!({"type": "summary", "summary": "x"})],
            4,
        );
        put(
            &config,
            cwd,
            "blank",
            &[
                user("<local-command-stdout></local-command-stdout>"),
                json!({"type": "last-prompt"}),
            ],
            3,
        );
        let list = saved_conversations(cwd, Some(&env_of(&config)));
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["lo que sí dije"], "{list:?}");
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn a_long_transcript_gives_its_title_from_the_end_and_its_message_from_the_start() {
        let config = scratch("long");
        let cwd = Path::new("/work/long");
        let mut lines = vec![user("la primera cosa")];
        for n in 0..4000 {
            lines.push(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": format!("relleno {n} {}", "x".repeat(80))}]}}));
        }
        put(&config, cwd, "big", &lines, 5);
        let list = saved_conversations(cwd, Some(&env_of(&config)));
        assert_eq!(
            list[0].name, "la primera cosa",
            "no title: the first message"
        );
        lines.push(json!({"type": "custom-title", "customTitle": "al final"}));
        put(&config, cwd, "big", &lines, 5);
        assert_eq!(
            saved_conversations(cwd, Some(&env_of(&config)))[0].name,
            "al final"
        );
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn only_fifty_are_listed_and_a_long_name_is_cut() {
        let config = scratch("many");
        let cwd = Path::new("/work/many");
        for n in 0..60u64 {
            put(
                &config,
                cwd,
                &format!("s{n:02}"),
                &[user(&"largo ".repeat(40))],
                n + 1,
            );
        }
        let list = saved_conversations(cwd, Some(&env_of(&config)));
        assert_eq!(list.len(), 50);
        assert_eq!(list[0].id, "s00");
        assert_eq!(list[0].name.chars().count(), 80);
        assert!(list[0].name.ends_with('…'));
        let _ = std::fs::remove_dir_all(config);
    }
}
