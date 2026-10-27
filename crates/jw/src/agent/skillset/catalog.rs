//! The entries of the skillset: the catalog that is fixed in the code and the ones the person adds
//! in a file (N184b, REQ-1008, REQ-1009).

use std::path::Path;

use serde::Deserialize;

use super::super::roles;
use super::super::{bin, roles_skill_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::agent) enum Kind {
    Skill,
    Role,
    Plugin,
}

impl Kind {
    pub(in crate::agent) fn name(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Role => "role",
            Kind::Plugin => "plugin",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::agent) enum Source {
    /// Text the binary carries.
    Embedded,
    /// A skill's `SKILL.md` or a role's `.md` in a GitHub repository: its folder is copied.
    Github {
        repo: String,
        path: String,
        branch: Option<String>,
    },
    /// `claude plugin install <plugin>`, its marketplace being the GitHub repository `repo`.
    Plugin { plugin: String, repo: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::agent) struct Entry {
    pub name: String,
    pub kind: Kind,
    pub description: String,
    pub source: Source,
    /// Added by the person's file and not by the code.
    pub yours: bool,
}

fn embedded(name: &str, kind: Kind, description: &str) -> Entry {
    Entry {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        source: Source::Embedded,
        yours: false,
    }
}

/// The catalog of the code (REQ-1008): reviewed, so a change to it is a change of the code.
pub(in crate::agent) fn builtin() -> Vec<Entry> {
    let mut entries = vec![
        embedded(
            bin(),
            Kind::Skill,
            "drive the other workspaces and agents of the session",
        ),
        embedded(
            &format!("{}-tasks", bin()),
            Kind::Skill,
            "turn a plan into the workspace's task list and work it to the end",
        ),
        embedded(
            roles_skill_name(),
            Kind::Skill,
            "take a role (brain, researcher, ...) in this session",
        ),
    ];
    entries.extend(
        roles::built_in_descriptions()
            .into_iter()
            .map(|(name, description)| embedded(&name, Kind::Role, &description)),
    );
    entries.push(Entry {
        name: "caveman".into(),
        kind: Kind::Skill,
        description: "talk with far fewer tokens, the same technical content".into(),
        source: Source::Github {
            repo: "juliusbrussee/caveman".into(),
            path: "skills/caveman/SKILL.md".into(),
            branch: None,
        },
        yours: false,
    });
    entries.push(Entry {
        name: "ponytail".into(),
        kind: Kind::Plugin,
        description: "the smallest change that solves the task (a plugin with hooks)".into(),
        source: Source::Plugin {
            plugin: "ponytail@ponytail".into(),
            repo: "DietrichGebert/ponytail".into(),
        },
        yours: false,
    });
    entries
}

/// `[A-Za-z0-9._-]`, not starting with `-` and not made of dots only: a part of a GitHub name.
fn plain(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !part.starts_with('-')
        && part.chars().any(|c| c != '.')
}

/// `<owner>/<repo>`.
pub(in crate::agent) fn valid_repo(repo: &str) -> bool {
    matches!(repo.split_once('/'), Some((o, r)) if plain(o) && plain(r))
}

/// A path inside a repository: relative, with plain parts, no `.` or `..`.
fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(plain)
}

/// `<name>@<marketplace>`.
fn valid_plugin(plugin: &str) -> bool {
    matches!(plugin.split_once('@'), Some((n, m)) if plain(n) && plain(m))
}

#[derive(Debug, Deserialize)]
struct Raw {
    name: String,
    kind: Kind,
    #[serde(default)]
    description: String,
    source: String,
    repo: Option<String>,
    path: Option<String>,
    branch: Option<String>,
    plugin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default)]
    entry: Vec<toml::Value>,
}

