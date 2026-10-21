//! The shell integration: scripts that make zsh, bash and fish report each prompt and each
//! command (OSC 133 and 633), and how a pane's shell is started with them. Any other shell
//! runs unchanged and its pane has no blocks.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const ZSHENV: &str = include_str!("../shell/zsh/.zshenv");
const ZPROFILE: &str = include_str!("../shell/zsh/.zprofile");
const ZSHRC: &str = include_str!("../shell/zsh/.zshrc");
const BASH_RC: &str = include_str!("../shell/bash/rc.sh");
const FISH: &str = include_str!("../shell/fish/jw.fish");

/// What a shell needs to start with the integration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Setup {
    /// Arguments before `-i` (bash `--rcfile`, fish `--init-command`).
    pub args: Vec<String>,
    /// Environment variables (zsh `ZDOTDIR`).
    pub env: Vec<(String, String)>,
}

/// Write the scripts under `dir` (the daemon's state directory).
pub fn install(dir: &Path) -> io::Result<()> {
    for (rel, text) in [
        ("zsh/.zshenv", ZSHENV),
        ("zsh/.zprofile", ZPROFILE),
        ("zsh/.zshrc", ZSHRC),
        ("bash/rc.sh", BASH_RC),
        ("fish/jw.fish", FISH),
    ] {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent"))?;
        if fs::read_to_string(&path).ok().as_deref() != Some(text) {
            fs::write(&path, text)?;
        }
    }
    Ok(())
}

fn shell_name(shell: &str) -> &str {
    shell.rsplit('/').next().unwrap_or(shell)
}

/// How to start `shell` with the integration installed in `dir`. `lookup` reads an
/// environment variable as the pane's shell will see it.
pub fn setup(shell: &str, dir: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> Setup {
    let path = |rel: &str| -> PathBuf { dir.join(rel) };
    match shell_name(shell) {
        "zsh" => {
            let user = lookup("ZDOTDIR")
                .or_else(|| lookup("HOME"))
                .unwrap_or_default();
            Setup {
                args: vec![],
                env: vec![
                    ("ZDOTDIR".into(), path("zsh").to_string_lossy().into_owned()),
                    (identity::env_var("USER_ZDOTDIR"), user),
                ],
            }
        }
        "bash" => Setup {
            args: vec![
                "--rcfile".into(),
                path("bash/rc.sh").to_string_lossy().into_owned(),
            ],
            env: vec![],
        },
        "fish" => Setup {
            args: vec![
                "--init-command".into(),
                format!("source {}", path("fish/jw.fish").display()),
            ],
            env: vec![],
        },
        _ => Setup::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scripts_read_the_env_names_of_the_identity_and_no_others() {
        for (name, text) in [
            ("zsh/.zshenv", ZSHENV),
            ("zsh/.zprofile", ZPROFILE),
            ("zsh/.zshrc", ZSHRC),
            ("bash/rc.sh", BASH_RC),
            ("fish/jw.fish", FISH),
        ] {
            let prefix = format!("{}_", identity::get().env_prefix);
            let own: Vec<&str> = text
                .split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
                .filter(|w| w.starts_with("JW") || w.starts_with("JWN"))
                .collect();
            assert!(
                own.iter().all(|w| w.starts_with(&prefix)),
                "{name} reads {own:?}, not {prefix}*"
            );
        }
        for suffix in ["BIN", "PANE_ID", "CONVERSATION", "WORKSPACE_DIR"] {
            for text in [BASH_RC, ZSHRC, FISH] {
                assert!(text.contains(&identity::env_var(suffix)), "{suffix}");
            }
        }
        assert!(ZSHRC.contains(&identity::env_var("USER_ZDOTDIR")));
    }

    #[test]
    fn each_shell_is_started_its_own_way_and_others_not_at_all() {
        let dir = Path::new("/state/shell");
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_owned());
        let zsh = setup("/bin/zsh", dir, &env);
        assert_eq!(zsh.env[0], ("ZDOTDIR".into(), "/state/shell/zsh".into()));
        assert_eq!(
            zsh.env[1],
            (identity::env_var("USER_ZDOTDIR"), "/home/u".into())
        );
        assert_eq!(
            setup("/bin/bash", dir, &env).args,
            ["--rcfile", "/state/shell/bash/rc.sh"]
        );
        assert_eq!(setup("/usr/bin/fish", dir, &env).args[0], "--init-command");
        assert_eq!(setup("/bin/sh", dir, &env), Setup::default());
    }

    #[test]
    fn a_user_zdotdir_is_remembered() {
        let env = |k: &str| (k == "ZDOTDIR").then(|| "/cfg/zsh".to_owned());
        assert_eq!(setup("zsh", Path::new("/s"), &env).env[1].1, "/cfg/zsh");
    }

    #[test]
    fn the_scripts_are_written_once_and_kept_current() {
        let dir = std::env::temp_dir().join(format!("jwn-int-{}", std::process::id()));
        install(&dir).unwrap();
        assert!(
            dir.join("zsh/.zshrc").exists()
                && dir.join("bash/rc.sh").exists()
                && dir.join("fish/jw.fish").exists()
        );
        fs::write(dir.join("bash/rc.sh"), "old").unwrap();
        install(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join("bash/rc.sh")).unwrap(), BASH_RC);
        let _ = fs::remove_dir_all(dir);
    }
}
