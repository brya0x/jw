//! `skillset`: one catalog of skills, roles and plugins that jw lists (`ls`) and activates (`on`) or
//! removes (`off`); activating installs for real (N184b, REQ-1008 to REQ-1022). It reads files and
//! runs `git` and `claude`, so it works with no daemon.

mod catalog;
mod github;
mod lock;

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

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

/// Where things are: the Claude configuration, the user's roles, and what the lock says.
struct Ctx {
    claude: Option<PathBuf>,
    roles: PathBuf,
    lock: Lock,
    lock_path: PathBuf,
}

impl Ctx {
    /// `strict`: a lock that cannot be read is an error (`on`, `off`); else it is a warning and an
    /// empty lock, which only makes things look as if another tool installed them (RISK-171).
    fn load(strict: bool, warnings: &mut Vec<String>) -> Out<Ctx> {
        let lock_path = identity::skillset_lock();
        let lock = match lock::read(&lock_path) {
            Ok(l) => l,
            Err(e) if strict => return fail(e),
            Err(e) => {
                warnings.push(e);
                Lock::default()
            }
        };
        Ok(Ctx {
            claude: claude_base(
                std::env::var("CLAUDE_CONFIG_DIR").ok(),
                std::env::var("HOME").ok(),
            ),
            roles: identity::user_roles_dir(),
            lock,
            lock_path,
        })
    }

    fn claude(&self) -> Out<&Path> {
        self.claude
            .as_deref()
            .ok_or_else(|| Fail::Error("no HOME and no CLAUDE_CONFIG_DIR to install into".into()))
    }
}

/// The catalog and the person's entries (REQ-1009).
fn catalog(warnings: &mut Vec<String>) -> Vec<Entry> {
    let mut all = catalog::builtin();
    let mine = catalog::yours(&identity::skillset_file(), &all, warnings);
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

    /// May jw touch this file of its lock entry? (REQ-1017)
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
        ["ls", rest @ ..] => ls(rest),
        ["on", rest @ ..] => on(rest),
        ["off", rest @ ..] => off(rest),
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

fn ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json", "check"])?;
    a.words::<0>()?;
    let mut warnings = Vec::new();
    let ctx = Ctx::load(false, &mut warnings)?;
    let entries = catalog(&mut warnings);
    let ids = if entries.iter().any(|e| e.kind == Kind::Plugin) {
        plugins::installed_ids().ok()
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
        if a.has("check") && r.state == State::Installed {
            let (url, from_env) = github::url_of(repo);
            match crate::update::ls_remote(&url, branch.as_deref(), from_env) {
                Ok(c) => {
                    r.state = state(&ctx, &r.entry, &ids, Some(&c));
                    r.latest = Some(c);
                }
                Err(e) => warnings.push(format!("{}: {e}", r.entry.name)),
            }
        }
    }
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

/// `y` or `yes` (REQ-1005).
fn is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The person's yes: `--yes`, or `install? [y/N]` at a terminal; with neither, nothing is installed
/// (REQ-1005, REQ-1014).
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

fn find(name: &str) -> Out<(Entry, Vec<String>)> {
    let mut warnings = Vec::new();
    let entries = catalog(&mut warnings);
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

/// An embedded skill, written by the mark's table (REQ-1012); one that is a link is another tool's.
fn on_embedded_skill(ctx: &Ctx, e: &Entry) -> Out {
    let base = ctx.claude()?;
    let (path, text) =
        embedded_skill(base, &e.name).ok_or_else(|| Fail::Error(format!("{}: no text", e.name)))?;
    if embedded_state(ctx, &e.name) == State::OtherTool {
        return Err(other_tool(&e.name));
    }
    skillfile::install(&path, &text)
}

/// `skill install`: the three embedded skills of the built-in catalog and nothing of the person's
/// file (REQ-1019). One that is another tool's does not stop the others.
pub(super) fn install_own_skills() -> Out {
    let ctx = Ctx::load(false, &mut Vec::new())?;
    ctx.claude()?;
    let mut first = None;
    for e in catalog::builtin()
        .iter()
        .filter(|e| e.kind == Kind::Skill && e.source == Source::Embedded)
    {
        if let Err(f) = on_embedded_skill(&ctx, e) {
            first.get_or_insert(f);
        }
    }
    first.map_or(Ok(()), Err)
}

/// `skill install --plugins`: the plugins of the built-in catalog and no other (REQ-1013).
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

fn on_github(ctx: &mut Ctx, e: &Entry, yes: bool) -> Out {
    let Source::Github { repo, path, branch } = &e.source else {
        unreachable!("a github entry")
    };
    let role = e.kind == Kind::Role;
    ctx.claude()?;
    let dest =
        Dest::of(ctx, e).ok_or_else(|| Fail::Error(format!("{}: nowhere to install", e.name)))?;
    let refuse = |st: State, name: &str| match st {
        State::OtherTool => Some(other_tool(name)),
        State::Modified => Some(Fail::Error(format!(
            "{name}: what {} wrote there was edited, left alone: move your changes away and run it again",
            bin()
        ))),
        _ => None,
    };
    if let Some(f) = refuse(github_state(ctx, e, None), &e.name) {
        return Err(f);
    }
    let (url, from_env) = github::url_of(repo);
    let branch_name = branch.as_deref().unwrap_or("HEAD");
    eprintln!("fetching {url}, branch {branch_name}");
    let fetched =
        github::fetch(&url, from_env, branch.as_deref(), path, role).map_err(Fail::Error)?;
    if let Some(l) = locked(ctx, e, &dest)
        && l.commit == fetched.commit
        && github_state(ctx, e, None) == State::Installed
    {
        println!("unchanged {} (commit {})", e.name, short(&fetched.commit));
        return Ok(());
    }
    let targets = dest.targets(&fetched.files);
    println!(
        "from {url}, branch {branch_name}, commit {}",
        fetched.commit
    );
    println!("will write {} file(s):", targets.len());
    for (target, bytes) in &targets {
        println!("  {} ({} bytes)", target.display(), bytes.len());
    }
    let scripts = github::not_markdown(&fetched.files);
    if !scripts.is_empty() && !role {
        println!(
            "this folder has files that are not Markdown ({}): a skill can carry scripts",
            scripts.join(", ")
        );
    }
    if !confirm(yes)? {
        println!("not installed: you did not say yes");
        return Ok(());
    }
    // The destination is looked at again right before writing: the fetch took a while.
    if let Some(f) = refuse(github_state(ctx, e, None), &e.name) {
        return Err(f);
    }
    github::write_all(&targets, (!role).then_some(dest.dir.as_path())).map_err(Fail::Error)?;
    let old = locked(ctx, e, &dest).cloned();
    let new: std::collections::BTreeMap<String, String> = targets
        .iter()
        .map(|(p, b)| (p.to_string_lossy().into_owned(), lock::hash_bytes(b)))
        .collect();
    // What an earlier install wrote and this one does not have is taken away if it is as it was.
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
            commit: fetched.commit.clone(),
            dir: dest.lock_dir(),
            installed_at: now(),
            files: new,
        },
    );
    lock::write(&ctx.lock_path, &ctx.lock).map_err(|why| {
        for (p, _) in &targets {
            let _ = std::fs::remove_file(p);
        }
        Fail::Error(why)
    })?;
    println!(
        "installed {} (commit {}): {} file(s) in {}",
        e.name,
        short(&fetched.commit),
        targets.len(),
        dest.role_file.as_ref().unwrap_or(&dest.dir).display()
    );
    Ok(())
}

