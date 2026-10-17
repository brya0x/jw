//! Markdown as blocks (spec REQ-465 to REQ-468, RAT-45): the file of a workspace or the plan that
//! waits in a conversation, read once and parsed with `pulldown-cmark` into the `MarkdownDoc`
//! every client draws its own way. Nothing here changes a file.

use std::fs;
use std::iter::Peekable;
use std::path::{Path, PathBuf};

use contract::ErrorCode;
use contract::markdown::{Inline, MarkdownDoc, MdBlock, MdItem};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::commands::{Done, ErrorBody, err};
use crate::server::Shared;

/// The most a file may weigh to be read: a document is one reply.
const MAX_FILE: u64 = 1024 * 1024;
/// The most of a plan that is kept.
const MAX_PLAN: usize = 64 * 1024;

/// `read_markdown`: the blocks of a Markdown file of the workspace (REQ-465, REQ-466).
pub fn read(shared: &Shared, workspace: &str, path: &str) -> Done {
    let root = shared
        .lock()
        .workspace_dir(workspace)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no workspace {workspace}")))?;
    let file = file_in(&root, path)?;
    let text = fs::read_to_string(&file).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => err(
            ErrorCode::BadRequest,
            format!("{path} is not text: the viewer reads UTF-8"),
        ),
        _ => err(ErrorCode::Io, format!("cannot read {path}: {e}")),
    })?;
    let name = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    reply(document(
        &text,
        &name,
        Some(file.to_string_lossy().into_owned()),
        false,
    ))
}

/// `read_plan`: the plan that waits as `ask` in the conversation `node`, whole (REQ-467).
pub fn read_plan(shared: &Shared, node: &str, ask: &str) -> Done {
    let (mut text, path) = shared
        .lock()
        .plan_waiting(node, ask)
        .ok_or_else(|| err(ErrorCode::BadRequest, "that plan is not waiting"))?;
    let truncated = text.len() > MAX_PLAN;
    if truncated {
        let mut end = MAX_PLAN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    reply(document(&text, "plan", path, truncated))
}

fn reply(doc: MarkdownDoc) -> Done {
    Ok(Some(
        serde_json::to_value(doc).expect("a document serialises"),
    ))
}

/// The file `path` names, which must be a Markdown file inside `root` once links are followed,
/// small enough to be read.
fn file_in(root: &Path, path: &str) -> Result<PathBuf, ErrorBody> {
    let root = fs::canonicalize(root).map_err(|e| {
        err(
            ErrorCode::Io,
            format!("cannot read {}: {e}", root.display()),
        )
    })?;
    let file = fs::canonicalize(root.join(path))
        .map_err(|_| err(ErrorCode::NotFound, format!("no file {path}")))?;
    if !file.starts_with(&root) {
        return Err(err(
            ErrorCode::BadRequest,
            "that file is outside the workspace",
        ));
    }
    let markdown = file
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["md", "markdown"].contains(&e.to_lowercase().as_str()));
    if !markdown {
        return Err(err(
            ErrorCode::BadRequest,
            format!("{path} is not a Markdown file: the viewer reads .md and .markdown"),
        ));
    }
    let meta =
        fs::metadata(&file).map_err(|e| err(ErrorCode::Io, format!("cannot read {path}: {e}")))?;
    if !meta.is_file() {
        return Err(err(ErrorCode::BadRequest, format!("{path} is not a file")));
    }
    if meta.len() > MAX_FILE {
        return Err(err(
            ErrorCode::BadRequest,
            format!(
                "{path} weighs {} KiB: the viewer reads up to {} KiB",
                meta.len() / 1024,
                MAX_FILE / 1024
            ),
        ));
    }
    Ok(file)
}

/// The document of `text`; its title is the first heading, else `name`.
fn document(text: &str, name: &str, path: Option<String>, truncated: bool) -> MarkdownDoc {
    let blocks = parse(text);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MdBlock::Heading { text, .. } => Some(plain(text)),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| name.to_owned());
    MarkdownDoc {
        title,
        path,
        truncated,
        blocks,
    }
}

/// The text of a run, without its styles.
fn plain(text: &[Inline]) -> String {
    text.iter().map(|i| i.t.as_str()).collect()
}

/// How deep quotes and lists may nest before the text is not read as blocks: the reader
/// recurses once for each level, and a crash here takes down the daemon.
const MAX_DEPTH: usize = 32;

