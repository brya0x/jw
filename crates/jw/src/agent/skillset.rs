//! `skillset`: one catalog of skills, roles and plugins that jw lists (`ls`) and activates (`on`) or
//! removes (`off`); activating installs for real (N184b, REQ-1016 to REQ-1030). It reads files and
//! runs `git` and `claude`, so it works with no daemon.

mod catalog;
mod github;
mod lock;

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::json;

use catalog::{Entry, Kind, Source};
use lock::{Lock, Locked};

use super::skillfile::{self, Plan};
use super::{
    Args, Fail, Out, bin, claude_base, fail, plugins, roles_skill_text, skill_text,
    tasks_skill_text,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Available,
    Installed,
    Outdated,
    Modified,
    OtherTool,
    BuiltIn,
    Unknown,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Available => "available",
            State::Installed => "installed",
            State::Outdated => "outdated",
            State::Modified => "modified",
            State::OtherTool => "other tool",
            State::BuiltIn => "built-in",
            State::Unknown => "unknown",
        }
    }
}

/// Where the skillset works: the Claude configuration, the user's roles, the lock, the person's file,
/// the base of GitHub and the program `claude`. `here` is the process's; a test or the screen's test
/// builds its own, so none of them reads the real home or the network (REQ-1043).
#[derive(Clone, Debug)]
pub(crate) struct Place {
    claude: Option<PathBuf>,
    roles: PathBuf,
    lock: PathBuf,
    file: PathBuf,
    github: Option<String>,
    claude_cmd: String,
}

impl Place {
    pub(crate) fn here() -> Place {
        Place {
            claude: claude_base(
                std::env::var("CLAUDE_CONFIG_DIR").ok(),
                std::env::var("HOME").ok(),
            ),
            roles: identity::user_roles_dir(),
            lock: identity::skillset_lock(),
            file: identity::skillset_file(),
            github: None,
            claude_cmd: plugins::CLAUDE.to_owned(),
        }
    }

    #[cfg(test)]
    /// A place of one's own: a Claude configuration, a roles folder, the lock, the person's file,
    /// the base of GitHub (a folder of repositories, `file` protocol) and a `claude` program.
    pub(crate) fn at(
        claude: PathBuf,
        roles: PathBuf,
        lock: PathBuf,
        file: PathBuf,
        github: Option<String>,
        claude_cmd: String,
    ) -> Place {
        Place {
            claude: Some(claude),
            roles,
            lock,
            file,
            github,
            claude_cmd,
        }
    }
}

/// Where things are, and what the lock says.
struct Ctx {
    claude: Option<PathBuf>,
    roles: PathBuf,
    lock: Lock,
    lock_path: PathBuf,
    file: PathBuf,
    github: Option<String>,
    claude_cmd: String,
}

impl Ctx {
    /// `strict`: a lock that cannot be read is an error (`on`, `off`); else it is a warning and an
    /// empty lock, which only makes things look as if another tool installed them (RISK-173).
    fn load(strict: bool, warnings: &mut Vec<String>) -> Out<Ctx> {
        Ctx::load_at(&Place::here(), strict, warnings)
    }

    fn load_at(place: &Place, strict: bool, warnings: &mut Vec<String>) -> Out<Ctx> {
        let lock = match lock::read(&place.lock) {
            Ok(l) => l,
            Err(e) if strict => return fail(e),
            Err(e) => {
                warnings.push(e);
                Lock::default()
            }
        };
        Ok(Ctx {
            claude: place.claude.clone(),
            roles: place.roles.clone(),
            lock,
            lock_path: place.lock.clone(),
            file: place.file.clone(),
            github: place.github.clone(),
            claude_cmd: place.claude_cmd.clone(),
        })
    }

    fn claude(&self) -> Out<&Path> {
        self.claude
            .as_deref()
            .ok_or_else(|| Fail::Error("no HOME and no CLAUDE_CONFIG_DIR to install into".into()))
    }
}

/// The catalog and the person's entries (REQ-1017).
fn catalog(file: &Path, warnings: &mut Vec<String>) -> Vec<Entry> {
    let mut all = catalog::builtin();
    let mine = catalog::yours(file, &all, warnings);
    all.extend(mine);
    all
}

fn warn(warnings: Vec<String>) {
    for w in warnings {
        eprintln!("warning: {w}");
    }
}

/// The embedded skill of that name: where it goes and its text.
fn embedded_skill(base: &Path, name: &str) -> Option<(PathBuf, String)> {
    let text = if name == bin() {
        skill_text()
    } else if name == format!("{}-tasks", bin()) {
        tasks_skill_text()
    } else if name == super::roles_skill_name() {
        roles_skill_text()
    } else {
        return None;
    };
    Some((base.join("skills").join(name).join("SKILL.md"), text))
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn embedded_state(ctx: &Ctx, name: &str) -> State {
    let Some((path, text)) = ctx.claude.as_deref().and_then(|b| embedded_skill(b, name)) else {
        return State::Unknown;
    };
    if is_link(path.parent().unwrap()) || is_link(&path) {
        return State::OtherTool;
    }
    match skillfile::read(&path) {
        None => State::Available,
        Some(on_disk) => match skillfile::decide(Some(&on_disk), &text) {
            Plan::Unchanged => State::Installed,
            Plan::Write => State::Outdated,
            Plan::Keep => State::Modified,
        },
    }
}

/// Where a `github` entry goes: a skill's folder, or the user's roles folder and the role's file.
struct Dest {
    dir: PathBuf,
    role_file: Option<PathBuf>,
}

impl Dest {
    fn of(ctx: &Ctx, e: &Entry) -> Option<Dest> {
        match e.kind {
            Kind::Skill => Some(Dest {
                dir: ctx.claude.as_ref()?.join("skills").join(&e.name),
                role_file: None,
            }),
            Kind::Role => Some(Dest {
                dir: ctx.roles.clone(),
                role_file: Some(ctx.roles.join(format!("{}.md", e.name))),
            }),
            Kind::Plugin => None,
        }
    }

    /// Anything there, a link (even a broken one) included.
    fn present(&self) -> bool {
        std::fs::symlink_metadata(self.role_file.as_ref().unwrap_or(&self.dir)).is_ok()
    }

    fn is_link(&self) -> bool {
        is_link(self.role_file.as_ref().unwrap_or(&self.dir)) || is_link(&self.dir)
    }

    /// May jw touch this file of its lock entry? (REQ-1025)
    fn owns(&self, file: &Path) -> bool {
        match &self.role_file {
            Some(f) => file == f,
            None => lock::inside(file, &self.dir),
        }
    }

    fn lock_dir(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    /// The paths a fetch writes to.
    fn targets(&self, files: &[(String, Vec<u8>)]) -> Vec<(PathBuf, Vec<u8>)> {
        match &self.role_file {
            Some(f) => files
                .iter()
                .take(1)
                .map(|(_, b)| (f.clone(), b.clone()))
                .collect(),
            None => files
                .iter()
                .map(|(rel, b)| (self.dir.join(rel), b.clone()))
                .collect(),
        }
    }
}

/// The lock entry of `e`, when it is for this destination.
fn locked<'a>(ctx: &'a Ctx, e: &Entry, dest: &Dest) -> Option<&'a Locked> {
    ctx.lock
        .entries
        .get(&e.name)
        .filter(|l| l.dir == dest.lock_dir())
}

