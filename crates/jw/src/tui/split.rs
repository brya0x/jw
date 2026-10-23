//! The panels' layout on the screen (spec REQ-286 to REQ-292): the rectangles of the tree the
//! daemon keeps, which panel is on a side of another, and the way `n` splits.

use std::collections::HashMap;

use contract::{Layout, SplitDir};

use super::Rect;

/// A side of a panel, for moving to the panel there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// The rectangle of each panel of `layout` inside `area`: a split gives its first side the share
/// `ratio` of the width or height, rounded down, and always leaves each side one cell when there are two.
pub fn rects(layout: &Layout, area: Rect) -> HashMap<&str, Rect> {
    let mut out = HashMap::new();
    place(layout, area, &mut out);
    out
}

fn place<'a>(layout: &'a Layout, (x, y, w, h): Rect, out: &mut HashMap<&'a str, Rect>) {
    match layout {
        Layout::Pane { pane } => {
            out.insert(pane, (x, y, w, h));
        }
        Layout::Split { dir, ratio, a, b } => {
            let share = |len: usize| {
                let first = len * usize::from(*ratio) / 1000;
                if len >= 2 {
                    first.clamp(1, len - 1)
                } else {
                    len
                }
            };
            match dir {
                SplitDir::Row => {
                    let first = share(w);
                    place(a, (x, y, first, h), out);
                    place(b, (x + first, y, w - first, h), out);
                }
                SplitDir::Col => {
                    let first = share(h);
                    place(a, (x, y, w, first), out);
                    place(b, (x, y + first, w, h - first), out);
                }
            }
        }
    }
}

/// Is `pane` the first side (`a`: left or upper) of the nearest split of `dir` around it, the one
/// `grow_pane` moves (REQ-295)? `None` when there is no split of that direction around it.
pub fn first_side(layout: &Layout, pane: &str, dir: SplitDir) -> Option<bool> {
    fn holds(l: &Layout, pane: &str) -> bool {
        match l {
            Layout::Pane { pane: p } => p == pane,
            Layout::Split { a, b, .. } => holds(a, pane) || holds(b, pane),
        }
    }
    let Layout::Split { dir: d, a, b, .. } = layout else {
        return None;
    };
    let in_a = holds(a, pane);
    first_side(if in_a { a } else { b }, pane, dir)
        .or((*d == dir && (in_a || holds(b, pane))).then_some(in_a))
}

/// The panel next to `pane` on `side`: of those that touch its side and overlap it across, the
/// nearest, and among equals the one whose middle is closest.
pub fn neighbour<'a>(rects: &HashMap<&'a str, Rect>, pane: &str, side: Side) -> Option<&'a str> {
    let &(x, y, w, h) = rects.get(pane)?;
    rects
        .iter()
        .filter(|(id, r)| **id != pane && r.2 > 0 && r.3 > 0)
        .filter_map(|(id, &(ox, oy, ow, oh))| {
            let across_rows = oy < y + h && oy + oh > y;
            let across_cols = ox < x + w && ox + ow > x;
            let (gap, along) = match side {
                Side::Left if across_rows && ox + ow <= x => (x - (ox + ow), middle(y, h, oy, oh)),
                Side::Right if across_rows && ox >= x + w => (ox - (x + w), middle(y, h, oy, oh)),
                Side::Up if across_cols && oy + oh <= y => (y - (oy + oh), middle(x, w, ox, ow)),
                Side::Down if across_cols && oy >= y + h => (oy - (y + h), middle(x, w, ox, ow)),
                _ => return None,
            };
            Some(((gap, along, *id), *id))
        })
        .min()
        .map(|(_, id)| id)
}

/// How far apart the middles of two spans are, in half cells.
fn middle(a: usize, alen: usize, b: usize, blen: usize) -> usize {
    (2 * a + alen).abs_diff(2 * b + blen)
}

