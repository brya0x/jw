//! The diff in the terminal client (prototype: `docs/prototypes/tui-diff.html`). It takes the
//! panes' area of the highlighted workspace: three views (Sin commitear, Commits, General), a
//! tree of the changed files on the left and every file of the view in one scroll on the
//! right, side by side when the body is wide. It holds only what the person is doing (which
//! view, where the scroll is); the files come from the daemon's `diff` and `commits` nodes,
//! and what it wants from the daemon it queues as `Ask`s for the loop to send.

use std::collections::{HashMap, HashSet};

use contract::{Commits, Diff, DiffScope, DiffState, FileStatus, Seen};
use crossterm::event::KeyCode;

use super::diff_lines::{
    Kind, Line, Row, Tone, TreeRow, ago, build, clean, order as tree_order, tones, tree_rows,
};
use super::{Grid, SIDEBAR, Style, clip, look};

/// Width of the tree column.
const TREE: usize = 30;
/// Under this many columns for the whole area the tree hides, and `tab` shows it over the body.
const TREE_MIN_AREA: usize = 84;
/// Body width (without its scrollbar) from which `auto` draws two columns.
const SPLIT_MIN: usize = 100;
/// The least body width the person can force two columns on.
const SPLIT_FORCED_MIN: usize = 60;
const WHEEL: usize = 3;
/// How long `actualizado` stays in the title after the daemon patched the node.
const FLASH_MS: u64 = 2500;

/// What the viewer needs from the documents the client holds.
pub struct Docs<'a> {
    pub diffs: &'a HashMap<String, Diff>,
    pub commits: &'a HashMap<String, Commits>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Uncommitted,
    Commits,
    General,
}

impl Tab {
    /// The node a tab shows before anything is chosen inside it.
    fn root(self) -> Src {
        match self {
            Tab::Uncommitted => Src::Uncommitted,
            Tab::Commits => Src::Commits,
            Tab::General => Src::General,
        }
    }
}

/// Where a node of this viewer comes from; the loop hands the answer back by it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Src {
    Uncommitted,
    General,
    Commits,
    Commit(String),
}

