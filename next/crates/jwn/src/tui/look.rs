//! The colours the client draws with: one of the ten palettes of `contract::themes`, in force
//! for the thread that draws. `View` sets it at the start of every frame, so what it draws
//! follows the person's mode and theme without a colour travelling through every function,
//! and a test that never sets it draws the default theme, dark.

use std::cell::Cell;

use contract::themes::{self, Palette, Rgb};

thread_local! {
    static CURRENT: Cell<(Palette, bool)> =
        Cell::new((themes::get(themes::DEFAULT_DARK, true).palette, true));
}

/// Draw with `palette` from now on; `dark` says which member of its theme it is.
pub fn set(palette: Palette, dark: bool) {
    CURRENT.with(|c| c.set((palette, dark)));
}

pub fn is_dark() -> bool {
    CURRENT.with(|c| c.get().1)
}

fn pal() -> Palette {
    CURRENT.with(|c| c.get().0)
}

pub fn bg() -> Rgb {
    pal().bg
}
pub fn panel() -> Rgb {
    pal().panel
}
pub fn fg() -> Rgb {
    pal().fg
}
pub fn dim() -> Rgb {
    pal().dim
}
pub fn line() -> Rgb {
    pal().line
}
pub fn sel() -> Rgb {
    pal().sel
}
pub fn blue() -> Rgb {
    pal().blue
}
pub fn green() -> Rgb {
    pal().green
}
pub fn yellow() -> Rgb {
    pal().yellow
}
pub fn red() -> Rgb {
    pal().red
}
pub fn magenta() -> Rgb {
    pal().magenta
}
pub fn cyan() -> Rgb {
    pal().cyan
}

/// A line the diff added or deleted, and the words inside it that changed: the ground with a
/// little of green or red in it, so they read on every theme.
pub fn add_bg() -> Rgb {
    mix(bg(), green(), 0.16)
}
pub fn del_bg() -> Rgb {
    mix(bg(), red(), 0.16)
}
pub fn add_word() -> Rgb {
    mix(bg(), green(), 0.34)
}
pub fn del_word() -> Rgb {
    mix(bg(), red(), 0.34)
}

/// `a` with `t` (0 to 1) of `b` in it.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let one = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (one(a.0, b.0), one(a.1, b.1), one(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_one_dark_and_set_changes_it_for_this_thread() {
        assert_eq!(bg(), (0x28, 0x2c, 0x34));
        assert!(is_dark());
        let light = themes::get("one-light", false).palette;
        set(light, false);
        assert_eq!(bg(), light.bg);
        assert!(!is_dark());
    }

    #[test]
    fn the_diff_tints_sit_between_the_ground_and_the_colour() {
        assert_eq!(mix((0, 0, 0), (100, 200, 50), 0.5), (50, 100, 25));
        let (a, b) = (add_bg(), add_word());
        assert!(a.1 > bg().1 && b.1 > a.1, "{a:?} {b:?}");
    }
}
