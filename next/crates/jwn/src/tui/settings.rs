//! The look the person chose for the terminal client: a mode (system, dark, light) and a
//! theme for each kind, in `settings.json` under the config directory (spec REQ-151 to
//! REQ-154). Nothing
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

/// What the person chose: the mode, and the theme each kind uses. A dark theme is used while
/// the mode is dark, a light one while it is light, so changing the mode brings back the theme
/// last picked for it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    pub mode: Mode,
    /// Ids of `contract::themes`: a dark one and a light one.
    pub dark: String,
    pub light: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            mode: Mode::System,
            dark: themes::DEFAULT_DARK.to_owned(),
            light: themes::DEFAULT_LIGHT.to_owned(),
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
            dark: kept(&doc["dark"], true),
            light: kept(&doc["light"], false),
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
        let doc = serde_json::json!({
            "mode": self.mode.id(),
            "dark": self.dark,
            "light": self.light,
        });
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

    /// The theme in force: the dark choice while it is dark, the light one while it is light.
    pub fn theme(&self, os_dark: bool) -> &'static themes::Theme {
        let dark = self.is_dark(os_dark);
        themes::get(if dark { &self.dark } else { &self.light }, dark)
    }

    pub fn palette(&self, os_dark: bool) -> themes::Palette {
        self.theme(os_dark).palette
    }

    /// Use `id` for the kind in force.
    pub fn choose(&mut self, os_dark: bool, id: &str) {
        let slot = if self.is_dark(os_dark) {
            &mut self.dark
        } else {
            &mut self.light
        };
        *slot = id.to_owned();
    }
}

/// The id in `value` when it names a theme of that kind, else the kind's default.
fn kept(value: &serde_json::Value, dark: bool) -> String {
    let default = if dark {
        themes::DEFAULT_DARK
    } else {
        themes::DEFAULT_LIGHT
    };
    let id = value.as_str().unwrap_or(default);
    themes::get(id, dark).id.clone()
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
    fn each_value_falls_back_alone_and_a_theme_must_be_of_its_kind() {
        let s = Settings::parse(r#"{"mode": "light", "dark": "nope", "light": "gruvbox-light"}"#);
        assert_eq!(s.mode, Mode::Light);
        assert_eq!(
            (s.dark.as_str(), s.light.as_str()),
            ("one-dark", "gruvbox-light")
        );
        let s =
            Settings::parse(r#"{"mode": "sepia", "dark": "tokyo-night", "light": "tokyo-night"}"#);
        assert_eq!(s.mode, Mode::System);
        assert_eq!(
            (s.dark.as_str(), s.light.as_str()),
            ("tokyo-night", "one-light"),
            "a dark theme is not a light choice"
        );
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = std::env::temp_dir().join(format!("jwn-settings-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let s = Settings {
            mode: Mode::Dark,
            dark: "tokyo-night".into(),
            light: "tokyo-day".into(),
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
    fn the_theme_in_force_follows_the_kind_and_each_kind_keeps_its_choice() {
        let mut s = Settings::default();
        s.choose(true, "gruvbox-dark");
        assert_eq!(s.theme(true).id, "gruvbox-dark");
        assert_eq!(
            s.theme(false).id,
            "one-light",
            "the light choice was not touched"
        );
        s.mode = Mode::Light;
        s.choose(true, "solarized-light");
        assert_eq!(
            (s.dark.as_str(), s.light.as_str()),
            ("gruvbox-dark", "solarized-light")
        );
        s.mode = Mode::Dark;
        assert_eq!(s.theme(false).id, "gruvbox-dark", "back to the dark one");
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
