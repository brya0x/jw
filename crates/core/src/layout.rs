//! The split trees of a workspace's panes (docs/specs/rust-tui.md, "Tree"
//! and "Layout config"): [`Node`] is `[layout]` as written in the project
//! TOML, the shape a workspace starts with; [`Tree`] is the live tree the
//! daemon keeps, with pane ids, that `t`, `x`, `HJKL` and `n` edit. Both turn
//! into rectangles for any terminal size. Pure: no terminal, no daemon.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    /// A project's layout when its config has none: one shell, and the user
    /// builds the rest (REQ-118).
    pub fn default_tree() -> Self {
        Self::leaf("shell")
    }

    /// Editor and agent side by side over a full-width shell: the old
    /// default, kept as a three-pane fixture for tests.
    #[cfg(test)]
    pub(crate) fn three_panes() -> Self {
        Self::split(
            Dir::Down,
            0.7,
            Self::split(Dir::Right, 0.5, Self::leaf("editor"), Self::leaf("agent")),
            Self::leaf("shell"),
        )
    }

    /// The node as a TOML inline table: `{ run = "shell" }` or
    /// `{ split = "right", ratio = 0.5, a = …, b = … }`.
    pub fn inline(&self) -> String {
        let keys = self.keys();
        format!("{{ {} }}", keys.join(", "))
    }

    /// The `[layout]` lines that hold this tree, one key per line.
    pub fn table(&self) -> String {
        self.keys().into_iter().map(|k| k + "\n").collect()
    }

    fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(run) = &self.run {
            out.push(format!("run = {}", quote(run)));
        }
        if let Some(dir) = self.split {
            let dir = match dir {
                Dir::Down => "down",
                Dir::Right => "right",
            };
            out.push(format!("split = \"{dir}\""));
        }
        if let Some(r) = self.ratio {
            out.push(format!("ratio = {}", (r * 100.0).round() / 100.0));
        }
        if let Some(a) = &self.a {
            out.push(format!("a = {}", a.inline()));
        }
        if let Some(b) = &self.b {
            out.push(format!("b = {}", b.inline()));
        }
        out
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
        let (ra, rb) = halves(area, dir, self.ratio.unwrap_or(0.5));
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

/// A TOML basic string.
pub(crate) fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What a leaf of a [`Tree`] is known by.
pub trait Keyed {
    fn key(&self) -> u64;
}

/// The live tree of a workspace's panes (RAT-9). `L` is what a leaf carries:
/// a pane to start on the way to the daemon, a running pane on the way back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tree<L> {
    Leaf(L),
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Tree<L>>,
        b: Box<Tree<L>>,
    },
}

impl<L> Tree<L> {
    /// The shape of `node`, with its leaves taken in order from `leaves`.
    /// `None` when `leaves` runs out.
    pub fn from_node(node: &Node, leaves: &mut impl Iterator<Item = L>) -> Option<Self> {
        match (node.split, &node.a, &node.b) {
            (Some(dir), Some(a), Some(b)) => Some(Self::Split {
                dir,
                ratio: node.ratio.unwrap_or(0.5),
                a: Box::new(Self::from_node(a, leaves)?),
                b: Box::new(Self::from_node(b, leaves)?),
            }),
            _ => leaves.next().map(Self::Leaf),
        }
    }

    /// The tree as a layout, each leaf's `run` given by `run` (REQ-120).
    pub fn to_node(&self, run: &impl Fn(&L) -> String) -> Node {
        match self {
            Self::Leaf(l) => Node::leaf(&run(l)),
            Self::Split { dir, ratio, a, b } => {
                Node::split(*dir, *ratio, a.to_node(run), b.to_node(run))
            }
        }
    }

    /// The leaves in tree order (a before b).
    pub fn leaves(&self) -> Vec<&L> {
        match self {
            Self::Leaf(l) => vec![l],
            Self::Split { a, b, .. } => {
                let mut out = a.leaves();
                out.extend(b.leaves());
                out
            }
        }
    }

    pub fn leaves_mut(&mut self) -> Vec<&mut L> {
        match self {
            Self::Leaf(l) => vec![l],
            Self::Split { a, b, .. } => {
                let mut out = a.leaves_mut();
                out.extend(b.leaves_mut());
                out
            }
        }
    }

    /// The same shape with every leaf replaced by `f(leaf)`.
    pub fn try_map<M, E>(self, f: &mut impl FnMut(L) -> Result<M, E>) -> Result<Tree<M>, E> {
        Ok(match self {
            Self::Leaf(l) => Tree::Leaf(f(l)?),
            Self::Split { dir, ratio, a, b } => Tree::Split {
                dir,
                ratio,
                a: Box::new(a.try_map(f)?),
                b: Box::new(b.try_map(f)?),
            },
        })
    }

