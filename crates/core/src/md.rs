//! Markdown as blocks for a phone (mobile REQ-81): the document as a tree of headings, paragraphs, lists, quotes,
//! code and tables, with its inline styles as spans, so that the app draws it without a parser of its own. The
//! parser is the one the TUI's reader uses. Pure: Markdown in, blocks out; serde makes the wire form.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;

/// The most blocks one document yields, and the deepest nesting of lists and quotes: a hostile file must not make
/// an answer or a stack without bound.
const MAX_BLOCKS: usize = 20_000;
const MAX_DEPTH: usize = 16;

/// A piece of inline text with its style.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Span {
    pub t: String,
    #[serde(skip_serializing_if = "is_false")]
    pub b: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub i: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub s: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub code: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "k")]
pub enum Block {
    #[serde(rename = "h")]
    Heading { level: u8, spans: Vec<Span> },
    #[serde(rename = "p")]
    Paragraph { spans: Vec<Span> },
    #[serde(rename = "code")]
    Code { lang: String, text: String },
    #[serde(rename = "ul")]
    Ul { items: Vec<Vec<Block>> },
    #[serde(rename = "ol")]
    Ol { start: u64, items: Vec<Vec<Block>> },
    #[serde(rename = "quote")]
    Quote { blocks: Vec<Block> },
    #[serde(rename = "hr")]
    Rule,
    #[serde(rename = "table")]
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
}

/// What is being built: the blocks of the document, a quote or an item, or a list's items, or a table.
enum Ctx {
    Blocks(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
        row: Vec<Vec<Span>>,
    },
}

#[derive(Default)]
struct Style {
    b: u32,
    i: u32,
    s: u32,
    href: Vec<String>,
}

struct Builder {
    stack: Vec<Ctx>,
    spans: Vec<Span>,
    style: Style,
    /// Text of a code block being read, and its language.
    code: Option<(String, String)>,
    blocks: usize,
    /// Containers that were too deep to open: their end closes nothing.
    skipped: usize,
}

/// The blocks of `src`. At most [`MAX_BLOCKS`] of them.
pub fn blocks(src: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let mut b = Builder {
        stack: vec![Ctx::Blocks(Vec::new())],
        spans: Vec::new(),
        style: Style::default(),
        code: None,
        blocks: 0,
        skipped: 0,
    };
    for ev in Parser::new_ext(src, opts) {
        if b.blocks >= MAX_BLOCKS {
            break;
        }
        b.event(ev);
    }
    b.finish()
}

impl Builder {
    fn finish(mut self) -> Vec<Block> {
        self.flush_text();
        self.skipped = 0;
        // Whatever is still open (a cut document) closes into its parent.
        while self.stack.len() > 1 {
            self.close_container();
        }
        match self.stack.pop() {
            Some(Ctx::Blocks(b)) => b,
            _ => Vec::new(),
        }
    }

    fn push_block(&mut self, block: Block) {
        self.blocks += 1;
        match self.stack.last_mut() {
            Some(Ctx::Blocks(v)) => v.push(block),
            // A block directly in a list (never from the parser) goes into a new item.
            Some(Ctx::List { items, .. }) => items.push(vec![block]),
            _ => {}
        }
    }

    /// Text that was read with no paragraph around it (a tight list's item) becomes a paragraph.
    fn flush_text(&mut self) {
        if !self.spans.is_empty() {
            let spans = std::mem::take(&mut self.spans);
            self.push_block(Block::Paragraph { spans });
        }
    }

    fn text(&mut self, t: &str, code: bool) {
        if t.is_empty() {
            return;
        }
        let span = Span {
            t: t.to_string(),
            b: self.style.b > 0,
            i: self.style.i > 0,
            s: self.style.s > 0,
            code,
            href: self.style.href.last().cloned(),
        };
        if let Some(last) = self.spans.last_mut()
            && (last.b, last.i, last.s, last.code, &last.href)
                == (span.b, span.i, span.s, span.code, &span.href)
        {
            last.t.push_str(t);
            return;
        }
        self.spans.push(span);
    }

    fn open_container(&mut self, c: Ctx) {
        if self.stack.len() >= MAX_DEPTH {
            // Too deep: what is inside lands in the parent, flat.
            self.skipped += 1;
            return;
        }
        self.flush_text();
        self.stack.push(c);
    }