/// The longest text of a conversation that is read as blocks (REQ-560).
const MAX_CONVERSATION_TEXT: usize = 64 * 1024;

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// Whether quotes and lists nest deeper than the reader follows. It walks the events with a
/// counter, so it does not recurse itself.
fn too_deep(text: &str) -> bool {
    let mut depth = 0usize;
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::List(_)) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return true;
                }
            }
            Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_)) => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    false
}

/// The blocks of Markdown `text`. Text nested deeper than `MAX_DEPTH` is one block of code, as
/// it was written.
pub fn parse(text: &str) -> Vec<MdBlock> {
    if too_deep(text) {
        return vec![MdBlock::Code {
            lang: String::new(),
            text: text.to_owned(),
        }];
    }
    let mut events = Parser::new_ext(text, options()).peekable();
    blocks_until(&mut events, None)
}

/// The blocks of a finished text of a conversation, or `None` when it is plain paragraphs (the
/// client draws `text` as it is), too long, or nested too deep (REQ-560).
pub fn conversation_blocks(text: &str) -> Option<Vec<MdBlock>> {
    if text.len() > MAX_CONVERSATION_TEXT || too_deep(text) {
        return None;
    }
    let blocks = parse(text);
    let plain = blocks.iter().all(|b| match b {
        MdBlock::Para { text } => text
            .iter()
            .all(|i| !i.b && !i.i && !i.code && i.href.is_none()),
        _ => false,
    });
    (!plain).then_some(blocks)
}

type Events<'a> = Peekable<Parser<'a>>;

/// The style the text being read has.
#[derive(Default, Clone)]
struct Style {
    bold: u32,
    italic: u32,
    links: Vec<String>,
}

/// Blocks up to the end of the container that began (`None`: the end of the document). Text that
/// comes with no paragraph around it (an item of a tight list) is a paragraph.
fn blocks_until<'a>(events: &mut Events<'a>, end: Option<TagEnd>) -> Vec<MdBlock> {
    let mut out = Vec::new();
    let mut run: Vec<Inline> = Vec::new();
    let mut style = Style::default();
    let flush = |run: &mut Vec<Inline>, out: &mut Vec<MdBlock>| {
        if !run.is_empty() {
            out.push(MdBlock::Para {
                text: std::mem::take(run),
            });
        }
    };
    while let Some(event) = events.next() {
        match event {
            Event::End(tag) if Some(tag) == end => break,
            Event::End(_) => {}
            Event::Start(Tag::Paragraph) => {
                flush(&mut run, &mut out);
                let text = inline_until(events, TagEnd::Paragraph);
                out.push(MdBlock::Para { text });
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut run, &mut out);
                let text = inline_until(events, TagEnd::Heading(level));
                out.push(MdBlock::Heading {
                    level: level_of(level),
                    text,
                });
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                flush(&mut run, &mut out);
                out.push(MdBlock::Quote {
                    blocks: blocks_until(events, Some(TagEnd::BlockQuote(kind))),
                });
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut run, &mut out);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                let mut code = String::new();
                for e in events.by_ref() {
                    match e {
                        Event::Text(t) => code.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                out.push(MdBlock::Code { lang, text: code });
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut run, &mut out);
                out.push(MdBlock::List {
                    start: start.map(|n| n.min(u64::from(u32::MAX)) as u32),
                    items: items_until(events),
                });
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut run, &mut out);
                out.push(table_until(events));
            }
            Event::Rule => {
                flush(&mut run, &mut out);
                out.push(MdBlock::Rule);
            }
            Event::Html(h) => {
                flush(&mut run, &mut out);
                out.push(MdBlock::Para {
                    text: vec![text_of(h.trim_end().to_owned(), &Style::default())],
                });
            }
            // Text with no paragraph around it: the contents of an item of a tight list.
            other => inline_event(other, events, &mut style, &mut run),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// The items of a list, up to its end.
fn items_until<'a>(events: &mut Events<'a>) -> Vec<MdItem> {
    let mut items = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(TagEnd::List(_)) => break,
            Event::Start(Tag::Item) => {
                // A checkbox comes first among what the item holds.
                let task = match events.peek() {
                    Some(Event::TaskListMarker(done)) => Some(*done),
                    _ => None,
                };
                if task.is_some() {
                    events.next();
                }
                let blocks = blocks_until(events, Some(TagEnd::Item));
                items.push(MdItem { task, blocks });
            }
            _ => {}
        }
    }
    items
}

