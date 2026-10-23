//! The viewer of text files of the terminal client (spec REQ-615 to REQ-620, REQ-622; prototype
//! `docs/prototypes/file-viewer.html`): a read-only screen over the panes that shows the lines the
//! daemon read, with a bar above, the number of each line, the colour of each token, a search and
//! wrapping on demand. This is its state and what its keys do; where it is on the screen and what
//! it asks of the daemon are the client's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use contract::text::{Run, TextDoc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::viewer::fold;
use super::{Grid, Style, clip, erase, is_markdown, look, select};

/// What the person asked of the client by a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    Close,
    /// `esc` with nothing to cancel: give the keys back to the client's commands (REQ-720); `q`
    /// closes.
    Leave,
    Copy(String),
    /// Read the file again.
    Reread,
    /// Show the `.md` rendered, as the Markdown viewer does.
    Rendered,
    /// Give the keys to the file tree beside the viewer, to open another file in a tab (REQ-640).
    Tree,
    /// Write where these lines are (from 1, both included) in the field of the agent: `path` is
    /// the file as it was asked of the daemon (REQ-639).
    Refer {
        path: String,
        from: usize,
        to: usize,
    },
}

#[derive(Default)]
struct Search {
    typing: bool,
    buf: String,
    query: String,
    at: usize,
}

/// A tab is this many columns (REQ-614).
const TAB: &str = "    ";

pub struct FileView {
    /// The workspace the file was asked of, for the next `r` or `s`.
    pub workspace: String,
    doc: TextDoc,
    /// What the daemon refused (a binary file), instead of lines.
    refusal: Option<String>,
    /// Whether the file is a `.md`, which can be read rendered.
    markdown: bool,
    /// The file as it was asked of the daemon, from the workspace's folder; empty when it is not known.
    rel: String,
    /// The text of each line as it is in the file, for `y`.
    plain: Vec<String>,
    /// The characters of each line once its tabs are spaces.
    len: Vec<usize>,
    scroll: usize,
    /// The cursor: a line and a character of it (its tabs are spaces), and the column it goes back
    /// to when a line is long enough (REQ-639).
    line: usize,
    col: usize,
    want: usize,
    /// Where the selection began, when one is open: it covers up to the cursor, both included.
    anchor: Option<(usize, usize)>,
    /// A press of the mouse that is still down: the cell it began on and whether it moved (REQ-640).
    pull: Option<((usize, usize), bool)>,
    wrap: bool,
    side: usize,
    search: Search,
    /// The matches of the search: the line and the column in characters, in order.
    found: Vec<(usize, usize)>,
    says: String,
    most: Cell<usize>,
    /// Where each line begins, in rows, for the width the lines were wrapped at.
    starts: RefCell<Option<(usize, Rc<Vec<usize>>)>>,
}

/// Where the parts of the viewer are in the rectangle it covers.
struct Geo {
    /// The columns of the numbers, with the bar `│` and a space.
    gutter: usize,
    /// The characters of a line shown in a row, with a column left for `›`.
    cw: usize,
    top: usize,
    rows: usize,
    notice_y: Option<usize>,
    search_y: Option<usize>,
}

impl FileView {
    pub fn new(workspace: String, mut doc: TextDoc) -> FileView {
        let markdown = is_markdown(&doc.title);
        let plain = doc
            .lines
            .iter()
            .map(|l| l.iter().map(|r| r.t.as_str()).collect())
            .collect();
        for run in doc.lines.iter_mut().flatten() {
            if run.t.contains('\t') {
                run.t = run.t.replace('\t', TAB);
            }
        }
        let len = doc
            .lines
            .iter()
            .map(|l| l.iter().map(|r| r.t.chars().count()).sum())
            .collect();
        FileView {
            workspace,
            doc,
            refusal: None,
            markdown,
            rel: String::new(),
            plain,
            len,
            scroll: 0,
            line: 0,
            col: 0,
            want: 0,
            anchor: None,
            pull: None,
            wrap: false,
            side: 0,
            search: Search::default(),
            found: Vec::new(),
            says: String::new(),
            most: Cell::new(0),
            starts: RefCell::new(None),
        }
    }

    /// The name of the file, as the tab says it.
    #[cfg(test)]
    pub fn doc_title(&self) -> &str {
        &self.doc.title
    }

    /// Where the cursor is: a line and a character of it, both from 0.
    #[cfg(test)]
    pub fn cursor(&self) -> (usize, usize) {
        (self.line, self.col)
    }

    /// Whether a search is being typed: the keys are its letters.
    pub fn typing(&self) -> bool {
        self.search.typing
    }

    /// The path the file was asked of the daemon by, from the workspace's folder.
    pub fn at(mut self, rel: String) -> FileView {
        self.rel = rel;
        self
    }

    /// The file from the workspace's folder, for a reference and for reading it again (the canonical
    /// path when the asked one is not known).
    pub fn rel(&self) -> &str {
        if self.rel.is_empty() {
            &self.doc.path
        } else {
            &self.rel
        }
    }

    /// What is shown for a file the daemon would not read: its `path`, `title` and why.
    pub fn refused(workspace: String, path: String, title: String, why: String) -> FileView {
        let mut v = FileView::new(
            workspace,
            TextDoc {
                title,
                path,
                size: 0,
                truncated: false,
                lines: Vec::new(),
            },
        );
        v.refusal = Some(why);
        v
    }

    /// The canonical path of the file, which the editor and a reread are given.
    pub fn path(&self) -> &str {
        &self.doc.path
    }

    /// A message for the status bar (a copy, or that nothing was found), once.
    pub fn take_says(&mut self) -> Option<String> {
        (!self.says.is_empty()).then(|| std::mem::take(&mut self.says))
    }

    /// Keep what the person was reading when the same file is read again.
    pub fn keep_place(&mut self, old: &FileView) {
        self.scroll = old.scroll;
        self.wrap = old.wrap;
        self.side = old.side;
        self.go_line(old.line);
        self.go_col(old.col);
    }

    fn geo(&self, (_, y, w, h): (usize, usize, usize, usize)) -> Geo {
        let digits = self.doc.lines.len().max(1).to_string().len();
        let gutter = digits + 3;
        let last = (y + h).saturating_sub(1);
        let mut foot = h.saturating_sub(1);
        let search_y = (self.search.typing || !self.search.query.is_empty()).then(|| {
            foot = foot.saturating_sub(1);
            last
        });
        let notice_y = self.doc.truncated.then(|| {
            foot = foot.saturating_sub(1);
            last.saturating_sub(usize::from(search_y.is_some()))
        });
        Geo {
            gutter,
            cw: w.saturating_sub(gutter + 2).max(1),
            top: y + 1,
            rows: foot.max(1),
            notice_y,
            search_y,
        }
    }

    /// The row each line begins at when the lines are wrapped at `cw` characters.
    fn starts(&self, cw: usize) -> Rc<Vec<usize>> {
        let mut cache = self.starts.borrow_mut();
        if let Some((at, starts)) = cache.as_ref()
            && *at == cw
        {
            return Rc::clone(starts);
        }
        let mut at = 0;
        let mut starts = Vec::with_capacity(self.len.len() + 1);
        for n in &self.len {
            starts.push(at);
            at += n.div_ceil(cw).max(1);
        }
        starts.push(at);
        let starts = Rc::new(starts);
        *cache = Some((cw, Rc::clone(&starts)));
        starts
    }

    /// The rows the file takes at `cw` characters.
    fn total(&self, cw: usize) -> usize {
        if self.wrap {
            *self.starts(cw).last().unwrap_or(&0)
        } else {
            self.doc.lines.len()
        }
    }

    /// The line a row belongs to and the row of that line it is.
    fn line_at(&self, row: usize, cw: usize) -> (usize, usize) {
        if !self.wrap {
            return (row, 0);
        }
        let starts = self.starts(cw);
        let line = starts.partition_point(|s| *s <= row).saturating_sub(1);
        (line, row - starts[line].min(row))
    }

