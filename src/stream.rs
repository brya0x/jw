//! What opening a stream means: which panes to start, with which command,
//! directory and environment (REQ-7, REQ-8). Port of the stream half of
//! internal/commands/open.go and project.go (jwEnv); the daemon then starts
//! what this returns.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::connectors::git::Repo;
use crate::core::config::{self, Config};
use crate::core::expand::{Vars, expand};
use crate::core::registry::{self, Entry, Registry};
use crate::layout::Node;

/// A stream resolved against its project's config, ready to open.
#[derive(Debug, Clone)]
pub struct Stream {
    pub entry: Entry,
    pub cfg: Config,
    /// The default branch of origin, for `{base}`.
    pub base: String,
    pub tree: Node,
}

/// One pane to start: `cmd` runs through `sh -c`, `None` is a plain shell.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneSpec {
    pub role: String,
    pub cmd: Option<String>,
    /// The agent's resume command, for when the daemon restarts it.
    pub resume: Option<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl PaneSpec {
    /// The daemon message that starts this pane at `(cols, rows)`.
    pub fn spawn(self, stream: &str, (cols, rows): (u16, u16)) -> crate::proto::ClientMsg {
        crate::proto::ClientMsg::Spawn {
            stream: stream.to_string(),
            role: self.role,
            cmd: self.cmd,
            cwd: self.cwd,
            env: self.env,
            cols,
            rows,
        }
    }
}

impl Stream {
    /// Finds the project of `entry` from its worktree and loads its config.
    /// An opened folder (REQ-55): a repository with a jw config starts with
    /// its layout; anything else with one shell.
    pub fn resolve(entry: &Entry) -> Result<Self> {
        if crate::folders::is_folder(entry) {
            let dir = Path::new(&entry.path);
            if !dir.is_dir() {
                bail!("{} is not a folder", entry.path);
            }
            let (cfg, base, tree) = match crate::folders::repo(dir) {
                Some(repo) => {
                    let cfg = config::load(&repo.root, &repo.remote, &entry.project)?;
                    let tree = if cfg.source.is_some() {
                        cfg.layout.tree()
                    } else {
                        Node::leaf("shell")
                    };
                    (cfg, repo.default_branch().unwrap_or_default(), tree)
                }
                None => (crate::folders::config(), String::new(), Node::leaf("shell")),
            };
            return Ok(Self {
                entry: entry.clone(),
                cfg,
                base,
                tree,
            });
        }
        let repo = Repo::open(Path::new(&entry.path))?;
        let cfg = config::load(&repo.root, &repo.remote, &entry.project)?;
        let base = repo.default_branch()?;
        let tree = cfg.layout.tree();
        Ok(Self {
            entry: entry.clone(),
            cfg,
            base,
            tree,
        })
    }

    pub fn vars(&self) -> Vars {
        self.cfg.vars(&self.entry.name, &self.base, self.entry.slot)
    }

    /// The JW_* variables every pane of the stream gets.
    pub fn env(&self) -> BTreeMap<String, String> {
        if crate::folders::is_folder(&self.entry) {
            // No slot and no ports: only who the pane belongs to.
            return BTreeMap::from([
                ("JW_ID".to_string(), self.entry.id.clone()),
                ("JW_NAME".to_string(), self.entry.name.clone()),
                ("JW_PROJECT".to_string(), self.entry.project.clone()),
            ]);
        }
        crate::actions::jw_env(&self.entry, &self.vars())
    }

    /// One spec per leaf of the layout, in tree order.
    pub fn panes(&self) -> Result<Vec<PaneSpec>> {
        let vars = self.vars();
        let env = self.env();
        self.tree
            .leaves()
            .into_iter()
            .map(|leaf| {
                Ok(PaneSpec {
                    cmd: self.command(&leaf.run, &vars)?,
                    resume: if leaf.run == "agent" {
                        self.agent_resume(&vars)
                    } else {
                        None
                    },
                    role: leaf.role,
                    cwd: PathBuf::from(&self.entry.path),
                    env: env.clone(),
                })
            })
            .collect()
    }

    /// The panes to start when the stream opens; with `setup`, the shell
    /// pane runs the config's setup first, showing it, then stays a shell.
    /// The note says when setup had nowhere to run.
    pub fn open_specs(&self, setup: bool) -> Result<(Vec<PaneSpec>, Option<String>)> {
        let mut specs = self.panes()?;
        let mut note = None;
        if setup && let Some(line) = crate::actions::setup_line(&self.cfg, &self.vars())? {
            match specs.iter_mut().find(|s| s.cmd.is_none()) {
                Some(shell) => {
                    shell.cmd = Some(format!(
                        "printf '%s\\n' {}; {line}; exec \"${{SHELL:-sh}}\"",
                        shell_quote(&format!("$ {line}"))
                    ))
                }
                None => note = Some("setup skipped: the layout has no shell pane".into()),
            }
        }
        Ok((specs, note))
    }