fn github_state(ctx: &Ctx, e: &Entry, latest: Option<&str>) -> State {
    let Some(dest) = Dest::of(ctx, e) else {
        return State::Unknown;
    };
    if !dest.present() {
        return State::Available;
    }
    let Some(l) = locked(ctx, e, &dest).filter(|_| !dest.is_link()) else {
        return State::OtherTool;
    };
    let disk = lock::disk_of(l);
    if !disk.edited.is_empty() || !disk.gone.is_empty() {
        State::Modified
    } else if latest.is_some_and(|c| c != l.commit) {
        State::Outdated
    } else {
        State::Installed
    }
}

fn state(ctx: &Ctx, e: &Entry, ids: &Option<Vec<String>>, latest: Option<&str>) -> State {
    match (&e.kind, &e.source) {
        (Kind::Role, Source::Embedded) => State::BuiltIn,
        (Kind::Skill, Source::Embedded) => embedded_state(ctx, &e.name),
        (Kind::Plugin, Source::Plugin { plugin, .. }) => match ids {
            None => State::Unknown,
            Some(v) if v.contains(plugin) => State::Installed,
            Some(_) => State::Available,
        },
        (_, Source::Github { .. }) => github_state(ctx, e, latest),
        _ => State::Unknown,
    }
}

fn source_text(s: &Source) -> String {
    match s {
        Source::Embedded => "embedded".into(),
        Source::Github { repo, .. } => format!("github {repo}"),
        Source::Plugin { plugin, .. } => format!("plugin {plugin}"),
    }
}

fn source_json(s: &Source) -> serde_json::Value {
    match s {
        Source::Embedded => json!({"type": "embedded"}),
        Source::Github { repo, path, branch } => {
            json!({"type": "github", "repo": repo, "path": path, "branch": branch})
        }
        Source::Plugin { plugin, repo } => {
            json!({"type": "plugin", "plugin": plugin, "repo": repo})
        }
    }
}

pub(super) fn run(args: &[&str]) -> Out {
    match args {
        ["ls", rest @ ..] => run_ls(rest),
        ["on", rest @ ..] => run_on(rest),
        ["off", rest @ ..] => run_off(rest),
        _ => Err(Fail::Usage(String::new())),
    }
}

struct Row {
    entry: Entry,
    state: State,
    commit: Option<String>,
    latest: Option<String>,
}

fn table(rows: &[Row]) -> String {
    let w =
        |f: fn(&Row) -> usize, head: &str| rows.iter().map(f).max().unwrap_or(0).max(head.len());
    let (nw, kw, sw, cw) = (
        w(|r| r.entry.name.len(), "NAME"),
        w(|r| r.entry.kind.name().len(), "KIND"),
        w(|r| r.state.name().len(), "STATE"),
        w(
            |r| source_text(&r.entry.source).len() + if r.entry.yours { 8 } else { 0 },
            "SOURCE",
        ),
    );
    let mut out = format!(
        "{:<nw$}  {:<kw$}  {:<sw$}  {:<cw$}  DESCRIPTION\n",
        "NAME", "KIND", "STATE", "SOURCE"
    );
    for r in rows {
        let source = format!(
            "{}{}",
            source_text(&r.entry.source),
            if r.entry.yours { " (yours)" } else { "" }
        );
        let line = format!(
            "{:<nw$}  {:<kw$}  {:<sw$}  {:<cw$}  {}",
            r.entry.name,
            r.entry.kind.name(),
            r.state.name(),
            source,
            r.entry.description
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if rows.iter().any(|r| r.entry.yours) {
        out.push_str(&format!(
            "note: entries marked (yours) come from {}: their sources are text of third parties\n",
            identity::skillset_file().display()
        ));
    }
    out
}

/// The entries with their states, in the catalog's order; `check` asks GitHub for the commit of each
/// installed `github` entry (REQ-1018). What could not be done comes back as warnings.
fn rows(place: &Place, check: bool) -> Out<(Vec<Row>, Vec<String>)> {
    let mut warnings = Vec::new();
    let ctx = Ctx::load_at(place, false, &mut warnings)?;
    let entries = catalog(&ctx.file, &mut warnings);
    let ids = if entries.iter().any(|e| e.kind == Kind::Plugin) {
        plugins::installed_ids(&ctx.claude_cmd).ok()
    } else {
        None
    };
    let mut rows: Vec<Row> = entries
        .into_iter()
        .map(|entry| {
            let st = state(&ctx, &entry, &ids, None);
            Row {
                entry,
                state: st,
                commit: None,
                latest: None,
            }
        })
        .collect();
    for r in &mut rows {
        let Source::Github { repo, branch, .. } = &r.entry.source else {
            continue;
        };
        let Some(l) = Dest::of(&ctx, &r.entry).and_then(|d| locked(&ctx, &r.entry, &d).cloned())
        else {
            continue;
        };
        r.commit = Some(l.commit.clone());
        if check && r.state == State::Installed {
            let (url, from_env) = github::url_of(ctx.github.as_deref(), repo);
            match crate::update::ls_remote(&url, branch.as_deref(), from_env) {
                Ok(c) => {
                    r.state = state(&ctx, &r.entry, &ids, Some(&c));
                    r.latest = Some(c);
                }
                Err(e) => warnings.push(format!("{}: {e}", r.entry.name)),
            }
        }
    }
    Ok((rows, warnings))
}

fn run_ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json", "check"])?;
    a.words::<0>()?;
    let (mut rows, warnings) = rows(&Place::here(), a.has("check"))?;
    rows.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    warn(warnings);
    if a.has("json") {
        let list: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "name": r.entry.name,
                    "kind": r.entry.kind.name(),
                    "description": r.entry.description,
                    "state": r.state.name(),
                    "source": source_json(&r.entry.source),
                    "yours": r.entry.yours,
                    "commit": r.commit,
                    "latest": r.latest,
                })
            })
            .collect();
        println!("{}", json!(list));
    } else {
        print!("{}", table(&rows));
    }
    Ok(())
}

