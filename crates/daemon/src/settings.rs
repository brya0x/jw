//! What Claude's own settings say that the conversation shows: the model the advisor runs on
//! (spec REQ-574). Read once when the conversation is opened.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::conversation::short_model;

/// Claude's own folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn claude_dir(env: Option<&BTreeMap<String, String>>) -> Option<PathBuf> {
    env.and_then(|e| e.get("CLAUDE_CONFIG_DIR").cloned())
        .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
        .map(PathBuf::from)
        .or_else(|| {
            env.and_then(|e| e.get("HOME").cloned())
                .or_else(|| std::env::var("HOME").ok())
                .map(|h| PathBuf::from(h).join(".claude"))
        })
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The model of the advisor as Claude Code names it (`Opus 5.5`): the `advisorModel` of the
/// folder's `.claude/settings.local.json`, else its `.claude/settings.json`, else Claude's own
/// `settings.json`. An alias (`opus`) is the newest model of that name among the ones the
/// settings list in `modelSettings`, and just its name when none is listed.
pub(crate) fn advisor_label(env: Option<&BTreeMap<String, String>>, cwd: &Path) -> Option<String> {
    let user = claude_dir(env).map(|d| d.join("settings.json"));
    let files: Vec<Value> = [
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
    ]
    .into_iter()
    .chain(user)
    .filter_map(|p| read(&p))
    .collect();
    let setting = files
        .iter()
        .find_map(|f| f["advisorModel"].as_str().filter(|m| !m.is_empty()))?;
    let known: Vec<&str> = files
        .iter()
        .filter_map(|f| f["modelSettings"].as_object())
        .flat_map(|m| m.keys().map(String::as_str))
        .collect();
    Some(label_of(setting, &known))
}

/// `opus-5.5` as `Opus 5.5`.
fn pretty(short: &str) -> String {
    let (name, version) = short.split_once('-').unwrap_or((short, ""));
    let mut name = name.to_owned();
    if let Some(first) = name.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if version.is_empty() {
        name
    } else {
        format!("{name} {version}")
    }
}

fn version_of(short: &str) -> Vec<u32> {
    short
        .split_once('-')
        .map(|(_, v)| v.split('.').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default()
}

fn label_of(setting: &str, known: &[&str]) -> String {
    if setting.starts_with("claude-") {
        return pretty(&short_model(setting));
    }
    let alias = setting.to_lowercase();
    known
        .iter()
        .filter(|k| k.starts_with(&format!("claude-{alias}-")))
        .map(|k| short_model(k))
        .max_by_key(|k| version_of(k))
        .map_or_else(|| pretty(&alias), |k| pretty(&k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-574: an alias is the newest model of that name that the settings list, a full id is
    /// itself, and an alias with none listed is its name.
    #[test]
    fn the_model_of_the_advisor_is_named_as_claude_code_names_it() {
        let known = [
            "claude-sonnet-5",
            "claude-opus-5-1",
            "claude-opus-5-5",
            "claude-haiku-5-5",
        ];
        assert_eq!(label_of("opus", &known), "Opus 5.5");
        assert_eq!(label_of("Sonnet", &known), "Sonnet 5");
        assert_eq!(label_of("claude-opus-5-1", &known), "Opus 5.1");
        assert_eq!(label_of("opus", &[]), "Opus");
    }

    #[test]
    fn the_folders_settings_come_before_claudes_own() {
        let dir = std::env::temp_dir().join(format!("jwn-settings-{}", std::process::id()));
        let (claude, proj) = (dir.join("claude"), dir.join("proj"));
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(proj.join(".claude")).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"advisorModel":"opus","modelSettings":{"claude-opus-5-5":{}}}"#,
        )
        .unwrap();
        let env: BTreeMap<String, String> = [(
            "CLAUDE_CONFIG_DIR".to_owned(),
            claude.to_string_lossy().into_owned(),
        )]
        .into();
        assert_eq!(
            advisor_label(Some(&env), &proj).as_deref(),
            Some("Opus 5.5")
        );
        std::fs::write(
            proj.join(".claude/settings.json"),
            r#"{"advisorModel":"claude-sonnet-5-5"}"#,
        )
        .unwrap();
        assert_eq!(
            advisor_label(Some(&env), &proj).as_deref(),
            Some("Sonnet 5.5")
        );
        std::fs::write(claude.join("settings.json"), "{}").unwrap();
        std::fs::remove_file(proj.join(".claude/settings.json")).unwrap();
        assert_eq!(advisor_label(Some(&env), &proj), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