    /// The row a line begins at.
    fn row_of(&self, line: usize, cw: usize) -> usize {
        if self.wrap {
            self.starts(cw).get(line).copied().unwrap_or(0)
        } else {
            line
        }
    }

    fn clamp(&mut self, rect: (usize, usize, usize, usize)) {
        let geo = self.geo(rect);
        let most = self.total(geo.cw).saturating_sub(geo.rows);
        self.most.set(most);
        self.scroll = self.scroll.min(most);
    }

    pub fn scroll_by(&mut self, up: bool, n: usize) {
        self.scroll = if up {
            self.scroll.saturating_sub(n)
        } else {
            (self.scroll + n).min(self.most.get())
        };
    }

    pub fn key(&mut self, ev: &KeyEvent, rect: (usize, usize, usize, usize)) -> Act {
        if self.search.typing {
            return self.search_key(ev, rect);
        }
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return Act::Nothing;
        }
        if self.refusal.is_some() {
            return match ev.code {
                KeyCode::Esc => Act::Leave,
                KeyCode::Char('q') => Act::Close,
                _ => Act::Nothing,
            };
        }
        self.clamp(rect);
        let geo = self.geo(rect);
        let page = geo.rows.saturating_sub(1).max(1);
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.anchor.take().is_some() {
                    // the selection lets go first
                } else if self.search.query.is_empty() {
                    return if ev.code == KeyCode::Esc {
                        Act::Leave
                    } else {
                        Act::Close
                    };
                } else {
                    self.search = Search::default();
                    self.found.clear();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.go_line(self.line + 1),
            KeyCode::Char('k') | KeyCode::Up => self.go_line(self.line.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => self.go_col(self.col + 1),
            KeyCode::Char('h') | KeyCode::Left => self.go_col(self.col.saturating_sub(1)),
            KeyCode::Char(' ') | KeyCode::PageDown => self.go_line(self.line + page),
            KeyCode::PageUp => self.go_line(self.line.saturating_sub(page)),
            KeyCode::Char('e') => self.word_end(),
            KeyCode::Char('b') => self.word_back(),
            KeyCode::Char('g') | KeyCode::Home => self.go_line(0),
            KeyCode::Char('G') | KeyCode::End => self.go_line(usize::MAX),
            KeyCode::Char('w') => self.toggle_wrap(geo.cw),
            KeyCode::Char('t') => return Act::Tree,
            KeyCode::Char('v') => {
                self.anchor = match self.anchor {
                    Some(_) => None,
                    None => Some((self.line, self.col)),
                };
            }
            KeyCode::Enter => {
                let (from, to) = match self.selection() {
                    Some(((a, _), (b, _))) => (a, b),
                    None => (self.line, self.line),
                };
                self.anchor = None;
                return Act::Refer {
                    path: self.rel().to_owned(),
                    from: from + 1,
                    to: to + 1,
                };
            }
            KeyCode::Char('/') => {
                self.search.typing = true;
                self.search.buf.clear();
            }
            KeyCode::Char('n') => self.next_match(true, geo.cw),
            KeyCode::Char('N') => self.next_match(false, geo.cw),
            KeyCode::Char('y') => {
                let text = match self.selected_text() {
                    Some(text) => text,
                    None => self.plain.join("\n"),
                };
                self.anchor = None;
                self.says = super::conversation::copied(&text);
                return Act::Copy(text);
            }
            KeyCode::Char('r') => return Act::Reread,
            KeyCode::Char('s') if self.markdown => return Act::Rendered,
            _ => {}
        }
        self.follow(rect);
        self.clamp(rect);
        Act::Nothing
    }

    /// The lines of the file, for a cursor to be on one.
    fn last_line(&self) -> usize {
        self.doc.lines.len().saturating_sub(1)
    }

    /// The cursor goes to line `to` (the last one if it is past it), at the column it wants.
    fn go_line(&mut self, to: usize) {
        self.line = to.min(self.last_line());
        let n = self.len.get(self.line).copied().unwrap_or(0);
        self.col = self.want.min(n.saturating_sub(1));
    }

    /// The cursor goes to character `to` of its line (the last one if it is past it), and wants to stay there.
    fn go_col(&mut self, to: usize) {
        let n = self.len.get(self.line).copied().unwrap_or(0);
        self.col = to.min(n.saturating_sub(1));
        self.want = self.col;
    }

    /// The characters of line `n`, for a motion by words.
    fn chars_of(&self, n: usize) -> Vec<char> {
        self.line_text(n).chars().collect()
    }

    /// The cursor goes to the end of the word it is on, or of the next one when it is already there
    /// (Neovim's `e`, REQ-639): a word is letters, digits and `_`, or a run of other marks, and a
    /// blank or a line break is the space between them.
    fn word_end(&mut self) {
        let last = self.last_line();
        let (mut l, mut c) = (self.line, self.col + 1);
        let mut chars = self.chars_of(l);
        // forward over the blanks and the line breaks
        loop {
            while c < chars.len() && chars[c].is_whitespace() {
                c += 1;
            }
            if c < chars.len() {
                break;
            }
            if l >= last {
                return;
            }
            l += 1;
            c = 0;
            chars = self.chars_of(l);
        }
        let class = word_class(chars[c]);
        while c + 1 < chars.len() && word_class(chars[c + 1]) == class {
            c += 1;
        }
        self.go_to(l, c);
    }

    /// The cursor goes to the start of the word it is on, or of the one before when it is already there (Neovim's `b`).
    fn word_back(&mut self) {
        let (mut l, mut c) = (self.line, self.col);
        let mut chars = self.chars_of(l);
        // back over the blanks and the line breaks
        loop {
            while c > 0 && chars.get(c - 1).is_some_and(|ch| ch.is_whitespace()) {
                c -= 1;
            }
            if c > 0 {
                break;
            }
            if l == 0 {
                return;
            }
            l -= 1;
            chars = self.chars_of(l);
            c = chars.len();
        }
        let class = word_class(chars[c - 1]);
        while c > 1 && word_class(chars[c - 2]) == class {
            c -= 1;
        }
        self.go_to(l, c - 1);
    }

    /// The cursor goes to character `col` of line `line`, and wants to stay at that column.
    fn go_to(&mut self, line: usize, col: usize) {
        self.line = line;
        self.col = col;
        self.want = col;
    }

    /// The press that began a drag was the second of a double click: the word under the cursor is
    /// selected, and the release copies it (REQ-637, REQ-640b).
    pub fn widen_to_word(&mut self) {
        let Some((from, to)) = select::word_at(&self.line_text(self.line), self.col) else {
            return;
        };
        self.anchor = Some((self.line, from));
        self.go_to(self.line, to);
        if let Some((start, _)) = self.pull {
            self.pull = Some((start, true));
        }
    }

    /// The row of the screen the cursor is on, counted from the first of the file.
    fn cursor_row(&self, cw: usize) -> usize {
        self.row_of(self.line, cw) + if self.wrap { self.col / cw } else { 0 }
    }

    /// Scroll, and move the text sideways, only as far as the cursor needs to be in view: two rows
    /// of margin above and below (REQ-639).
    fn follow(&mut self, rect: (usize, usize, usize, usize)) {
        let geo = self.geo(rect);
        let most = self.total(geo.cw).saturating_sub(geo.rows);
        let row = self.cursor_row(geo.cw);
        let margin = 2.min(geo.rows.saturating_sub(1) / 2);
        if row < self.scroll + margin {
            self.scroll = row.saturating_sub(margin);
        } else if row + margin >= self.scroll + geo.rows {
            self.scroll = (row + margin + 1).saturating_sub(geo.rows);
        }
        self.scroll = self.scroll.min(most);
        if !self.wrap {
            if self.col < self.side {
                self.side = self.col;
            } else if self.col >= self.side + geo.cw {
                self.side = self.col + 1 - geo.cw;
            }
        }
    }

