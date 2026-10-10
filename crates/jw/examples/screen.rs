//! Prints the screen a byte stream leaves on a terminal of `rows cols`
//! (default 35 120): what `scripts/drive.py` uses to show the TUI as text.

use std::io::Read;

fn main() {
    let a: Vec<u16> = std::env::args()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let (rows, cols) = (
        a.first().copied().unwrap_or(35),
        a.get(1).copied().unwrap_or(120),
    );
    let mut b = Vec::new();
    std::io::stdin().read_to_end(&mut b).unwrap();
    let mut p = vt100::Parser::new(rows, cols, 0);
    p.process(&b);
    print!("{}", p.screen().contents());
}
