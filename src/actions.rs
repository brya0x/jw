//! What new and rm do to the disk, with the checks of their Go commands
//! (internal/commands/new.go, rm.go; REQ-11). No terminal here: the TUI asks
//! the questions and these functions do the work, which can take seconds
//! (fetch, worktree add), so the TUI runs them off its main thread.
//!
//! The registry is loaded fresh right before each write: Go and other jw
//! clients write it too (RISK-5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::connectors::git::{self, Repo};
use crate::core::config::{self, Config};
use crate::core::envfile;
use crate::core::expand::{Vars, expand};
use crate::core::registry::{self, Entry, Registry};

/// A repository with its config: what every action needs.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub repo: Repo,
    pub cfg: Config,
}

impl Project {
    /// The project that `dir` (any checkout or worktree of it) belongs to.
    pub fn open(dir: &Path) -> Result<Self> {
        let repo = Repo::open(dir)?;
        let name = git::project_name(&repo.remote);
        let cfg = config::load(&repo.root, &repo.remote, &name)?;
        Ok(Self { name, repo, cfg })
    }
}

/// Lowercase letters, digits and dashes, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    let mut cs = name.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Debug, Clone, Default)]
pub struct NewOptions {
    pub name: String,
    /// Ref to start from; empty is origin/<default branch>.
    pub from: String,
    /// Empty takes the config's template; an existing branch is checked out
    /// as is, but only when named here on purpose.
    pub branch: String,
}

/// Creates the worktree, writes .jw.env, copies env files and registers the
/// entry. Setup is not run here: the TUI runs it in the stream's shell pane
/// so its output is visible. Anything that fails after the worktree exists
/// is rolled back.
pub fn new_stream(p: &Project, o: &NewOptions, reg_path: &Path) -> Result<Entry> {
    if !valid_name(&o.name) {
        bail!(
            "invalid name {:?}: use lowercase letters, digits and dashes",
            o.name
        );
    }
    // Everything that can fail cheaply, before touching the disk.
    let reg = Registry::load(reg_path)?;
    if reg.has(&p.name, &o.name) {
        bail!("{}/{} already exists", p.name, o.name);
    }
    let base = p.repo.default_branch()?;
    let slot = reg.next_slot();
    let vars = p.cfg.vars(&o.name, &base, slot);
    let branch = if o.branch.is_empty() {
        expand(&p.cfg.branch, &vars)?
    } else {
        o.branch.clone()
    };
    let path = PathBuf::from(&p.cfg.root).join(&o.name);
    if path.exists() {
        bail!("{} already exists", path.display());
    }

    // Fetch before looking at branches, so one that only exists on origin is seen.
    p.repo.fetch()?;
    let existing = p.repo.branch_exists(&branch) || p.repo.remote_branch_exists(&branch);
    if existing && o.branch.is_empty() {
        bail!("branch {branch} already exists; name it in the branch field to work on it");
    }
    if existing && !o.from.is_empty() {
        bail!("`from` doesn't apply to an existing branch ({branch} already has its history)");
    }
    let from = if o.from.is_empty() {
        format!("origin/{base}")
    } else {
        o.from.clone()
    };
    if existing {
        p.repo.add_worktree_existing(&path, &branch)?;
    } else {
        p.repo.add_worktree(&path, &branch, &from)?;
    }

    let entry = Entry {
        id: registry::new_id()?,
        name: o.name.clone(),
        project: p.name.clone(),
        branch: branch.clone(),
        path: path.display().to_string(),
        slot,
        adopted: existing,
        created: registry::now_rfc3339(),
        ..Entry::default()
    };
    // From here a worktree exists: undo it if anything fails, so a
    // half-created stream never lingers outside the registry.
    if let Err(e) = provision(p, &entry, &vars, reg_path) {
        let _ = p.repo.remove_worktree(&path);
        if !existing {
            // A branch jw didn't create is never jw's to delete.
            let _ = p.repo.delete_branch(&branch);
        }
        return Err(e.context(format!("rolled back {}", o.name)));
    }
    Ok(entry)
}

