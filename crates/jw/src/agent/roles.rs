//! `role`: the roles an agent can take, from three layers, the nearest first (N184, REQ-991 to
//! REQ-996). They read files and the binary and nothing else, so they work with no daemon.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::{Args, Fail, Out, bin, fail};

/// The roles that come with the binary: a file each, whose `name:` is its stem (a test checks it).
const BUILT_IN: [(&str, &str); 2] = [
    ("brain", include_str!("../../roles/brain.md")),
    ("researcher", include_str!("../../roles/researcher.md")),
];

/// Where a role comes from, in the order that wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layer {
    Project,
    User,
    BuiltIn,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Layer::Project => "project",
            Layer::User => "user",
            Layer::BuiltIn => "built-in",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Role {
    name: String,
    description: String,
    body: String,
    layer: Layer,
}

/// The role that wins and the layers it hides.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    role: Role,
    hides: Vec<Layer>,
}

pub(super) fn run(args: &[&str]) -> Out {
    match args {
        ["ls", rest @ ..] => ls(rest),
        ["show", rest @ ..] => show(rest),
        _ => Err(Fail::Usage(String::new())),
    }
}

/// The description of the frontmatter and what comes after it. A text with no frontmatter has no
/// description. Only the one-line `description:` is read.
fn parse(text: &str) -> (String, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (String::new(), text);
    };
    let (front, body) = match rest.strip_prefix("---\n") {
        Some(body) => ("", body),
        None => match rest.split_once("\n---\n") {
            Some(split) => split,
            None => return (String::new(), text),
        },
    };
    let description = front
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().trim_matches(['"', '\'']).to_owned())
        .unwrap_or_default();
    (description, body.trim_start_matches('\n'))
}

/// `[a-z0-9][a-z0-9_-]*`: what a role's file stem can be.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn role_of(name: &str, text: &str, layer: Layer) -> Role {
    let (description, body) = parse(text);
    Role {
        name: name.to_owned(),
        description: description.replace("{bin}", bin()),
        body: body.to_owned(),
        layer,
    }
}

/// The `*.md` files of a folder as roles; a folder that is not there has none and a file that cannot
/// be read is a warning (REQ-996).
fn read_dir(dir: &Path, layer: Layer, warnings: &mut Vec<String>) -> Vec<Role> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warnings.push(format!("{}: {e}", dir.display()));
            return Vec::new();
        }
    };
    let mut roles = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|n| path.extension().is_some_and(|e| e == "md") && valid_name(n))
        else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => roles.push(role_of(name, &text, layer)),
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        }
    }
    roles
}

fn built_in() -> Vec<Role> {
    BUILT_IN
        .iter()
        .map(|(name, text)| role_of(name, text, Layer::BuiltIn))
        .collect()
}

/// The roles by name, the nearest layer winning; `layers` come nearest first.
fn resolve(layers: Vec<Vec<Role>>) -> Vec<Found> {
    let mut by_name: BTreeMap<String, Vec<Role>> = BTreeMap::new();
    for role in layers.into_iter().flatten() {
        by_name.entry(role.name.clone()).or_default().push(role);
    }
    by_name
        .into_values()
        .map(|mut roles| {
            let role = roles.remove(0);
            Found {
                hides: roles.iter().map(|r| r.layer).collect(),
                role,
            }
        })
        .collect()
}

