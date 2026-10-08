//! The per-project jw config: where worktrees go, how to set them up, which
//! ports each service gets and which env files to copy.
//! Port of internal/core/config/config.go; reads the same TOML files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use super::expand::{Vars, expand};

/// Every slot owns a block of ports: PORT_BLOCK_START + slot*PORT_BLOCK_SIZE + offset.
pub const PORT_BLOCK_START: u32 = 20000;
pub const PORT_BLOCK_SIZE: u32 = 100;

/// Unknown keys are rejected, so a typo like `setpu` fails loudly instead
/// of being silently ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub r#match: String,
    pub root: String,
    pub workspace: String,
    pub branch: String,
    pub setup: Vec<String>,
    /// Service → offset inside the slot's port block.
    pub ports: BTreeMap<String, i64>,
    pub dev: BTreeMap<String, Vec<String>>,
    pub env: Vec<EnvFile>,
    pub agent: Agent,
    pub layout: Layout,
    /// How sync updates a branch: "rebase" (default) or "merge".
    pub sync: String,
    pub tui: Tui,

    /// The file this config came from, `None` for defaults.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

/// Copied from the main checkout into each worktree. Keys in `set` are
/// overwritten (or appended) after placeholders are expanded.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnvFile {
    pub file: String,
    pub set: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Agent {
    pub default: String,
    pub claude: AgentCmd,
    pub codex: AgentCmd,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCmd {
    pub start: String,
    pub resume: String,
}

/// The split tree joins this table in P4 (docs/specs/rust-tui.md).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub editor: String,
}

/// Rust-only settings; the Go binary skips this table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tui {
    pub leader: Option<String>,
    pub animations: Option<bool>,
}

/// The config directory, honouring XDG_CONFIG_HOME.
pub fn dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("jw"));
    }
    Ok(home()?.join(".config").join("jw"))
}

/// Finds the config for a repository, in order:
///
/// 1. `<repo_root>/.jw.toml`
/// 2. a file in [`dir`] whose `match` equals the origin URL
/// 3. `dir()/<project>.toml` without a `match`
/// 4. defaults
pub fn load(repo_root: &Path, remote: &str, project: &str) -> Result<Config> {
    let c = match decode_file(&repo_root.join(".jw.toml"))? {
        Some(c) => c,
        None => find_personal(&dir()?, remote, project)?.unwrap_or_default(),
    };
    c.with_defaults(repo_root, project)
}

fn find_personal(dir: &Path, remote: &str, project: &str) -> Result<Option<Config>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| dir.display().to_string()),
    };
    files.sort();

    let (mut by_match, mut by_name): (Option<Config>, Option<Config>) = (None, None);
    for f in files {
        let Some(c) = decode_file(&f)? else { continue };
        if !c.r#match.is_empty() && same_repo(&c.r#match, remote) {
            if let Some(prev) = &by_match {
                bail!(
                    "both {} and {} match {remote}",
                    show(&prev.source),
                    f.display()
                );
            }
            by_match = Some(c);
        } else if c.r#match.is_empty() && f.file_stem().is_some_and(|s| s == project) {
            by_name = Some(c);
        }
    }
    Ok(by_match.or(by_name))
}

/// Parses one config file; `None` when it doesn't exist.
fn decode_file(path: &Path) -> Result<Option<Config>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    let mut c: Config =
        toml::from_str(&text).map_err(|e| anyhow!("{}: {}", path.display(), e.message()))?;
    c.source = Some(path.to_path_buf());
    Ok(Some(c))
}

impl Config {
    fn with_defaults(mut self, repo_root: &Path, project: &str) -> Result<Self> {
        if self.root.is_empty() {
            self.root = format!("{}-wt", repo_root.display());
        }
        let root = expand_home(&self.root);
        self.root = if root.is_absolute() {
            root
        } else {
            repo_root.join(root)
        }
        .display()
        .to_string();
        if self.workspace.is_empty() {
            self.workspace = project.to_string();
        }
        if self.branch.is_empty() {
            self.branch = "feat/{name}".into();
        }
        if self.agent.default.is_empty() {
            self.agent.default = "claude".into();
        }
        if self.agent.claude == AgentCmd::default() {
            self.agent.claude = agent_cmd("claude", "claude --continue");
        }
        if self.agent.codex == AgentCmd::default() {
            self.agent.codex = agent_cmd("codex", "codex resume --last");
        }
        if self.sync.is_empty() {
            self.sync = "rebase".into();
        }
        if self.layout.editor.is_empty() {
            self.layout.editor = "nvim -c 'DiffviewOpen origin/{base}...HEAD'".into();
        }
        self.validate()?;
        Ok(self)
    }