    /// What a selection covers, from the first end to the last, as (line, character) pairs.
    fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = (self.line, self.col);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// The text of line `n` as it is shown, its tabs being spaces. An empty file has no line 0.
    fn line_text(&self, n: usize) -> String {
        self.doc
            .lines
            .get(n)
            .map(|runs| runs.iter().map(|r| r.t.as_str()).collect())
            .unwrap_or_default()
    }

    /// The text the selection covers, line by line, when one is open.
    fn selected_text(&self) -> Option<String> {
        let ((l0, c0), (l1, c1)) = self.selection()?;
        let mut out = Vec::new();
        for n in l0..=l1 {
            let text: Vec<char> = self.line_text(n).chars().collect();
            let from = if n == l0 { c0 } else { 0 };
            let to = if n == l1 {
                (c1 + 1).min(text.len())
            } else {
                text.len()
            };
            out.push(
                text.get(from..to.max(from))
                    .unwrap_or(&[])
                    .iter()
                    .collect::<String>(),
            );
        }
        Some(out.join("\n"))
    }

    /// The line and the character of the text at the cell (`x`, `y`) of the screen, when it is on one.
    fn cell_at(
        &self,
        rect: (usize, usize, usize, usize),
        x: usize,
        y: usize,
    ) -> Option<(usize, usize)> {
        let geo = self.geo(rect);
        let text_x = rect.0 + geo.gutter;
        if y < geo.top || y >= geo.top + geo.rows || x < text_x {
            return None;
        }
        let scroll = self.scroll.min(self.most.get());
        let (line, sub) = self.line_at(scroll + (y - geo.top), geo.cw);
        if line >= self.doc.lines.len() {
            return None;
        }
        let from = if self.wrap { sub * geo.cw } else { self.side };
        Some((
            line,
            (from + x - text_x).min(self.len[line].saturating_sub(1)),
        ))
    }

    /// The button went down on the text: the cursor goes there, and a drag from it selects
    /// (REQ-640). `true` when it was on the text.
    pub fn press(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) -> bool {
        if self.refusal.is_some() {
            return false;
        }
        let Some((line, col)) = self.cell_at(rect, x, y) else {
            return false;
        };
        self.line = line;
        self.col = col;
        self.want = col;
        self.anchor = None;
        self.pull = Some(((line, col), false));
        true
    }

    /// The pointer moved with the button down: the selection goes from the cell the press began on to this one.
    pub fn drag_to(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) {
        let Some((start, _)) = self.pull else {
            return;
        };
        let Some((line, col)) = self.cell_at(rect, x, y) else {
            return;
        };
        if (line, col) == (self.line, self.col) {
            return;
        }
        self.anchor.get_or_insert(start);
        self.line = line;
        self.col = col;
        self.want = col;
        self.pull = Some((start, true));
    }

    /// The button was let go: what a drag covered, to copy; the selection stays, so `↵` writes its reference.
    pub fn release(&mut self) -> Option<String> {
        let (_, moved) = self.pull.take()?;
        if !moved {
            return None;
        }
        let text = self.selected_text()?;
        self.says = super::conversation::copied(&text);
        Some(text)
    }

    /// Wrap or cut the lines, staying at the line that is at the top.
    fn toggle_wrap(&mut self, cw: usize) {
        let (line, _) = self.line_at(self.scroll, cw);
        self.wrap = !self.wrap;
        self.side = 0;
        self.scroll = self.row_of(line, cw);
    }

    fn search_key(&mut self, ev: &KeyEvent, rect: (usize, usize, usize, usize)) -> Act {
        match ev.code {
            KeyCode::Esc => {
                self.search = Search::default();
                self.found.clear();
            }
            KeyCode::Enter => {
                self.search.typing = false;
                self.search.query = std::mem::take(&mut self.search.buf);
                self.search.at = 0;
                self.found = self.matches();
                if self.found.is_empty() {
                    if !self.search.query.is_empty() {
                        self.says = format!("no hay «{}» en el archivo", self.search.query);
                    }
                } else {
                    self.show(0, self.geo(rect).cw);
                    self.follow(rect);
                }
            }
            KeyCode::Backspace => {
                erase::back(&mut self.search.buf, ev);
            }
            KeyCode::Char(c) if !ev.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.buf.push(c);
            }
            _ => {}
        }
        self.clamp(rect);
        Act::Nothing
    }

    /// Every place the query is in the text, whatever its case.
    fn matches(&self) -> Vec<(usize, usize)> {
        let q: Vec<char> = fold(&self.search.query).chars().collect();
        let mut found = Vec::new();
        if q.is_empty() {
            return found;
        }
        for (n, line) in self.doc.lines.iter().enumerate() {
            let text: Vec<char> = fold(&line.iter().map(|r| r.t.as_str()).collect::<String>())
                .chars()
                .collect();
            let mut i = 0;
            while i + q.len() <= text.len() {
                if text[i..i + q.len()] == q[..] {
                    found.push((n, i));
                    i += q.len();
                } else {
                    i += 1;
                }
            }
        }
        found
    }

    fn next_match(&mut self, forward: bool, cw: usize) {
        let n = self.found.len();
        if n == 0 {
            return;
        }
        self.search.at = if forward {
            (self.search.at + 1) % n
        } else {
            (self.search.at + n - 1) % n
        };
        self.show(self.search.at, cw);
    }

    /// Go to match `at`: its row two rows from the top, and its column in view.
    fn show(&mut self, at: usize, cw: usize) {
        let Some(&(line, col)) = self.found.get(at) else {
            return;
        };
        let q = self.search.query.chars().count();
        self.line = line;
        self.col = col;
        self.want = col;
        let sub = if self.wrap { col / cw } else { 0 };
        self.scroll = (self.row_of(line, cw) + sub).saturating_sub(2);
        if !self.wrap && (col < self.side || col + q > self.side + cw) {
            self.side = col.saturating_sub(cw / 3);
        }
    }

    /// Draw the viewer over `rect`.
    pub fn draw(&self, g: &mut Grid, rect: (usize, usize, usize, usize)) {
        let (x, y, w, h) = rect;
        if w < 20 || h < 6 {
            return;
        }
        for row in y..y + h {
            g.put(x, row, &" ".repeat(w), Style::default());
        }
        let geo = self.geo(rect);
        let total = self.total(geo.cw);
        let most = total.saturating_sub(geo.rows);
        self.most.set(most);
        let scroll = self.scroll.min(most);
        let dim = Style::fg(look::dim());
        g.paint_bg(x, y, w, look::panel());
        let mut left = format!(" ▤ {}", self.doc.title);
        if !self.doc.lines.is_empty() || self.refusal.is_none() {
            left.push_str(&format!(" · {} líneas", self.doc.lines.len()));
        }
        if self.doc.size > 0 {
            left.push_str(&format!(" · {}", size_text(self.doc.size)));
        }
        g.put(
            x,
            y,
            &clip(&left, w.saturating_sub(14)),
            Style::fg(look::fg()).bold().on(look::panel()),
        );
        let pct = (scroll * 100).checked_div(most).unwrap_or(100);
        let right = if self.refusal.is_some() {
            format!("{pct} % ")
        } else {
            format!("L{}:{} · {pct} % ", self.line + 1, self.col + 1)
        };
        g.put(
            (x + w).saturating_sub(right.chars().count()),
            y,
            &right,
            dim.on(look::panel()),
        );
        if let Some(why) = &self.refusal {
            let msg = clip(why, w.saturating_sub(4));
            g.put(
                x + w.saturating_sub(msg.chars().count()) / 2,
                y + h / 2,
                &msg,
                Style::fg(look::yellow()),
            );
            return;
        }
        let digits = geo.gutter - 3;
        let text_x = x + geo.gutter;
        let current = self.found.get(self.search.at).copied();
        for r in 0..geo.rows {
            let cy = geo.top + r;
            let (line, sub) = self.line_at(scroll + r, geo.cw);
            if line >= self.doc.lines.len() {
                break;
            }
            let from = if self.wrap { sub * geo.cw } else { self.side };
            let to = from + geo.cw;
            let n = if sub == 0 {
                format!("{:>digits$}", line + 1)
            } else {
                " ".repeat(digits)
            };
            g.put(x, cy, &format!("{n} │ "), dim);
            let mut cx = text_x;
            for (text, class) in pieces(&self.doc.lines[line], from, to) {
                g.put_over(cx, cy, &text, class_style(class));
                cx += text.chars().count();
            }
            if !self.wrap && self.len[line] > to {
                g.put(text_x + geo.cw, cy, "›", Style::fg(look::yellow()));
            }
            if let Some(((l0, c0), (l1, c1))) = self.selection()
                && (l0..=l1).contains(&line)
            {
                let a = if line == l0 { c0 } else { 0 };
                let b = if line == l1 {
                    c1 + 1
                } else {
                    self.len[line].max(1)
                };
                let (a, b) = (a.max(from), b.min(to));
                if a < b {
                    g.light(text_x + a - from, text_x + b - from, cy);
                }
            }
            let first = self.found.partition_point(|m| m.0 < line);
            for (i, &(l, col)) in self.found.iter().enumerate().skip(first) {
                if l != line {
                    break;
                }
                let end = col + self.search.query.chars().count();
                if end > from && col < to {
                    let hit = if Some((l, col)) == current && i == self.search.at {
                        look::magenta()
                    } else {
                        look::yellow()
                    };
                    g.tint(
                        text_x + col.saturating_sub(from),
                        text_x + end.min(to) - from,
                        cy,
                        hit,
                        look::panel(),
                    );
                }
            }
            if line == self.line && (from..to).contains(&self.col) {
                let at = text_x + self.col - from;
                g.tint(at, at + 1, cy, look::fg(), look::panel());
            }
        }
        if let Some(at) = (scroll * (geo.rows - 1)).checked_div(most) {
            g.put(x + w - 1, geo.top + at, "▐", dim);
        }
        if let Some(ny) = geo.notice_y {
            let text = format!(
                " se muestra el primer MiB de {} · cortado en el final de una línea",
                size_text(self.doc.size)
            );
            g.put(x, ny, &clip(&text, w), Style::fg(look::yellow()));
        }
        if let Some(sy) = geo.search_y {
            let text = if self.search.typing {
                format!(" /{}▏", self.search.buf)
            } else if self.found.is_empty() {
                format!(" /{} · sin resultados", self.search.query)
            } else {
                format!(
                    " /{} · {}/{} · n N",
                    self.search.query,
                    self.search.at + 1,
                    self.found.len()
                )
            };
            g.paint_bg(x, sy, w, look::panel());
            g.put(
                x,
                sy,
                &clip(&text, w),
                Style::fg(look::yellow()).on(look::panel()),
            );
        }
    }

    /// What the status bar says the keys are.
    pub fn hint(&self) -> String {
        if self.refusal.is_some() {
            return "esc volver · q cerrar".into();
        }
        if let Some(((l0, _), (l1, _))) = self.selection() {
            let n = l1 - l0 + 1;
            let lines = if n == 1 { "línea" } else { "líneas" };
            return format!(
                "SELECCIÓN · {n} {lines} · ↵ escribe la referencia · y copiar · esc suelta"
            );
        }
        let md = if self.markdown { " · s vista" } else { "" };
        format!(
            "esc volver · q cerrar · h j k l e b cursor · v elegir · ↵ referencia · t archivos · ␣ g G página · / n N buscar · w ajustar · r releer · y copiar{md}"
        )
    }
}