    /// Records that an agent ran here, so the next open resumes its
    /// conversation (as `jw open` did).
    pub fn mark_opened(&self) -> Result<()> {
        if self.entry.opened {
            return Ok(());
        }
        if crate::folders::is_folder(&self.entry) {
            let dir = self.entry.path.clone();
            return crate::folders::edit(|all| {
                if let Some(f) = all.folders.iter_mut().find(|f| f.dir == dir) {
                    f.opened = true;
                }
            });
        }
        let path = registry::default_path()?;
        let mut reg = Registry::load(&path)?;
        if let Some(e) = reg.entries.iter_mut().find(|e| e.id == self.entry.id) {
            e.opened = true;
            reg.save(&path)?;
        }
        Ok(())
    }

    /// What a leaf's `run` starts: `editor`, `agent`, `shell`, `dev:<svc>`,
    /// or any other command line, with placeholders expanded.
    fn command(&self, run: &str, vars: &Vars) -> Result<Option<String>> {
        let line = match run {
            "shell" => return Ok(None),
            "editor" => self.cfg.layout.editor.clone(),
            "agent" => self.agent_command()?,
            _ => match run.strip_prefix("dev:") {
                Some(svc) => {
                    let Some(cmds) = self.cfg.dev.get(svc) else {
                        bail!("layout runs dev:{svc}, but [dev] has no {svc}");
                    };
                    // Several commands for one service run side by side,
                    // like `jw dev` did with splits.
                    cmds.join(" & ") + if cmds.len() > 1 { " & wait" } else { "" }
                }
                None => run.to_string(),
            },
        };
        Ok(Some(expand(&line, vars)?))
    }

    /// The agent's resume command, expanded; none when the config has none.
    fn agent_resume(&self, vars: &Vars) -> Option<String> {
        let agents = &self.cfg.agent;
        let cmd = match agents.default.as_str() {
            "codex" => &agents.codex,
            _ => &agents.claude,
        };
        (!cmd.resume.trim().is_empty())
            .then(|| expand(&cmd.resume, vars).ok())
            .flatten()
    }

    /// The agent resumes its conversation in a worktree that had one before.
    fn agent_command(&self) -> Result<String> {
        let agents = &self.cfg.agent;
        let cmd = match agents.default.as_str() {
            "codex" => &agents.codex,
            _ => &agents.claude,
        };
        let line = if self.entry.opened {
            &cmd.resume
        } else {
            &cmd.start
        };
        if line.trim().is_empty() {
            bail!("empty agent command for {}", agents.default);
        }
        Ok(line.clone())
    }
}

/// Single-quotes a string for sh.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dir;

    fn stream(opened: bool) -> Stream {
        let mut cfg: Config = toml::from_str(
            "[ports]\nweb = 0\napi = 1\n[dev]\nweb = [\"vite --port {port.web}\"]\n",
        )
        .unwrap();
        cfg.layout.editor = "nvim -c 'DiffviewOpen origin/{base}...HEAD'".into();
        cfg.agent.default = "claude".into();
        cfg.agent.claude.start = "claude".into();
        cfg.agent.claude.resume = "claude --continue".into();
        Stream {
            entry: Entry {
                id: "id-1".into(),
                name: "web".into(),
                project: "myapp".into(),
                path: "/wt/web".into(),
                slot: 3,
                opened,
                ..Entry::default()
            },
            cfg,
            base: "trunk".into(),
            tree: Node::default_tree(),
        }
    }

    #[test]
    fn default_panes() {
        let panes = stream(false).panes().unwrap();
        let got: Vec<(&str, Option<&str>)> = panes
            .iter()
            .map(|p| (p.role.as_str(), p.cmd.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("editor", Some("nvim -c 'DiffviewOpen origin/trunk...HEAD'")),
                ("agent", Some("claude")),
                ("shell", None),
            ]
        );
        assert_eq!(panes[0].cwd, PathBuf::from("/wt/web"));
    }

    #[test]
    fn env_has_the_jw_variables() {
        let env = stream(false).env();
        assert_eq!(env["JW_NAME"], "web");
        assert_eq!(env["JW_PROJECT"], "myapp");
        assert_eq!(env["JW_SLOT"], "3");
        assert_eq!(env["JW_PORT_BASE"], "20300");
        assert_eq!(env["JW_PORT_API"], "20301");
    }

    #[test]
    fn opened_stream_resumes_the_agent() {
        let panes = stream(true).panes().unwrap();
        assert_eq!(panes[1].cmd.as_deref(), Some("claude --continue"));
    }

    #[test]
    fn dev_leaf_runs_the_service() {
        let mut s = stream(false);
        s.tree = Node::split(
            Dir::Right,
            0.5,
            Node::leaf("dev:web"),
            Node::leaf("dev:api"),
        );
        assert!(s.panes().is_err(), "api has no dev command");
        s.tree = Node::leaf("dev:web");
        assert_eq!(
            s.panes().unwrap()[0].cmd.as_deref(),
            Some("vite --port 20300")
        );
    }
}