    /// Catches bad offsets and unknown placeholders at load time, not
    /// halfway through creating a worktree.
    fn validate(&self) -> Result<()> {
        let src = show(&self.source);
        for (svc, &off) in &self.ports {
            if !(0..PORT_BLOCK_SIZE as i64).contains(&off) {
                bail!(
                    "{src}: port offset {svc} = {off}, must be 0–{}",
                    PORT_BLOCK_SIZE - 1
                );
            }
        }
        if self.sync != "rebase" && self.sync != "merge" {
            bail!(
                "{src}: sync = {:?}, must be \"rebase\" or \"merge\"",
                self.sync
            );
        }

        let probe = self.vars("name", "main", 1);
        let check = |what: &str, s: &str| -> Result<()> {
            expand(s, &probe).map_err(|e| anyhow!("{src}: {what}: {e}"))?;
            Ok(())
        };

        check("workspace", &self.workspace)?;
        if self.workspace.contains("{name}") {
            // The label also names the stream's agent, which must be a plain identifier.
            let label = expand(&self.workspace, &probe)?;
            if !is_agent_label(&label) {
                bail!(
                    "{src}: workspace = {:?}: with {{name}} it also names the agent, so use only a-z, 0-9, - and _, starting with a letter",
                    self.workspace
                );
            }
        }
        check("branch", &self.branch)?;
        check("layout.editor", &self.layout.editor)?;
        for (i, s) in self.setup.iter().enumerate() {
            check(&format!("setup[{i}]"), s)?;
        }
        for (svc, cmds) in &self.dev {
            for s in cmds {
                check(&format!("dev.{svc}"), s)?;
            }
        }
        for e in &self.env {
            if e.file.is_empty() {
                bail!("{src}: [[env]] entry without file");
            }
            for (k, v) in &e.set {
                check(&format!("{} {k}", e.file), v)?;
            }
        }
        Ok(())
    }

    /// The concrete port of every service for a slot.
    pub fn ports_for(&self, slot: u32) -> BTreeMap<String, u32> {
        self.ports
            .iter()
            .map(|(svc, &off)| (svc.clone(), port_base(slot) + off as u32))
            .collect()
    }

    /// The placeholder values for one worktree.
    pub fn vars(&self, name: &str, base: &str, slot: u32) -> Vars {
        Vars {
            name: name.into(),
            base: base.into(),
            slot,
            ports: self.ports_for(slot),
        }
    }
}

pub fn port_base(slot: u32) -> u32 {
    PORT_BLOCK_START + slot * PORT_BLOCK_SIZE
}

/// Compares two remote URLs ignoring scheme, user, .git and ssh vs https.
pub fn same_repo(a: &str, b: &str) -> bool {
    normalize_remote(a) == normalize_remote(b)
}

fn normalize_remote(u: &str) -> String {
    let mut u = u.trim().to_lowercase();
    for p in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = u.strip_prefix(p) {
            u = rest.to_string();
        }
    }
    if let Some(at) = u.find('@') {
        u = u[at + 1..].to_string();
    }
    let u = u.replacen(':', "/", 1);
    let u = u.trim_end_matches('/');
    u.strip_suffix(".git").unwrap_or(u).to_string()
}

/// `^[a-z][a-z0-9_-]*$`
fn is_agent_label(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase())
        && cs.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

fn agent_cmd(start: &str, resume: &str) -> AgentCmd {
    AgentCmd {
        start: start.into(),
        resume: resume.into(),
    }
}

fn expand_home(p: &str) -> PathBuf {
    if (p == "~" || p.starts_with("~/"))
        && let Ok(home) = home()
    {
        return home.join(p.trim_start_matches('~').trim_start_matches('/'));
    }
    PathBuf::from(p)
}

pub(crate) fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("$HOME is not set"))
}