/// `y` or `yes` (REQ-1013).
fn is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The person's yes: `--yes`, or `install? [y/N]` at a terminal; with neither, nothing is installed
/// (REQ-1013, REQ-1022).
pub(super) fn confirm(yes: bool) -> Out<bool> {
    if yes {
        return Ok(true);
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return fail("not installed: no terminal to ask on; run this in a terminal or pass --yes");
    }
    eprint!("install? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Fail::Error(e.to_string()))?;
    Ok(is_yes(&line))
}

fn find(file: &Path, name: &str) -> Out<(Entry, Vec<String>)> {
    let mut warnings = Vec::new();
    let entries = catalog(file, &mut warnings);
    match entries.iter().find(|e| e.name == name) {
        Some(e) => Ok((e.clone(), warnings)),
        None => {
            warn(warnings);
            let there: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
            fail(format!(
                "no entry \"{name}\": there are {}",
                there.join(", ")
            ))
        }
    }
}

fn other_tool(name: &str) -> Fail {
    Fail::Error(format!("{name}: installed by another tool, left alone"))
}

/// An embedded skill, written by the mark's table (REQ-1020); one that is a link is another tool's.
fn on_embedded_skill(ctx: &Ctx, e: &Entry, say: &mut dyn FnMut(String)) -> Out {
    let base = ctx.claude()?;
    let (path, text) =
        embedded_skill(base, &e.name).ok_or_else(|| Fail::Error(format!("{}: no text", e.name)))?;
    if embedded_state(ctx, &e.name) == State::OtherTool {
        return Err(other_tool(&e.name));
    }
    skillfile::install(&path, &text, say)
}

/// `skill install`: the three embedded skills of the built-in catalog and nothing of the person's
/// file (REQ-1027). One that is another tool's does not stop the others.
pub(super) fn install_own_skills() -> Out {
    let ctx = Ctx::load(false, &mut Vec::new())?;
    ctx.claude()?;
    let mut first = None;
    for e in catalog::builtin()
        .iter()
        .filter(|e| e.kind == Kind::Skill && e.source == Source::Embedded)
    {
        if let Err(f) = on_embedded_skill(&ctx, e, &mut |l| println!("{l}")) {
            first.get_or_insert(f);
        }
    }
    first.map_or(Ok(()), Err)
}

/// `skill install --plugins`: the plugins of the built-in catalog and no other (REQ-1021).
pub(super) fn install_own_plugins(yes: bool) -> Out {
    for e in catalog::builtin() {
        if let Source::Plugin { plugin, repo } = &e.source {
            plugins::on(plugin, repo, yes)?;
        }
    }
    Ok(())
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A `github` entry fetched and not written yet: what the sheet shows and `apply` writes (REQ-1022).
pub(crate) struct Prepared {
    entry: Entry,
    url: String,
    branch: String,
    from: Option<String>,
    targets: Vec<(PathBuf, Vec<u8>)>,
    scripts: Vec<String>,
    fetched: github::Fetched,
}

impl Prepared {
    /// The lines of the confirmation, as the CLI prints them (REQ-1022).
    fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "from {}, branch {}, commit {}",
                self.url, self.branch, self.fetched.commit
            ),
            format!("will write {} file(s):", self.targets.len()),
        ];
        for (target, bytes) in &self.targets {
            lines.push(format!("  {} ({} bytes)", target.display(), bytes.len()));
        }
        if !self.scripts.is_empty() && self.entry.kind != Kind::Role {
            lines.push(format!(
                "this folder has files that are not Markdown ({}): a skill can carry scripts",
                self.scripts.join(", ")
            ));
        }
        lines
    }
}

enum Prep {
    /// The commit fetched is the one installed: the line that says it, and nothing to ask.
    Unchanged(String),
    Ask(Box<Prepared>),
}

/// The refusals of `on` for a `github` entry whose destination is not free (REQ-1023).
fn refusal(st: State, name: &str) -> Option<Fail> {
    match st {
        State::OtherTool => Some(other_tool(name)),
        State::Modified => Some(Fail::Error(format!(
            "{name}: what {} wrote there was edited, left alone: move your changes away and run it again",
            bin()
        ))),
        _ => None,
    }
}

/// Everything of `on` for a `github` entry up to the question: the refusals, the fetch (`announce`
/// says what is being fetched, the CLI on stderr), the commit, the files and the warning. `cancel`:
/// the flag of a caller that owns the terminal, in place of signals.
fn prepare_github(
    ctx: &Ctx,
    e: &Entry,
    cancel: Option<Arc<AtomicBool>>,
    announce: &mut dyn FnMut(String),
) -> Out<Prep> {
    let Source::Github { repo, path, branch } = &e.source else {
        unreachable!("a github entry")
    };
    let role = e.kind == Kind::Role;
    ctx.claude()?;
    let dest =
        Dest::of(ctx, e).ok_or_else(|| Fail::Error(format!("{}: nowhere to install", e.name)))?;
    if let Some(f) = refusal(github_state(ctx, e, None), &e.name) {
        return Err(f);
    }
    let (url, from_env) = github::url_of(ctx.github.as_deref(), repo);
    let branch_name = branch.as_deref().unwrap_or("HEAD");
    announce(format!("fetching {url}, branch {branch_name}"));
    let fetched = github::fetch(&url, from_env, branch.as_deref(), path, role, cancel)
        .map_err(Fail::Error)?;
    let old = locked(ctx, e, &dest).map(|l| l.commit.clone());
    if old.as_deref() == Some(fetched.commit.as_str())
        && github_state(ctx, e, None) == State::Installed
    {
        return Ok(Prep::Unchanged(format!(
            "unchanged {} (commit {})",
            e.name,
            short(&fetched.commit)
        )));
    }
    let targets = dest.targets(&fetched.files);
    let scripts = github::not_markdown(&fetched.files)
        .into_iter()
        .map(String::from)
        .collect();
    Ok(Prep::Ask(Box::new(Prepared {
        entry: e.clone(),
        url,
        branch: branch_name.to_owned(),
        from: old,
        targets,
        scripts,
        fetched,
    })))
}

