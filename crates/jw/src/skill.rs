//! `jw skill [install]`: the Claude Code skill that tells an agent in a jw
//! pane how to use the CLI (REQ-78). It is built into the binary, so the
//! installed copy always matches the jw that runs.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub const SKILL: &str = include_str!("../skill/SKILL.md");

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [] => {
            print!("{SKILL}");
            Ok(())
        }
        [a] if a == "install" => {
            let path = install()?;
            println!("installed {}", path.display());
            Ok(())
        }
        _ => bail!("usage: jw skill [install]"),
    }
}

/// Writes the skill to `skills/jw/SKILL.md` in Claude's config dir
/// (`$CLAUDE_CONFIG_DIR`, else `~/.claude`), replacing what was there.
fn install() -> Result<PathBuf> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".claude"),
    }
    .join("skills/jw");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    let tmp = dir.join(".SKILL.md.tmp");
    std::fs::write(&tmp, SKILL).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_names_every_agent_command() {
        assert!(SKILL.starts_with("---\nname: jw\n"));
        for cmd in [
            "jw ls",
            "jw read",
            "jw worktree",
            "jw prompt",
            "jw sessions",
            "jw help",
        ] {
            assert!(SKILL.contains(cmd), "the skill doesn't mention {cmd}");
        }
    }
}