/// How many files the viewer keeps open at once (REQ-640).
const MAX_TABS: usize = 8;

/// The files open in the viewer of text, a tab each (REQ-640): the tab that shows is the one the
/// keys are for, and each keeps its own cursor, selection and place.
pub struct FileTabs {
    tabs: Vec<FileView>,
    cur: usize,
    says: String,
}

impl FileTabs {
    pub fn new(first: FileView) -> FileTabs {
        FileTabs {
            tabs: vec![first],
            cur: 0,
            says: String::new(),
        }
    }

    /// The tab that shows.
    /// The files the tabs show, as they were asked of the daemon, the oldest first.
    pub fn rels(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.rel().to_owned()).collect()
    }

    pub fn current(&self) -> &FileView {
        &self.tabs[self.cur]
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.tabs.len()
    }

    /// A file was read. If it is open, its tab shows: with `keep` (it was read again) it takes the
    /// new reading where the person was reading, and without it (it was opened again) it stays as it
    /// was. Any other file is a new tab that shows; past `MAX_TABS` the oldest tab that is not the
    /// one showing goes.
    pub fn open(&mut self, mut view: FileView, keep: bool) {
        let same = |t: &FileView| t.workspace == view.workspace && t.doc.path == view.doc.path;
        if let Some(i) = self.tabs.iter().position(same) {
            if keep {
                view.keep_place(&self.tabs[i]);
                self.tabs[i] = view;
            }
            self.cur = i;
            return;
        }
        if self.tabs.len() >= MAX_TABS {
            let gone = usize::from(self.cur == 0);
            self.tabs.remove(gone);
            if self.cur > gone {
                self.cur -= 1;
            }
        }
        self.tabs.push(view);
        self.cur = self.tabs.len() - 1;
    }

    /// Show the next tab, or the previous one, going round.
    fn step(&mut self, back: bool) {
        let n = self.tabs.len();
        self.cur = if back {
            (self.cur + n - 1) % n
        } else {
            (self.cur + 1) % n
        };
    }

    /// The rectangle of the file under the bar of tabs, which a lone file does not have.
    fn under_bar(
        &self,
        (x, y, w, h): (usize, usize, usize, usize),
    ) -> (usize, usize, usize, usize) {
        if self.tabs.len() > 1 {
            (x, y + 1, w, h.saturating_sub(1))
        } else {
            (x, y, w, h)
        }
    }

    pub fn key(&mut self, ev: &KeyEvent, rect: (usize, usize, usize, usize)) -> Act {
        let inner = self.under_bar(rect);
        if !self.tabs[self.cur].typing() && self.tabs.len() > 1 {
            match ev.code {
                KeyCode::Tab => {
                    self.step(false);
                    return Act::Nothing;
                }
                KeyCode::BackTab => {
                    self.step(true);
                    return Act::Nothing;
                }
                _ => {}
            }
        }
        let act = self.tabs[self.cur].key(ev, inner);
        if act == Act::Close && self.tabs.len() > 1 {
            self.tabs.remove(self.cur);
            self.cur = self.cur.min(self.tabs.len() - 1);
            self.says = "pestaña cerrada".into();
            return Act::Nothing;
        }
        act
    }

    pub fn scroll_by(&mut self, up: bool, n: usize) {
        self.tabs[self.cur].scroll_by(up, n);
    }

    pub fn take_says(&mut self) -> Option<String> {
        if self.says.is_empty() {
            self.tabs[self.cur].take_says()
        } else {
            Some(std::mem::take(&mut self.says))
        }
    }

    pub fn hint(&self) -> String {
        let hint = self.tabs[self.cur].hint();
        if self.tabs.len() > 1 && self.tabs[self.cur].selection().is_none() {
            format!("{hint} · Tab pestañas")
        } else {
            hint
        }
    }

    /// Where the names of the tabs are on the bar: the column each begins at, its width and the tab.
    fn bar(&self, w: usize) -> Vec<(usize, usize, usize)> {
        let mut at = 0;
        let mut out = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let n = t.doc.title.chars().count() + 2;
            if at + n > w {
                break;
            }
            out.push((at, n, i));
            at += n + 1;
        }
        out
    }

    /// The bar is drawn when there are several tabs and the area is big enough for it.
    fn bar_shown(&self, w: usize, h: usize) -> bool {
        self.tabs.len() > 1 && w >= 20 && h >= 7
    }

    /// The tab whose name is at (`x`, `y`), when the bar is there.
    pub fn tab_at(&self, rect: (usize, usize, usize, usize), x: usize, y: usize) -> Option<usize> {
        let (rx, ry, w, h) = rect;
        if !self.bar_shown(w, h) || y != ry || x < rx {
            return None;
        }
        self.bar(w)
            .into_iter()
            .find(|(at, n, _)| (rx + at..rx + at + n).contains(&x))
            .map(|(.., i)| i)
    }

    /// The button went down: on a name of the bar it is that tab; on the text, the cursor (REQ-640).
    pub fn press(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) -> bool {
        if let Some(i) = self.tab_at(rect, x, y) {
            self.go(i);
            return false;
        }
        let inner = self.under_bar(rect);
        self.tabs[self.cur].press(inner, x, y)
    }

    pub fn drag_to(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) {
        let inner = self.under_bar(rect);
        self.tabs[self.cur].drag_to(inner, x, y);
    }

    /// The press was the second of a double click: the word under the cursor.
    pub fn widen_to_word(&mut self) {
        self.tabs[self.cur].widen_to_word();
    }

    pub fn release(&mut self) -> Option<String> {
        self.tabs[self.cur].release()
    }

    /// Show tab `i`.
    fn go(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.cur = i;
        }
    }

    /// Draw the bar of tabs, when there are several, and the file that shows under it.
    pub fn draw(&self, g: &mut Grid, rect: (usize, usize, usize, usize)) {
        let (x, y, w, h) = rect;
        if self.bar_shown(w, h) {
            g.put(x, y, &" ".repeat(w), Style::default());
            g.paint_bg(x, y, w, look::panel());
            for (at, n, i) in self.bar(w) {
                let style = if i == self.cur {
                    Style::fg(look::blue()).bold().on(look::sel())
                } else {
                    Style::fg(look::dim()).on(look::panel())
                };
                g.put(x + at, y, &format!(" {} ", self.tabs[i].doc.title), style);
                debug_assert_eq!(n, self.tabs[i].doc.title.chars().count() + 2);
            }
        }
        self.tabs[self.cur].draw(g, self.under_bar(rect));
    }
}

