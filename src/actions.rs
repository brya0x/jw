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

/// Gives every entry its project's main checkout (RISK-17): one `git` per
/// project, once, for entries seeded from Go. Whether anything changed.
pub fn fill_roots(reg: &mut Registry) -> bool {
    let mut known: BTreeMap<String, String> = reg
        .entries
        .iter()
        .filter(|e| !e.root.is_empty())
        .map(|e| (e.project.clone(), e.root.clone()))
        .collect();
    let mut changed = false;
    for e in reg.entries.iter_mut().filter(|e| e.root.is_empty()) {
        let root = match known.get(&e.project) {
            Some(r) => r.clone(),
            None => match Repo::open(Path::new(&e.path)) {
                Ok(r) => r.root.display().to_string(),
                Err(_) => continue,
            },
        };
        known.insert(e.project.clone(), root.clone());
        e.root = root;
        changed = true;
    }
    changed
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
        root: p.repo.root.display().to_string(),
        session: crate::session::stored(&crate::session::current()),
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

/// What a rename changes, shown before it happens.
#[derive(Debug, Clone, PartialEq)]
pub struct Renamed {
    pub path: PathBuf,
    /// The new branch; the same one when jw didn't name it.
    pub branch: String,
}

/// Where `e` goes when it's called `name` (REQ-40): the folder beside the
/// old one, and the branch from the template, but only when jw made the
/// branch from it.
pub fn rename_plan(p: &Project, e: &Entry, name: &str, reg_path: &Path) -> Result<Renamed> {
    if !valid_name(name) {
        bail!("invalid name {name:?}: use lowercase letters, digits and dashes");
    }
    if name != e.name && Registry::load(reg_path)?.has(&p.name, name) {
        bail!("{}/{name} already exists", p.name);
    }
    let old = Path::new(&e.path);
    let path = old.with_file_name(name);
    if path != old && path.exists() {
        bail!("{} already exists", path.display());
    }
    let base = p.repo.default_branch().unwrap_or_default();
    let ours = !e.adopted
        && expand(&p.cfg.branch, &p.cfg.vars(&e.name, &base, e.slot))
            .ok()
            .as_deref()
            == Some(e.branch.as_str());
    let branch = if ours {
        expand(&p.cfg.branch, &p.cfg.vars(name, &base, e.slot))?
    } else {
        e.branch.clone()
    };
    if branch != e.branch && p.repo.branch_exists(&branch) {
        bail!("branch {branch} already exists");
    }
    Ok(Renamed { path, branch })
}

/// Renames a worktree: its folder, its branch (when jw named it), its
/// registry entry (same id and slot) and its .jw.env.
pub fn rename(p: &Project, e: &Entry, name: &str, reg_path: &Path) -> Result<Entry> {
    let plan = rename_plan(p, e, name, reg_path)?;
    let old = PathBuf::from(&e.path);
    if plan.path != old {
        p.repo.move_worktree(&old, &plan.path)?;
    }
    if plan.branch != e.branch
        && let Err(err) = p.repo.rename_branch(&e.branch, &plan.branch)
    {
        let _ = p.repo.move_worktree(&plan.path, &old);
        return Err(err.context("rolled back the folder"));
    }
    let mut renamed = e.clone();
    renamed.name = name.to_string();
    renamed.path = plan.path.display().to_string();
    if renamed.original == e.branch {
        renamed.original = plan.branch.clone();
    }
    renamed.branch = plan.branch;
    let base = p.repo.default_branch().unwrap_or_default();
    let vars = p.cfg.vars(name, &base, e.slot);
    let env: String = jw_env(&renamed, &vars)
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(plan.path.join(".jw.env"), env)?;
    let mut reg = Registry::load(reg_path)?;
    if let Some(x) = reg.entries.iter_mut().find(|x| x.id == e.id) {
        *x = renamed.clone();
    }
    reg.save(reg_path)?;
    Ok(renamed)
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

/// How a sync ended, in words for the status line.
#[derive(Debug, Clone, PartialEq)]
pub enum Synced {
    UpToDate {
        base: String,
    },
    Merged {
        behind: u32,
        base: String,
    },
    Rebased {
        ahead: u32,
        base: String,
        needs_force: bool,
    },
}

impl Synced {
    pub fn message(&self) -> String {
        match self {
            Synced::UpToDate { base } => format!("already up to date with {base}"),
            Synced::Merged { behind, base } => {
                format!("merged {behind} commit(s) from {base}; publish with git push")
            }
            Synced::Rebased {
                ahead,
                base,
                needs_force: true,
            } => format!(
                "{ahead} commit(s) replayed onto {base}; it was already pushed: git push --force-with-lease"
            ),
            Synced::Rebased { ahead, base, .. } => {
                format!("{ahead} commit(s) replayed onto {base}")
            }
        }
    }
}

/// Brings the stream's branch up to date with origin's default branch, by
/// the config's `sync` mode (internal/commands/sync.go). It never pushes.
/// A rebase or merge that stops on conflicts is left in place for the agent
/// (or you) to resolve, and the error lists the files.
pub fn sync(p: &Project, e: &Entry) -> Result<Synced> {
    let path = Path::new(&e.path);
    if !path.exists() {
        bail!("{}: worktree {} is missing", e.name, e.path);
    }
    if let Some(op) = git::operation(path) {
        bail!(
            "{} has a {op} in progress: finish it (resolve, git add, git {op} --continue) or abort it (git {op} --abort)",
            e.name
        );
    }
    if git::dirty(path)? {
        bail!("{} has uncommitted changes — commit them first", e.name);
    }
    let base = format!("origin/{}", p.repo.default_branch()?);
    p.repo.fetch()?;
    let (behind, ahead) = git::divergence(path, &base)?;
    if behind == 0 {
        return Ok(Synced::UpToDate { base });
    }
    // A rebase rewrites commits that may already be on origin; know that
    // before starting.
    let published = p.repo.remote_branch_exists(&e.branch);
    let merge = p.cfg.sync == "merge";
    let result = if merge {
        git::merge(path, &base)
    } else {
        git::rebase(path, &base)
    };
    if let Err(cause) = result {
        let mode = if merge { "merge" } else { "rebase" };
        let files = git::conflicts(path).unwrap_or_default();
        if files.is_empty() {
            return Err(cause.context(format!("{mode} failed")));
        }
        bail!(
            "{mode} stopped at conflicts in {}: resolve, git add, git {mode} --continue (or --abort); the agent can do it",
            files.join(", ")
        );
    }
    Ok(if merge {
        Synced::Merged { behind, base }
    } else {
        Synced::Rebased {
            ahead,
            base,
            needs_force: published && ahead > 0,
        }
    })
}

/// Checks that a stream is finished (internal/commands/done.go): nothing
/// uncommitted, its PR merged, and its HEAD part of what was merged. On
/// success it returns the plan for [`rm`] and the PR, for the question.
pub fn done_plan(
    p: &Project,
    e: &Entry,
    prs: &dyn crate::connectors::PullRequests,
) -> Result<(RmPlan, crate::connectors::Pr)> {
    let path = Path::new(&e.path);
    let on_disk = path.exists();
    if on_disk && git::dirty(path)? {
        bail!(
            "{} has uncommitted changes — commit or discard them first",
            e.name
        );
    }
    let Some(pr) = prs.for_branch(&p.repo.root, &e.branch)? else {
        bail!("no pull request for {}", e.branch);
    };
    if pr.state != "MERGED" {
        bail!(
            "PR #{} is {}, not merged yet ({})",
            pr.number,
            pr.status(),
            pr.url
        );
    }
    // The local HEAD must be in what was merged. Comparing with the PR's head,
    // not an upstream branch, keeps working once the forge deletes the branch.
    if on_disk {
        let head = git::head(path)?;
        if head != pr.head_sha {
            if !p.repo.has_commit(&pr.head_sha) {
                let _ = p.repo.fetch_commit(&pr.head_sha);
            }
            if !p.repo.is_ancestor(&head, &pr.head_sha) {
                let merged = pr.merged_at.as_deref().and_then(registry::parse_rfc3339);
                if let (Some(m), Some(c)) = (merged, registry::parse_rfc3339(&e.created))
                    && m < c
                {
                    bail!(
                        "the only PR for {} is #{}, merged before this stream existed: the branch name was used before. Open a PR for this work, or remove the stream",
                        e.branch,
                        pr.number
                    );
                }
                bail!(
                    "{} has commits that are not in PR #{} — push them or open another PR",
                    e.name,
                    pr.number
                );
            }
        }
    }
    let mut deletes = Vec::new();
    if on_disk {
        deletes.push(format!("worktree {}", e.path));
    }
    if p.repo.branch_exists(&e.branch) {
        deletes.push(format!("local branch {}", e.branch));
    }
    // done deletes the branch even when it was adopted: its work is merged.
    let plan = RmPlan {
        on_disk,
        keep_branch: false,
        deletes,
        ..RmPlan::default()
    };
    Ok((plan, pr))
}

/// A service's commands from `[dev]`, expanded for this worktree.
pub fn dev_commands(cfg: &Config, service: &str, vars: &Vars) -> Result<Vec<String>> {
    let Some(raw) = cfg.dev.get(service).filter(|r| !r.is_empty()) else {
        if cfg.dev.is_empty() {
            bail!("no [dev] services in the config");
        }
        let names: Vec<&str> = cfg.dev.keys().map(String::as_str).collect();
        bail!("unknown service {service:?}; have: {}", names.join(", "));
    };
    raw.iter().map(|c| expand(c, vars)).collect()
}

/// One line for a shell pane that runs all of a service's commands and
/// stops them all with ctrl+c: `kill 0` takes the subshell's whole group,
/// where plain `a & b & wait` would leave the background ones running.
pub fn dev_line(cmds: &[String]) -> String {
    match cmds {
        [one] => one.clone(),
        many => format!("(trap 'kill 0' INT TERM; {} & wait)", many.join(" & ")),
    }
}

/// Fails if a port the service uses is taken, naming who holds it: a server
/// that finds its port taken fails late, from inside the app.
pub fn check_ports(
    p: &Project,
    e: &Entry,
    service: &str,
    vars: &Vars,
    shell: &dyn crate::connectors::Shell,
    reg: &Registry,
) -> Result<()> {
    let mut taken = Vec::new();
    let raw = p.cfg.dev.get(service).cloned().unwrap_or_default();
    let mut seen = Vec::new();
    for svc in raw.iter().flat_map(|c| crate::core::expand::ports_in(c)) {
        if seen.contains(&svc) {
            continue;
        }
        seen.push(svc.clone());
        let Some(&port) = vars.ports.get(&svc) else {
            continue;
        };
        let Some(owner) = shell.port_owner(port as u16) else {
            continue;
        };
        taken.push(format!(
            "{port} ({svc}): {}",
            describe_owner(e, &owner, reg)
        ));
    }
    if taken.is_empty() {
        return Ok(());
    }
    bail!(
        "port(s) in use, not starting {service}: {}",
        taken.join("; ")
    )
}

/// Who holds a port, in terms of streams when it can.
fn describe_owner(e: &Entry, o: &crate::connectors::PortOwner, reg: &Registry) -> String {
    if o.pid == 0 {
        return "taken by a process jw can't see".into();
    }
    let mut who = format!("pid {}", o.pid);
    if !o.cmdline.is_empty() {
        who += &format!(" {}", o.cmdline);
    }
    if o.cwd.is_empty() {
        return who;
    }
    let inside = |root: &str| {
        let (dir, root) = (real(&o.cwd), real(root));
        dir == root || dir.starts_with(&root)
    };
    if inside(&e.path) {
        return format!("{who}, already running in this worktree");
    }
    if let Some(other) = reg.entries.iter().find(|x| inside(&x.path)) {
        return format!("{who}, started from stream {}", other.name);
    }
    format!("{who}, in {}", o.cwd)
}

fn real(p: &str) -> PathBuf {
    Path::new(p)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(p))
}

/// What `info` showed, as label/value rows (internal/commands/info.go).
pub fn info(
    p: &Project,
    e: &Entry,
    prs: &dyn crate::connectors::PullRequests,
    shell: &dyn crate::connectors::Shell,
) -> Vec<(String, String)> {
    let mut rows = vec![("id".to_string(), e.id.clone())];
    let source = p
        .cfg
        .source
        .as_ref()
        .map_or("none, defaults".to_string(), |s| s.display().to_string());
    rows.push((
        "project".into(),
        format!("{}  (config {source})", e.project),
    ));
    let mut branch = e.branch.clone();
    if e.adopted {
        branch += "  (adopted: existed before jw)";
    }
    rows.push(("branch".into(), branch));
    let mut path = e.path.clone();
    if !Path::new(&e.path).exists() {
        path += "  (missing)";
    }
    rows.push(("path".into(), path));
    let base = config::port_base(e.slot);
    rows.push((
        "slot".into(),
        format!("{}  (ports {base}–{})", e.slot, base + 99),
    ));
    for (svc, port) in p.cfg.ports_for(e.slot) {
        let up = if shell.port_owner(port as u16).is_some() {
            "  listening"
        } else {
            ""
        };
        rows.push(("port".into(), format!("{svc} {port}{up}")));
    }
    let pr = match prs.for_branch(&p.repo.root, &e.branch) {
        Ok(Some(pr)) => format!("#{} {}  {}", pr.number, pr.status(), pr.url),
        Ok(None) => "none".into(),
        Err(_) => "? (gh unavailable)".into(),
    };
    rows.push(("pr".into(), pr));
    if !p.cfg.dev.is_empty() {
        let svcs: Vec<&str> = p.cfg.dev.keys().map(String::as_str).collect();
        rows.push(("dev".into(), svcs.join(", ")));
    }
    rows
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
    fn fills_roots_from_the_worktree() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/workspaces.json");
        let p = project(&work, &dir.path().join("wt"));
        let o = NewOptions {
            name: "ws-1".into(),
            ..NewOptions::default()
        };
        let e = new_stream(&p, &o, &reg_path).unwrap();
        let root = p.repo.root.display().to_string();
        assert_eq!(e.root, root);

        // An entry seeded from Go has no root yet.
        let mut reg = Registry::load(&reg_path).unwrap();
        reg.entries[0].root.clear();
        assert!(fill_roots(&mut reg));
        assert_eq!(reg.entries[0].root, root);
        assert!(!fill_roots(&mut reg));
    }

    #[test]
    fn names() {
        assert!(valid_name("web-2") && valid_name("2fa"));
        assert!(!valid_name("") && !valid_name("-x") && !valid_name("Web") && !valid_name("a_b"));
    }

    #[test]
    fn rename_moves_the_folder_branch_and_entry() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("state/registry.json");
        let p = project(&work, &dir.path().join("wt"));
        let o = NewOptions {
            name: "ws-1".into(),
            ..NewOptions::default()
        };
        let e = new_stream(&p, &o, &reg_path).unwrap();

        assert!(rename_plan(&p, &e, "Bad", &reg_path).is_err());
        let plan = rename_plan(&p, &e, "login", &reg_path).unwrap();
        assert_eq!(plan.branch, "feat/login");

        let r = rename(&p, &e, "login", &reg_path).unwrap();
        assert_eq!((r.id.as_str(), r.slot), (e.id.as_str(), e.slot));
        assert_eq!(r.name, "login");
        let wt = Path::new(&r.path);
        assert!(wt.ends_with("wt/login") && wt.is_dir());
        assert!(!Path::new(&e.path).exists());
        assert_eq!(git::current_branch(wt).unwrap(), "feat/login");
        assert!(!p.repo.branch_exists("feat/ws-1"));
        let env = std::fs::read_to_string(wt.join(".jw.env")).unwrap();
        assert!(env.contains("JW_NAME=login\n"), "{env}");
        assert_eq!(
            Registry::load(&reg_path).unwrap().entries,
            std::slice::from_ref(&r)
        );

        // A second worktree can't take the name.
        let o2 = NewOptions {
            name: "other".into(),
            ..NewOptions::default()
        };
        let e2 = new_stream(&p, &o2, &reg_path).unwrap();
        assert!(rename_plan(&p, &e2, "login", &reg_path).is_err());
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

    struct FakePrs(Option<crate::connectors::Pr>);

    impl crate::connectors::PullRequests for FakePrs {
        fn for_branch(&self, _: &Path, _: &str) -> Result<Option<crate::connectors::Pr>> {
            Ok(self.0.clone())
        }
        fn by_branch(&self, _: &Path) -> Result<BTreeMap<String, crate::connectors::Pr>> {
            Ok(BTreeMap::new())
        }
    }

    fn commit(dir: &Path, msg: &str) {
        must_git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                msg,
            ],
        );
    }

    #[test]
    fn sync_rebases_onto_the_new_base() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("registry.json");
        let p = project(&work, &dir.path().join("wt"));
        let e = new_stream(
            &p,
            &NewOptions {
                name: "web".into(),
                ..NewOptions::default()
            },
            &reg_path,
        )
        .unwrap();
        let wt = Path::new(&e.path);

        assert_eq!(
            sync(&p, &e).unwrap(),
            Synced::UpToDate {
                base: "origin/trunk".into()
            }
        );

        commit(wt, "mine");
        commit(&work, "theirs");
        must_git(&work, &["push", "-q"]);
        let got = sync(&p, &e).unwrap();
        assert_eq!(
            got,
            Synced::Rebased {
                ahead: 1,
                base: "origin/trunk".into(),
                needs_force: false
            }
        );
        assert_eq!(git::divergence(wt, "origin/trunk").unwrap(), (0, 1));

        std::fs::write(wt.join("wip.txt"), "x").unwrap();
        let err = sync(&p, &e).unwrap_err();
        assert!(err.to_string().contains("uncommitted"), "{err}");
    }

    #[test]
    fn done_needs_a_merged_pr_containing_head() {
        let (dir, work) = new_test_repo();
        let reg_path = dir.path().join("registry.json");
        let p = project(&work, &dir.path().join("wt"));
        let e = new_stream(
            &p,
            &NewOptions {
                name: "web".into(),
                ..NewOptions::default()
            },
            &reg_path,
        )
        .unwrap();
        let head = git::head(Path::new(&e.path)).unwrap();

        let err = done_plan(&p, &e, &FakePrs(None)).unwrap_err();
        assert!(err.to_string().contains("no pull request"), "{err}");

        let mut pr = crate::connectors::Pr {
            number: 7,
            state: "OPEN".into(),
            head_sha: head.clone(),
            ..Default::default()
        };
        let err = done_plan(&p, &e, &FakePrs(Some(pr.clone()))).unwrap_err();
        assert!(err.to_string().contains("not merged"), "{err}");

        pr.state = "MERGED".into();
        let (plan, got) = done_plan(&p, &e, &FakePrs(Some(pr.clone()))).unwrap();
        assert_eq!(got.number, 7);
        assert_eq!(plan.deletes.len(), 2);

        // A local commit that isn't in the PR.
        commit(Path::new(&e.path), "later");
        let err = done_plan(&p, &e, &FakePrs(Some(pr))).unwrap_err();
        assert!(err.to_string().contains("not in PR #7"), "{err}");
    }

    #[test]
    fn dev_lines() {
        let cfg: Config = toml::from_str(
            "[ports]\nweb = 0\n[dev]\nweb = [\"vite --port {port.web}\"]\nboth = [\"a\", \"b\"]\n",
        )
        .unwrap();
        let vars = cfg.vars("x", "main", 2);
        let web = dev_commands(&cfg, "web", &vars).unwrap();
        assert_eq!(dev_line(&web), "vite --port 20200");
        let both = dev_commands(&cfg, "both", &vars).unwrap();
        assert_eq!(dev_line(&both), "(trap 'kill 0' INT TERM; a & b & wait)");
        let err = dev_commands(&cfg, "api", &vars).unwrap_err();
        assert!(err.to_string().contains("have: both, web"), "{err}");
    }

    struct BusyPorts(Vec<u16>);

    impl crate::connectors::Shell for BusyPorts {
        fn run(&self, _: &Path, _: &[(String, String)], _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn port_owner(&self, port: u16) -> Option<crate::connectors::PortOwner> {
            self.0
                .contains(&port)
                .then(|| crate::connectors::PortOwner {
                    pid: 42,
                    cmdline: "node vite".into(),
                    cwd: String::new(),
                })
        }
    }

    #[test]
    fn check_ports_names_the_owner() {
        let (dir, work) = new_test_repo();
        let mut p = project(&work, &dir.path().join("wt"));
        p.cfg
            .dev
            .insert("web".into(), vec!["vite --port {port.web}".into()]);
        let e = Entry {
            name: "x".into(),
            slot: 2,
            ..Entry::default()
        };
        let vars = p.cfg.vars("x", "main", 2);
        let reg = Registry::default();
        check_ports(&p, &e, "web", &vars, &BusyPorts(vec![]), &reg).unwrap();
        let err = check_ports(&p, &e, "web", &vars, &BusyPorts(vec![20200]), &reg).unwrap_err();
        assert!(
            err.to_string().contains("20200 (web): pid 42 node vite"),
            "{err}"
        );
    }
}
