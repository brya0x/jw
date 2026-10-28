//! Starting a program inside a pane, and bringing an agent back after a restart.
//!
//! A program is started by the shell, not typed at its prompt: the pane runs
//! `$SHELL -i -c '<program>; exec $SHELL -i'`. Nothing races the prompt (no keystrokes to
//! lose to a slow rc file, no echo to confuse the screen), the shell still reads its rc
//! files (`-i`), and when the program ends the same process becomes an ordinary shell, so
//! the pane never dies with it. The integration's hooks belong to the prompt, so they never see the
//! program: the script prints the marks of a command around it itself (REQ-1056), and the pane
//! is blocks from the program's first byte. A program that takes the screen, an agent and a
//! shell with no integration get no marks: until the shell that takes over shows its first
//! prompt the pane is its screen and not a list of blocks (REQ-844).

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::procs::title_for;

/// Flags that name a Claude conversation: the value after them is its id.
const SESSION_FLAGS: [&str; 3] = ["--session-id", "--resume", "-r"];
/// Flags that already choose which conversation to use.
const CHOOSING_FLAGS: [&str; 5] = ["--session-id", "--resume", "-r", "--continue", "-c"];

fn is_fish(shell: &str) -> bool {
    shell.rsplit('/').next() == Some("fish")
}

