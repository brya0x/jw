//! What runs inside each pane. A pane is always the user's shell; a program (claude, nvim,
//! cargo) runs in it. The program is found from the terminal's foreground process group,
//! never from a role chosen when the pane was made.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use contract::Id;

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
