//! What runs inside each pane. A pane is always the user's shell; a program (claude, nvim,
//! cargo) runs in it. The program is found from the terminal's foreground process group,
//! never from a role chosen when the pane was made.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use contract::{Block, BlocksMode, Id};

use crate::server::Shared;

/// How often the foreground programs are looked at.
const EVERY: Duration = Duration::from_millis(700);

/// Interpreters that stand in front of the program the person thinks they ran.
const RUNTIMES: [&str; 12] = [
    "node", "python", "python3", "ruby", "bun", "deno", "perl", "java", "sh", "bash", "zsh", "fish",
];

fn file_name(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

/// What to call a running program: `nvim` for `/usr/bin/nvim a.rs`, `claude` for
/// `node /opt/claude/cli.js`-style launches is the script's name without its extension.
pub fn title_for(argv: &[String]) -> Option<String> {
    let first = file_name(argv.first()?).trim_start_matches('-');
    if RUNTIMES.contains(&first)
        && let Some(script) = argv.iter().skip(1).find(|a| !a.starts_with('-'))
    {
        let name = file_name(script);
        return Some(
            name.rsplit_once('.')
                .map_or(name, |(stem, _)| stem)
                .to_owned(),
        );
    }
    (!first.is_empty()).then(|| first.to_owned())
}

/// Programs that draw their own screen, with or without the alternate one (claude does not
/// use it): while one is in the foreground the pane is drawn as its screen, not as blocks.
const FULL_SCREEN: [&str; 15] = [
    "claude", "nvim", "vim", "vi", "nano", "emacs", "htop", "btop", "top", "less", "man", "tmux",
    "ssh", "lazygit", "k9s",
];

pub fn takes_screen(argv: &[String]) -> bool {
    title_for(argv).is_some_and(|t| FULL_SCREEN.contains(&t.as_str()))
}

/// How a pane with a blocks node is drawn (spec REQ-250, REQ-251). While the shell reports a
/// command running, the pane is its screen when the command names a program that takes it
/// (known the moment the shell says so) or when the foreground is one (the look every 0.7 s
/// finds what an alias or a script started). With no command running it is `stored`: the
/// foreground is not asked, as it still names a program that has already ended for up to one look.
pub fn mode_of(stored: BlocksMode, blocks: &[Block], foreground: Option<&[String]>) -> BlocksMode {
    let Some(running) = blocks.last().filter(|b| b.running) else {
        return stored;
    };
    let named: Vec<String> = running.cmd.split_whitespace().map(str::to_owned).collect();
    if takes_screen(&named) || foreground.is_some_and(takes_screen) {
        BlocksMode::Full
    } else {
        stored
    }
}

/// The command lines of `pids`, from one `ps`.
fn args_of(pids: &HashSet<u32>) -> HashMap<u32, Vec<String>> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(out) = Command::new("ps")
        .args(["-o", "pid=,args=", "-p", &list])
        .output()
    else {
        return HashMap::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, args) = line.split_once(char::is_whitespace)?;
            Some((
                pid.parse().ok()?,
                args.split_whitespace().map(str::to_owned).collect(),
            ))
        })
        .collect()
}

/// One look at every pane: the program in the foreground, or `None` at the prompt.
fn look(shared: &Shared) {
    let terms = shared.lock().terms_snapshot();
    let foreground: Vec<(Id, Option<u32>)> = terms
        .iter()
        .map(|(id, term)| {
            (
                id.clone(),
                term.foreground().filter(|fg| Some(*fg) != term.shell_pid()),
            )
        })
        .collect();
    let pids: HashSet<u32> = foreground.iter().filter_map(|(_, p)| *p).collect();
    let args = args_of(&pids);
    let updates: HashMap<Id, Option<Vec<String>>> = foreground
        .into_iter()
        .map(|(id, pid)| (id, pid.and_then(|p| args.get(&p).cloned())))
        .collect();
    let mut core = shared.lock();
    if core.set_running(&updates) {
        core.refresh();
    }
}

pub fn run(shared: Shared, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        look(&shared);
        std::thread::sleep(EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn block(cmd: &str, running: bool) -> Block {
        Block {
            id: 1,
            cmd: cmd.into(),
            cwd: "/".into(),
            started: 0,
            running,
            code: None,
            ms: None,
            truncated: false,
            full: false,
            out: Vec::new(),
        }
    }

    #[test]
    fn the_mode_follows_the_command_the_shell_reports_and_not_only_the_look_at_the_foreground() {
        let claude = argv("claude");
        // The command names the program: full at once, with the foreground still unknown.
        let named = [block("ls", false), block("claude --resume 1", true)];
        assert_eq!(mode_of(BlocksMode::Blocks, &named, None), BlocksMode::Full);
        // An alias or a script: the look at the foreground finds it.
        let alias = [block("ai", true)];
        assert_eq!(
            mode_of(BlocksMode::Blocks, &alias, None),
            BlocksMode::Blocks
        );
        assert_eq!(
            mode_of(BlocksMode::Blocks, &alias, Some(&claude)),
            BlocksMode::Full
        );
        // The shell says it ended: blocks, though the foreground may still say claude.
        let ended = [block("claude", false)];
        assert_eq!(
            mode_of(BlocksMode::Blocks, &ended, Some(&claude)),
            BlocksMode::Blocks
        );
        assert_eq!(
            mode_of(BlocksMode::Blocks, &[], Some(&claude)),
            BlocksMode::Blocks,
            "no command has been reported at all"
        );
        // A command that draws no screen of its own, and what the store saw of an alternate screen.
        let sleeping = [block("sleep 5", true)];
        assert_eq!(
            mode_of(BlocksMode::Blocks, &sleeping, None),
            BlocksMode::Blocks
        );
        assert_eq!(mode_of(BlocksMode::Full, &sleeping, None), BlocksMode::Full);
    }

    #[test]
    fn a_program_that_draws_its_own_screen_is_found_by_its_name() {
        assert!(takes_screen(&argv("claude --resume 1234")));
        assert!(takes_screen(&argv("/usr/bin/nvim a.rs")));
        assert!(!takes_screen(&argv("sleep 60")));
        assert!(!takes_screen(&argv("cargo test")));
    }

    #[test]
    fn a_program_is_named_by_its_file_name() {
        assert_eq!(
            title_for(&argv("/usr/bin/nvim src/main.rs")).as_deref(),
            Some("nvim")
        );
        assert_eq!(
            title_for(&argv("claude --resume 1234")).as_deref(),
            Some("claude")
        );
        assert_eq!(title_for(&argv("-zsh")).as_deref(), Some("zsh"));
    }

    #[test]
    fn an_interpreter_gives_way_to_its_script() {
        assert_eq!(
            title_for(&argv("node /opt/claude-code/cli.js --flag")).as_deref(),
            Some("cli")
        );
        assert_eq!(
            title_for(&argv("python3 -u tools/serve.py")).as_deref(),
            Some("serve")
        );
        assert_eq!(title_for(&argv("node")).as_deref(), Some("node"));
    }

    #[test]
    fn nothing_running_has_no_name() {
        assert_eq!(title_for(&[]), None);
    }
}
