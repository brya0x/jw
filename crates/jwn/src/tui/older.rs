//! The items of a conversation that came before the ones its node holds (spec REQ-567, REQ-568,
//! prototype `docs/prototypes/older-items.html`): the terminal asks the daemon for them when the
//! list is scrolled to the top, keeps them beside the node's window and draws the two as one list.
//!
//! The list the rest of the client sees is the *effective* conversation: `items` is the older
//! items followed by the node's, and `first` is `base`, the number of the first of them (the
//! number of an item that is in the node's window is its own plus the older items that were
//! asked for, which keeps the number of each item while more are loaded). When
//! the node's window slides (each new item pushes one out of its head and `first` grows), the
//! items that left the window go to the end of the older ones, so the two stay in a row.

use std::cell::Cell;

use contract::conversation::{Conversation, Item, OlderItems};

/// How many items one request asks for.
pub(super) const PAGE: u32 = 100;

/// The most older items a conversation keeps here: what is older is left in Claude's file.
const CAP: usize = 3000;

/// The `item` of the row that says what is above the first item: not the number of any.
pub(super) const TOP: u32 = u32::MAX;

/// Where to ask for what comes before the ones held.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Nothing was loaded yet: the node says where.
    #[default]
    Node,
    /// The last answer says where; none when the start of the file was reached.
    At(Option<u64>),
}

#[derive(Default)]
pub(super) struct Older {
    /// The older items, oldest first, and the number of the first.
    items: Vec<Item>,
    base: u32,
    /// The `first` the node says, when the window starts right after the older items: it grows as
    /// the window slides, and what the node says is not what the items are numbered by, which is
    /// `base` and on.
    anchor: u32,
    /// The daemon's epoch the items were read in: another one renumbers the conversation.
    epoch: String,
    next: Next,
    /// The `before` that is being asked for, when a request is out.
    busy: Option<u64>,
    failed: bool,
    /// The list was scrolled to its top when it was drawn last.
    at_top: Cell<bool>,
}

/// What the row above the first item says.
pub(super) enum Row {
    Hint,
    Loading,
    Failed,
    Start,
    Capped,
}

impl Row {
    pub(super) fn text(&self) -> &'static str {
        match self {
            Row::Hint => "▲ hay anteriores · sube para cargar",
            Row::Loading => "✻ cargando anteriores…",
            Row::Failed => "✗ no se pudo leer el archivo · pulsa para reintentar",
            Row::Start => "── inicio de la conversación ──",
            Row::Capped => "── más antiguo en el archivo de Claude ──",
        }
    }
}

/// Whether two items are the same one, though what is in it may have grown or ended.
fn same(a: &Item, b: &Item) -> bool {
    match (a, b) {
        (Item::Tool { id: x, .. }, Item::Tool { id: y, .. })
        | (Item::Agent { id: x, .. }, Item::Agent { id: y, .. }) => x == y,
        // What is not a tool cannot change once it is there, but a text that arrives grows.
        (Item::Text { .. }, Item::Text { .. }) => true,
        _ => a == b,
    }
}