/// The accent of a syntax class (REQ-622); a missing or unknown class is plain.
fn class_style(class: Option<&str>) -> Style {
    let fg = match class {
        Some("kw") => look::magenta(),
        Some("str") => look::green(),
        Some("num") => look::yellow(),
        Some("ty" | "attr") => look::cyan(),
        Some("fn") => look::blue(),
        Some("cm") => {
            return Style {
                italic: true,
                ..Style::fg(look::dim())
            };
        }
        _ => look::fg(),
    };
    Style::fg(fg)
}

/// The text of the characters `from..to` of a line, as pieces of one class each.
fn pieces(line: &[Run], from: usize, to: usize) -> Vec<(String, Option<&str>)> {
    let mut out = Vec::new();
    let mut at = 0;
    for run in line {
        let n = run.t.chars().count();
        let (a, b) = (from.max(at), to.min(at + n));
        if a < b {
            let text: String = run.t.chars().skip(a - at).take(b - a).collect();
            out.push((text, run.k.as_deref()));
        }
        at += n;
        if at >= to {
            break;
        }
    }
    out
}

/// What a motion by words takes a character for: a blank (0), a letter, a digit or `_` (1), or another mark (2).
fn word_class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// A weight in bytes as a person reads it.
pub fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MiB", b / (KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: (usize, usize, usize, usize) = (0, 0, 60, 12);

    fn run(t: &str, k: Option<&str>) -> Run {
        Run {
            t: t.into(),
            k: k.map(Into::into),
        }
    }

    fn doc(title: &str, lines: Vec<Vec<Run>>) -> TextDoc {
        TextDoc {
            title: title.into(),
            path: format!("/work/{title}"),
            size: 2048,
            truncated: false,
            lines,
        }
    }

    fn numbered(n: usize) -> Vec<Vec<Run>> {
        (1..=n)
            .map(|i| vec![run(&format!("línea {i}"), None)])
            .collect()
    }

    fn view(lines: Vec<Vec<Run>>) -> FileView {
        FileView::new("ws".into(), doc("main.rs", lines))
    }

    fn key(v: &mut FileView, code: KeyCode) -> Act {
        v.key(&KeyEvent::new(code, KeyModifiers::NONE), R)
    }

    fn typed(v: &mut FileView, s: &str) {
        for c in s.chars() {
            key(v, KeyCode::Char(c));
        }
    }

    fn screen(v: &FileView) -> Vec<String> {
        let mut g = Grid::new(R.2, R.3);
        v.draw(&mut g, R);
        g.rows_plain()
    }

    #[test]
    fn the_bar_says_the_file_the_lines_and_the_weight_and_each_line_has_its_number() {
        let v = view(numbered(120));
        let rows = screen(&v);
        assert!(
            rows[0].contains("▤ main.rs · 120 líneas · 2.0 KB") && rows[0].contains("0 %"),
            "{}",
            rows[0]
        );
        assert!(rows[1].starts_with("  1 │ línea 1"), "{}", rows[1]);
        assert!(rows[10].starts_with(" 10 │ línea 10"), "{}", rows[10]);
    }

    /// REQ-639: `j` `k` move the cursor and the view follows it, with two rows of margin; the wheel
    /// scrolls without moving it.
    #[test]
    fn the_cursor_moves_by_a_line_a_page_and_to_the_ends_and_the_view_follows_it() {
        let mut v = view(numbered(120));
        screen(&v);
        key(&mut v, KeyCode::Char('j'));
        assert_eq!(
            (v.line, v.scroll),
            (1, 0),
            "the view stays while the cursor is in it"
        );
        key(&mut v, KeyCode::Up);
        key(&mut v, KeyCode::Char('k'));
        assert_eq!((v.line, v.scroll), (0, 0), "not above the start");
        for _ in 0..9 {
            key(&mut v, KeyCode::Char('j'));
        }
        assert_eq!(
            (v.line, v.scroll),
            (9, 1),
            "two rows of margin under the cursor"
        );
        key(&mut v, KeyCode::Char(' '));
        assert_eq!(v.line, 19, "a page is the rows of the text less one");
        key(&mut v, KeyCode::PageUp);
        assert_eq!(v.line, 9);
        key(&mut v, KeyCode::Char('G'));
        let rows = screen(&v);
        assert!(rows[11].contains("línea 120"), "{}", rows[11]);
        assert!(
            rows[0].contains("L120:1") && rows[0].contains("100 %"),
            "{}",
            rows[0]
        );
        key(&mut v, KeyCode::Char('j'));
        assert_eq!(v.line, 119, "not past the end");
        key(&mut v, KeyCode::Char('g'));
        assert_eq!((v.line, v.scroll), (0, 0));
        v.scroll_by(false, 5);
        assert_eq!(
            (v.line, v.scroll),
            (0, 5),
            "the wheel does not move the cursor"
        );
    }

    #[test]
    fn a_tab_is_four_columns_and_y_copies_the_file_as_it_is() {
        let mut v = view(vec![
            vec![run("\tfn a() {}", Some("kw"))],
            vec![run("x", None)],
        ]);
        let rows = screen(&v);
        assert!(rows[1].contains("│     fn a() {}"), "{}", rows[1]);
        assert_eq!(
            key(&mut v, KeyCode::Char('y')),
            Act::Copy("\tfn a() {}\nx".into()),
            "the tab is a tab in the copy"
        );
    }

    /// REQ-639: `l` and `h` move the cursor along the line and the text goes sideways only to keep
    /// it in view; wrapped, nothing goes sideways.
    #[test]
    fn a_long_line_is_cut_with_a_mark_the_cursor_walks_it_and_w_wraps_it() {
        let long = "abcdefghij".repeat(10);
        let mut v = view(vec![vec![run(&long, None)], vec![run("fin", None)]]);
        let rows = screen(&v);
        assert!(
            rows[1].ends_with('›') || rows[1].contains('›'),
            "{}",
            rows[1]
        );
        assert!(rows[1].contains("abcdefghij"));
        for _ in 0..53 {
            key(&mut v, KeyCode::Char('l'));
        }
        assert_eq!((v.col, v.side), (53, 0), "the last column in view");
        key(&mut v, KeyCode::Char('l'));
        assert_eq!(
            (v.col, v.side),
            (54, 1),
            "one more, and the text goes one column"
        );
        for _ in 0..200 {
            key(&mut v, KeyCode::Char('l'));
        }
        assert_eq!(
            (v.col, v.side),
            (99, 100 - 54),
            "not past the end of the line"
        );
        let rows = screen(&v);
        assert!(rows[1].contains("│ ghijabcdefghij"), "{}", rows[1]);
        for _ in 0..200 {
            key(&mut v, KeyCode::Char('h'));
        }
        assert_eq!((v.col, v.side), (0, 0), "not left of the start");
        // wrapped: the whole line is there, without the mark, and the cursor goes down it
        key(&mut v, KeyCode::Char('w'));
        let rows = screen(&v);
        assert!(!rows.iter().any(|r| r.contains('›')), "{rows:?}");
        let text: String = rows[1..3]
            .iter()
            .map(|r| r.split("│ ").nth(1).unwrap_or("").trim_end().to_owned())
            .collect();
        assert_eq!(text, long);
        for _ in 0..60 {
            key(&mut v, KeyCode::Char('l'));
        }
        assert_eq!((v.col, v.side), (60, 0));
        assert_eq!(v.cursor_row(54), 1, "the second row of the line");
        assert!(rows.iter().any(|r| r.contains("fin")));
    }

    /// REQ-639: `v` opens a selection that the cursor grows, `y` copies it and `esc` lets go of it
    /// before it does anything else.
    #[test]
    fn v_opens_a_selection_that_the_cursor_grows_and_y_copies_it_and_esc_lets_go() {
        let mut v = view(numbered(5));
        key(&mut v, KeyCode::Char('j'));
        for _ in 0..2 {
            key(&mut v, KeyCode::Char('l'));
        }
        key(&mut v, KeyCode::Char('v'));
        key(&mut v, KeyCode::Char('j'));
        for _ in 0..2 {
            key(&mut v, KeyCode::Char('l'));
        }
        assert_eq!(v.selection(), Some(((1, 2), (2, 4))));
        assert!(v.hint().starts_with("SELECCIÓN · 2 líneas"), "{}", v.hint());
        let rows = screen(&v);
        let (lit, _) = look::selection();
        let at = |g: &Grid, x: usize, y: usize| g.cells[y * g.cols + x].1.bg;
        let mut g = Grid::new(R.2, R.3);
        v.draw(&mut g, R);
        assert_eq!(
            at(&g, 4 + 2, 2),
            Some(lit),
            "from the character the selection began on"
        );
        assert_eq!(at(&g, 4 + 1, 2), None, "not before it");
        assert_eq!(at(&g, 4, 3), Some(lit), "the next line, from its start");
        assert!(rows[0].contains("L3:5"), "{}", rows[0]);
        assert_eq!(
            key(&mut v, KeyCode::Char('y')),
            Act::Copy("nea 2\nlínea".into()),
            "the characters it covers, line by line"
        );
        assert_eq!(v.selection(), None, "copying lets go");
        key(&mut v, KeyCode::Char('v'));
        assert!(v.selection().is_none() || v.anchor.is_some());
        assert_eq!(
            key(&mut v, KeyCode::Esc),
            Act::Nothing,
            "esc lets go of the selection"
        );
        assert_eq!(v.anchor, None);
        assert_eq!(
            key(&mut v, KeyCode::Esc),
            Act::Leave,
            "and then it gives the keys back"
        );
    }

    /// REQ-639: `↵` asks for a reference of the selected lines, or of the line of the cursor.
    #[test]
    fn enter_asks_for_a_reference_of_the_selected_lines_or_of_the_line_of_the_cursor() {
        let mut v = view(numbered(9)).at("src/auth.rs".into());
        for _ in 0..2 {
            key(&mut v, KeyCode::Char('j'));
        }
        assert_eq!(
            key(&mut v, KeyCode::Enter),
            Act::Refer {
                path: "src/auth.rs".into(),
                from: 3,
                to: 3
            },
            "no selection: the line of the cursor"
        );
        key(&mut v, KeyCode::Char('v'));
        for _ in 0..3 {
            key(&mut v, KeyCode::Char('j'));
        }
        assert_eq!(
            key(&mut v, KeyCode::Enter),
            Act::Refer {
                path: "src/auth.rs".into(),
                from: 3,
                to: 6
            }
        );
        assert_eq!(v.anchor, None, "the selection lets go");
        // backwards it is the same range
        key(&mut v, KeyCode::Char('v'));
        for _ in 0..2 {
            key(&mut v, KeyCode::Char('k'));
        }
        assert_eq!(
            key(&mut v, KeyCode::Enter),
            Act::Refer {
                path: "src/auth.rs".into(),
                from: 4,
                to: 6
            }
        );
        // a viewer that does not know the asked path gives the canonical one
        let mut v = view(numbered(2));
        assert_eq!(
            key(&mut v, KeyCode::Enter),
            Act::Refer {
                path: "/work/main.rs".into(),
                from: 1,
                to: 1
            }
        );
    }

    /// REQ-639: the cursor is a block on its character and a reread keeps it where it was.
    #[test]
    fn the_cursor_is_drawn_on_its_character_and_a_reread_keeps_it() {
        let mut v = view(numbered(30));
        for _ in 0..4 {
            key(&mut v, KeyCode::Char('j'));
        }
        key(&mut v, KeyCode::Char('l'));
        let mut g = Grid::new(R.2, R.3);
        v.draw(&mut g, R);
        let cell = g.cells[5 * g.cols + 5 + 1];
        assert_eq!(cell.0, 'í');
        assert_eq!(
            (cell.1.bg, cell.1.fg),
            (Some(look::fg()), Some(look::panel()))
        );
        let mut again = view(numbered(3));
        again.keep_place(&v);
        assert_eq!(
            (again.line, again.col),
            (2, 1),
            "kept, and not past the end of the new file"
        );
    }

    #[test]
    fn a_search_lights_the_matches_and_n_goes_through_them_and_esc_ends_it() {
        let mut lines = numbered(60);
        lines[10] = vec![run("aquí está la CLAVE", None)];
        lines[40] = vec![run("otra clave más", None)];
        let mut v = view(lines);
        screen(&v);
        key(&mut v, KeyCode::Char('/'));
        typed(&mut v, "clave");
        key(&mut v, KeyCode::Enter);
        let rows = screen(&v);
        assert!(
            rows.iter().any(|r| r.contains("CLAVE")),
            "the first match is in view"
        );
        assert!(
            rows.last().unwrap().contains("/clave · 1/2"),
            "{:?}",
            rows.last()
        );
        key(&mut v, KeyCode::Char('n'));
        let rows = screen(&v);
        assert!(rows.iter().any(|r| r.contains("otra clave")));
        assert!(rows.last().unwrap().contains("2/2"));
        key(&mut v, KeyCode::Char('N'));
        assert_eq!(v.search.at, 0);
        assert_eq!(
            key(&mut v, KeyCode::Esc),
            Act::Nothing,
            "the first esc ends the search"
        );
        assert!(v.found.is_empty());
        assert_eq!(
            key(&mut v, KeyCode::Esc),
            Act::Leave,
            "the next gives the keys back"
        );
        // nothing found says so once
        key(&mut v, KeyCode::Char('/'));
        typed(&mut v, "zzz");
        key(&mut v, KeyCode::Enter);
        assert_eq!(v.take_says().as_deref(), Some("no hay «zzz» en el archivo"));
        assert_eq!(v.take_says(), None);
    }

    #[test]
    fn a_class_has_the_accent_of_the_theme_and_an_unknown_one_is_plain() {
        let mut v = view(vec![vec![
            run("fn", Some("kw")),
            run(" ", None),
            run("\"a\"", Some("str")),
            run(" // c", Some("cm")),
            run("x", Some("from-the-future")),
        ]]);
        v.col = 2; // the cursor is on the space, over no class
        let mut g = Grid::new(R.2, R.3);
        v.draw(&mut g, R);
        let at = |col: usize| g.cells[R.2 + 4 + col].1;
        assert_eq!(at(0).fg, Some(look::magenta()), "kw");
        assert_eq!(at(3).fg, Some(look::green()), "str");
        assert!(at(8).italic && at(8).fg == Some(look::dim()), "cm");
        assert_eq!(at(11).fg, Some(look::fg()), "unknown is plain");
    }

    #[test]
    fn a_file_that_was_cut_says_so_under_the_text_and_a_refusal_has_only_esc() {
        let mut d = doc("app.log", numbered(30));
        d.truncated = true;
        d.size = 3 * 1024 * 1024 + 1024 * 200;
        let v = FileView::new("ws".into(), d);
        let rows = screen(&v);
        assert!(
            rows.last()
                .unwrap()
                .contains("se muestra el primer MiB de 3.2 MiB"),
            "{}",
            rows.last().unwrap()
        );
        let mut v = FileView::refused(
            "ws".into(),
            "/work/logo.png".into(),
            "logo.png".into(),
            "binario · 48.0 KB · no se puede mostrar".into(),
        );
        let rows = screen(&v);
        assert!(
            rows.iter()
                .any(|r| r.contains("binario · 48.0 KB · no se puede mostrar"))
        );
        assert_eq!(key(&mut v, KeyCode::Char('j')), Act::Nothing);
        assert_eq!(
            key(&mut v, KeyCode::Char('e')),
            Act::Nothing,
            "no editor opens from here (N151)"
        );
        assert_eq!(key(&mut v, KeyCode::Esc), Act::Leave);
        assert_eq!(key(&mut v, KeyCode::Char('q')), Act::Close);
        assert_eq!(v.hint(), "esc volver · q cerrar");
    }

    #[test]
    fn r_and_s_ask_the_client_s_is_only_for_a_markdown_and_e_asks_nothing() {
        let mut v = view(numbered(3));
        assert_eq!(key(&mut v, KeyCode::Char('e')), Act::Nothing);
        assert_eq!(key(&mut v, KeyCode::Char('r')), Act::Reread);
        assert_eq!(key(&mut v, KeyCode::Char('s')), Act::Nothing);
        let mut md = FileView::new("ws".into(), doc("NOTES.md", numbered(3)));
        assert_eq!(key(&mut md, KeyCode::Char('s')), Act::Rendered);
        assert!(md.hint().contains("s vista"));
        assert!(!v.hint().contains("s vista"));
    }

    #[test]
    fn a_panel_with_no_room_or_no_lines_does_not_panic() {
        let mut v = view(Vec::new());
        for rect in [(0, 0, 10, 3), (0, 0, 60, 6), (5, 5, 20, 6)] {
            let mut g = Grid::new(80, 20);
            v.draw(&mut g, rect);
            v.key(&KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE), rect);
            v.key(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE), rect);
        }
        assert!(screen(&v)[0].contains("0 líneas"));
    }

    #[test]
    fn a_big_wrapped_file_is_laid_out_once_for_a_width() {
        let lines: Vec<Vec<Run>> = (0..50_000)
            .map(|i| vec![run(&"x".repeat(i % 200), None)])
            .collect();
        let mut v = view(lines);
        key(&mut v, KeyCode::Char('w'));
        screen(&v);
        let first = v.starts(30);
        assert!(Rc::ptr_eq(&first, &v.starts(30)), "kept for the same width");
        assert!(!Rc::ptr_eq(&first, &v.starts(31)));
    }

    fn file(name: &str, lines: usize) -> FileView {
        FileView::new("ws".into(), doc(name, numbered(lines)))
    }

    fn tab_key(t: &mut FileTabs, code: KeyCode) -> Act {
        t.key(&KeyEvent::new(code, KeyModifiers::NONE), R)
    }

    fn names(t: &FileTabs) -> Vec<&str> {
        t.tabs.iter().map(|v| v.doc.title.as_str()).collect()
    }

    /// REQ-640: each file is a tab with its own cursor, `Tab` and `⇧Tab` go round, and the bar is
    /// there only when there are two or more.
    #[test]
    fn two_files_are_two_tabs_with_each_its_own_cursor_and_tab_goes_round() {
        let mut t = FileTabs::new(file("a.rs", 20));
        let mut g = Grid::new(R.2, R.3);
        t.draw(&mut g, R);
        assert!(
            g.rows_plain()[0].contains("▤ a.rs"),
            "no bar for a lone file"
        );
        for _ in 0..3 {
            tab_key(&mut t, KeyCode::Char('j'));
        }
        t.open(file("b.rs", 20), false);
        assert_eq!(names(&t), ["a.rs", "b.rs"]);
        assert_eq!(t.current().doc.title, "b.rs", "the new one shows");
        let mut g = Grid::new(R.2, R.3);
        t.draw(&mut g, R);
        let rows = g.rows_plain();
        assert!(
            rows[0].contains(" a.rs ") && rows[0].contains(" b.rs"),
            "{}",
            rows[0]
        );
        assert!(rows[1].contains("▤ b.rs"), "{}", rows[1]);
        assert!(
            !rows[0].contains('─'),
            "nothing of what was under the bar stays: {}",
            rows[0]
        );
        tab_key(&mut t, KeyCode::Char('l'));
        assert_eq!(t.current().cursor(), (0, 1));
        tab_key(&mut t, KeyCode::Tab);
        assert_eq!(t.current().doc.title, "a.rs");
        assert_eq!(
            t.current().cursor(),
            (3, 0),
            "each tab keeps its own cursor"
        );
        tab_key(&mut t, KeyCode::BackTab);
        assert_eq!(t.current().doc.title, "b.rs", "going back goes round");
        tab_key(&mut t, KeyCode::Tab);
        tab_key(&mut t, KeyCode::Tab);
        assert_eq!(t.current().doc.title, "b.rs");
    }

    /// REQ-640: a file that is open goes to its tab; read again it takes the new reading where it was.
    #[test]
    fn a_file_that_is_open_goes_to_its_tab_and_a_reread_keeps_the_cursor() {
        let mut t = FileTabs::new(file("a.rs", 20));
        tab_key(&mut t, KeyCode::Char('j'));
        tab_key(&mut t, KeyCode::Char('j'));
        t.open(file("b.rs", 20), false);
        t.open(file("a.rs", 3), false);
        assert_eq!(names(&t), ["a.rs", "b.rs"], "no third tab");
        assert_eq!(t.current().doc.title, "a.rs");
        assert_eq!(t.current().doc.lines.len(), 20, "as it was, not read again");
        t.open(file("a.rs", 3), true);
        assert_eq!(t.current().doc.lines.len(), 3, "the new reading");
        assert_eq!(t.current().cursor(), (2, 0), "where it was");
    }

    /// REQ-640: at most eight tabs; the ninth closes the oldest that is not the one showing.
    #[test]
    fn the_ninth_tab_closes_the_oldest_that_is_not_showing() {
        let mut t = FileTabs::new(file("f0.rs", 3));
        for i in 1..8 {
            t.open(file(&format!("f{i}.rs"), 3), false);
        }
        assert_eq!(t.count(), 8);
        t.open(file("f8.rs", 3), false);
        assert_eq!(t.count(), 8);
        assert_eq!(names(&t)[0], "f1.rs", "the oldest went");
        assert_eq!(t.current().doc.title, "f8.rs");
        t.go(0);
        t.open(file("f9.rs", 3), false);
        assert_eq!(t.count(), 8);
        assert!(
            names(&t).contains(&"f1.rs"),
            "the one showing is kept: {:?}",
            names(&t)
        );
        assert!(
            !names(&t).contains(&"f2.rs"),
            "the next oldest went: {:?}",
            names(&t)
        );
    }

    /// REQ-640, REQ-723: `q` closes the tab that shows, and the last one closes the viewer; `esc`
    /// closes nothing: it gives the keys back.
    #[test]
    fn q_closes_a_tab_and_the_last_one_closes_the_viewer_and_esc_leaves() {
        let mut t = FileTabs::new(file("a.rs", 3));
        t.open(file("b.rs", 3), false);
        assert_eq!(tab_key(&mut t, KeyCode::Esc), Act::Leave);
        assert_eq!(names(&t), ["a.rs", "b.rs"]);
        assert_eq!(tab_key(&mut t, KeyCode::Char('q')), Act::Nothing);
        assert_eq!(names(&t), ["a.rs"]);
        assert_eq!(t.take_says().as_deref(), Some("pestaña cerrada"));
        assert_eq!(tab_key(&mut t, KeyCode::Char('q')), Act::Close);
        // a selection lets go first, and `Tab` with one tab is nothing
        let mut t = FileTabs::new(file("a.rs", 3));
        tab_key(&mut t, KeyCode::Char('v'));
        assert_eq!(tab_key(&mut t, KeyCode::Esc), Act::Nothing);
        assert_eq!(tab_key(&mut t, KeyCode::Tab), Act::Nothing);
        assert_eq!(tab_key(&mut t, KeyCode::Esc), Act::Leave);
    }

    /// REQ-640: a click on a name of the bar is that tab.
    #[test]
    fn a_name_of_the_bar_is_where_a_click_goes_to_its_tab() {
        let mut t = FileTabs::new(file("a.rs", 3));
        t.open(file("bb.rs", 3), false);
        assert_eq!(t.tab_at(R, 1, 0), Some(0));
        assert_eq!(
            t.tab_at(R, 7, 0),
            Some(1),
            "after the first name and a space"
        );
        assert_eq!(t.tab_at(R, 30, 0), None);
        assert_eq!(t.tab_at(R, 1, 1), None, "only on the bar");
        let lone = FileTabs::new(file("a.rs", 3));
        assert_eq!(lone.tab_at(R, 1, 0), None, "a lone file has no bar");
    }

    /// REQ-640: a click puts the cursor on the character, a drag selects and copies when it ends,
    /// and a click that does not move copies nothing.
    #[test]
    fn a_click_puts_the_cursor_there_and_a_drag_selects_and_copies_on_release() {
        let mut v = view(numbered(5));
        screen(&v);
        assert!(!v.press(R, 1, 2), "the numbers are not text");
        assert!(v.press(R, 4 + 2, 1 + 1));
        assert_eq!(v.cursor(), (1, 2));
        assert_eq!(
            v.release(),
            None,
            "a click that did not move copies nothing"
        );
        assert!(v.press(R, 4 + 2, 1 + 1));
        v.drag_to(R, 4 + 5, 1 + 2);
        assert_eq!(v.selection(), Some(((1, 2), (2, 5))));
        assert_eq!(v.release(), Some("nea 2\nlínea ".into()));
        assert_eq!(
            v.selection(),
            Some(((1, 2), (2, 5))),
            "the selection stays, for `↵` and `y`"
        );
        assert!(v.take_says().unwrap().starts_with("copiado"));
        assert!(!v.press(R, 4 + 2, 1 + 7), "below the last line is not text");
    }

    /// REQ-640: a click on a name of the bar goes to the tab and is not a drag.
    #[test]
    fn a_click_on_a_name_of_the_bar_goes_to_that_tab() {
        let mut t = FileTabs::new(file("a.rs", 3));
        t.open(file("b.rs", 3), false);
        assert!(!t.press(R, 1, 0));
        assert_eq!(t.current().doc.title, "a.rs");
        assert!(
            t.press(R, 4 + 1, 2),
            "under the bar the text is one row lower"
        );
        assert_eq!(t.current().cursor(), (0, 1));
    }

    /// REQ-639: `e` goes to the end of the word and `b` to the start of one, as Neovim's do, across
    /// marks and lines, and with a selection open they grow it.
    #[test]
    fn e_and_b_walk_the_words_as_neovims_do_across_marks_and_lines() {
        let mut v = view(vec![
            vec![run("let n = foo_bar(1);", None)],
            vec![],
            vec![run("  x", None)],
        ]);
        let at = |v: &FileView| v.cursor();
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 2), "the end of `let`");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 4), "the next word, `n`");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 6), "the `=` is a word of marks");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 14), "`foo_bar`: the underscore is a letter");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 15), "the `(`");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 16), "the `1`");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (0, 18), "`);` is a word of marks");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (2, 2), "past the empty line to the end of `x`");
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(at(&v), (2, 2), "nothing after the last word");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(at(&v), (0, 17), "`);` begins on the `)`");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(at(&v), (0, 16), "the `1`");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(at(&v), (0, 15), "the `(`");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(at(&v), (0, 8), "the start of `foo_bar`");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(at(&v), (0, 6), "the `=`");
        for _ in 0..4 {
            key(&mut v, KeyCode::Char('b'));
        }
        assert_eq!(at(&v), (0, 0), "not before the start");
        // a selection grows with them
        key(&mut v, KeyCode::Char('v'));
        key(&mut v, KeyCode::Char('e'));
        key(&mut v, KeyCode::Char('e'));
        assert_eq!(v.selection(), Some(((0, 0), (0, 4))));
    }

    /// REQ-637, REQ-640b: a double click selects the word under the cursor, and the release copies it.
    #[test]
    fn a_double_click_selects_the_word_and_the_release_copies_it_and_keeps_it() {
        let mut v = view(vec![vec![run("use std::time;", None)]]);
        screen(&v);
        assert!(v.press(R, 4 + 6, 1));
        v.widen_to_word();
        assert_eq!(v.selection(), Some(((0, 4), (0, 6))), "`std`");
        assert_eq!(v.release(), Some("std".into()));
        assert!(v.selection().is_some(), "and it stays");
        let mut none = view(vec![vec![run("a  b", None)]]);
        screen(&none);
        assert!(none.press(R, 4 + 1, 1));
        none.widen_to_word();
        assert_eq!(none.release(), None, "a blank is no word");
    }

    /// An empty file has no lines at all (the daemon sends zero for it): the word motions, the
    /// selection and the double click must not index into them.
    #[test]
    fn an_empty_file_stays_at_its_start_under_the_word_motions_and_selection() {
        let mut v = view(vec![]);
        screen(&v);
        for code in [KeyCode::Char('e'), KeyCode::Char('b'), KeyCode::Char('G')] {
            key(&mut v, code);
            assert_eq!((v.line, v.col), (0, 0));
        }
        key(&mut v, KeyCode::Char('v'));
        assert!(matches!(key(&mut v, KeyCode::Char('y')), Act::Copy(t) if t.is_empty()));
        assert!(!v.press(R, 4, 1), "no line to press on");
        v.widen_to_word();
        assert_eq!(v.release(), None);
    }

    /// A bar that is too small to draw is not a bar to click: its row is the file's.
    #[test]
    fn a_bar_too_small_to_draw_takes_no_click() {
        let mut t = FileTabs::new(FileView::new("ws".into(), doc("a.rs", numbered(3))));
        t.open(FileView::new("ws".into(), doc("b.rs", numbered(3))), false);
        assert_eq!(t.tab_at((0, 0, 19, 12), 1, 0), None);
        assert_eq!(t.tab_at((0, 0, 19, 6), 1, 0), None);
        assert_eq!(t.tab_at((0, 0, 60, 12), 1, 0), Some(0), "the bar is there");
    }
}
