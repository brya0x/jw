//! How the panes of a workspace are laid out (spec REQ-286 to REQ-291): a tree of splits whose
//! leaves are exactly the workspace's panes. The tree is the daemon's, kept in `session.json`, so
//! every client draws the same arrangement and it survives a restart.
//!
//! Only the shape is kept here. Panes come and go in many places (a new one, a closed one, one
//! whose shell would not start after a restart), so `reconcile` brings any tree to agree with the
//! panes there are instead of every place keeping it up to date.
//!
//! Since N167 a leaf is a pane or a view (the diff, a viewer): `reconcile_views` keeps both, and
//! a pane that has no place goes beside the last *pane*, never inside a view's room.

use std::collections::HashSet;

use contract::{Id, Layout, SplitDir};

/// The least and the most of a split's width or height one side may have, in thousandths.
pub const MIN_RATIO: u16 = 100;
pub const MAX_RATIO: u16 = 900;

fn leaf(id: &str) -> Layout {
    Layout::Pane {
        pane: id.to_owned(),
    }
}

/// The panes of a tree, left to right and top to bottom.
pub fn leaves(tree: &Layout) -> Vec<&str> {
    match tree {
        Layout::Pane { pane } => vec![pane],
        Layout::Split { a, b, .. } => {
            let mut v = leaves(a);
            v.extend(leaves(b));
            v
        }
    }
}

/// The first pane on the left, the others stacked on the right, each with an even share: what a
/// workspace gets when nothing says otherwise.
pub fn default_tree(ids: &[Id]) -> Option<Layout> {
    fn stack(first: &Id, rest: &[Id]) -> Layout {
        match rest.split_first() {
            None => leaf(first),
            Some((next, after)) => Layout::Split {
                dir: SplitDir::Col,
                ratio: (1000 / (rest.len() + 1)) as u16,
                a: Box::new(leaf(first)),
                b: Box::new(stack(next, after)),
            },
        }
    }
    let (first, rest) = ids.split_first()?;
    Some(match rest.split_first() {
        None => leaf(first),
        Some((next, after)) => Layout::Split {
            dir: SplitDir::Row,
            ratio: 500,
            a: Box::new(leaf(first)),
            b: Box::new(stack(next, after)),
        },
    })
}

/// The tree with `new` beside `beside`, which keeps the first side and the half.
pub fn split(tree: Layout, beside: &str, new: &str, dir: SplitDir) -> Layout {
    match tree {
        Layout::Pane { pane } if pane == beside => Layout::Split {
            dir,
            ratio: 500,
            a: Box::new(Layout::Pane { pane }),
            b: Box::new(leaf(new)),
        },
        Layout::Pane { .. } => tree,
        Layout::Split {
            dir: d,
            ratio,
            a,
            b,
        } => Layout::Split {
            dir: d,
            ratio,
            a: Box::new(split(*a, beside, new, dir)),
            b: Box::new(split(*b, beside, new, dir)),
        },
    }
}

/// The tree with the places of two panes exchanged.
pub fn swap(tree: Layout, a: &str, b: &str) -> Layout {
    match tree {
        Layout::Pane { pane } if pane == a => leaf(b),
        Layout::Pane { pane } if pane == b => leaf(a),
        Layout::Pane { .. } => tree,
        Layout::Split {
            dir,
            ratio,
            a: x,
            b: y,
        } => Layout::Split {
            dir,
            ratio,
            a: Box::new(swap(*x, a, b)),
            b: Box::new(swap(*y, a, b)),
        },
    }
}

/// What `grow` knows about a part of the tree.
#[derive(PartialEq, Eq)]
enum Seen {
    /// The pane is not in it.
    No,
    /// The pane is in it, and no split of the direction asked for has been moved yet.
    Inside,
    Done,
}