fn show(source: &Option<PathBuf>) -> String {
    source
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Each test gets its own repo dir and an empty personal config dir.
    /// Tests that set XDG_CONFIG_HOME pass it through `with_config_home`,
    /// since the environment is shared between test threads.
    struct Env {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        personal: PathBuf,
    }

    fn setup() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("myapp");
        let personal = tmp.path().join("config").join("jw");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&personal).unwrap();
        Env {
            _tmp: tmp,
            repo,
            personal,
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `load` with the personal dir passed explicitly instead of through
    /// XDG_CONFIG_HOME.
    fn load_in(env: &Env, remote: &str) -> Result<Config> {
        let c = match decode_file(&env.repo.join(".jw.toml"))? {
            Some(c) => c,
            None => find_personal(&env.personal, remote, "myapp")?.unwrap_or_default(),
        };
        c.with_defaults(&env.repo, "myapp")
    }

    fn repo_file(env: &Env, text: &str) -> Result<Config> {
        write(&env.repo.join(".jw.toml"), text);
        load_in(env, "x")
    }

    #[test]
    fn defaults() {
        let env = setup();
        let c = load_in(&env, "git@github.com:acme/myapp.git").unwrap();
        assert!(c.source.is_none());
        assert_eq!(c.root, format!("{}-wt", env.repo.display()));
        assert_eq!(c.branch, "feat/{name}");
        assert_eq!(c.workspace, "myapp");
        assert_eq!(c.sync, "rebase");
        assert_eq!(c.agent.claude.resume, "claude --continue");
    }

    #[test]
    fn repo_file_wins_over_personal() {
        let env = setup();
        write(&env.repo.join(".jw.toml"), r#"branch = "repo/{name}""#);
        write(
            &env.personal.join("myapp.toml"),
            r#"branch = "personal/{name}""#,
        );
        let c = load_in(&env, "https://github.com/acme/myapp").unwrap();
        assert_eq!(c.branch, "repo/{name}");
    }

    #[test]
    fn personal_matched_by_remote() {
        let env = setup();
        // The file name doesn't matter when `match` is set; ssh vs https doesn't either.
        write(
            &env.personal.join("work.toml"),
            "match = \"github.com/acme/myapp\"\nroot = \"worktrees\"\n",
        );
        let c = load_in(&env, "git@github.com:acme/myapp.git").unwrap();
        assert_eq!(c.root, env.repo.join("worktrees").display().to_string());
    }

    #[test]
    fn two_matches_fail() {
        let env = setup();
        for f in ["a.toml", "b.toml"] {
            write(&env.personal.join(f), "match = \"github.com/acme/myapp\"\n");
        }
        let err = load_in(&env, "https://github.com/acme/myapp").unwrap_err();
        assert!(err.to_string().contains("both"), "{err}");
    }

    #[test]
    fn unknown_key_fails() {
        let env = setup();
        let err = repo_file(&env, r#"setpu = ["pnpm install"]"#).unwrap_err();
        assert!(err.to_string().contains("setpu"), "{err}");
    }

    #[test]
    fn unknown_nested_key_fails() {
        let env = setup();
        let err = repo_file(&env, "[agent.claude]\nstrat = \"claude\"\n").unwrap_err();
        assert!(err.to_string().contains("strat"), "{err}");
    }

    #[test]
    fn tui_table_is_read() {
        let env = setup();
        let c = repo_file(&env, "[tui]\nleader = \"C-Space\"\nanimations = false\n").unwrap();
        assert_eq!(c.tui.leader.as_deref(), Some("C-Space"));
        assert_eq!(c.tui.animations, Some(false));
    }

    #[test]
    fn unknown_placeholder_fails() {
        let env = setup();
        let err = repo_file(
            &env,
            "[ports]\nweb = 0\n\n[dev]\nweb = [\"vite --port {port.wbe}\"]\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("{port.wbe}"), "{err}");
    }

    #[test]
    fn offset_out_of_block_fails() {
        let env = setup();
        assert!(repo_file(&env, "[ports]\nweb = 100\n").is_err());
        assert!(repo_file(&env, "[ports]\nweb = -1\n").is_err());
    }

    #[test]
    fn sync_mode() {
        let env = setup();
        let err = repo_file(&env, r#"sync = "squash""#).unwrap_err();
        assert!(err.to_string().contains("rebase"), "{err}");
    }

    #[test]
    fn workspace_per_stream_must_be_an_agent_name() {
        let env = setup();
        for (label, ok) in [
            ("myapp-{name}", true),
            ("{name}", true),
            ("My App", true), // no {name}: only labels the workspace
            ("myapp {name}", false),
            ("MyApp-{name}", false),
        ] {
            let got = repo_file(&env, &format!("workspace = \"{label}\"\n"));
            assert_eq!(got.is_ok(), ok, "workspace {label:?}: {got:?}");
        }
    }

    #[test]
    fn same_repo_ignores_scheme_user_and_suffix() {
        for u in [
            "git@github.com:Acme/MyApp.git",
            "https://github.com/acme/myapp",
            "ssh://git@github.com/acme/myapp.git",
        ] {
            assert!(same_repo("github.com/acme/myapp", u), "{u}");
        }
        assert!(!same_repo(
            "github.com/acme/myapp",
            "github.com/acme/myapp-api"
        ));
    }

    #[test]
    fn ports_for_a_slot() {
        let env = setup();
        let c = repo_file(&env, "[ports]\nweb = 0\napi = 3\n").unwrap();
        let p = c.ports_for(4);
        assert_eq!((p["web"], p["api"]), (20400, 20403));
    }

    /// A config the Go binary accepts, including the keys only Rust reads.
    #[test]
    fn reads_the_go_fixture() {
        let env = setup();
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/config.go.toml"),
        )
        .unwrap();
        let c = repo_file(&env, &text).unwrap();
        assert_eq!(c.setup.len(), 2);
        assert_eq!(c.dev["web"].len(), 1);
        assert_eq!(c.env[0].set["API_URL"], "http://localhost:{port.api}");
        assert_eq!(c.layout.editor, "nvim");
        assert_eq!(c.agent.default, "codex");
    }
}
