//! The Markdown reader (REQ-22): a document rendered for reading, not its
//! source. Headings, wrapped paragraphs with inline styles, lists (nested,
//! numbered, tasks), quotes, highlighted code blocks, tables, rules.
//! Pure: Markdown in, ratatui lines out, laid out for one width.

use crate::theme::p;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::highlight::Highlighter;

/// Narrower than this, nothing reads well anyway.
const MIN_WIDTH: usize = 12;

pub struct Doc {
    pub lines: Vec<Line<'static>>,
    pub headings: Vec<Heading>,
}

/// Where a heading landed, for a table of contents and n/N jumps.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub line: usize,
    pub level: u8,
    pub text: String,
}

/// A piece of inline text with its style; `\n` alone is a hard break.
type Run = (String, Style);

pub fn render(src: &str, width: u16) -> Doc {
    let mut r = Renderer {
        width: usize::from(width).max(MIN_WIDTH),
        ..Renderer::default()
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    for ev in Parser::new_ext(src, opts) {
        r.event(ev);
    }
    r.flush();
    while r.out.last().is_some_and(|l| l.width() == 0) {
        r.out.pop();
    }
    Doc {
        lines: r.out,
        headings: r.headings,
    }
}

#[derive(Default)]
struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    headings: Vec<Heading>,
    /// Inline text of the block being read.
    runs: Vec<Run>,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Option<String>,
    heading: Option<u8>,
    quote: usize,
    /// One per open list: the next number, or None for bullets.
    lists: Vec<Option<u64>>,
    /// One per open item: how wide its marker is (the hanging indent).
    items: Vec<usize>,
    /// The marker of an item whose first line hasn't been written yet.
    marker: Option<Span<'static>>,
    /// Inside an item, blocks after the first don't get a blank line when
    /// the list is tight.
    tight: bool,
    code: Option<(String, Vec<String>)>,
    table: Option<Table>,
}

#[derive(Default)]
struct Table {
    align: Vec<Alignment>,
    head: Vec<Vec<Run>>,
    rows: Vec<Vec<Vec<Run>>>,
    in_head: bool,
    cell: Vec<Run>,
    row: Vec<Vec<Run>>,
}

