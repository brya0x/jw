//! The GitHub-style diff viewer (REQ-21, addendum 17): a sidebar on the left
//! with three views (General, Uncommitted, Commits) and their files, the
//! chosen view's files stacked in one scroll on the right with the current
//! file's header pinned on top, side by side by default with changed words
//! marked. It holds what git said and where the reader is in each view; the
//! git work itself runs elsewhere, asked for through [`Request`].

use std::collections::{BTreeMap, BTreeSet};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use unicode_width::UnicodeWidthChar;

use crate::diff::{self, Commit, Commits, File, Kind, Loaded, Status};
use crate::theme::p;
use crate::view::md::truncate;

const TREE: u16 = 30;

/// Below this many columns inside the pane the sidebar is not drawn beside
/// the code; `Tab` opens it over the code instead (REQ-232).
const SIDEBAR_MIN: u16 = 100;

/// The three views of the pane (REQ-227).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    General,
    Uncommitted,
    Commits,
}

impl Which {
    /// In the order the sidebar lists them and `1 2 3` pick them.
    const ALL: [Which; 3] = [Which::Uncommitted, Which::Commits, Which::General];

    fn n(self) -> usize {
        Self::ALL
            .iter()
            .position(|w| *w == self)
            .map_or(0, |i| i + 1)
    }

    fn name(self) -> &'static str {
        match self {
            Which::General => "General",
            Which::Uncommitted => "Uncommitted",
            Which::Commits => "Commits",
        }
    }
}

/// A piece of git work that fills part of the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    General,
    Uncommitted,
    /// The list of commits.
    Commits,
    /// The diff of one commit, by sha.
    Commit(String),
}

/// What a finished piece of git work brings back.
pub enum Part {
    Files(Loaded),
    List(Commits),
}

/// Git work the viewer asks of its owner, and the number to answer it with:
/// a result whose number is not the one its slot waits for is old (REQ-236).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub slot: Slot,
    pub seq: u64,
}

enum Load<T> {
    Idle,
    Loading(u64),
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    /// Needs asking for; a failure is asked again only when the reader goes
    /// back to it (`retry`), never right after it failed.
    fn wants(&self, retry: bool) -> bool {
        matches!(self, Load::Idle) || (retry && matches!(self, Load::Failed(_)))
    }
}

/// A view's files, or just their list when the diff was too large to read.
struct Shown {
    files: Vec<File>,
    /// How many files the diff has, when it was too large (REQ-238).
    big: Option<usize>,
}

impl From<Loaded> for Shown {
    fn from(l: Loaded) -> Self {
        let mut shown = match l {
            Loaded::Files(files) => Shown { files, big: None },
            Loaded::TooLarge { files, total } => Shown {
                files: files
                    .into_iter()
                    .map(|(status, path)| File {
                        path,
                        old_path: None,
                        status,
                        binary: false,
                        untracked: false,
                        unreadable: None,
                        hunks: Vec::new(),
                    })
                    .collect(),
                big: Some(total),
            },
        };
        // The scroll follows the sidebar's tree, top to bottom (REQ-247).
        shown.files.sort_by(|a, b| tree_cmp(&a.path, &b.path));
        shown
    }
}

/// Directories before files at every level, each by name.
fn tree_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.split('/').peekable(), b.split('/').peekable());
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => {
                let (dx, dy) = (a.peek().is_some(), b.peek().is_some());
                if dx != dy {
                    return dy.cmp(&dx);
                }
                if x != y {
                    return x.cmp(y);
                }
            }
            (x, y) => return x.is_some().cmp(&y.is_some()),
        }
    }
}

/// One directory row of the sidebar tree: a chain of directories with one
/// child each is a single row (REQ-248).
struct Dir {
    /// Its path with a trailing `/`, the key of its collapsed state.
    key: String,
    label: String,
    depth: usize,
    parent: Option<usize>,
    files: usize,
    added: usize,
    deleted: usize,
}

enum Node {
    Dir(usize),
    File(usize),
}

/// The files, sorted in tree order, as the rows they would be with every
/// directory open.
#[derive(Default)]
struct Tree {
    dirs: Vec<Dir>,
    nodes: Vec<Node>,
    /// By file: how deep it sits and the directory row it is under.
    file_depth: Vec<usize>,
    file_parent: Vec<Option<usize>>,
}

impl Tree {
    fn new(files: &[File]) -> Self {
        let paths: Vec<Vec<&str>> = files.iter().map(|f| f.path.split('/').collect()).collect();
        let mut t = Tree {
            file_depth: vec![0; files.len()],
            file_parent: vec![None; files.len()],
            ..Tree::default()
        };
        t.build(files, &paths, 0..files.len(), 0, 0, None);
        t
    }

    /// `range` of files all share their first `base` path parts.
    fn build(
        &mut self,
        files: &[File],
        paths: &[Vec<&str>],
        range: std::ops::Range<usize>,
        base: usize,
        depth: usize,
        parent: Option<usize>,
    ) {
        let mut i = range.start;
        while i < range.end {
            if paths[i].len() == base + 1 {
                self.nodes.push(Node::File(i));
                self.file_depth[i] = depth;
                self.file_parent[i] = parent;
                i += 1;
                continue;
            }
            let name = paths[i][base];
            let mut end = i;
            while end < range.end && paths[end].len() > base + 1 && paths[end][base] == name {
                end += 1;
            }
            // Take in the directories below while it has no file of its own
            // and only one directory in it.
            let mut k = 1;
            while (i..end).all(|j| paths[j].len() > base + k + 1)
                && (i..end).all(|j| paths[j][base + k] == paths[i][base + k])
            {
                k += 1;
            }
            let at = self.dirs.len();
            self.dirs.push(Dir {
                key: format!("{}/", paths[i][..base + k].join("/")),
                label: paths[i][base..base + k].join("/"),
                depth,
                parent,
                files: end - i,
                added: files[i..end].iter().map(File::added).sum(),
                deleted: files[i..end].iter().map(File::deleted).sum(),
            });
            self.nodes.push(Node::Dir(at));
            self.build(files, paths, i..end, base + k, depth + 1, Some(at));
            i = end;
        }
    }

    /// Whether `d` or a directory above it is closed.
    fn hidden(&self, d: usize, closed: &BTreeSet<String>) -> bool {
        let mut at = Some(d);
        while let Some(i) = at {
            if closed.contains(&self.dirs[i].key) {
                return true;
            }
            at = self.dirs[i].parent;
        }
        false
    }

    /// The row a file is shown on: its own, or the outermost closed directory
    /// over it.
    fn shown_as(&self, f: usize, closed: &BTreeSet<String>) -> Row {
        let mut hide = None;
        let mut at = self.file_parent.get(f).copied().flatten();
        while let Some(i) = at {
            if closed.contains(&self.dirs[i].key) {
                hide = Some(i);
            }
            at = self.dirs[i].parent;
        }
        hide.map_or(Row::File(f), Row::Dir)
    }
}

/// Where the reader is in one view; kept apart for each (REQ-233), and
/// `viewed` by path so a list that changes keeps it (REQ-234).
#[derive(Default)]
struct Pos {
    /// Index into the flattened items of the top visible row.
    scroll: usize,
    viewed: BTreeSet<String>,
    /// Directories closed in the sidebar, by [`Dir::key`] (REQ-249).
    closed: BTreeSet<String>,
}