fn provision(p: &Project, e: &Entry, vars: &Vars, reg_path: &Path) -> Result<()> {
    let env: String = jw_env(e, vars)
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(Path::new(&e.path).join(".jw.env"), env)?;
    p.repo.exclude(".jw.env")?;
    copy_env_files(p, e, vars)?;
    let mut reg = Registry::load(reg_path)?;
    if reg.entries.iter().any(|x| x.slot == e.slot) {
        bail!(
            "slot {} was taken while creating {}; try again",
            e.slot,
            e.name
        );
    }
    reg.add(e.clone());
    reg.save(reg_path)
}

/// The JW_* variables of a worktree, as in .jw.env and every pane.
pub fn jw_env(e: &Entry, vars: &Vars) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("JW_ID".to_string(), e.id.clone()),
        ("JW_NAME".to_string(), e.name.clone()),
        ("JW_PROJECT".to_string(), e.project.clone()),
        ("JW_SLOT".to_string(), e.slot.to_string()),
        (
            "JW_PORT_BASE".to_string(),
            config::port_base(e.slot).to_string(),
        ),
    ]);
    for (svc, port) in &vars.ports {
        env.insert(crate::core::expand::env_name(svc), port.to_string());
    }
    env
}

/// Brings gitignored env files from the main checkout into the worktree,
/// pointing their ports at this slot. A file missing in the main checkout is
/// skipped: not every clone has every app set up.
fn copy_env_files(p: &Project, e: &Entry, vars: &Vars) -> Result<()> {
    for f in &p.cfg.env {
        let src = p.repo.root.join(&f.file);
        let data = match std::fs::read_to_string(&src) {
            Ok(d) => d,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| src.display().to_string()),
        };
        let mut set = BTreeMap::new();
        for (k, v) in &f.set {
            set.insert(k.clone(), expand(v, vars)?);
        }
        let dst = Path::new(&e.path).join(&f.file);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&dst, envfile::set(&data, &set))?;
        set_mode(&dst, 0o600)?;
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// The setup commands, expanded, as one line for the stream's shell pane.
pub fn setup_line(cfg: &Config, vars: &Vars) -> Result<Option<String>> {
    if cfg.setup.is_empty() {
        return Ok(None);
    }
    let cmds = cfg
        .setup
        .iter()
        .map(|c| expand(c, vars))
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(cmds.join(" && ")))
}

/// What `rm` would lose and delete, computed before asking.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RmPlan {
    pub on_disk: bool,
    pub dirty: Vec<String>,
    pub unpushed: Vec<String>,
    pub keep_branch: bool,
    /// What a yes deletes, for the question.
    pub deletes: Vec<String>,
}

impl RmPlan {
    /// Work that only exists here: rm refuses unless forced (in the TUI,
    /// by typing the stream's name).
    pub fn loses_work(&self) -> bool {
        !self.dirty.is_empty() || (!self.unpushed.is_empty() && !self.keep_branch)
    }
}

pub fn rm_plan(p: &Project, e: &Entry) -> Result<RmPlan> {
    let path = Path::new(&e.path);
    let on_disk = path.exists();
    let (dirty, unpushed) = if on_disk {
        (git::dirty_files(path)?, git::unpushed(path)?)
    } else {
        (Vec::new(), Vec::new())
    };
    let keep_branch = e.adopted;
    let mut deletes = Vec::new();
    if on_disk {
        deletes.push(format!("worktree {}", e.path));
    }
    if !keep_branch && p.repo.branch_exists(&e.branch) {
        deletes.push(format!("local branch {}", e.branch));
    }
    Ok(RmPlan {
        on_disk,
        dirty,
        unpushed,
        keep_branch,
        deletes,
    })
}

