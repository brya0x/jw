//! The palettes jw draws with (REQ-43): Atom One Dark and One Light built
//! in, and any theme the user keeps as JSON in `themes/` of jw's config dir
//! (addendum 3). Settings name one theme for dark mode and one for light
//! mode; everything that draws asks [`p`] for colours, so a switch of the
//! system's appearance or of a theme file repaints the next frame.

use std::path::PathBuf;
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

pub static ONE_DARK: Palette = Palette {
    dark: true,
    bg: rgb(0x282c34),
    panel: rgb(0x21252b),
    fg: rgb(0xabb2bf),
    dim: rgb(0x5c6370),
    line: rgb(0x3b4048),
    sel: rgb(0x2c313a),
    blue: rgb(0x61afef),
    green: rgb(0x98c379),
    yellow: rgb(0xe5c07b),
    red: rgb(0xe06c75),
    magenta: rgb(0xc678dd),
    cyan: rgb(0x56b6c2),
    // green and red at 13% (lines) and 34% (words) over bg.
    add_bg: rgb(0x37403d),
    del_bg: rgb(0x40343c),
    add_word: rgb(0x4e5f4b),
    del_word: rgb(0x67424a),
};

pub static ONE_LIGHT: Palette = Palette {
    dark: false,
    bg: rgb(0xfafafa),
    panel: rgb(0xf0f0f1),
    fg: rgb(0x383a42),
    dim: rgb(0xa0a1a7),
    line: rgb(0xd4d4d6),
    sel: rgb(0xe5e5e6),
    blue: rgb(0x4078f2),
    green: rgb(0x50a14f),
    yellow: rgb(0xc18401),
    red: rgb(0xe45649),
    magenta: rgb(0xa626a4),
    cyan: rgb(0x0184bc),
    add_bg: rgb(0xe4eee4),
    del_bg: rgb(0xf7e5e3),
    add_word: rgb(0xc0dcc0),
    del_word: rgb(0xf3c2be),
};

static DARK: AtomicBool = AtomicBool::new(true);
/// The themes for dark and light mode; null is the built-in one.
static DARK_P: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());
static LIGHT_P: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());

/// The palette in use.
pub fn p() -> &'static Palette {
    let (slot, builtin) = if DARK.load(Ordering::Relaxed) {
        (&DARK_P, &ONE_DARK)
    } else {
        (&LIGHT_P, &ONE_LIGHT)
    };
    let ptr = slot.load(Ordering::Acquire);
    if ptr.is_null() {
        builtin
    } else {
        // SAFETY: only `install` stores here, a leaked Box that is never
        // freed: a reload leaks the old palette (a few dozen bytes).
        unsafe { &*ptr }
    }
}

fn install(slot: &AtomicPtr<Palette>, pal: Palette) {
    slot.store(Box::into_raw(Box::new(pal)), Ordering::Release);
}

/// Where a user theme lives.
pub fn file(name: &str) -> Result<PathBuf> {
    Ok(crate::core::config::dir()?
        .join("themes")
        .join(format!("{name}.json")))
}

/// The palette of theme `name`: a built-in one or a JSON file.
pub fn named(name: &str) -> Result<Palette> {
    match name {
        "one-dark" => Ok(ONE_DARK),
        "one-light" => Ok(ONE_LIGHT),
        _ => {
            let path = file(name)?;
            let data = std::fs::read(&path)
                .with_context(|| format!("theme {name}: {}", path.display()))?;
            parse(&data).with_context(|| format!("theme {name}: {}", path.display()))
        }
    }
}

/// Every theme: the built-in ones, then the files, by name.
pub fn names() -> Vec<String> {
    let mut out = vec!["one-dark".to_string(), "one-light".to_string()];
    if let Ok(dir) = file("x").map(|p| p.with_file_name(""))
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        let mut files: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .filter(|n| n != "one-dark" && n != "one-light")
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

/// Uses the themes the settings name (REQ-69). A theme that doesn't load
/// keeps its slot's last palette, and the error says which.
pub fn load(s: &crate::settings::Settings) -> Result<()> {
    let mut errors = Vec::new();
    for (slot, name) in [(&DARK_P, s.dark_theme()), (&LIGHT_P, s.light_theme())] {
        match named(name) {
            Ok(pal) => install(slot, pal),
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

/// Theme `from` written as `themes/<to>.json`, to edit (`c` in settings).
pub fn copy(from: &str, to: &str) -> Result<PathBuf> {
    let p = named(from)?;
    let path = file(to)?;
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let t = ThemeFile {
        name: to.to_string(),
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
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&t)?)?;
    Ok(path)
}

/// What panes sit on, for the daemon to answer the programs in them that
/// ask (S14).
pub fn wire() -> crate::proto::Theme {
    let pal = p();
    let builtin = if pal.dark { &ONE_DARK } else { &ONE_LIGHT };
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
        // ONE_DARK's diff colours are the same mix, written out.
        let p = ONE_DARK;
        let near = |a: Color, b: Color| match (a, b) {
            (Color::Rgb(a, b, c), Color::Rgb(x, y, z)) => {
                a.abs_diff(x) <= 2 && b.abs_diff(y) <= 2 && c.abs_diff(z) <= 2
            }
            _ => false,
        };
        assert!(near(mix(p.green, p.bg, 0.13), p.add_bg));
        assert!(near(mix(p.red, p.bg, 0.34), p.del_word));
    }
}
