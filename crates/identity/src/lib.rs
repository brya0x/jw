//! The product identity, read from `identity.toml` at the root (or, for a test install, from the
//! file `IDENTITY_FILE` names when building: `build.rs`). Nothing else in the workspace spells a
//! product name, env prefix, directory or socket by hand.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Identity {
    pub name: String,
    pub env_prefix: String,
    pub label: String,
    pub android_application_id: String,
    pub tunnel_program: String,
    pub pairing_store: String,
    pub repo: String,
    pub agent_skill: String,
    /// A test install (N190): `update` and `android install` refuse. Absent means `false`.
    #[serde(default)]
    pub test: bool,
}

const SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/identity.toml"));

pub fn get() -> &'static Identity {
    static ID: OnceLock<Identity> = OnceLock::new();
    ID.get_or_init(|| toml::from_str(SOURCE).expect("identity.toml is valid"))
}

/// `<PREFIX>_<suffix>` for the current identity.
pub fn env_var(suffix: &str) -> String {
    format!("{}_{suffix}", get().env_prefix)
}

/// What a client calls itself on the wire: `<name>-<role>`.
pub fn peer(role: &str) -> String {
    format!("{}-{role}", get().name)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(&get().name)
}

/// `$<PREFIX>_STATE_DIR`, else `$XDG_STATE_HOME/<name>`, else `~/.local/state/<name>`.
pub fn state_dir() -> PathBuf {
    match std::env::var_os(env_var("STATE_DIR")) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => xdg("XDG_STATE_HOME", ".local/state").join(&get().name),
    }
}

/// Where the conversations of their own have their folders: `<state>/chats` (N173, REQ-806).
/// The state directory is the caller's, as the daemon's is (a test gives it its own).
pub fn chats_dir(state: &Path) -> PathBuf {
    state.join("chats")
}

/// Where the task lists are: `<state>/tasks` (N188, REQ-1061).
pub fn tasks_dir(state: &Path) -> PathBuf {
    state.join("tasks")
}

/// A workspace's task list: `<state>/tasks/<name>-<hash>.md` (N188, REQ-1061, REQ-892). The hash is the
/// FNV-1a of the folder's canonical path, written here because `DefaultHasher` is not promised to be the
/// same between releases; a folder that is not there is canonical from its nearest ancestor that is, so
/// it has the name it had. The name is the folder's last name with only letters, digits, `-` and `_`.
pub fn tasks_path(state: &Path, folder: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut base = folder;
    let canonical = loop {
        match std::fs::canonicalize(base) {
            Ok(c) => break c,
            Err(_) => match (base.parent(), base.file_name()) {
                (Some(parent), Some(last)) => {
                    rest.push(last);
                    base = parent;
                }
                _ => break base.to_path_buf(),
            },
        }
    };
    let canonical = rest.iter().rev().fold(canonical, |p, last| p.join(last));
    let hash = canonical
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
    let name: String = canonical
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let name = if name.is_empty() { "workspace" } else { &name };
    tasks_dir(state).join(format!("{name}-{hash:016x}.md"))
}

/// Where N180 kept the task list, `<folder>/.<name>/tasks.md`: the place a list is moved from (N188, REQ-1063).
/// The directory is the product's name with a dot, so nothing else spells it.
pub fn legacy_tasks_path(folder: &Path) -> PathBuf {
    folder.join(format!(".{}", get().name)).join("tasks.md")
}

/// A repository's own roles: `<folder>/.<name>/roles` (N184, REQ-1000).
pub fn roles_dir(folder: &Path) -> PathBuf {
    folder.join(format!(".{}", get().name)).join("roles")
}

/// The person's own roles: `<config dir>/roles` (N184, REQ-1000).
pub fn user_roles_dir() -> PathBuf {
    config_dir().join("roles")
}

/// The entries the person adds to the skillset: `<config dir>/skillset.toml` (N184b, REQ-1017).
pub fn skillset_file() -> PathBuf {
    config_dir().join("skillset.toml")
}

/// What the skillset installed from outside: `<state dir>/skillset.lock` (N184b, REQ-1024).
pub fn skillset_lock() -> PathBuf {
    state_dir().join("skillset.lock")
}

