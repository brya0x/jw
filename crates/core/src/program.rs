//! What runs in a pane: which foreground programs are agents (addendum 15).

/// The programs jw treats as agents. A pane running one takes `jw prompt`
/// and the agent marks, whatever its role.
const AGENTS: [&str; 5] = ["claude", "codex", "gemini", "aider", "opencode"];

/// Whether a command name is a known agent. The directory and a login
/// shell's leading `-` are not part of the name.
pub fn is_agent_program(name: &str) -> bool {
    let name = name.rsplit('/').next().unwrap_or(name);
    AGENTS.contains(&name.trim_start_matches('-'))
}

/// Whether a pane is an agent's: role `agent`, or an agent in the foreground.
pub fn is_agent_pane(role: &str, fg: Option<&str>) -> bool {
    role == "agent" || fg.is_some_and(is_agent_program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_and_out() {
        for name in [
            "claude",
            "codex",
            "/usr/local/bin/claude",
            "-claude",
            "aider",
        ] {
            assert!(is_agent_program(name), "{name}");
        }
        for name in ["zsh", "nvim", "node", "claude-code", "", "claudex"] {
            assert!(!is_agent_program(name), "{name}");
        }
    }

    #[test]
    fn role_or_program_makes_an_agent_pane() {
        assert!(is_agent_pane("agent", None));
        assert!(is_agent_pane("agent", Some("zsh")));
        assert!(is_agent_pane("shell", Some("claude")));
        assert!(!is_agent_pane("shell", Some("nvim")));
        assert!(!is_agent_pane("shell", None));
    }
}