/// What the viewer wants the daemon to do. The loop runs them; `View` stays pure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    OpenDiff {
        src: Src,
        workspace: String,
        scope: DiffScope,
        commit: Option<String>,
    },
    OpenCommits {
        workspace: String,
    },
    /// Open a node to be patched.
    Watch(String),
    /// Read a node again now.
    Refresh(String),
    SetViewed {
        node: String,
        path: String,
        viewed: bool,
    },
    Close(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Body,
    Tree,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Split {
    Auto,
    One,
    Two,
}

#[derive(Default)]
struct Slot {
    node: Option<String>,
    error: Option<String>,
}

/// The lines of the diff being shown, rebuilt only when its document or the layout changes.
struct Cache {
    key: (String, u64, bool),
    order: Vec<usize>,
    lines: Vec<Line>,
}

/// After marking a file, go to this one once the daemon has patched the node.
struct Jump {
    path: String,
    after_rev: u64,
}

pub struct Changes {
    workspace: String,
    tab: Tab,
    /// The commit open in the Commits view.
    commit: Option<String>,
    slots: HashMap<Src, Slot>,
    focus: Focus,
    row: usize,
    shut: HashSet<String>,
    split: Split,
    scroll: usize,
    /// The file the keys act on, as a place in the tree order: the one `j` and `k` moved to,
    /// else the one at the top of the body.
    cur: usize,
    cache: Option<Cache>,
    jump: Option<Jump>,
    asks: Vec<Ask>,
    msg: String,
    /// The revision of the uncommitted node last seen, and until when to say it changed.
    live_rev: Option<u64>,
    live_until: u64,
    /// A mark was sent: the next patch is its echo, not a change of the files.
    own_patch: bool,
}

/// Whether the viewer stays open after a key.
#[derive(PartialEq, Eq, Debug)]
pub enum Outcome {
    Stay,
    Close,
}

/// What an item of the left column is.
enum Item {
    Back,
    Commit(usize),
    Tree(TreeRow),
}

#[derive(Clone, Copy)]
struct Geom {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    ix: usize,
    iw: usize,
    tree_on: bool,
    bx: usize,
    bw: usize,
    top: usize,
    rows: usize,
    split: bool,
}

fn geom((cols, rows): (usize, usize), mode: Split) -> Geom {
    let (x, y) = (SIDEBAR + 1, 1);
    let (w, h) = (cols.saturating_sub(x), rows.saturating_sub(2));
    let (ix, iw) = (x + 1, w.saturating_sub(2));
    let tree_on = w >= TREE_MIN_AREA;
    let (bx, bw) = if tree_on {
        (ix + TREE + 1, iw.saturating_sub(TREE + 1))
    } else {
        (ix, iw)
    };
    let body = bw.saturating_sub(1);
    Geom {
        x,
        y,
        w,
        h,
        ix,
        iw,
        tree_on,
        bx,
        bw,
        top: y + 2,
        rows: h.saturating_sub(3),
        split: match mode {
            Split::One => false,
            Split::Two => body >= SPLIT_FORCED_MIN,
            Split::Auto => body >= SPLIT_MIN,
        },
    }
}

impl Changes {
    /// Open the viewer on a workspace: it asks for the uncommitted changes.
    pub fn open(workspace: &str) -> Changes {
        let mut c = Changes {
            workspace: workspace.to_owned(),
            tab: Tab::Uncommitted,
            commit: None,
            slots: HashMap::new(),
            focus: Focus::Body,
            row: 0,
            shut: HashSet::new(),
            split: Split::Auto,
            scroll: 0,
            cur: 0,
            cache: None,
            jump: None,
            asks: Vec::new(),
            msg: String::new(),
            live_rev: None,
            live_until: 0,
            own_patch: false,
        };
        c.want(Src::Uncommitted);
        c
    }

    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// What was asked since the last call.
    pub fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// Close the viewer: stop watching its nodes, so the daemon stops reading git for them.
    pub fn close(mut self) -> Vec<Ask> {
        let open = self.slots.values().filter_map(|s| s.node.clone());
        self.asks.extend(open.map(Ask::Close));
        self.asks
    }

    /// The daemon made the node for `src`.
    pub fn made(&mut self, src: Src, node: &str) {
        let slot = self.slots.entry(src).or_default();
        slot.node = Some(node.to_owned());
        slot.error = None;
        self.asks.push(Ask::Watch(node.to_owned()));
    }

    /// The daemon could not make the node for `src`.
    pub fn failed(&mut self, src: Src, why: &str) {
        self.slots.entry(src).or_default().error = Some(why.to_owned());
    }

    pub fn flash(&mut self, text: &str) {
        self.msg = text.to_owned();
    }

    /// Ask for the node of `src` unless it was asked for.
    fn want(&mut self, src: Src) {
        if self.slots.contains_key(&src) {
            return;
        }
        self.slots.insert(src.clone(), Slot::default());
        let workspace = self.workspace.clone();
        self.asks.push(match src {
            Src::Uncommitted | Src::General => Ask::OpenDiff {
                scope: if src == Src::General {
                    DiffScope::General
                } else {
                    DiffScope::Uncommitted
                },
                src,
                workspace,
                commit: None,
            },
            Src::Commits => Ask::OpenCommits { workspace },
            Src::Commit(sha) => Ask::OpenDiff {
                src: Src::Commit(sha.clone()),
                workspace,
                scope: DiffScope::Commit,
                commit: Some(sha),
            },
        });
    }

    fn src(&self) -> Src {
        match (self.tab, &self.commit) {
            (Tab::Uncommitted, _) => Src::Uncommitted,
            (Tab::General, _) => Src::General,
            (Tab::Commits, None) => Src::Commits,
            (Tab::Commits, Some(sha)) => Src::Commit(sha.clone()),
        }
    }

    fn node(&self, src: &Src) -> Option<&str> {
        self.slots.get(src)?.node.as_deref()
    }

    fn diff<'a>(&self, docs: &Docs<'a>) -> Option<&'a Diff> {
        docs.diffs.get(self.node(&self.src())?)
    }

    fn list<'a>(&self, docs: &Docs<'a>) -> Option<&'a Commits> {
        docs.commits.get(self.node(&Src::Commits)?)
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.commit = None;
        self.scroll = 0;
        self.cur = 0;
        self.row = 0;
        // The list of commits lives in the left column, so that is where the keys go.
        self.focus = if tab == Tab::Commits {
            Focus::Tree
        } else {
            Focus::Body
        };
        let src = self.src();
        self.want(src);
    }

    /// The documents changed, or the terminal did: rebuild what is drawn, follow a mark to the
    /// next file, and keep the scroll inside the body.
    pub fn update(&mut self, docs: &Docs, size: (usize, usize), now_ms: u64) {
        let g = geom(size, self.split);
        let Some(d) = self.diff(docs) else {
            self.cache = None;
            self.scroll = 0;
            return;
        };
        if self.src() == Src::Uncommitted {
            if self.live_rev.is_some_and(|r| r != d.head.rev) && !self.own_patch {
                self.live_until = now_ms + FLASH_MS;
            }
            if self.live_rev != Some(d.head.rev) {
                self.own_patch = false;
            }
            self.live_rev = Some(d.head.rev);
        }
        let key = (d.head.id.clone(), d.head.rev, g.split);
        if self.cache.as_ref().map(|c| &c.key) != Some(&key) {
            let order = tree_order(d);
            let lines = build(d, &order, g.split);
            self.cache = Some(Cache { key, order, lines });
            if let Some(j) = self.jump.take_if(|j| d.head.rev > j.after_rev) {
                self.go_to_path(d, &j.path, g);
            }
        }
        self.clamp(g);
    }

    /// The cached lines, when they are the lines of this very document: a drawing that mixed
    /// the lines of one revision with the files of another would index past the files.
    fn cache_for(&self, d: &Diff) -> Option<&Cache> {
        self.cache
            .as_ref()
            .filter(|c| c.key.0 == d.head.id && c.key.1 == d.head.rev)
    }

    fn clamp(&mut self, g: Geom) {
        let n = self.cache.as_ref().map_or(0, |c| c.lines.len());
        self.scroll = self.scroll.min(n.saturating_sub(g.rows));
    }

    fn go_to_path(&mut self, d: &Diff, path: &str, g: Geom) {
        let Some(cache) = &self.cache else { return };
        if let Some(pos) = cache.order.iter().position(|&i| d.files[i].path == path) {
            self.go_to(pos, g);
        }
    }

    /// Scroll so the file at `pos` of the tree order starts the body.
    fn go_to(&mut self, pos: usize, g: Geom) {
        let Some(cache) = &self.cache else { return };
        self.cur = pos;
        if let Some(at) = cache.lines.iter().position(|l| *l == Line::File(pos)) {
            self.scroll = at;
            self.clamp(g);
        }
    }

    /// After the body was scrolled by hand: the file at its top is the one the keys act on.
    fn follow(&mut self) {
        if let Some(line) = self.cache.as_ref().and_then(|c| c.lines.get(self.scroll)) {
            self.cur = line.file();
        }
    }

    fn current(&self) -> usize {
        self.cur
    }

    fn items(&self, docs: &Docs) -> Vec<Item> {
        if self.tab == Tab::Commits && self.commit.is_none() {
            let n = self.list(docs).map_or(0, |l| l.commits.len());
            return (0..n).map(Item::Commit).collect();
        }
        let Some(d) = self.diff(docs) else {
            return Vec::new();
        };
        let Some(cache) = self.cache_for(d) else {
            return Vec::new();
        };
        let prefix = self.shut_prefix();
        let rows = tree_rows(d, &cache.order, &|k| {
            self.shut.contains(&format!("{prefix}{k}"))
        });
        let back = (self.tab == Tab::Commits).then_some(Item::Back);
        back.into_iter()
            .chain(rows.into_iter().map(Item::Tree))
            .collect()
    }

    /// Folded folders are remembered per view and commit.
    fn shut_prefix(&self) -> String {
        format!("{:?}|", self.src())
    }

    /// Act on the row under the cursor of the left column.
    fn activate(&mut self, docs: &Docs, g: Geom) {
        let items = self.items(docs);
        match items.get(self.row) {
            Some(Item::Back) => {
                self.commit = None;
                self.scroll = 0;
                self.row = 0;
            }
            Some(Item::Commit(i)) => {
                let Some(sha) = self.list(docs).and_then(|l| l.commits.get(*i)) else {
                    return;
                };
                self.commit = Some(sha.sha.clone());
                self.scroll = 0;
                self.row = 1;
                let src = self.src();
                self.want(src);
            }
            Some(Item::Tree(TreeRow::Dir { key, shut, .. })) => {
                let full = format!("{}{key}", self.shut_prefix());
                if *shut {
                    self.shut.remove(&full);
                } else {
                    self.shut.insert(full);
                }
            }
            Some(Item::Tree(TreeRow::File { pos, .. })) => {
                self.go_to(*pos, g);
                self.focus = Focus::Body;
            }
            None => {}
        }
    }

    fn step_file(&mut self, forward: bool, g: Geom) {
        let n = self.cache.as_ref().map_or(0, |c| c.order.len());
        if n == 0 {
            return;
        }
        let now = self.current();
        let to = if forward {
            (now + 1).min(n - 1)
        } else {
            now.saturating_sub(1)
        };
        self.go_to(to, g);
    }

    fn step_hunk(&mut self, forward: bool, g: Geom) {
        let Some(cache) = &self.cache else { return };
        let hunks = cache
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Line::Hunk(..)))
            .map(|(i, _)| i);
        let target = if forward {
            hunks.into_iter().find(|i| *i > self.scroll)
        } else {
            hunks.into_iter().rev().find(|i| *i < self.scroll)
        };
        if let Some(t) = target {
            self.cur = cache.lines[t].file();
            self.scroll = t;
            self.clamp(g);
        }
    }

    /// Mark the file at the top as looked at (or not, when it already is) and go to the next.
    fn mark(&mut self, docs: &Docs) {
        let Some(d) = self.diff(docs) else { return };
        let Some(cache) = self.cache_for(d) else {
            return;
        };
        let pos = self.current();
        let Some(&at) = cache.order.get(pos) else {
            return;
        };
        if d.too_large {
            self.msg = "este diff es demasiado grande para marcarlo".into();
            return;
        }
        let file = &d.files[at];
        let viewed = file.seen != Some(Seen::Yes);
        let next = cache
            .order
            .get(pos + 1)
            .map(|&i| d.files[i].path.clone())
            .filter(|_| viewed);
        let Some(node) = self.node(&self.src()).map(str::to_owned) else {
            return;
        };
        self.asks.push(Ask::SetViewed {
            node,
            path: file.path.clone(),
            viewed,
        });
        self.own_patch = true;
        self.jump = Some(Jump {
            path: next.unwrap_or_else(|| file.path.clone()),
            after_rev: d.head.rev,
        });
    }

    pub fn press(&mut self, code: KeyCode, docs: &Docs, size: (usize, usize)) -> Outcome {
        let g = geom(size, self.split);
        self.msg.clear();
        match self.tree_or_body(code, docs, g) {
            Some(outcome) => outcome,
            None => self.global(code, docs),
        }
    }

    /// Keys of the focused column; `None` for one that is not theirs.
    fn tree_or_body(&mut self, code: KeyCode, docs: &Docs, g: Geom) -> Option<Outcome> {
        if self.focus == Focus::Tree {
            let n = self.items(docs).len();
            match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.row = (self.row + 1).min(n.saturating_sub(1))
                }
                KeyCode::Char('k') | KeyCode::Up => self.row = self.row.saturating_sub(1),
                KeyCode::Char('l') | KeyCode::Right => {
                    if let Some(Item::Tree(TreeRow::Dir {
                        key, shut: true, ..
                    })) = self.items(docs).get(self.row)
                    {
                        let full = format!("{}{key}", self.shut_prefix());
                        self.shut.remove(&full);
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => match self.items(docs).get(self.row) {
                    Some(Item::Tree(TreeRow::Dir {
                        key, shut: false, ..
                    })) => {
                        let full = format!("{}{key}", self.shut_prefix());
                        self.shut.insert(full);
                    }
                    _ if self.tab == Tab::Commits && self.commit.is_some() => {
                        self.row = 0;
                        self.activate(docs, g);
                    }
                    _ => {}
                },
                KeyCode::Enter => self.activate(docs, g),
                KeyCode::Tab => self.focus = Focus::Body,
                _ => return None,
            }
            return Some(Outcome::Stay);
        }
        let page = g.rows.saturating_sub(2).max(1);
        let most = self
            .cache
            .as_ref()
            .map_or(0, |c| c.lines.len().saturating_sub(g.rows));
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.step_file(true, g),
            KeyCode::Char('k') | KeyCode::Up => self.step_file(false, g),
            KeyCode::Char(']') => self.step_hunk(true, g),
            KeyCode::Char('[') => self.step_hunk(false, g),
            KeyCode::Char(' ') | KeyCode::PageDown => {
                self.scroll_to((self.scroll + page).min(most))
            }
            KeyCode::Char('b') | KeyCode::PageUp => {
                self.scroll_to(self.scroll.saturating_sub(page))
            }
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to(most),
            KeyCode::Char('v') => self.mark(docs),
            KeyCode::Tab => self.focus = Focus::Tree,
            KeyCode::Char('h') if self.tab == Tab::Commits && self.commit.is_some() => {
                self.commit = None;
                self.scroll = 0;
                self.row = 0;
                self.focus = Focus::Tree;
            }
            _ => return None,
        }
        Some(Outcome::Stay)
    }

    /// Keys that mean the same wherever the focus is.
    fn global(&mut self, code: KeyCode, docs: &Docs) -> Outcome {
        match code {
            KeyCode::Char('1') => self.set_tab(Tab::Uncommitted),
            KeyCode::Char('2') => self.set_tab(Tab::Commits),
            KeyCode::Char('3') => self.set_tab(Tab::General),
            KeyCode::Char('(') | KeyCode::Char(')') if self.tab == Tab::Commits => {
                self.step_commit(code == KeyCode::Char(')'), docs);
            }
            KeyCode::Char('s') => {
                self.split = match self.split {
                    Split::Auto => Split::One,
                    Split::One => Split::Two,
                    Split::Two => Split::Auto,
                };
                self.msg = match self.split {
                    Split::Auto => "según el ancho",
                    Split::One => "una columna",
                    Split::Two => "lado a lado",
                }
                .into();
                self.cache = None;
            }
            KeyCode::Char('r') => self.reread(),
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            _ => {}
        }
        Outcome::Stay
    }

    /// Read the shown node again now; a view that failed is asked for again.
    fn reread(&mut self) {
        let src = self.src();
        match self
            .slots
            .get(&src)
            .map(|s| (s.node.clone(), s.error.is_some()))
        {
            Some((_, true)) => {
                self.slots.remove(&src);
                self.want(src);
            }
            Some((Some(node), false)) => {
                self.asks.push(Ask::Refresh(node));
                self.msg = "releyendo".into();
            }
            _ => {}
        }
    }

    /// `(` and `)`: the previous or next commit of the list.
    fn step_commit(&mut self, forward: bool, docs: &Docs) {
        let (Some(sha), Some(list)) = (self.commit.clone(), self.list(docs)) else {
            return;
        };
        let Some(at) = list.commits.iter().position(|c| c.sha == sha) else {
            return;
        };
        let to = if forward { at + 1 } else { at.wrapping_sub(1) };
        if let Some(c) = list.commits.get(to) {
            self.commit = Some(c.sha.clone());
            self.scroll = 0;
            self.row = 1;
            let src = self.src();
            self.want(src);
        }
    }

    /// A left click at cell (`x`, `y`): a tab, or a row of the left column.
    pub fn click(&mut self, x: usize, y: usize, docs: &Docs, size: (usize, usize)) {
        let g = geom(size, self.split);
        self.msg.clear();
        if y == g.y + 1 {
            let mut at = g.ix + 1;
            for (tab, label) in self.tab_labels(docs) {
                let w = label.chars().count();
                if x >= at && x < at + w {
                    self.set_tab(tab);
                    return;
                }
                at += w + 1;
            }
            return;
        }
        let in_tree = x >= g.ix && x < g.ix + TREE && (g.tree_on || self.focus == Focus::Tree);
        if in_tree && y >= g.top && y < g.top + g.rows {
            let per = self.rows_per_item();
            let items = self.items(docs).len();
            let first = window(items, per, g.rows, self.row);
            let row = first + (y - g.top) / per;
            if row < items {
                self.row = row;
                self.focus = Focus::Tree;
                self.activate(docs, g);
            }
        }
    }

    /// The wheel over the body scrolls it.
    pub fn wheel(&mut self, down: bool, x: usize, size: (usize, usize)) {
        let g = geom(size, self.split);
        if x < g.bx {
            return;
        }
        let most = self
            .cache
            .as_ref()
            .map_or(0, |c| c.lines.len().saturating_sub(g.rows));
        self.scroll_to(if down {
            (self.scroll + WHEEL).min(most)
        } else {
            self.scroll.saturating_sub(WHEEL)
        });
    }

    /// Scroll by hand; the file at the top becomes the one the keys act on.
    fn scroll_to(&mut self, to: usize) {
        if to != self.scroll {
            self.scroll = to;
            self.follow();
        }
    }

    fn rows_per_item(&self) -> usize {
        if self.tab == Tab::Commits && self.commit.is_none() {
            2
        } else {
            1
        }
    }

    /// The tabs, with what each holds once it is known.
    fn tab_labels(&self, docs: &Docs) -> Vec<(Tab, String)> {
        let count = |src: Src, tab: Tab| -> String {
            let slot = self.slots.get(&src);
            let n = match tab {
                Tab::Commits => self.list(docs).map(|l| l.commits.len()),
                _ => slot
                    .and_then(|s| s.node.as_ref())
                    .and_then(|id| docs.diffs.get(id))
                    .map(|d| d.files.len()),
            };
            match (slot, n) {
                (Some(s), _) if s.error.is_some() => "!".into(),
                (Some(_), Some(n)) => n.to_string(),
                (Some(_), None) => "…".into(),
                (None, _) => String::new(),
            }
        };
        [
            (Tab::Uncommitted, "1 Sin commitear"),
            (Tab::Commits, "2 Commits"),
            (Tab::General, "3 General"),
        ]
        .into_iter()
        .map(|(tab, name)| {
            let n = count(tab.root(), tab);
            let label = if n.is_empty() {
                format!(" {name} ")
            } else {
                format!(" {name} {n} ")
            };
            (tab, label)
        })
        .collect()
    }

    /// The hint for the status bar, and `looked at / files` when there is a diff.
    pub fn status(&self, docs: &Docs) -> (String, Option<(usize, usize)>) {
        let hint = if !self.msg.is_empty() {
            self.msg.clone()
        } else if self.focus == Focus::Tree {
            "j/k mover · h/l plegar · ↵ ir · tab cuerpo · esc cerrar".into()
        } else if self.tab == Tab::Commits && self.commit.is_some() {
            "j/k archivo · ]/[ hunk · v visto · ( ) commit · h commits · tab árbol · esc cerrar"
                .into()
        } else {
            "j/k archivo · ]/[ hunk · v visto · tab árbol · 1 2 3 vistas · s lado a lado · r releer · esc cerrar".into()
        };
        let seen = self.diff(docs).filter(|d| !d.too_large).map(|d| {
            (
                d.files.iter().filter(|f| f.seen == Some(Seen::Yes)).count(),
                d.files.len(),
            )
        });
        (hint, seen)
    }

    /// Draw the viewer over the panes' area.
    pub fn draw(&self, g: &mut Grid, docs: &Docs, label: &str, now_ms: u64) {
        let m = geom((g.cols, g.rows), self.split);
        for y in m.y..m.y + m.h {
            for x in m.x..m.x + m.w {
                g.cells[y * g.cols + x] = (' ', Style::default());
            }
        }
        let src = self.src();
        let diff = self.diff(docs);
        let against = match (self.tab, diff.and_then(|d| d.base.as_deref())) {
            (Tab::General, Some(base)) => base.to_owned(),
            (Tab::General, None) => "origin".into(),
            (Tab::Commits, _) => self
                .commit
                .as_deref()
                .map_or("commits".into(), |s| format!("commit {}", short_sha(s))),
            _ => "HEAD".into(),
        };
        let watching = src == Src::Uncommitted && diff.is_some_and(|d| d.state == DiffState::Ok);
        let live = watching.then_some(if now_ms < self.live_until {
            ("actualizado", look::yellow())
        } else {
            ("en vivo", look::green())
        });
        g.boxed(
            (m.x, m.y, m.w, m.h),
            &format!("cambios · {} vs {against}", clean(label)),
            look::line(),
            Style::fg(look::cyan()).bold(),
            live.map(|(t, c)| (format!("● {t}"), Style::fg(c).bold())),
        );
        let mut at = m.ix + 1;
        for (tab, text) in self.tab_labels(docs) {
            let on = tab == self.tab;
            let broken = self.slots.get(&tab.root());
            let style = if on {
                Style {
                    fg: Some(look::panel()),
                    bg: Some(look::blue()),
                    bold: true,
                    ..Style::default()
                }
            } else if broken.is_some_and(|s| s.error.is_some()) {
                Style::fg(look::red())
            } else {
                Style::fg(look::dim())
            };
            g.put(at, m.y + 1, &text, style);
            at += text.chars().count() + 1;
        }
        if let Some((title, detail, tone)) = self.state_message(docs, &src) {
            self.centre(g, m, &title, &detail, tone);
            return;
        }
        let tree = m.tree_on || self.focus == Focus::Tree;
        if m.tree_on {
            for y in m.top..m.top + m.rows {
                g.put(m.ix + TREE, y, "│", Style::fg(look::line()));
            }
        }
        if self.tab == Tab::Commits && self.commit.is_none() {
            let rest = Geom {
                ix: m.bx,
                iw: m.bw,
                ..m
            };
            self.centre(g, rest, "elige un commit con ↵", "", look::dim());
        } else if let Some((d, cache)) = diff.and_then(|d| Some((d, self.cache_for(d)?))) {
            self.draw_body(g, m, d, cache);
        }
        if tree {
            if !m.tree_on {
                for y in m.top..m.top + m.rows {
                    g.put(
                        m.ix,
                        y,
                        &" ".repeat(TREE),
                        Style::default().on(look::panel()),
                    );
                }
            }
            self.draw_tree(g, m, docs, now_ms);
        }
        if let Some(d) = diff.filter(|d| d.too_large) {
            let text = match d.omitted {
                Some(n) => format!(
                    "pasó de 2 MiB: se listan {} y faltan {n}; míralo entero en un editor",
                    d.files.len()
                ),
                None => {
                    "pasó de 2 MiB: solo se listan los nombres; míralo entero en un editor".into()
                }
            };
            let y = m.top + m.rows.saturating_sub(1);
            g.put(
                m.bx,
                y,
                &" ".repeat(m.bw.saturating_sub(1)),
                Style::default().on(look::panel()),
            );
            g.put(
                m.bx + 1,
                y,
                &clip(&text, m.bw.saturating_sub(3)),
                Style::fg(look::yellow()).on(look::panel()),
            );
        }
    }

    /// What replaces the tree and body when there is nothing to list: still loading, failed,
    /// no base, or no changes. The title, an explanation and the tone of the title.
    fn state_message(&self, docs: &Docs, src: &Src) -> Option<(String, String, crate::tui::Rgb)> {
        let slot = self.slots.get(src);
        if let Some(why) = slot.and_then(|s| s.error.as_deref()) {
            return Some((
                "no se pudo leer".into(),
                format!(
                    "{}\nr reintentar · las otras vistas siguen: 1 2 3",
                    clean(why)
                ),
                look::red(),
            ));
        }
        match src {
            Src::Commits => match self.list(docs) {
                None => Some(loading(self.tab)),
                Some(l) if l.commits.is_empty() => {
                    Some(("sin commits en la rama".into(), String::new(), look::dim()))
                }
                Some(_) => None,
            },
            _ => {
                let Some(d) = self.diff(docs) else {
                    return Some(loading(self.tab));
                };
                if d.state == DiffState::NoBase {
                    return Some((
                        "origin no tiene rama por defecto: no hay contra qué comparar".into(),
                        "mira «Sin commitear» o «Commits»".into(),
                        look::fg(),
                    ));
                }
                d.files.is_empty().then(|| {
                    let text = match self.tab {
                        Tab::General => "nada commiteado contra el origen",
                        Tab::Uncommitted => "sin cambios sin commitear",
                        Tab::Commits => "este commit no cambió archivos",
                    };
                    (text.into(), String::new(), look::dim())
                })
            }
        }
    }

    fn centre(&self, g: &mut Grid, m: Geom, title: &str, detail: &str, tone: crate::tui::Rgb) {
        let y0 = m.top + m.rows / 2 - (m.rows / 2).min(2);
        let mid = m.ix + m.iw / 2;
        let rows = std::iter::once((title, tone)).chain(detail.lines().map(|l| (l, look::dim())));
        for (i, (text, colour)) in rows.enumerate() {
            let text = clip(text, m.iw.saturating_sub(2));
            let w = text.chars().count();
            g.put(mid.saturating_sub(w / 2), y0 + i, &text, Style::fg(colour));
        }
    }

    fn draw_tree(&self, g: &mut Grid, m: Geom, docs: &Docs, now_ms: u64) {
        let items = self.items(docs);
        let per = self.rows_per_item();
        let visible = (m.rows / per).max(1);
        let first = window(items.len(), per, m.rows, self.row);
        let d = self.diff(docs);
        let big = d.is_some_and(|d| d.too_large);
        let here = self.current();
        for (n, item) in items.iter().skip(first).take(visible).enumerate() {
            let y = m.top + n * per;
            let cursor = self.focus == Focus::Tree && first + n == self.row;
            let on = |st: Style| if cursor { st.on(look::sel()) } else { st };
            if cursor {
                for k in 0..per {
                    g.paint_bg(m.ix, y + k, TREE, look::sel());
                }
            }
            match item {
                Item::Back => {
                    g.put_over(m.ix + 1, y, "‹ commits", on(Style::fg(look::blue()).bold()));
                    if let Some(sha) = &self.commit {
                        let short = short_sha(sha);
                        let at = (m.ix + TREE).saturating_sub(1 + short.chars().count());
                        g.put_over(at, y, &short, on(Style::fg(look::dim())));
                    }
                }
                Item::Commit(i) => {
                    let Some(c) = self.list(docs).and_then(|l| l.commits.get(*i)) else {
                        continue;
                    };
                    g.put_over(
                        m.ix + 1,
                        y,
                        &clip(&clean(&c.subject), TREE - 3),
                        on(Style::fg(look::fg()).bold()),
                    );
                    let line = format!("{} · {}", c.short, ago(now_ms / 1000, c.time));
                    g.put_over(
                        m.ix + 1,
                        y + 1,
                        &clip(&line, TREE - 3),
                        on(Style::fg(look::dim())),
                    );
                }
                Item::Tree(TreeRow::Dir {
                    label,
                    depth,
                    added,
                    deleted,
                    shut,
                    ..
                }) => {
                    let ind = m.ix + 1 + depth * 2;
                    g.put_over(
                        ind,
                        y,
                        if *shut { "▸" } else { "▾" },
                        on(Style::fg(look::dim())),
                    );
                    let right = if big {
                        String::new()
                    } else {
                        format!("+{added} −{deleted}")
                    };
                    let room = (m.ix + TREE).saturating_sub(ind + 3 + right.chars().count());
                    g.put_over(
                        ind + 2,
                        y,
                        &clip(&format!("{}/", clean(label)), room),
                        on(Style::fg(look::fg()).bold()),
                    );
                    counts(g, m.ix + TREE - 1, y, *added, *deleted, big, cursor);
                }
                Item::Tree(TreeRow::File { pos, depth }) => {
                    let Some((d, cache)) = d.and_then(|d| Some((d, self.cache_for(d)?))) else {
                        continue;
                    };
                    let f = &d.files[cache.order[*pos]];
                    let ind = m.ix + 1 + depth * 2;
                    let (letter, colour) = status(f.status);
                    g.put_over(ind, y, letter, on(Style::fg(colour).bold()));
                    let seen = f.seen == Some(Seen::Yes);
                    let name = f.path.rsplit('/').next().unwrap_or(&f.path);
                    let style = if *pos == here && !seen {
                        Style::fg(look::blue()).bold()
                    } else if seen {
                        Style::fg(look::dim())
                    } else {
                        Style::fg(look::fg())
                    };
                    let room = (m.ix + TREE).saturating_sub(ind + 4 + if big { 0 } else { 9 });
                    g.put_over(ind + 2, y, &clip(&clean(name), room), on(style));
                    counts(g, m.ix + TREE - 3, y, f.added, f.deleted, big, cursor);
                    let (mark, colour) = match f.seen {
                        Some(Seen::Yes) => ("✓", look::green()),
                        Some(Seen::Changed) => ("◌", look::yellow()),
                        None if *pos == here => ("▸", look::blue()),
                        None => (" ", look::fg()),
                    };
                    g.put_over(m.ix + TREE - 2, y, mark, on(Style::fg(colour)));
                }
            }
        }
    }

    fn draw_body(&self, g: &mut Grid, m: Geom, d: &Diff, cache: &Cache) {
        let w = m.bw.saturating_sub(1);
        let lines = &cache.lines;
        let top_line = lines.get(self.scroll);
        // The pinned header takes a row of its own, unless the top line is a file's header.
        let pin = usize::from(top_line.is_some_and(|l| !matches!(l, Line::File(_))));
        for (r, line) in lines
            .iter()
            .skip(self.scroll)
            .take(m.rows - pin)
            .enumerate()
        {
            let y = m.top + pin + r;
            match line {
                Line::File(pos) => file_head(g, m.bx, y, w, d, cache, *pos),
                Line::Note(_, text) => g.put(
                    m.bx + 3,
                    y,
                    &clip(text, w.saturating_sub(4)),
                    Style::fg(look::dim()),
                ),
                Line::Hunk(_, text) => {
                    g.paint_bg(m.bx, y, w, look::sel());
                    g.put_over(
                        m.bx + 1,
                        y,
                        &clip(text, w.saturating_sub(2)),
                        Style::fg(look::cyan()).on(look::sel()),
                    );
                }
                Line::One(_, row) => one_column(g, m.bx, y, w, row),
                Line::Two(_, left, right) => two_columns(g, m.bx, y, w, left, right),
                Line::Gap(_) => {}
            }
        }
        if pin == 1
            && let Some(line) = top_line
        {
            file_head(g, m.bx, m.top, w, d, cache, line.file());
        }
        let n = lines.len();
        let thumb = (m.rows * m.rows / n.max(1)).clamp(1, m.rows);
        let pos = if n > m.rows {
            (m.rows - thumb) * self.scroll / (n - m.rows)
        } else {
            0
        };
        for r in 0..m.rows {
            let on = n > m.rows && r >= pos && r < pos + thumb;
            g.put(
                m.bx + w,
                m.top + r,
                if on { "┃" } else { "│" },
                Style::fg(if on { look::blue() } else { look::line() }),
            );
        }
    }
}