/// What the body shows.
enum Content<'a> {
    Loading,
    Failed(&'a str),
    NoCommits,
    Shown(&'a Shown),
}

pub struct DiffView {
    /// The workspace's name, for the title.
    pub name: String,
    /// The branch it is compared with; empty when origin has none.
    pub base: String,
    /// Where the files live, to open one in the reader.
    pub dir: String,
    which: Which,
    general: Load<Shown>,
    uncommitted: Load<Shown>,
    list: Load<Commits>,
    /// The selected commit, an index into the list.
    commit: usize,
    details: BTreeMap<String, Load<Shown>>,
    pos: BTreeMap<String, Pos>,
    next_gen: u64,
    /// The sidebar's cursor, an index into its rows, while it has the keys.
    sb: Option<usize>,
    /// Side by side, as last drawn: wide panes are, narrow ones aren't,
    /// unless `t` said otherwise.
    split: bool,
    forced: Option<bool>,
    /// Whether its pane has the focus, for the border.
    pub focused: bool,
    /// Where the sidebar's rows were drawn, and which row came first, for
    /// clicks.
    sb_at: Option<(Rect, usize)>,
    /// Rows the content showed last frame, for paging.
    height: usize,
}

/// One row of the sidebar (REQ-227).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Section(Which),
    Commit(usize),
    /// A directory of the tree, an index into [`Tree::dirs`].
    Dir(usize),
    File(usize),
    /// "older commits not shown".
    More,
}

/// One row of the flattened scroll.
#[derive(Clone, Copy, PartialEq)]
enum Item {
    File(usize),
    Hunk(usize, usize),
    /// File, hunk, row of that hunk (split rows or plain lines).
    Row(usize, usize, usize),
    Note(usize),
    Gap,
}

/// What a key asked of the viewer beyond moving around in it.
pub enum Action {
    None,
    Close,
    /// Open this file of the worktree (REQ-42).
    Open(String),
    /// Run this git work and hand the result back to [`DiffView::apply`].
    Load(Request),
}

fn load(r: Option<Request>) -> Action {
    r.map_or(Action::None, Action::Load)
}

impl DiffView {
    /// A pane that opens on Uncommitted, which `uncommitted` already holds
    /// (REQ-222).
    pub fn new(
        name: String,
        base: String,
        dir: String,
        uncommitted: Result<Loaded, String>,
    ) -> Self {
        Self {
            name,
            base,
            dir,
            which: Which::Uncommitted,
            general: Load::Idle,
            uncommitted: match uncommitted {
                Ok(l) => Load::Ready(l.into()),
                Err(e) => Load::Failed(e),
            },
            list: Load::Idle,
            commit: 0,
            details: BTreeMap::new(),
            pos: BTreeMap::new(),
            next_gen: 1,
            sb: None,
            split: true,
            forced: None,
            focused: true,
            sb_at: None,
            height: 20,
        }
    }

    // ---- what is loaded ----

    fn pos_key(&self) -> String {
        match self.which {
            Which::General => "general".into(),
            Which::Uncommitted => "uncommitted".into(),
            Which::Commits => self
                .selected()
                .map_or("commits".into(), |c| format!("c:{}", c.sha)),
        }
    }

    fn pos(&self) -> &Pos {
        static NONE: Pos = Pos {
            scroll: 0,
            viewed: BTreeSet::new(),
            closed: BTreeSet::new(),
        };
        self.pos.get(&self.pos_key()).unwrap_or(&NONE)
    }

    fn pos_mut(&mut self) -> &mut Pos {
        let key = self.pos_key();
        self.pos.entry(key).or_default()
    }

    fn selected(&self) -> Option<&Commit> {
        match &self.list {
            Load::Ready(l) => l.list.get(self.commit),
            _ => None,
        }
    }

