//! The palettes jw draws with (REQ-43). Every theme is JSON in one format
//! (`themes/schema.json`): the built-ins are the files in `crates/tui/themes/`,
//! compiled in, and a user's are in `themes/` of jw's config dir, where a
//! file named like a built-in replaces it (S16). Settings name one theme for
//! dark mode and one for light mode; everything that draws asks [`p`] for
//! colours, so a switch of the system's appearance or of a theme file
//! repaints the next frame.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use anyhow::{Context, Result, bail};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color,
    /// The sidebar, the status bar, modals.
    pub panel: Color,
    pub fg: Color,
    pub dim: Color,
    pub line: Color,
    /// A selected row.
    pub sel: Color,
    pub blue: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub magenta: Color,
    pub cyan: Color,
    /// Diff lines and the words that changed in them.
    pub add_bg: Color,
    pub del_bg: Color,
    pub add_word: Color,
    pub del_word: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The built-in themes, in the order settings list them (REQ-101).
const BUILTIN: &[(&str, &str)] = &[
    ("one-dark", include_str!("../themes/one-dark.json")),
    ("one-light", include_str!("../themes/one-light.json")),
    (
        "catppuccin-mocha",
        include_str!("../themes/catppuccin-mocha.json"),
    ),
    (
        "catppuccin-latte",
        include_str!("../themes/catppuccin-latte.json"),
    ),
    ("tokyo-night", include_str!("../themes/tokyo-night.json")),
    (
        "tokyo-night-day",
        include_str!("../themes/tokyo-night-day.json"),
    ),
    ("gruvbox-dark", include_str!("../themes/gruvbox-dark.json")),
    (
        "gruvbox-light",
        include_str!("../themes/gruvbox-light.json"),
    ),
    (
        "solarized-dark",
        include_str!("../themes/solarized-dark.json"),
    ),
    (
        "solarized-light",
        include_str!("../themes/solarized-light.json"),
    ),
];

/// The JSON Schema of a theme file, and where it is published.
pub const SCHEMA: &str = include_str!("../themes/schema.json");
pub const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/brya0x/jw/main/crates/tui/themes/schema.json";

/// The built-in theme `name`. The files are checked by a test, so a broken
/// one never ships.
fn builtin(name: &str) -> Option<&'static Palette> {
    static PARSED: OnceLock<Vec<Palette>> = OnceLock::new();
    let all = PARSED.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|(n, json)| {
                parse(json.as_bytes()).unwrap_or_else(|e| panic!("built-in theme {n}: {e:#}"))
            })
            .collect()
    });
    BUILTIN
        .iter()
        .position(|(n, _)| *n == name)
        .map(|i| &all[i])
}

/// One Dark or One Light: what draws before the settings load.
fn fallback(dark: bool) -> &'static Palette {
    builtin(if dark { "one-dark" } else { "one-light" })
        .expect("one-dark and one-light are built in")
}

static DARK: AtomicBool = AtomicBool::new(true);
/// The themes for dark and light mode; null is the built-in one.
static DARK_P: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());
static LIGHT_P: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());

/// The palette in use.
pub fn p() -> &'static Palette {
    let dark = DARK.load(Ordering::Relaxed);
    let slot = if dark { &DARK_P } else { &LIGHT_P };
    let ptr = slot.load(Ordering::Acquire);
    if ptr.is_null() {
        fallback(dark)
    } else {
        // SAFETY: only `install` stores here, a leaked Box that is never
        // freed: a reload leaks the old palette (a few dozen bytes).
        unsafe { &*ptr }
    }
}

fn put(slot: &AtomicPtr<Palette>, pal: Palette) {
    slot.store(Box::into_raw(Box::new(pal)), Ordering::Release);
}

/// Where the user's themes live: `themes/` in jw's config dir.
fn user_dir() -> Result<PathBuf> {
    Ok(crate::core::config::dir()?.join("themes"))
}

/// Where a user theme lives.
pub fn file(name: &str) -> Result<PathBuf> {
    Ok(user_dir()?.join(format!("{name}.json")))
}

/// The palette of theme `name`: the user's file, else the built-in one
/// (REQ-103).
pub fn named(name: &str) -> Result<Palette> {
    named_in(user_dir().ok().as_deref(), name)
}

fn named_in(dir: Option<&Path>, name: &str) -> Result<Palette> {
    let path = dir
        .map(|d| d.join(format!("{name}.json")))
        .filter(|p| p.exists());
    match (path, builtin(name)) {
        (Some(path), _) => {
            let data = std::fs::read(&path)
                .with_context(|| format!("theme {name}: {}", path.display()))?;
            parse(&data).with_context(|| format!("theme {name}: {}", path.display()))
        }
        (None, Some(b)) => Ok(*b),
        (None, None) => bail!("no theme {name}"),
    }
}