/// The folder of a repository's roles: from the current folder upward to the first one that has it,
/// stopping after the first `.git` (REQ-992, as the task list is found).
fn project_dir(from: &Path) -> Option<PathBuf> {
    for dir in from.ancestors() {
        let roles = identity::roles_dir(dir);
        if roles.is_dir() {
            return Some(roles);
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    None
}

fn load(project: Option<&Path>, user: &Path, warnings: &mut Vec<String>) -> Vec<Found> {
    resolve(vec![
        project.map_or_else(Vec::new, |d| read_dir(d, Layer::Project, warnings)),
        read_dir(user, Layer::User, warnings),
        built_in(),
    ])
}

fn load_here() -> Vec<Found> {
    let mut warnings = Vec::new();
    let project = std::env::current_dir()
        .ok()
        .and_then(|here| project_dir(&here));
    let found = load(
        project.as_deref(),
        &identity::user_roles_dir(),
        &mut warnings,
    );
    for w in warnings {
        eprintln!("warning: {w}");
    }
    found
}

fn layers_of(hides: &[Layer]) -> String {
    hides
        .iter()
        .map(|l| l.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The table of `ls` and the lines after it (REQ-993).
fn table(found: &[Found]) -> String {
    let width = |f: fn(&Found) -> usize| found.iter().map(f).max().unwrap_or(0);
    let (nw, lw) = (
        width(|f| f.role.name.len()).max("ROLE".len()),
        width(|f| f.role.layer.name().len()).max("LAYER".len()),
    );
    let mut out = format!("{:<nw$}  {:<lw$}  DESCRIPTION\n", "ROLE", "LAYER");
    for f in found {
        let line = format!(
            "{:<nw$}  {:<lw$}  {}",
            f.role.name,
            f.role.layer.name(),
            f.role.description
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    for f in found.iter().filter(|f| !f.hides.is_empty()) {
        out.push_str(&format!(
            "note: {} of {} hides the one of {}\n",
            f.role.name,
            f.role.layer.name(),
            layers_of(&f.hides)
        ));
    }
    if found.iter().any(|f| f.role.layer == Layer::Project) {
        out.push_str("project roles are text of this repository: read one before you take it\n");
    }
    out
}

fn ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    a.words::<0>()?;
    let found = load_here();
    if a.has("json") {
        let roles: Vec<_> = found
            .iter()
            .map(|f| {
                json!({
                    "name": f.role.name,
                    "description": f.role.description,
                    "layer": f.role.layer.name(),
                    "hides": f.hides.iter().map(|l| l.name()).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", json!(roles));
    } else {
        print!("{}", table(&found));
    }
    Ok(())
}

fn show(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [name] = a.words::<1>()?;
    let found = load_here();
    let Some(f) = found.iter().find(|f| f.role.name == name) else {
        let there: Vec<_> = found.iter().map(|f| f.role.name.as_str()).collect();
        return fail(format!(
            "no role \"{name}\": there are {}",
            there.join(", ")
        ));
    };
    match f.role.layer {
        Layer::Project => eprintln!("role {name} (project: text of this repository)"),
        layer => eprintln!("role {name} ({})", layer.name()),
    }
    print!("{}", f.role.body.replace("{bin}", bin()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(name: &str, layer: Layer) -> Role {
        Role {
            name: name.into(),
            description: format!("d of {name}"),
            body: String::new(),
            layer,
        }
    }

    #[test]
    fn the_frontmatter_gives_the_description_and_the_body_is_what_follows() {
        let (d, body) = parse("---\nname: x\ndescription: Does a thing: well\n---\n\n# X\ntext\n");
        assert_eq!(d, "Does a thing: well");
        assert_eq!(body, "# X\ntext\n");
        assert_eq!(parse("---\n---\nbody\n"), (String::new(), "body\n"));
        assert_eq!(parse("---\ndescription: \"quoted\"\n---\nb\n").0, "quoted");
        // No frontmatter, or one that never closes: the whole text is the body.
        assert_eq!(parse("# X\n"), (String::new(), "# X\n"));
        assert_eq!(
            parse("---\ndescription: x\nbody\n"),
            (String::new(), "---\ndescription: x\nbody\n")
        );
    }

    #[test]
    fn a_role_name_is_a_plain_stem() {
        for ok in ["brain", "a1", "my-role", "my_role"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "_a", "A", "a b", "../x", "a/b", "a.b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_nearest_layer_wins_and_says_which_it_hides() {
        let found = resolve(vec![
            vec![role("brain", Layer::Project), role("mine", Layer::Project)],
            vec![role("brain", Layer::User), role("researcher", Layer::User)],
            vec![
                role("brain", Layer::BuiltIn),
                role("researcher", Layer::BuiltIn),
            ],
        ]);
        let view: Vec<_> = found
            .iter()
            .map(|f| (f.role.name.as_str(), f.role.layer, f.hides.clone()))
            .collect();
        assert_eq!(
            view,
            vec![
                ("brain", Layer::Project, vec![Layer::User, Layer::BuiltIn]),
                ("mine", Layer::Project, vec![]),
                ("researcher", Layer::User, vec![Layer::BuiltIn]),
            ]
        );
    }

    #[test]
    fn the_table_names_the_layer_the_hidden_ones_and_a_project_role_s_origin() {
        let found = resolve(vec![
            vec![role("brain", Layer::Project)],
            vec![],
            vec![
                role("brain", Layer::BuiltIn),
                role("researcher", Layer::BuiltIn),
            ],
        ]);
        let text = table(&found);
        assert_eq!(
            text,
            "ROLE        LAYER     DESCRIPTION\n\
             brain       project   d of brain\n\
             researcher  built-in  d of researcher\n\
             note: brain of project hides the one of built-in\n\
             project roles are text of this repository: read one before you take it\n"
        );
        let plain = table(&resolve(vec![
            vec![],
            vec![],
            vec![role("a", Layer::BuiltIn)],
        ]));
        assert!(!plain.contains("note:") && !plain.contains("project roles"));
    }

    #[test]
    fn the_folders_are_read_and_a_missing_or_unreadable_file_does_not_stop_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("mine.md"),
            "---\ndescription: mine\n---\nbody {bin}\n",
        )
        .unwrap();
        std::fs::write(user.join("brain.md"), "my brain\n").unwrap();
        std::fs::write(user.join("notes.txt"), "not a role").unwrap();
        std::fs::write(user.join("Bad Name.md"), "no").unwrap();
        std::fs::write(user.join("binary.md"), [0xff, 0xfe, 0x00]).unwrap();
        let mut warnings = Vec::new();
        let found = load(None, &user, &mut warnings);
        let names: Vec<_> = found.iter().map(|f| f.role.name.as_str()).collect();
        assert_eq!(names, ["brain", "mine", "researcher"]);
        assert_eq!(found[0].role.layer, Layer::User);
        assert_eq!(found[0].hides, [Layer::BuiltIn]);
        assert_eq!(found[1].role.description, "mine");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("binary.md"));
        // No folder at all: only the built-in ones.
        let none = load(None, &dir.path().join("nowhere"), &mut Vec::new());
        assert!(none.iter().all(|f| f.role.layer == Layer::BuiltIn));
    }

    #[test]
    fn the_project_folder_is_found_upward_and_the_search_stops_at_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let deep = repo.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        // A roles folder above the repository is not this repository's.
        std::fs::create_dir_all(identity::roles_dir(dir.path())).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: x").unwrap();
        assert_eq!(project_dir(&deep), None);
        std::fs::create_dir_all(identity::roles_dir(&repo)).unwrap();
        assert_eq!(project_dir(&deep), Some(identity::roles_dir(&repo)));
    }

    #[test]
    fn the_built_in_roles_have_their_stem_as_name_one_line_of_description_and_step_zero() {
        for (stem, text) in BUILT_IN {
            assert!(valid_name(stem), "{stem}");
            let front = text
                .strip_prefix("---\n")
                .unwrap()
                .split("\n---\n")
                .next()
                .unwrap();
            assert!(
                front.lines().any(|l| l == format!("name: {stem}")),
                "{stem}: name"
            );
            let (description, body) = parse(text);
            assert!(!description.is_empty(), "{stem}: description");
            assert!(body.contains("ponytail:ponytail"), "{stem}: step 0");
            assert!(
                !text.contains(identity::get().name.as_str()),
                "{stem}: the binary is `{{bin}}`"
            );
        }
    }
}