/// Removes the worktree, the branch jw made and the registry entry. The
/// stream's panes must be gone already, so nothing holds the directory.
/// Returns a note about anything kept.
pub fn rm(p: &Project, e: &Entry, plan: &RmPlan, reg_path: &Path) -> Result<Option<String>> {
    if plan.on_disk {
        p.repo.remove_worktree(Path::new(&e.path))?;
    } else {
        p.repo.prune_worktrees()?;
    }
    if !plan.keep_branch && p.repo.branch_exists(&e.branch) {
        p.repo.delete_branch(&e.branch)?;
    }
    let mut note = None;
    // The branch jw first created, if the worktree moved off it: only when
    // git sees it merged.
    if !e.original.is_empty()
        && e.original != e.branch
        && p.repo.branch_exists(&e.original)
        && p.repo.delete_merged_branch(&e.original).is_err()
    {
        note = Some(format!(
            "kept branch {} (git says it isn't merged)",
            e.original
        ));
    }
    let mut reg = Registry::load(reg_path)?;
    reg.remove(&e.id);
    reg.save(reg_path).map_err(|err| {
        anyhow!(
            "removed {} but could not update the registry: {err}",
            e.name
        )
    })?;
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::git::tests::{must_git, new_test_repo};

    fn project(work: &Path, root: &Path) -> Project {
        let mut cfg: Config = toml::from_str(
            "setup = [\"echo {name} {port.web}\"]\n[ports]\nweb = 0\n[[env]]\nfile = \".env\"\nset = { PORT = \"{port.web}\" }\n",
        )
        .unwrap();
        cfg.root = root.display().to_string();
        cfg.branch = "feat/{name}".into();
        Project {
            name: "myapp".into(),
            repo: Repo::open(work).unwrap(),
            cfg,
        }
    }

    #[test]
    fn names() {
        assert!(valid_name("web-2") && valid_name("2fa"));
        assert!(!valid_name("") && !valid_name("-x") && !valid_name("Web") && !valid_name("a_b"));
    }

    #[test]
    fn new_then_rm() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/registry.json");
        // Env files are gitignored in a real repo; they only exist locally.
        std::fs::write(work.join(".gitignore"), ".env\n").unwrap();
        must_git(&work, &["add", ".gitignore"]);
        must_git(
            &work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "ignore",
            ],
        );
        must_git(&work, &["push", "-q"]);
        std::fs::write(work.join(".env"), "# local\nPORT=3000\n").unwrap();
        let p = project(&work, &dir.path().join("wt"));

        let o = NewOptions {
            name: "web".into(),
            ..NewOptions::default()
        };
        let e = new_stream(&p, &o, &reg_path).unwrap();
        assert_eq!(
            (e.branch.as_str(), e.slot, e.adopted),
            ("feat/web", 1, false)
        );
        let wt = Path::new(&e.path);
        assert_eq!(git::current_branch(wt).unwrap(), "feat/web");
        let jw_env = std::fs::read_to_string(wt.join(".jw.env")).unwrap();
        assert!(jw_env.contains("JW_PORT_WEB=20100\n"), "{jw_env}");
        assert_eq!(
            std::fs::read_to_string(wt.join(".env")).unwrap(),
            "# local\nPORT=20100\n"
        );
        assert!(!git::dirty(wt).unwrap(), ".jw.env is excluded");
        assert_eq!(
            Registry::load(&reg_path).unwrap().entries,
            std::slice::from_ref(&e)
        );

        let again = new_stream(&p, &o, &reg_path).unwrap_err();
        assert!(again.to_string().contains("already exists"), "{again}");

        let vars = p.cfg.vars("web", "trunk", 1);
        assert_eq!(
            setup_line(&p.cfg, &vars).unwrap().as_deref(),
            Some("echo web 20100")
        );

        // A dirty worktree loses work: the TUI refuses without the name typed.
        std::fs::write(wt.join("wip.txt"), "x").unwrap();
        let plan = rm_plan(&p, &e).unwrap();
        assert!(plan.loses_work());
        assert_eq!(plan.deletes.len(), 2);

        rm(&p, &e, &plan, &reg_path).unwrap();
        assert!(!wt.exists());
        assert!(!p.repo.branch_exists("feat/web"));
        assert!(Registry::load(&reg_path).unwrap().entries.is_empty());
    }

    #[test]
    fn existing_branch_must_be_named() {
        let (dir, work) = new_test_repo();
        must_git(&work, &["branch", "feat/api"]);
        let reg_path = dir.path().join("registry.json");
        let p = project(&work, &dir.path().join("wt"));

        let mut o = NewOptions {
            name: "api".into(),
            ..NewOptions::default()
        };
        let err = new_stream(&p, &o, &reg_path).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");

        o.branch = "feat/api".into();
        let e = new_stream(&p, &o, &reg_path).unwrap();
        assert!(e.adopted);
        let plan = rm_plan(&p, &e).unwrap();
        assert!(
            plan.keep_branch,
            "an adopted branch is never jw's to delete"
        );
        rm(&p, &e, &plan, &reg_path).unwrap();
        assert!(p.repo.branch_exists("feat/api"));
    }
}