/// A table, up to its end: the first row is the head.
fn table_until<'a>(events: &mut Events<'a>) -> MdBlock {
    let mut head = Vec::new();
    let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
    let mut row: Vec<Vec<Inline>> = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(TagEnd::Table) => break,
            Event::Start(Tag::TableCell) => row.push(inline_until(events, TagEnd::TableCell)),
            Event::End(TagEnd::TableHead) => head = std::mem::take(&mut row),
            Event::End(TagEnd::TableRow) => rows.push(std::mem::take(&mut row)),
            _ => {}
        }
    }
    MdBlock::Table { head, rows }
}

/// The text up to the end of `end`.
fn inline_until<'a>(events: &mut Events<'a>, end: TagEnd) -> Vec<Inline> {
    let mut run = Vec::new();
    let mut style = Style::default();
    while let Some(event) = events.next() {
        if matches!(&event, Event::End(tag) if *tag == end) {
            break;
        }
        inline_event(event, events, &mut style, &mut run);
    }
    run
}

/// One event of text: its words, with the style it is in.
fn inline_event<'a>(
    event: Event<'a>,
    events: &mut Events<'a>,
    style: &mut Style,
    run: &mut Vec<Inline>,
) {
    match event {
        Event::Text(t) => push(run, text_of(t.into_string(), style)),
        Event::Code(t) => {
            let mut i = text_of(t.into_string(), style);
            i.code = true;
            push(run, i);
        }
        Event::SoftBreak => push(run, text_of(" ".into(), style)),
        Event::HardBreak => push(run, text_of("\n".into(), style)),
        Event::InlineHtml(h) | Event::Html(h) => push(run, text_of(h.into_string(), style)),
        Event::FootnoteReference(l) => push(run, text_of(format!("[{l}]"), style)),
        Event::Start(Tag::Strong) => style.bold += 1,
        Event::End(TagEnd::Strong) => style.bold = style.bold.saturating_sub(1),
        Event::Start(Tag::Emphasis) => style.italic += 1,
        Event::End(TagEnd::Emphasis) => style.italic = style.italic.saturating_sub(1),
        Event::Start(Tag::Link { dest_url, .. }) => style.links.push(dest_url.into_string()),
        Event::End(TagEnd::Link) => {
            style.links.pop();
        }
        Event::Start(Tag::Image { .. }) => {
            let mut alt = String::new();
            for e in events.by_ref() {
                match e {
                    Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                    Event::End(TagEnd::Image) => break,
                    _ => {}
                }
            }
            push(run, text_of(format!("[imagen: {alt}]"), style));
        }
        _ => {}
    }
}

fn text_of(t: String, style: &Style) -> Inline {
    Inline {
        t,
        b: style.bold > 0,
        i: style.italic > 0,
        code: false,
        href: style.links.last().cloned(),
    }
}

/// Add a run, joined to the one before it when they look the same.
fn push(run: &mut Vec<Inline>, next: Inline) {
    if next.t.is_empty() {
        return;
    }
    if let Some(last) = run.last_mut()
        && (last.b, last.i, last.code, &last.href) == (next.b, next.i, next.code, &next.href)
    {
        last.t.push_str(&next.t);
        return;
    }
    run.push(next);
}