/// The daemon's socket: `$<PREFIX>_SOCKET`, else `<state_dir>/<name>.sock`.
pub fn socket_path() -> PathBuf {
    match std::env::var_os(env_var("SOCKET")) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => state_dir().join(format!("{}.sock", get().name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_parses_and_derives_names() {
        assert_eq!(env_var("SOCKET"), format!("{}_SOCKET", get().env_prefix));
        assert_eq!(env_var("NAME"), format!("{}_NAME", get().env_prefix));
        match std::env::var_os(env_var("STATE_DIR")) {
            Some(dir) if !dir.is_empty() => assert_eq!(state_dir(), PathBuf::from(dir)),
            _ => assert!(state_dir().ends_with(&get().name)),
        }
        assert!(config_dir().ends_with(&get().name));
        assert_eq!(chats_dir(Path::new("/s")), PathBuf::from("/s/chats"));
    }

    #[test]
    fn the_test_identity_is_the_products_plus_test() {
        let product: toml::Table = include_str!("../../../identity.toml").parse().unwrap();
        let test: toml::Table = include_str!("../../../identity.test.toml").parse().unwrap();
        let mut keys: Vec<_> = product
            .keys()
            .cloned()
            .chain(["test".to_string()])
            .collect();
        keys.sort();
        let mut test_keys: Vec<_> = test.keys().cloned().collect();
        test_keys.sort();
        assert_eq!(test_keys, keys);
        for k in [
            "name",
            "env_prefix",
            "agent_skill",
            "android_application_id",
        ] {
            assert_ne!(product[k], test[k], "{k} is the product's");
        }
        for k in ["repo", "tunnel_program", "pairing_store"] {
            assert_eq!(product[k], test[k], "{k}");
        }
        assert_eq!(test["test"], toml::Value::Boolean(true));
        assert!(!product.contains_key("test"));
    }

    #[test]
    fn the_test_identity_parses_and_the_product_is_not_a_test_install() {
        let test: Identity = toml::from_str(include_str!("../../../identity.test.toml")).unwrap();
        assert!(test.test && test.name == "jwtest" && test.env_prefix == "JWTEST");
        assert!(!get().test);
    }

    #[test]
    fn the_repository_is_an_https_url_with_no_credentials() {
        let repo = &get().repo;
        let rest = repo.strip_prefix("https://").expect("https");
        let host = rest.split('/').next().unwrap();
        assert!(!host.is_empty() && !host.contains('@'), "{repo}");
        assert!(
            !repo.ends_with('/') && !repo.contains(char::is_whitespace),
            "{repo}"
        );
    }

    #[test]
    fn the_task_list_was_in_the_products_dot_directory() {
        let path = legacy_tasks_path(Path::new("/w"));
        assert_eq!(path, PathBuf::from(format!("/w/.{}/tasks.md", get().name)));
    }

    /// REQ-1061: a name that never changes between releases, and one file for one folder.
    #[test]
    fn the_task_list_of_a_folder_is_one_file_that_never_changes_its_name() {
        let state = Path::new("/s");
        // the FNV-1a of `/no/such/dir/app`, pinned: a change of the hash would lose every list
        assert_eq!(
            tasks_path(state, Path::new("/no/such/dir/app")),
            PathBuf::from("/s/tasks/app-abceb34ba314045d.md")
        );
        // a relative path that is nowhere keeps all of its names, once
        assert_eq!(
            tasks_path(state, Path::new("rel-gone-xyz/x")),
            PathBuf::from("/s/tasks/x-4ddb06c915fbc265.md")
        );
        let a = tasks_path(state, Path::new("/one/app"));
        let b = tasks_path(state, Path::new("/two/app"));
        assert_ne!(a, b, "two folders with one name are two files");
        assert!(a.starts_with(tasks_dir(state)));
    }

    /// REQ-1061: the name has no strange characters, and a folder with none, or a path with spaces, has a file.
    #[test]
    fn the_name_in_the_file_has_only_safe_characters() {
        let state = Path::new("/s");
        let name = |p: &str| {
            tasks_path(state, Path::new(p))
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert!(
            name("/no/my app (1)/.hidden é").starts_with("_hidden__-"),
            "{}",
            name("/no/my app (1)/.hidden é")
        );
        assert!(name("/").starts_with("workspace-"));
        let long = name(&format!("/no/{}", "a".repeat(100)));
        assert_eq!(long.len(), 40 + 1 + 16 + 3, "{long}");
        for n in [name("/no/my app (1)/x y"), long] {
            assert!(
                n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                "{n}"
            );
        }
    }

    /// REQ-1061: a link and the folder it points at are one list; a folder that is not there keeps its name.
    #[test]
    fn a_link_to_a_folder_and_a_folder_that_is_gone_have_the_file_they_had() {
        let root = std::env::temp_dir().join(format!("jw-ident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join("link")).unwrap();
        let state = Path::new("/s");
        let by_real = tasks_path(state, &real);
        assert_eq!(
            by_real.file_name(),
            tasks_path(state, &real.join("../real")).file_name()
        );
        assert_ne!(
            tasks_path(state, &root.join("link"))
                .file_name()
                .unwrap()
                .to_string_lossy()
                .split('-')
                .next(),
            Some("link"),
            "the name is the real folder's"
        );
        assert_eq!(tasks_path(state, &root.join("link")), by_real);
        let sub = real.join("deep/er");
        let before = tasks_path(state, &sub);
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            tasks_path(state, &sub),
            before,
            "the folder came to be: the same file"
        );
        std::fs::remove_dir_all(&sub).unwrap();
        assert_eq!(tasks_path(state, &sub), before, "and went: the same file");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_roles_are_in_the_products_directories_and_the_skill_has_a_name() {
        let name = &get().name;
        assert_eq!(
            roles_dir(Path::new("/w")),
            PathBuf::from(format!("/w/.{name}/roles"))
        );
        assert_eq!(user_roles_dir(), config_dir().join("roles"));
        assert_eq!(skillset_file(), config_dir().join("skillset.toml"));
        assert_eq!(skillset_lock(), state_dir().join("skillset.lock"));
        let skill = &get().agent_skill;
        assert!(
            !skill.is_empty()
                && skill.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                && skill != "agents",
            "{skill}"
        );
    }

    /// REQ-892: no source spells the directory or the file; they come from `tasks_path`.
    #[test]
    fn no_source_spells_the_task_list_directory_or_file() {
        fn walk(dir: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("identity/src/lib.rs")
                {
                    for (n, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                        let code = line.trim_start();
                        let spelled = format!(".{}/", get().name);
                        if !code.starts_with("//")
                            && (code.contains(&spelled) || code.contains("tasks.md"))
                        {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let mut found = Vec::new();
        for krate in std::fs::read_dir(&crates).unwrap().flatten() {
            for sub in ["src", "tests"] {
                if krate.path().join(sub).is_dir() {
                    walk(&krate.path().join(sub), &mut found);
                }
            }
        }
        assert!(
            found.is_empty(),
            "spelled by hand, use identity::tasks_path: {found:?}"
        );
    }
}
