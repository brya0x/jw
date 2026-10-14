//! The look the person chose for the terminal client: a mode (system, dark, light) and a
//! theme, in `settings.json` under the config directory (spec REQ-151 to REQ-154). Nothing
//! here fails: a missing or unreadable file, or a value it does not know, is the default for
//! that value.

use std::path::{Path, PathBuf};
use std::process::Command;

use contract::themes;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    System,
    Dark,
    Light,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::System, Mode::Dark, Mode::Light];

    pub fn id(self) -> &'static str {
        match self {
            Mode::System => "system",
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    /// What the screen says.
    pub fn label(self) -> &'static str {
        match self {
            Mode::System => "sistema",
            Mode::Dark => "oscuro",
            Mode::Light => "claro",
        }
    }

    /// The next mode, or the one before it.
    pub fn step(self, forward: bool) -> Mode {
        let at = Mode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Mode::ALL[(at + if forward { 1 } else { Mode::ALL.len() - 1 }) % Mode::ALL.len()]
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    pub mode: Mode,
    /// The id of one of `contract::themes`.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            mode: Mode::System,
            theme: themes::DEFAULT.to_owned(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        identity::config_dir().join("settings.json")
    }

    /// The file's settings; each value the file lacks or does not understand is the default.
    pub fn load() -> Settings {
        Settings::load_from(&Settings::path())
    }

    fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path).map_or_else(|_| Settings::default(), |t| Settings::parse(&t))
    }

    pub fn parse(text: &str) -> Settings {
        let doc: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        Settings {
            mode: Mode::ALL
                .into_iter()
                .find(|m| doc["mode"].as_str() == Some(m.id()))
                .unwrap_or_default(),
            theme: doc["theme"]
                .as_str()
                .and_then(themes::find)
                .map_or_else(|| themes::DEFAULT.to_owned(), |t| t.id.clone()),
        }
    }

    /// Write the file whole, through a rename, so a reader never sees half of it.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Settings::path())
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let doc = serde_json::json!({ "mode": self.mode.id(), "theme": self.theme });
        std::fs::write(&tmp, serde_json::to_string_pretty(&doc)? + "\n")?;
        std::fs::rename(&tmp, path)
    }

    /// Dark or light now: what `System` follows.
    pub fn is_dark(&self, os_dark: bool) -> bool {
        match self.mode {
            Mode::System => os_dark,
            Mode::Dark => true,
            Mode::Light => false,
        }
    }

    pub fn palette(&self, os_dark: bool) -> contract::themes::Palette {
        themes::get(&self.theme).palette(self.is_dark(os_dark))
    }
}

/// Whether the machine is in dark mode: dark where it cannot tell (macOS prints `Dark` when
/// it is, and fails when it is light; nothing is asked on other systems).
pub fn os_is_dark() -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map_or(true, |o| {
            !o.status.success() || String::from_utf8_lossy(&o.stdout).trim() == "Dark"
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_broken_file_is_the_defaults() {
        for text in ["", "nope", "[]", "{}", r#"{"mode": 3, "theme": null}"#] {
            assert_eq!(Settings::parse(text), Settings::default(), "{text}");
        }
    }

    #[test]
    fn each_value_falls_back_alone() {
        let s = Settings::parse(r#"{"mode": "light", "theme": "nope"}"#);
        assert_eq!((s.mode, s.theme.as_str()), (Mode::Light, "one"));
        let s = Settings::parse(r#"{"mode": "sepia", "theme": "gruvbox"}"#);
        assert_eq!((s.mode, s.theme.as_str()), (Mode::System, "gruvbox"));
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = std::env::temp_dir().join(format!("jwn-settings-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let s = Settings {
            mode: Mode::Dark,
            theme: "tokyo-night".into(),
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the rename left nothing"
        );
        assert_eq!(
            Settings::load_from(&dir.join("none.json")),
            Settings::default()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_mode_steps_both_ways_round() {
        assert_eq!(Mode::System.step(true), Mode::Dark);
        assert_eq!(Mode::Light.step(true), Mode::System);
        assert_eq!(Mode::System.step(false), Mode::Light);
    }

    #[test]
    fn system_follows_the_machine_and_the_others_do_not() {
        let mut s = Settings::default();
        assert!(s.is_dark(true) && !s.is_dark(false));
        s.mode = Mode::Light;
        assert!(!s.is_dark(true));
        s.mode = Mode::Dark;
        assert!(s.is_dark(false));
    }
}
