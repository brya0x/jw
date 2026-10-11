//! A QR code as a grid of modules, with no terminal in it (mobile spec,
//! REQ-47). The CLI draws it with escape codes and the TUI with its own
//! cells; both start from here.

use anyhow::{Context, Result};
use qrcode::{Color, QrCode};

/// Modules of white around the code, which a scanner needs to find it.
const QUIET: usize = 2;

/// A code with its quiet zone: `size` by `size` modules.
pub struct Qr {
    size: usize,
    dark: Vec<bool>,
}

impl Qr {
    pub fn new(text: &str) -> Result<Self> {
        let code = QrCode::new(text.as_bytes()).context("making the QR")?;
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
        assert!(n >= 29 + 2 * QUIET, "{n}");
        for i in 0..n {
            for edge in [0, 1, n - 2, n - 1] {
                assert!(!qr.dark(i, edge), "row {edge} is white");
                assert!(!qr.dark(edge, i), "column {edge} is white");
            }
        }
        assert!(!qr.dark(n, 0) && !qr.dark(0, n));
        let black = (0..n * n).filter(|i| qr.dark(i % n, i / n)).count();
        assert!(black > n * n / 5 && black < n * n * 3 / 5, "{black} black");
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