fn level_of(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(text: &[Inline]) -> Vec<(String, bool, bool, bool, Option<&str>)> {
        text.iter()
            .map(|i| (i.t.clone(), i.b, i.i, i.code, i.href.as_deref()))
            .collect()
    }

    #[test]
    fn text_keeps_bold_italic_code_and_links_and_joins_what_looks_the_same() {
        let blocks = parse("a **b *c* d** `e` [f **g**](u) ![h](i.png)\nl\\\nm");
        let [MdBlock::Para { text }] = &blocks[..] else {
            panic!("{blocks:?}");
        };
        let got = runs(text);
        assert_eq!(got[0], ("a ".into(), false, false, false, None));
        assert_eq!(got[1], ("b ".into(), true, false, false, None));
        assert_eq!(got[2], ("c".into(), true, true, false, None));
        assert_eq!(got[3], (" d".into(), true, false, false, None));
        assert_eq!(got[5], ("e".into(), false, false, true, None));
        assert_eq!(got[7], ("f ".into(), false, false, false, Some("u")));
        assert_eq!(got[8], ("g".into(), true, false, false, Some("u")));
        assert_eq!(
            got[9].0, " [imagen: h] l\nm",
            "an image is text, a soft break a space, a hard break a line, and what looks the same is one run"
        );
        assert_eq!(got.len(), 10);
    }

    #[test]
    fn lists_nest_tasks_are_marked_and_a_tight_list_has_paragraphs() {
        let blocks = parse("1. uno\n2. dos\n   - [x] a\n   - [ ] b\n\n- suelto\n");
        let MdBlock::List { start, items } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(*start, Some(1));
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0].blocks[0], MdBlock::Para { text } if plain(text) == "uno"));
        let MdBlock::List {
            start: None,
            items: inner,
        } = &items[1].blocks[1]
        else {
            panic!("{:?}", items[1]);
        };
        assert_eq!(
            inner.iter().map(|i| i.task).collect::<Vec<_>>(),
            [Some(true), Some(false)]
        );
        assert!(
            matches!(&blocks[1], MdBlock::List { start: None, .. }),
            "the next list is its own"
        );
    }

    #[test]
    fn quotes_code_tables_rules_and_html_are_blocks() {
        let blocks = parse(
            "> cita\n> - dentro\n\n```rust\nlet x = 1;\n```\n\n    sangrado\n\n| a | b |\n|---|---|\n| `c` | d |\n\n---\n\n<div>html</div>\n",
        );
        assert!(matches!(&blocks[0], MdBlock::Quote { blocks } if blocks.len() == 2));
        assert_eq!(
            blocks[1],
            MdBlock::Code {
                lang: "rust".into(),
                text: "let x = 1;\n".into()
            }
        );
        assert!(
            matches!(&blocks[2], MdBlock::Code { lang, text } if lang.is_empty() && text == "sangrado\n")
        );
        let MdBlock::Table { head, rows } = &blocks[3] else {
            panic!("{:?}", blocks[3]);
        };
        assert_eq!((head.len(), rows.len(), rows[0].len()), (2, 1, 2));
        assert!(rows[0][0][0].code);
        assert_eq!(blocks[4], MdBlock::Rule);
        assert!(
            matches!(&blocks[5], MdBlock::Para { text } if plain(text).contains("<div>")),
            "html is text, not lost"
        );
    }

    #[test]
    fn the_title_is_the_first_heading_without_its_marks_else_the_name_and_a_big_plan_is_cut_on_a_character()
     {
        assert_eq!(
            document("texto\n\n## **Hola** `x`\n", "a.md", None, false).title,
            "Hola x"
        );
        assert_eq!(document("solo texto", "a.md", None, false).title, "a.md");
        assert_eq!(document("", "a.md", None, false).blocks, []);
        assert!(parse("").is_empty());
        // a document of lines that go on (no panic, every line kept)
        let long = "- x\n".repeat(5000);
        let MdBlock::List { items, .. } = &parse(&long)[0] else {
            panic!("a list");
        };
        assert_eq!(items.len(), 5000);
    }

    #[test]
    fn a_text_of_a_conversation_is_blocks_only_when_it_is_more_than_plain_paragraphs() {
        assert_eq!(
            conversation_blocks("Listo.\n\nSigo con el siguiente."),
            None
        );
        assert_eq!(conversation_blocks("una línea\notra línea"), None);
        let rich = conversation_blocks("Ver `x` y **y**.").expect("styled text");
        assert!(matches!(&rich[..], [MdBlock::Para { .. }]));
        let table = conversation_blocks("a\n\n| c | d |\n|---|---|\n| 1 | 2 |").expect("a table");
        assert!(matches!(
            &table[..],
            [MdBlock::Para { .. }, MdBlock::Table { .. }]
        ));
        assert!(conversation_blocks("[la guía](docs/a.md)").is_some());
        assert!(conversation_blocks("- uno\n- dos").is_some());
    }

    #[test]
    fn a_text_that_is_too_long_or_nested_too_deep_is_not_read_as_blocks() {
        let long = format!("**b** {}", "x".repeat(MAX_CONVERSATION_TEXT));
        assert_eq!(conversation_blocks(&long), None);
        let quote = format!("{} x", ">".repeat(10_000));
        assert_eq!(conversation_blocks(&quote), None);
        let nested: String = (0..5_000)
            .map(|n| format!("{}- x\n", "  ".repeat(n)))
            .collect();
        assert_eq!(conversation_blocks(&nested), None);
        let near = format!("{} x", ">".repeat(MAX_DEPTH));
        assert!(
            conversation_blocks(&near).is_some(),
            "the limit itself is read"
        );
    }

    #[test]
    fn a_document_nested_too_deep_is_one_block_of_code_and_does_not_overflow() {
        let quote = format!("{} x", ">".repeat(10_000));
        let blocks = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || parse(&quote))
            .unwrap()
            .join()
            .expect("no overflow");
        assert!(matches!(&blocks[..], [MdBlock::Code { .. }]));
    }
}
