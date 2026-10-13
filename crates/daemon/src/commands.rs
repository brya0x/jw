//! What a phone can type after `/` in a Claude conversation (mobile REQ-101): Claude's own commands, the ones in
//! `<config>/commands` and in the folder's `.claude/commands`, and the skills in `<config>/skills` and the folder's
//! `.claude/skills`. The files are Claude's and the user's, not jw's: every shape is read leniently and a file that
//! cannot be read is left out, never an error. Commands that plugins or MCP servers add are not listed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::chat::config_dir;
use crate::proto::CommandInfo;

/// Claude's own commands. The phone has their words.
const BUILTIN: &[&str] = &[
    "add-dir",
    "agents",
    "clear",
    "compact",
    "config",
    "context",
    "cost",
    "doctor",
    "export",
    "help",
    "hooks",
    "init",
    "login",
    "logout",
    "mcp",
    "memory",
    "model",
    "permissions",
    "plan",
    "release-notes",
    "resume",
    "review",
    "rewind",
    "status",
    "theme",
    "usage",
    "vim",
];
/// The most things listed, and the most characters of a description.
const MAX_ITEMS: usize = 400;
const DESC_MAX: usize = 200;
/// The most folders walked, for a folder far from any `.claude`.
const MAX_DEPTH: usize = 6;

/// Everything offered after `/` for a Claude that runs in `cwd`, by name: a project's own wins over a user's, and over Claude's.
pub fn list(cwd: Option<&Path>) -> Vec<CommandInfo> {
    let mut found: BTreeMap<String, CommandInfo> = BTreeMap::new();
    let mut put = |name: String, desc: String, kind: &str| {
        found.insert(
            name.clone(),
            CommandInfo {
                name: format!("/{name}"),
                desc,
                kind: kind.into(),
            },
        );
    };
    for b in BUILTIN {
        put((*b).into(), String::new(), "claude");
    }
    if let Some(cfg) = config_dir() {
        for (n, d) in commands_in(&cfg.join("commands")) {
            put(n, d, "user");
        }
        for (n, d) in skills_in(&cfg.join("skills")) {
            put(n, d, "skill");
        }
    }
    // The folder and the ones above it, the nearest last so that it wins.
    let mut folders: Vec<PathBuf> = cwd
        .map(|c| {
            c.ancestors()
                .take(MAX_DEPTH)
                .map(Path::to_path_buf)
                .collect()
        })
        .unwrap_or_default();
    folders.reverse();
    for f in folders {
        for (n, d) in commands_in(&f.join(".claude/commands")) {
            put(n, d, "project");
        }
        for (n, d) in skills_in(&f.join(".claude/skills")) {
            put(n, d, "skill");
        }
    }
    found.into_values().take(MAX_ITEMS).collect()
}

/// The `.md` files under `dir` as commands: `a/b.md` is `a:b`.
fn commands_in(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, String)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if path.is_dir() {
            if depth < 3 {
                walk(root, &path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|x| x == "md") {
            let Ok(rel) = path
                .with_extension("")
                .strip_prefix(root)
                .map(Path::to_path_buf)
            else {
                continue;
            };
            let name = rel
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join(":");
            let text = fs::read_to_string(&path).unwrap_or_default();
            let (front, body) = split_front(&text);
            let desc = front_value(front, "description").unwrap_or_else(|| first_line(body));
            if !name.is_empty() {
                out.push((name, cut(&desc)));
            }
        }
    }
}

/// The folders under `dir` that hold a `SKILL.md`, by the name it gives or by the folder's.
fn skills_in(dir: &Path) -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path().join("SKILL.md");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let (front, _) = split_front(&text);
        let Some(name) =
            front_value(front, "name").or_else(|| e.file_name().to_str().map(str::to_string))
        else {
            continue;
        };
        out.push((
            name,
            cut(&front_value(front, "description").unwrap_or_default()),
        ));
    }
    out
}

/// A file as its front matter (between the `---` lines it starts with) and the rest.
fn split_front(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    match rest.find("\n---") {
        Some(end) => (&rest[..end], rest[end + 4..].trim_start_matches('\n')),
        None => ("", text),
    }
}

/// The value of `key:` in a front matter: on its line, or (when the line holds only `>` or `|`) the indented lines under it.
fn front_value(front: &str, key: &str) -> Option<String> {
    let mut lines = front.lines();
    while let Some(l) = lines.next() {
        let Some(v) = l.strip_prefix(key).and_then(|r| r.strip_prefix(':')) else {
            continue;
        };
        let v = v.trim();
        if v.is_empty() || v == ">" || v == "|" || v == ">-" || v == "|-" {
            let rest: Vec<&str> = lines
                .clone()
                .take_while(|l| l.starts_with(' ') || l.starts_with('\t'))
                .map(str::trim)
                .collect();
            let joined = rest.join(" ");
            return (!joined.is_empty()).then_some(joined);
        }
        return Some(v.trim_matches(|c| c == '"' || c == '\'').to_string());
    }
    None
}

fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn cut(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= DESC_MAX {
        return one;
    }
    one.chars().take(DESC_MAX - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::chat::CONFIG_ENV as CONFIG;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_commands_and_skills_of_the_user_and_the_folder_are_listed_with_their_words() {
        let _g = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        write(
            &cfg.path().join("commands/commit.md"),
            "---\ndescription: Commits it all\n---\nRun git status",
        );
        write(
            &cfg.path().join("commands/team/ship.md"),
            "# Ship it\n\nSteps",
        );
        write(
            &cfg.path().join("skills/jw/SKILL.md"),
            "---\nname: jw\ndescription: >\n  How to work\n  in a jw workspace\n---\nBody",
        );
        write(
            &proj.path().join(".claude/commands/commit.md"),
            "---\ndescription: \"The project's own\"\n---\n",
        );
        write(
            &proj.path().join(".claude/skills/local/SKILL.md"),
            "---\ndescription: A local one\n---\n",
        );
        write(
            &proj.path().join(".claude/commands/notes.txt"),
            "not a command",
        );
        // SAFETY: the tests that set this variable take turns on CONFIG.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", cfg.path()) };
        let items = list(Some(proj.path()));
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        let by = |n: &str| {
            items
                .iter()
                .find(|c| c.name == n)
                .map(|c| (c.desc.as_str(), c.kind.as_str()))
        };
        // The project's own wins over the user's of the same name.
        assert_eq!(by("/commit"), Some(("The project's own", "project")));
        assert_eq!(by("/team:ship"), Some(("Ship it", "user")));
        assert_eq!(by("/jw"), Some(("How to work in a jw workspace", "skill")));
        assert_eq!(by("/local"), Some(("A local one", "skill")));
        assert_eq!(by("/model"), Some(("", "claude")));
        assert_eq!(by("/notes"), None);
        // By name.
        let names: Vec<&str> = items.iter().map(|c| c.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn a_folder_with_nothing_still_has_claudes_own_commands() {
        let _g = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", cfg.path()) };
        let items = list(None);
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        assert!(
            items.iter().all(|c| c.kind == "claude") && items.iter().any(|c| c.name == "/help")
        );
    }
}
