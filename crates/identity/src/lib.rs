//! The product identity, read from `identity.toml` at the root. Nothing else in the
//! workspace spells a product name, env prefix, directory or socket by hand.

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
}

const SOURCE: &str = include_str!("../../../identity.toml");

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

/// A workspace's task list: `<folder>/.<name>/tasks.md` (N180, REQ-892). The directory is the product's
/// name with a dot, so nothing else spells it.
pub fn tasks_path(folder: &Path) -> PathBuf {
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
    fn the_task_list_is_in_the_products_dot_directory() {
        let path = tasks_path(Path::new("/w"));
        assert_eq!(path, PathBuf::from(format!("/w/.{}/tasks.md", get().name)));
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