    fn content(&self) -> Content<'_> {
        fn one(l: &Load<Shown>) -> Content<'_> {
            match l {
                Load::Ready(s) => Content::Shown(s),
                Load::Failed(e) => Content::Failed(e),
                Load::Idle | Load::Loading(_) => Content::Loading,
            }
        }
        match self.which {
            Which::General => one(&self.general),
            Which::Uncommitted => one(&self.uncommitted),
            Which::Commits => match &self.list {
                Load::Ready(l) if l.list.is_empty() => Content::NoCommits,
                Load::Ready(_) => self
                    .selected()
                    .and_then(|c| self.details.get(&c.sha))
                    .map_or(Content::Loading, one),
                Load::Failed(e) => Content::Failed(e),
                Load::Idle | Load::Loading(_) => Content::Loading,
            },
        }
    }

    fn files(&self) -> &[File] {
        match self.content() {
            Content::Shown(s) => &s.files,
            _ => &[],
        }
    }

    fn big(&self) -> Option<usize> {
        match self.content() {
            Content::Shown(s) => s.big,
            _ => None,
        }
    }

    // ---- asking for git work ----

    /// Marks `slot` as waiting and says what to run, unless it has what it
    /// needs or is already on the way.
    fn want(&mut self, slot: Slot, retry: bool) -> Option<Request> {
        let seq = self.next_gen;
        let wants = match &slot {
            Slot::General => &mut self.general,
            Slot::Uncommitted => &mut self.uncommitted,
            Slot::Commit(sha) => self.details.entry(sha.clone()).or_insert(Load::Idle),
            Slot::Commits => {
                if !self.list.wants(retry) {
                    return None;
                }
                self.list = Load::Loading(seq);
                self.next_gen += 1;
                return Some(Request { slot, seq });
            }
        };
        if !wants.wants(retry) {
            return None;
        }
        *wants = Load::Loading(seq);
        self.next_gen += 1;
        Some(Request { slot, seq })
    }

    /// What the shown view still needs from git (REQ-235).
    fn ensure(&mut self, retry: bool) -> Option<Request> {
        match self.which {
            Which::General => self.want(Slot::General, retry),
            Which::Uncommitted => self.want(Slot::Uncommitted, retry),
            Which::Commits => match self.want(Slot::Commits, retry) {
                Some(r) => Some(r),
                None => {
                    let sha = self.selected()?.sha.clone();
                    self.want(Slot::Commit(sha), retry)
                }
            },
        }
    }

    /// Takes what git work brought back. An answer whose number is not the
    /// one its slot waits for is dropped (REQ-236). Returns more work when
    /// the answer makes the shown view need it.
    pub fn apply(&mut self, slot: Slot, seq: u64, result: Result<Part, String>) -> Option<Request> {
        fn settle<T>(l: &mut Load<T>, seq: u64, r: Result<T, String>) -> bool {
            if !matches!(l, Load::Loading(g) if *g == seq) {
                return false;
            }
            *l = match r {
                Ok(v) => Load::Ready(v),
                Err(e) => Load::Failed(e),
            };
            true
        }
        let files = |r: Result<Part, String>| {
            r.and_then(|p| match p {
                Part::Files(l) => Ok(Shown::from(l)),
                Part::List(_) => Err("a list where files were asked".to_string()),
            })
        };
        match slot {
            Slot::General => settle(&mut self.general, seq, files(result)),
            Slot::Uncommitted => settle(&mut self.uncommitted, seq, files(result)),
            Slot::Commit(sha) => self
                .details
                .get_mut(&sha)
                .is_some_and(|l| settle(l, seq, files(result))),
            Slot::Commits => {
                let list = result.and_then(|p| match p {
                    Part::List(l) => Ok(l),
                    Part::Files(_) => Err("files where a list was asked".to_string()),
                });
                settle(&mut self.list, seq, list)
            }
        }
        .then(|| self.ensure(false))
        .flatten()
    }

    /// Whether the sidebar has the keys (REQ-230).
    pub fn in_sidebar(&self) -> bool {
        self.sb.is_some()
    }

    /// Whether the Commits view is the one shown (REQ-231).
    pub fn in_commits(&self) -> bool {
        self.which == Which::Commits
    }

    // ---- moving between views and commits ----

    fn show(&mut self, which: Which) -> Action {
        self.which = which;
        if self.sb.is_some() {
            self.sb = self.row_of(Row::Section(which));
        }
        load(self.ensure(true))
    }

    fn select_commit(&mut self, i: usize) -> Action {
        let len = match &self.list {
            Load::Ready(l) => l.list.len(),
            _ => 0,
        };
        if i >= len {
            return Action::None;
        }
        self.commit = i;
        if self.sb.is_some() {
            self.sb = self.row_of(Row::Commit(i));
        }
        load(self.ensure(true))
    }

    // ---- the sidebar ----

    fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for w in Which::ALL {
            out.push(Row::Section(w));
            if w != self.which {
                continue;
            }
            if w == Which::Commits {
                if let Load::Ready(l) = &self.list {
                    for i in 0..l.list.len() {
                        out.push(Row::Commit(i));
                        if i == self.commit {
                            self.file_rows(&mut out);
                        }
                    }
                    if l.more {
                        out.push(Row::More);
                    }
                }
            } else {
                self.file_rows(&mut out);
            }
        }
        out
    }

    /// The tree's rows without what a closed directory hides (REQ-248).
    fn file_rows(&self, out: &mut Vec<Row>) {
        let tree = Tree::new(self.files());
        let closed = &self.pos().closed;
        out.extend(tree.nodes.iter().filter_map(|n| match n {
            Node::Dir(d) if !tree.dirs[*d].parent.is_some_and(|p| tree.hidden(p, closed)) => {
                Some(Row::Dir(*d))
            }
            Node::File(f) if !tree.file_parent[*f].is_some_and(|p| tree.hidden(p, closed)) => {
                Some(Row::File(*f))
            }
            _ => None,
        }));
    }

    fn row_of(&self, row: Row) -> Option<usize> {
        self.rows().iter().position(|r| *r == row)
    }

    /// `Tab`: the cursor goes into the sidebar, on the shown commit or view.
    fn enter_sidebar(&mut self) {
        let row = match (self.which, self.selected()) {
            (Which::Commits, Some(_)) => Row::Commit(self.commit),
            _ => Row::Section(self.which),
        };
        self.sb = self.row_of(row).or(Some(0));
    }

    fn sidebar_key(&mut self, k: &KeyEvent) -> Option<Action> {
        let cur = self.sb?;
        let n = self.rows().len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.sb = Some((cur + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => self.sb = Some(cur.saturating_sub(1)),
            KeyCode::Tab | KeyCode::Esc => self.sb = None,
            KeyCode::Enter => return Some(self.choose(cur)),
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(Row::Dir(d)) = self.rows().get(cur).copied() {
                    if self.is_closed(d) {
                        self.toggle(d);
                    } else {
                        self.sb = Some((cur + 1).min(n - 1));
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let rows = self.rows();
                let tree = Tree::new(self.files());
                let (open, parent) = match rows.get(cur).copied() {
                    Some(Row::Dir(d)) => (!self.is_closed(d), tree.dirs[d].parent),
                    Some(Row::File(f)) => (false, tree.file_parent[f]),
                    _ => (false, None),
                };
                if let (true, Some(Row::Dir(d))) = (open, rows.get(cur).copied()) {
                    self.toggle(d);
                } else if let Some(i) =
                    parent.and_then(|p| rows.iter().position(|r| *r == Row::Dir(p)))
                {
                    self.sb = Some(i);
                }
            }
            _ => return None,
        }
        Some(Action::None)
    }

    fn is_closed(&self, d: usize) -> bool {
        let tree = Tree::new(self.files());
        self.pos().closed.contains(&tree.dirs[d].key)
    }

    /// Opens a closed directory and closes an open one (REQ-249).
    fn toggle(&mut self, d: usize) {
        let key = Tree::new(self.files()).dirs[d].key.clone();
        let closed = &mut self.pos_mut().closed;
        if !closed.remove(&key) {
            closed.insert(key);
        }
    }

    /// `↵` or a click on a row of the sidebar.
    fn choose(&mut self, at: usize) -> Action {
        match self.rows().get(at).copied() {
            Some(Row::Section(w)) => self.show(w),
            Some(Row::Commit(i)) => self.select_commit(i),
            Some(Row::Dir(d)) => {
                self.toggle(d);
                Action::None
            }
            Some(Row::File(f)) => {
                self.sb = None;
                self.goto_file(f);
                Action::None
            }
            Some(Row::More) | None => Action::None,
        }
    }

    // ---- the flattened scroll ----

    fn rows_of(&self, f: usize, h: usize) -> usize {
        let hunk = &self.files()[f].hunks[h];
        if self.split {
            diff::split_rows(hunk).len()
        } else {
            hunk.lines.len()
        }
    }

    fn items(&self) -> Vec<Item> {
        let mut out = Vec::new();
        let viewed = &self.pos().viewed;
        for (f, file) in self.files().iter().enumerate() {
            out.push(Item::File(f));
            if viewed.contains(&file.path) {
                continue;
            }
            if file.binary || file.hunks.is_empty() {
                out.push(Item::Note(f));
            }
            for h in 0..file.hunks.len() {
                out.push(Item::Hunk(f, h));
                out.extend((0..self.rows_of(f, h)).map(|r| Item::Row(f, h, r)));
            }
            out.push(Item::Gap);
        }
        out
    }

    /// The file the top of the view is in; 0 with no files.
    fn current(&self, items: &[Item]) -> usize {
        items
            .iter()
            .take(self.pos().scroll + 1)
            .rev()
            .find_map(|i| match i {
                Item::File(f) => Some(*f),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        if let Some(a) = self.sidebar_key(&k) {
            return a;
        }
        let items = self.items();
        let last = items.len().saturating_sub(1);
        let page = self.height.max(2) - 1;
        let scroll = self.pos().scroll;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let find = |from: usize, forward: bool, want: &dyn Fn(&Item) -> bool| -> Option<usize> {
            if forward {
                (from + 1..items.len()).find(|&i| want(&items[i]))
            } else {
                (0..from).rev().find(|&i| want(&items[i]))
            }
        };
        let is_file = |i: &Item| matches!(i, Item::File(_));
        let is_hunk = |i: &Item| matches!(i, Item::Hunk(..));
        let to = match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Char('1') => return self.show(Which::ALL[0]),
            KeyCode::Char('2') => return self.show(Which::ALL[1]),
            KeyCode::Char('3') => return self.show(Which::ALL[2]),
            KeyCode::Tab => {
                self.enter_sidebar();
                return Action::None;
            }
            KeyCode::Char(')') if self.which == Which::Commits => {
                return self.select_commit(self.commit + 1);
            }
            KeyCode::Char('(') if self.which == Which::Commits => {
                return match self.commit.checked_sub(1) {
                    Some(i) => self.select_commit(i),
                    None => Action::None,
                };
            }
            KeyCode::Char('j') => find(scroll, true, &is_file).unwrap_or(scroll),
            KeyCode::Char('k') => find(scroll, false, &is_file).unwrap_or(0),
            KeyCode::Char(']') => find(scroll, true, &is_hunk).unwrap_or(scroll),
            KeyCode::Char('[') => find(scroll, false, &is_hunk).unwrap_or(0),
            KeyCode::Down => (scroll + 1).min(last),
            KeyCode::Up => scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => (scroll + page / 2).min(last),
            KeyCode::Char('u') if ctrl => scroll.saturating_sub(page / 2),
            KeyCode::PageDown | KeyCode::Char(' ') => (scroll + page).min(last),
            KeyCode::PageUp => scroll.saturating_sub(page),
            KeyCode::Char('g') => 0,
            KeyCode::Char('G') => last,
            KeyCode::Char('t') => {
                // Keep the same file on top across the switch.
                let f = self.current(&items);
                self.split = !self.split;
                self.forced = Some(self.split);
                self.goto_file(f);
                return Action::None;
            }
            KeyCode::Char('v') => {
                let f = self.current(&items);
                let Some(path) = self.files().get(f).map(|f| f.path.clone()) else {
                    return Action::None;
                };
                if !self.pos_mut().viewed.remove(&path) {
                    self.pos_mut().viewed.insert(path);
                    // Like GitHub: marking a file viewed moves on to the next.
                    self.goto_file((f + 1).min(self.files().len().saturating_sub(1)));
                    return Action::None;
                }
                self.goto_file(f);
                return Action::None;
            }
            KeyCode::Enter => {
                if let Some(file) = self.files().get(self.current(&items))
                    && file.status != Status::Deleted
                {
                    return Action::Open(file.path.clone());
                }
                return Action::None;
            }
            _ => return Action::None,
        };
        self.pos_mut().scroll = to;
        Action::None
    }

    /// The wheel: `by` rows down (up when negative).
    pub fn scroll_by(&mut self, by: isize) {
        let last = self.items().len().saturating_sub(1);
        let to = self.pos().scroll.saturating_add_signed(by).min(last);
        self.pos_mut().scroll = to;
    }

    /// A click: on a row of the sidebar, as `↵` on it.
    pub fn click(&mut self, x: u16, y: u16) -> Action {
        let Some((r, first)) = self.sb_at else {
            return Action::None;
        };
        if x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height {
            let at = first + usize::from(y - r.y);
            if at < self.rows().len() {
                if self.sb.is_some() {
                    self.sb = Some(at);
                }
                return self.choose(at);
            }
        }
        Action::None
    }

    fn goto_file(&mut self, f: usize) {
        if let Some(i) = self.items().iter().position(|i| *i == Item::File(f)) {
            self.pos_mut().scroll = i;
        }
    }

    // ---- drawing ----

    /// What the title says of the view (REQ-242).
    fn title(&self) -> String {
        match self.which {
            Which::General if self.base.is_empty() => format!("{} · committed", self.name),
            Which::General => format!("{} vs {} · committed", self.name, self.base),
            Which::Uncommitted => format!("{} · not committed", self.name),
            Which::Commits => match &self.list {
                Load::Ready(l) => format!(
                    "{} · {}{} commits",
                    self.name,
                    l.list.len(),
                    if l.more { "+" } else { "" }
                ),
                _ => format!("{} · commits", self.name),
            },
        }
    }

    /// The count after a view's name in the sidebar.
    fn count(&self, w: Which) -> Span<'static> {
        let dim = Style::default().fg(p().dim);
        let n = |l: &Load<Shown>| match l {
            Load::Ready(s) => Span::styled(s.big.unwrap_or(s.files.len()).to_string(), dim),
            Load::Loading(_) => Span::styled(spinner(), Style::default().fg(p().blue)),
            Load::Failed(_) => Span::styled("!", Style::default().fg(p().red)),
            Load::Idle => Span::styled("…", dim),
        };
        match w {
            Which::General => n(&self.general),
            Which::Uncommitted => n(&self.uncommitted),
            Which::Commits => match &self.list {
                Load::Ready(l) => Span::styled(
                    format!("{}{}", l.list.len(), if l.more { "+" } else { "" }),
                    dim,
                ),
                Load::Loading(_) => Span::styled(spinner(), Style::default().fg(p().blue)),
                Load::Failed(_) => Span::styled("!", Style::default().fg(p().red)),
                Load::Idle => Span::styled("…", dim),
            },
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        // Side by side needs room for two columns of code (REQ-57).
        let split = self.forced.unwrap_or(area.width >= 110);
        if split != self.split {
            let items = self.items();
            let f = self.current(&items);
            self.split = split;
            self.goto_file(f);
        }
        let accent = if self.focused { p().blue } else { p().line };
        let (adds, dels): (usize, usize) = self
            .files()
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added(), d + f.deleted()));
        let mode = if self.split { "split" } else { "unified" };
        let mut title = vec![
            Span::styled(
                " changes ",
                Style::default()
                    .fg(if self.focused { p().blue } else { p().fg })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{} ", self.title()), Style::default().fg(p().dim)),
        ];
        if adds + dels > 0 {
            title.push(Span::styled(
                format!("+{adds} "),
                Style::default().fg(p().green),
            ));
            title.push(Span::styled(
                format!("−{dels} "),
                Style::default().fg(p().yellow),
            ));
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Line::from(title))
            .title(
                Line::from(Span::styled(
                    format!(" {mode} "),
                    Style::default().fg(p().dim),
                ))
                .right_aligned(),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The sidebar beside the code where there is room for it; below that
        // a line says where you are and `Tab` opens the sidebar over the code.
        let wide = inner.width >= SIDEBAR_MIN;
        self.sb_at = None;
        let body = if wide {
            let tree_w = TREE.min(inner.width / 3);
            let tree = Rect {
                width: tree_w,
                ..inner
            };
            self.draw_sidebar(frame, tree);
            for y in inner.y..inner.y + inner.height {
                if let Some(c) = frame.buffer_mut().cell_mut((inner.x + tree_w, y)) {
                    c.set_symbol("│").set_style(Style::default().fg(p().line));
                }
            }
            Rect {
                x: inner.x + tree_w + 1,
                width: inner.width.saturating_sub(tree_w + 1),
                ..inner
            }
        } else {
            frame.render_widget(Paragraph::new(self.crumb()), Rect { height: 1, ..inner });
            Rect {
                y: inner.y + 1,
                height: inner.height.saturating_sub(1),
                ..inner
            }
        };
        self.draw_body(frame, body);
        if !wide && self.sb.is_some() {
            frame.render_widget(Clear, body);
            self.draw_sidebar(frame, body);
        }
    }

    /// The line a narrow pane shows in place of the sidebar (REQ-232).
    fn crumb(&self) -> Line<'static> {
        let dim = Style::default().fg(p().dim);
        let mut spans = vec![Span::styled(
            format!(" {} ", self.which.name()),
            Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
        )];
        match (self.which, self.selected()) {
            (Which::Commits, Some(c)) => {
                let n = match &self.list {
                    Load::Ready(l) => l.list.len(),
                    _ => 0,
                };
                spans.push(Span::styled(
                    c.short.clone(),
                    Style::default().fg(p().yellow),
                ));
                spans.push(Span::styled(format!(" {} ", c.subject), dim));
                spans.push(Span::styled(format!("({}/{n}) ", self.commit + 1), dim));
            }
            _ => spans.push(Span::styled(
                format!("· {} files ", self.big().unwrap_or(self.files().len())),
                dim,
            )),
        }
        spans.push(Span::styled("· Tab sidebar", dim));
        Line::from(spans)
    }

    fn draw_sidebar(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.rows();
        let items = self.items();
        let here = self.current(&items);
        // Keep the cursor, or the file being read, in sight in a long list.
        let focus = self
            .sb
            .or_else(|| {
                let tree = Tree::new(self.files());
                let at = tree.shown_as(here, &self.pos().closed);
                rows.iter().position(|r| *r == at)
            })
            .unwrap_or(0);
        let room = area.height as usize;
        let first = focus.saturating_sub(room.saturating_sub(1));
        self.sb_at = Some((area, first));
        let w = area.width as usize;
        let lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(first)
            .take(room)
            .map(|(i, r)| {
                let mut line = self.row_line(*r, here, w);
                if self.sb == Some(i) {
                    line = line.style(Style::default().bg(p().sel));
                }
                line
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// How far in the tree starts: a level more below a commit.
    fn tree_indent(&self) -> usize {
        if self.which == Which::Commits { 6 } else { 2 }
    }

    fn row_line(&self, row: Row, here: usize, w: usize) -> Line<'static> {
        let dim = Style::default().fg(p().dim);
        match row {
            Row::Section(which) => {
                let on = which == self.which;
                let label = format!(
                    " {} {} {}",
                    which.n(),
                    if on { "▾" } else { "▸" },
                    which.name()
                );
                let count = self.count(which);
                let pad = w.saturating_sub(label.chars().count() + count.width() + 1);
                let style = if on {
                    Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p().dim).add_modifier(Modifier::BOLD)
                };
                Line::from(vec![
                    Span::styled(label, style),
                    Span::raw(" ".repeat(pad)),
                    count,
                    Span::raw(" "),
                ])
                .style(Style::default().bg(p().panel))
            }
            Row::Commit(i) => {
                let on = i == self.commit;
                let Load::Ready(l) = &self.list else {
                    return Line::default();
                };
                let c = &l.list[i];
                let age = age(c.when);
                let room = w.saturating_sub(c.short.chars().count() + age.chars().count() + 6);
                let subject = truncate(&c.subject, room);
                let pad = w.saturating_sub(
                    4 + c.short.chars().count() + 1 + subject.chars().count() + age.chars().count(),
                );
                Line::from(vec![
                    Span::styled(
                        if on { "  ▶ " } else { "    " },
                        Style::default().fg(p().blue),
                    ),
                    Span::styled(c.short.clone(), Style::default().fg(p().yellow)),
                    Span::styled(
                        format!(" {subject}"),
                        if on {
                            Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(p().fg)
                        },
                    ),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(age, dim),
                ])
            }
            Row::File(i) => {
                let Some(f) = self.files().get(i) else {
                    return Line::default();
                };
                let on = i == here;
                let viewed = self.pos().viewed.contains(&f.path);
                let tree = Tree::new(self.files());
                let indent = " ".repeat(self.tree_indent() + 2 * tree.file_depth[i]);
                let (mark, color) = status_mark(f);
                let name = f.path.rsplit('/').next().unwrap_or(&f.path);
                let room = w.saturating_sub(indent.len() + 4);
                let style = if on {
                    Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
                } else if viewed {
                    dim
                } else {
                    Style::default().fg(p().fg)
                };
                let mut line = Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(mark, Style::default().fg(color)),
                    Span::styled(format!(" {}", truncate(name, room)), style),
                    if viewed {
                        Span::styled(" ✓", Style::default().fg(p().green))
                    } else {
                        Span::raw("")
                    },
                ]);
                if on {
                    line = line.style(Style::default().bg(p().panel));
                }
                line
            }
            Row::Dir(d) => {
                let tree = Tree::new(self.files());
                let Some(dir) = tree.dirs.get(d) else {
                    return Line::default();
                };
                let closed = self.pos().closed.contains(&dir.key);
                let indent = " ".repeat(self.tree_indent() + 2 * dir.depth);
                let mut counts = vec![
                    Span::styled(format!("{} ", dir.files), dim),
                    Span::styled(format!("+{} ", dir.added), Style::default().fg(p().green)),
                    Span::styled(format!("−{}", dir.deleted), Style::default().fg(p().yellow)),
                ];
                let wide: usize = counts.iter().map(|s| s.content.chars().count()).sum();
                let mut room = w.saturating_sub(indent.len() + 2 + wide + 2);
                if room < 8 {
                    counts.truncate(1);
                    room =
                        w.saturating_sub(indent.len() + 2 + counts[0].content.chars().count() + 2);
                }
                let label = truncate(&format!("{}/", dir.label), room);
                let pad = room.saturating_sub(label.chars().count()) + 1;
                let mut spans = vec![
                    Span::raw(indent),
                    Span::styled(if closed { "▸ " } else { "▾ " }, dim),
                    Span::styled(label, Style::default().fg(p().fg)),
                    Span::raw(" ".repeat(pad)),
                ];
                spans.extend(counts);
                Line::from(spans)
            }
            Row::More => Line::from(Span::styled("    older commits not shown after 200", dim)),
        }
    }

    fn draw_body(&mut self, frame: &mut Frame, body: Rect) {
        let dim = Style::default().fg(p().dim);
        let mut note = |text: String, style: Style| {
            frame.render_widget(Paragraph::new(Span::styled(text, style)), body);
        };
        match self.content() {
            Content::Loading => {
                return note(format!(" {} loading…", spinner()), dim);
            }
            Content::Failed(e) => {
                let first = e.lines().next().unwrap_or("git failed");
                return note(format!(" {first}"), Style::default().fg(p().red));
            }
            Content::NoCommits => {
                let past = if self.base.is_empty() {
                    String::new()
                } else {
                    format!(" beyond origin/{}", self.base)
                };
                return note(format!(" No commits{past}."), dim);
            }
            Content::Shown(s) if s.files.is_empty() => {
                let text = match self.which {
                    Which::General if self.base.is_empty() => {
                        " origin has no default branch: nothing to compare. 2 shows what is uncommitted."
                            .to_string()
                    }
                    Which::General => format!(" No commits beyond origin/{}.", self.base),
                    Which::Uncommitted => " Nothing uncommitted.".into(),
                    Which::Commits => " This commit changes no files.".into(),
                };
                return note(text, dim);
            }
            Content::Shown(_) => {}
        }

        // A diff too large to draw says so on a line of its own, above the list.
        let body = if let Some(total) = self.big() {
            let line = format!(
                " Too large to draw: {total} files, the first {} listed. ↵ opens one.",
                self.files().len()
            );
            frame.render_widget(
                Paragraph::new(Span::styled(line, Style::default().fg(p().yellow))),
                Rect { height: 1, ..body },
            );
            Rect {
                y: body.y + 1,
                height: body.height.saturating_sub(1),
                ..body
            }
        } else {
            body
        };
        let items = self.items();
        let top = self.pos().scroll.min(items.len().saturating_sub(1));
        self.pos_mut().scroll = top;
        let current = self.current(&items);
        self.height = body.height as usize;
        let mut lines = Vec::with_capacity(self.height);
        // The current file's header stays on top while scrolling through it.
        lines.push(self.file_header(current, body.width));
        let start = top + usize::from(matches!(items.get(top), Some(Item::File(_))));
        for item in items.iter().skip(start).take(self.height.saturating_sub(1)) {
            lines.push(self.item_line(*item, body.width));
        }
        frame.render_widget(Paragraph::new(lines), body);
    }

    fn file_header(&self, f: usize, width: u16) -> Line<'static> {
        let file = &self.files()[f];
        let status = match file.status {
            Status::Added if file.untracked => "untracked",
            Status::Added => "new",
            Status::Deleted => "deleted",
            Status::Renamed => "renamed",
            Status::Modified => "",
        };
        let name = match &file.old_path {
            Some(old) => format!("{old} → {}", file.path),
            None => file.path.clone(),
        };
        let mut spans = vec![Span::styled(
            format!(" {name} "),
            Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
        )];
        if self.big().is_none() {
            spans.push(Span::styled(
                format!("+{} ", file.added()),
                Style::default().fg(p().green),
            ));
            spans.push(Span::styled(
                format!("−{} ", file.deleted()),
                Style::default().fg(p().yellow),
            ));
        }
        if !status.is_empty() {
            spans.push(Span::styled(
                format!(" {status} "),
                Style::default().fg(p().cyan),
            ));
        }
        if self.pos().viewed.contains(&file.path) {
            spans.push(Span::styled(" ✓ viewed ", Style::default().fg(p().green)));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::raw(" ".repeat((width as usize).saturating_sub(used))));
        Line::from(spans).style(Style::default().bg(p().panel))
    }

    fn item_line(&self, item: Item, width: u16) -> Line<'static> {
        match item {
            Item::File(f) => self.file_header(f, width),
            Item::Gap => Line::default(),
            Item::Note(f) => {
                let file = &self.files()[f];
                let note = if self.big().is_some() {
                    " not drawn"
                } else if file.binary {
                    " binary file"
                } else {
                    " no text changes"
                };
                Line::from(Span::styled(note, Style::default().fg(p().dim)))
            }
            Item::Hunk(f, h) => {
                let hunk = &self.files()[f].hunks[h];
                Line::from(vec![
                    Span::styled(
                        format!(" @@ -{} +{} @@ ", hunk.old_start, hunk.new_start),
                        Style::default().fg(p().cyan),
                    ),
                    Span::styled(hunk.context.clone(), Style::default().fg(p().dim)),
                ])
            }
            Item::Row(f, h, r) => {
                let file = &self.files()[f];
                let hunk = &file.hunks[h];
                if self.split {
                    split_line(&diff::split_rows(hunk)[r], width, &file.path)
                } else {
                    uni_line(&hunk.lines[r], width, &file.path)
                }
            }
        }
    }
}

