//! The GitHub-style diff viewer (REQ-21): a file tree on the left, every
//! file stacked in one scroll on the right with the current file's header
//! pinned on top, side by side by default with changed words marked.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::draw::{BG, DEV, DIM, FG, FOCUS, LINE, PANEL, WAIT, WORK};
use crate::diff::{self, File, Kind, Status};

const DEL_BG: Color = Color::Rgb(0x3b, 0x22, 0x29);
const DEL_WORD: Color = Color::Rgb(0x7a, 0x2e, 0x3a);
const ADD_BG: Color = Color::Rgb(0x17, 0x37, 0x2b);
const ADD_WORD: Color = Color::Rgb(0x1e, 0x6b, 0x47);
const TREE: u16 = 30;

pub struct DiffView {
    pub title: String,
    /// Where the files live, to open one in the reader.
    pub dir: String,
    pub files: Vec<File>,
    /// Index into the flattened items of the top visible row.
    scroll: usize,
    viewed: BTreeSet<usize>,
    split: bool,
    /// Rows the content showed last frame, for paging.
    height: usize,
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
    /// Open this file of the worktree in the Markdown reader.
    Read(String),
}

impl DiffView {
    pub fn new(title: String, dir: String, files: Vec<File>) -> Self {
        Self {
            title,
            dir,
            files,
            scroll: 0,
            viewed: BTreeSet::new(),
            split: true,
            height: 20,
        }
    }

    fn rows_of(&self, f: usize, h: usize) -> usize {
        let hunk = &self.files[f].hunks[h];
        if self.split {
            diff::split_rows(hunk).len()
        } else {
            hunk.lines.len()
        }
    }

