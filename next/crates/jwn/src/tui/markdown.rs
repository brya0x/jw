//! A Markdown document laid out for the terminal client (spec REQ-471): the blocks the daemon
//! parsed (`MarkdownDoc`) as rows of styled text, as wide as the panel. Wrapping, the colours and
//! the borders are the client's; what the document says is the daemon's (RAT-45).

use contract::markdown::{Inline, MarkdownDoc, MdBlock};

use super::diff_lines::{Tone, tones};
use super::{Rgb, Style, look};

type Seg = (String, Style);

/// A row of the document on the screen.
pub struct Row {
    pub spans: Vec<Seg>,
    /// The ground of the whole row (code).
    pub bg: Option<Rgb>,
    /// It goes on from the row above, which the width broke: copied, they are one line.
    pub soft: bool,
    /// It goes on from the middle of a word that was longer than a row: copied, with no space.
    pub glued: bool,
    /// How many characters of the row are only how it is set (the bar of a quote, the hang under a
    /// list's mark): a copy leaves them out.
    pub lead: usize,
    /// The first row of a heading: its level and its text.
    pub head: Option<(u8, String)>,
}

impl Row {
    fn of(spans: Vec<Seg>) -> Row {
        Row {
            spans,
            bg: None,
            soft: false,
            glued: false,
            lead: 0,
            head: None,
        }
    }

    fn blank() -> Row {
        Row::of(Vec::new())
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|(t, _)| t.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.spans.iter().map(|(t, _)| t.chars().count()).sum()
    }

    /// Put `spans` before the row; when `decor` they are only how it is set.
    fn prefixed(mut self, spans: Seg, decor: bool) -> Row {
        let n = spans.0.chars().count();
        self.spans.insert(0, spans);
        self.lead = if decor { self.lead + n } else { 0 };
        self
    }
}

/// A heading of the document and the row it is on.
pub struct Head {
    pub level: u8,
    pub text: String,
    pub row: usize,
}

pub struct Layout {
    pub rows: Vec<Row>,
    pub heads: Vec<Head>,
}

/// The document as rows `width` cells wide.
pub fn layout(doc: &MarkdownDoc, width: usize) -> Layout {
    let rows = blocks(&doc.blocks, width.max(8), true);
    let heads = rows
        .iter()
        .enumerate()
        .filter_map(|(row, r)| {
            r.head.as_ref().map(|(level, text)| Head {
                level: *level,
                text: text.clone(),
                row,
            })
        })
        .collect();
    Layout { rows, heads }
}