/// A file's letter in the sidebar and its colour (REQ-243).
fn status_mark(f: &File) -> (&'static str, Color) {
    match f.status {
        Status::Added if f.untracked => ("?", p().magenta),
        Status::Added => ("A", p().green),
        Status::Deleted => ("D", p().red),
        Status::Renamed => ("R", p().cyan),
        Status::Modified => ("M", p().yellow),
    }
}

/// A braille frame for the time it is; the pane is drawn at least twice a
/// second, so a spinner turns while something loads (REQ-228).
fn spinner() -> String {
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    FRAMES[(ms / 120) as usize % FRAMES.len()].to_string()
}

/// How long ago a commit was made: `5m`, `2h`, `3d`, `7w`, `3y`.
fn age(when: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let s = (now - when).max(0);
    match s {
        0..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=604_799 => format!("{}d", s / 86_400),
        604_800..=31_535_999 => format!("{}w", s / 604_800),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// One side-by-side row: old on the left, new on the right, changed words
/// of a paired change painted brighter.
fn split_line(row: &diff::Row, width: u16, path: &str) -> Line<'static> {
    let half = (width.saturating_sub(1) / 2) as usize;
    let (mut lw, mut rw) = (Vec::new(), Vec::new());
    if let (Some(l), Some(r)) = (row.left, row.right)
        && l.kind == Kind::Del
        && r.kind == Kind::Add
    {
        (lw, rw) = diff::word_changes(&expand_tabs(&l.text), &expand_tabs(&r.text));
    }
    let mut spans = side(row.left, half, &lw, true, path);
    spans.push(Span::styled("│", Style::default().fg(p().line)));
    spans.extend(side(row.right, half, &rw, false, path));
    Line::from(spans)
}

fn side(
    line: Option<&diff::Line>,
    width: usize,
    words: &[diff::Span],
    old: bool,
    path: &str,
) -> Vec<Span<'static>> {
    let Some(l) = line else {
        // Nothing on this side: left blank, as GitHub greys it out.
        return vec![Span::styled(" ".repeat(width), Style::default().bg(p().bg))];
    };
    let no = if old { l.old_no } else { l.new_no };
    let (bg, word_bg, sign) = match l.kind {
        Kind::Del => (p().del_bg, p().del_word, "-"),
        Kind::Add => (p().add_bg, p().add_word, "+"),
        Kind::Ctx => (p().bg, p().bg, " "),
    };
    let gutter = format!(
        "{:>4} {sign}",
        no.map(|n| n.to_string()).unwrap_or_default()
    );
    let text_w = width.saturating_sub(gutter.chars().count() + 1);
    let mut spans = vec![
        Span::styled(gutter, Style::default().fg(p().dim).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ];
    spans.extend(marked(&l.text, text_w, words, bg, word_bg, path));
    spans
}

fn uni_line(l: &diff::Line, width: u16, path: &str) -> Line<'static> {
    let (bg, sign) = match l.kind {
        Kind::Del => (p().del_bg, "-"),
        Kind::Add => (p().add_bg, "+"),
        Kind::Ctx => (p().bg, " "),
    };
    let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let gutter = format!("{:>4} {:>4} {sign} ", num(l.old_no), num(l.new_no));
    let text_w = (width as usize).saturating_sub(gutter.chars().count());
    let mut spans = vec![Span::styled(gutter, Style::default().fg(p().dim).bg(bg))];
    spans.extend(marked(&l.text, text_w, &[], bg, bg, path));
    Line::from(spans)
}