/// The rest of `on` after a yes: the destination is looked at again (the fetch took a while), the
/// files are written, what an earlier install wrote and this one does not have is taken away if it
/// is as it was, and the lock is written last. The line that says it (REQ-1022 to REQ-1024).
fn apply_github(ctx: &mut Ctx, plan: &Prepared) -> Out<String> {
    let e = &plan.entry;
    let Source::Github { repo, path, .. } = &e.source else {
        unreachable!("a github entry")
    };
    let role = e.kind == Kind::Role;
    let dest =
        Dest::of(ctx, e).ok_or_else(|| Fail::Error(format!("{}: nowhere to install", e.name)))?;
    if let Some(f) = refusal(github_state(ctx, e, None), &e.name) {
        return Err(f);
    }
    if plan.fetched.interrupted() {
        return fail("interrupted: nothing installed");
    }
    let targets = &plan.targets;
    github::write_all(targets, (!role).then_some(dest.dir.as_path())).map_err(Fail::Error)?;
    let old = locked(ctx, e, &dest).cloned();
    let new: std::collections::BTreeMap<String, String> = targets
        .iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), lock::hash_bytes(b)))
        .collect();
    if let Some(old) = old {
        let stale: Vec<PathBuf> = old
            .files
            .iter()
            .filter(|(f, h)| {
                let path = Path::new(f.as_str());
                !new.contains_key(*f)
                    && dest.owns(path)
                    && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
                    && std::fs::read(path).is_ok_and(|b| &lock::hash_bytes(&b) == *h)
            })
            .map(|(f, _)| PathBuf::from(f))
            .collect();
        for f in &stale {
            let _ = std::fs::remove_file(f);
        }
        if !role {
            lock::remove_empty_dirs(
                &stale.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
                &dest.dir,
            );
        }
    }
    ctx.lock.entries.insert(
        e.name.clone(),
        Locked {
            kind: e.kind.name().into(),
            source: format!("{repo} {path}"),
            commit: plan.fetched.commit.clone(),
            dir: dest.lock_dir(),
            installed_at: now(),
            files: new,
        },
    );
    lock::write(&ctx.lock_path, &ctx.lock).map_err(|why| {
        for (p, _) in targets {
            let _ = std::fs::remove_file(p);
        }
        Fail::Error(why)
    })?;
    Ok(format!(
        "installed {} (commit {}): {} file(s) in {}",
        e.name,
        short(&plan.fetched.commit),
        targets.len(),
        dest.role_file.as_ref().unwrap_or(&dest.dir).display()
    ))
}

fn on_github(ctx: &mut Ctx, e: &Entry, yes: bool) -> Out {
    let plan = match prepare_github(ctx, e, None, &mut |l| eprintln!("{l}"))? {
        Prep::Unchanged(line) => {
            println!("{line}");
            return Ok(());
        }
        Prep::Ask(plan) => plan,
    };
    for line in plan.lines() {
        println!("{line}");
    }
    // A Ctrl-C is not a yes: the handlers of the fetch keep the process alive, so it is looked at
    // after the answer is read and again before anything is written (REQ-1022).
    let said_yes = confirm(yes)?;
    if plan.fetched.interrupted() {
        return fail("interrupted: nothing installed");
    }
    if !said_yes {
        println!("not installed: you did not say yes");
        return Ok(());
    }
    println!("{}", apply_github(ctx, &plan)?);
    Ok(())
}

fn built_in_line(name: &str) -> String {
    format!(
        "{name} is built in: always available with {} role show {name}",
        bin()
    )
}

fn run_on(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["yes"])?;
    let [name] = a.words::<1>()?;
    let mut ctx = Ctx::load(true, &mut Vec::new())?;
    let (entry, warnings) = find(&ctx.file, name)?;
    warn(warnings);
    match (&entry.kind, &entry.source) {
        (Kind::Skill, Source::Embedded) => {
            on_embedded_skill(&ctx, &entry, &mut |l| println!("{l}"))
        }
        (Kind::Role, Source::Embedded) => {
            println!("{}", built_in_line(name));
            Ok(())
        }
        (Kind::Plugin, Source::Plugin { plugin, repo }) => plugins::on(plugin, repo, a.has("yes")),
        _ => on_github(&mut ctx, &entry, a.has("yes")),
    }
}

fn off_github(ctx: &mut Ctx, e: &Entry, say: &mut dyn FnMut(String)) -> Out {
    let dest =
        Dest::of(ctx, e).ok_or_else(|| Fail::Error(format!("{}: nothing to remove", e.name)))?;
    let Some(l) = locked(ctx, e, &dest).cloned().filter(|_| !dest.is_link()) else {
        if dest.present() {
            return fail(format!(
                "{}: not installed by {}, left alone",
                e.name,
                bin()
            ));
        }
        say(format!("{} is not installed", e.name));
        return Ok(());
    };
    let disk = lock::disk_of(&l);
    let mut kept = std::collections::BTreeMap::new();
    let mut removed = Vec::new();
    for file in &l.files {
        let (name, hash) = file;
        let path = Path::new(name.as_str());
        if !dest.owns(path) {
            say(format!("left {name}: not in {}", dest.dir.display()));
        } else if disk.same.contains(name) {
            let _ = std::fs::remove_file(path);
            removed.push(path.to_path_buf());
        } else if disk.edited.contains(name) {
            say(format!("kept {name}: it was edited"));
            kept.insert(name.clone(), hash.clone());
        }
    }
    if !e.kind.eq(&Kind::Role) {
        lock::remove_empty_dirs(
            &removed.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            &dest.dir,
        );
    }
    if kept.is_empty() {
        ctx.lock.entries.remove(&e.name);
    } else if let Some(entry) = ctx.lock.entries.get_mut(&e.name) {
        entry.files = kept;
    }
    lock::write(&ctx.lock_path, &ctx.lock).map_err(Fail::Error)?;
    say(format!("removed {} file(s) of {}", removed.len(), e.name));
    Ok(())
}