    fn items(&self) -> Vec<Item> {
        let mut out = Vec::new();
        for (f, file) in self.files.iter().enumerate() {
            out.push(Item::File(f));
            if self.viewed.contains(&f) {
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

    /// The file the top of the view is in.
    fn current(&self, items: &[Item]) -> usize {
        items[..=self.scroll.min(items.len().saturating_sub(1))]
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::File(f) => Some(*f),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        let items = self.items();
        let last = items.len().saturating_sub(1);
        let page = self.height.max(2) - 1;
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
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Char('j') => {
                self.scroll = find(self.scroll, true, &is_file).unwrap_or(self.scroll)
            }
            KeyCode::Char('k') => self.scroll = find(self.scroll, false, &is_file).unwrap_or(0),
            KeyCode::Char(']') => {
                self.scroll = find(self.scroll, true, &is_hunk).unwrap_or(self.scroll)
            }
            KeyCode::Char('[') => self.scroll = find(self.scroll, false, &is_hunk).unwrap_or(0),
            KeyCode::Down => self.scroll = (self.scroll + 1).min(last),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.scroll = (self.scroll + page / 2).min(last),
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(page / 2),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll = (self.scroll + page).min(last),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = last,
            KeyCode::Char('t') => {
                // Keep the same file on top across the switch.
                let f = self.current(&items);
                self.split = !self.split;
                self.goto_file(f);
            }
            KeyCode::Char('v') => {
                let f = self.current(&items);
                if !self.viewed.remove(&f) {
                    self.viewed.insert(f);
                    // Like GitHub: marking a file viewed moves on to the next.
                    self.goto_file((f + 1).min(self.files.len().saturating_sub(1)));
                    return Action::None;
                }
                self.goto_file(f);
            }
            KeyCode::Enter => {
                let f = self.current(&items);
                let path = &self.files[f].path;
                if path.ends_with(".md") && self.files[f].status != Status::Deleted {
                    return Action::Read(path.clone());
                }
            }
            _ => {}
        }
        Action::None
    }

    fn goto_file(&mut self, f: usize) {
        if let Some(i) = self.items().iter().position(|i| *i == Item::File(f)) {
            self.scroll = i;
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let (adds, dels): (usize, usize) = self
            .files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added(), d + f.deleted()));
        let mode = if self.split { "split" } else { "unified" };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(FOCUS))
            .title(Line::from(vec![
                Span::styled(
                    format!(" diff · {} ", self.title),
                    Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} files ", self.files.len()),
                    Style::default().fg(DIM),
                ),
                Span::styled(format!("+{adds} "), Style::default().fg(DEV)),
                Span::styled(format!("−{dels} "), Style::default().fg(WAIT)),
            ]))
            .title(
                Line::from(Span::styled(format!(" {mode} "), Style::default().fg(DIM)))
                    .right_aligned(),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.files.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    " No changes against the base branch.",
                    Style::default().fg(DIM),
                )),
                inner,
            );
            return;
        }

        let items = self.items();
        self.scroll = self.scroll.min(items.len().saturating_sub(1));
        let current = self.current(&items);
        let tree_w = TREE.min(inner.width / 3);
        let tree = Rect {
            width: tree_w,
            ..inner
        };
        let body = Rect {
            x: inner.x + tree_w + 1,
            width: inner.width.saturating_sub(tree_w + 1),
            ..inner
        };
        self.draw_tree(frame, tree, current);
        for y in inner.y..inner.y + inner.height {
            if let Some(c) = frame.buffer_mut().cell_mut((inner.x + tree_w, y)) {
                c.set_symbol("│").set_style(Style::default().fg(LINE));
            }
        }

        self.height = body.height as usize;
        let mut lines = Vec::with_capacity(self.height);
        // The current file's header stays on top while scrolling through it.
        let mut start = self.scroll;
        if !matches!(items.get(start), Some(Item::File(_))) {
            lines.push(self.file_header(current, body.width));
        } else {
            lines.push(self.file_header(current, body.width));
            start += 1;
        }
        for item in items.iter().skip(start).take(self.height.saturating_sub(1)) {
            lines.push(self.item_line(*item, body.width));
        }
        frame.render_widget(Paragraph::new(lines), body);
    }

    fn draw_tree(&self, frame: &mut Frame, area: Rect, current: usize) {
        let mut lines = vec![Line::from(Span::styled(
            " Files",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        ))];
        // Keep the current file in sight in a long list.
        let room = area.height.saturating_sub(1) as usize;
        let first = current.saturating_sub(room.saturating_sub(1));
        for (i, f) in self.files.iter().enumerate().skip(first).take(room) {
            let on = i == current;
            let mark = if self.viewed.contains(&i) {
                "✓"
            } else if on {
                "▶"
            } else {
                " "
            };
            let name = f.path.rsplit('/').next().unwrap_or(&f.path);
            let counts = format!("+{} −{}", f.added(), f.deleted());
            let w = area.width as usize;
            let room_for_name = w.saturating_sub(counts.chars().count() + 4);
            let style = if on {
                Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)
            } else if self.viewed.contains(&i) {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(FG)
            };
            let mut line = Line::from(vec![
                Span::styled(
                    format!(" {mark} "),
                    Style::default().fg(if on { FOCUS } else { DEV }),
                ),
                Span::styled(fit(name, room_for_name), style),
                Span::raw(" "),
                Span::styled(counts, Style::default().fg(DIM)),
            ]);
            if on {
                line = line.style(Style::default().bg(PANEL));
            }
            lines.push(line);
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn file_header(&self, f: usize, width: u16) -> Line<'static> {
        let file = &self.files[f];
        let status = match file.status {
            Status::Added => "new",
            Status::Deleted => "deleted",
            Status::Renamed => "renamed",
            Status::Modified => "",
        };
        let name = match &file.old_path {
            Some(old) => format!("{old} → {}", file.path),
            None => file.path.clone(),
        };
        let mut spans = vec![
            Span::styled(
                format!(" {name} "),
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("+{} ", file.added()), Style::default().fg(DEV)),
            Span::styled(format!("−{} ", file.deleted()), Style::default().fg(WAIT)),
        ];
        if !status.is_empty() {
            spans.push(Span::styled(
                format!(" {status} "),
                Style::default().fg(WORK),
            ));
        }
        if self.viewed.contains(&f) {
            spans.push(Span::styled(" ✓ viewed ", Style::default().fg(DEV)));
        }
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        spans.push(Span::raw(" ".repeat((width as usize).saturating_sub(used))));
        Line::from(spans).style(Style::default().bg(PANEL))
    }

    fn item_line(&self, item: Item, width: u16) -> Line<'static> {
        match item {
            Item::File(f) => self.file_header(f, width),
            Item::Gap => Line::default(),
            Item::Note(f) => {
                let file = &self.files[f];
                let note = if file.binary {
                    " binary file"
                } else {
                    " no text changes"
                };
                Line::from(Span::styled(note, Style::default().fg(DIM)))
            }
            Item::Hunk(f, h) => {
                let hunk = &self.files[f].hunks[h];
                Line::from(vec![
                    Span::styled(
                        format!(" @@ -{} +{} @@ ", hunk.old_start, hunk.new_start),
                        Style::default().fg(WORK),
                    ),
                    Span::styled(hunk.context.clone(), Style::default().fg(DIM)),
                ])
            }
            Item::Row(f, h, r) => {
                let hunk = &self.files[f].hunks[h];
                if self.split {
                    split_line(&diff::split_rows(hunk)[r], width, &self.files[f].path)
                } else {
                    uni_line(&hunk.lines[r], width, &self.files[f].path)
                }
            }
        }
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
    spans.push(Span::styled("│", Style::default().fg(LINE)));
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
        return vec![Span::styled(" ".repeat(width), Style::default().bg(BG))];
    };
    let no = if old { l.old_no } else { l.new_no };
    let (bg, word_bg, sign) = match l.kind {
        Kind::Del => (DEL_BG, DEL_WORD, "-"),
        Kind::Add => (ADD_BG, ADD_WORD, "+"),
        Kind::Ctx => (BG, BG, " "),
    };
    let gutter = format!(
        "{:>4} {sign}",
        no.map(|n| n.to_string()).unwrap_or_default()
    );
    let text_w = width.saturating_sub(gutter.chars().count() + 1);
    let mut spans = vec![
        Span::styled(gutter, Style::default().fg(DIM).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ];
    spans.extend(marked(&l.text, text_w, words, bg, word_bg, path));
    spans
}