/// Every theme, each once: the built-ins, then the user's files by name.
pub fn names() -> Vec<String> {
    names_in(user_dir().ok().as_deref())
}

fn names_in(dir: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    if let Some(rd) = dir.and_then(|d| std::fs::read_dir(d).ok()) {
        let mut files: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .filter(|n| builtin(n).is_none())
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

/// Where theme `name` comes from, as `jw theme ls` says it (REQ-108).
pub fn source(name: &str) -> &'static str {
    let file = file(name).is_ok_and(|p| p.exists());
    match (file, builtin(name).is_some()) {
        (true, true) => "file, replaces built-in",
        (true, false) => "file",
        _ => "built-in",
    }
}

/// Uses the themes the settings name (REQ-69). A theme that doesn't load
/// keeps its slot's last palette, and the error says which.
pub fn load(s: &crate::settings::Settings) -> Result<()> {
    let mut errors = Vec::new();
    for (slot, name) in [(&DARK_P, s.dark_theme()), (&LIGHT_P, s.light_theme())] {
        match named(name) {
            Ok(pal) => put(slot, pal),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

/// A theme file: the colours as `#rrggbb`. The four diff colours may be
/// left out: they are mixed from green and red over `bg`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    /// The schema's URL, for editors; jw doesn't read it.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    #[serde(default)]
    name: String,
    dark: bool,
    colors: Colors,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Colors {
    bg: String,
    panel: String,
    line: String,
    fg: String,
    dim: String,
    sel: String,
    blue: String,
    green: String,
    yellow: String,
    red: String,
    magenta: String,
    cyan: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    add_bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    del_bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    add_word: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    del_word: Option<String>,
}

fn hex(s: &str) -> Result<Color> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 {
        bail!("{s:?} is not #rrggbb");
    }
    let n = u32::from_str_radix(h, 16).with_context(|| format!("{s:?} is not #rrggbb"))?;
    Ok(rgb(n))
}

fn to_hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#000000".into(),
    }
}

/// `fg` over `bg` at `t` (0..1).
fn mix(fg: Color, bg: Color, t: f32) -> Color {
    let (Color::Rgb(a, b, c), Color::Rgb(x, y, z)) = (fg, bg) else {
        return bg;
    };
    let m = |f: u8, b: u8| (f32::from(b) + (f32::from(f) - f32::from(b)) * t).round() as u8;
    Color::Rgb(m(a, x), m(b, y), m(c, z))
}

fn parse(data: &[u8]) -> Result<Palette> {
    let t: ThemeFile = serde_json::from_slice(data)?;
    let c = &t.colors;
    let (bg, green, red) = (hex(&c.bg)?, hex(&c.green)?, hex(&c.red)?);
    let or = |v: &Option<String>, d: Color| v.as_deref().map_or(Ok(d), hex);
    Ok(Palette {
        dark: t.dark,
        bg,
        panel: hex(&c.panel)?,
        fg: hex(&c.fg)?,
        dim: hex(&c.dim)?,
        line: hex(&c.line)?,
        sel: hex(&c.sel)?,
        blue: hex(&c.blue)?,
        green,
        yellow: hex(&c.yellow)?,
        red,
        magenta: hex(&c.magenta)?,
        cyan: hex(&c.cyan)?,
        add_bg: or(&c.add_bg, mix(green, bg, 0.13))?,
        del_bg: or(&c.del_bg, mix(red, bg, 0.13))?,
        add_word: or(&c.add_word, mix(green, bg, 0.34))?,
        del_word: or(&c.del_word, mix(red, bg, 0.34))?,
    })
}

/// Palette `p` as a theme file named `name`, every colour written out.
fn to_file(name: &str, p: &Palette) -> ThemeFile {
    ThemeFile {
        schema: Some(SCHEMA_URL.to_string()),
        name: name.to_string(),
        dark: p.dark,
        colors: Colors {
            bg: to_hex(p.bg),
            panel: to_hex(p.panel),
            line: to_hex(p.line),
            fg: to_hex(p.fg),
            dim: to_hex(p.dim),
            sel: to_hex(p.sel),
            blue: to_hex(p.blue),
            green: to_hex(p.green),
            yellow: to_hex(p.yellow),
            red: to_hex(p.red),
            magenta: to_hex(p.magenta),
            cyan: to_hex(p.cyan),
            add_bg: Some(to_hex(p.add_bg)),
            del_bg: Some(to_hex(p.del_bg)),
            add_word: Some(to_hex(p.add_word)),
            del_word: Some(to_hex(p.del_word)),
        },
    }
}

/// Theme `name` as JSON that `install` takes back (REQ-106).
pub fn export(name: &str) -> Result<String> {
    let mut json = serde_json::to_string_pretty(&to_file(name, &named(name)?))?;
    json.push('\n');
    Ok(json)
}

/// Theme `from` written as `themes/<to>.json`, to edit (`c` in settings).
pub fn copy(from: &str, to: &str) -> Result<PathBuf> {
    let json = serde_json::to_string_pretty(&to_file(to, &named(from)?))? + "\n";
    install_in(&user_dir()?, json.as_bytes(), Some(to), None, false)
}

/// Adds a theme to the user's themes (REQ-105). Its name is `name`, else the
/// one in the JSON, else `stem` (the file it came from). Nothing is written
/// unless it parses, and an existing theme is kept unless `force`.
pub fn install(
    data: &[u8],
    name: Option<&str>,
    stem: Option<&str>,
    force: bool,
) -> Result<PathBuf> {
    install_in(&user_dir()?, data, name, stem, force)
}

fn install_in(
    dir: &Path,
    data: &[u8],
    name: Option<&str>,
    stem: Option<&str>,
    force: bool,
) -> Result<PathBuf> {
    parse(data).context("not a jw theme")?;
    let inner: ThemeFile = serde_json::from_slice(data)?;
    let name = name
        .or((!inner.name.is_empty()).then_some(inner.name.as_str()))
        .or(stem)
        .context("the theme has no name: give it one with --name")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("{name:?} is not a theme name: lowercase letters, digits and -");
    }
    let path = dir.join(format!("{name}.json"));
    if path.exists() && !force {
        bail!("{} already exists (--force replaces it)", path.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.json.tmp"));
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// What panes sit on, for the daemon to answer the programs in them that
/// ask (S14).
pub fn wire() -> crate::proto::Theme {
    wire_of(p())
}

fn wire_of(pal: &Palette) -> crate::proto::Theme {
    let builtin = fallback(pal.dark);
    let rgb = |c: Color, or: Color| match (c, or) {
        (Color::Rgb(r, g, b), _) | (_, Color::Rgb(r, g, b)) => [r, g, b],
        _ => [0, 0, 0],
    };
    crate::proto::Theme {
        dark: pal.dark,
        fg: rgb(pal.fg, builtin.fg),
        bg: rgb(pal.bg, builtin.bg),
    }
}

/// Switches palettes; true when that changed anything.
pub fn set_dark(dark: bool) -> bool {
    DARK.swap(dark, Ordering::Relaxed) != dark
}

/// Dark or light when `$JW_THEME` or the settings pin one; `None`
/// follows the system.
pub fn pinned() -> Option<bool> {
    let mode = std::env::var("JW_THEME")
        .ok()
        .filter(|m| m == "dark" || m == "light")
        .unwrap_or_else(|| crate::settings::get().theme_mode().to_string());
    match mode.trim() {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

/// Whether the system is in dark mode. macOS answers through
/// `AppleInterfaceStyle` (only set when dark); elsewhere jw stays dark.
pub fn system_dark() -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "Dark")
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO: &str = r##"{"name": "tokyo", "dark": true, "colors": {
        "bg": "#1a1b26", "panel": "#16161e", "line": "#292e42", "fg": "#c0caf5",
        "dim": "#565f89", "sel": "#283457", "blue": "#7aa2f7", "green": "#9ece6a",
        "yellow": "#e0af68", "red": "#f7768e", "magenta": "#bb9af7", "cyan": "#7dcfff"}}"##;

    #[test]
    fn a_theme_file_without_diff_colours_mixes_them() {
        let p = parse(TOKYO.as_bytes()).unwrap();
        assert!(p.dark);
        assert_eq!(p.bg, rgb(0x1a1b26));
        assert_eq!(p.add_bg, mix(rgb(0x9ece6a), rgb(0x1a1b26), 0.13));
        assert_ne!(p.add_bg, p.bg);
        let bad = TOKYO.replace("#1a1b26", "blue");
        assert!(parse(bad.as_bytes()).is_err());
        assert!(parse(TOKYO.replace("\"bg\"", "\"bgg\"").as_bytes()).is_err());
    }

    #[test]
    fn the_built_in_mix_matches_one_dark() {
        // one-dark's diff colours are the same mix, written out.
        let p = *builtin("one-dark").unwrap();
        let near = |a: Color, b: Color| match (a, b) {
            (Color::Rgb(a, b, c), Color::Rgb(x, y, z)) => {
                a.abs_diff(x) <= 2 && b.abs_diff(y) <= 2 && c.abs_diff(z) <= 2
            }
            _ => false,
        };
        assert!(near(mix(p.green, p.bg, 0.13), p.add_bg));
        assert!(near(mix(p.red, p.bg, 0.34), p.del_word));
    }

    #[test]
    fn every_built_in_parses_under_its_own_name() {
        for (name, json) in BUILTIN {
            let t: ThemeFile = serde_json::from_str(json).unwrap();
            assert_eq!(t.name, *name);
            assert_eq!(t.schema.as_deref(), Some(SCHEMA_URL));
            assert!(builtin(name).is_some(), "{name}");
        }
        let darks = BUILTIN
            .iter()
            .filter(|(n, _)| builtin(n).unwrap().dark)
            .count();
        assert_eq!(darks * 2, BUILTIN.len(), "a light theme for every dark one");
    }

    #[test]
    fn the_schema_and_the_format_agree() {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        // Every field a full export writes, and nothing else, is in the schema.
        let full = serde_json::to_value(to_file("x", builtin("one-dark").unwrap())).unwrap();
        assert_eq!(keys(&schema["properties"]), keys(&full));
        let colors = &schema["properties"]["colors"];
        assert_eq!(keys(&colors["properties"]), keys(&full["colors"]));
        // The required colours are exactly the ones jw can't do without.
        let required: Vec<&str> = colors["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let only = |names: &[&str]| {
            let c: serde_json::Map<_, _> = names
                .iter()
                .map(|n| (n.to_string(), full["colors"][*n].clone()))
                .collect();
            serde_json::json!({"dark": true, "colors": c}).to_string()
        };
        assert!(parse(only(&required).as_bytes()).is_ok());
        for n in &required {
            let fewer: Vec<&str> = required.iter().copied().filter(|r| r != n).collect();
            assert!(parse(only(&fewer).as_bytes()).is_err(), "{n} is required");
        }
    }

    #[test]
    fn a_users_file_replaces_the_built_in_and_is_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        let mine = TOKYO.replace("\"tokyo\"", "\"one-dark\"");
        std::fs::write(dir.path().join("one-dark.json"), &mine).unwrap();
        std::fs::write(dir.path().join("zzz.json"), TOKYO).unwrap();
        assert_eq!(
            named_in(Some(dir.path()), "one-dark").unwrap().bg,
            rgb(0x1a1b26)
        );
        assert_eq!(
            named_in(Some(dir.path()), "one-light").unwrap(),
            *builtin("one-light").unwrap()
        );
        let names = names_in(Some(dir.path()));
        assert_eq!(names.iter().filter(|n| *n == "one-dark").count(), 1);
        assert_eq!(names.last().map(String::as_str), Some("zzz"));
        assert!(named_in(Some(dir.path()), "nope").is_err());
    }

    #[test]
    fn install_takes_an_export_back_and_refuses_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let json = serde_json::to_string_pretty(&to_file(
            "gruvbox-dark",
            builtin("gruvbox-dark").unwrap(),
        ))
        .unwrap();
        let path = install_in(dir.path(), json.as_bytes(), None, None, false).unwrap();
        assert_eq!(path, dir.path().join("gruvbox-dark.json"));
        assert_eq!(
            named_in(Some(dir.path()), "gruvbox-dark").unwrap(),
            *builtin("gruvbox-dark").unwrap()
        );
        // It exists: only --force replaces it.
        assert!(install_in(dir.path(), json.as_bytes(), None, None, false).is_err());
        assert!(install_in(dir.path(), json.as_bytes(), None, None, true).is_ok());
        // --name wins over the JSON's name.
        assert!(install_in(dir.path(), json.as_bytes(), Some("mine"), None, false).is_ok());
        assert!(dir.path().join("mine.json").exists());
        // Not a theme, or a name that would leave the directory.
        assert!(install_in(dir.path(), b"{}", Some("x"), None, false).is_err());
        assert!(install_in(dir.path(), json.as_bytes(), Some("../x"), None, false).is_err());
        assert!(!dir.path().join("x.json").exists());
        // No name anywhere but the file it came from.
        let nameless = TOKYO.replace("\"name\": \"tokyo\", ", "");
        let p = install_in(dir.path(), nameless.as_bytes(), None, Some("night"), false).unwrap();
        assert!(p.ends_with("night.json"));
    }

    #[test]
    fn the_daemons_default_is_one_dark() {
        assert_eq!(
            crate::proto::Theme::default(),
            wire_of(builtin("one-dark").unwrap())
        );
    }
}