/// A raw entry made an entry, or why it is not one (REQ-1009).
fn entry_of(raw: Raw, taken: &[Entry]) -> Result<Entry, String> {
    let name = raw.name;
    if !roles::valid_name(&name) {
        return Err(format!("`{name}` is not a name ([a-z0-9][a-z0-9_-]*)"));
    }
    if taken.iter().any(|e| e.name == name) {
        return Err(format!(
            "`{name}` is taken: the catalog's entries cannot be replaced"
        ));
    }
    let need = |v: Option<String>, what: &str| v.ok_or(format!("`{name}` has no `{what}`"));
    let repo = need(raw.repo, "repo")?;
    if !valid_repo(&repo) {
        return Err(format!("`{name}`: `{repo}` is not <owner>/<repo>"));
    }
    let source = match (raw.source.as_str(), raw.kind) {
        ("github", Kind::Skill | Kind::Role) => {
            let path = need(raw.path, "path")?;
            let leaf = if raw.kind == Kind::Skill {
                "SKILL.md"
            } else {
                ".md"
            };
            let components = path.matches('/').count();
            if !valid_path(&path)
                || !path.ends_with(leaf)
                || (raw.kind == Kind::Skill && components == 0)
            {
                return Err(format!(
                    "`{name}`: `{path}` is not the path of a {} ({}, in a folder of the repository)",
                    raw.kind.name(),
                    if raw.kind == Kind::Skill {
                        "its SKILL.md"
                    } else {
                        "a .md file"
                    }
                ));
            }
            if let Some(b) = &raw.branch
                && !crate::update::valid_ref(b)
            {
                return Err(format!("`{name}`: `{b}` is not a branch"));
            }
            Source::Github {
                repo,
                path,
                branch: raw.branch,
            }
        }
        ("plugin", Kind::Plugin) => {
            let plugin = need(raw.plugin, "plugin")?;
            if !valid_plugin(&plugin) {
                return Err(format!("`{name}`: `{plugin}` is not <name>@<marketplace>"));
            }
            Source::Plugin { plugin, repo }
        }
        (source, kind) => {
            return Err(format!(
                "`{name}`: source `{source}` is not for a {} (github for a skill or a role, plugin for a plugin)",
                kind.name()
            ));
        }
    };
    Ok(Entry {
        name,
        kind: raw.kind,
        description: raw.description,
        source,
        yours: true,
    })
}

/// The entries of the person's file after the catalog's (REQ-1009); a file that is not there has
/// none, and what cannot be an entry is a warning.
pub(in crate::agent) fn yours(
    file: &Path,
    taken: &[Entry],
    warnings: &mut Vec<String>,
) -> Vec<Entry> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warnings.push(format!("{}: {e}", file.display()));
            return Vec::new();
        }
    };
    parse(&text, taken)
        .into_iter()
        .filter_map(|r| match r {
            Ok(e) => Some(e),
            Err(why) => {
                warnings.push(format!("{}: {why}", file.display()));
                None
            }
        })
        .collect()
}