fn on(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["yes"])?;
    let [name] = a.words::<1>()?;
    let (entry, warnings) = find(name)?;
    warn(warnings);
    let mut ctx = Ctx::load(true, &mut Vec::new())?;
    match (&entry.kind, &entry.source) {
        (Kind::Skill, Source::Embedded) => on_embedded_skill(&ctx, &entry),
        (Kind::Role, Source::Embedded) => {
            println!(
                "{name} is built in: always available with {} role show {name}",
                bin()
            );
            Ok(())
        }
        (Kind::Plugin, Source::Plugin { plugin, repo }) => plugins::on(plugin, repo, a.has("yes")),
        _ => on_github(&mut ctx, &entry, a.has("yes")),
    }
}

fn off_github(ctx: &mut Ctx, e: &Entry) -> Out {
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
        println!("{} is not installed", e.name);
        return Ok(());
    };
    let disk = lock::disk_of(&l);
    let mut kept = std::collections::BTreeMap::new();
    let mut removed = Vec::new();
    for file in &l.files {
        let (name, hash) = file;
        let path = Path::new(name.as_str());
        if !dest.owns(path) {
            println!("left {name}: not in {}", dest.dir.display());
        } else if disk.same.contains(name) {
            let _ = std::fs::remove_file(path);
            removed.push(path.to_path_buf());
        } else if disk.edited.contains(name) {
            println!("kept {name}: it was edited");
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
    println!("removed {} file(s) of {}", removed.len(), e.name);
    Ok(())
}

fn off(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [name] = a.words::<1>()?;
    let (entry, warnings) = find(name)?;
    warn(warnings);
    let mut ctx = Ctx::load(true, &mut Vec::new())?;
    match (&entry.kind, &entry.source) {
        (Kind::Role, Source::Embedded) => {
            println!("{name} is built in: it is always available, there is nothing to turn off");
            Ok(())
        }
        (Kind::Plugin, Source::Plugin { plugin, .. }) => {
            println!(
                "{bin} does not uninstall plugins; to remove it run: claude plugin uninstall {plugin}",
                bin = bin()
            );
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
                _ if on_disk.is_none() => println!("{name} is not installed"),
                Plan::Keep => println!(
                    "kept {}: not written by {} or edited",
                    path.display(),
                    bin()
                ),
                Plan::Unchanged | Plan::Write => {
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_dir(path.parent().unwrap());
                    println!("removed {}", path.display());
                }
            }
            Ok(())
        }
        _ => off_github(&mut ctx, &entry),
    }
}

#[cfg(test)]
mod tests {
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
        skillfile::install(&path, &text).unwrap();
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
}
