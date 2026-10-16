//! The settings of a repository: its `.jw.toml`, the file the old jw wrote and jwn reads as it
//! is, or the person's own file of the same syntax outside the repository (spec REQ-330 to
//! REQ-334, REQ-482 to REQ-487, RAT-34, RAT-46). It is read afresh each time it is needed; a key
//! jwn does not use is ignored, so an old file keeps `ports` and `env` for the part that reads them.

use std::path::{Component, Path, PathBuf};

use contract::{ConfigFrom, SplitDir};
use serde::Deserialize;
use toml::{Spanned, Table, Value};

use crate::layout::{MAX_RATIO, MIN_RATIO};
use crate::procs::title_for;

const FILE: &str = ".jw.toml";
const MAX_PANES: usize = 8;
/// What the layout keeps of a worktree's height when a setup pane goes under it, in thousandths.
pub const SETUP_SHARE: u16 = 680;

/// What a leaf of the layout starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Agent,
    Editor,
    Shell,
    Command(Vec<String>),
}

/// The tree of panes a worktree opens with, as the old jw wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf(Run),
    Split {
        dir: SplitDir,
        ratio: u16,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    /// The leaves from left to right, top to bottom: the order panes are started in.
    pub fn leaves(&self) -> Vec<&Run> {
        match self {
            Node::Leaf(run) => vec![run],
            Node::Split { a, b, .. } => {
                let mut out = a.leaves();
                out.extend(b.leaves());
                out
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The file that was read; none when there is none and the defaults are used.
    pub source: Option<PathBuf>,
    /// Which of the two it is.
    pub from: Option<ConfigFrom>,
    /// Where the personal file is or would be.
    pub personal: PathBuf,
    pub root: PathBuf,
    /// The template of a worktree's branch.
    pub branch: String,
    pub setup: Vec<String>,
    pub agent: Vec<String>,
    pub editor: Vec<String>,
    pub layout: Option<Node>,
}

#[derive(Deserialize, Default)]
struct Raw {
    root: Option<String>,
    branch: Option<Spanned<String>>,
    setup: Option<Vec<Spanned<String>>>,
    agent: Option<Spanned<Table>>,
    layout: Option<Spanned<Table>>,
}

/// The settings of the repository whose top level is `top`; `personal_dir` holds the personal
/// files (REQ-482).
pub fn read(top: &Path, personal_dir: &Path) -> Result<Project, String> {
    read_with(
        top,
        &|k| std::env::var(k).ok(),
        &crate::browse::home(),
        personal_of(top, personal_dir),
    )
}

/// The personal file of the repository `top` is the top level of, or of a worktree of it (REQ-483).
fn personal_of(top: &Path, personal_dir: &Path) -> PathBuf {
    personal_file(personal_dir, origin_of(top).as_deref(), &main_top(top))
}

/// The top level of the repository a worktree belongs to: `top` itself when it is the repository.
fn main_top(top: &Path) -> PathBuf {
    crate::git::run(
        top,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(PathBuf::from)
    .filter(|d| d.file_name().is_some_and(|n| n == ".git"))
    .and_then(|d| d.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| top.to_path_buf())
}

/// The URL the repository came from, when it has one.
fn origin_of(top: &Path) -> Option<String> {
    crate::git::run(top, &["remote", "get-url", "origin"])
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
}

/// The name of the personal file of a repository, without its extension (REQ-483): its origin
/// as `host_path`, whatever way it was written, or its path when it has none.
pub fn key(origin: Option<&str>, top: &Path) -> String {
    let slug = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => c,
                '/' => '_',
                _ => '-',
            })
            .collect()
    };
    match origin.map(str::trim).filter(|o| !o.is_empty()) {
        Some(url) => {
            let url = url.to_lowercase();
            // `git@host:path` is `host/path`; a URL has its scheme and user dropped.
            let (host, path) = match url.split_once("://") {
                Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
                None => url.split_once(':').unwrap_or((url.as_str(), "")),
            };
            let host = host.rsplit('@').next().unwrap_or(host);
            let path = path.trim_matches('/');
            let path = path.strip_suffix(".git").unwrap_or(path);
            slug(&format!("{host}/{path}"))
        }
        None => format!(
            "local_{}",
            slug(top.to_string_lossy().trim_start_matches('/'))
        ),
    }
}

/// The personal file of the repository `top` (REQ-483).
pub fn personal_file(personal_dir: &Path, origin: Option<&str>, top: &Path) -> PathBuf {
    personal_dir.join(format!("{}.toml", key(origin, top)))
}

fn read_with(
    top: &Path,
    var: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    personal: PathBuf,
) -> Result<Project, String> {
    let found = [
        (top.join(FILE), ConfigFrom::Repo),
        (personal.clone(), ConfigFrom::Personal),
    ];
    for (file, from) in found {
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let project = parse(&text, Some(file), top, var, home)?;
                return Ok(Project {
                    from: Some(from),
                    personal,
                    ..project
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", label_of(Some(&file)))),
        }
    }
    let project = parse("", None, top, var, home)?;
    Ok(Project {
        personal,
        ..project
    })
}

/// How a file is named in an error: `.jw.toml`, or the name of the personal file.
fn label_of(source: Option<&PathBuf>) -> String {
    source
        .and_then(|s| s.file_name())
        .map_or_else(|| FILE.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Write `text` as the settings of the repository `top`, to the file `to` says (REQ-486): after
/// it reads as a file would, and whole, so that a reader never sees half of it. Says where it
/// went and whether the personal file is hidden by the repository's own (REQ-487).
pub fn write(
    top: &Path,
    personal_dir: &Path,
    to: ConfigFrom,
    text: &str,
) -> Result<(PathBuf, bool), String> {
    let personal = personal_of(top, personal_dir);
    let target = match to {
        ConfigFrom::Repo => top.join(FILE),
        ConfigFrom::Personal => personal,
    };
    parse(
        text,
        Some(target.clone()),
        top,
        &|k| std::env::var(k).ok(),
        &crate::browse::home(),
    )?;
    let dir = target.parent().ok_or("the file has no folder")?;
    let name = label_of(Some(&target));
    let private = to == ConfigFrom::Personal;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.tmp"));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(if private { 0o600 } else { 0o644 })
            .open(&tmp)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })?;
    Ok((target, private && top.join(FILE).exists()))
}

fn parse(
    text: &str,
    source: Option<PathBuf>,
    top: &Path,
    var: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Result<Project, String> {
    let label = label_of(source.as_ref());
    let fail = |at: usize, why: &str| format!("{label}, line {}: {why}", line_of(text, at));
    let raw: Raw = toml::from_str(text).map_err(|e| match e.span() {
        Some(span) => fail(span.start, e.message()),
        None => format!("{label}: {}", e.message()),
    })?;

    let root = match raw.root.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(root) => {
            let path = match root.strip_prefix('~') {
                Some("") => home.to_path_buf(),
                Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
                _ => PathBuf::from(root),
            };
            clean(&if path.is_absolute() {
                path
            } else {
                top.join(path)
            })
        }
        None => {
            let name = top.file_name().ok_or("the repository has no name")?;
            top.with_file_name(format!("{}-wt", name.to_string_lossy()))
        }
    };

    let branch = match raw.branch {
        Some(b) => {
            markers(b.get_ref(), &["name"]).map_err(|why| fail(b.span().start, &why))?;
            let t = b.get_ref().trim();
            if t.is_empty() {
                return Err(fail(b.span().start, "`branch` is empty"));
            }
            t.to_owned()
        }
        None => "feat/{name}".to_owned(),
    };

    let mut setup = Vec::new();
    for line in raw.setup.unwrap_or_default() {
        markers(line.get_ref(), &["name", "branch"])
            .map_err(|why| fail(line.span().start, &why))?;
        if !line.get_ref().trim().is_empty() {
            setup.push(line.into_inner());
        }
    }

    let agent = match raw.agent {
        Some(t) => {
            let at = t.span().start;
            agent_of(t.get_ref()).map_err(|why| fail(at, &why))?
        }
        None => vec!["claude".to_owned()],
    };

    let (mut editor, mut layout) = (Vec::new(), None);
    if let Some(t) = raw.layout {
        let at = t.span().start;
        let t = t.get_ref();
        if let Some(e) = t.get("editor") {
            let e = e
                .as_str()
                .ok_or_else(|| fail(at, "`editor` must be a string"))?;
            editor = words(e).map_err(|why| fail(at, &format!("`editor`: {why}")))?;
        }
        if t.contains_key("run") || t.contains_key("split") {
            let mut leaves = 0;
            layout = Some(node(t, &mut leaves).map_err(|why| fail(at, &why))?);
            if leaves > MAX_PANES {
                return Err(fail(
                    at,
                    &format!("the layout has {leaves} panes and at most {MAX_PANES} are allowed"),
                ));
            }
        }
    }
    if editor.is_empty() {
        editor = ["VISUAL", "EDITOR"]
            .iter()
            .filter_map(|k| var(k))
            .filter_map(|v| words(&v).ok())
            .find(|w| !w.is_empty())
            .unwrap_or_else(|| vec!["nvim".to_owned()]);
    }

    Ok(Project {
        source,
        from: None,
        personal: PathBuf::new(),
        root,
        branch,
        setup,
        agent,
        editor,
        layout,
    })
}

/// The program of the agent `default` names: its `start`, or else its own name.
fn agent_of(t: &Table) -> Result<Vec<String>, String> {
    let name = match t.get("default") {
        None => "claude",
        Some(Value::String(n)) if !n.trim().is_empty() => n.trim(),
        Some(_) => return Err("`[agent] default` must be the name of an agent".into()),
    };
    match t.get(name) {
        Some(Value::Table(own)) => match own.get("start") {
            Some(Value::String(start)) if !start.trim().is_empty() => {
                words(start).map_err(|why| format!("`[agent.{name}] start`: {why}"))
            }
            _ => Ok(vec![name.to_owned()]),
        },
        _ => Ok(vec![name.to_owned()]),
    }
}

fn node(t: &Table, leaves: &mut usize) -> Result<Node, String> {
    match (t.get("run"), t.get("split")) {
        (Some(_), Some(_)) => Err("a layout node has both `run` and `split`".into()),
        (None, None) => Err("a layout node needs `run`, or `split` with `a` and `b`".into()),
        (Some(run), None) => {
            let run = run.as_str().ok_or("`run` must be a string")?.trim();
            *leaves += 1;
            Ok(Node::Leaf(match run {
                "" => return Err("`run` is empty".into()),
                "agent" => Run::Agent,
                "editor" => Run::Editor,
                "shell" => Run::Shell,
                line => {
                    let w = words(line).map_err(|why| format!("`run`: {why}"))?;
                    Run::Command(w)
                }
            }))
        }
        (None, Some(split)) => {
            let dir = match split.as_str() {
                Some("right") => SplitDir::Row,
                Some("down") => SplitDir::Col,
                _ => {
                    return Err(format!(
                        "`split` must be \"right\" or \"down\", not {split}"
                    ));
                }
            };
            let ratio = match t.get("ratio") {
                None => 0.5,
                Some(Value::Float(f)) => *f,
                Some(Value::Integer(i)) => *i as f64,
                Some(_) => return Err("`ratio` must be a number".into()),
            };
            let thousandths = (ratio * 1000.0).round().clamp(0.0, f64::from(u16::MAX)) as u16;
            let mut side = |key: &str| -> Result<Box<Node>, String> {
                let table = t
                    .get(key)
                    .and_then(Value::as_table)
                    .ok_or_else(|| format!("`split` needs `{key}` as a table"))?;
                node(table, leaves).map(Box::new)
            };
            Ok(Node::Split {
                dir,
                ratio: thousandths.clamp(MIN_RATIO, MAX_RATIO),
                a: side("a")?,
                b: side("b")?,
            })
        }
    }
}

impl Project {
    /// What a leaf starts: its words, or none for the plain shell.
    pub fn argv(&self, run: &Run) -> Option<Vec<String>> {
        match run {
            Run::Agent => Some(self.agent.clone()),
            Run::Editor => Some(self.editor.clone()),
            Run::Shell => None,
            Run::Command(words) => Some(words.clone()),
        }
    }

    /// The programs of the panes the layout starts, by the names the panes carry.
    pub fn pane_names(&self, shell: &str) -> Vec<String> {
        let shell_name = || {
            Path::new(shell)
                .file_name()
                .map_or_else(|| shell.to_owned(), |n| n.to_string_lossy().into_owned())
        };
        match &self.layout {
            None => vec![shell_name()],
            Some(tree) => tree
                .leaves()
                .into_iter()
                .map(|run| {
                    self.argv(run)
                        .and_then(|a| title_for(&a))
                        .unwrap_or_else(shell_name)
                })
                .collect(),
        }
    }
}

/// The script of a worktree's setup: each line is written after `❯ ` and then run, one after
/// the other, stopping at the first that fails, and the end is reported as a shell would
/// report it (OSC 133;D with the exit status), which raises `shell_done` for the pane.
/// `{name}` and `{branch}` come already quoted.
pub fn setup_script(lines: &[String], name: &str, branch: &str) -> String {
    let quote = |w: &str| crate::launch::quote("/bin/sh", w);
    let (name, branch) = (quote(name), quote(branch));
    let chain = lines
        .iter()
        .map(|l| {
            let line = l.replace("{name}", &name).replace("{branch}", &branch);
            // A line in its own subshell and on its own lines: a trailing comment or an
            // `||` in it cannot swallow what comes after.
            format!(
                "printf '%s\\n' {} && (\n{line}\n)",
                quote(&format!("❯ {}", l.trim()))
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    format!("{chain}; s=$?; printf '\\033]133;D;%s\\007' \"$s\"; exit \"$s\"")
}

/// The words of `line` as a shell splits them: spaces separate, `'…'` and `"…"` group and
/// `\` quotes the next character. Nothing is expanded.
pub fn words(line: &str) -> Result<Vec<String>, String> {
    let open = || "a quote is left open".to_owned();
    let (mut out, mut cur, mut started) = (Vec::new(), String::new(), false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err(open()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => cur.push(c),
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err(open()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err(open()),
                    }
                }
            }
            '\\' => {
                started = true;
                cur.push(chars.next().unwrap_or('\\'));
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    Ok(out)
}

/// Fails on a `{key}` that is not one of `allowed`.
fn markers(s: &str, allowed: &[&str]) -> Result<(), String> {
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let key = &after[..close];
            let is_key = !key.is_empty()
                && key
                    .chars()
                    .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '.' | '-'));
            if is_key && !allowed.contains(&key) {
                let uses: Vec<String> = allowed.iter().map(|a| format!("{{{a}}}")).collect();
                return Err(format!(
                    "«{{{key}}}» is no marker (use {})",
                    uses.join(" or ")
                ));
            }
        }
        rest = after;
    }
    Ok(())
}

/// `path` without `.` and `..`, which are taken off lexically: `top/../x` is `x` beside `top`.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            part => out.push(part),
        }
    }
    out
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_text(text: &str) -> Result<Project, String> {
        parse(
            text,
            Some(PathBuf::from("/code/app/.jw.toml")),
            Path::new("/code/app"),
            &|_| None,
            Path::new("/home/u"),
        )
    }

    fn leaf(run: Run) -> Node {
        Node::Leaf(run)
    }

    #[test]
    fn with_no_file_everything_is_the_default() {
        let p = parse("", None, Path::new("/code/app"), &|_| None, Path::new("/h")).unwrap();
        assert_eq!(p.source, None);
        assert_eq!(p.root, Path::new("/code/app-wt"));
        assert_eq!(p.branch, "feat/{name}");
        assert!(p.setup.is_empty());
        assert_eq!(p.agent, ["claude"]);
        assert_eq!(p.editor, ["nvim"]);
        assert_eq!(p.layout, None);
        assert_eq!(p.pane_names("/bin/zsh"), ["zsh"]);
    }

    #[test]
    fn the_file_of_design_studio_reads_as_it_is() {
        let p = read_text(
            "# jw config for sa-design-studio, written by jw on its first worktree.\n\
             setup = [\n  \"bun install --frozen-lockfile\",\n]\n\n\
             [agent]\ndefault = \"claude\"\n\n[layout]\neditor = \"nvim\"\nrun = \"agent\"\n",
        )
        .unwrap();
        assert_eq!(p.setup, ["bun install --frozen-lockfile"]);
        assert_eq!(p.layout, Some(leaf(Run::Agent)));
        assert_eq!(p.pane_names("/bin/zsh"), ["claude"]);
        assert_eq!(p.source.as_deref(), Some(Path::new("/code/app/.jw.toml")));
    }

    #[test]
    fn every_key_is_read_and_the_ones_not_used_are_ignored() {
        let p = read_text(
            "root = \"../app-trees\"\nbranch = \"bryan/{name}\"\nports = { web = 1 }\nsync = \"merge\"\n\
             [env]\nfile = \".env\"\n[tui]\nleader = \"x\"\n\
             [agent]\ndefault = \"codex\"\n[agent.codex]\nstart = \"codex --full-auto\"\nresume = \"codex resume\"\n\
             [layout]\neditor = \"nvim -c 'Diffview'\"\nsplit = \"right\"\nratio = 0.6\n\
             a = { run = \"editor\" }\nb = { split = \"down\", a = { run = \"agent\" }, b = { run = \"htop -d 5\" } }\n",
        )
        .unwrap();
        assert_eq!(p.root, Path::new("/code/app-trees"));
        assert_eq!(p.branch, "bryan/{name}");
        assert_eq!(p.agent, ["codex", "--full-auto"]);
        assert_eq!(p.editor, ["nvim", "-c", "Diffview"]);
        let Some(Node::Split { dir, ratio, a, b }) = p.layout.clone() else {
            panic!("a split")
        };
        assert_eq!((dir, ratio), (SplitDir::Row, 600));
        assert_eq!(*a, leaf(Run::Editor));
        let Node::Split { dir, ratio, a, b } = *b else {
            panic!("a split")
        };
        assert_eq!((dir, ratio), (SplitDir::Col, 500));
        assert_eq!(*a, leaf(Run::Agent));
        assert_eq!(
            *b,
            leaf(Run::Command(vec!["htop".into(), "-d".into(), "5".into()]))
        );
        assert_eq!(p.pane_names("/bin/zsh"), ["nvim", "codex", "htop"]);
    }

    #[test]
    fn root_takes_the_home_and_starts_relative_paths_at_the_repository() {
        let root = |r: &str| read_text(&format!("root = \"{r}\"")).unwrap().root;
        assert_eq!(root("~/trees"), Path::new("/home/u/trees"));
        assert_eq!(root("~"), Path::new("/home/u"));
        assert_eq!(root("trees"), Path::new("/code/app/trees"));
        assert_eq!(root("/srv/wt"), Path::new("/srv/wt"));
        assert_eq!(root("../../x/./y"), Path::new("/x/y"));
        assert_eq!(root("  "), Path::new("/code/app-wt"));
    }

    #[test]
    fn a_ratio_outside_the_range_is_kept_inside_it() {
        let ratio = |r: &str| {
            let p = read_text(&format!(
                "[layout]\nsplit = \"right\"\nratio = {r}\na = {{ run = \"shell\" }}\nb = {{ run = \"shell\" }}"
            ))
            .unwrap();
            let Some(Node::Split { ratio, .. }) = p.layout else {
                panic!("a split")
            };
            ratio
        };
        assert_eq!(ratio("0.01"), MIN_RATIO);
        assert_eq!(ratio("2"), MAX_RATIO);
        assert_eq!(ratio("1"), MAX_RATIO);
        assert_eq!(ratio("0.25"), 250);
    }

    #[test]
    fn the_editor_comes_from_the_file_then_visual_then_editor_then_nvim() {
        let with = |env: &'static [(&'static str, &'static str)], text: &str| {
            parse(
                text,
                None,
                Path::new("/c/a"),
                &|k| {
                    env.iter()
                        .find(|(n, _)| *n == k)
                        .map(|(_, v)| (*v).to_owned())
                },
                Path::new("/h"),
            )
            .unwrap()
            .editor
        };
        let env = &[("VISUAL", "code -w"), ("EDITOR", "vim")];
        assert_eq!(with(env, "[layout]\neditor = \"hx\""), ["hx"]);
        assert_eq!(with(env, ""), ["code", "-w"]);
        assert_eq!(with(&[("EDITOR", "vim")], ""), ["vim"]);
        assert_eq!(with(&[("VISUAL", "")], ""), ["nvim"]);
    }

    #[test]
    fn a_leaf_without_a_layout_table_or_with_only_an_editor_gives_no_layout() {
        assert_eq!(
            read_text("[layout]\neditor = \"nvim\"").unwrap().layout,
            None
        );
    }

    #[test]
    fn each_kind_of_error_names_the_file_and_the_line() {
        let bad = |text: &str| read_text(text).unwrap_err();
        let m = bad("branch = \"feat/{name}\"\nsetup = [\"ok\", \"cd {nme}\"]\n");
        assert!(
            m.starts_with(".jw.toml, line 2: «{nme}» is no marker (use {name} or {branch})"),
            "{m}"
        );
        let m = bad("\n\nbranch = \"x/{branch}\"");
        assert!(
            m.starts_with(".jw.toml, line 3: «{branch}» is no marker (use {name})"),
            "{m}"
        );
        let m = bad("setup = [1]");
        assert!(m.starts_with(".jw.toml, line 1:"), "{m}");
        let m = bad("root = \nx");
        assert!(m.starts_with(".jw.toml, line"), "{m}");
        let m = bad("branch = \" \"");
        assert!(m.contains("`branch` is empty"), "{m}");
        let m = bad("\n[layout]\nrun = \"agent\"\nsplit = \"right\"");
        assert!(
            m.starts_with(".jw.toml, line 2:") && m.contains("both"),
            "{m}"
        );
        let m = bad("[layout]\nsplit = \"right\"\na = { run = \"agent\" }");
        assert!(m.contains("needs `b`"), "{m}");
        let m = bad("[layout]\nsplit = \"left\"\na = { run = \"agent\" }\nb = { run = \"agent\" }");
        assert!(m.contains("\"right\" or \"down\""), "{m}");
        let m = bad("[layout]\nsplit = \"right\"\na = {}\nb = { run = \"agent\" }");
        assert!(m.contains("needs `run`"), "{m}");
        let m = bad("[layout]\nrun = \"nvim 'x\"");
        assert!(m.contains("a quote is left open"), "{m}");
        let m = bad("[layout]\nrun = \"\"");
        assert!(m.contains("`run` is empty"), "{m}");
        let m = bad("[agent]\ndefault = 3");
        assert!(m.contains("name of an agent"), "{m}");
    }

    #[test]
    fn more_than_eight_panes_are_refused_and_eight_are_not() {
        let tree = |n: usize| {
            let mut t = "{ run = \"shell\" }".to_owned();
            for _ in 1..n {
                t = format!("{{ split = \"right\", a = {{ run = \"shell\" }}, b = {t} }}");
            }
            format!("[layout]\nsplit = \"down\"\na = {{ run = \"shell\" }}\nb = {t}")
        };
        // The tree has one leaf more than the nesting asked for.
        assert!(read_text(&tree(7)).is_ok());
        let m = read_text(&tree(8)).unwrap_err();
        assert!(m.contains("at most 8"), "{m}");
    }

    #[test]
    fn words_split_as_a_shell_does_without_expanding_anything() {
        let w = |s: &str| words(s).unwrap();
        assert_eq!(w("  nvim  -c   x "), ["nvim", "-c", "x"]);
        assert_eq!(
            w("nvim -c 'DiffviewOpen origin/main...HEAD'"),
            ["nvim", "-c", "DiffviewOpen origin/main...HEAD"]
        );
        assert_eq!(w("echo \"a b\" c\\ d"), ["echo", "a b", "c d"]);
        assert_eq!(w("echo \"\\$x \\q\""), ["echo", "$x \\q"]);
        assert_eq!(w("a '' b"), ["a", "", "b"]);
        assert_eq!(w("a$HOME ~"), ["a$HOME", "~"]);
        assert!(w("").is_empty());
        assert!(words("a 'b").is_err());
        assert!(words("a \"b").is_err());
    }

    #[test]
    fn a_setup_script_shows_each_line_runs_it_apart_and_quotes_the_markers() {
        let script = setup_script(
            &["bun install # now".into(), "echo {name} {branch}".into()],
            "ws-1",
            "feat/it's",
        );
        assert!(
            script
                .starts_with("printf '%s\\n' '❯ bun install # now' && (\nbun install # now\n) && ")
        );
        assert!(
            script.contains(" && (\necho 'ws-1' 'feat/it'\\''s'\n); s=$?;"),
            "{script}"
        );
    }

    #[test]
    fn the_script_runs_in_sh_stops_at_the_first_failure_and_shows_the_lines() {
        let script = setup_script(
            &[
                "echo one".into(),
                "echo {name} # c".into(),
                "false".into(),
                "echo never".into(),
            ],
            "ws-1",
            "b",
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            text,
            "❯ echo one\none\n❯ echo {name} # c\nws-1\n❯ false\n\x1b]133;D;1\x07"
        );
        assert_eq!(out.status.code(), Some(1));
    }

    #[test]
    fn the_personal_file_is_named_by_the_origin_however_it_is_written_and_by_the_path_without_one()
    {
        let top = Path::new("/home/u/code/App");
        for url in [
            "git@github.com:Acme/App.git",
            "https://github.com/acme/app.git",
            "https://user:secret@GitHub.com/acme/app/",
            "ssh://git@github.com/acme/app",
            "  git@github.com:acme/app\n",
        ] {
            assert_eq!(key(Some(url), top), "github.com_acme_app", "{url}");
        }
        assert_eq!(
            key(Some("ssh://git@host.example:2222/a/b.git"), top),
            "host.example-2222_a_b"
        );
        assert_eq!(
            key(None, top),
            "local_home_u_code_App",
            "a path keeps its case"
        );
        assert_eq!(
            key(Some("  "), top),
            key(None, top),
            "an empty origin is none"
        );
        assert_ne!(
            key(Some("https://github.com/acme-app"), top),
            key(Some("https://github.com/acme/app"), top),
            "a `-` and a `/` do not come to the same name"
        );
        assert_eq!(
            personal_file(Path::new("/c/projects"), Some("git@h:o/r.git"), top),
            Path::new("/c/projects/h_o_r.toml")
        );
    }
}