fn loading(tab: Tab) -> (String, String, crate::tui::Rgb) {
    let what = if tab == Tab::Commits {
        "los commits"
    } else {
        "los cambios"
    };
    (
        format!("◌ leyendo {what}…"),
        "git corre en el PC; puede tardar en repos grandes".into(),
        look::yellow(),
    )
}

/// The first 7 characters of a commit id.
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The first row of `items` rows of `per` lines each, in `rows` lines, that keeps `cursor` in view.
fn window(items: usize, per: usize, rows: usize, cursor: usize) -> usize {
    let visible = (rows / per).max(1);
    cursor
        .saturating_sub(visible - 1)
        .min(items.saturating_sub(visible))
}

fn status(s: FileStatus) -> (&'static str, crate::tui::Rgb) {
    match s {
        FileStatus::Modified => ("M", look::yellow()),
        FileStatus::Added => ("A", look::green()),
        FileStatus::Deleted => ("D", look::red()),
        FileStatus::Renamed => ("R", look::cyan()),
        FileStatus::Untracked => ("?", look::dim()),
    }
}

/// `+a −d` ending at column `end` (exclusive), green and red; nothing for a diff too large to count.
fn counts(g: &mut Grid, end: usize, y: usize, added: u32, deleted: u32, big: bool, cursor: bool) {
    if big {
        return;
    }
    let (a, d) = (format!("+{added}"), format!("−{deleted}"));
    let at = end.saturating_sub(a.chars().count() + 1 + d.chars().count());
    let on = |c| {
        let s = Style::fg(c);
        if cursor { s.on(look::sel()) } else { s }
    };
    g.put_over(at, y, &a, on(look::green()));
    g.put_over(at + a.chars().count() + 1, y, &d, on(look::red()));
}