impl Renderer {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((_, lines)) = &mut self.code {
                    lines.extend(t.lines().map(str::to_string));
                } else {
                    self.text(&t, self.style());
                }
            }
            Event::Code(t) => {
                let style = Style::default().fg(p().blue).bg(p().panel);
                self.text(&t, style);
            }
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => self.push_run("\n".into(), Style::default()),
            Event::Rule => {
                self.block_gap();
                let w = self.width - self.prefix_width();
                let rule = Span::styled("─".repeat(w), Style::default().fg(p().line));
                self.out.push(Line::from(vec![self.quote_prefix(), rule]));
            }
            Event::TaskListMarker(done) => {
                let glyph = if done { "☑ " } else { "☐ " };
                let color = if done { p().dim } else { p().blue };
                self.marker = Some(Span::styled(glyph, Style::default().fg(color)));
                if let Some(w) = self.items.last_mut() {
                    *w = 2;
                }
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                self.text(&t, Style::default().fg(p().dim));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if !(self.tight && self.marker.is_some()) {
                    self.block_gap();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.block_gap();
                self.heading = Some(level_num(level));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.block_gap();
                self.quote += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.block_gap();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, Vec::new()));
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.block_gap();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        Span::styled(m, Style::default().fg(p().cyan))
                    }
                    _ => {
                        let bullet = ["• ", "◦ ", "▪ "][(self.lists.len() + 2) % 3];
                        Span::styled(bullet, Style::default().fg(p().cyan))
                    }
                };
                self.items.push(marker.width());
                self.marker = Some(marker);
                self.tight = true;
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { .. } => {
                self.text("▣ ", Style::default().fg(p().dim));
                self.link = Some(String::new());
            }
            Tag::Table(align) => {
                self.flush();
                self.block_gap();
                self.table = Some(Table {
                    align,
                    ..Table::default()
                });
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let text: String = self.runs.iter().map(|(t, _)| t.as_str()).collect();
                let line = self.out.len();
                let style = match level {
                    1 => Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
                    2 => Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
                    3 => Style::default().fg(p().cyan).add_modifier(Modifier::BOLD),
                    _ => Style::default().fg(p().dim).add_modifier(Modifier::BOLD),
                };
                for run in &mut self.runs {
                    run.1 = run.1.patch(style);
                }
                self.flush();
                self.headings.push(Heading {
                    line,
                    level,
                    text: text.trim().to_string(),
                });
                if level <= 2 {
                    let w = self.width - self.prefix_width();
                    let (ch, color) = if level == 1 {
                        ("━", p().blue)
                    } else {
                        ("─", p().line)
                    };
                    let rule = Span::styled(ch.repeat(w), Style::default().fg(color));
                    self.out.push(Line::from(vec![self.quote_prefix(), rule]));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                if let Some((lang, lines)) = self.code.take() {
                    self.code_block(&lang, &lines);
                }
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                self.items.pop();
                self.marker = None;
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    let text: String = self.runs.iter().map(|(t, _)| t.as_str()).collect();
                    if !url.is_empty() && !text.ends_with(&url) && !url.starts_with('#') {
                        self.push_run(format!(" ({url})"), Style::default().fg(p().dim));
                    }
                }
            }
            TagEnd::Image => self.link = None,
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell = std::mem::take(&mut self.runs);
                    t.cell = cell;
                    let cell = std::mem::take(&mut t.cell);
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.head = std::mem::take(&mut t.row);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.table_block(&t);
                }
            }
            _ => {}
        }
    }

    fn style(&self) -> Style {
        let mut s = Style::default().fg(p().fg);
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link.is_some() {
            s = s.fg(p().cyan).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn text(&mut self, t: &str, style: Style) {
        self.push_run(t.to_string(), style);
    }

    fn push_run(&mut self, t: String, style: Style) {
        self.runs.push((t, style));
    }

    /// One blank line between blocks, never two, never at the top.
    fn block_gap(&mut self) {
        if self
            .out
            .last()
            .is_some_and(|l| l.width() > self.prefix_width())
        {
            self.out.push(Line::from(self.quote_prefix()));
        }
    }

    fn quote_prefix(&self) -> Span<'static> {
        Span::styled("│ ".repeat(self.quote), Style::default().fg(p().line))
    }

    fn prefix_width(&self) -> usize {
        self.quote * 2 + self.items.iter().sum::<usize>()
    }

    /// Writes the pending inline text as wrapped lines under the current
    /// quote and list prefixes.
    fn flush(&mut self) {
        if self.table.is_some() || self.runs.is_empty() && self.marker.is_none() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let outer: usize = self.items.iter().sum::<usize>()
            - if self.marker.is_some() {
                self.items.last().copied().unwrap_or(0)
            } else {
                0
            };
        let mut first = vec![self.quote_prefix(), Span::raw(" ".repeat(outer))];
        if let Some(m) = self.marker.take() {
            first.push(m);
        }
        let rest = vec![
            self.quote_prefix(),
            Span::raw(" ".repeat(self.items.iter().sum::<usize>())),
        ];
        let lines = wrap(&runs, self.width, first, rest);
        self.out.extend(lines);
    }

    fn code_block(&mut self, lang: &str, lines: &[String]) {
        let inner = self.width - self.prefix_width();
        let indent = Span::raw(" ".repeat(self.items.iter().sum::<usize>()));
        let bg = Style::default().bg(p().panel);
        let label = if lang.is_empty() { "code" } else { lang };
        let head = fit(
            vec![Span::styled(
                format!(" {label} "),
                Style::default().fg(p().dim).bg(p().panel),
            )],
            inner,
            bg,
        );
        self.out
            .push(prefixed(self.quote_prefix(), indent.clone(), head));
        let mut hl = Highlighter::new(lang);
        for l in lines {
            let mut spans = vec![Span::styled(" ", bg)];
            spans.extend(hl.line(&l.replace('\t', "    ")).into_iter().map(|s| {
                let style = s.style.bg(p().panel);
                Span::styled(s.content, style)
            }));
            self.out.push(prefixed(
                self.quote_prefix(),
                indent.clone(),
                fit(spans, inner, bg),
            ));
        }
    }

    fn table_block(&mut self, t: &Table) {
        let cols = t
            .head
            .len()
            .max(t.rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return;
        }
        let text = |c: &Vec<Run>| -> String { c.iter().map(|(s, _)| s.as_str()).collect() };
        let mut widths = vec![1usize; cols];
        for row in std::iter::once(&t.head).chain(&t.rows) {
            for (i, c) in row.iter().enumerate() {
                widths[i] = widths[i].max(text(c).width());
            }
        }
        // Borders take cols+1 cells and each cell has a space either side.
        let avail = self.width - self.prefix_width();
        let frame = cols + 1 + 2 * cols;
        while widths.iter().sum::<usize>() + frame > avail {
            let (i, w) = widths
                .iter()
                .copied()
                .enumerate()
                .max_by_key(|(_, w)| *w)
                .unwrap_or((0, 0));
            if w <= 3 {
                break;
            }
            widths[i] -= 1;
        }
        let border = Style::default().fg(p().line);
        let rule = |l: &str, m: &str, r: &str| -> Line<'static> {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            Line::from(Span::styled(format!("{l}{}{r}", parts.join(m)), border))
        };
        let row_line = |row: &Vec<Vec<Run>>, head: bool| -> Line<'static> {
            let mut spans = vec![Span::styled("│", border)];
            for (i, w) in widths.iter().enumerate() {
                let raw = row.get(i).map(text).unwrap_or_default();
                let cut = truncate(&raw, *w);
                let pad = w - cut.width();
                let (left, right) = match t.align.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                let style = row
                    .get(i)
                    .and_then(|c| c.first())
                    .map_or(Style::default().fg(p().fg), |(_, s)| *s);
                let style = if head {
                    Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
                } else {
                    style
                };
                spans.push(Span::raw(" ".repeat(left + 1)));
                spans.push(Span::styled(cut, style));
                spans.push(Span::raw(" ".repeat(right + 1)));
                spans.push(Span::styled("│", border));
            }
            Line::from(spans)
        };
        let q = self.quote_prefix();
        let mut push = |l: Line<'static>| {
            let mut spans = vec![q.clone()];
            spans.extend(l.spans);
            self.out.push(Line::from(spans));
        };
        push(rule("┌", "┬", "┐"));
        if !t.head.is_empty() {
            push(row_line(&t.head, true));
            push(rule("├", "┼", "┤"));
        }
        for row in &t.rows {
            push(row_line(row, false));
        }
        push(rule("└", "┴", "┘"));
    }
}