/// The tree with the nearest split of `dir` around `pane` moved so that the pane gets `by`
/// thousandths more of it (less when negative), kept within range; `None` when no split of
/// that direction is around the pane, and then the tree as it was.
pub fn grow(tree: Layout, pane: &str, dir: SplitDir, by: i16) -> Result<Layout, Layout> {
    fn go(tree: Layout, pane: &str, dir: SplitDir, by: i16) -> (Layout, Seen) {
        let Layout::Split {
            dir: d,
            ratio,
            a,
            b,
        } = tree
        else {
            let seen = if matches!(&tree, Layout::Pane { pane: p } if p == pane) {
                Seen::Inside
            } else {
                Seen::No
            };
            return (tree, seen);
        };
        let (a, in_a) = go(*a, pane, dir, by);
        let (b, in_b) = if in_a == Seen::No {
            go(*b, pane, dir, by)
        } else {
            (*b, Seen::No)
        };
        let (side_is_a, seen) = if in_a == Seen::No {
            (false, in_b)
        } else {
            (true, in_a)
        };
        let (seen, ratio) = match seen {
            // This split is around the pane and goes the way asked for: it is the nearest.
            Seen::Inside if d == dir => {
                let by = i32::from(if side_is_a { by } else { -by });
                let moved =
                    (i32::from(ratio) + by).clamp(i32::from(MIN_RATIO), i32::from(MAX_RATIO));
                (Seen::Done, moved as u16)
            }
            other => (other, ratio),
        };
        let tree = Layout::Split {
            dir: d,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        };
        (tree, seen)
    }
    match go(tree, pane, dir, by) {
        (tree, Seen::Done) => Ok(tree),
        (tree, _) => Err(tree),
    }
}