fn file_head(g: &mut Grid, x: usize, y: usize, w: usize, d: &Diff, cache: &Cache, pos: usize) {
    let Some(&at) = cache.order.get(pos) else {
        return;
    };
    let f = &d.files[at];
    for i in 0..w {
        g.cells[y * g.cols + x + i] = (' ', Style::default().on(look::panel()));
    }
    let (letter, colour) = status(f.status);
    g.put(x + 1, y, letter, Style::fg(colour).bold().on(look::panel()));
    let seen = match f.seen {
        Some(Seen::Yes) => "   ✓ visto",
        Some(Seen::Changed) => "   ◌ cambió",
        None => "",
    };
    let totals = if d.too_large {
        String::new()
    } else {
        format!("+{} −{}", f.added, f.deleted)
    };
    let right = format!("{totals}{seen}");
    let name = match &f.old_path {
        Some(old) => format!("{}  ← {}", clean(&f.path), clean(old)),
        None => clean(&f.path),
    };
    g.put(
        x + 3,
        y,
        &clip(&name, w.saturating_sub(4 + right.chars().count())),
        Style::fg(look::fg()).bold().on(look::panel()),
    );
    let at = x + w - 1 - right.chars().count();
    let tone = match f.seen {
        Some(Seen::Yes) => look::green(),
        Some(Seen::Changed) => look::yellow(),
        None => look::dim(),
    };
    g.put(at, y, &right, Style::fg(tone).on(look::panel()));
    if !d.too_large {
        g.put(
            at,
            y,
            &format!("+{}", f.added),
            Style::fg(look::green()).on(look::panel()),
        );
        let minus = at + format!("+{}", f.added).chars().count() + 1;
        g.put(
            minus,
            y,
            &format!("−{}", f.deleted),
            Style::fg(look::red()).on(look::panel()),
        );
    }
}

