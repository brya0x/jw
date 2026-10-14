//! The ten palettes the clients draw with (`contract/themes.json`): five dark and five light,
//! each saying which it is, so a selector shows only the kind in force. The file is the home
//! of the colours; the terminal client reads it through here and the phone's table is tested
//! against it.

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

/// One palette: a theme of the list, dark or light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub palette: Palette,
}

/// The themes a person who chose nothing gets, one for each kind.
pub const DEFAULT_DARK: &str = "one-dark";
pub const DEFAULT_LIGHT: &str = "one-light";

const FILE: &str = include_str!("../../../contract/themes.json");

/// Every theme, in the order the file lists them.
pub fn all() -> &'static [Theme] {
    static ALL: OnceLock<Vec<Theme>> = OnceLock::new();
    ALL.get_or_init(|| parse(FILE).expect("contract/themes.json is not a list of themes"))
}

/// The themes of one kind: what a selector shows while the mode is dark (or light).
pub fn of_kind(dark: bool) -> impl Iterator<Item = &'static Theme> {
    all().iter().filter(move |t| t.dark == dark)
}

/// The theme named `id` when it is of this kind, else the default one of the kind.
pub fn get(id: &str, dark: bool) -> &'static Theme {
    let default = if dark { DEFAULT_DARK } else { DEFAULT_LIGHT };
    of_kind(dark)
        .find(|t| t.id == id)
        .or_else(|| of_kind(dark).find(|t| t.id == default))
        .or_else(|| of_kind(dark).next())
        .unwrap_or(&all()[0])
}

#[derive(Deserialize)]
struct RawFile {
    themes: Vec<RawTheme>,
}

#[derive(Deserialize)]
struct RawTheme {
    id: String,
    name: String,
    kind: String,
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

impl RawTheme {
    fn theme(&self) -> Result<Theme, String> {
        let dark = match self.kind.as_str() {
            "dark" => true,
            "light" => false,
            other => return Err(format!("{}: kind is {other}, not dark or light", self.id)),
        };
        Ok(Theme {
            id: self.id.clone(),
            name: self.name.clone(),
            dark,
            palette: Palette {
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
            },
        })
    }
}

fn parse(text: &str) -> Result<Vec<Theme>, String> {
    let file: RawFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    file.themes.iter().map(RawTheme::theme).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_five_dark_and_five_light_themes_and_each_says_which() {
        let ids = |dark| of_kind(dark).map(|t| t.id.as_str()).collect::<Vec<_>>();
        assert_eq!(
            ids(true),
            [
                "one-dark",
                "catppuccin-mocha",
                "tokyo-night",
                "gruvbox-dark",
                "solarized-dark"
            ]
        );
        assert_eq!(
            ids(false),
            [
                "one-light",
                "catppuccin-latte",
                "tokyo-day",
                "gruvbox-light",
                "solarized-light"
            ]
        );
        for t in all() {
            let (bg, fg) = (t.palette.bg, t.palette.fg);
            let lum = |c: Rgb| u32::from(c.0) + u32::from(c.1) + u32::from(c.2);
            assert_eq!(
                lum(bg) < lum(fg),
                t.dark,
                "{}: the ground is the kind",
                t.id
            );
        }
        let all_ids: std::collections::HashSet<_> = all().iter().map(|t| &t.id).collect();
        assert_eq!(all_ids.len(), all().len(), "ids are unique");
    }

    #[test]
    fn a_theme_of_the_other_kind_or_an_unknown_one_is_the_default_of_the_kind() {
        assert_eq!(get("nope", true).id, DEFAULT_DARK);
        assert_eq!(get("nope", false).id, DEFAULT_LIGHT);
        assert_eq!(
            get("one-light", true).id,
            DEFAULT_DARK,
            "light is not a dark choice"
        );
        assert_eq!(get("gruvbox-light", false).name, "Gruvbox Light");
        assert_eq!(get("one-dark", true).palette.bg, (0x28, 0x2c, 0x34));
    }

    #[test]
    fn a_kind_that_is_neither_dark_nor_light_is_an_error() {
        let text = FILE.replacen("\"kind\": \"dark\"", "\"kind\": \"dim\"", 1);
        assert!(parse(&text).is_err());
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