/// What a range of rows says as text: the marks of how a row is set left out, the rows a width
/// broke in one line, the blanks between blocks kept.
pub fn text_of(rows: &[Row], from: (usize, usize), to: (usize, usize)) -> String {
    let mut out: Vec<String> = Vec::new();
    for (at, row) in rows.iter().enumerate().take(to.0 + 1).skip(from.0) {
        let chars: Vec<char> = row.text().chars().collect();
        let a = if at == from.0 { from.1 } else { 0 };
        let b = if at == to.0 {
            (to.1 + 1).min(chars.len())
        } else {
            chars.len()
        };
        let a = a.max(row.lead);
        if chars.is_empty() {
            out.push(String::new());
            continue;
        }
        if a >= b {
            continue;
        }
        let piece: String = chars[a..b].iter().collect();
        match out.last_mut() {
            Some(last) if row.soft && !last.is_empty() => {
                if !row.glued {
                    last.push(' ');
                }
                last.push_str(piece.trim());
            }
            _ => out.push(piece.trim_end().to_owned()),
        }
    }
    while out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// Blocks one under another; `gap` puts a blank row between them.
fn blocks(blocks: &[MdBlock], w: usize, gap: bool) -> Vec<Row> {
    let mut out = Vec::new();
    for b in blocks {
        let rows = block(b, w);
        if gap && !out.is_empty() && !rows.is_empty() {
            out.push(Row::blank());
        }
        out.extend(rows);
    }
    out
}

fn block(b: &MdBlock, w: usize) -> Vec<Row> {
    match b {
        MdBlock::Heading { level, text } => heading(*level, text, w),
        MdBlock::Para { text } => wrap(&segs(text, Style::fg(look::fg())), w),
        MdBlock::Quote { blocks: inner } => blocks(inner, w.saturating_sub(2), true)
            .into_iter()
            .map(|r| r.prefixed(("▎ ".into(), Style::fg(look::yellow())), true))
            .collect(),
        MdBlock::List { start, items } => {
            let mut out = Vec::new();
            for (n, item) in items.iter().enumerate() {
                let (mark, style) = match (item.task, start) {
                    (Some(true), _) => ("☑ ".to_owned(), Style::fg(look::green())),
                    (Some(false), _) => ("☐ ".to_owned(), Style::fg(look::dim())),
                    (None, Some(s)) => (format!("{}. ", *s as usize + n), Style::fg(look::dim())),
                    (None, None) => ("• ".to_owned(), Style::fg(look::cyan())),
                };
                let width = mark.chars().count();
                let mut rows = blocks(&item.blocks, w.saturating_sub(width), false).into_iter();
                match rows.next() {
                    Some(first) => out.push(first.prefixed((mark, style), false)),
                    None => out.push(Row::of(vec![(mark, style)])),
                }
                out.extend(rows.map(|r| r.prefixed((" ".repeat(width), Style::default()), true)));
            }
            out
        }
        MdBlock::Code { lang, text } => code(lang, text, w),
        MdBlock::Table { head, rows } => table(head, rows, w),
        MdBlock::Rule => vec![Row::of(vec![("─".repeat(w), Style::fg(look::dim()))])],
    }
}

fn heading(level: u8, text: &[Inline], w: usize) -> Vec<Row> {
    let colour = match level {
        1 => look::blue(),
        2 => look::cyan(),
        3 => look::green(),
        _ => look::fg(),
    };
    let base = Style::fg(colour).bold();
    let mut all = vec![(
        format!("{} ", "#".repeat(level.into())),
        Style::fg(look::dim()),
    )];
    all.extend(segs(text, base));
    let mut rows = wrap(&all, w);
    if let Some(first) = rows.first_mut() {
        let name: String = text.iter().map(|i| i.t.as_str()).collect();
        first.head = Some((level, name));
    }
    if level == 1 {
        // The line under it is only how it is set: a copy leaves it out.
        let mut under = Row::of(vec![("─".repeat(w), Style::fg(look::dim()))]);
        under.lead = w;
        rows.push(under);
    }
    rows
}

/// The text of a run as styled pieces, over `base`.
fn segs(text: &[Inline], base: Style) -> Vec<Seg> {
    text.iter()
        .map(|i| {
            let mut s = base;
            if i.b {
                s.bold = true;
            }
            if i.i {
                s.italic = true;
            }
            if i.code {
                s.fg = Some(look::cyan());
            }
            if i.href.is_some() {
                s.fg = Some(look::blue());
                s.underline = true;
            }
            (i.t.clone(), s)
        })
        .collect()
}

/// One thing a paragraph is made of.
enum Piece {
    /// What has no space inside it, whatever styles it is made of.
    Word(Vec<Seg>, usize),
    Space(Seg),
    Break,
}

fn pieces(segs: &[Seg]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut word: Vec<Seg> = Vec::new();
    let flush = |word: &mut Vec<Seg>, out: &mut Vec<Piece>| {
        if !word.is_empty() {
            let n = word.iter().map(|(t, _)| t.chars().count()).sum();
            out.push(Piece::Word(std::mem::take(word), n));
        }
    };
    for (text, style) in segs {
        let mut run = String::new();
        let mut spaces = false;
        let push = |run: &mut String, spaces: bool, word: &mut Vec<Seg>, out: &mut Vec<Piece>| {
            if run.is_empty() {
                return;
            }
            if spaces {
                flush(word, out);
                out.push(Piece::Space((std::mem::take(run), *style)));
            } else {
                word.push((std::mem::take(run), *style));
            }
        };
        for c in text.chars() {
            if c == '\n' {
                push(&mut run, spaces, &mut word, &mut out);
                flush(&mut word, &mut out);
                out.push(Piece::Break);
                continue;
            }
            if (c == ' ') != spaces {
                push(&mut run, spaces, &mut word, &mut out);
                spaces = c == ' ';
            }
            run.push(c);
        }
        push(&mut run, spaces, &mut word, &mut out);
    }
    flush(&mut word, &mut out);
    out
}

/// Split `atoms` after `room` characters.
fn cut(atoms: Vec<Seg>, room: usize) -> (Vec<Seg>, Vec<Seg>) {
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let mut left = room;
    for (text, style) in atoms {
        let n = text.chars().count();
        if left >= n {
            head.push((text, style));
            left -= n;
        } else if left == 0 {
            tail.push((text, style));
        } else {
            let a: String = text.chars().take(left).collect();
            let b: String = text.chars().skip(left).collect();
            head.push((a, style));
            tail.push((b, style));
            left = 0;
        }
    }
    (head, tail)
}

/// `segs` broken into rows of at most `w` cells, at spaces; a word longer than a row is cut.
fn wrap(segs: &[Seg], w: usize) -> Vec<Row> {
    // Nested quotes and lists can leave no room at all; a row holds at least a cell, or a word
    // longer than it would never be cut.
    let w = w.max(1);
    let mut rows: Vec<Row> = Vec::new();
    let mut row: Vec<Seg> = Vec::new();
    let (mut len, mut fresh) = (0usize, true);
    // What the row being made is: the continuation of the one above, and of a cut word.
    let (mut soft, mut glued) = (false, false);
    let finish = |row: &mut Vec<Seg>, soft: bool, glued: bool, rows: &mut Vec<Row>| {
        while row.last().is_some_and(|(t, _)| t.trim().is_empty()) {
            row.pop();
        }
        let mut r = Row::of(std::mem::take(row));
        r.soft = soft;
        r.glued = glued;
        rows.push(r);
    };
    for piece in pieces(segs) {
        match piece {
            Piece::Break => {
                finish(&mut row, soft, glued, &mut rows);
                (len, fresh, soft, glued) = (0, true, false, false);
            }
            Piece::Space(s) => {
                let n = s.0.chars().count();
                if !fresh && len + n <= w {
                    row.push(s);
                    len += n;
                }
            }
            Piece::Word(mut atoms, mut n) => {
                if len + n > w && !fresh {
                    finish(&mut row, soft, glued, &mut rows);
                    (len, soft, glued) = (0, true, false);
                }
                while len + n > w {
                    let (head, tail) = cut(atoms, w - len);
                    row.extend(head);
                    finish(&mut row, soft, glued, &mut rows);
                    (len, soft, glued) = (0, true, true);
                    n = tail.iter().map(|(t, _)| t.chars().count()).sum();
                    atoms = tail;
                }
                row.extend(atoms);
                len += n;
                fresh = false;
            }
        }
    }
    finish(&mut row, soft, glued, &mut rows);
    rows
}

fn tone_colour(t: Tone) -> Rgb {
    match t {
        Tone::Plain => look::fg(),
        Tone::Keyword => look::magenta(),
        Tone::Text => look::green(),
        Tone::Number => look::yellow(),
        Tone::Comment => look::dim(),
    }
}

/// Code on a ground of its own, with its language above it.
fn code(lang: &str, text: &str, w: usize) -> Vec<Row> {
    let ground = look::panel();
    let mut rows = Vec::new();
    let mut label = Row::of(vec![(
        format!(" {}", if lang.is_empty() { "código" } else { lang }),
        Style::fg(look::dim()),
    )]);
    label.bg = Some(ground);
    rows.push(label);
    for line in text.trim_end_matches('\n').lines() {
        let line = line.replace('\t', "    ");
        let shown: String = line.chars().take(w.saturating_sub(2)).collect();
        let tones = tones(&shown);
        let mut spans: Vec<Seg> = vec![(" ".into(), Style::default())];
        for (c, t) in shown.chars().zip(tones) {
            let style = Style::fg(tone_colour(t));
            match spans.last_mut() {
                Some((s, st)) if *st == style => s.push(c),
                _ => spans.push((c.to_string(), style)),
            }
        }
        let mut r = Row::of(spans);
        r.bg = Some(ground);
        r.lead = 1;
        rows.push(r);
    }
    let mut end = Row::of(Vec::new());
    end.bg = Some(ground);
    rows.push(end);
    rows
}

/// A table with its borders; a column that does not fit is cut with `…`.
fn table(head: &[Vec<Inline>], body: &[Vec<Vec<Inline>>], w: usize) -> Vec<Row> {
    let rows: Vec<&[Vec<Inline>]> = std::iter::once(head)
        .chain(body.iter().map(Vec::as_slice))
        .collect();
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return Vec::new();
    }
    let plain = |cell: &[Inline]| -> usize { cell.iter().map(|i| i.t.chars().count()).sum() };
    let mut widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| r.get(c).map_or(0, |x| plain(x)))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    // Borders and the space each side of a cell take 3 cells a column, and one more at the end.
    while widths.iter().sum::<usize>() + 3 * cols + 1 > w {
        let (at, widest) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, x)| **x)
            .map(|(i, x)| (i, *x))
            .unwrap_or((0, 0));
        if widest <= 3 {
            break;
        }
        widths[at] -= 1;
    }
    let dim = Style::fg(look::dim());
    let line = |l: &str, m: &str, r: &str| {
        let inner: Vec<String> = widths.iter().map(|x| "─".repeat(x + 2)).collect();
        Row::of(vec![(format!("{l}{}{r}", inner.join(m)), dim)])
    };
    let mut out = vec![line("┌", "┬", "┐")];
    for (ri, row) in rows.iter().enumerate() {
        let mut spans: Vec<Seg> = vec![("│".into(), dim)];
        for (c, width) in widths.iter().enumerate() {
            let cell = row.get(c).map_or(&[][..], |x| x.as_slice());
            let base = if ri == 0 {
                Style::fg(look::cyan()).bold()
            } else {
                Style::fg(look::fg())
            };
            let mut cell_segs = segs(cell, base);
            let n: usize = cell_segs.iter().map(|(t, _)| t.chars().count()).sum();
            if n > *width {
                let (mut kept, _) = cut(cell_segs, width.saturating_sub(1));
                kept.push(("…".into(), base));
                cell_segs = kept;
            }
            let shown: usize = cell_segs.iter().map(|(t, _)| t.chars().count()).sum();
            spans.push((" ".into(), Style::default()));
            spans.extend(cell_segs);
            spans.push((" ".repeat(width - shown.min(*width) + 1), Style::default()));
            spans.push(("│".into(), dim));
        }
        out.push(Row::of(spans));
        if ri == 0 {
            out.push(line("├", "┼", "┤"));
        }
    }
    out.push(line("└", "┴", "┘"));
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use contract::markdown::MdItem;

    pub fn t(s: &str) -> Inline {
        Inline {
            t: s.into(),
            b: false,
            i: false,
            code: false,
            href: None,
        }
    }

    pub fn para(s: &str) -> MdBlock {
        MdBlock::Para { text: vec![t(s)] }
    }

    pub fn head(level: u8, s: &str) -> MdBlock {
        MdBlock::Heading {
            level,
            text: vec![t(s)],
        }
    }

    pub fn doc(blocks: Vec<MdBlock>) -> MarkdownDoc {
        MarkdownDoc {
            title: "doc".into(),
            path: None,
            truncated: false,
            blocks,
        }
    }

    fn texts(l: &Layout) -> Vec<String> {
        l.rows.iter().map(Row::text).collect()
    }

    #[test]
    fn headings_are_marked_in_their_colours_and_listed_with_the_row_they_are_on() {
        let l = layout(
            &doc(vec![head(1, "Plan"), para("texto"), head(2, "Cambios")]),
            40,
        );
        assert_eq!(
            texts(&l),
            [
                "# Plan",
                "──────────────────────────────────────── ".trim_end(),
                "",
                "texto",
                "",
                "## Cambios"
            ]
        );
        assert_eq!(
            l.heads
                .iter()
                .map(|h| (h.level, h.text.as_str(), h.row))
                .collect::<Vec<_>>(),
            [(1, "Plan", 0), (2, "Cambios", 5)]
        );
        let style_of = |row: usize, word: &str| {
            l.rows[row]
                .spans
                .iter()
                .find(|(t, _)| t == word)
                .map(|(_, s)| *s)
                .unwrap()
        };
        assert_eq!(style_of(0, "Plan").fg, Some(look::blue()));
        assert_eq!(style_of(5, "Cambios").fg, Some(look::cyan()));
        assert!(style_of(0, "Plan").bold);
    }

    #[test]
    fn a_paragraph_wraps_by_words_keeps_a_word_with_what_touches_it_and_cuts_a_long_one() {
        let l = layout(&doc(vec![para("uno dos tres cuatro")]), 9);
        assert_eq!(texts(&l), ["uno dos", "tres", "cuatro"].map(String::from));
        assert_eq!(
            l.rows.iter().map(|r| r.soft).collect::<Vec<_>>(),
            [false, true, true],
            "the rows the width broke go on from the first"
        );
        // `x`. stays together: the dot does not start a row alone
        let b = MdBlock::Para {
            text: vec![
                t("antes "),
                Inline {
                    code: true,
                    ..t("Clock")
                },
                t(". Y más"),
            ],
        };
        assert_eq!(texts(&layout(&doc(vec![b]), 12))[0], "antes Clock.");
        assert_eq!(
            texts(&layout(&doc(vec![para("abcdefghijklmnopqrst")]), 8)),
            ["abcdefgh", "ijklmnop", "qrst"].map(String::from),
            "a word longer than a row is cut"
        );
        assert_eq!(
            texts(&layout(&doc(vec![para("a\nb")]), 8)),
            ["a", "b"],
            "a hard break is a new row, not a soft one"
        );
    }

    #[test]
    fn lists_hang_under_their_text_nest_and_mark_tasks_and_numbers() {
        let item = |task, text: &str| MdItem {
            task,
            blocks: vec![para(text)],
        };
        let nested = MdItem {
            task: None,
            blocks: vec![
                para("padre con un texto largo"),
                MdBlock::List {
                    start: None,
                    items: vec![item(Some(true), "hecho"), item(Some(false), "falta")],
                },
            ],
        };
        let l = layout(
            &doc(vec![MdBlock::List {
                start: Some(9),
                items: vec![item(None, "uno"), nested],
            }]),
            16,
        );
        assert_eq!(
            texts(&l),
            [
                "9. uno",
                "10. padre con un",
                "    texto largo",
                "    ☑ hecho",
                "    ☐ falta"
            ]
            .map(String::from)
        );
        assert_eq!(l.rows[2].lead, 4, "the hang is only how it is set");
        let bullets = layout(
            &doc(vec![MdBlock::List {
                start: None,
                items: vec![item(None, "a")],
            }]),
            16,
        );
        assert_eq!(texts(&bullets), ["• a"]);
    }

    #[test]
    fn a_quote_has_its_bar_and_code_its_own_ground_language_and_tones() {
        let l = layout(
            &doc(vec![
                MdBlock::Quote {
                    blocks: vec![para("una cita que ocupa varias filas")],
                },
                MdBlock::Code {
                    lang: "rust".into(),
                    text: "let x = \"a\"; // 1\n".into(),
                },
            ]),
            14,
        );
        let rows = texts(&l);
        assert_eq!(&rows[..3], ["▎ una cita que", "▎ ocupa varias", "▎ filas"]);
        assert_eq!(l.rows[1].lead, 2, "a copy leaves the bar out");
        assert_eq!(rows[4], " rust");
        assert_eq!(rows[5], " let x = \"a\";", "cut to the width");
        assert!(l.rows[4].bg.is_some() && l.rows[5].bg.is_some() && l.rows[6].bg.is_some());
        let tones: Vec<_> = l.rows[5]
            .spans
            .iter()
            .map(|(t, s)| (t.as_str(), s.fg))
            .collect();
        assert_eq!(tones[1], ("let", Some(look::magenta())));
        assert!(
            tones
                .iter()
                .any(|(t, f)| t.contains('"') && *f == Some(look::green()))
        );
        let bare = layout(
            &doc(vec![MdBlock::Code {
                lang: String::new(),
                text: "x\n".into(),
            }]),
            14,
        );
        assert_eq!(texts(&bare)[0], " código");
    }

    #[test]
    fn a_table_has_borders_a_head_and_cuts_what_does_not_fit() {
        let cell = |s: &str| vec![t(s)];
        let tb = MdBlock::Table {
            head: vec![cell("archivo"), cell("cambio")],
            rows: vec![vec![cell("src/auth.rs"), cell("el reloj y todo lo demás")]],
        };
        let l = layout(&doc(vec![tb.clone()]), 60);
        assert_eq!(
            texts(&l),
            [
                "┌─────────────┬──────────────────────────┐",
                "│ archivo     │ cambio                   │",
                "├─────────────┼──────────────────────────┤",
                "│ src/auth.rs │ el reloj y todo lo demás │",
                "└─────────────┴──────────────────────────┘",
            ]
        );
        let narrow = layout(&doc(vec![tb]), 30);
        assert!(
            texts(&narrow).iter().all(|r| r.chars().count() <= 30),
            "{:?}",
            texts(&narrow)
        );
        assert!(texts(&narrow)[3].contains('…'));
    }

    #[test]
    fn what_is_copied_leaves_the_marks_out_joins_broken_rows_and_keeps_the_blanks() {
        let l = layout(
            &doc(vec![
                MdBlock::Quote {
                    blocks: vec![para("una cita que ocupa varias filas")],
                },
                para("fin"),
            ]),
            14,
        );
        let last = l.rows.len() - 1;
        assert_eq!(
            text_of(&l.rows, (0, 0), (last, usize::MAX - 1)),
            "una cita que ocupa varias filas\n\nfin"
        );
        assert_eq!(text_of(&l.rows, (0, 6), (0, 9)), "cita", "a part of a row");
    }

    #[test]
    fn quotes_nested_deeper_than_the_panel_is_wide_still_come_out() {
        let mut b = para("una palabralarguísima y otra");
        for _ in 0..12 {
            b = MdBlock::Quote { blocks: vec![b] };
        }
        let l = layout(&doc(vec![b]), 20);
        assert!(l.rows.len() > 5, "{} rows", l.rows.len());
        let text = text_of(&l.rows, (0, 0), (l.rows.len() - 1, usize::MAX - 1));
        assert!(
            text.contains("palabralarguísima"),
            "a cut word is copied with no space in it: {text}"
        );
    }
}