impl Older {
    /// How many older items are held.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.items.len()
    }

    fn reset(&mut self) {
        *self = Older {
            at_top: self.at_top.clone(),
            ..Older::default()
        };
    }

    /// The numbers start from `first`: nothing older is held.
    fn start_at(&mut self, first: u32) {
        self.base = first;
        self.anchor = first;
    }

    /// The older items are dropped and the node is drawn as it comes.
    fn start_again(&mut self, c: Conversation, epoch: &str) -> Conversation {
        self.reset();
        epoch.clone_into(&mut self.epoch);
        self.start_at(c.first);
        c
    }

    /// The conversation as the client draws it: the node `c` with the older items before its
    /// own. `prev` is what was drawn before, to take what left the head of the window.
    pub(super) fn merged(
        &mut self,
        prev: Option<&Conversation>,
        mut c: Conversation,
        epoch: &str,
    ) -> Conversation {
        if self.epoch != epoch {
            self.reset();
            epoch.clone_into(&mut self.epoch);
        }
        if self.items.is_empty() {
            self.start_at(c.first);
            return c;
        }
        let expected = self.anchor;
        let window = prev.map_or(&[][..], |p| p.items.get(self.items.len()..).unwrap_or(&[]));
        if c.first > expected {
            let d = (c.first - expected) as usize;
            let joined = d <= window.len()
                && match (c.items.first(), window.get(d)) {
                    (Some(now), Some(then)) => same(now, then),
                    (None, None) => true,
                    _ => false,
                };
            if joined {
                self.items.extend_from_slice(&window[..d]);
                self.anchor = c.first;
            } else {
                return self.start_again(c, epoch);
            }
        } else if c.first < expected {
            return self.start_again(c, epoch);
        }
        let mut all = self.items.clone();
        all.append(&mut c.items);
        c.items = all;
        c.first = self.base;
        c
    }

    /// Whether the list wants older items now, and the time to ask with: it is at its top, the
    /// conversation has a transcript, nothing is being asked and the last try did not fail.
    pub(super) fn ask(&mut self, conv: &Conversation) -> Option<u64> {
        if !self.at_top.get() || self.busy.is_some() || self.failed || self.items.len() >= CAP {
            return None;
        }
        let before = match self.next {
            Next::Node => conv.older?,
            Next::At(at) => at?,
        };
        self.busy = Some(before);
        Some(before)
    }

    /// A tap on the row of a failed try asks again.
    pub(super) fn retry(&mut self) {
        self.failed = false;
    }

    /// The answer to the request with `before`, put in `conv`: whether it was taken, and how many
    /// items it added before the others. An answer for a request that is not the last one is
    /// not.
    pub(super) fn replied(
        &mut self,
        before: u64,
        page: Option<OlderItems>,
        conv: &mut Conversation,
    ) -> Option<usize> {
        if self.busy != Some(before) {
            return None;
        }
        self.busy = None;
        self.at_top.set(false);
        let Some(mut page) = page else {
            self.failed = true;
            return Some(0);
        };
        let held = self.items.len();
        if held == 0 {
            self.start_at(conv.first);
        }
        // What would go past the cap is the oldest of the page.
        let room = CAP.saturating_sub(held);
        let over = page.items.len().saturating_sub(room);
        page.items.drain(..over);
        let m = page.items.len();
        // The cursor must go back; an answer that does not would be asked again for ever.
        self.next = Next::At(page.older.filter(|at| *at < before));
        let window = conv.items.split_off(held.min(conv.items.len()));
        let mut all = page.items;
        all.append(&mut self.items);
        self.items = all;
        let mut items = self.items.clone();
        items.extend(window);
        conv.items = items;
        conv.first = self.base;
        Some(m)
    }

    /// The row above the first item, when the conversation has older ones to say something of.
    pub(super) fn row(&self, conv: &Conversation) -> Option<Row> {
        conv.older?;
        Some(if self.busy.is_some() {
            Row::Loading
        } else if self.failed {
            Row::Failed
        } else if self.items.len() >= CAP {
            Row::Capped
        } else if matches!(self.next, Next::At(None)) {
            Row::Start
        } else {
            Row::Hint
        })
    }

    pub(super) fn set_at_top(&self, at: bool) {
        self.at_top.set(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(t: &str) -> Item {
        Item::Note {
            text: t.into(),
            tone: None,
        }
    }

    fn conv(first: u32, items: &[&str]) -> Conversation {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-conversation-running.json")).unwrap(),
        )
        .unwrap();
        let mut c: Conversation = serde_json::from_value(doc["node"].clone()).unwrap();
        c.first = first;
        c.items = items.iter().map(|t| note(t)).collect();
        c.older = Some(5);
        c
    }

    fn names(c: &Conversation) -> Vec<String> {
        c.items
            .iter()
            .map(|i| match i {
                Item::Note { text, .. } => text.clone(),
                _ => "?".into(),
            })
            .collect()
    }

    fn page(items: &[&str], older: Option<u64>) -> OlderItems {
        OlderItems {
            items: items.iter().map(|t| note(t)).collect(),
            older,
        }
    }

    /// The older items go before the node's, and what leaves the head of the window as the
    /// conversation goes on goes to the end of the older ones, so nothing is lost or twice.
    #[test]
    fn the_older_items_and_what_leaves_the_window_stay_in_a_row() {
        let mut o = Older::default();
        let c = o.merged(None, conv(10, &["d", "e", "f"]), "e1");
        let mut eff = c.clone();
        o.set_at_top(true);
        assert_eq!(o.ask(&eff), Some(5));
        assert_eq!(
            o.replied(5, Some(page(&["a", "b", "c"], Some(3))), &mut eff),
            Some(3)
        );
        assert_eq!(names(&eff), ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(eff.first, 10, "`first` is the number of the first of them");
        // two items come and two leave the window
        let next = o.merged(Some(&eff), conv(12, &["f", "g", "h"]), "e1");
        assert_eq!(names(&next), ["a", "b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(next.first, 10);
        // a node that starts somewhere else is another conversation
        let other = o.merged(Some(&next), conv(2, &["x"]), "e1");
        assert_eq!(names(&other), ["x"]);
        assert_eq!(o.len(), 0);
    }

    #[test]
    fn a_window_that_does_not_join_the_older_items_starts_them_again() {
        let mut o = Older::default();
        let mut eff = o.merged(None, conv(10, &["d"]), "e1");
        o.set_at_top(true);
        o.ask(&eff);
        o.replied(5, Some(page(&["c"], None)), &mut eff);
        // `first` jumped further than the window held: a gap
        let after = o.merged(Some(&eff), conv(40, &["z"]), "e1");
        assert_eq!(names(&after), ["z"]);
        assert_eq!(o.len(), 0);
        // another epoch also starts again
        let mut o = Older::default();
        let mut eff = o.merged(None, conv(10, &["d"]), "e1");
        o.set_at_top(true);
        o.ask(&eff);
        o.replied(5, Some(page(&["c"], None)), &mut eff);
        let after = o.merged(Some(&eff), conv(10, &["d"]), "e2");
        assert_eq!(names(&after), ["d"]);
    }

    #[test]
    fn one_request_at_a_time_a_stale_answer_is_dropped_and_a_failure_waits_for_a_tap() {
        let mut o = Older::default();
        let mut eff = o.merged(None, conv(10, &["d"]), "e1");
        assert_eq!(o.ask(&eff), None, "not at the top yet");
        o.set_at_top(true);
        assert_eq!(o.ask(&eff), Some(5));
        assert_eq!(o.ask(&eff), None, "one is out");
        assert_eq!(
            o.replied(4, Some(page(&["x"], None)), &mut eff),
            None,
            "not the one asked"
        );
        assert_eq!(o.replied(5, None, &mut eff), Some(0));
        assert!(matches!(o.row(&eff), Some(Row::Failed)));
        o.set_at_top(true);
        assert_eq!(o.ask(&eff), None, "a failure waits");
        o.retry();
        assert_eq!(o.ask(&eff), Some(5));
    }

    #[test]
    fn the_cursor_goes_back_and_the_start_of_the_file_ends_the_asking() {
        let mut o = Older::default();
        let mut eff = o.merged(None, conv(10, &["d"]), "e1");
        o.set_at_top(true);
        assert_eq!(o.ask(&eff), Some(5));
        // an answer that points forward is taken as the end: asking again would not stop
        o.replied(5, Some(page(&["c"], Some(9))), &mut eff);
        o.set_at_top(true);
        assert_eq!(o.ask(&eff), None);
        assert!(matches!(o.row(&eff), Some(Row::Start)));
        eff.older = None;
        assert!(o.row(&eff).is_none(), "no transcript, no row");
    }

    /// A page that would go past the cap is cut at its oldest end, and what is held stops there.
    #[test]
    fn a_page_that_would_pass_the_cap_is_cut_at_its_oldest_end() {
        let mut o = Older::default();
        let mut eff = o.merged(None, conv(10, &["d"]), "e1");
        o.set_at_top(true);
        o.ask(&eff);
        let many: Vec<String> = (0..CAP + 50).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            o.replied(5, Some(page(&refs, Some(3))), &mut eff),
            Some(CAP)
        );
        assert_eq!(o.len(), CAP);
        assert_eq!(names(&eff).last().map(String::as_str), Some("d"));
        assert_eq!(names(&eff)[0], "n50", "the oldest of the page went");
        assert!(matches!(o.row(&eff), Some(Row::Capped)));
    }
}