/// `off` of one entry, its lines said as they come.
fn off_entry(ctx: &mut Ctx, entry: &Entry, say: &mut dyn FnMut(String)) -> Out {
    let name = entry.name.as_str();
    match (&entry.kind, &entry.source) {
        (Kind::Role, Source::Embedded) => {
            say(format!(
                "{name} is built in: it is always available, there is nothing to turn off"
            ));
            Ok(())
        }
        (Kind::Plugin, Source::Plugin { plugin, .. }) => {
            say(format!(
                "{bin} does not uninstall plugins; to remove it run: claude plugin uninstall {plugin}",
                bin = bin()
            ));
            Ok(())
        }
        (Kind::Skill, Source::Embedded) => {
            let base = ctx.claude()?;
            let (path, text) = embedded_skill(base, name)
                .ok_or_else(|| Fail::Error(format!("{name}: no text")))?;
            if is_link(&path) || is_link(path.parent().unwrap()) {
                return Err(other_tool(name));
            }
            let on_disk = skillfile::read(&path);
            match skillfile::decide(on_disk.as_deref(), &text) {
                _ if on_disk.is_none() => say(format!("{name} is not installed")),
                Plan::Keep => say(format!(
                    "kept {}: not written by {} or edited",
                    path.display(),
                    bin()
                )),
                Plan::Unchanged | Plan::Write => {
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_dir(path.parent().unwrap());
                    say(format!("removed {}", path.display()));
                }
            }
            Ok(())
        }
        _ => off_github(ctx, entry, say),
    }
}

fn run_off(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [name] = a.words::<1>()?;
    let mut ctx = Ctx::load(true, &mut Vec::new())?;
    let (entry, warnings) = find(&ctx.file, name)?;
    warn(warnings);
    off_entry(&mut ctx, &entry, &mut |l| println!("{l}"))
}

/// What the screen asks and the CLI does not: the same steps as data, for a client that draws and
/// asks in its own way (N185). What fails is the text of the failure.
fn text(f: Fail) -> String {
    match f {
        Fail::Usage(m) | Fail::Error(m) | Fail::Idle(m) | Fail::Waiting(m) | Fail::Changes(m) => m,
    }
}

/// One row of the list: the catalog's entry and its state, with the CLI's words (REQ-1032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub name: String,
    pub kind: &'static str,
    pub state: &'static str,
    pub source: String,
    pub description: String,
    pub yours: bool,
    pub commit: Option<String>,
}

/// The entries in the catalog's order with their states; `check` is `ls --check`. The second part is
/// what could not be read or asked (REQ-1031, REQ-1041).
pub(crate) fn items(place: &Place, check: bool) -> Result<(Vec<Item>, Vec<String>), String> {
    let (rows, warnings) = rows(place, check).map_err(text)?;
    let items = rows
        .into_iter()
        .map(|r| Item {
            source: format!(
                "{}{}",
                source_text(&r.entry.source),
                if r.entry.yours { " (yours)" } else { "" }
            ),
            name: r.entry.name,
            kind: r.entry.kind.name(),
            state: r.state.name(),
            description: r.entry.description,
            yours: r.entry.yours,
            commit: r.commit,
        })
        .collect();
    Ok((items, warnings))
}

/// What is to be asked before something third-party is written (REQ-1037): it holds what was fetched
/// (and its temporary directory, deleted when it drops) or the `claude` commands.
pub(crate) enum Ask {
    Github(Box<Prepared>),
    Plugin(plugins::Offer),
}

/// What the sheet draws of an `Ask`, as plain data (REQ-1036, REQ-1038).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sheet {
    Github {
        name: String,
        url: String,
        branch: String,
        commit: String,
        /// The commit installed now, when this is an update.
        from: Option<String>,
        files: Vec<(String, usize)>,
        scripts: Vec<String>,
    },
    Plugin {
        plugin: String,
        repo: String,
        marketplace: String,
        commands: Vec<String>,
    },
}

impl Ask {
    pub(crate) fn sheet(&self) -> Sheet {
        match self {
            Ask::Github(p) => Sheet::Github {
                name: p.entry.name.clone(),
                url: p.url.clone(),
                branch: p.branch.clone(),
                commit: p.fetched.commit.clone(),
                from: p.from.clone(),
                files: p
                    .targets
                    .iter()
                    .map(|(f, b)| (f.display().to_string(), b.len()))
                    .collect(),
                scripts: if p.entry.kind == Kind::Role {
                    Vec::new()
                } else {
                    p.scripts.clone()
                },
            },
            Ask::Plugin(o) => Sheet::Plugin {
                plugin: o.plugin.clone(),
                repo: o.repo.clone(),
                marketplace: o.marketplace.clone(),
                commands: o.todo.iter().map(|c| c.join(" ")).collect(),
            },
        }
    }
}

/// What `on` did or asks.
pub(crate) enum Step {
    /// Done, with the lines the CLI would print.
    Done(Vec<String>),
    Ask(Ask),
}

/// `skillset on <name>` up to the question: the entry is installed at once when it is jw's own, else
/// what is to be asked comes back, fetched when it is a `github` one (`cancel` ends the fetch).
pub(crate) fn on(place: &Place, name: &str, cancel: Arc<AtomicBool>) -> Result<Step, String> {
    let ctx = Ctx::load_at(place, true, &mut Vec::new()).map_err(text)?;
    let (entry, _) = find(&ctx.file, name).map_err(text)?;
    let step = match (&entry.kind, &entry.source) {
        (Kind::Skill, Source::Embedded) => {
            let mut lines = Vec::new();
            on_embedded_skill(&ctx, &entry, &mut |l| lines.push(l)).map_err(text)?;
            Step::Done(lines)
        }
        (Kind::Role, Source::Embedded) => Step::Done(vec![built_in_line(name)]),
        (Kind::Plugin, Source::Plugin { plugin, repo }) => {
            match plugins::prepare(&ctx.claude_cmd, plugin, repo).map_err(text)? {
                None => Step::Done(vec![format!("plugins: {plugin} is installed")]),
                Some(offer) => Step::Ask(Ask::Plugin(offer)),
            }
        }
        _ => match prepare_github(&ctx, &entry, Some(cancel), &mut |_| {}).map_err(text)? {
            Prep::Unchanged(line) => Step::Done(vec![line]),
            Prep::Ask(p) => Step::Ask(Ask::Github(p)),
        },
    };
    Ok(step)
}