/// `n` splits along the panel's longer side, a cell being about twice as tall as wide.
pub fn auto_dir((_, _, w, h): Rect) -> SplitDir {
    if w < 2 * h {
        SplitDir::Col
    } else {
        SplitDir::Row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> Layout {
        Layout::Pane { pane: id.into() }
    }

    fn split(dir: SplitDir, ratio: u16, a: Layout, b: Layout) -> Layout {
        Layout::Split {
            dir,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// `a` on the left, `b` over `c` on the right.
    fn three() -> Layout {
        split(
            SplitDir::Row,
            500,
            pane("a"),
            split(SplitDir::Col, 500, pane("b"), pane("c")),
        )
    }

    #[test]
    fn first_side_is_the_side_in_the_nearest_split_of_that_direction() {
        // `a` | (`b` over `c`), and `c` in a row split of its own inside `b`'s column.
        let tree = split(
            SplitDir::Row,
            500,
            pane("a"),
            split(
                SplitDir::Col,
                500,
                pane("b"),
                split(SplitDir::Row, 500, pane("c"), pane("d")),
            ),
        );
        let at = |p, d| first_side(&tree, p, d);
        assert_eq!(at("a", SplitDir::Row), Some(true));
        assert_eq!(at("a", SplitDir::Col), None);
        assert_eq!(
            at("b", SplitDir::Row),
            Some(false),
            "the outer one, b has none inside"
        );
        assert_eq!(at("b", SplitDir::Col), Some(true));
        assert_eq!(
            at("c", SplitDir::Row),
            Some(true),
            "the inner one, not the outer"
        );
        assert_eq!(at("d", SplitDir::Row), Some(false));
        assert_eq!(at("d", SplitDir::Col), Some(false));
        assert_eq!(at("zz", SplitDir::Row), None);
    }

    #[test]
    fn a_split_shares_the_area_by_its_ratio_and_the_sides_meet() {
        let tree = three();
        let m = rects(&tree, (10, 1, 80, 20));
        assert_eq!(m["a"], (10, 1, 40, 20));
        assert_eq!(m["b"], (50, 1, 40, 10));
        assert_eq!(m["c"], (50, 11, 40, 10));
        let tree = split(SplitDir::Row, 250, pane("a"), pane("b"));
        let m = rects(&tree, (0, 0, 81, 5));
        assert_eq!(
            (m["a"].2, m["b"].0, m["b"].2),
            (20, 20, 61),
            "a quarter, rounded down"
        );
    }

    #[test]
    fn a_split_in_a_tiny_area_leaves_each_side_a_cell_when_it_can() {
        let tree = split(SplitDir::Row, 900, pane("a"), pane("b"));
        let m = rects(&tree, (0, 0, 2, 4));
        assert_eq!((m["a"].2, m["b"].2), (1, 1));
        let tree = split(SplitDir::Row, 500, pane("a"), pane("b"));
        let m = rects(&tree, (0, 0, 1, 4));
        assert_eq!((m["a"].2, m["b"].2), (1, 0), "no room: the second has none");
    }

    #[test]
    fn the_neighbour_is_the_nearest_panel_that_touches_the_side() {
        let tree = three();
        let m = rects(&tree, (0, 0, 80, 20));
        assert_eq!(
            neighbour(&m, "a", Side::Right),
            Some("b"),
            "the upper of the two it touches"
        );
        assert_eq!(neighbour(&m, "b", Side::Left), Some("a"));
        assert_eq!(neighbour(&m, "c", Side::Left), Some("a"));
        assert_eq!(neighbour(&m, "b", Side::Down), Some("c"));
        assert_eq!(neighbour(&m, "c", Side::Up), Some("b"));
        for (p, s) in [
            ("a", Side::Left),
            ("a", Side::Up),
            ("b", Side::Up),
            ("c", Side::Down),
            ("c", Side::Right),
        ] {
            assert_eq!(neighbour(&m, p, s), None, "{p} {s:?}");
        }
        assert_eq!(neighbour(&m, "zz", Side::Left), None);
    }

    #[test]
    fn a_panel_with_no_room_is_nobodys_neighbour() {
        let tree = three();
        let mut m = rects(&tree, (0, 0, 80, 20));
        m.insert("b", (0, 0, 0, 0));
        assert_eq!(neighbour(&m, "a", Side::Right), Some("c"));
    }

    #[test]
    fn n_splits_along_the_longer_side() {
        assert_eq!(auto_dir((0, 0, 100, 30)), SplitDir::Row);
        assert_eq!(
            auto_dir((0, 0, 60, 30)),
            SplitDir::Row,
            "twice as wide is wide"
        );
        assert_eq!(auto_dir((0, 0, 59, 30)), SplitDir::Col);
        assert_eq!(auto_dir((0, 0, 20, 40)), SplitDir::Col);
    }
}