/// The tree without the panes that `keep` does not hold, and without a pane it has twice: a split
/// that loses a side is its other side, so a closed pane's room goes to its neighbour. Ratios are
/// brought into range.
fn prune(tree: Layout, keep: &HashSet<&str>, seen: &mut HashSet<String>) -> Option<Layout> {
    match tree {
        Layout::Pane { pane } => (keep.contains(pane.as_str()) && seen.insert(pane.clone()))
            .then_some(Layout::Pane { pane }),
        Layout::Split { dir, ratio, a, b } => {
            match (prune(*a, keep, seen), prune(*b, keep, seen)) {
                (Some(a), Some(b)) => Some(Layout::Split {
                    dir,
                    ratio: ratio.clamp(MIN_RATIO, MAX_RATIO),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (a, b) => a.or(b),
            }
        }
    }
}

/// Bring `tree` to agree with `ids`: none when there are no panes, the default one when there is
/// no tree, else the tree without the panes that are gone, with each that has none split off the
/// last one (beside it when it is alone, below it when it is not: how the default tree grows).
#[cfg(test)]
pub fn reconcile(tree: Option<Layout>, ids: &[Id]) -> Option<Layout> {
    reconcile_views(tree, ids, &[])
}

/// `reconcile` for a tree whose leaves are the panes `ids` and the views `views`: no tree when
/// there are no panes (a view needs a pane to sit beside), a pane without a place goes off the
/// last pane, and a view without one goes at the right edge with half.
pub fn reconcile_views(tree: Option<Layout>, ids: &[Id], views: &[Id]) -> Option<Layout> {
    if ids.is_empty() {
        return None;
    }
    let keep: HashSet<&str> = ids.iter().chain(views).map(String::as_str).collect();
    let panes: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let pruned = tree.and_then(|t| prune(t, &keep, &mut seen));
    let mut tree = match pruned {
        Some(t) if leaves(&t).iter().any(|l| panes.contains(l)) => t,
        _ => {
            // The default tree has every pane; only the views still have no place.
            seen = ids.iter().cloned().collect();
            default_tree(ids)?
        }
    };
    for id in ids.iter().filter(|id| !seen.contains(*id)) {
        let last = leaves(&tree)
            .into_iter()
            .rev()
            .find(|l| panes.contains(l))
            .map(str::to_owned);
        let dir = if matches!(tree, Layout::Pane { .. }) {
            SplitDir::Row
        } else {
            SplitDir::Col
        };
        if let Some(last) = last {
            tree = split(tree, &last, id, dir);
        }
    }
    for id in views.iter().filter(|id| !seen.contains(*id)) {
        tree = edge(tree, id, 500);
    }
    Some(tree)
}

/// The tree with `new` at its right edge, taking `share` thousandths of the whole width.
pub fn edge(tree: Layout, new: &str, share: u16) -> Layout {
    Layout::Split {
        dir: SplitDir::Row,
        ratio: 1000 - share.clamp(MIN_RATIO, MAX_RATIO),
        a: Box::new(tree),
        b: Box::new(leaf(new)),
    }
}

/// Which side of what it sits beside a leaf is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// Where a view was, so the next one of its kind can come back: the leaves of what took its room,
/// the side it was on, and its share in thousandths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub beside: Vec<String>,
    pub side: Side,
    pub share: u16,
}

/// Where `id` is: beside the leaves of its sibling. None when it is the whole tree or not in it.
pub fn place_of(tree: &Layout, id: &str) -> Option<Place> {
    let Layout::Split { dir, ratio, a, b } = tree else {
        return None;
    };
    let is = |n: &Layout| matches!(n, Layout::Pane { pane } if pane == id);
    let own = |sibling: &Layout, side: Side, share: u16| Place {
        beside: leaves(sibling).into_iter().map(str::to_owned).collect(),
        side,
        share,
    };
    if is(a) {
        let side = if *dir == SplitDir::Row {
            Side::Left
        } else {
            Side::Above
        };
        return Some(own(b, side, *ratio));
    }
    if is(b) {
        let side = if *dir == SplitDir::Row {
            Side::Right
        } else {
            Side::Below
        };
        return Some(own(a, side, 1000 - *ratio));
    }
    place_of(a, id).or_else(|| place_of(b, id))
}

/// The tree with `new` on `side` of the part of it whose leaves are exactly `beside`, taking
/// `share` thousandths of what the two make; the tree as it was, as an `Err`, when no part of it
/// has those leaves (the panes were swapped, closed or made since).
pub fn insert(
    tree: Layout,
    beside: &[String],
    new: &str,
    side: Side,
    share: u16,
) -> Result<Layout, Layout> {
    let want: HashSet<&str> = beside.iter().map(String::as_str).collect();
    fn go(tree: Layout, want: &HashSet<&str>, new: &str, side: Side, share: u16) -> (Layout, bool) {
        let have: HashSet<&str> = leaves(&tree).into_iter().collect();
        if &have == want {
            let share = share.clamp(MIN_RATIO, MAX_RATIO);
            let (dir, view_first) = match side {
                Side::Left => (SplitDir::Row, true),
                Side::Right => (SplitDir::Row, false),
                Side::Above => (SplitDir::Col, true),
                Side::Below => (SplitDir::Col, false),
            };
            let (a, b, ratio) = if view_first {
                (leaf(new), tree, share)
            } else {
                (tree, leaf(new), 1000 - share)
            };
            let tree = Layout::Split {
                dir,
                ratio,
                a: Box::new(a),
                b: Box::new(b),
            };
            return (tree, true);
        }
        match tree {
            Layout::Pane { .. } => (tree, false),
            Layout::Split { dir, ratio, a, b } => {
                let (a, done) = go(*a, want, new, side, share);
                let (b, done) = if done {
                    (*b, true)
                } else {
                    go(*b, want, new, side, share)
                };
                (
                    Layout::Split {
                        dir,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    },
                    done,
                )
            }
        }
    }
    match go(tree, &want, new, side, share) {
        (tree, true) => Ok(tree),
        (tree, false) => Err(tree),
    }
}

/// The tree without the leaves that are not in `keep`, a split that loses a side being its other.
pub fn retain(tree: Layout, keep: &[Id]) -> Option<Layout> {
    let keep: HashSet<&str> = keep.iter().map(String::as_str).collect();
    prune(tree, &keep, &mut HashSet::new())
}

/// Whether `reconcile` would leave the tree as it is.
#[cfg(test)]
pub fn agrees(tree: &Option<Layout>, ids: &[Id]) -> bool {
    agrees_views(tree, ids, &[])
}

/// `agrees` for a tree of panes `ids` and views `views`.
pub fn agrees_views(tree: &Option<Layout>, ids: &[Id], views: &[Id]) -> bool {
    match tree {
        None => ids.is_empty(),
        Some(t) => {
            let mut have = leaves(t);
            let mut want: Vec<&str> = ids.iter().chain(views).map(String::as_str).collect();
            have.sort_unstable();
            want.sort_unstable();
            have == want && ratios_in_range(t)
        }
    }
}

fn ratios_in_range(tree: &Layout) -> bool {
    match tree {
        Layout::Pane { .. } => true,
        Layout::Split { ratio, a, b, .. } => {
            (MIN_RATIO..=MAX_RATIO).contains(ratio) && ratios_in_range(a) && ratios_in_range(b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<Id> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    fn row(a: Layout, b: Layout) -> Layout {
        Layout::Split {
            dir: SplitDir::Row,
            ratio: 500,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    #[test]
    fn the_default_is_the_first_on_the_left_and_the_rest_stacked_with_even_shares() {
        assert_eq!(default_tree(&[]), None);
        assert_eq!(default_tree(&ids(&["a"])), Some(leaf("a")));
        assert_eq!(
            default_tree(&ids(&["a", "b"])),
            Some(row(leaf("a"), leaf("b")))
        );
        let three = default_tree(&ids(&["a", "b", "c"])).unwrap();
        assert_eq!(
            three,
            row(
                leaf("a"),
                Layout::Split {
                    dir: SplitDir::Col,
                    ratio: 500,
                    a: Box::new(leaf("b")),
                    b: Box::new(leaf("c")),
                }
            )
        );
        let four = default_tree(&ids(&["a", "b", "c", "d"])).unwrap();
        let Layout::Split { b, .. } = four else {
            panic!("a split")
        };
        let Layout::Split { ratio, .. } = *b else {
            panic!("a stack")
        };
        assert_eq!(ratio, 333, "a third of the stack for the first of three");
    }

    #[test]
    fn a_split_puts_the_new_pane_beside_the_one_asked_for_whatever_its_depth() {
        let t = row(leaf("a"), leaf("b"));
        let t = split(t, "b", "c", SplitDir::Col);
        assert_eq!(leaves(&t), ["a", "b", "c"]);
        let Layout::Split { b, .. } = &t else {
            panic!("a split")
        };
        assert!(matches!(
            **b,
            Layout::Split {
                dir: SplitDir::Col,
                ratio: 500,
                ..
            }
        ));
        assert_eq!(
            split(leaf("a"), "zz", "c", SplitDir::Row),
            leaf("a"),
            "none to split"
        );
    }

    #[test]
    fn a_closed_panes_room_goes_to_its_neighbour() {
        let t = split(row(leaf("a"), leaf("b")), "b", "c", SplitDir::Col);
        let after = reconcile(Some(t), &ids(&["a", "c"])).unwrap();
        assert_eq!(after, row(leaf("a"), leaf("c")));
        assert_eq!(reconcile(Some(after), &ids(&["c"])), Some(leaf("c")));
        assert_eq!(reconcile(Some(leaf("a")), &[]), None);
    }

    #[test]
    fn reconcile_adds_the_panes_that_have_no_place_the_way_the_default_grows() {
        assert_eq!(
            reconcile(None, &ids(&["a", "b"])),
            Some(row(leaf("a"), leaf("b")))
        );
        let one = reconcile(Some(leaf("a")), &ids(&["a", "b"])).unwrap();
        assert_eq!(one, row(leaf("a"), leaf("b")));
        let two = reconcile(Some(one), &ids(&["a", "b", "c"])).unwrap();
        assert_eq!(leaves(&two), ["a", "b", "c"]);
        let Layout::Split { b, .. } = &two else {
            panic!("a split")
        };
        assert!(
            matches!(
                **b,
                Layout::Split {
                    dir: SplitDir::Col,
                    ..
                }
            ),
            "below the last"
        );
    }

    #[test]
    fn a_pane_the_tree_has_twice_or_does_not_know_is_not_kept_and_ratios_come_into_range() {
        let t = Layout::Split {
            dir: SplitDir::Row,
            ratio: 5,
            a: Box::new(leaf("a")),
            b: Box::new(row(leaf("a"), leaf("ghost"))),
        };
        assert!(!agrees(&Some(t.clone()), &ids(&["a"])));
        assert_eq!(reconcile(Some(t), &ids(&["a"])), Some(leaf("a")));
        let wide = Layout::Split {
            dir: SplitDir::Col,
            ratio: 990,
            a: Box::new(leaf("a")),
            b: Box::new(leaf("b")),
        };
        assert!(!agrees(&Some(wide.clone()), &ids(&["a", "b"])));
        let Some(Layout::Split { ratio, .. }) = reconcile(Some(wide), &ids(&["a", "b"])) else {
            panic!("a split")
        };
        assert_eq!(ratio, MAX_RATIO);
    }

    #[test]
    fn a_swap_exchanges_two_places_and_keeps_the_shape_and_the_shares() {
        let t = split(row(leaf("a"), leaf("b")), "b", "c", SplitDir::Col);
        let s = swap(t.clone(), "a", "c");
        assert_eq!(leaves(&s), ["c", "b", "a"]);
        assert_eq!(swap(s, "a", "c"), t, "swapping back");
        assert_eq!(swap(t.clone(), "a", "zz"), leaf_replaced(&t, "a", "zz"));
    }

    /// `t` with the pane `from` called `to`, which is what a swap with a pane that is not there does.
    fn leaf_replaced(t: &Layout, from: &str, to: &str) -> Layout {
        match t {
            Layout::Pane { pane } if pane == from => leaf(to),
            Layout::Pane { .. } => t.clone(),
            Layout::Split { dir, ratio, a, b } => Layout::Split {
                dir: *dir,
                ratio: *ratio,
                a: Box::new(leaf_replaced(a, from, to)),
                b: Box::new(leaf_replaced(b, from, to)),
            },
        }
    }

    fn ratio_of(tree: &Layout, path: &[bool]) -> u16 {
        match (tree, path.split_first()) {
            (Layout::Split { ratio, .. }, None) => *ratio,
            (Layout::Split { a, b, .. }, Some((first, rest))) => {
                ratio_of(if *first { a } else { b }, rest)
            }
            _ => panic!("no split there"),
        }
    }

    #[test]
    fn a_pane_grows_by_moving_the_nearest_split_of_the_direction_toward_the_other_side() {
        // `a` on the left, `b` over `c` on the right.
        let t = split(row(leaf("a"), leaf("b")), "b", "c", SplitDir::Col);
        let wider = grow(t.clone(), "a", SplitDir::Row, 50).unwrap();
        assert_eq!(
            ratio_of(&wider, &[]),
            550,
            "`a` is the first side: its share grows"
        );
        let wider_b = grow(t.clone(), "b", SplitDir::Row, 50).unwrap();
        assert_eq!(ratio_of(&wider_b, &[]), 450, "`b` is in the second side");
        let taller = grow(t.clone(), "c", SplitDir::Col, 50).unwrap();
        assert_eq!(
            ratio_of(&taller, &[]),
            500,
            "the outer split is not touched"
        );
        assert_eq!(ratio_of(&taller, &[false]), 450, "the one around `c` is");
        let lower = grow(t.clone(), "b", SplitDir::Col, -50).unwrap();
        assert_eq!(ratio_of(&lower, &[false]), 450);
        assert_eq!(
            grow(t.clone(), "a", SplitDir::Col, 50),
            Err(t.clone()),
            "none of that direction around `a`"
        );
        assert_eq!(
            grow(t.clone(), "zz", SplitDir::Row, 50),
            Err(t),
            "not a pane of the tree"
        );
        assert_eq!(
            grow(leaf("a"), "a", SplitDir::Row, 50),
            Err(leaf("a")),
            "a lone pane has no split"
        );
    }

    #[test]
    fn growing_stops_at_the_least_and_the_most() {
        let t = row(leaf("a"), leaf("b"));
        let mut t = t;
        for _ in 0..20 {
            t = grow(t, "a", SplitDir::Row, 50).unwrap_or_else(|t| t);
        }
        assert_eq!(ratio_of(&t, &[]), MAX_RATIO);
        for _ in 0..40 {
            t = grow(t, "a", SplitDir::Row, -50).unwrap_or_else(|t| t);
        }
        assert_eq!(ratio_of(&t, &[]), MIN_RATIO);
    }

    #[test]
    fn a_tree_that_agrees_is_left_as_it_is() {
        let t = row(leaf("b"), leaf("a"));
        assert!(agrees(&Some(t.clone()), &ids(&["a", "b"])));
        assert_eq!(reconcile(Some(t.clone()), &ids(&["a", "b"])), Some(t));
        assert!(agrees(&None, &[]));
        assert!(!agrees(&None, &ids(&["a"])));
    }

    fn col(a: Layout, b: Layout) -> Layout {
        Layout::Split {
            dir: SplitDir::Col,
            ratio: 500,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    #[test]
    fn a_view_survives_reconcile_and_a_pane_without_a_place_goes_beside_the_last_pane() {
        // shell | claude | diff: the diff is the last leaf, and a new pane must not cut it.
        let tree = Some(edge(row(leaf("a"), leaf("b")), "diff", 500));
        let kept = reconcile_views(tree.clone(), &ids(&["a", "b"]), &ids(&["diff"])).unwrap();
        assert_eq!(Some(kept.clone()), tree);
        let grown = reconcile_views(tree, &ids(&["a", "b", "c"]), &ids(&["diff"])).unwrap();
        assert_eq!(
            grown,
            edge(row(leaf("a"), col(leaf("b"), leaf("c"))), "diff", 500)
        );
        assert!(agrees_views(
            &Some(grown),
            &ids(&["a", "b", "c"]),
            &ids(&["diff"])
        ));
    }

    #[test]
    fn a_view_with_no_place_goes_at_the_edge_and_no_pane_means_no_tree() {
        let tree = reconcile_views(Some(leaf("a")), &ids(&["a"]), &ids(&["v"])).unwrap();
        assert_eq!(tree, edge(leaf("a"), "v", 500));
        assert_eq!(reconcile_views(Some(tree), &[], &ids(&["v"])), None);
        // a tree of views alone is made again from the panes
        let only = reconcile_views(Some(leaf("v")), &ids(&["a"]), &ids(&["v"])).unwrap();
        assert_eq!(only, edge(leaf("a"), "v", 500));
    }

    #[test]
    fn edge_takes_the_share_asked_for_in_range() {
        assert_eq!(
            edge(leaf("a"), "v", 300),
            Layout::Split {
                dir: SplitDir::Row,
                ratio: 700,
                a: Box::new(leaf("a")),
                b: Box::new(leaf("v")),
            }
        );
        let Layout::Split { ratio, .. } = edge(leaf("a"), "v", 0) else {
            panic!("a split")
        };
        assert_eq!(ratio, 900);
    }

    #[test]
    fn a_place_is_the_whole_sibling_so_a_view_comes_back_where_it_was() {
        // diff at the edge of shell | claude, closed, reopened: the tree is the same.
        let before = edge(row(leaf("a"), leaf("b")), "d", 400);
        let place = place_of(&before, "d").unwrap();
        assert_eq!(place.beside, ["a", "b"]);
        assert_eq!(place.side, Side::Right);
        assert_eq!(place.share, 400);
        let closed = retain(before.clone(), &ids(&["a", "b"])).unwrap();
        assert_eq!(closed, row(leaf("a"), leaf("b")));
        let back = insert(closed, &place.beside, "d", place.side, place.share).unwrap();
        assert_eq!(back, before);
    }

    #[test]
    fn a_place_on_the_left_or_below_keeps_its_side_and_share() {
        let before = row(leaf("d"), col(leaf("a"), leaf("b")));
        let place = place_of(&before, "d").unwrap();
        assert_eq!((place.side, place.share), (Side::Left, 500));
        let before = col(row(leaf("a"), leaf("b")), leaf("d"));
        let place = place_of(&before, "d").unwrap();
        assert_eq!((place.beside, place.side), (ids(&["a", "b"]), Side::Below));
        assert_eq!(place_of(&leaf("d"), "d"), None);
        assert_eq!(place_of(&leaf("a"), "d"), None);
    }

    #[test]
    fn a_place_whose_neighbours_are_gone_does_not_insert() {
        let tree = row(leaf("a"), leaf("b"));
        let err = insert(tree.clone(), &ids(&["a", "z"]), "d", Side::Right, 500).unwrap_err();
        assert_eq!(err, tree);
        // the order the leaves are listed in does not matter, and a deeper part is found
        let deep = col(leaf("x"), row(leaf("a"), leaf("b")));
        let got = insert(deep, &ids(&["b", "a"]), "d", Side::Below, 250).unwrap();
        let Layout::Split { b, .. } = got else {
            panic!("a split")
        };
        let Layout::Split { dir, ratio, .. } = *b else {
            panic!("a split")
        };
        assert_eq!((dir, ratio), (SplitDir::Col, 750));
    }

    #[test]
    fn retain_drops_the_leaves_it_is_not_given_and_the_neighbour_takes_the_room() {
        let tree = edge(row(leaf("a"), leaf("b")), "d", 500);
        assert_eq!(
            retain(tree.clone(), &ids(&["a", "b"])),
            Some(row(leaf("a"), leaf("b")))
        );
        assert_eq!(retain(tree, &ids(&["d"])), Some(leaf("d")));
    }
}