/// A line of code: its chars with the tone of what they are and, for the words that
/// changed, a stronger background.
fn code(g: &mut Grid, x: usize, y: usize, w: usize, row: &Row, bg: Option<crate::tui::Rgb>) {
    let strong = if row.kind == Kind::Add {
        look::add_word()
    } else {
        look::del_word()
    };
    let cells: Vec<(char, Tone)> = row.text.chars().zip(tones(&row.text)).collect();
    for i in 0..w {
        let (ch, tone) = cells.get(i).copied().unwrap_or((' ', Tone::Plain));
        let fg = match tone {
            Tone::Plain => look::fg(),
            Tone::Keyword => look::magenta(),
            Tone::Text => look::green(),
            Tone::Number => look::yellow(),
            Tone::Comment => look::dim(),
        };
        let changed = row.words.is_some_and(|(a, b)| i >= a && i < b);
        g.cells[y * g.cols + x + i] = (
            ch,
            Style {
                fg: Some(fg),
                bg: if changed { Some(strong) } else { bg },
                ..Style::default()
            },
        );
    }
}

fn row_bg(row: &Row) -> Option<crate::tui::Rgb> {
    match row.kind {
        Kind::Add => Some(look::add_bg()),
        Kind::Del => Some(look::del_bg()),
        Kind::Ctx => None,
    }
}