fn parse(text: &str, taken: &[Entry]) -> Vec<Result<Entry, String>> {
    let file: File = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => return vec![Err(format!("not valid: {e}"))],
    };
    let mut made: Vec<Entry> = taken.to_vec();
    let mut out = Vec::new();
    for value in file.entry {
        let one = value
            .try_into::<Raw>()
            .map_err(|e| format!("an entry is not valid: {e}"))
            .and_then(|raw| entry_of(raw, &made));
        if let Ok(e) = &one {
            made.push(e.clone());
        }
        out.push(one);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_the_entries_of_the_spec_and_every_name_is_a_name() {
        let all = builtin();
        let view: Vec<_> = all.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        assert_eq!(
            view,
            [
                (bin(), Kind::Skill),
                (format!("{}-tasks", bin()).as_str(), Kind::Skill),
                (roles_skill_name(), Kind::Skill),
                ("brain", Kind::Role),
                ("researcher", Kind::Role),
                ("caveman", Kind::Skill),
                ("ponytail", Kind::Plugin),
            ]
        );
        for e in &all {
            assert!(roles::valid_name(&e.name) && !e.yours && !e.description.is_empty());
            match &e.source {
                Source::Github { repo, path, branch } => {
                    assert!(valid_repo(repo) && valid_path(path) && branch.is_none());
                }
                Source::Plugin { plugin, repo } => {
                    assert!(valid_plugin(plugin) && valid_repo(repo))
                }
                Source::Embedded => {}
            }
        }
        let mut names: Vec<_> = all.iter().map(|e| &e.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "a name twice");
        let ponytail = all.iter().find(|e| e.name == "ponytail").unwrap();
        assert_eq!(
            ponytail.source,
            Source::Plugin {
                plugin: "ponytail@ponytail".into(),
                repo: "DietrichGebert/ponytail".into()
            }
        );
    }

    #[test]
    fn what_goes_into_an_argument_has_a_charset() {
        for ok in ["a/b", "juliusbrussee/caveman", "o.w-n_1/r.e-p_o"] {
            assert!(valid_repo(ok), "{ok}");
        }
        for bad in [
            "", "a", "a/", "/b", "a/b/c", "-a/b", "a/-b", "../b", "a/..", "a b/c", "a/b;x", "a/b\n",
        ] {
            assert!(!valid_repo(bad), "{bad:?}");
        }
        assert!(valid_path("skills/caveman/SKILL.md"));
        for bad in [
            "",
            "/etc/x",
            "a//b",
            "a/../b",
            "./a",
            "a/b/",
            "-x/SKILL.md",
            "a b/SKILL.md",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(valid_plugin("ponytail@ponytail"));
        for bad in ["ponytail", "@x", "x@", "-x@y", "x@-y", "a@b@c", "x y@z"] {
            assert!(!valid_plugin(bad), "{bad:?}");
        }
    }

    fn ok(r: &Result<Entry, String>) -> &Entry {
        r.as_ref().unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_persons_file_adds_entries_and_marks_them() {
        let text = r#"
[[entry]]
name = "mine"
kind = "skill"
description = "my skill"
source = "github"
repo = "me/skills"
path = "mine/SKILL.md"
branch = "dev"

[[entry]]
name = "tool"
kind = "plugin"
source = "plugin"
plugin = "tool@market"
repo = "me/market"

[[entry]]
name = "voice"
kind = "role"
source = "github"
repo = "me/roles"
path = "roles/voice.md"
"#;
        let made = parse(text, &builtin());
        assert_eq!(made.len(), 3);
        let mine = ok(&made[0]);
        assert!(mine.yours && mine.kind == Kind::Skill && mine.description == "my skill");
        assert_eq!(
            mine.source,
            Source::Github {
                repo: "me/skills".into(),
                path: "mine/SKILL.md".into(),
                branch: Some("dev".into())
            }
        );
        assert_eq!(
            ok(&made[1]).source,
            Source::Plugin {
                plugin: "tool@market".into(),
                repo: "me/market".into()
            }
        );
        assert_eq!(ok(&made[2]).description, "");
    }

    #[test]
    fn an_entry_that_is_not_one_is_a_refusal_and_never_replaces_the_catalogs() {
        let case = |body: &str| {
            let text = format!(
                "[[entry]]\nkind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"x/SKILL.md\"\n{body}\n"
            );
            parse(&text, &builtin()).remove(0)
        };
        assert!(ok(&case("name = \"fine\"")).yours);
        // A name of the catalog: caveman cannot be re-pointed.
        let e = case("name = \"caveman\"").unwrap_err();
        assert!(e.contains("taken"), "{e}");
        for bad in ["name = \"Not A Name\"", "name = \"\"", "name = \"../x\""] {
            assert!(case(bad).is_err(), "{bad}");
        }
        let other = |extra: &str| {
            let text = format!("[[entry]]\nname = \"x\"\n{extra}\n");
            parse(&text, &builtin()).remove(0)
        };
        let bad = [
            "kind = \"skill\"\nsource = \"embedded\"\nrepo = \"a/b\"\npath = \"x/SKILL.md\"",
            "kind = \"skill\"\nsource = \"github\"\nrepo = \"--upload-pack=x/b\"\npath = \"x/SKILL.md\"",
            "kind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"../x/SKILL.md\"",
            "kind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"SKILL.md\"",
            "kind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"x/other.md\"",
            "kind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"x/SKILL.md\"\nbranch = \"-x\"",
            "kind = \"skill\"\nsource = \"plugin\"\nrepo = \"a/b\"\nplugin = \"x@y\"",
            "kind = \"plugin\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"x/SKILL.md\"",
            "kind = \"plugin\"\nsource = \"plugin\"\nrepo = \"a/b\"\nplugin = \"-x@y\"",
            "kind = \"plugin\"\nsource = \"plugin\"\nrepo = \"a/b\"",
            "kind = \"thing\"\nsource = \"github\"",
        ];
        for b in bad {
            assert!(other(b).is_err(), "{b}");
        }
        // The same name twice in the file: the second is refused.
        let twice = "[[entry]]\nname = \"a\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"x/SKILL.md\"\n[[entry]]\nname = \"a\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"a/b\"\npath = \"y/SKILL.md\"\n";
        let made = parse(twice, &builtin());
        assert!(made[0].is_ok() && made[1].is_err());
        // Not TOML at all: one warning and nothing added.
        assert!(parse("[[entry", &builtin())[0].is_err());
    }

    #[test]
    fn a_file_that_is_not_there_has_no_entries_and_a_bad_one_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();
        assert!(yours(&dir.path().join("none.toml"), &builtin(), &mut warnings).is_empty());
        assert!(warnings.is_empty());
        let file = dir.path().join("s.toml");
        std::fs::write(
            &file,
            "[[entry]]\nname = \"caveman\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"evil/caveman\"\npath = \"x/SKILL.md\"\n",
        )
        .unwrap();
        assert!(yours(&file, &builtin(), &mut warnings).is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("s.toml") && warnings[0].contains("taken"));
    }
}
