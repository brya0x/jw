//! Starting a program inside a pane, and bringing an agent back after a restart.
//!
//! A program is started by the shell, not typed at its prompt: the pane runs
//! `$SHELL -i -c '<program>; exec $SHELL -i'`. Nothing races the prompt (no keystrokes to
//! lose to a slow rc file, no echo to confuse the screen), the shell still reads its rc
//! files (`-i`), and when the program ends the same process becomes an ordinary shell, so
//! the pane never dies with it.

use crate::procs::title_for;

/// Flags that name a Claude conversation: the value after them is its id.
const SESSION_FLAGS: [&str; 3] = ["--session-id", "--resume", "-r"];
/// Flags that already choose which conversation to use.
const CHOOSING_FLAGS: [&str; 5] = ["--session-id", "--resume", "-r", "--continue", "-c"];

/// Quote one word for `shell`: fish escapes inside single quotes, the POSIX shells close and
/// reopen them.
pub fn quote(shell: &str, word: &str) -> String {
    if shell.rsplit('/').next() == Some("fish") {
        format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The `-c` script that runs `argv` and then turns the shell back into an ordinary one.
pub fn script(shell: &str, argv: &[String]) -> String {
    let words: Vec<String> = argv.iter().map(|w| quote(shell, w)).collect();
    format!("{}; exec {} -i", words.join(" "), quote(shell, shell))
}

pub fn is_agent(argv: &[String]) -> bool {
    title_for(argv).as_deref() == Some("claude")
}

/// An agent started by jwn gets its own conversation id up front, so two agents in one
/// folder never share a transcript and a restart can resume exactly this one.
pub fn with_session(argv: &[String], new_id: impl FnOnce() -> String) -> Vec<String> {
    let mut out = argv.to_vec();
    if is_agent(argv) && !argv.iter().any(|a| CHOOSING_FLAGS.contains(&a.as_str())) {
        out.extend(["--session-id".to_owned(), new_id()]);
    }
    out
}

/// What to run to bring a running agent back: `claude --resume <id>`, when its conversation
/// is known. A plain `claude` typed at a prompt has no known id and is not brought back.
pub fn agent_resume(argv: &[String]) -> Option<Vec<String>> {
    if !is_agent(argv) {
        return None;
    }
    let at = argv
        .iter()
        .position(|a| SESSION_FLAGS.contains(&a.as_str()))?;
    let id = argv.get(at + 1)?;
    Some(vec!["claude".into(), "--resume".into(), id.clone()])
}

/// Whether Claude has a saved conversation with this id: it writes `<id>.jsonl` under
/// `projects/` in its config directory once the first message is sent. Resuming an id that
/// has none only prints an error, so a pane whose agent never got a message restarts as a
/// plain shell.
pub fn transcript_exists(env: &[(String, String)], id: &str) -> bool {
    let from_env = |k: &str| {
        env.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(k).ok())
    };
    let dir = from_env("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| from_env("HOME").map(|h| std::path::Path::new(&h).join(".claude")));
    let Some(projects) = dir.map(|d| d.join("projects")) else {
        return false;
    };
    std::fs::read_dir(projects)
        .into_iter()
        .flatten()
        .flatten()
        .any(|project| project.path().join(format!("{id}.jsonl")).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn words_are_quoted_for_the_shell_that_reads_them() {
        assert_eq!(quote("/bin/zsh", "it's a test"), r"'it'\''s a test'");
        assert_eq!(
            quote("/opt/homebrew/bin/fish", "it's a \\ test"),
            r"'it\'s a \\ test'"
        );
    }

    #[test]
    fn the_script_runs_the_program_then_becomes_the_shell() {
        let s = script("/bin/zsh", &["nvim".into(), "a b.rs".into()]);
        assert_eq!(s, "'nvim' 'a b.rs'; exec '/bin/zsh' -i");
    }

    #[test]
    fn an_agent_gets_a_conversation_id_unless_it_already_chose_one() {
        let new = || "11111111-1111-4111-8111-111111111111".to_owned();
        assert_eq!(
            with_session(&argv("claude"), new),
            argv("claude --session-id 11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            with_session(&argv("claude --resume abc"), new),
            argv("claude --resume abc")
        );
        assert_eq!(with_session(&argv("claude -c"), new), argv("claude -c"));
        assert_eq!(
            with_session(&argv("nvim a.rs"), new),
            argv("nvim a.rs"),
            "only agents"
        );
    }

    #[test]
    fn a_conversation_exists_once_claude_wrote_its_transcript() {
        let dir = std::env::temp_dir().join(format!("jwn-tr-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("projects/p")).unwrap();
        let env = vec![(
            "CLAUDE_CONFIG_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        )];
        assert!(!transcript_exists(&env, "abc"));
        std::fs::write(dir.join("projects/p/abc.jsonl"), "{}").unwrap();
        assert!(transcript_exists(&env, "abc"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_running_agent_with_a_known_conversation_can_be_resumed() {
        assert_eq!(
            agent_resume(&argv("claude --session-id abc")),
            Some(argv("claude --resume abc"))
        );
        assert_eq!(
            agent_resume(&argv("claude -r abc --verbose")),
            Some(argv("claude --resume abc"))
        );
        assert_eq!(
            agent_resume(&argv("node /x/claude.js --resume abc")),
            Some(argv("claude --resume abc"))
        );
        assert_eq!(agent_resume(&argv("claude")), None, "no known conversation");
        assert_eq!(agent_resume(&argv("nvim --resume abc")), None);
    }
}