/// Quote one word for `shell`: fish escapes inside single quotes, the POSIX shells close and
/// reopen them.
pub fn quote(shell: &str, word: &str) -> String {
    if is_fish(shell) {
        format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

fn takes_screen(argv: &[String]) -> bool {
    crate::procs::takes_screen(argv) || agent_name(argv).is_some()
}

/// The marks of a command the integration's own scripts print (`shell/*`): its text and folder
/// before it, its status after it. fish says `$status` and writes a substitution `(…)`.
fn marks(shell: &str, argv: &[String]) -> (String, &'static str) {
    let fish = is_fish(shell);
    let b64 = |word: String| {
        let encode = format!("printf %s {word} | base64 | tr -d '\\n'");
        if fish {
            format!("({encode})")
        } else {
            format!("\"$({encode})\"")
        }
    };
    let before = format!(
        "printf '\\e]633;E;%s\\a\\e]633;P;Cwd=%s\\a\\e]133;C\\a' {} {}; ",
        b64(quote(shell, &argv.join(" "))),
        b64("\"$PWD\"".to_owned())
    );
    (before, if fish { "$status" } else { "$?" })
}

/// The `-c` script that runs `argv` — and, when it ends well, `then` — and then turns the shell
/// back into an ordinary one. `then` is typed in the interactive shell, so `claude` is the
/// function that opens a conversation (REQ-371) and not the original. `marked` asks for the
/// program to be reported as a command (REQ-1056), which is done only where nothing else
/// draws it (REQ-1058).
pub fn script(
    shell: &str,
    argv: &[String],
    then: Option<&[String]>,
    setup: &crate::integration::Setup,
    marked: bool,
) -> String {
    let line = |a: &[String]| -> String {
        a.iter()
            .map(|w| quote(shell, w))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let words = match then {
        Some(then) => format!("{} && {}", line(argv), line(then)),
        None => line(argv),
    };
    // The shell that takes over keeps the integration: its environment and its arguments.
    let env: Vec<String> = setup
        .env
        .iter()
        .map(|(k, v)| quote(shell, &format!("{k}={v}")))
        .collect();
    let args: Vec<String> = setup.args.iter().map(|a| quote(shell, a)).collect();
    let env_prefix = if env.is_empty() {
        String::new()
    } else {
        format!("env {} ", env.join(" "))
    };
    let arg_part = if args.is_empty() {
        String::new()
    } else {
        format!("{} ", args.join(" "))
    };
    let reported = marked
        && setup.integrated()
        && !takes_screen(argv)
        && then.is_none_or(|t| !takes_screen(t));
    let (before, after) = match reported.then(|| marks(shell, argv)) {
        Some((before, status)) => (before, format!("printf '\\e]133;D;%d\\a' {status}; ")),
        None => Default::default(),
    };
    // `^C` reaches the shell as well as the program (an interactive `-c` has no job control in zsh) and
    // would end the script and the pane: a trap, unlike an ignore, is not inherited by the program. fish's
    // own trap is a handler of the signal, which keeps it from cancelling the script.
    let trap = if is_fish(shell) {
        "function __jw_int --on-signal INT; end; "
    } else {
        "trap : INT; "
    };
    format!(
        "{trap}{before}{words}; {after}exec {env_prefix}{} {arg_part}-i",
        quote(shell, shell)
    )
}

/// `claude` or `codex` when `argv` runs that agent, whoever started it: an interpreter's script
/// counts (`node /opt/claude/cli.js`).
pub fn agent_name(argv: &[String]) -> Option<&'static str> {
    let title = title_for(argv)?;
    ["claude", "codex"].into_iter().find(|a| *a == title)
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

/// The hooks an agent is started with: each runs `<exe> hook <event>`, which tells the daemon
/// (`agents.rs`). `Notification` is only asked for the kinds that need a person, and
/// `PreToolUse` only for the tools that ask a question or present a plan.
pub fn hook_settings(exe: &Path) -> String {
    let exe = quote("/bin/sh", &exe.to_string_lossy());
    let hook = |event: &str, matcher: Option<&str>| hook_group(&exe, event, matcher);
    json!({"hooks": {
        "Notification": hook("notification", Some("permission_prompt|elicitation_dialog|agent_needs_input")),
        "PreToolUse": hook("pre_tool_use", Some("AskUserQuestion|ExitPlanMode")),
        "PostToolUse": hook("post_tool_use", None),
        "UserPromptSubmit": hook("user_prompt_submit", None),
        "Stop": hook("stop", None),
        "SessionEnd": hook("session_end", None),
    }})
    .to_string()
}

fn hook_group(exe: &str, event: &str, matcher: Option<&str>) -> serde_json::Value {
    let mut group = json!({"hooks": [{
        "type": "command",
        "command": format!("{exe} hook {event}"),
        "timeout": 5,
    }]});
    if let Some(m) = matcher {
        group["matcher"] = m.into();
    }
    json!([group])
}

/// The hooks an engine is started with (REQ-930): `Stop`, which a task loop answers, and
/// `UserPromptSubmit`, whose answer names the skill of the task lists (REQ-934). A conversation takes its
/// attention from the engine's own events, so the rest of `hook_settings` would raise each one twice.
pub fn loop_hook_settings(exe: &Path) -> String {
    let exe = quote("/bin/sh", &exe.to_string_lossy());
    json!({"hooks": {
        "Stop": hook_group(&exe, "stop", None),
        "UserPromptSubmit": hook_group(&exe, "user_prompt_submit", None),
    }})
    .to_string()
}

/// Write the engine's hooks to `<state>/claude-loop-hooks.json`, beside the agent's.
pub fn write_loop_hooks(state_dir: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    let file = state_dir.join("claude-loop-hooks.json");
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, loop_hook_settings(exe))?;
    std::fs::rename(&tmp, &file)?;
    Ok(file)
}

/// Write the agent's hooks to `<state>/claude-hooks.json`, which `--settings` takes.
pub fn write_hooks(state_dir: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    let file = state_dir.join("claude-hooks.json");
    // Through a rename, so an agent starting meanwhile never reads half of it.
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, hook_settings(exe))?;
    std::fs::rename(&tmp, &file)?;
    Ok(file)
}

/// An agent gets the hooks for this run only (`--settings` writes nothing and merges with the
/// person's own hooks), unless it was started with settings of its own.
pub fn with_hooks(argv: &[String], file: &Path) -> Vec<String> {
    let mut out = argv.to_vec();
    if is_agent(argv)
        && !argv
            .iter()
            .any(|a| a == "--settings" || a.starts_with("--settings="))
    {
        out.extend(["--settings".to_owned(), file.to_string_lossy().into_owned()]);
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

    /// REQ-930: the engine is given the hook of a task loop and no other.
    #[test]
    fn the_engines_hooks_are_the_stop_and_the_prompt_hooks_alone() {
        let v: serde_json::Value =
            serde_json::from_str(&loop_hook_settings(Path::new("/bin/jw"))).unwrap();
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(
            hooks.keys().collect::<Vec<_>>(),
            ["Stop", "UserPromptSubmit"]
        );
        let command = hooks["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.ends_with(" hook stop"), "{command}");
        let nudge = hooks["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(nudge.ends_with(" hook user_prompt_submit"), "{nudge}");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["timeout"], 5);
        // the panes' own file is a different one, with the rest
        let all: serde_json::Value =
            serde_json::from_str(&hook_settings(Path::new("/bin/jw"))).unwrap();
        assert!(all["hooks"].as_object().unwrap().len() > 1);
        let dir = std::env::temp_dir().join(format!("jw-loop-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = write_loop_hooks(&dir, Path::new("/bin/jw")).unwrap();
        assert_ne!(file, dir.join("claude-hooks.json"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            loop_hook_settings(Path::new("/bin/jw"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let s = script(
            "/bin/zsh",
            &["nvim".into(), "a b.rs".into()],
            None,
            &Default::default(),
            true,
        );
        assert_eq!(s, "trap : INT; 'nvim' 'a b.rs'; exec '/bin/zsh' -i");
        // what follows runs only when the first ended well (REQ-507); the shell stays either way
        let chained = script(
            "/bin/zsh",
            &["sh".into(), "-c".into(), "make".into()],
            Some(&["claude".into(), "--session-id".into(), "x".into()]),
            &Default::default(),
            true,
        );
        assert_eq!(
            chained,
            "trap : INT; 'sh' '-c' 'make' && 'claude' '--session-id' 'x'; exec '/bin/zsh' -i"
        );
        let with = crate::integration::Setup {
            args: vec!["--rcfile".into(), "/s/rc.sh".into()],
            env: vec![("K".into(), "v w".into())],
        };
        assert_eq!(
            script("/bin/bash", &["ls".into()], None, &with, false),
            "trap : INT; 'ls'; exec env 'K=v w' '/bin/bash' '--rcfile' '/s/rc.sh' -i"
        );
    }

    fn integrated(shell: &str) -> crate::integration::Setup {
        crate::integration::setup(shell, Path::new("/s"), &|_| Some("/h".into()))
    }

    /// REQ-1056 and REQ-1060: the marks of a command around the program, with its words quoted for the shell.
    #[test]
    fn the_script_reports_the_program_as_a_command_for_zsh_bash_and_fish() {
        let argv = ["pnpm".to_owned(), "it's a b".to_owned()];
        let tail = |shell: &str| match shell {
            "/bin/zsh" => "exec env 'ZDOTDIR=/s/zsh' 'JW_USER_ZDOTDIR=/h' '/bin/zsh' -i",
            "/bin/bash" => "exec '/bin/bash' '--rcfile' '/s/bash/rc.sh' -i",
            _ => "exec '/bin/fish' '--init-command' 'source /s/fish/jw.fish' -i",
        };
        let b = "printf '\\e]633;E;%s\\a\\e]633;P;Cwd=%s\\a\\e]133;C\\a'";
        let words = "'pnpm' 'it'\\''s a b'";
        let zsh = script("/bin/zsh", &argv, None, &integrated("/bin/zsh"), true);
        assert_eq!(
            zsh,
            format!(
                "trap : INT; {b} \"$(printf %s 'pnpm it'\\''s a b' | base64 | tr -d '\\n')\" \"$(printf %s \"$PWD\" | base64 | tr -d '\\n')\"; {words}; printf '\\e]133;D;%d\\a' $?; {}",
                tail("/bin/zsh")
            )
        );
        let bash = script("/bin/bash", &argv, None, &integrated("/bin/bash"), true);
        assert!(
            bash.starts_with(&format!("trap : INT; {b} \"$(printf %s 'pnpm it'")),
            "{bash}"
        );
        assert!(
            bash.ends_with(&format!("$?; {}", tail("/bin/bash"))),
            "{bash}"
        );
        let fish = script("/bin/fish", &argv, None, &integrated("/bin/fish"), true);
        assert_eq!(
            fish,
            format!(
                "function __jw_int --on-signal INT; end; {b} (printf %s 'pnpm it\\'s a b' | base64 | tr -d '\\n') (printf %s \"$PWD\" | base64 | tr -d '\\n'); 'pnpm' 'it\\'s a b'; printf '\\e]133;D;%d\\a' $status; {}",
                tail("/bin/fish")
            )
        );
    }

    /// REQ-1058: no mark where the pane is drawn by something else, or asked not to be.
    #[test]
    fn nothing_is_reported_of_a_screen_an_agent_a_setup_or_a_shell_with_no_integration() {
        let zsh = integrated("/bin/zsh");
        let marked =
            |argv: &[&str], then: Option<&[&str]>, setup: &crate::integration::Setup, ask| {
                let argv: Vec<String> = argv.iter().map(|w| (*w).to_owned()).collect();
                let then: Option<Vec<String>> =
                    then.map(|t| t.iter().map(|w| (*w).to_owned()).collect());
                script("/bin/zsh", &argv, then.as_deref(), setup, ask).contains("133;C")
            };
        assert!(marked(&["pnpm", "dev"], None, &zsh, true));
        assert!(
            !marked(&["pnpm", "dev"], None, &zsh, false),
            "not asked for"
        );
        assert!(
            !marked(&["nvim", "a.rs"], None, &zsh, true),
            "takes the screen"
        );
        assert!(!marked(&["claude"], None, &zsh, true), "an agent");
        assert!(!marked(&["codex"], None, &zsh, true), "an agent");
        assert!(
            !marked(&["sh", "-c", "make"], Some(&["claude"]), &zsh, true),
            "a setup before an agent"
        );
        assert!(
            !marked(&["pnpm", "dev"], None, &Default::default(), true),
            "a shell with no integration"
        );
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
    fn an_agent_gets_the_hooks_once_and_others_do_not() {
        let file = Path::new("/s/claude-hooks.json");
        assert_eq!(
            with_hooks(&argv("claude --session-id abc"), file),
            argv("claude --session-id abc --settings /s/claude-hooks.json")
        );
        let own = argv("claude --settings /mine.json");
        assert_eq!(with_hooks(&own, file), own, "its own settings stay");
        let joined = argv("claude --settings=/mine.json");
        assert_eq!(with_hooks(&joined, file), joined, "also as --settings=…");
        assert_eq!(with_hooks(&argv("nvim a.rs"), file), argv("nvim a.rs"));
    }

    #[test]
    fn the_hooks_run_the_binary_with_the_event_and_ask_only_for_what_needs_a_person() {
        let v: serde_json::Value =
            serde_json::from_str(&hook_settings(Path::new("/opt/my jwn/jwn"))).unwrap();
        let h = &v["hooks"];
        assert_eq!(
            h["Stop"][0]["hooks"][0]["command"], "'/opt/my jwn/jwn' hook stop",
            "the path is quoted for the shell that runs it"
        );
        assert_eq!(h["Stop"][0]["hooks"][0]["timeout"], 5);
        assert!(h["Stop"][0].get("matcher").is_none());
        assert_eq!(
            h["Notification"][0]["matcher"],
            "permission_prompt|elicitation_dialog|agent_needs_input"
        );
        assert_eq!(
            h["PreToolUse"][0]["matcher"],
            "AskUserQuestion|ExitPlanMode"
        );
        for e in ["PostToolUse", "UserPromptSubmit", "SessionEnd"] {
            assert!(h[e].is_array(), "{e}");
        }
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

    #[test]
    fn an_agent_is_the_program_claude_or_codex_whoever_started_it() {
        assert_eq!(agent_name(&argv("claude --session-id x")), Some("claude"));
        assert_eq!(
            agent_name(&argv("/usr/local/bin/codex resume")),
            Some("codex")
        );
        assert_eq!(
            agent_name(&argv("node /opt/claude/cli.js")),
            None,
            "the script is cli"
        );
        assert_eq!(agent_name(&argv("node /opt/bin/claude")), Some("claude"));
        assert_eq!(
            agent_name(&argv("python3 -u codex.py")),
            Some("codex"),
            "a script's name without its extension"
        );
        assert_eq!(agent_name(&argv("-claude")), Some("claude"));
        assert_eq!(agent_name(&argv("nvim claude.md")), None);
        assert_eq!(agent_name(&argv("claude-notes")), None);
        assert_eq!(agent_name(&[]), None);
    }
}