fn marker(kind: Kind) -> (&'static str, Style) {
    match kind {
        Kind::Add => ("+", Style::fg(look::green()).bold()),
        Kind::Del => ("−", Style::fg(look::red()).bold()),
        Kind::Ctx => (" ", Style::default()),
    }
}

fn number(n: Option<u32>) -> String {
    n.map_or("    ".into(), |n| format!("{n:>4}"))
}

fn one_column(g: &mut Grid, x: usize, y: usize, w: usize, row: &Row) {
    let bg = row_bg(row);
    let tint = |s: Style| Style { bg, ..s };
    g.put(
        x,
        y,
        &format!("{} {} ", number(row.old), number(row.new)),
        tint(Style::fg(look::dim())),
    );
    let (m, style) = marker(row.kind);
    g.put(x + 9, y, m, tint(style));
    g.put(x + 10, y, " ", tint(Style::default()));
    code(g, x + 11, y, w.saturating_sub(11), row, bg);
}

fn two_columns(
    g: &mut Grid,
    x: usize,
    y: usize,
    w: usize,
    left: &Option<Row>,
    right: &Option<Row>,
) {
    let half = w.saturating_sub(1) / 2;
    let side = |g: &mut Grid, x: usize, w: usize, row: &Option<Row>, old: bool| {
        let Some(row) = row else {
            for i in 0..w {
                g.cells[y * g.cols + x + i] = (' ', Style::default().on(look::panel()));
            }
            return;
        };
        let bg = row_bg(row);
        let tint = |s: Style| Style { bg, ..s };
        let n = if old { row.old } else { row.new };
        g.put(
            x,
            y,
            &format!("{} ", number(n)),
            tint(Style::fg(look::dim())),
        );
        let (m, style) = marker(row.kind);
        g.put(x + 5, y, m, tint(style));
        g.put(x + 6, y, " ", tint(Style::default()));
        code(g, x + 7, y, w.saturating_sub(7), row, bg);
    };
    side(g, x, half, left, true);
    g.put(x + half, y, "│", Style::fg(look::line()));
    side(g, x + half + 1, w - half - 1, right, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contract/fixtures/daemon")
            .join(name);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        doc["node"].clone()
    }

    struct Rig {
        diffs: HashMap<String, Diff>,
        commits: HashMap<String, Commits>,
        changes: Changes,
        size: (usize, usize),
    }

    /// A viewer opened on the fixtures: the uncommitted node is the general fixture's
    /// document, as if the daemon had just sent it.
    fn rig(size: (usize, usize)) -> Rig {
        let mut r = Rig {
            diffs: HashMap::new(),
            commits: HashMap::new(),
            changes: Changes::open("ws"),
            size,
        };
        let d: Diff = serde_json::from_value(load("snapshot-diff.json")).unwrap();
        r.changes.made(Src::Uncommitted, &d.head.id.clone());
        r.diffs.insert(d.head.id.clone(), d);
        r.changes.update(
            &Docs {
                diffs: &r.diffs,
                commits: &r.commits,
            },
            size,
            0,
        );
        r
    }

    impl Rig {
        fn docs(&self) -> Docs<'_> {
            Docs {
                diffs: &self.diffs,
                commits: &self.commits,
            }
        }
        fn key(&mut self, code: KeyCode) -> Outcome {
            let (size, diffs, commits) = (self.size, &self.diffs, &self.commits);
            let out = self.changes.press(code, &Docs { diffs, commits }, size);
            self.changes.update(
                &Docs {
                    diffs: &self.diffs,
                    commits: &self.commits,
                },
                size,
                0,
            );
            out
        }
        fn text(&self) -> String {
            let mut g = Grid::new(self.size.0, self.size.1);
            self.changes.draw(&mut g, &self.docs(), "feat/login", 0);
            g.rows_plain().join("\n")
        }
    }

    #[test]
    fn opening_asks_for_the_uncommitted_changes_once() {
        let mut c = Changes::open("ws");
        assert_eq!(
            c.take_asks(),
            [Ask::OpenDiff {
                src: Src::Uncommitted,
                workspace: "ws".into(),
                scope: DiffScope::Uncommitted,
                commit: None
            }]
        );
        c.made(Src::Uncommitted, "node");
        assert_eq!(c.take_asks(), [Ask::Watch("node".into())]);
    }

    #[test]
    fn while_the_node_has_not_arrived_it_says_it_is_reading() {
        let c = Changes::open("ws");
        let (d, k) = (HashMap::new(), HashMap::new());
        let mut g = Grid::new(180, 44);
        c.draw(
            &mut g,
            &Docs {
                diffs: &d,
                commits: &k,
            },
            "demo",
            0,
        );
        let text = g.rows_plain().join("\n");
        assert!(text.contains("leyendo los cambios"), "{text}");
    }

    #[test]
    fn the_tree_and_the_body_show_the_files_of_the_diff() {
        let r = rig((180, 44));
        let t = r.text();
        assert!(
            t.contains("cambios · feat/login vs HEAD") || t.contains("vs origin"),
            "{t}"
        );
        assert!(
            t.contains("crates/web/src/"),
            "the chained folders are one row:\n{t}"
        );
        assert!(t.contains("auth.rs"));
        assert!(t.contains("@@ -41,9 +41,10 @@ impl Auth {"), "{t}");
        assert!(
            t.contains("+            self.limiter.fail(addr);")
                || t.contains("self.limiter.fail(addr);")
        );
        assert!(t.contains("✓ visto"), "the renamed file was looked at");
    }

    #[test]
    fn a_wide_body_is_two_columns_and_a_narrow_one_is_one() {
        let wide = rig((180, 44)).text();
        assert!(
            wide.lines()
                .any(|l| l.matches('│').count() >= 4 && l.contains("pub fn check")),
            "{wide}"
        );
        let narrow = rig((140, 36)).text();
        assert!(
            !narrow
                .lines()
                .any(|l| l.contains("pub fn check") && l.matches("pub fn check").count() > 1)
        );
        let mut forced = rig((140, 36));
        forced.key(KeyCode::Char('s'));
        forced.key(KeyCode::Char('s'));
        assert!(
            forced
                .text()
                .lines()
                .any(|l| l.matches("pub fn check").count() >= 1)
        );
    }

    #[test]
    fn under_84_columns_the_tree_hides_and_tab_shows_it_over_the_body() {
        let mut r = rig((96, 28));
        assert!(!r.text().contains("▾ crates"), "{}", r.text());
        r.key(KeyCode::Tab);
        assert!(r.text().contains("▾ crates"), "{}", r.text());
    }

    #[test]
    fn v_marks_the_file_and_asks_the_daemon_and_marks_again_to_undo() {
        let mut r = rig((180, 44));
        r.changes.take_asks();
        // The top file in tree order is assets/logo.png.
        r.key(KeyCode::Char('v'));
        let asks = r.changes.take_asks();
        assert_eq!(
            asks,
            [Ask::SetViewed {
                node: r.diffs.keys().next().unwrap().clone(),
                path: "assets/logo.png".into(),
                viewed: true
            }]
        );
        // The renamed file is already seen: v on it forgets the mark.
        r.key(KeyCode::Char('j'));
        r.key(KeyCode::Char('j'));
        r.key(KeyCode::Char('j'));
        r.key(KeyCode::Char('j'));
        let top = r.changes.current();
        r.key(KeyCode::Char('v'));
        let asks = r.changes.take_asks();
        let Ask::SetViewed { path, viewed, .. } = &asks[0] else {
            panic!("{asks:?}")
        };
        assert_eq!(
            (path.as_str(), *viewed),
            ("crates/web/src/session.rs", false),
            "pos {top}"
        );
    }

    #[test]
    fn j_and_k_move_between_files_and_brackets_between_hunks() {
        // Small enough that the diff does not fit and so scrolls.
        let mut r = rig((100, 20));
        let start = r.changes.scroll;
        r.key(KeyCode::Char('j'));
        assert!(r.changes.scroll > start);
        r.key(KeyCode::Char('k'));
        assert_eq!(r.changes.scroll, start);
        r.key(KeyCode::Char(']'));
        assert!(matches!(
            r.changes.cache.as_ref().unwrap().lines[r.changes.scroll],
            Line::Hunk(..)
        ));
    }

    #[test]
    fn the_other_views_are_asked_once_and_the_commit_list_leads_to_a_commit() {
        let mut r = rig((180, 44));
        r.changes.take_asks();
        r.key(KeyCode::Char('3'));
        r.key(KeyCode::Char('1'));
        r.key(KeyCode::Char('3'));
        assert_eq!(
            r.changes.take_asks(),
            [Ask::OpenDiff {
                src: Src::General,
                workspace: "ws".into(),
                scope: DiffScope::General,
                commit: None
            }]
        );
        r.key(KeyCode::Char('2'));
        assert_eq!(
            r.changes.take_asks(),
            [Ask::OpenCommits {
                workspace: "ws".into()
            }]
        );
        let list: Commits = serde_json::from_value(load("snapshot-commits.json")).unwrap();
        r.changes.made(Src::Commits, &list.head.id.clone());
        r.commits.insert(list.head.id.clone(), list);
        r.changes.take_asks();
        r.changes.update(
            &Docs {
                diffs: &r.diffs,
                commits: &r.commits,
            },
            r.size,
            0,
        );
        let t = r.text();
        assert!(t.contains("fix(web): valida el id de …"), "{t}");
        assert!(t.contains("elige un commit con ↵"));
        r.key(KeyCode::Char('j'));
        r.key(KeyCode::Enter);
        let sha = "89abcde0123456789abcdef0123456789abcdef0".to_owned();
        assert_eq!(
            r.changes.take_asks(),
            [Ask::OpenDiff {
                src: Src::Commit(sha.clone()),
                workspace: "ws".into(),
                scope: DiffScope::Commit,
                commit: Some(sha)
            }]
        );
    }

    #[test]
    fn a_view_that_failed_says_why_and_r_asks_again() {
        let mut c = Changes::open("ws");
        c.take_asks();
        c.failed(Src::Uncommitted, "not inside a git repository");
        let (d, k) = (HashMap::new(), HashMap::new());
        let mut g = Grid::new(180, 44);
        c.draw(
            &mut g,
            &Docs {
                diffs: &d,
                commits: &k,
            },
            "demo",
            0,
        );
        let text = g.rows_plain().join("\n");
        assert!(
            text.contains("no se pudo leer") && text.contains("not inside a git repository"),
            "{text}"
        );
        c.press(
            KeyCode::Char('r'),
            &Docs {
                diffs: &d,
                commits: &k,
            },
            (180, 44),
        );
        assert!(matches!(c.take_asks()[0], Ask::OpenDiff { .. }));
    }

    #[test]
    fn closing_stops_watching_every_node() {
        let mut r = rig((180, 44));
        r.changes.take_asks();
        assert_eq!(r.key(KeyCode::Esc), Outcome::Close);
        let asks = r.changes.close();
        assert_eq!(asks.len(), 1);
        assert!(matches!(asks[0], Ask::Close(_)));
    }

    #[test]
    fn nothing_to_compare_and_nothing_changed_have_their_own_words() {
        let mut r = rig((180, 44));
        let none: Diff = serde_json::from_value(load("snapshot-diff-no-base.json")).unwrap();
        r.changes.made(Src::General, &none.head.id.clone());
        r.diffs.insert(none.head.id.clone(), none);
        r.key(KeyCode::Char('3'));
        assert!(
            r.text().contains("origin no tiene rama por defecto"),
            "{}",
            r.text()
        );
    }

    #[test]
    fn a_diff_over_the_limit_lists_names_and_says_so() {
        let mut r = rig((180, 44));
        let big: Diff = serde_json::from_value(load("snapshot-diff-too-large.json")).unwrap();
        r.changes.made(Src::General, &big.head.id.clone());
        r.diffs.insert(big.head.id.clone(), big);
        r.key(KeyCode::Char('3'));
        let t = r.text();
        assert!(
            t.contains("pasó de 2 MiB") && t.contains("faltan 1034"),
            "{t}"
        );
        assert!(
            t.contains("a.rs") && !t.contains("+0 −0"),
            "no counts for names only:\n{t}"
        );
    }

    #[test]
    fn a_patch_of_the_node_is_said_in_the_title_for_a_moment() {
        let mut r = rig((180, 44));
        let id = r.diffs.keys().next().unwrap().clone();
        r.diffs.get_mut(&id).unwrap().head.rev += 1;
        r.changes.update(
            &Docs {
                diffs: &r.diffs,
                commits: &r.commits,
            },
            r.size,
            1000,
        );
        let mut g = Grid::new(180, 44);
        r.changes.draw(&mut g, &r.docs(), "demo", 1500);
        assert!(g.rows_plain().join("\n").contains("● actualizado"));
        let mut later = Grid::new(180, 44);
        r.changes.draw(&mut later, &r.docs(), "demo", 9000);
        assert!(later.rows_plain().join("\n").contains("● en vivo"));
    }

    #[test]
    fn frames_fit_the_terminal_at_every_size() {
        for size in [
            (180, 44),
            (140, 36),
            (120, 32),
            (96, 28),
            (60, 20),
            (30, 10),
        ] {
            let mut r = rig(size);
            for k in ['j', ']', 'v', '3', '2', '1', 's', 's', '\t'] {
                r.key(if k == '\t' {
                    KeyCode::Tab
                } else {
                    KeyCode::Char(k)
                });
                let _ = r.text();
            }
        }
    }
}
