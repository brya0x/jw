//! How the panes of a workspace are laid out (spec REQ-286 to REQ-291): a tree of splits whose
//! leaves are exactly the workspace's panes. The tree is the daemon's, kept in `session.json`, so
//! every client draws the same arrangement and it survives a restart.
//!
//! Only the shape is kept here. Panes come and go in many places (a new one, a closed one, one
//! whose shell would not start after a restart), so `reconcile` brings any tree to agree with the
//! panes there are instead of every place keeping it up to date.

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
    fn stack(ids: &[Id]) -> Layout {
        match ids {
            [one] => leaf(one),
            [first, rest @ ..] => Layout::Split {
                dir: SplitDir::Col,
                ratio: (1000 / ids.len()) as u16,
                a: Box::new(leaf(first)),
                b: Box::new(stack(rest)),
            },
            [] => unreachable!("a stack has a pane"),
        }
    }
    match ids {
        [] => None,
        [one] => Some(leaf(one)),
        [first, rest @ ..] => Some(Layout::Split {
            dir: SplitDir::Row,
            ratio: 500,
            a: Box::new(leaf(first)),
            b: Box::new(stack(rest)),
        }),
    }
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
pub fn reconcile(tree: Option<Layout>, ids: &[Id]) -> Option<Layout> {
    if ids.is_empty() {
        return None;
    }
    let keep: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let Some(mut tree) = tree.and_then(|t| prune(t, &keep, &mut seen)) else {
        return default_tree(ids);
    };
    for id in ids.iter().filter(|id| !seen.contains(*id)) {
        let last = leaves(&tree).last().map(|l| (*l).to_owned());
        let dir = if matches!(tree, Layout::Pane { .. }) {
            SplitDir::Row
        } else {
            SplitDir::Col
        };
        if let Some(last) = last {
            tree = split(tree, &last, id, dir);
        }
    }
    Some(tree)
}

/// Whether `reconcile` would leave the tree as it is.
pub fn agrees(tree: &Option<Layout>, ids: &[Id]) -> bool {
    match tree {
        None => ids.is_empty(),
        Some(t) => {
            let mut have = leaves(t);
            let mut want: Vec<&str> = ids.iter().map(String::as_str).collect();
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
    fn a_tree_that_agrees_is_left_as_it_is() {
        let t = row(leaf("b"), leaf("a"));
        assert!(agrees(&Some(t.clone()), &ids(&["a", "b"])));
        assert_eq!(reconcile(Some(t.clone()), &ids(&["a", "b"])), Some(t));
        assert!(agrees(&None, &[]));
        assert!(!agrees(&None, &ids(&["a"])));
    }
}
