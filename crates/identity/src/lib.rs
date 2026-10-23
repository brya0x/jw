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
        for name in ["NAME", "PROJECT"] {
            assert_eq!(env_var(name), format!("{}_{name}", get().env_prefix));
        }
        match std::env::var_os(env_var("STATE_DIR")) {
            Some(dir) if !dir.is_empty() => assert_eq!(state_dir(), PathBuf::from(dir)),
            _ => assert!(state_dir().ends_with(&get().name)),
        }
        assert!(config_dir().ends_with(&get().name));
        assert_eq!(chats_dir(Path::new("/s")), PathBuf::from("/s/chats"));
    }
}