/// `text` cut or padded to `width`, with the byte ranges in `words` on
/// `word_bg`.
fn marked(
    text: &str,
    width: usize,
    words: &[diff::Span],
    bg: Color,
    word_bg: Color,
    path: &str,
) -> Vec<Span<'static>> {
    // Offsets in `words` are into the tab-expanded text, like this one.
    let text = expand_tabs(text);
    let in_words = |at: usize| words.iter().any(|&(s, e)| s <= at && at < e);
    // Syntax colours give the foreground, the diff gives the background:
    // walk the highlighted pieces and cut them again where a changed word
    // starts or ends.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    let mut at = 0;
    let paint = |changed: Option<bool>, fg: Color| {
        let bg = if changed == Some(true) { word_bg } else { bg };
        Style::default().fg(fg).bg(bg)
    };
    let mut full = false;
    for piece in crate::view::highlight::highlight_line(&text, path) {
        let fg = piece.style.fg.unwrap_or(p().fg);
        let mut run = String::new();
        let mut run_changed = None;
        for c in piece.content.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > width {
                full = true;
                break;
            }
            let changed = in_words(at);
            if run_changed.is_some_and(|r| r != changed) {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    paint(run_changed, fg),
                ));
            }
            run_changed = Some(changed);
            run.push(c);
            at += c.len_utf8();
            used += cw;
        }
        // The run the cut fell in still shows, up to the cut.
        if !run.is_empty() {
            spans.push(Span::styled(run, paint(run_changed, fg)));
        }
        if full {
            break;
        }
    }
    if used < width {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }
    spans
}