fn level_num(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn prefixed(
    quote: Span<'static>,
    indent: Span<'static>,
    body: Vec<Span<'static>>,
) -> Line<'static> {
    let mut spans = vec![quote, indent];
    spans.extend(body);
    Line::from(spans)
}

/// Cuts spans to `width` columns (with `…` when cut) and pads them with
/// `fill` to exactly that width.
fn fit(spans: Vec<Span<'static>>, width: usize, fill: Style) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    let mut out = Vec::new();
    if total <= width {
        out.extend(spans);
        if total < width {
            out.push(Span::styled(" ".repeat(width - total), fill));
        }
        return out;
    }
    let mut used = 0;
    for s in spans {
        let w = s.width();
        if used + w < width {
            used += w;
            out.push(s);
            continue;
        }
        let cut = truncate(&s.content, width - used);
        used += cut.width();
        out.push(Span::styled(cut, s.style));
        break;
    }
    if used < width {
        out.push(Span::styled(" ".repeat(width - used), fill));
    }
    out
}

/// `s` in at most `width` columns, ending in `…` when cut.
fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Word-wraps styled runs to `width`, starting lines with `first` then
/// `rest`. Words longer than a line are broken mid-word.
fn wrap(
    runs: &[Run],
    width: usize,
    first: Vec<Span<'static>>,
    rest: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    // Split into words (keeping style) and the gaps between them.
    enum Tok {
        Word(String, Style),
        Space(Style),
        Break,
    }
    let mut toks = Vec::new();
    for (text, style) in runs {
        if text == "\n" {
            toks.push(Tok::Break);
            continue;
        }
        let mut word = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                if !word.is_empty() {
                    toks.push(Tok::Word(std::mem::take(&mut word), *style));
                }
                toks.push(Tok::Space(*style));
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            toks.push(Tok::Word(word, *style));
        }
    }

    let prefix_w = |p: &Vec<Span<'static>>| p.iter().map(Span::width).sum::<usize>();
    let mut lines = Vec::new();
    let mut cur: Vec<Span<'static>> = first.clone();
    let mut used = prefix_w(&first);
    let mut fresh = true; // nothing but the prefix yet
    let mut pending_space: Option<Style> = None;
    let new_line = |lines: &mut Vec<Line<'static>>, cur: &mut Vec<Span<'static>>| {
        lines.push(Line::from(std::mem::replace(cur, rest.clone())));
    };
    for tok in toks {
        match tok {
            Tok::Break => {
                new_line(&mut lines, &mut cur);
                used = prefix_w(&rest);
                fresh = true;
                pending_space = None;
            }
            Tok::Space(s) => {
                if !fresh {
                    pending_space = Some(s);
                }
            }
            Tok::Word(mut w, style) => {
                let ww = w.width();
                let space = usize::from(pending_space.is_some());
                if !fresh && used + space + ww > width {
                    new_line(&mut lines, &mut cur);
                    used = prefix_w(&rest);
                    pending_space = None;
                }
                if let Some(s) = pending_space.take() {
                    cur.push(Span::styled(" ", s));
                    used += 1;
                }
                // A word wider than a whole line: break it.
                while used + w.width() > width {
                    let room = width.saturating_sub(used).max(1);
                    let mut head = String::new();
                    let mut hw = 0;
                    let mut chars = w.chars();
                    for c in chars.by_ref() {
                        let cw = c.width().unwrap_or(0);
                        if hw + cw > room {
                            head.push(c);
                            break;
                        }
                        head.push(c);
                        hw += cw;
                    }
                    // `head` took one char too many when it overflowed.
                    let overflow = head.width() > room;
                    let tail: String = if overflow {
                        let last = head.pop().map(String::from).unwrap_or_default();
                        last + chars.as_str()
                    } else {
                        chars.as_str().to_string()
                    };
                    cur.push(Span::styled(head, style));
                    new_line(&mut lines, &mut cur);
                    used = prefix_w(&rest);
                    w = tail;
                    if w.is_empty() {
                        break;
                    }
                }
                if !w.is_empty() {
                    used += w.width();
                    cur.push(Span::styled(w, style));
                }
                fresh = false;
            }
        }
    }
    if !fresh || lines.is_empty() {
        lines.push(Line::from(cur));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Console errors

Some **bold**, some *italic* and `inline code`, then a [link](https://example.com/a/very/long/url/that/wraps) and a very long paragraph that has to wrap across several lines because the width is narrow on purpose.

## Lists

- one
- two
  - nested item that is long enough to wrap onto a second line here
- [ ] todo
- [x] done

1. first
2. second

> a quote
> that continues

### Code

```rust
/* a block
   comment */
fn main() { let x = 1; }
```

| Name | Value | Notes |
|------|------:|:-----:|
| port | 20100 | the web server listens here, a long note |
| slot | 1 | |

---

Last paragraph.
"#;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn every_line_fits_the_width() {
        for width in [20u16, 40, 72, 120] {
            let doc = render(SAMPLE, width);
            for l in &doc.lines {
                assert!(
                    l.width() <= usize::from(width),
                    "width {width}: {:?} is {}",
                    text(l),
                    l.width()
                );
            }
        }
    }

    #[test]
    fn headings_are_indexed_where_they_land() {
        let doc = render(SAMPLE, 60);
        let got: Vec<(u8, &str)> = doc
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str()))
            .collect();
        assert_eq!(got, [(1, "Console errors"), (2, "Lists"), (3, "Code")]);
        for h in &doc.headings {
            assert_eq!(text(&doc.lines[h.line]).trim(), h.text);
        }
        // H1 and H2 get a rule under them.
        assert!(text(&doc.lines[doc.headings[0].line + 1]).starts_with('━'));
        assert!(text(&doc.lines[doc.headings[1].line + 1]).starts_with('─'));
        assert_eq!(
            doc.lines[0].width(),
            "Console errors".len(),
            "no blank line on top"
        );
    }

    #[test]
    fn inline_styles_lists_and_tasks() {
        let doc = render(SAMPLE, 60);
        let all: Vec<String> = doc.lines.iter().map(text).collect();
        let joined = all.join("\n");
        assert!(joined.contains("link"));
        assert!(
            joined.contains("(https://example.com"),
            "the url shows, dim"
        );
        assert!(joined.contains("• one"));
        assert!(joined.contains("◦ nested"));
        assert!(joined.contains("☐ todo") && joined.contains("☑ done"));
        assert!(joined.contains("1. first") && joined.contains("2. second"));
        assert!(joined.contains("│ a quote that continues"));
        let bold = doc
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "bold");
        assert!(bold.is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)));
        let code = doc
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "inline");
        assert_eq!(code.and_then(|s| s.style.bg), Some(p().panel));
        // The nested item's second line hangs under its text, not its bullet.
        let i = all.iter().position(|l| l.contains("◦ nested")).unwrap();
        let col = all[i].find("nested").unwrap();
        let next = &all[i + 1];
        assert_eq!(
            next.len() - next.trim_start().len(),
            all[i][..col].chars().count()
        );
    }

    #[test]
    fn code_blocks_are_highlighted_on_a_panel() {
        let doc = render(SAMPLE, 60);
        let label = doc
            .lines
            .iter()
            .position(|l| text(l).trim() == "rust")
            .unwrap();
        let body = &doc.lines[label + 3];
        assert!(text(body).contains("fn main"));
        let mut fgs: Vec<_> = body.spans.iter().filter_map(|s| s.style.fg).collect();
        fgs.dedup();
        assert!(fgs.len() > 1, "{body:?}");
        assert!(
            body.spans
                .iter()
                .skip(2)
                .all(|s| s.style.bg == Some(p().panel))
        );
        assert_eq!(body.width(), 60, "the panel spans the width");
    }

    #[test]
    fn tables_have_borders_and_fit() {
        let doc = render(SAMPLE, 40);
        let all: Vec<String> = doc.lines.iter().map(text).collect();
        let top = all.iter().position(|l| l.starts_with('┌')).unwrap();
        assert!(all[top + 1].contains("Name") && all[top + 1].contains("Value"));
        assert!(all[top + 2].starts_with('├'));
        assert!(all.iter().any(|l| l.starts_with('└')));
        assert!(all.iter().any(|l| l.contains('…')), "the long note is cut");
        // Right-aligned column: the number touches the border.
        let row = all.iter().find(|l| l.contains("20100")).unwrap();
        assert!(row.contains("20100 │"), "{row}");
    }

    #[test]
    fn narrow_and_empty() {
        assert!(render("", 40).lines.is_empty());
        let doc = render("supercalifragilisticexpialidocious-and-more", 12);
        assert!(doc.lines.len() > 1);
        assert!(doc.lines.iter().all(|l| l.width() <= 12));
    }
}
