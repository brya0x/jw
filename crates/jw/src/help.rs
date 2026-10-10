//! `jw help`, `jw -h`, `jw <command> --help`: what each command does and
//! how to call it (REQ-77). The usage lines here are the ones the commands
//! print when they are called wrong.

/// A command as `jw help` shows it.
pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub what: &'static str,
}

/// Every command a person or an agent types, in the order `jw help` lists
/// them. `jw daemon` is left out: the client starts it.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "",
        usage: "jw [session]",
        what: "opens the TUI on the session, or on the last one used",
    },
    Command {
        name: "new",
        usage: "jw new <session> [--dir <folder>]",
        what: "starts a session in the folder (the current one by default) and opens it",
    },
    Command {
        name: "sessions",
        usage: "jw sessions",
        what: "lists the sessions, how many workspaces each has open, and what their agents do",
    },
    Command {
        name: "ls",
        usage: "jw ls [--json]",
        what: "lists $JW_SESSION's workspaces: state, marks, branch and PR",
    },
    Command {
        name: "read",
        usage: "jw read <workspace> [--pane <role>] [--lines N]",
        what: "prints a pane's last lines, scrollback included (the agent's pane by default)",
    },
    Command {
        name: "worktree",
        usage: "jw worktree <name> [--in <project>] [--branch <branch>] [--from <ref>] [--task <text>]",
        what: "creates a worktree of the project, opens it, and hands the task to its agent",
    },
    Command {
        name: "prompt",
        usage: "jw prompt <workspace> <text>",
        what: "types a task into an open workspace's agent once it is quiet",
    },
    Command {
        name: "skill",
        usage: "jw skill [install]",
        what: "prints the Claude Code skill for jw, or installs it in ~/.claude/skills/jw",
    },
    Command {
        name: "server",
        usage: "jw server status|stop",
        what: "says whether the daemon runs, or stops it: its panes close, and jw brings the workspaces back when it opens",
    },
    Command {
        name: "theme",
        usage: "jw theme [ls | install <file|url|-> [--name <name>] [--force] | export <name> | use <name>]",
        what: "lists the themes, installs one (JSON, see themes/schema.json), prints one as a template, or picks one for dark or light mode",
    },
    Command {
        name: "hook",
        usage: "jw hook working|waiting|idle",
        what: "what claude's hooks run in a jw pane to report the agent's state",
    },
    Command {
        name: "help",
        usage: "jw help [command]",
        what: "this list, or one command's usage",
    },
];

/// Whether an argument asks for help.
pub fn asks(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .find(|c| !c.name.is_empty() && c.name == name)
}

/// Every command, one line of usage and one of what it does.
pub fn all() -> String {
    let mut out = format!(
        "jw {}: a terminal for running coding agents in parallel\n\n",
        env!("CARGO_PKG_VERSION")
    );
    for c in COMMANDS {
        out.push_str(&format!("  {}\n      {}\n", c.usage, c.what));
    }
    out.push_str(
        "\nInside the TUI, press Ctrl-Space and wait: every key shows.\n\
         jw --version prints the version.\n",
    );
    out
}

/// One command's usage and what it does.
pub fn one(c: &Command) -> String {
    format!("usage: {}\n\n{}\n", c.usage, c.what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_listed_once() {
        for c in COMMANDS.iter().filter(|c| !c.name.is_empty()) {
            assert_eq!(
                find(c.name).map(|f| f.usage),
                Some(c.usage),
                "{} is listed twice",
                c.name
            );
            assert!(
                c.usage.starts_with(&format!("jw {}", c.name)),
                "{}",
                c.usage
            );
            assert!(all().contains(c.usage));
        }
        assert!(find("daemon").is_none());
    }

    #[test]
    fn a_session_name_is_not_a_command() {
        for c in COMMANDS.iter().filter(|c| !c.name.is_empty()) {
            assert!(
                !jw_core::session::valid(c.name),
                "{} can be taken as a session name",
                c.name
            );
        }
    }
}
