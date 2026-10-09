//! The two palettes jw draws with, Atom One Dark and One Light (REQ-43),
//! and which one is in use. Everything that draws asks [`p`] for colours, so
//! a switch of the system's appearance repaints the next frame.

use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::Color;

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

/// The palette in use.
pub fn p() -> &'static Palette {
    if DARK.load(Ordering::Relaxed) {
        &ONE_DARK
    } else {
        &ONE_LIGHT
    }
}

/// Switches palettes; true when that changed anything.
pub fn set_dark(dark: bool) -> bool {
    DARK.swap(dark, Ordering::Relaxed) != dark
}

/// What `$JW_THEME` asks for: `Some(dark)` for `dark`/`light`, `None` for
/// `auto` or nothing, which follows the system.
pub fn pinned() -> Option<bool> {
    match std::env::var("JW_THEME").ok()?.trim() {
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