    /// Closes the container on top and hands it to its parent.
    fn close_container(&mut self) {
        self.flush_text();
        if self.skipped > 0 {
            self.skipped -= 1;
            return;
        }
        if self.stack.len() < 2 {
            return;
        }
        let Some(top) = self.stack.pop() else { return };
        match top {
            Ctx::Blocks(blocks) => match self.stack.last_mut() {
                // An item's blocks are one more item of its list.
                Some(Ctx::List { items, .. }) => items.push(blocks),
                // A quote's blocks are a quote in whatever holds it.
                _ => self.push_block(Block::Quote { blocks }),
            },
            Ctx::List { start, items } => self.push_block(match start {
                Some(start) => Block::Ol { start, items },
                None => Block::Ul { items },
            }),
            Ctx::Table { head, rows, .. } => self.push_block(Block::Table { head, rows }),
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => match &mut self.code {
                Some((_, text)) => text.push_str(&t),
                None => self.text(&t, false),
            },
            Event::Code(t) => self.text(&t, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => {
                self.flush_text();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "☑ " } else { "☐ " }, false),
            // Raw HTML and the rest are not drawn.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::Heading { .. } => self.flush_text(),
            Tag::BlockQuote(_) => self.open_container(Ctx::Blocks(Vec::new())),
            Tag::List(start) => self.open_container(Ctx::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => self.open_container(Ctx::Blocks(Vec::new())),
            Tag::CodeBlock(kind) => {
                self.flush_text();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::Table(_) => self.open_container(Ctx::Table {
                head: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            }),
            Tag::Emphasis => self.style.i += 1,
            Tag::Strong => self.style.b += 1,
            Tag::Strikethrough => self.style.s += 1,
            Tag::Link { dest_url, .. } => self.style.href.push(dest_url.to_string()),
            // An image is its alt text, as text.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let spans = std::mem::take(&mut self.spans);
                if !spans.is_empty() {
                    self.push_block(Block::Paragraph { spans });
                }
            }
            TagEnd::Heading(level) => {
                let spans = std::mem::take(&mut self.spans);
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.push_block(Block::Heading { level, spans });
            }
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::Table => {
                self.close_container();
            }
            TagEnd::CodeBlock => {
                if let Some((lang, mut text)) = self.code.take() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block::Code { lang, text });
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let spans_done = std::mem::take(&mut self.spans);
                debug_assert!(spans_done.is_empty());
                if let Some(Ctx::Table { head, rows, row }) = self.stack.last_mut() {
                    let done = std::mem::take(row);
                    if matches!(tag, TagEnd::TableHead) {
                        *head = done;
                    } else {
                        rows.push(done);
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.spans);
                if let Some(Ctx::Table { row, .. }) = self.stack.last_mut() {
                    row.push(cell);
                }
            }
            TagEnd::Emphasis => self.style.i = self.style.i.saturating_sub(1),
            TagEnd::Strong => self.style.b = self.style.b.saturating_sub(1),
            TagEnd::Strikethrough => self.style.s = self.style.s.saturating_sub(1),
            TagEnd::Link => {
                self.style.href.pop();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> Span {
        Span {
            t: text.into(),
            b: false,
            i: false,
            s: false,
            code: false,
            href: None,
        }
    }

    #[test]
    fn headings_paragraphs_and_inline_styles() {
        let b = blocks("# Title\n\nSome **bold**, *em*, `code` and [a link](http://x).\n");
        assert_eq!(
            b[0],
            Block::Heading {
                level: 1,
                spans: vec![t("Title")]
            }
        );
        let Block::Paragraph { spans } = &b[1] else {
            panic!("{b:?}")
        };
        assert_eq!(spans[0], t("Some "));
        assert!(spans[1].b && spans[1].t == "bold");
        assert!(spans[3].i && spans[3].t == "em");
        assert!(spans[5].code && spans[5].t == "code");
        assert_eq!(spans[7].href.as_deref(), Some("http://x"));
        assert_eq!(spans[7].t, "a link");
    }

    #[test]
    fn lists_nest_and_tight_items_are_paragraphs() {
        let b = blocks("- one\n- two\n  - inner\n\n1. a\n2. b\n");
        let Block::Ul { items } = &b[0] else {
            panic!("{b:?}")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            vec![Block::Paragraph {
                spans: vec![t("one")]
            }]
        );
        assert!(matches!(items[1][1], Block::Ul { .. }));
        let Block::Ol { start, items } = &b[1] else {
            panic!("{b:?}")
        };
        assert_eq!((*start, items.len()), (1, 2));
    }

    #[test]
    fn code_quote_rule_and_table() {
        let b = blocks(
            "```rust\nfn main() {}\n```\n\n> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        );
        assert_eq!(
            b[0],
            Block::Code {
                lang: "rust".into(),
                text: "fn main() {}".into()
            }
        );
        assert!(matches!(&b[1], Block::Quote { blocks } if blocks.len() == 1));
        assert_eq!(b[2], Block::Rule);
        let Block::Table { head, rows } = &b[3] else {
            panic!("{b:?}")
        };
        assert_eq!(head, &vec![vec![t("a")], vec![t("b")]]);
        assert_eq!(rows, &vec![vec![vec![t("1")], vec![t("2")]]]);
    }

    #[test]
    fn task_items_and_raw_html() {
        let b = blocks("- [x] done\n- [ ] todo\n\n<div>raw</div>\n");
        let Block::Ul { items } = &b[0] else {
            panic!("{b:?}")
        };
        let Block::Paragraph { spans } = &items[0][0] else {
            panic!()
        };
        assert_eq!(spans[0].t, "☑ done");
        assert!(!format!("{b:?}").contains("raw"));
    }

    #[test]
    fn a_hostile_document_is_bounded() {
        // Quotes nested far past the limit, and more blocks than the cap.
        let deep = "> ".repeat(500) + "x";
        let b = blocks(&deep);
        assert!(!b.is_empty());
        let many = "a\n\n".repeat(MAX_BLOCKS + 500);
        assert!(blocks(&many).len() <= MAX_BLOCKS);
    }

    #[test]
    fn the_wire_form_is_the_documented_one() {
        let json = serde_json::to_value(blocks("# H\n\n**x**\n")).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"k": "h", "level": 1, "spans": [{"t": "H"}]},
                {"k": "p", "spans": [{"t": "x", "b": true}]}
            ])
        );
    }
}