/// The rest of `on` after the person's yes (REQ-1036, REQ-1038): the lines the CLI would print. The
/// commands of a plugin run with their output captured.
pub(crate) fn apply(place: &Place, ask: Ask) -> Result<Vec<String>, String> {
    match ask {
        Ask::Github(p) => {
            let mut ctx = Ctx::load_at(place, true, &mut Vec::new()).map_err(text)?;
            Ok(vec![apply_github(&mut ctx, &p).map_err(text)?])
        }
        Ask::Plugin(o) => {
            let mut lines = Vec::new();
            plugins::run(&place.claude_cmd, &o, true, &mut |l| lines.push(l)).map_err(text)?;
            lines.push(format!("plugins: {} is installed", o.plugin));
            Ok(lines)
        }
    }
}

/// `skillset off <name>`: the lines the CLI would print (REQ-1039).
pub(crate) fn off(place: &Place, name: &str) -> Result<Vec<String>, String> {
    let mut ctx = Ctx::load_at(place, true, &mut Vec::new()).map_err(text)?;
    let (entry, _) = find(&ctx.file, name).map_err(text)?;
    let mut lines = Vec::new();
    off_entry(&mut ctx, &entry, &mut |l| lines.push(l)).map_err(text)?;
    Ok(lines)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn only_yes_goes_on() {
        for yes in ["y", "Y\n", " yes ", "YES"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "n", "\n", "ye", "yep", "sure"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    fn ctx(dir: &Path) -> Ctx {
        Ctx {
            claude: Some(dir.join("claude")),
            roles: dir.join("roles"),
            lock: Lock::default(),
            lock_path: dir.join("lock"),
            file: dir.join("skillset.toml"),
            github: None,
            claude_cmd: "claude".into(),
        }
    }

    fn skill(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::Skill,
            description: String::new(),
            source: Source::Github {
                repo: "o/r".into(),
                path: "x/SKILL.md".into(),
                branch: None,
            },
            yours: false,
        }
    }

    #[test]
    fn a_thing_there_that_the_lock_does_not_have_is_another_tools() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let e = skill("caveman");
        assert_eq!(github_state(&ctx, &e, None), State::Available);
        // A link (the `skills` tool's), even one that leads nowhere.
        std::fs::create_dir_all(dir.path().join("claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/caveman",
            dir.path().join("claude/skills/caveman"),
        )
        .unwrap();
        assert_eq!(github_state(&ctx, &e, None), State::OtherTool);
        // A folder of the person's.
        std::fs::remove_file(dir.path().join("claude/skills/caveman")).unwrap();
        std::fs::create_dir_all(dir.path().join("claude/skills/caveman")).unwrap();
        assert_eq!(github_state(&ctx, &e, None), State::OtherTool);
    }

    #[test]
    fn the_lock_makes_it_installed_modified_or_outdated_and_only_for_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        let e = skill("caveman");
        let skill_dir = dir.path().join("claude/skills/caveman");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let file = skill_dir.join("SKILL.md");
        std::fs::write(&file, "text").unwrap();
        ctx.lock.entries.insert(
            "caveman".into(),
            Locked {
                dir: skill_dir.to_string_lossy().into_owned(),
                commit: "abc".into(),
                files: [(
                    file.to_string_lossy().into_owned(),
                    lock::hash_bytes(b"text"),
                )]
                .into(),
                ..Locked::default()
            },
        );
        assert_eq!(github_state(&ctx, &e, None), State::Installed);
        assert_eq!(github_state(&ctx, &e, Some("abc")), State::Installed);
        assert_eq!(github_state(&ctx, &e, Some("def")), State::Outdated);
        std::fs::write(&file, "edited").unwrap();
        assert_eq!(github_state(&ctx, &e, None), State::Modified);
        std::fs::write(&file, "text").unwrap();
        std::fs::remove_file(&file).unwrap();
        assert_eq!(github_state(&ctx, &e, None), State::Modified);
        // Another Claude configuration: the lock is not about this folder.
        ctx.claude = Some(dir.path().join("elsewhere"));
        std::fs::create_dir_all(dir.path().join("elsewhere/skills/caveman")).unwrap();
        assert_eq!(github_state(&ctx, &e, None), State::OtherTool);
    }

    #[test]
    fn the_embedded_skills_are_available_installed_outdated_modified_or_a_links() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let name = bin();
        assert_eq!(embedded_state(&ctx, name), State::Available);
        let (path, text) = embedded_skill(ctx.claude.as_deref().unwrap(), name).unwrap();
        skillfile::install(&path, &text, &mut |_| {}).unwrap();
        assert_eq!(embedded_state(&ctx, name), State::Installed);
        std::fs::write(&path, "mine\n").unwrap();
        assert_eq!(embedded_state(&ctx, name), State::Modified);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(dir.path(), path.parent().unwrap()).unwrap();
        assert_eq!(embedded_state(&ctx, name), State::OtherTool);
        assert_eq!(embedded_state(&ctx, "not-a-skill-of-jw"), State::Unknown);
    }

    #[test]
    fn what_a_lock_names_outside_the_folder_is_not_ours_to_touch() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let e = skill("caveman");
        let dest = Dest::of(&ctx, &e).unwrap();
        assert!(dest.owns(&dest.dir.join("SKILL.md")));
        assert!(!dest.owns(&dest.dir));
        assert!(!dest.owns(&dest.dir.join("../other/SKILL.md")));
        assert!(!dest.owns(Path::new("/etc/passwd")));
        let role = Entry {
            kind: Kind::Role,
            ..skill("voice")
        };
        let dest = Dest::of(&ctx, &role).unwrap();
        assert!(dest.owns(&dir.path().join("roles/voice.md")));
        assert!(!dest.owns(&dir.path().join("roles/other.md")));
    }

    // ---- the library as the screen uses it (N185a): a place of its own, a repository that is a
    // folder, a `claude` that is a script; no network, no home.

    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::Ordering;

    pub(crate) struct World {
        pub(crate) dir: tempfile::TempDir,
        pub(crate) place: Place,
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    impl World {
        pub(crate) fn new() -> World {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            std::fs::create_dir_all(root.join("bin")).unwrap();
            std::fs::write(root.join("list.json"), "[]").unwrap();
            // A `claude` that records every call and answers the two lists from files.
            let claude = root.join("bin/claude");
            std::fs::write(
                &claude,
                format!(
                    "#!/bin/sh\necho \"$@\" >> '{log}'\ncase \"$*\" in\n  \"plugin list --json\") cat '{list}';;\n  \"plugin marketplace list --json\") echo '[]';;\n  \"plugin install\"*) [ -f '{fail}' ] && echo 'nope' >&2 && exit 1;;\nesac\nexit 0\n",
                    log = root.join("claude.log").display(),
                    list = root.join("list.json").display(),
                    fail = root.join("install.fails").display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
            let repo = root.join("gh/juliusbrussee/caveman");
            std::fs::create_dir_all(repo.join("skills/caveman")).unwrap();
            std::fs::write(
                repo.join("skills/caveman/SKILL.md"),
                "---\nname: caveman\n---\none\n",
            )
            .unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "-q", "-m", "one"]);
            let place = Place::at(
                root.join("claude"),
                root.join("roles"),
                root.join("lock.json"),
                root.join("skillset.toml"),
                Some(format!("file://{}", root.join("gh").display())),
                claude.display().to_string(),
            );
            World { dir, place }
        }

        fn repo(&self) -> PathBuf {
            self.dir.path().join("gh/juliusbrussee/caveman")
        }

        pub(crate) fn skill_file(&self) -> PathBuf {
            self.dir.path().join("claude/skills/caveman/SKILL.md")
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("claude.log")).unwrap_or_default()
        }

        fn state(&self, name: &str, check: bool) -> &'static str {
            let (items, _) = items(&self.place, check).unwrap();
            items.iter().find(|i| i.name == name).unwrap().state
        }

        fn cancel() -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }

        fn ask(&self, name: &str) -> Ask {
            match on(&self.place, name, World::cancel()) {
                Ok(Step::Ask(a)) => a,
                Ok(Step::Done(l)) => panic!("done, not asked: {l:?}"),
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn the_items_are_the_catalogs_entries_in_its_order_with_their_states() {
        let w = World::new();
        let (items, warnings) = items(&w.place, false).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                bin(),
                &format!("{}-tasks", bin()),
                "agent",
                "brain",
                "researcher",
                "caveman",
                "ponytail"
            ]
        );
        let states: Vec<_> = items.iter().map(|i| i.state).collect();
        assert_eq!(
            states,
            [
                "available",
                "available",
                "available",
                "built-in",
                "built-in",
                "available",
                "available"
            ]
        );
        let caveman = &items[5];
        assert_eq!(
            (caveman.kind, caveman.source.as_str()),
            ("skill", "github juliusbrussee/caveman")
        );
        assert!(!caveman.yours);
    }

    #[test]
    fn an_embedded_skill_goes_on_at_once_and_off_with_the_lines_of_the_cli() {
        let w = World::new();
        let Ok(Step::Done(lines)) = on(&w.place, bin(), World::cancel()) else {
            panic!("an embedded skill is installed with no question")
        };
        let file = w
            .dir
            .path()
            .join("claude/skills")
            .join(bin())
            .join("SKILL.md");
        assert_eq!(lines, [format!("installed {}", file.display())]);
        assert_eq!(w.state(bin(), false), "installed");
        let Ok(Step::Done(again)) = on(&w.place, bin(), World::cancel()) else {
            panic!()
        };
        assert_eq!(again, [format!("unchanged {}", file.display())]);
        assert_eq!(
            off(&w.place, bin()).unwrap(),
            [format!("removed {}", file.display())]
        );
        assert_eq!(w.state(bin(), false), "available");
        // A role is built in: nothing to do either way.
        let Ok(Step::Done(role)) = on(&w.place, "brain", World::cancel()) else {
            panic!()
        };
        assert!(role[0].starts_with("brain is built in"), "{role:?}");
        assert!(off(&w.place, "brain").unwrap()[0].contains("nothing to turn off"));
    }

    #[test]
    fn a_github_entry_is_fetched_and_asked_about_and_only_a_yes_writes_it() {
        let w = World::new();
        let ask = w.ask("caveman");
        // The fetch is done and nothing is written: this is what the sheet shows.
        assert!(!w.skill_file().exists());
        let Sheet::Github {
            name,
            url,
            branch,
            commit,
            from,
            files,
            scripts,
        } = ask.sheet()
        else {
            panic!("a sheet of a repository")
        };
        assert_eq!(name, "caveman");
        assert!(url.ends_with("/gh/juliusbrussee/caveman"), "{url}");
        assert_eq!(branch, "HEAD");
        assert_eq!(commit, git(&w.repo(), &["rev-parse", "HEAD"]));
        assert_eq!(from, None);
        assert_eq!(files, [(w.skill_file().display().to_string(), 26)]);
        assert!(scripts.is_empty());
        let lines = apply(&w.place, ask).unwrap();
        assert_eq!(
            lines,
            [format!(
                "installed caveman (commit {}): 1 file(s) in {}",
                short(&commit),
                w.skill_file().parent().unwrap().display()
            )]
        );
        assert!(w.skill_file().exists());
        assert_eq!(w.state("caveman", false), "installed");
        // The same commit again is not asked about.
        let Ok(Step::Done(same)) = on(&w.place, "caveman", World::cancel()) else {
            panic!("the commit installed is not asked about")
        };
        assert_eq!(
            same,
            [format!("unchanged caveman (commit {})", short(&commit))]
        );
    }

    #[test]
    fn dropping_what_was_fetched_installs_nothing() {
        let w = World::new();
        drop(w.ask("caveman"));
        assert!(!w.skill_file().exists());
        assert!(!w.dir.path().join("lock.json").exists());
        assert_eq!(w.state("caveman", false), "available");
    }

    #[test]
    fn a_file_that_is_not_markdown_is_on_the_sheet_and_a_newer_commit_names_the_one_installed() {
        let w = World::new();
        let first = {
            let ask = w.ask("caveman");
            let Sheet::Github { commit, .. } = ask.sheet() else {
                panic!()
            };
            apply(&w.place, ask).unwrap();
            commit
        };
        std::fs::create_dir_all(w.repo().join("skills/caveman/scripts")).unwrap();
        std::fs::write(
            w.repo().join("skills/caveman/scripts/stats.py"),
            "print(1)\n",
        )
        .unwrap();
        git(&w.repo(), &["add", "."]);
        git(&w.repo(), &["commit", "-q", "-m", "two"]);
        // Only the check knows of the newer commit.
        assert_eq!(w.state("caveman", false), "installed");
        assert_eq!(w.state("caveman", true), "outdated");
        let ask = w.ask("caveman");
        let Sheet::Github {
            from,
            files,
            scripts,
            commit,
            ..
        } = ask.sheet()
        else {
            panic!()
        };
        assert_eq!(from.as_deref(), Some(first.as_str()));
        assert_ne!(commit, first);
        assert_eq!(files.len(), 2);
        assert_eq!(scripts, ["scripts/stats.py"]);
        apply(&w.place, ask).unwrap();
        assert_eq!(w.state("caveman", true), "installed");
    }

    #[test]
    fn what_another_tool_put_there_and_what_the_person_edited_are_refused_with_the_cli_lines() {
        let w = World::new();
        let link = w.dir.path().join("claude/skills/caveman");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(w.dir.path().join("elsewhere"), &link).unwrap();
        assert_eq!(w.state("caveman", false), "other tool");
        assert_eq!(
            on(&w.place, "caveman", World::cancel()).err().unwrap(),
            "caveman: installed by another tool, left alone"
        );
        std::fs::remove_file(&link).unwrap();
        apply(&w.place, w.ask("caveman")).unwrap();
        std::fs::write(w.skill_file(), "mine\n").unwrap();
        assert_eq!(w.state("caveman", false), "modified");
        let refused = on(&w.place, "caveman", World::cancel()).err().unwrap();
        assert!(refused.contains("was edited, left alone"), "{refused}");
        // `off` takes away only what was not edited and says so.
        let lines = off(&w.place, "caveman").unwrap();
        assert!(
            lines[0].starts_with("kept ") && lines[0].ends_with("it was edited"),
            "{lines:?}"
        );
        assert_eq!(lines.last().unwrap(), "removed 0 file(s) of caveman");
        assert_eq!(std::fs::read_to_string(w.skill_file()).unwrap(), "mine\n");
    }

    #[test]
    fn a_flag_that_is_up_ends_the_fetch_and_nothing_is_written() {
        let w = World::new();
        let cancel = World::cancel();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            on(&w.place, "caveman", cancel).err().unwrap(),
            "interrupted"
        );
        assert!(!w.skill_file().exists());
        assert!(!w.dir.path().join("lock.json").exists());
    }

    #[test]
    fn a_repository_that_does_not_exist_says_why_and_a_link_in_it_is_refused() {
        let w = World::new();
        std::fs::remove_dir_all(w.repo().join(".git")).unwrap();
        let e = on(&w.place, "caveman", World::cancel()).err().unwrap();
        assert!(e.contains("git") && e.contains("failed"), "{e}");
        assert!(!e.contains('\n'), "one line");
        let w = World::new();
        std::os::unix::fs::symlink("/etc/hosts", w.repo().join("skills/caveman/hosts")).unwrap();
        git(&w.repo(), &["add", "."]);
        git(&w.repo(), &["commit", "-q", "-m", "link"]);
        let e = on(&w.place, "caveman", World::cancel()).err().unwrap();
        assert!(e.ends_with("a link, refused"), "{e}");
    }

    #[test]
    fn a_plugin_is_asked_about_with_its_commands_and_run_only_after_a_yes() {
        let w = World::new();
        let ask = w.ask("ponytail");
        let Sheet::Plugin {
            plugin,
            repo,
            marketplace,
            commands,
        } = ask.sheet()
        else {
            panic!("a sheet of a plugin")
        };
        assert_eq!(
            (plugin.as_str(), repo.as_str(), marketplace.as_str()),
            ("ponytail@ponytail", "DietrichGebert/ponytail", "ponytail")
        );
        assert_eq!(
            commands,
            [
                "plugin marketplace add DietrichGebert/ponytail",
                "plugin install ponytail@ponytail"
            ]
        );
        // Asking called `claude` only for the two lists.
        assert!(!w.log().contains("install"), "{}", w.log());
        let lines = apply(&w.place, ask).unwrap();
        assert_eq!(
            lines.last().unwrap(),
            "plugins: ponytail@ponytail is installed"
        );
        assert!(
            w.log().contains("plugin install ponytail@ponytail"),
            "{}",
            w.log()
        );
        // Installed: not asked again, and `off` says the command and uninstalls nothing.
        std::fs::write(
            w.dir.path().join("list.json"),
            r#"[{"id":"ponytail@ponytail"}]"#,
        )
        .unwrap();
        assert_eq!(w.state("ponytail", false), "installed");
        let Ok(Step::Done(done)) = on(&w.place, "ponytail", World::cancel()) else {
            panic!()
        };
        assert_eq!(done, ["plugins: ponytail@ponytail is installed"]);
        assert_eq!(
            off(&w.place, "ponytail").unwrap(),
            [format!(
                "{} does not uninstall plugins; to remove it run: claude plugin uninstall ponytail@ponytail",
                bin()
            )]
        );
        assert!(!w.log().contains("uninstall"));
    }

    #[test]
    fn a_plugin_command_that_fails_is_shown_with_what_claude_said() {
        let w = World::new();
        std::fs::write(w.dir.path().join("install.fails"), "").unwrap();
        let e = apply(&w.place, w.ask("ponytail")).err().unwrap();
        assert!(
            e.contains("plugin install ponytail@ponytail failed") && e.ends_with(": nope"),
            "{e}"
        );
    }

    #[test]
    fn a_claude_that_does_not_answer_makes_a_plugin_unknown() {
        let w = World::new();
        let place = Place::at(
            w.dir.path().join("claude"),
            w.dir.path().join("roles"),
            w.dir.path().join("lock.json"),
            w.dir.path().join("skillset.toml"),
            None,
            w.dir.path().join("no-such-claude").display().to_string(),
        );
        let (items, _) = items(&place, false).unwrap();
        assert_eq!(
            items.iter().find(|i| i.name == "ponytail").unwrap().state,
            "unknown"
        );
    }
}
