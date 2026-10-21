//! The settings of a repository: its `.jw.toml`, the file the old jw wrote and jwn reads as it
//! is, or the person's own file of the same syntax outside the repository (spec REQ-330 to
//! REQ-334, REQ-482 to REQ-487, RAT-34, RAT-46). It is read afresh each time it is needed; a key
//! jwn does not use is ignored, so an old file keeps `env` and `dev` for the part that reads them.

use std::path::{Component, Path, PathBuf};

use contract::{ConfigFrom, OpenKind, ProjectEdit};
use serde::Deserialize;
use toml::{Spanned, Table};

const FILE: &str = ".jw.toml";
/// The most a settings text may weigh to be written, and the most a file name is made of.
const MAX_TEXT: usize = 64 * 1024;
const MAX_KEY: usize = 180;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The file that was read; none when there is none and the defaults are used.
    pub source: Option<PathBuf>,
    /// Which of the two it is.
    pub from: Option<ConfigFrom>,
    /// Where the personal file is or would be.
    pub personal: PathBuf,
    pub root: PathBuf,
    /// The `root` key as the file wrote it, when it did (REQ-516).
    pub root_text: Option<String>,
    pub setup: Vec<String>,
    /// The files of the checkout a new worktree gets a copy of (REQ-489).
    pub copy: Vec<String>,
    /// The names of the ports and their offsets in a worktree's block (REQ-494).
    pub ports: std::collections::BTreeMap<String, u32>,
    /// What the one panel of a workspace is (REQ-504).
    pub open: OpenKind,
}

#[derive(Deserialize, Default)]
struct Raw {
    root: Option<String>,
    setup: Option<Vec<Spanned<String>>>,
    copy: Option<Vec<Spanned<String>>>,
    ports: Option<Spanned<Table>>,
    open: Option<Spanned<String>>,
}

/// The settings of the repository whose top level is `top`; `personal_dir` holds the personal
/// files (REQ-482).
pub fn read(top: &Path, personal_dir: &Path) -> Result<Project, String> {
    read_with(top, &crate::browse::home(), personal_of(top, personal_dir))
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

/// The settings of a folder opened as a workspace (REQ-501): its repository's when it is the top
/// level of one, else its own `.jw.toml`, else the personal file named by its path.
pub fn read_folder(folder: &Path, personal_dir: &Path) -> Result<Project, String> {
    let is_top = crate::git::toplevel(folder)
        .ok()
        .and_then(|t| std::fs::canonicalize(t).ok())
        .is_some_and(|t| std::fs::canonicalize(folder).is_ok_and(|f| f == t));
    if is_top {
        return read(folder, personal_dir);
    }
    read_with(
        folder,
        &crate::browse::home(),
        personal_file(personal_dir, None, folder),
    )
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
    // No `/` and nothing that is not a letter, a digit, `.`, `-` or `_` is left in a name, so a
    // hostile origin cannot name a file outside the personal folder; a long one is cut.
    let slug = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => c,
                '/' => '_',
                _ => '-',
            })
            .take(MAX_KEY)
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