    /// Each leaf's rectangle inside `area`, in the order of [`Tree::leaves`],
    /// tiling `area` exactly.
    pub fn rects(&self, area: Rect) -> Vec<Rect> {
        let mut out = Vec::new();
        self.place(area, &mut out);
        out
    }

    fn place(&self, area: Rect, out: &mut Vec<Rect>) {
        match self {
            Self::Leaf(_) => out.push(area),
            Self::Split { dir, ratio, a, b } => {
                let (ra, rb) = halves(area, *dir, *ratio);
                a.place(ra, out);
                b.place(rb, out);
            }
        }
    }
}

/// Where a split sits: from the root, `false` for `a` and `true` for `b`.
pub type SplitPath = Vec<bool>;

/// A split between two parts of a tree, as drawn: the line where they meet
/// can be dragged.
#[derive(Debug, Clone, PartialEq)]
pub struct Border {
    pub path: SplitPath,
    pub dir: Dir,
    /// The whole split's area.
    pub area: Rect,
}

impl<L> Tree<L> {
    /// Every split with its area, outermost first.
    pub fn borders(&self, area: Rect) -> Vec<Border> {
        let mut out = Vec::new();
        self.collect_borders(area, &mut Vec::new(), &mut out);
        out
    }

    fn collect_borders(&self, area: Rect, path: &mut SplitPath, out: &mut Vec<Border>) {
        if let Self::Split { dir, ratio, a, b } = self {
            out.push(Border {
                path: path.clone(),
                dir: *dir,
                area,
            });
            let (ra, rb) = halves(area, *dir, *ratio);
            path.push(false);
            a.collect_borders(ra, path, out);
            path.pop();
            path.push(true);
            b.collect_borders(rb, path, out);
            path.pop();
        }
    }

    /// The split whose dividing line is at `(x, y)`: the border columns (or
    /// rows) of the two panes that meet there. The innermost one wins.
    pub fn border_at(&self, area: Rect, x: u16, y: u16) -> Option<Border> {
        self.borders(area).into_iter().rev().find(|b| {
            let (ra, _) = match self.split_at(&b.path) {
                Some((dir, ratio)) => halves(b.area, dir, ratio),
                None => return false,
            };
            let a = b.area;
            match b.dir {
                Dir::Right => {
                    let edge = ra.x + ra.w;
                    (x + 1 == edge || x == edge) && y >= a.y && y < a.y + a.h
                }
                Dir::Down => {
                    let edge = ra.y + ra.h;
                    (y + 1 == edge || y == edge) && x >= a.x && x < a.x + a.w
                }
            }
        })
    }

    /// The share `a` keeps in the split at `path`.
    pub fn ratio_at(&self, path: &[bool]) -> Option<f32> {
        self.split_at(path).map(|(_, r)| r)
    }

    fn split_at(&self, path: &[bool]) -> Option<(Dir, f32)> {
        match (self, path.split_first()) {
            (Self::Split { dir, ratio, .. }, None) => Some((*dir, *ratio)),
            (Self::Split { a, b, .. }, Some((side, rest))) => {
                if *side { b } else { a }.split_at(rest)
            }
            _ => None,
        }
    }

    /// Sets the share `a` keeps in the split at `path`, kept between 10% and
    /// 90%. False when there is no split there.
    pub fn set_ratio(&mut self, path: &[bool], to: f32) -> bool {
        match (self, path.split_first()) {
            (Self::Split { ratio, .. }, None) => {
                *ratio = to.clamp(0.1, 0.9);
                true
            }
            (Self::Split { a, b, .. }, Some((side, rest))) => {
                if *side { b } else { a }.set_ratio(rest, to)
            }
            _ => false,
        }
    }
}

