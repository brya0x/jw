//! A QR code as a grid of modules, with no terminal in it (mobile spec,
//! REQ-47). The CLI draws it with escape codes and the TUI with its own
//! cells; both start from here.

use anyhow::{Context, Result};
use qrcode::{Color, EcLevel, QrCode};

/// Modules of white around the code, which a scanner needs to find it: the
/// QR standard asks for four.
const QUIET: usize = 4;

/// A code with its quiet zone: `size` by `size` modules.
pub struct Qr {
    size: usize,
    dark: Vec<bool>,
}

impl Qr {
    pub fn new(text: &str) -> Result<Self> {
        // Level L: a screen shows it without damage, and a smaller code fits the
        // modal (mobile spec, REQ-47).
        let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L)
            .context("making the QR")?;
        let width = code.width();
        let size = width + 2 * QUIET;
        let mut dark = vec![false; size * size];
        for (i, c) in code.to_colors().into_iter().enumerate() {
            let (x, y) = (i % width + QUIET, i / width + QUIET);
            dark[y * size + x] = c == Color::Dark;
        }
        Ok(Self { size, dark })
    }

    /// Modules on a side, quiet zone included.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether the module at (`x`, `y`) is black; outside is white.
    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }

    /// Rows of character cells, two modules tall each: (the upper one dark,
    /// the lower one dark). Drawn as `▀`, foreground the upper and background
    /// the lower, it keeps the modules square. The last row is half empty
    /// when `size` is odd.
    pub fn cells(&self) -> Vec<Vec<(bool, bool)>> {
        (0..self.size)
            .step_by(2)
            .map(|y| {
                (0..self.size)
                    .map(|x| (self.dark(x, y), self.dark(x, y + 1)))
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str =
        "jw://guys-slowly-dietary-flags.trycloudflare.com/00112233445566778899aabbccddeeff";

    #[test]
    fn a_code_is_a_square_with_a_quiet_zone() {
        let qr = Qr::new(PAYLOAD).unwrap();
        let n = qr.size();
        assert!(n >= 21 + 2 * QUIET, "{n}");
        // Four modules of white on every side, and the code starts right after.
        for i in 0..n {
            for edge in (0..QUIET).chain(n - QUIET..n) {
                assert!(!qr.dark(i, edge), "row {edge} is white");
                assert!(!qr.dark(edge, i), "column {edge} is white");
            }
        }
        assert_eq!(QUIET, 4);
        // The finder pattern's corner is the first black module, at (4, 4).
        assert!(qr.dark(QUIET, QUIET) && qr.dark(n - QUIET - 1, QUIET));
        assert!((0..n).any(|i| qr.dark(i, QUIET)) && (0..n).all(|i| !qr.dark(i, QUIET - 1)));
        assert!(!qr.dark(n, 0) && !qr.dark(0, n));
        let black = (0..n * n).filter(|i| qr.dark(i % n, i / n)).count();
        assert!(black > n * n / 5 && black < n * n * 3 / 5, "{black} black");
    }

    #[test]
    fn a_real_link_is_version_5_so_it_fits_the_modal() {
        // 88 bytes: a 50-character tunnel name and a 32-digit token.
        let host = "micro-interview-purchased-unlock.trycloudflare.com";
        let qr = Qr::new(&format!("jw://{host}/{}", "0123456789abcdef".repeat(2))).unwrap();
        assert_eq!(qr.size(), 37 + 2 * QUIET);
    }

    #[test]
    fn cells_pack_two_modules_to_a_row_of_text() {
        let qr = Qr::new(PAYLOAD).unwrap();
        let cells = qr.cells();
        assert_eq!(cells.len(), qr.size().div_ceil(2));
        assert!(cells.iter().all(|r| r.len() == qr.size()));
        // The quiet zone on top, and a module that is dark where the grid is.
        assert!(cells[0].iter().all(|&(t, b)| !t && !b));
        let (x, y) = (0..qr.size() * qr.size())
            .map(|i| (i % qr.size(), i / qr.size()))
            .find(|&(x, y)| qr.dark(x, y))
            .unwrap();
        let (top, bottom) = cells[y / 2][x];
        assert!(if y % 2 == 0 { top } else { bottom });
    }
}