fn uni_line(l: &diff::Line, width: u16, path: &str) -> Line<'static> {
    let (bg, sign) = match l.kind {
        Kind::Del => (DEL_BG, "-"),
        Kind::Add => (ADD_BG, "+"),
        Kind::Ctx => (BG, " "),
    };
    let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let gutter = format!("{:>4} {:>4} {sign} ", num(l.old_no), num(l.new_no));
    let text_w = (width as usize).saturating_sub(gutter.chars().count());
    let mut spans = vec![Span::styled(gutter, Style::default().fg(DIM).bg(bg))];
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
    'outer: for piece in crate::view::highlight::highlight_line(&text, path) {
        let fg = piece.style.fg.unwrap_or(FG);
        let mut run = String::new();
        let mut run_changed = None;
        for c in piece.content.chars() {
            if used >= width {
                break 'outer;
            }
            let changed = in_words(at);
            if run_changed.is_some_and(|r| r != changed) {
                let bg = if run_changed == Some(true) {
                    word_bg
                } else {
                    bg
                };
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(fg).bg(bg),
                ));
            }
            run_changed = Some(changed);
            run.push(c);
            at += c.len_utf8();
            used += 1;
        }
        if !run.is_empty() {
            let bg = if run_changed == Some(true) {
                word_bg
            } else {
                bg
            };
            spans.push(Span::styled(run, Style::default().fg(fg).bg(bg)));
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

fn fit(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    let keep: String = s.chars().take(width.saturating_sub(1)).collect();
    format!("{keep}…")
}