impl<L: Keyed> Tree<L> {
    pub fn find(&self, id: u64) -> Option<&L> {
        self.leaves().into_iter().find(|l| l.key() == id)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut L> {
        self.leaves_mut().into_iter().find(|l| l.key() == id)
    }

    /// Each leaf's key with its rectangle.
    pub fn placed(&self, area: Rect) -> Vec<(u64, Rect)> {
        self.leaves()
            .into_iter()
            .map(Keyed::key)
            .zip(self.rects(area))
            .collect()
    }

    /// Splits the leaf `beside` in two along `dir`: it keeps the first half
    /// and `leaf` takes the second. Gives `leaf` back when there is no such
    /// leaf.
    pub fn insert(self, beside: u64, dir: Dir, leaf: L) -> (Self, Option<L>) {
        match self {
            Self::Leaf(l) if l.key() == beside => (
                Self::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Self::Leaf(l)),
                    b: Box::new(Self::Leaf(leaf)),
                },
                None,
            ),
            Self::Leaf(_) => (self, Some(leaf)),
            Self::Split {
                dir: d,
                ratio,
                a,
                b,
            } => {
                let (a, rest) = a.insert(beside, dir, leaf);
                let (b, rest) = match rest {
                    Some(leaf) => b.insert(beside, dir, leaf),
                    None => (*b, None),
                };
                (
                    Self::Split {
                        dir: d,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    },
                    rest,
                )
            }
        }
    }

    /// The whole tree on the left and `leaf` on the right, with `share` of
    /// the width.
    pub fn dock(self, leaf: L, share: f32) -> Self {
        Self::Split {
            dir: Dir::Right,
            ratio: (1.0 - share).clamp(0.1, 0.9),
            a: Box::new(self),
            b: Box::new(Self::Leaf(leaf)),
        }
    }

    /// Takes the leaf `id` out; its sibling gets the space. `None` for the
    /// tree when that was the last leaf.
    pub fn remove(self, id: u64) -> (Option<Self>, Option<L>) {
        match self {
            Self::Leaf(l) if l.key() == id => (None, Some(l)),
            Self::Leaf(_) => (Some(self), None),
            Self::Split { dir, ratio, a, b } => {
                let (a, gone) = a.remove(id);
                let (b, gone) = match gone {
                    Some(g) => (Some(*b), Some(g)),
                    None => b.remove(id),
                };
                let tree = match (a, b) {
                    (Some(a), Some(b)) => Some(Self::Split {
                        dir,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (one, None) | (None, one) => one,
                };
                (tree, gone)
            }
        }
    }

    /// Trades the places of two leaves. False when either is missing.
    pub fn swap(&mut self, x: u64, y: u64) -> bool
    where
        L: Clone,
    {
        let (Some(lx), Some(ly)) = (self.find(x).cloned(), self.find(y).cloned()) else {
            return false;
        };
        for l in self.leaves_mut() {
            if l.key() == x {
                *l = ly.clone();
            } else if l.key() == y {
                *l = lx.clone();
            }
        }
        true
    }

    /// The leaf next to `id` towards `(dx, dy)` (one of them ±1): the
    /// nearest rectangle on that side that overlaps it on the other axis.
    pub fn neighbour(&self, id: u64, dx: i32, dy: i32, area: Rect) -> Option<u64> {
        let placed = self.placed(area);
        let (_, from) = placed.iter().find(|(k, _)| *k == id)?;
        let (fx0, fy0) = (i32::from(from.x), i32::from(from.y));
        let (fx1, fy1) = (fx0 + i32::from(from.w), fy0 + i32::from(from.h));
        placed
            .iter()
            .filter(|(k, _)| *k != id)
            .filter_map(|(k, r)| {
                let (x0, y0) = (i32::from(r.x), i32::from(r.y));
                let (x1, y1) = (x0 + i32::from(r.w), y0 + i32::from(r.h));
                let (gap, overlap, along) = match (dx, dy) {
                    (1, _) => (x0 - fx1, y0 < fy1 && y1 > fy0, (y0 - fy0).abs()),
                    (-1, _) => (fx0 - x1, y0 < fy1 && y1 > fy0, (y0 - fy0).abs()),
                    (_, 1) => (y0 - fy1, x0 < fx1 && x1 > fx0, (x0 - fx0).abs()),
                    _ => (fy0 - y1, x0 < fx1 && x1 > fx0, (x0 - fx0).abs()),
                };
                (gap >= 0 && overlap).then_some(((gap, along), *k))
            })
            .min_by_key(|(d, _)| *d)
            .map(|(_, k)| k)
    }
}

/// Splits `area` along `dir`, `a` keeping `ratio` of it.
fn halves(area: Rect, dir: Dir, ratio: f32) -> (Rect, Rect) {
    match dir {
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

    /// REQ-117, 120: a tree written as TOML reads back the same, and a live
    /// tree turns back into its layout.
    #[test]
    fn a_node_round_trips_through_toml() {
        #[derive(Deserialize)]
        struct Wrap {
            layout: Node,
        }
        let mut t = Node::three_panes();
        t.a.as_mut().unwrap().b = Some(Box::new(Node::leaf("pnpm dev \"x\"")));
        let back: Wrap = toml::from_str(&format!("[layout]\n{}", t.table())).unwrap();
        assert_eq!(back.layout, t);
        assert_eq!(Node::leaf("shell").inline(), r#"{ run = "shell" }"#);

        let mut ids = [1u64, 2, 3].into_iter();
        let live = Tree::from_node(&Node::three_panes(), &mut ids).unwrap();
        let runs = ["", "editor", "agent", "shell"];
        assert_eq!(
            live.to_node(&|id| runs[*id as usize].to_string()),
            Node::three_panes()
        );
    }

    #[test]
    fn the_default_is_one_shell_and_three_panes_tile() {
        assert_eq!(Node::default_tree().leaves()[0].role, "shell");
        assert_eq!(Node::default_tree().leaves().len(), 1);
        let t = Node::three_panes();
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
        let t = Node::three_panes();
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

    impl Keyed for u64 {
        fn key(&self) -> u64 {
            *self
        }
    }

    /// editor | agent over a full-width shell, as ids 1 | 2 over 3.
    fn three() -> Tree<u64> {
        let mut ids = [1u64, 2, 3].into_iter();
        Tree::from_node(&Node::three_panes(), &mut ids).unwrap()
    }

    fn keys(t: &Tree<u64>) -> Vec<u64> {
        t.leaves().into_iter().copied().collect()
    }

    #[test]
    fn a_tree_takes_the_shape_of_its_layout() {
        let t = three();
        assert_eq!(keys(&t), [1, 2, 3]);
        assert_eq!(
            t.rects(area(100, 40)),
            Node::three_panes().rects(area(100, 40))
        );
        let mut short = [1u64].into_iter();
        assert!(Tree::from_node(&Node::three_panes(), &mut short).is_none());
    }

    #[test]
    fn insert_splits_the_leaf_and_remove_gives_the_space_back() {
        let (t, rest) = three().insert(3, Dir::Right, 4);
        assert!(rest.is_none());
        assert_eq!(keys(&t), [1, 2, 3, 4]);
        let r = t.rects(area(100, 40));
        assert_eq!((r[2].w, r[3].x), (50, 50), "the shell halves");

        let (t, missing) = t.insert(9, Dir::Down, 5);
        assert_eq!(missing, Some(5));

        let (t, gone) = t.remove(4);
        assert_eq!(gone, Some(4));
        let t = t.unwrap();
        assert_eq!(t.rects(area(100, 40)), three().rects(area(100, 40)));

        let (t, _) = t.remove(1);
        let (t, _) = t.unwrap().remove(2);
        assert_eq!(keys(t.as_ref().unwrap()), [3]);
        assert_eq!(t.unwrap().rects(area(100, 40)), vec![area(100, 40)]);
        let (none, gone) = Tree::Leaf(7u64).remove(7);
        assert!(none.is_none() && gone == Some(7));
    }

    #[test]
    fn swap_trades_places() {
        let mut t = three();
        assert!(t.swap(1, 3));
        assert_eq!(keys(&t), [3, 2, 1]);
        assert!(!t.swap(1, 9));
    }

    #[test]
    fn neighbours_are_on_that_side_and_overlapping() {
        let t = three();
        let a = area(100, 40);
        assert_eq!(t.neighbour(1, 1, 0, a), Some(2));
        assert_eq!(t.neighbour(2, -1, 0, a), Some(1));
        assert_eq!(t.neighbour(1, 0, 1, a), Some(3));
        assert_eq!(t.neighbour(2, 0, 1, a), Some(3));
        // Up from the wide shell: the left one, the nearest by its edge.
        assert_eq!(t.neighbour(3, 0, -1, a), Some(1));
        assert_eq!(t.neighbour(1, -1, 0, a), None);
        assert_eq!(t.neighbour(3, 0, 1, a), None);
    }

    #[test]
    fn trees_round_trip_as_json() {
        let t = three();
        let back: Tree<u64> = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn borders_are_found_and_dragged() {
        let mut t = three();
        let a = area(100, 40);
        // editor | agent over the shell: the vertical line at x 49/50 in
        // the top 28 rows, the horizontal one at y 27/28 across.
        let v = t.border_at(a, 49, 5).unwrap();
        assert_eq!((v.path.clone(), v.dir), (vec![false], Dir::Right));
        assert_eq!(t.border_at(a, 50, 5).unwrap().path, vec![false]);
        let h = t.border_at(a, 10, 28).unwrap();
        assert_eq!((h.path.clone(), h.dir), (vec![], Dir::Down));
        assert!(t.border_at(a, 20, 10).is_none());
        assert!(
            t.border_at(a, 49, 35).is_none(),
            "the shell has no vertical line"
        );

        assert!(t.set_ratio(&[false], 0.25));
        assert_eq!(t.rects(a)[0].w, 25);
        assert!(t.set_ratio(&[], 2.0));
        assert_eq!(t.rects(a)[2].y, 36, "kept at 90%");
        assert!(!t.set_ratio(&[true], 0.5), "the shell is a leaf");
    }
}
