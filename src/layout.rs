//! The split tree of a stream's panes (docs/specs/rust-tui.md, "Layout"):
//! read from `[layout]` in the project TOML, turned into rectangles for any
//! terminal size. Pure: no terminal, no daemon.

use anyhow::{Result, bail};
use serde::Deserialize;

/// One node of the tree as written in TOML: a leaf has `run`, a split has
/// `split`, `a` and `b` (and optionally `ratio`, the share `a` keeps).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Node {
    pub split: Option<Dir>,
    pub ratio: Option<f32>,
    pub a: Option<Box<Node>>,
    pub b: Option<Box<Node>>,
    pub run: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// `a` on top, `b` below.
    Down,
    /// `a` on the left, `b` on the right.
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// A leaf of the tree with a name unique within the stream: the `run` value,
/// with `-2`, `-3`… when several leaves run the same thing. The daemon keeps
/// it as the pane's role, which is how a client that reattaches finds which
/// pane goes where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub role: String,
    pub run: String,
}

impl Node {
    pub fn leaf(run: &str) -> Self {
        Self {
            run: Some(run.into()),
            ..Self::default()
        }
    }

    pub fn split(dir: Dir, ratio: f32, a: Node, b: Node) -> Self {
        Self {
            split: Some(dir),
            ratio: Some(ratio),
            a: Some(Box::new(a)),
            b: Some(Box::new(b)),
            run: None,
        }
    }

    /// What `jw open` builds with herdr today (internal/backends/terminal):
    /// editor and agent side by side on top, a full-width shell below.
    pub fn default_tree() -> Self {
        Self::split(
            Dir::Down,
            0.7,
            Self::split(Dir::Right, 0.5, Self::leaf("editor"), Self::leaf("agent")),
            Self::leaf("shell"),
        )
    }

    /// Rejects a node that is both or neither a leaf and a split.
    pub fn validate(&self) -> Result<()> {
        match (&self.run, self.split, &self.a, &self.b) {
            (Some(run), None, None, None) if !run.trim().is_empty() => Ok(()),
            (None, Some(_), Some(a), Some(b)) => {
                if let Some(r) = self.ratio
                    && !(r > 0.0 && r < 1.0)
                {
                    bail!("layout: ratio = {r}, must be between 0 and 1");
                }
                a.validate()?;
                b.validate()
            }
            _ => bail!("layout: each node needs either `run`, or `split` with `a` and `b`"),
        }
    }

    /// The leaves in tree order (a before b), with unique roles.
    pub fn leaves(&self) -> Vec<Leaf> {
        let mut runs = Vec::new();
        self.walk(&mut |n| runs.push(n.run.clone().unwrap_or_default()));
        let mut out: Vec<Leaf> = Vec::with_capacity(runs.len());
        for run in runs {
            let base = run.split_whitespace().next().unwrap_or("pane").to_string();
            let seen = out
                .iter()
                .filter(|l| l.role == base || l.role.starts_with(&format!("{base}-")))
                .count();
            let role = if seen == 0 {
                base
            } else {
                format!("{base}-{}", seen + 1)
            };
            out.push(Leaf { role, run });
        }
        out
    }

    /// Each leaf's rectangle inside `area`, in the same order as [`leaves`].
    /// Integer rounding gives the remainder to `b`, so the rectangles tile
    /// `area` exactly.
    pub fn rects(&self, area: Rect) -> Vec<Rect> {
        let mut out = Vec::new();
        self.place(area, &mut out);
        out
    }

    fn place(&self, area: Rect, out: &mut Vec<Rect>) {
        let (Some(dir), Some(a), Some(b)) = (self.split, &self.a, &self.b) else {
            out.push(area);
            return;
        };
        let ratio = self.ratio.unwrap_or(0.5);
        let (ra, rb) = match dir {
            Dir::Down => {
                let h = cut(area.h, ratio);
                (
                    Rect { h, ..area },
                    Rect {
                        y: area.y + h,
                        h: area.h - h,
                        ..area
                    },
                )
            }
            Dir::Right => {
                let w = cut(area.w, ratio);
                (
                    Rect { w, ..area },
                    Rect {
                        x: area.x + w,
                        w: area.w - w,
                        ..area
                    },
                )
            }
        };
        a.place(ra, out);
        b.place(rb, out);
    }

    fn walk(&self, f: &mut impl FnMut(&Node)) {
        match (&self.a, &self.b) {
            (Some(a), Some(b)) if self.split.is_some() => {
                a.walk(f);
                b.walk(f);
            }
            _ => f(self),
        }
    }
}

/// `a`'s share of `len`, leaving at least one cell to each side when there is
/// room for both.
fn cut(len: u16, ratio: f32) -> u16 {
    let n = (f32::from(len) * ratio).round() as u16;
    if len >= 2 {
        n.clamp(1, len - 1)
    } else {
        n.min(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect { x: 0, y: 0, w, h }
    }

    #[test]
    fn default_tree_matches_the_herdr_layout() {
        let t = Node::default_tree();
        let roles: Vec<String> = t.leaves().into_iter().map(|l| l.role).collect();
        assert_eq!(roles, ["editor", "agent", "shell"]);
        let r = t.rects(area(100, 40));
        assert_eq!(
            r[0],
            Rect {
                x: 0,
                y: 0,
                w: 50,
                h: 28
            }
        );
        assert_eq!(
            r[1],
            Rect {
                x: 50,
                y: 0,
                w: 50,
                h: 28
            }
        );
        assert_eq!(
            r[2],
            Rect {
                x: 0,
                y: 28,
                w: 100,
                h: 12
            }
        );
    }

    #[test]
    fn rects_tile_the_area_at_any_size() {
        let t = Node::default_tree();
        for (w, h) in [(1, 1), (2, 2), (3, 7), (79, 23), (237, 61)] {
            let total: u32 = t
                .rects(area(w, h))
                .iter()
                .map(|r| u32::from(r.w) * u32::from(r.h))
                .sum();
            assert_eq!(total, u32::from(w) * u32::from(h), "{w}x{h}");
        }
    }

    #[test]
    fn parses_from_toml_and_names_repeated_leaves() {
        let t: Node = toml::from_str(
            r#"
split = "right"
ratio = 0.6
a = { run = "agent" }
[b]
split = "down"
a = { run = "dev:web" }
b = { run = "pnpm test --watch" }
"#,
        )
        .unwrap();
        t.validate().unwrap();
        let roles: Vec<String> = t.leaves().into_iter().map(|l| l.role).collect();
        assert_eq!(roles, ["agent", "dev:web", "pnpm"]);

        let twice = Node::split(Dir::Right, 0.5, Node::leaf("shell"), Node::leaf("shell"));
        let roles: Vec<String> = twice.leaves().into_iter().map(|l| l.role).collect();
        assert_eq!(roles, ["shell", "shell-2"]);
    }

    #[test]
    fn rejects_half_nodes() {
        let bad: Node = toml::from_str("split = \"down\"\na = { run = \"x\" }\n").unwrap();
        assert!(bad.validate().is_err());
        let both: Node = toml::from_str("run = \"x\"\nsplit = \"down\"\n").unwrap();
        assert!(both.validate().is_err());
        let ratio = Node::split(Dir::Down, 1.5, Node::leaf("a"), Node::leaf("b"));
        assert!(ratio.validate().is_err());
    }
}