fn read_with(top: &Path, home: &Path, personal: PathBuf) -> Result<Project, String> {
    let found = [
        (top.join(FILE), ConfigFrom::Repo),
        (personal.clone(), ConfigFrom::Personal),
    ];
    for (file, from) in found {
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let project = parse(&text, Some(file), top, home)?;
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
    let project = parse("", None, top, home)?;
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
    if text.len() > MAX_TEXT {
        return Err(format!(
            "the settings weigh {} KiB: at most {} KiB are written",
            text.len() / 1024,
            MAX_TEXT / 1024
        ));
    }
    let personal = personal_of(top, personal_dir);
    let target = match to {
        ConfigFrom::Repo => top.join(FILE),
        ConfigFrom::Personal => personal,
    };
    parse(text, Some(target.clone()), top, &crate::browse::home())?;
    let dir = target.parent().ok_or("the file has no folder")?;
    let name = label_of(Some(&target));
    let private = to == ConfigFrom::Personal;
    {
        use std::os::unix::fs::DirBuilderExt;
        // The personal folder is the person's: nobody else lists what projects there are.
        let mut folder = std::fs::DirBuilder::new();
        folder.recursive(true);
        if private {
            folder.mode(0o700);
        }
        folder
            .create(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = dir.join(format!("{name}.tmp"));
    // A leftover, or a link a repository planted there, is removed and never written through.
    let _ = std::fs::remove_file(&tmp);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
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

/// Change some settings of the repository `top` in the file `to` says and keep what the file
/// holds besides them as it is, comments and order included (REQ-518): what the file says of
/// each field is compared with what is asked, and only a different one is written.
pub fn edit(
    top: &Path,
    personal_dir: &Path,
    to: ConfigFrom,
    set: &ProjectEdit,
) -> Result<(PathBuf, bool), String> {
    let target = match to {
        ConfigFrom::Repo => top.join(FILE),
        ConfigFrom::Personal => personal_of(top, personal_dir),
    };
    let text = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", target.display())),
    };
    let now = parse(&text, Some(target.clone()), top, &crate::browse::home())?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("{}: {e}", label_of(Some(&target))))?;
    change(&mut doc, &now, set);
    write(top, personal_dir, to, &render(&doc, &text))
}

/// The document as text; `toml_edit` reads CRLF as LF, so a file that had CRLF gets it back whole
/// and not half of its lines.
fn render(doc: &toml_edit::DocumentMut, original: &str) -> String {
    let out = doc.to_string();
    if original.contains("\r\n") {
        out.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        out
    }
}

/// Sets `key` to `new`, keeping the comment after the value it replaces.
fn put(doc: &mut toml_edit::DocumentMut, key: &str, mut new: toml_edit::Value) {
    if let Some(old) = doc.get(key).and_then(toml_edit::Item::as_value) {
        *new.decor_mut() = old.decor().clone();
    }
    doc[key] = toml_edit::Item::Value(new);
}

/// Puts into `doc` what `set` asks and `now` (what the document says) does not already say.
fn change(doc: &mut toml_edit::DocumentMut, now: &Project, set: &ProjectEdit) {
    use toml_edit::{Array, InlineTable};
    let lines = |v: &[String]| -> Vec<String> {
        v.iter()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect()
    };
    let list = |v: &[String]| toml_edit::Value::from(v.iter().collect::<Array>());
    if let Some(root) = &set.root {
        let root = root.trim();
        if root.is_empty() {
            doc.remove("root");
        } else if now.root_text.as_deref() != Some(root) {
            put(doc, "root", root.into());
        }
    }
    for (key, asked, now) in [
        ("setup", &set.setup, &now.setup),
        ("copy", &set.copy, &now.copy),
    ] {
        let Some(asked) = asked else { continue };
        let asked = lines(asked);
        if asked.is_empty() {
            doc.remove(key);
        } else if asked != lines(now) {
            put(doc, key, list(&asked));
        }
    }
    if let Some(ports) = &set.ports {
        if ports.is_empty() {
            doc.remove("ports");
        } else if *ports != now.ports {
            let table: InlineTable = ports
                .iter()
                .map(|(n, o)| (n.as_str(), i64::from(*o)))
                .collect();
            put(doc, "ports", table.into());
        }
    }
    if let Some(open) = set.open
        && open != now.open
    {
        let open = match open {
            OpenKind::Shell => "shell",
            OpenKind::Claude => "claude",
        };
        put(doc, "open", open.into());
    }
}

fn parse(text: &str, source: Option<PathBuf>, top: &Path, home: &Path) -> Result<Project, String> {
    let label = label_of(source.as_ref());
    let fail = |at: usize, why: &str| format!("{label}, line {}: {why}", line_of(text, at));
    let raw: Raw = toml::from_str(text).map_err(|e| match e.span() {
        Some(span) => fail(span.start, e.message()),
        None => format!("{label}: {}", e.message()),
    })?;

    let root_text = raw
        .root
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned);
    let root = match root_text.as_deref() {
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

    let mut setup = Vec::new();
    for line in raw.setup.unwrap_or_default() {
        markers(line.get_ref(), &["name", "branch"])
            .map_err(|why| fail(line.span().start, &why))?;
        if !line.get_ref().trim().is_empty() {
            setup.push(line.into_inner());
        }
    }

    let mut copy = Vec::new();
    for file in raw.copy.unwrap_or_default() {
        let path = file.get_ref().trim();
        if !crate::provision::is_inside(path) {
            return Err(fail(
                file.span().start,
                &format!("`copy` names {path:?}, which is not a path inside the repository"),
            ));
        }
        copy.push(path.to_owned());
    }

    let mut ports = std::collections::BTreeMap::new();
    if let Some(t) = raw.ports {
        let at = t.span().start;
        for (name, offset) in t.get_ref() {
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'));
            if !valid {
                return Err(fail(
                    at,
                    &format!("`ports` names {name:?}: use lowercase letters, digits and `_`"),
                ));
            }
            // `<PREFIX>_PORT_BASE` is the block's first port: a name must not take its place.
            if name == "base" {
                return Err(fail(
                    at,
                    "`ports` cannot name a port `base`: that is the first port of the block",
                ));
            }
            let Some(n) = offset.as_integer().filter(|n| (0..=99).contains(n)) else {
                return Err(fail(
                    at,
                    &format!("`ports.{name}` must be a whole number from 0 to 99"),
                ));
            };
            ports.insert(name.clone(), n as u32);
        }
    }

    let open = match raw.open {
        None => OpenKind::Shell,
        Some(o) => match o.get_ref().trim() {
            "shell" => OpenKind::Shell,
            "claude" => OpenKind::Claude,
            other => {
                return Err(fail(
                    o.span().start,
                    &format!("`open` is {other:?}: it must be \"shell\" or \"claude\""),
                ));
            }
        },
    };

    Ok(Project {
        source,
        from: None,
        personal: PathBuf::new(),
        root,
        root_text,
        setup,
        copy,
        ports,
        open,
    })
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
            Path::new("/home/u"),
        )
    }

    #[test]
    fn with_no_file_everything_is_the_default() {
        let p = parse("", None, Path::new("/code/app"), Path::new("/h")).unwrap();
        assert_eq!(p.source, None);
        assert_eq!(p.root, Path::new("/code/app-wt"));
        assert!(p.setup.is_empty());
        assert_eq!(p.open, OpenKind::Shell, "REQ-504");
    }

    #[test]
    fn open_is_a_shell_or_claude_and_what_else_is_refused_naming_the_line() {
        let open = |t: &str| read_text(t).map(|p| p.open);
        assert_eq!(open("open = \"claude\"\n"), Ok(OpenKind::Claude));
        assert_eq!(open("open = \" shell \"\n"), Ok(OpenKind::Shell));
        let m = open("setup = []\nopen = \"nvim\"\n").unwrap_err();
        assert!(
            m.starts_with(".jw.toml, line 2:") && m.contains("\"shell\" or \"claude\""),
            "{m}"
        );
        assert!(
            open("open = 3\n")
                .unwrap_err()
                .starts_with(".jw.toml, line 1:")
        );
    }

    #[test]
    fn the_layout_the_agent_and_the_editor_are_ignored_even_when_they_are_wrong() {
        // The file of design-studio, as N78 read it, and a layout with nothing right in it.
        let p = read_text(
            "setup = [\n  \"bun install --frozen-lockfile\",\n]\n\n\
             [agent]\ndefault = 3\n\n[layout]\neditor = \"nvim\"\nrun = \"agent\"\nsplit = \"left\"\n",
        )
        .unwrap();
        assert_eq!(p.setup, ["bun install --frozen-lockfile"]);
        assert_eq!(p.open, OpenKind::Shell, "REQ-505");
        let p = read_text(
            "root = \"../app-trees\"\nbranch = \"bryan/{name}\"\nsync = \"merge\"\n[env]\nfile = \".env\"\n[tui]\nleader = \"x\"\n",
        )
        .unwrap();
        assert_eq!(p.root, Path::new("/code/app-trees"));
    }

    /// What `change` makes of `text`, checked to read as a file does.
    fn changed(text: &str, set: &ProjectEdit) -> (String, Project) {
        let now = read_text(text).unwrap();
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        change(&mut doc, &now, set);
        let out = render(&doc, text);
        let project = read_text(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
        (out, project)
    }

    #[test]
    fn a_change_keeps_a_file_that_is_written_in_other_ways_and_reads_back() {
        let ports = |p: &[(&str, u32)]| ProjectEdit {
            ports: Some(p.iter().map(|(n, o)| ((*n).to_owned(), *o)).collect()),
            ..Default::default()
        };
        // `[ports]` as a table becomes the inline one, and the tables around it stay
        let (out, p) = changed(
            "# c\nopen = \"claude\"\n[env]\nfile = \".env\"\n[ports]\nweb = 1\n[tui]\nleader = \"x\"\n",
            &ports(&[("web", 1), ("api", 2)]),
        );
        assert_eq!(p.ports.len(), 2);
        assert!(
            out.contains("[env]") && out.contains("[tui]") && out.contains("# c"),
            "{out}"
        );
        // dotted keys
        let (_, p) = changed("ports.web = 1\n", &ports(&[("web", 5)]));
        assert_eq!(p.ports["web"], 5);
        // no newline at the end, and a file written with CRLF
        let set = ProjectEdit {
            open: Some(OpenKind::Claude),
            copy: Some(vec![".env".into()]),
            ..Default::default()
        };
        let (_, p) = changed("root = \"../x\"", &set);
        assert_eq!(
            (p.open, p.copy.as_slice()),
            (OpenKind::Claude, [".env".to_owned()].as_slice())
        );
        let (out, p) = changed("root = \"../x\"\r\n[env]\r\nfile = \".env\"\r\n", &set);
        assert_eq!(p.open, OpenKind::Claude);
        assert!(out.starts_with("root = \"../x\"\r\n"), "{out:?}");
        // taking out what is not there, or putting what is there, is no change at all
        let same =
            "# team\nroot = \"../x\" # beside\nsetup = [\"a\"]\n[agent]\ndefault = \"claude\"\n";
        let none = ProjectEdit {
            root: Some("../x".into()),
            setup: Some(vec!["a".into()]),
            copy: Some(vec![]),
            ports: Some(Default::default()),
            open: Some(OpenKind::Shell),
        };
        assert_eq!(changed(same, &none).0, same);
        // a replaced value keeps the comment after it
        let root = ProjectEdit {
            root: Some("../y".into()),
            ..Default::default()
        };
        assert!(changed(same, &root).0.contains("root = \"../y\" # beside"));
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
    fn each_kind_of_error_names_the_file_and_the_line() {
        let bad = |text: &str| read_text(text).unwrap_err();
        let m = bad("root = \"x\"\nsetup = [\"ok\", \"cd {nme}\"]\n");
        assert!(
            m.starts_with(".jw.toml, line 2: «{nme}» is no marker (use {name} or {branch})"),
            "{m}"
        );
        let m = bad("setup = [1]");
        assert!(m.starts_with(".jw.toml, line 1:"), "{m}");
        let m = bad("root = \nx");
        assert!(m.starts_with(".jw.toml, line"), "{m}");
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

    #[test]
    fn no_origin_can_name_a_file_outside_the_personal_folder_and_a_long_one_is_cut() {
        let top = Path::new("/r");
        for hostile in [
            "../../etc/passwd",
            "/etc/cron.d/x",
            "https://h/..%2f..%2fx",
            "..",
            "a/../../b",
        ] {
            let name = key(Some(hostile), top);
            assert!(
                !name.contains('/') && !name.contains('\0'),
                "{hostile}: {name}"
            );
            let file = personal_file(Path::new("/c/projects"), Some(hostile), top);
            assert_eq!(
                file.parent(),
                Some(Path::new("/c/projects")),
                "{hostile}: {file:?}"
            );
        }
        assert_eq!(
            key(Some(&format!("https://h/{}", "a".repeat(500))), top).len(),
            MAX_KEY
        );
        assert!(key(None, Path::new(&format!("/{}", "d/".repeat(300)))).len() <= MAX_KEY + 6);
    }

    #[test]
    fn ports_are_names_with_offsets_from_0_to_99_and_what_is_not_says_where() {
        let p = read_text("ports = { web = 0, api = 1, db_main = 99 }\n").unwrap();
        assert_eq!(
            p.ports.into_iter().collect::<Vec<_>>(),
            [
                ("api".to_owned(), 1),
                ("db_main".to_owned(), 99),
                ("web".to_owned(), 0)
            ]
        );
        assert!(read_text("").unwrap().ports.is_empty());
        for (text, why) in [
            ("ports = { Web = 1 }\n", "`ports` names \"Web\""),
            ("ports = { base = 1 }\n", "cannot name a port `base`"),
            ("ports = { \"a b\" = 1 }\n", "`ports` names \"a b\""),
            (
                "ports = { web = 100 }\n",
                "`ports.web` must be a whole number from 0 to 99",
            ),
            (
                "ports = { web = -1 }\n",
                "`ports.web` must be a whole number from 0 to 99",
            ),
            (
                "ports = { web = \"1\" }\n",
                "`ports.web` must be a whole number from 0 to 99",
            ),
            (
                "ports = { web = 1.5 }\n",
                "`ports.web` must be a whole number from 0 to 99",
            ),
        ] {
            let err = read_text(text).unwrap_err();
            assert!(err.starts_with(".jw.toml, line 1"), "{text}: {err}");
            assert!(err.contains(why), "{text}: {err}");
        }
    }
}
