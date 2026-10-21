//! The Markdown viewer of the terminal client (spec REQ-469 to REQ-478; prototype
//! `docs/prototypes/markdown.html`): a read-only screen over the panes that shows a document the
//! daemon parsed, with a bar above, the headings at the left when there is room, a search, a
//! drag that copies and, for a plan that waits, the three answers under it. This is its state and
//! what its keys and mouse do; where it is on the screen and what it is asked of the daemon are the
//! client's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use contract::markdown::MarkdownDoc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::markdown::{self, Layout};
use super::{Grid, Style, clip, look, select};

/// Where a document came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The plan that waits as `ask` in the conversation `node`.
    Plan { node: String, ask: String },
    /// A file of the workspace.
    File,
}

/// What the person asked of the client by a key or a click.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    Close,
    /// Answer the plan with this key (`accept`, `manual` or `no`).
    Answer(String),
    Copy(String),
    /// Open the document's file in the editor.
    Edit(String),
    /// Show the file of this path as text, as it is written (REQ-617).
    Source(String),
}

/// What a button let go of comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Released {
    Click(usize, usize),
    Copy(String),
    Nothing,
}

/// The headings are shown from this many columns on (REQ-474).
const OUTLINE_FROM: usize = 100;
const OUTLINE_W: usize = 26;

#[derive(Default)]
struct Search {
    typing: bool,
    buf: String,
    query: String,
    at: usize,
}

pub struct Viewer {
    doc: MarkdownDoc,
    pub source: Source,
    /// The workspace the file was asked of, for the switch to its source.
    pub workspace: String,
    scroll: usize,
    outline: bool,
    search: Search,
    laid: RefCell<Option<(usize, Rc<Layout>)>>,
    most: Cell<usize>,
    sel: Option<select::Selection>,
    pull: Option<select::Pull>,
    says: String,
    /// The ways to answer a plan that waits, as the question's box has them: the key and the words.
    answers: Vec<(String, String)>,
}

/// Where the parts of the viewer are in the rectangle it covers.
struct Geo {
    /// The columns of the headings, and where the text begins and how wide it is.
    outline: usize,
    text_x: usize,
    text_w: usize,
    /// The rows of the text.
    top: usize,
    rows: usize,
    /// The row of the search line and of the answers, when there are.
    search_y: Option<usize>,
    answers_y: Option<usize>,
}

impl Viewer {
    pub fn new(
        doc: MarkdownDoc,
        source: Source,
        answers: Vec<(String, String)>,
        workspace: String,
    ) -> Viewer {
        Viewer {
            doc,
            source,
            workspace,
            scroll: 0,
            outline: true,
            search: Search::default(),
            laid: RefCell::new(None),
            most: Cell::new(0),
            sel: None,
            pull: None,
            says: String::new(),
            answers,
        }
    }

    pub fn is_plan(&self) -> bool {
        matches!(self.source, Source::Plan { .. })
    }

    /// A message for the status bar (a copy), once.
    pub fn take_says(&mut self) -> Option<String> {
        (!self.says.is_empty()).then(|| std::mem::take(&mut self.says))
    }

    fn geo(&self, (x, y, w, h): (usize, usize, usize, usize)) -> Geo {
        let outline = if self.outline && w >= OUTLINE_FROM {
            OUTLINE_W
        } else {
            0
        };
        let last = (y + h).saturating_sub(1);
        let mut foot = h.saturating_sub(1);
        let answers_y = (self.is_plan() && !self.answers.is_empty()).then(|| {
            foot = foot.saturating_sub(1);
            last
        });
        let search_y = (self.search.typing || !self.search.query.is_empty()).then(|| {
            foot = foot.saturating_sub(1);
            last.saturating_sub(usize::from(answers_y.is_some()))
        });
        Geo {
            outline,
            text_x: x + outline + 1,
            text_w: w.saturating_sub(outline + 3),
            top: y + 1,
            rows: foot.max(1),
            search_y,
            answers_y,
        }
    }

    fn layout(&self, width: usize) -> Rc<Layout> {
        let mut cache = self.laid.borrow_mut();
        if let Some((w, l)) = cache.as_ref()
            && *w == width
        {
            return Rc::clone(l);
        }
        let l = Rc::new(markdown::layout(&self.doc, width));
        *cache = Some((width, Rc::clone(&l)));
        l
    }

