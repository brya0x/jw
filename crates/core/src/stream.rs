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
    /// The id of the claude conversation its agent pane runs: the one it
    /// had, or a new one (REQ-74).
    pub session: String,
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
            // The folder a session started in opens with one shell (REQ-61).
            let plain = crate::folders::load(&registry::state_dir()?)?
                .folders
                .iter()
                .any(|f| f.dir == entry.path && f.plain);
            let (cfg, base, tree) = match crate::folders::repo(dir) {
                Some(repo) => {
                    let cfg = config::load(&repo.root, &repo.remote, &entry.project)?;
                    let tree = if cfg.source.is_some() && !plain {
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
                session: session_of(entry),
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
            session: session_of(entry),
        })
    }

    pub fn vars(&self) -> Vars {
        self.cfg.vars(&self.entry.name, &self.base, self.entry.slot)
    }

    /// The JW_* variables every pane of the stream gets.
    pub fn env(&self) -> BTreeMap<String, String> {
        let mut env = if crate::folders::is_folder(&self.entry) {
            // No slot and no ports: only who the pane belongs to.
            BTreeMap::from([
                ("JW_ID".to_string(), self.entry.id.clone()),
                ("JW_NAME".to_string(), self.entry.name.clone()),
                ("JW_PROJECT".to_string(), self.entry.project.clone()),
            ])
        } else {
            crate::actions::jw_env(&self.entry, &self.vars())
        };
        // `jw ls` and friends in a pane act on its session.
        env.insert("JW_SESSION".into(), crate::session::current());
        env
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

    /// The panes to start when the stream opens. With `setup`, a shell pane
    /// runs the config's setup first, showing it, then stays a shell; with no
    /// shell pane, the editor, else the agent, else the first pane runs it
    /// before its program (REQ-119).
    pub fn open_specs(&self, setup: bool) -> Result<Vec<PaneSpec>> {
        let mut specs = self.panes()?;
        if setup && let Some(line) = crate::actions::setup_line(&self.cfg, &self.vars())? {
            let shown = format!(
                "printf '%s\\n' {}; {line}",
                shell_quote(&format!("$ {line}"))
            );
            let at = specs
                .iter()
                .position(|s| s.cmd.is_none())
                .or_else(|| specs.iter().position(|s| s.role == "editor"))
                .or_else(|| specs.iter().position(|s| s.role == "agent"))
                .unwrap_or(0);
            if let Some(pane) = specs.get_mut(at) {
                let then = pane
                    .cmd
                    .take()
                    .unwrap_or_else(|| "exec \"${SHELL:-sh}\"".into());
                pane.cmd = Some(format!("{shown}; {then}"));
            }
        }
        Ok(specs)
    }

    /// Records that an agent ran here, and which conversation, so the next
    /// open resumes it (as `jw open` did).
    pub fn mark_opened(&self) -> Result<()> {
        let has_agent = self.tree.leaves().iter().any(|l| l.run == "agent");
        let agent = if has_agent && self.is_claude() && !self.legacy() {
            self.session.clone()
        } else {
            self.entry.agent.clone()
        };
        if self.entry.opened && self.entry.agent == agent {
            return Ok(());
        }
        if crate::folders::is_folder(&self.entry) {
            let dir = self.entry.path.clone();
            return crate::folders::edit(|all| {
                if let Some(f) = all.folders.iter_mut().find(|f| f.dir == dir) {
                    f.opened = true;
                    f.agent = (!agent.is_empty()).then(|| agent.clone());
                }
            });
        }
        let path = registry::default_path()?;
        let mut reg = Registry::load(&path)?;
        if let Some(e) = reg.entries.iter_mut().find(|e| e.id == self.entry.id) {
            e.opened = true;
            e.agent = agent;
            reg.save(&path)?;
        }
        Ok(())
    }

    fn is_claude(&self) -> bool {
        self.cfg.agent.default != "codex"
    }

    /// An agent ran here before jw kept its conversation's id: only the
    /// config's resume command (`--continue`) can find it.
    fn legacy(&self) -> bool {
        self.entry.opened && self.entry.agent.is_empty()
    }

    /// What a leaf's `run` starts: `editor`, `agent`, `shell`, `dev:<svc>`,
    /// or any other command line, with placeholders expanded.
    fn command(&self, run: &str, vars: &Vars) -> Result<Option<String>> {
        let line = match run {
            "shell" => return Ok(None),
            "editor" => return Ok(Some(listen(&expand(&self.cfg.layout.editor, vars)?))),
            "agent" => return self.agent_command(vars).map(Some),
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

    /// What restarts the agent after the daemon restarts: claude by its
    /// conversation's id, else the config's resume command.
    fn agent_resume(&self, vars: &Vars) -> Option<String> {
        let agents = &self.cfg.agent;
        if self.is_claude() && !self.legacy() {
            let start = expand(&agents.claude.start, vars).ok()?;
            return Some(self.claude(&start, &format!("--resume {}", self.session)));
        }
        let cmd = match agents.default.as_str() {
            "codex" => &agents.codex,
            _ => &agents.claude,
        };
        let line = expand(&cmd.resume, vars).ok()?;
        if line.trim().is_empty() {
            return None;
        }
        Some(if self.is_claude() {
            self.claude(&line, "")
        } else {
            line
        })
    }

    /// The agent resumes its conversation in a worktree that had one before.
    /// claude gets its conversation's id and the hooks that report its state.
    fn agent_command(&self, vars: &Vars) -> Result<String> {
        let agents = &self.cfg.agent;
        let cmd = match agents.default.as_str() {
            "codex" => &agents.codex,
            _ => &agents.claude,
        };
        let line = if self.entry.opened && (!self.is_claude() || self.legacy()) {
            &cmd.resume
        } else {
            &cmd.start
        };
        if line.trim().is_empty() {
            bail!("empty agent command for {}", agents.default);
        }
        let line = expand(line, vars)?;
        if !self.is_claude() {
            return Ok(line);
        }
        Ok(if self.legacy() {
            self.claude(&line, "")
        } else if self.entry.opened {
            self.claude(&line, &format!("--resume {}", self.session))
        } else {
            self.claude(&line, &format!("--session-id {}", self.session))
        })
    }

    /// A claude command line with `extra` flags and jw's hooks (REQ-73).
    fn claude(&self, line: &str, extra: &str) -> String {
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!(" {extra}")
        };
        format!("{line}{extra} --settings {}", shell_quote(&hooks()))
    }
}

/// Where the nvim of pane `id` listens, so jw can open files in it with
/// `nvim --server` (REQ-75).
pub fn nvim_socket(id: u64) -> Option<PathBuf> {
    Some(nvim_dir()?.join(format!("{id}.sock")))
}

fn nvim_dir() -> Option<PathBuf> {
    Some(registry::state_dir().ok()?.join("nvim"))
}

/// An editor line that starts nvim gets `--listen` on its pane's socket;
/// any other editor runs as it is. The pane's id is only known when it
/// starts, so the shell expands `$JW_PANE_ID`.
pub fn listen(line: &str) -> String {
    let (prog, rest) = line.split_once(' ').unwrap_or((line, ""));
    let is_nvim = Path::new(prog).file_name().is_some_and(|n| n == "nvim");
    let Some(dir) = nvim_dir().filter(|_| is_nvim) else {
        return line.to_string();
    };
    let _ = std::fs::create_dir_all(&dir);
    let sock = format!(
        "{}/\"$JW_PANE_ID\".sock",
        shell_quote(&dir.display().to_string())
    );
    let rest = if rest.is_empty() {
        String::new()
    } else {
        format!(" {rest}")
    };
    format!("rm -f {sock}; exec {prog} --listen {sock}{rest}")
}

/// The conversation an entry's agent had, or a fresh id for a new one.
fn session_of(entry: &Entry) -> String {
    if entry.agent.is_empty() {
        registry::new_id().unwrap_or_default()
    } else {
        entry.agent.clone()
    }
}

/// Claude settings that report the agent's state to jw: working when it
/// gets a prompt or runs a tool, waiting when it asks for something, idle
/// when its turn ends. `jw hook` finds the pane by `JW_PANE_ID`.
pub fn hooks() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "jw".into());
    let run = |state: &str| {
        serde_json::json!([{ "hooks": [{
            "type": "command",
            "command": format!("{} hook {state}", shell_quote(&exe)),
            "timeout": 5,
        }] }])
    };
    serde_json::json!({ "hooks": {
        "UserPromptSubmit": run("working"),
        "PreToolUse": run("working"),
        "Notification": run("waiting"),
        "Stop": run("idle"),
    } })
    .to_string()
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
            tree: Node::three_panes(),
            session: "0000-s".into(),
        }
    }

    /// REQ-119: setup goes to the shell pane, else before the editor, else
    /// before the agent.
    #[test]
    fn setup_runs_in_the_shell_or_before_the_program() {
        let mut s = stream(false);
        s.cfg.setup = vec!["pnpm install".into()];
        let line = |s: &Stream| {
            s.open_specs(true)
                .unwrap()
                .into_iter()
                .filter_map(|p| {
                    p.cmd
                        .filter(|c| c.contains("pnpm install"))
                        .map(|c| (p.role, c))
                })
                .collect::<Vec<_>>()
        };
        let got = line(&s);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "shell");
        assert!(
            got[0].1.ends_with("pnpm install; exec \"${SHELL:-sh}\""),
            "{}",
            got[0].1
        );

        s.tree = Node::split(Dir::Right, 0.5, Node::leaf("editor"), Node::leaf("agent"));
        let got = line(&s);
        assert_eq!(got[0].0, "editor");
        assert!(got[0].1.contains("pnpm install; rm -f "), "{}", got[0].1);
        assert!(got[0].1.contains("exec nvim --listen"), "{}", got[0].1);

        s.tree = Node::leaf("agent");
        assert_eq!(line(&s)[0].0, "agent");
        assert!(
            s.open_specs(false).unwrap()[0]
                .cmd
                .as_deref()
                .is_some_and(|c| !c.contains("pnpm"))
        );
    }

    #[test]
    fn default_panes() {
        let s = stream(false);
        let agent = s.claude("claude", "--session-id 0000-s");
        let editor = listen("nvim -c 'DiffviewOpen origin/trunk...HEAD'");
        assert!(editor.contains("exec nvim --listen '"), "{editor}");
        assert!(editor.ends_with(".sock -c 'DiffviewOpen origin/trunk...HEAD'"));
        assert_eq!(listen("hx ."), "hx .");
        let panes = s.panes().unwrap();
        let got: Vec<(&str, Option<&str>)> = panes
            .iter()
            .map(|p| (p.role.as_str(), p.cmd.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("editor", Some(editor.as_str())),
                ("agent", Some(agent.as_str())),
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
        // Opened before jw kept the conversation's id: --continue.
        let panes = stream(true).panes().unwrap();
        let cmd = panes[1].cmd.as_deref().unwrap();
        assert!(cmd.starts_with("claude --continue --settings '"), "{cmd}");

        // With the id: that conversation, also after a daemon restart.
        let mut s = stream(true);
        s.entry.agent = "0000-s".into();
        let panes = s.panes().unwrap();
        let cmd = panes[1].cmd.as_deref().unwrap();
        assert!(
            cmd.starts_with("claude --resume 0000-s --settings '"),
            "{cmd}"
        );
        assert_eq!(panes[1].resume.as_deref(), Some(cmd));
    }

    #[test]
    fn a_new_agent_gets_an_id_and_the_hooks() {
        let panes = stream(false).panes().unwrap();
        let cmd = panes[1].cmd.as_deref().unwrap();
        assert!(
            cmd.starts_with("claude --session-id 0000-s --settings '"),
            "{cmd}"
        );
        assert!(cmd.contains("hook waiting"), "{cmd}");
        let resume = panes[1].resume.as_deref().unwrap();
        assert!(resume.starts_with("claude --resume 0000-s "), "{resume}");
        let json: serde_json::Value = serde_json::from_str(&hooks()).unwrap();
        assert!(json["hooks"]["Stop"][0]["hooks"][0]["command"].is_string());
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