fn expand_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(v: &mut DiffView, c: KeyCode) -> Action {
        v.key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn chr(v: &mut DiffView, c: char) -> Action {
        key(v, KeyCode::Char(c))
    }

    fn asked(a: Action) -> Request {
        match a {
            Action::Load(r) => r,
            _ => panic!("expected a request for git work"),
        }
    }

    /// Files named `names`, each with one hunk of `n` changed lines.
    fn files(names: &[&str], n: usize) -> Loaded {
        let mut text = String::new();
        for name in names {
            text += &format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n");
            text += &format!("@@ -1,{n} +1,{n} @@ fn f()\n");
            for i in 0..n {
                text += &format!("-old {i}\n+new {i}\n");
            }
        }
        Loaded::Files(diff::parse(&text))
    }

    fn view(general: Loaded) -> DiffView {
        DiffView::new("auth".into(), "main".into(), "/r".into(), Ok(general))
    }

    fn commit(sha: &str, subject: &str) -> Commit {
        Commit {
            sha: sha.into(),
            short: sha[..3].into(),
            subject: subject.into(),
            when: 0,
        }
    }

    fn list(shas: &[&str]) -> Part {
        Part::List(Commits {
            list: shas.iter().map(|s| commit(s, "subject")).collect(),
            more: false,
        })
    }

    fn screen(v: &mut DiffView, w: u16, h: u16) -> String {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        t.draw(|f| v.draw(f, f.area())).unwrap();
        let buf = t.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_changes_take_any_key_and_any_width() {
        let mut v = view(Loaded::Files(Vec::new()));
        for c in ['t', 'v', 'j', 'G', '(', ')'] {
            chr(&mut v, c);
        }
        assert!(matches!(key(&mut v, KeyCode::Enter), Action::None));
        for w in [60, 120] {
            screen(&mut v, w, 10);
        }
    }

    /// REQ-222, REQ-235: a pane opens on Uncommitted and the other views load
    /// when first asked for, once.
    #[test]
    fn other_views_load_on_first_visit_only() {
        let mut v = view(files(&["a.rs"], 1));
        assert_eq!(asked(chr(&mut v, '3')).slot, Slot::General);
        assert!(
            matches!(chr(&mut v, '3'), Action::None),
            "already on the way"
        );
        assert_eq!(asked(chr(&mut v, '2')).slot, Slot::Commits);
        assert!(
            matches!(chr(&mut v, '1'), Action::None),
            "Uncommitted is held"
        );
    }

    /// REQ-233: each view keeps its own scroll and viewed files.
    #[test]
    fn each_view_keeps_its_scroll_and_viewed_files() {
        let mut v = view(files(&["a.rs", "b.rs", "c.rs"], 3));
        chr(&mut v, 'j');
        chr(&mut v, 'v');
        let (scroll, viewed) = (v.pos().scroll, v.pos().viewed.clone());
        assert_eq!(viewed.len(), 1);

        let r = asked(chr(&mut v, '3'));
        assert!(
            v.apply(r.slot, r.seq, Ok(Part::Files(files(&["u.rs"], 1))))
                .is_none()
        );
        assert_eq!((v.pos().scroll, v.pos().viewed.len()), (0, 0));
        chr(&mut v, 'v');

        chr(&mut v, '1');
        assert_eq!((v.pos().scroll, v.pos().viewed.clone()), (scroll, viewed));
        chr(&mut v, '3');
        assert!(v.pos().viewed.contains("u.rs"));
    }

    /// REQ-236: an answer whose number is old is dropped.
    #[test]
    fn a_stale_answer_is_dropped() {
        let mut v = view(files(&["a.rs"], 1));
        let r = asked(chr(&mut v, '2'));
        assert!(
            v.apply(Slot::Commits, r.seq + 7, Ok(list(&["aaa111"])))
                .is_none()
        );
        assert!(screen(&mut v, 120, 12).contains("loading"));
        let more = v.apply(Slot::Commits, r.seq, Ok(list(&["aaa111", "bbb222"])));
        assert_eq!(more.map(|r| r.slot), Some(Slot::Commit("aaa111".into())));
    }

    /// REQ-225, REQ-231, REQ-233: the newest commit first, `(` and `)` move,
    /// a commit already loaded is not asked for again and keeps its scroll.
    #[test]
    fn commits_step_and_keep_their_place() {
        let mut v = view(files(&["a.rs"], 1));
        let r = asked(chr(&mut v, '2'));
        let r = v
            .apply(r.slot, r.seq, Ok(list(&["aaa111", "bbb222"])))
            .unwrap();
        assert_eq!(r.slot, Slot::Commit("aaa111".into()));
        assert!(
            v.apply(r.slot, r.seq, Ok(Part::Files(files(&["x.rs", "y.rs"], 4))))
                .is_none()
        );
        chr(&mut v, 'j');
        let top = v.pos().scroll;
        assert!(top > 0);

        let r = asked(chr(&mut v, ')'));
        assert_eq!(r.slot, Slot::Commit("bbb222".into()));
        v.apply(r.slot, r.seq, Ok(Part::Files(files(&["z.rs"], 1))));
        assert_eq!(v.pos().scroll, 0);
        assert!(
            matches!(chr(&mut v, '('), Action::None),
            "held, not asked again"
        );
        assert_eq!(v.pos().scroll, top);
        assert!(
            matches!(chr(&mut v, '('), Action::None),
            "no commit before the first"
        );
    }

    /// REQ-230: `Tab` into the sidebar, walk, choose a view, `Tab` back.
    #[test]
    fn the_sidebar_takes_the_keys_until_tab_or_esc() {
        let mut v = view(files(&["a.rs", "b.rs"], 1));
        assert!(!v.in_sidebar());
        key(&mut v, KeyCode::Tab);
        assert!(v.in_sidebar());
        // Uncommitted, its two files, Commits, General.
        key(&mut v, KeyCode::Down);
        key(&mut v, KeyCode::Down);
        key(&mut v, KeyCode::Down);
        let r = asked(key(&mut v, KeyCode::Enter));
        assert_eq!(r.slot, Slot::Commits);
        assert!(
            v.in_sidebar(),
            "choosing a view keeps the cursor in the sidebar"
        );
        key(&mut v, KeyCode::Esc);
        assert!(!v.in_sidebar());
        // A file closes it and goes there.
        key(&mut v, KeyCode::Tab);
        chr(&mut v, '1');
        key(&mut v, KeyCode::Down);
        key(&mut v, KeyCode::Down);
        key(&mut v, KeyCode::Enter);
        assert!(!v.in_sidebar());
        assert_eq!(v.current(&v.items()), 1);
    }

    /// REQ-227, REQ-232: three views stacked; below 100 columns a line and an
    /// overlay in place of the sidebar.
    #[test]
    fn the_sidebar_is_beside_the_code_or_over_it() {
        let mut v = view(files(&["a.rs", "b.rs"], 2));
        let wide = screen(&mut v, 130, 16);
        for want in [
            "1 ▾ Uncommitted",
            "2 ▸ Commits",
            "3 ▸ General",
            "a.rs",
            "not committed",
        ] {
            assert!(wide.contains(want), "{want}\n{wide}");
        }
        let narrow = screen(&mut v, 70, 16);
        assert!(
            narrow.contains("Uncommitted") && narrow.contains("Tab sidebar"),
            "{narrow}"
        );
        assert!(!narrow.contains("2 ▸ Commits"), "{narrow}");
        key(&mut v, KeyCode::Tab);
        let over = screen(&mut v, 70, 16);
        assert!(
            over.contains("2 ▸ Commits") && over.contains("3 ▸ General"),
            "{over}"
        );
    }

    /// REQ-242: what the title says of each view.
    #[test]
    fn the_title_names_the_view() {
        let mut v = view(files(&["a.rs"], 1));
        assert!(screen(&mut v, 120, 8).contains("auth · not committed"));
        chr(&mut v, '3');
        assert!(screen(&mut v, 120, 8).contains("auth vs main · committed"));
        let r = asked(chr(&mut v, '2'));
        v.apply(r.slot, r.seq, Ok(list(&["aaa111", "bbb222", "ccc333"])));
        assert!(screen(&mut v, 120, 8).contains("auth · 3 commits"));
    }

    /// REQ-238, REQ-246, REQ-226: a failed view says why, a too-large one
    /// lists its files, a base-less General says there is nothing to compare.
    #[test]
    fn a_view_can_fail_be_too_large_or_have_no_base() {
        let mut v = view(files(&["a.rs"], 1));
        let r = asked(chr(&mut v, '3'));
        v.apply(
            r.slot,
            r.seq,
            Err("git diff: fatal: bad thing\nsecond line".into()),
        );
        let s = screen(&mut v, 120, 10);
        assert!(
            s.contains("git diff: fatal: bad thing") && !s.contains("second line"),
            "{s}"
        );
        chr(&mut v, '1');
        assert!(
            screen(&mut v, 120, 10).contains("a.rs"),
            "the others stay usable"
        );

        let big = Loaded::TooLarge {
            files: vec![(Status::Added, "huge.log".into())],
            total: 214,
        };
        let mut v = view(big);
        // The first draw of a narrow pane re-anchors the scroll; the banner stays.
        let narrow = screen(&mut v, 70, 10);
        assert!(
            narrow.contains("Too large to draw") && narrow.contains("214 files"),
            "{narrow}"
        );
        assert!(!narrow.contains("+0"), "{narrow}");
        let s = screen(&mut v, 120, 10);
        assert!(s.contains("Too large to draw: 214 files"), "{s}");
        assert!(s.contains("huge.log"));
        assert!(matches!(key(&mut v, KeyCode::Enter), Action::Open(p) if p == "huge.log"));

        let mut v = DiffView::new("auth".into(), String::new(), "/r".into(), Ok(files(&[], 0)));
        let r = asked(chr(&mut v, '3'));
        v.apply(r.slot, r.seq, Ok(Part::Files(files(&[], 0))));
        assert!(screen(&mut v, 120, 10).contains("origin has no default branch"));
    }

    fn rows_text(v: &DiffView) -> Vec<String> {
        let tree = Tree::new(v.files());
        v.rows()
            .into_iter()
            .map(|r| match r {
                Row::Section(w) => w.name().to_string(),
                Row::Dir(d) => format!("{}{}/", " ".repeat(tree.dirs[d].depth), tree.dirs[d].label),
                Row::File(f) => format!(
                    "{}{}",
                    " ".repeat(tree.file_depth[f]),
                    v.files()[f].path.rsplit('/').next().unwrap()
                ),
                _ => String::new(),
            })
            .collect()
    }

    /// REQ-247, REQ-248: directories first, a chain of single-child
    /// directories is one row, the scroll follows the tree.
    #[test]
    fn the_sidebar_is_a_tree_with_compact_folders() {
        let v = view(files(
            &[
                "README.md",
                "crates/tui/src/a.rs",
                "crates/tui/src/b.rs",
                "crates/core/x.rs",
                "docs/specs/s.md",
            ],
            1,
        ));
        assert_eq!(
            rows_text(&v),
            [
                "Uncommitted",
                "crates/",
                " core/",
                "  x.rs",
                " tui/src/",
                "  a.rs",
                "  b.rs",
                "docs/specs/",
                " s.md",
                "README.md",
                "Commits",
                "General",
            ]
        );
        let paths: Vec<_> = v.files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "crates/core/x.rs",
                "crates/tui/src/a.rs",
                "crates/tui/src/b.rs",
                "docs/specs/s.md",
                "README.md"
            ]
        );
        let tree = Tree::new(v.files());
        let crates = &tree.dirs[0];
        assert_eq!((crates.files, crates.added, crates.deleted), (3, 3, 3));
    }

    /// REQ-249: `h` and `l` close and open a directory or move to its first
    /// child or parent, `↵` toggles it; a closed one hides its rows only.
    #[test]
    fn directories_close_and_open_in_the_sidebar() {
        let mut v = view(files(&["a/x.rs", "a/b/y.rs", "a/b/z.rs", "top.rs"], 1));
        key(&mut v, KeyCode::Tab);
        key(&mut v, KeyCode::Down);
        assert_eq!(v.sb, Some(1));
        assert!(matches!(chr(&mut v, 'h'), Action::None));
        assert!(v.is_closed(0));
        assert_eq!(
            rows_text(&v),
            ["Uncommitted", "a/", "top.rs", "Commits", "General"]
        );
        assert_eq!(v.files().len(), 4, "the scroll keeps its files");
        chr(&mut v, 'l');
        assert!(!v.is_closed(0));
        chr(&mut v, 'l');
        assert_eq!(v.sb, Some(2), "l on an open directory steps in");
        chr(&mut v, 'j');
        chr(&mut v, 'h');
        assert_eq!(v.sb, Some(2), "h on a file goes to its directory");
        key(&mut v, KeyCode::Enter);
        assert!(v.is_closed(1));
        assert_eq!(v.rows().len(), 7);
    }

    /// REQ-224, REQ-243: an untracked file shows `?` and the others their letter.
    #[test]
    fn marks_tell_untracked_from_added() {
        let mut f = diff::parse(
            "diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1 @@\n+x\n",
        );
        assert_eq!(status_mark(&f[0]).0, "A");
        f[0].untracked = true;
        assert_eq!(status_mark(&f[0]).0, "?");
    }

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_long_line_shows_up_to_the_cut() {
        let spans = marked("let x = 1; // a comment", 12, &[], p().bg, p().bg, "a.rs");
        assert_eq!(text(&spans), "let x = 1; /");
        assert_eq!(
            text(&marked("abcdefgh", 5, &[], p().bg, p().bg, "a.txt")),
            "abcde"
        );
    }

    #[test]
    fn wide_characters_keep_to_their_column() {
        let spans = marked("中文中文", 5, &[], p().bg, p().bg, "a.txt");
        assert_eq!(text(&spans), "中文 ");
        assert_eq!(spans.iter().map(Span::width).sum::<usize>(), 5);
    }
}