    /// The rows that hold what is searched for, whatever the case.
    fn found(&self, l: &Layout) -> Vec<usize> {
        let q = fold(&self.search.query);
        if q.is_empty() {
            return Vec::new();
        }
        l.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| fold(&r.text()).contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.most.get());
    }

    pub fn scroll_by(&mut self, up: bool, n: usize) {
        self.scroll = if up {
            self.scroll.saturating_sub(n)
        } else {
            (self.scroll + n).min(self.most.get())
        };
    }

    /// A key (REQ-472 to REQ-477); `rect` is what the viewer covers.
    pub fn key(&mut self, ev: &KeyEvent, rect: (usize, usize, usize, usize)) -> Act {
        self.unlight();
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        self.most.set(layout.rows.len().saturating_sub(geo.rows));
        if self.search.typing {
            return self.search_key(ev, &layout);
        }
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return Act::Nothing;
        }
        let page = geo.rows.saturating_sub(1).max(1);
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.search.query.is_empty() {
                    return Act::Close;
                }
                self.search = Search::default();
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(false, 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(true, 1),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll_by(false, page),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_by(true, page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = self.most.get(),
            KeyCode::Char(']') => {
                if let Some(h) = layout.heads.iter().find(|h| h.row > self.scroll) {
                    self.scroll = h.row;
                }
            }
            KeyCode::Char('[') => {
                self.scroll = layout
                    .heads
                    .iter()
                    .rev()
                    .find(|h| h.row < self.scroll)
                    .map_or(0, |h| h.row);
            }
            KeyCode::Char('o') => self.outline = !self.outline,
            KeyCode::Char('/') => {
                self.search.typing = true;
                self.search.buf.clear();
            }
            KeyCode::Char('n') => self.next_match(&layout, true),
            KeyCode::Char('N') => self.next_match(&layout, false),
            KeyCode::Char('y') => {
                let text = whole(&layout);
                self.says = super::conversation::copied(&text);
                return Act::Copy(text);
            }
            KeyCode::Char('s') if self.source == Source::File => match &self.doc.path {
                Some(path) => return Act::Source(path.clone()),
                None => self.says = "este documento no tiene un archivo que abrir".into(),
            },
            KeyCode::Char('e') => match &self.doc.path {
                Some(path) => return Act::Edit(path.clone()),
                None => self.says = "este documento no tiene un archivo que abrir".into(),
            },
            KeyCode::Char(c @ '1'..='9') if self.is_plan() => {
                if let Some((key, _)) = self.answers.get(c as usize - '1' as usize) {
                    return Act::Answer(key.clone());
                }
            }
            _ => {}
        }
        self.clamp();
        Act::Nothing
    }

    fn search_key(&mut self, ev: &KeyEvent, layout: &Layout) -> Act {
        match ev.code {
            KeyCode::Esc => {
                self.search = Search::default();
            }
            KeyCode::Enter => {
                self.search.typing = false;
                self.search.query = std::mem::take(&mut self.search.buf);
                self.search.at = 0;
                if let Some(first) = self.found(layout).first() {
                    self.scroll = (*first).min(self.most.get());
                } else if !self.search.query.is_empty() {
                    self.says = format!("no hay «{}» en el documento", self.search.query);
                }
            }
            KeyCode::Backspace => {
                self.search.buf.pop();
            }
            KeyCode::Char(c) if !ev.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.buf.push(c);
            }
            _ => {}
        }
        Act::Nothing
    }

    fn next_match(&mut self, layout: &Layout, forward: bool) {
        let found = self.found(layout);
        if found.is_empty() {
            return;
        }
        let n = found.len();
        self.search.at = if forward {
            (self.search.at + 1) % n
        } else {
            (self.search.at + n - 1) % n
        };
        self.scroll = found[self.search.at].min(self.most.get());
    }

    pub fn unlight(&mut self) {
        self.sel = None;
        self.pull = None;
    }

    /// A press: on a heading at the left it goes there, on an answer it answers; on the text it
    /// begins a drag, which is a click if the pointer never moves (REQ-478). `true` when it
    /// began a drag.
    pub fn press(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) -> bool {
        self.unlight();
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        let start = self.scroll.min(layout.rows.len().saturating_sub(geo.rows));
        if !(geo.top..geo.top + geo.rows).contains(&y)
            || x < geo.text_x
            || x >= geo.text_x + geo.text_w
            || start + (y - geo.top) >= layout.rows.len()
        {
            return false;
        }
        let anchor = select::Point {
            line: start + (y - geo.top),
            col: x - geo.text_x,
        };
        self.pull = Some(select::Pull::new((x, y), anchor));
        true
    }

    /// The press that began a drag was the second of a double click: it covers the word it is on
    /// (REQ-637).
    pub fn widen_to_word(&mut self, rect: (usize, usize, usize, usize)) {
        let Some(anchor) = self.pull.as_ref().map(|p| p.anchor) else {
            return;
        };
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        let Some((a, b)) = layout
            .rows
            .get(anchor.line)
            .and_then(|r| select::word_span(anchor.line, &r.text(), anchor.col))
        else {
            return;
        };
        if let Some(pull) = self.pull.as_mut() {
            pull.anchor = a;
            pull.moved = true;
        }
        self.sel = Some(select::Selection {
            pane: String::new(),
            anchor: a,
            head: b,
        });
    }

    /// The pointer moved with the button down (REQ-462's way of selecting).
    pub fn drag_to(
        &mut self,
        rect: (usize, usize, usize, usize),
        (x, y): (usize, usize),
        now_ms: u64,
    ) {
        if self.pull.is_none() {
            return;
        }
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        let start = self.scroll.min(layout.rows.len().saturating_sub(geo.rows));
        let last = layout.rows.len().saturating_sub(1);
        let (head, edge) = if y < geo.top {
            (
                select::Point {
                    line: start,
                    col: 0,
                },
                Some(true),
            )
        } else if y >= geo.top + geo.rows {
            (
                select::Point {
                    line: (start + geo.rows - 1).min(last),
                    col: usize::from(u16::MAX),
                },
                Some(false),
            )
        } else {
            (
                select::Point {
                    line: (start + (y - geo.top)).min(last),
                    col: x.saturating_sub(geo.text_x),
                },
                None,
            )
        };
        let Some(pull) = self.pull.as_mut() else {
            return;
        };
        pull.to(head, edge, now_ms);
        self.sel = Some(select::Selection {
            pane: String::new(),
            anchor: pull.anchor,
            head,
        });
    }

    /// While a drag is held past the edge, scroll a line every so often.
    pub fn drag_tick(&mut self, rect: (usize, usize, usize, usize), now_ms: u64) {
        let Some(up) = self.pull.as_ref().and_then(|p| p.due(now_ms)) else {
            return;
        };
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        self.most.set(layout.rows.len().saturating_sub(geo.rows));
        self.scroll_by(up, 1);
        let start = self.scroll.min(self.most.get());
        let last = layout.rows.len().saturating_sub(1);
        let (line, col) = if up {
            (start, 0)
        } else {
            ((start + geo.rows - 1).min(last), usize::from(u16::MAX))
        };
        if let (Some(pull), Some(sel)) = (self.pull.as_mut(), self.sel.as_mut()) {
            pull.scrolled(now_ms);
            sel.head = select::Point { line, col };
        }
    }

    /// The button was let go: a drag that never moved is the click it was; one that did copies
    /// what it covers.
    pub fn release(&mut self, rect: (usize, usize, usize, usize)) -> Released {
        let Some(pull) = self.pull.take() else {
            return Released::Nothing;
        };
        if !pull.moved {
            self.sel = None;
            return Released::Click(pull.start.0, pull.start.1);
        }
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        let text = self
            .sel
            .as_ref()
            .map(|s| {
                let (a, b) = s.ends();
                markdown::text_of(&layout.rows, (a.line, a.col), (b.line, b.col))
            })
            .unwrap_or_default();
        if text.is_empty() {
            self.sel = None;
            return Released::Nothing;
        }
        self.says = super::conversation::copied(&text);
        Released::Copy(text)
    }

    /// A click that was no drag: a heading of the list at the left goes there, an answer of a
    /// plan answers.
    pub fn click(&mut self, rect: (usize, usize, usize, usize), x: usize, y: usize) -> Act {
        let geo = self.geo(rect);
        if let Some(ay) = geo.answers_y
            && y == ay
        {
            let mut at = rect.0 + 1;
            for (key, label) in &self.answers {
                let width = 3 + label.chars().count() + 2;
                if (at..at + width).contains(&x) {
                    return Act::Answer(key.clone());
                }
                at += width + 2;
            }
            return Act::Nothing;
        }
        if geo.outline > 0 && x < rect.0 + geo.outline && (geo.top..geo.top + geo.rows).contains(&y)
        {
            let layout = self.layout(geo.text_w);
            if let Some(h) = layout.heads.get(y - geo.top) {
                self.scroll = h.row.min(layout.rows.len().saturating_sub(geo.rows));
            }
        }
        Act::Nothing
    }

    /// Draw the viewer over `rect`.
    pub fn draw(&self, g: &mut Grid, rect: (usize, usize, usize, usize), plan_waits: bool) {
        let (x, y, w, h) = rect;
        if w < 20 || h < 6 {
            return;
        }
        for row in y..y + h {
            g.put(x, row, &" ".repeat(w), Style::default());
        }
        let geo = self.geo(rect);
        let layout = self.layout(geo.text_w);
        let most = layout.rows.len().saturating_sub(geo.rows);
        self.most.set(most);
        let scroll = self.scroll.min(most);
        let dim = Style::fg(look::dim());
        // The bar above: what it is, and how far it is read.
        g.paint_bg(x, y, w, look::panel());
        let (kind, name) = match &self.source {
            Source::Plan { .. } => ("plan", self.doc.title.as_str()),
            Source::File => ("archivo", self.doc.title.as_str()),
        };
        let left = format!(" ▤ {kind} · {}", clip(name, w.saturating_sub(30)));
        g.put(x, y, &left, Style::fg(look::fg()).bold().on(look::panel()));
        let pct = (scroll * 100).checked_div(most).unwrap_or(100);
        let right = format!(
            "{}{pct} % ",
            if self.is_plan() && plan_waits {
                "plan listo · "
            } else if self.doc.truncated {
                "cortado · "
            } else {
                ""
            }
        );
        let at = (x + w).saturating_sub(right.chars().count());
        g.put(at, y, &right, dim.on(look::panel()));
        // The headings at the left, the one being read lit.
        if geo.outline > 0 {
            let current = layout
                .heads
                .iter()
                .rposition(|h| h.row <= scroll + 1)
                .unwrap_or(0);
            for (i, head) in layout.heads.iter().take(geo.rows).enumerate() {
                let cy = geo.top + i;
                let mark = if head.level == 1 { "▤ " } else { "· " };
                let text = format!(
                    "{}{mark}{}",
                    " ".repeat((usize::from(head.level) - 1).min(4) * 2),
                    head.text
                );
                let style = if i == current {
                    Style::fg(look::blue()).bold().on(look::sel())
                } else if head.level == 1 {
                    Style::fg(look::fg()).bold()
                } else {
                    dim
                };
                if i == current {
                    g.paint_bg(x, cy, geo.outline - 1, look::sel());
                }
                g.put_over(x + 1, cy, &clip(&text, geo.outline - 3), style);
            }
            for cy in geo.top..geo.top + geo.rows {
                g.put(x + geo.outline - 1, cy, "│", dim);
            }
        }
        // The text.
        let found = self.found(&layout);
        let current = found.get(self.search.at).copied();
        let q: Vec<char> = fold(&self.search.query).chars().collect();
        for (r, row) in layout.rows.iter().skip(scroll).take(geo.rows).enumerate() {
            let cy = geo.top + r;
            let at = scroll + r;
            if let Some(bg) = row.bg {
                g.paint_bg(geo.text_x - 1, cy, geo.text_w + 2, bg);
            }
            let mut cx = geo.text_x;
            for (text, style) in &row.spans {
                let text = clip(text, (geo.text_x + geo.text_w).saturating_sub(cx));
                g.put_over(cx, cy, &text, *style);
                cx += text.chars().count();
            }
            if !q.is_empty() && found.binary_search(&at).is_ok() {
                let line: Vec<char> = fold(&row.text()).chars().collect();
                let hit = if Some(at) == current {
                    look::magenta()
                } else {
                    look::yellow()
                };
                let mut i = 0;
                while i + q.len() <= line.len() {
                    if line[i..i + q.len()] == q[..] {
                        g.tint(
                            geo.text_x + i,
                            (geo.text_x + i + q.len()).min(geo.text_x + geo.text_w),
                            cy,
                            hit,
                            look::panel(),
                        );
                        i += q.len();
                    } else {
                        i += 1;
                    }
                }
            }
            if let Some(s) = &self.sel
                && let Some((from, to)) = s.on_line(at, row.len())
            {
                g.light(
                    geo.text_x + from,
                    (geo.text_x + to).min(geo.text_x + geo.text_w),
                    cy,
                );
            }
        }
        // Where the reading is: a mark on the right edge.
        if let Some(at) = (scroll * (geo.rows - 1)).checked_div(most) {
            g.put(x + w - 1, geo.top + at, "▐", dim);
        }
        if let Some(sy) = geo.search_y {
            let text = if self.search.typing {
                format!(" /{}▏", self.search.buf)
            } else {
                format!(
                    " /{} · {} coincidencias{}",
                    self.search.query,
                    found.len(),
                    if found.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}/{}", self.search.at + 1, found.len())
                    }
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
        if let Some(ay) = geo.answers_y {
            let mut cx = x + 1;
            for (i, (_, label)) in self.answers.iter().enumerate() {
                let key = format!(" {} ", i + 1);
                let name = format!(" {label} ");
                let key_style = if plan_waits {
                    Style::fg(look::panel()).bold().on(look::green())
                } else {
                    Style::fg(look::panel()).on(look::dim())
                };
                g.put(cx, ay, &key, key_style);
                g.put(
                    cx + key.chars().count(),
                    ay,
                    &name,
                    Style::fg(look::fg()).on(look::sel()),
                );
                cx += key.chars().count() + name.chars().count() + 2;
            }
        }
    }

    /// What the status bar says the keys are.
    pub fn hint(&self) -> String {
        let answers = if self.is_plan() && !self.answers.is_empty() {
            " · 1 2 3 responde"
        } else {
            ""
        };
        let source = if self.source == Source::File {
            " · s fuente"
        } else {
            ""
        };
        format!(
            "esc cerrar · j k ␣ b g G mover · [ ] título · / n N buscar · o esquema · y copiar · e editar{source}{answers}"
        )
    }
}

