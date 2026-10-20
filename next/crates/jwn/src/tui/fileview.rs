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
use super::{Grid, Style, clip, is_markdown, look};

/// What the person asked of the client by a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    Close,
    Copy(String),
    /// Open the file in the editor.
    Edit(String),
    /// Read the file again.
    Reread,
    /// Show the `.md` rendered, as the Markdown viewer does.
    Rendered,
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
/// How far `h` and `l` move a line that is not wrapped.
const SIDE: usize = 8;

pub struct FileView {
    /// The workspace the file was asked of, for the next `r` or `s`.
    pub workspace: String,
    doc: TextDoc,
    /// What the daemon refused (a binary file), instead of lines.
    refusal: Option<String>,
    /// Whether the file is a `.md`, which can be read rendered.
    markdown: bool,
    /// The text of each line as it is in the file, for `y`.
    plain: Vec<String>,
    /// The characters of each line once its tabs are spaces.
    len: Vec<usize>,
    scroll: usize,
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
            plain,
            len,
            scroll: 0,
            wrap: false,
            side: 0,
            search: Search::default(),
            found: Vec::new(),
            says: String::new(),
            most: Cell::new(0),
            starts: RefCell::new(None),
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
                KeyCode::Esc | KeyCode::Char('q') => Act::Close,
                KeyCode::Char('e') => Act::Edit(self.doc.path.clone()),
                _ => Act::Nothing,
            };
        }
        self.clamp(rect);
        let geo = self.geo(rect);
        let page = geo.rows.saturating_sub(1).max(1);
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.search.query.is_empty() {
                    return Act::Close;
                }
                self.search = Search::default();
                self.found.clear();
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(false, 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(true, 1),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll_by(false, page),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_by(true, page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = self.most.get(),
            KeyCode::Char('w') => self.toggle_wrap(geo.cw),
            KeyCode::Char('l') if !self.wrap => {
                let widest = self.len.iter().copied().max().unwrap_or(0);
                self.side = (self.side + SIDE).min(widest.saturating_sub(geo.cw));
            }
            KeyCode::Char('h') if !self.wrap => self.side = self.side.saturating_sub(SIDE),
            KeyCode::Char('/') => {
                self.search.typing = true;
                self.search.buf.clear();
            }
            KeyCode::Char('n') => self.next_match(true, geo.cw),
            KeyCode::Char('N') => self.next_match(false, geo.cw),
            KeyCode::Char('y') => {
                let text = self.plain.join("\n");
                self.says = super::conversation::copied(&text);
                return Act::Copy(text);
            }
            KeyCode::Char('e') => return Act::Edit(self.doc.path.clone()),
            KeyCode::Char('r') => return Act::Reread,
            KeyCode::Char('s') if self.markdown => return Act::Rendered,
            _ => {}
        }
        self.clamp(rect);
        Act::Nothing
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
        let right = format!("{pct} % ");
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
            return "esc cerrar · e editar".into();
        }
        let md = if self.markdown { " · s vista" } else { "" };
        let side = if self.wrap { "" } else { " · h l lado" };
        format!(
            "esc cerrar · j k ␣ b g G mover · / n N buscar · w ajustar{side} · r releer · y copiar · e editar{md}"
        )
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

    #[test]
    fn the_keys_scroll_by_a_line_a_page_and_to_the_ends_and_not_past_them() {
        let mut v = view(numbered(120));
        screen(&v);
        key(&mut v, KeyCode::Char('j'));
        assert_eq!(v.scroll, 1);
        key(&mut v, KeyCode::Up);
        key(&mut v, KeyCode::Char('k'));
        assert_eq!(v.scroll, 0, "not above the start");
        key(&mut v, KeyCode::Char(' '));
        assert_eq!(v.scroll, 10, "a page is the rows of the text less one");
        key(&mut v, KeyCode::Char('b'));
        assert_eq!(v.scroll, 0);
        key(&mut v, KeyCode::Char('G'));
        let rows = screen(&v);
        assert!(rows[11].contains("línea 120"), "{}", rows[11]);
        assert!(rows[0].contains("100 %"));
        key(&mut v, KeyCode::Char('j'));
        assert_eq!(v.scroll, v.most.get(), "not past the end");
        key(&mut v, KeyCode::Char('g'));
        assert_eq!(v.scroll, 0);
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

    #[test]
    fn a_long_line_is_cut_with_a_mark_and_h_l_move_it_and_w_wraps_it() {
        let long = "abcdefghij".repeat(10);
        let mut v = view(vec![vec![run(&long, None)], vec![run("fin", None)]]);
        let rows = screen(&v);
        assert!(
            rows[1].ends_with('›') || rows[1].contains('›'),
            "{}",
            rows[1]
        );
        assert!(rows[1].contains("abcdefghij"));
        key(&mut v, KeyCode::Char('l'));
        assert_eq!(v.side, SIDE);
        for _ in 0..30 {
            key(&mut v, KeyCode::Char('l'));
        }
        assert_eq!(v.side, 100 - 54, "not past the end of the widest line");
        key(&mut v, KeyCode::Char('g'));
        v.side = SIDE;
        let rows = screen(&v);
        assert!(rows[1].contains("│ ijabcdefghij"), "{}", rows[1]);
        key(&mut v, KeyCode::Char('h'));
        key(&mut v, KeyCode::Char('h'));
        assert_eq!(v.side, 0, "not left of the start");
        // wrapped: the whole line is there, without the mark, and h l do nothing
        key(&mut v, KeyCode::Char('w'));
        let rows = screen(&v);
        assert!(!rows.iter().any(|r| r.contains('›')), "{rows:?}");
        let text: String = rows[1..3]
            .iter()
            .map(|r| r.split("│ ").nth(1).unwrap_or("").trim_end().to_owned())
            .collect();
        assert_eq!(text, long);
        key(&mut v, KeyCode::Char('l'));
        assert_eq!(v.side, 0);
        assert!(rows.iter().any(|r| r.contains("fin")));
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
        assert_eq!(key(&mut v, KeyCode::Esc), Act::Close, "the next closes");
        // nothing found says so once
        key(&mut v, KeyCode::Char('/'));
        typed(&mut v, "zzz");
        key(&mut v, KeyCode::Enter);
        assert_eq!(v.take_says().as_deref(), Some("no hay «zzz» en el archivo"));
        assert_eq!(v.take_says(), None);
    }

    #[test]
    fn a_class_has_the_accent_of_the_theme_and_an_unknown_one_is_plain() {
        let v = view(vec![vec![
            run("fn", Some("kw")),
            run(" ", None),
            run("\"a\"", Some("str")),
            run(" // c", Some("cm")),
            run("x", Some("from-the-future")),
        ]]);
        let mut g = Grid::new(R.2, R.3);
        v.draw(&mut g, R);
        let at = |col: usize| g.cells[R.2 + 4 + col].1;
        assert_eq!(at(0).fg, Some(look::magenta()), "kw");
        assert_eq!(at(3).fg, Some(look::green()), "str");
        assert!(at(8).italic && at(8).fg == Some(look::dim()), "cm");
        assert_eq!(at(11).fg, Some(look::fg()), "unknown is plain");
    }

    #[test]
    fn a_file_that_was_cut_says_so_under_the_text_and_a_refusal_has_only_e_and_esc() {
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
            Act::Edit("/work/logo.png".into())
        );
        assert_eq!(key(&mut v, KeyCode::Esc), Act::Close);
        assert_eq!(v.hint(), "esc cerrar · e editar");
    }

    #[test]
    fn e_r_and_s_ask_the_client_and_s_is_only_for_a_markdown() {
        let mut v = view(numbered(3));
        assert_eq!(
            key(&mut v, KeyCode::Char('e')),
            Act::Edit("/work/main.rs".into())
        );
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
}
