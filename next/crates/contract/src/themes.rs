//! The ten palettes the clients draw with (`contract/themes.json`): five themes, each with a
//! dark and a light member. The file is the home of the colours; the terminal client reads it
//! through here and the phone's table is tested against it.

use std::sync::OnceLock;

use serde::Deserialize;

/// A colour as red, green and blue.
pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: Rgb,
    pub panel: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub line: Rgb,
    pub sel: Rgb,
    pub blue: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
    pub red: Rgb,
    pub magenta: Rgb,
    pub cyan: Rgb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: Palette,
    pub light: Palette,
}

impl Theme {
    pub fn palette(&self, dark: bool) -> Palette {
        if dark { self.dark } else { self.light }
    }
}

/// The theme a person who chose nothing gets.
pub const DEFAULT: &str = "one";

const FILE: &str = include_str!("../../../contract/themes.json");

/// Every theme, in the order the file lists them.
pub fn all() -> &'static [Theme] {
    static ALL: OnceLock<Vec<Theme>> = OnceLock::new();
    ALL.get_or_init(|| parse(FILE).expect("contract/themes.json is not a list of themes"))
}

pub fn find(id: &str) -> Option<&'static Theme> {
    all().iter().find(|t| t.id == id)
}

/// The theme named `id`, or the default one.
pub fn get(id: &str) -> &'static Theme {
    find(id).or_else(|| find(DEFAULT)).unwrap_or(&all()[0])
}

#[derive(Deserialize)]
struct RawFile {
    themes: Vec<RawTheme>,
}

#[derive(Deserialize)]
struct RawTheme {
    id: String,
    name: String,
    dark: RawPalette,
    light: RawPalette,
}

#[derive(Deserialize)]
struct RawPalette {
    bg: String,
    panel: String,
    fg: String,
    dim: String,
    line: String,
    sel: String,
    blue: String,
    green: String,
    yellow: String,
    red: String,
    magenta: String,
    cyan: String,
}

fn hex(text: &str) -> Result<Rgb, String> {
    let h = text
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let part = |a: usize| {
        h.and_then(|h| u8::from_str_radix(&h[a..a + 2], 16).ok())
            .ok_or_else(|| format!("not a colour: {text}"))
    };
    Ok((part(0)?, part(2)?, part(4)?))
}

impl RawPalette {
    fn palette(&self) -> Result<Palette, String> {
        Ok(Palette {
            bg: hex(&self.bg)?,
            panel: hex(&self.panel)?,
            fg: hex(&self.fg)?,
            dim: hex(&self.dim)?,
            line: hex(&self.line)?,
            sel: hex(&self.sel)?,
            blue: hex(&self.blue)?,
            green: hex(&self.green)?,
            yellow: hex(&self.yellow)?,
            red: hex(&self.red)?,
            magenta: hex(&self.magenta)?,
            cyan: hex(&self.cyan)?,
        })
    }
}

fn parse(text: &str) -> Result<Vec<Theme>, String> {
    let file: RawFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    file.themes
        .iter()
        .map(|t| {
            Ok(Theme {
                id: t.id.clone(),
                name: t.name.clone(),
                dark: t.dark.palette()?,
                light: t.light.palette()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_five_themes_with_two_members_each() {
        let ids: Vec<&str> = all().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            ["one", "catppuccin", "tokyo-night", "gruvbox", "solarized"]
        );
        for t in all() {
            assert_ne!(t.dark, t.light, "{}", t.id);
            assert!(t.dark.bg.0 < t.light.bg.0, "{}: dark is darker", t.id);
        }
    }

    #[test]
    fn an_unknown_theme_is_the_default_one() {
        assert_eq!(get("nope").id, DEFAULT);
        assert_eq!(get("gruvbox").name, "Gruvbox");
        assert_eq!(get("one").dark.bg, (0x28, 0x2c, 0x34));
    }

    #[test]
    fn a_bad_colour_is_an_error() {
        assert!(hex("#12345").is_err());
        assert!(hex("123456").is_err());
        assert!(hex("#+f+f+f").is_err(), "a sign is not a digit");
        assert!(hex("#ééé").is_err(), "multi-byte text does not panic");
        assert_eq!(hex("#0a0B0c"), Ok((10, 11, 12)));
    }
}