/// The text of the whole document, as `y` copies it.
fn whole(l: &Layout) -> String {
    let last = l.rows.len().saturating_sub(1);
    markdown::text_of(&l.rows, (0, 0), (last, usize::MAX - 1))
}

/// The text with each character lowercased, one for one, so a column in it is the column in `s`.
pub(super) fn fold(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::markdown::tests::{doc, head, para};
    use contract::markdown::MdBlock;

    const WIDE: (usize, usize, usize, usize) = (0, 0, 120, 20);
    const NARROW: (usize, usize, usize, usize) = (0, 0, 80, 20);

    /// A document of three titles with twelve one-line paragraphs under each.
    fn long() -> MarkdownDoc {
        let mut blocks = Vec::new();
        for (n, title) in ["Contexto", "Cambios", "Riesgos"].iter().enumerate() {
            blocks.push(head(2, title));
            for k in 0..12 {
                blocks.push(para(&format!("línea {n}-{k}")));
            }
        }
        doc(blocks)
    }

    fn viewer(source: Source) -> Viewer {
        let answers = if matches!(source, Source::Plan { .. }) {
            [
                ("accept", "aprobar y editar sin preguntar"),
                ("manual", "aprobar, preguntando cada edición"),
                ("no", "seguir planeando"),
            ]
            .into_iter()
            .map(|(k, l)| (k.to_owned(), l.to_owned()))
            .collect()
        } else {
            Vec::new()
        };
        Viewer::new(long(), source, answers, String::new())
    }

    fn key(v: &mut Viewer, rect: (usize, usize, usize, usize), code: KeyCode) -> Act {
        v.key(&KeyEvent::new(code, KeyModifiers::NONE), rect)
    }

    fn typed(v: &mut Viewer, rect: (usize, usize, usize, usize), s: &str) {
        for c in s.chars() {
            key(v, rect, KeyCode::Char(c));
        }
    }

    fn screen(v: &Viewer, rect: (usize, usize, usize, usize), plan_waits: bool) -> Vec<String> {
        let mut g = Grid::new(rect.2, rect.3);
        v.draw(&mut g, rect, plan_waits);
        g.rows_plain()
    }

    fn row_of(rows: &[String], text: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("{text:?} is not drawn:\n{}", rows.join("\n")))
    }

    #[test]
    fn the_bar_says_what_it_is_and_how_far_it_is_read_and_the_keys_move_it() {
        let mut v = viewer(Source::File);
        let rows = screen(&v, NARROW, false);
        assert!(
            rows[0].contains("▤ archivo · doc") && rows[0].contains("0 %"),
            "{}",
            rows[0]
        );
        row_of(&rows, "## Contexto");
        assert!(!rows.iter().any(|r| r.contains("## Riesgos")));
        // a line, a page, the ends, and what cannot be passed
        key(&mut v, NARROW, KeyCode::Char('j'));
        assert_eq!(v.scroll, 1);
        key(&mut v, NARROW, KeyCode::Up);
        key(&mut v, NARROW, KeyCode::Char('k'));
        assert_eq!(v.scroll, 0, "not above the start");
        key(&mut v, NARROW, KeyCode::Char(' '));
        assert_eq!(v.scroll, 18, "a page is the rows of the text less one");
        key(&mut v, NARROW, KeyCode::Char('b'));
        assert_eq!(v.scroll, 0);
        key(&mut v, NARROW, KeyCode::Char('G'));
        let rows = screen(&v, NARROW, false);
        assert!(rows[0].contains("100 %"));
        row_of(&rows, "línea 2-11");
        let end = v.scroll;
        key(&mut v, NARROW, KeyCode::Char('j'));
        key(&mut v, NARROW, KeyCode::PageDown);
        assert_eq!(v.scroll, end, "not below the end");
        key(&mut v, NARROW, KeyCode::Char('g'));
        assert_eq!(v.scroll, 0);
        // the wheel
        v.scroll_by(false, 3);
        assert_eq!(v.scroll, 3);
        v.scroll_by(true, 9);
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn the_brackets_go_to_the_previous_and_next_heading() {
        let mut v = viewer(Source::File);
        screen(&v, NARROW, false);
        key(&mut v, NARROW, KeyCode::Char(']'));
        assert_eq!(
            v.scroll, 26,
            "a title, 12 lines and a blank between each block"
        );
        key(&mut v, NARROW, KeyCode::Char(']'));
        assert_eq!(v.scroll, 52);
        key(&mut v, NARROW, KeyCode::Char(']'));
        assert_eq!(v.scroll, 52, "no heading after the last");
        key(&mut v, NARROW, KeyCode::Char('['));
        assert_eq!(v.scroll, 26);
        key(&mut v, NARROW, KeyCode::Char('['));
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn the_headings_are_at_the_left_only_when_the_panel_is_wide_and_o_hides_them() {
        let mut v = viewer(Source::File);
        let wide = screen(&v, WIDE, false);
        let y = row_of(&wide, "· Cambios");
        assert!(wide[y].contains('│'), "{}", wide[y]);
        let narrow = screen(&v, NARROW, false);
        assert!(!narrow.iter().any(|r| r.contains("· Cambios")), "no room");
        key(&mut v, WIDE, KeyCode::Char('o'));
        assert!(
            !screen(&v, WIDE, false)
                .iter()
                .any(|r| r.contains("· Cambios"))
        );
        key(&mut v, WIDE, KeyCode::Char('o'));
        // a click on a heading of the list goes there
        let rows = screen(&v, WIDE, false);
        let y = row_of(&rows, "· Riesgos");
        assert_eq!(v.click(WIDE, 3, y), Act::Nothing);
        assert_eq!(v.scroll, 52);
    }

    #[test]
    fn a_search_reads_a_word_lights_the_matches_and_n_goes_through_them() {
        let mut v = viewer(Source::File);
        screen(&v, NARROW, false);
        key(&mut v, NARROW, KeyCode::Char('/'));
        typed(&mut v, NARROW, "LÍNEA 1-");
        let rows = screen(&v, NARROW, false);
        row_of(&rows, "/LÍNEA 1-▏");
        key(&mut v, NARROW, KeyCode::Backspace);
        key(&mut v, NARROW, KeyCode::Char('-'));
        key(&mut v, NARROW, KeyCode::Enter);
        assert_eq!(v.search.query, "LÍNEA 1-");
        let first = v.scroll;
        assert!(first > 0, "it went to the first match");
        let rows = screen(&v, NARROW, false);
        row_of(&rows, "12 coincidencias · 1/12");
        // n and N go round, whatever the case
        key(&mut v, NARROW, KeyCode::Char('n'));
        assert_eq!(v.search.at, 1);
        key(&mut v, NARROW, KeyCode::Char('N'));
        key(&mut v, NARROW, KeyCode::Char('N'));
        assert_eq!(v.search.at, 11, "back past the first is the last");
        // the matches are lit in the grid
        let mut g = Grid::new(80, 20);
        v.draw(&mut g, NARROW, false);
        let lit = g
            .cells
            .iter()
            .filter(|(_, s)| s.bg == Some(look::yellow()) || s.bg == Some(look::magenta()))
            .count();
        assert!(lit >= 8, "the matched words are lit: {lit}");
        // esc while there is a search lets it go, and the next one closes
        assert_eq!(key(&mut v, NARROW, KeyCode::Esc), Act::Nothing);
        assert!(v.search.query.is_empty());
        assert_eq!(key(&mut v, NARROW, KeyCode::Esc), Act::Close);
        // typing can be cancelled, and a word that is not there is said
        key(&mut v, NARROW, KeyCode::Char('/'));
        typed(&mut v, NARROW, "zzz");
        key(&mut v, NARROW, KeyCode::Esc);
        assert!(!v.search.typing && v.search.query.is_empty());
        key(&mut v, NARROW, KeyCode::Char('/'));
        typed(&mut v, NARROW, "zzz");
        key(&mut v, NARROW, KeyCode::Enter);
        assert_eq!(
            v.take_says().as_deref(),
            Some("no hay «zzz» en el documento")
        );
    }

    #[test]
    fn a_plan_that_waits_is_answered_by_1_2_3_and_by_a_click_and_a_file_has_no_answers() {
        let plan = Source::Plan {
            node: "n".into(),
            ask: "a".into(),
        };
        let mut v = viewer(plan);
        let rows = screen(&v, NARROW, true);
        assert!(
            rows[0].contains("▤ plan") && rows[0].contains("plan listo"),
            "{}",
            rows[0]
        );
        row_of(&rows, "1  aprobar y editar sin preguntar");
        assert_eq!(
            key(&mut v, NARROW, KeyCode::Char('1')),
            Act::Answer("accept".into())
        );
        assert_eq!(
            key(&mut v, NARROW, KeyCode::Char('2')),
            Act::Answer("manual".into())
        );
        assert_eq!(
            key(&mut v, NARROW, KeyCode::Char('3')),
            Act::Answer("no".into())
        );
        assert_eq!(
            key(&mut v, NARROW, KeyCode::Char('4')),
            Act::Nothing,
            "only the answers the question has"
        );
        let y = NARROW.3 - 1;
        let rows = screen(&v, NARROW, true);
        let at = rows[y].chars().position(|c| c == '2').unwrap();
        assert_eq!(v.click(NARROW, at + 3, y), Act::Answer("manual".into()));
        assert_eq!(v.click(NARROW, 70, y - 3), Act::Nothing);
        // the text is one row shorter for the answers
        assert_eq!(v.geo(NARROW).rows, 18);
        // answered elsewhere: the bar no longer says it waits
        assert!(!screen(&v, NARROW, false)[0].contains("plan listo"));
        // a file has no answers
        let mut f = viewer(Source::File);
        assert_eq!(key(&mut f, NARROW, KeyCode::Char('1')), Act::Nothing);
        assert_eq!(f.geo(NARROW).rows, 19);
    }

    #[test]
    fn y_copies_all_of_it_e_opens_its_file_and_esc_or_q_close_it() {
        let mut v = viewer(Source::File);
        let Act::Copy(text) = key(&mut v, NARROW, KeyCode::Char('y')) else {
            panic!("a copy");
        };
        assert!(text.starts_with("## Contexto\n\nlínea 0-0\n\n"), "{text}");
        assert!(text.ends_with("línea 2-11"), "{text}");
        assert_eq!(
            text.lines().count(),
            3 * 25 + 2,
            "every block and the blank rows between"
        );
        assert!(v.take_says().unwrap().starts_with("copiado · "));
        // no file: it is said; with one it is opened
        assert_eq!(key(&mut v, NARROW, KeyCode::Char('e')), Act::Nothing);
        assert!(v.take_says().unwrap().contains("no tiene un archivo"));
        let mut d = long();
        d.path = Some("/w/plan.md".into());
        let mut v = Viewer::new(d, Source::File, Vec::new(), String::new());
        assert_eq!(
            key(&mut v, NARROW, KeyCode::Char('e')),
            Act::Edit("/w/plan.md".into())
        );
        assert_eq!(key(&mut v, NARROW, KeyCode::Esc), Act::Close);
        assert_eq!(key(&mut v, NARROW, KeyCode::Char('q')), Act::Close);
        // a control key is not a command of the viewer
        assert_eq!(
            v.key(
                &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
                NARROW
            ),
            Act::Nothing
        );
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn a_drag_covers_rows_and_columns_scrolls_at_the_edge_and_copies_what_it_reads_like() {
        let mut v = Viewer::new(
            doc(vec![
                head(2, "Plan"),
                para("una frase larga que se parte en varias filas del panel"),
                MdBlock::Rule,
            ]),
            Source::File,
            Vec::new(),
            String::new(),
        );
        let rect = (0, 0, 30, 12);
        let rows = screen(&v, rect, false);
        let y = row_of(&rows, "una frase");
        let x = rows[y].chars().position(|c| c == 'f').unwrap();
        assert!(v.press(rect, x, y));
        v.drag_to(rect, (x + 4, y + 1), 0);
        let mut g = Grid::new(30, 12);
        v.draw(&mut g, rect, false);
        assert!(
            g.cells[y * 30 + x].1.bg == Some(look::selection().0),
            "what it covers is lit"
        );
        let Released::Copy(text) = v.release(rect) else {
            panic!("a copy");
        };
        assert!(text.starts_with("frase larga"), "{text}");
        assert!(
            !text.contains('\n'),
            "rows the width broke are one line: {text:?}"
        );
        assert!(v.take_says().unwrap().starts_with("copiado · 1 línea"));
        // a press that never moves is a click
        assert!(v.press(rect, x, y));
        assert_eq!(v.release(rect), Released::Click(x, y));
        // a press outside the text is no drag
        assert!(!v.press(rect, 0, y));
        // held past the bottom it scrolls, a line each EDGE_MS
        let mut long_v = viewer(Source::File);
        screen(&long_v, NARROW, false);
        assert!(long_v.press(NARROW, 10, 5));
        long_v.drag_to(NARROW, (10, NARROW.3), 0);
        long_v.drag_tick(NARROW, 10);
        assert_eq!(long_v.scroll, 0, "nothing moves before its time");
        long_v.drag_tick(NARROW, select::EDGE_MS);
        assert_eq!(long_v.scroll, 1);
        long_v.drag_tick(NARROW, select::EDGE_MS + 1);
        assert_eq!(long_v.scroll, 1);
        long_v.drag_tick(NARROW, 2 * select::EDGE_MS);
        assert_eq!(long_v.scroll, 2);
    }

    #[test]
    fn a_panel_with_no_room_does_not_panic_on_a_key_a_press_or_a_drag() {
        let mut v = viewer(Source::Plan {
            node: "n".into(),
            ask: "a".into(),
        });
        for rect in [(0, 0, 3, 0), (0, 0, 5, 1), (4, 2, 120, 2)] {
            key(&mut v, rect, KeyCode::Char('/'));
            key(&mut v, rect, KeyCode::Esc);
            key(&mut v, rect, KeyCode::Char('G'));
            v.press(rect, 1, 1);
            v.drag_to(rect, (9, 9), 0);
            v.drag_tick(rect, 999);
            v.release(rect);
            v.click(rect, 0, 0);
            screen(&v, rect, true);
        }
    }
}
