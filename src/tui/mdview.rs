//! The Markdown reader (REQ-22): the file rendered, read-only, with its
//! headings listed on the left when there is room.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::diffview::DiffView;
use super::draw::{DIM, FG, FOCUS, LINE, PANEL};
use crate::view::md::{self, Doc};

/// Below this width the heading list would squeeze the text: no list.
const TOC_FROM: u16 = 100;
const TOC: u16 = 28;

pub struct MdView {
    pub title: String,
    src: String,
    doc: Doc,
    /// The width `doc` was rendered for; a resize renders it again.
    width: u16,
    scroll: usize,
    height: usize,
    /// The diff this was opened from, to go back to.
    pub back: Option<Box<DiffView>>,
}

impl MdView {
    pub fn new(title: String, src: String, back: Option<Box<DiffView>>) -> Self {
        Self {
            title,
            doc: md::render(&src, 80),
            src,
            width: 80,
            scroll: 0,
            height: 20,
            back,
        }
    }

    /// The heading the top of the view is under.
    fn current_heading(&self) -> Option<usize> {
        self.doc
            .headings
            .iter()
            .rposition(|h| h.line <= self.scroll)
    }

    /// Whether the reader should close.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        let last = self
            .doc
            .lines
            .len()
            .saturating_sub(self.height.min(self.doc.lines.len()));
        let page = self.height.max(2) - 1;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.scroll += page / 2,
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(page / 2),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll += page,
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = last,
            KeyCode::Char('n') => {
                if let Some(h) = self.doc.headings.iter().find(|h| h.line > self.scroll) {
                    self.scroll = h.line;
                }
            }
            KeyCode::Char('N') => {
                if let Some(h) = self
                    .doc
                    .headings
                    .iter()
                    .rev()
                    .find(|h| h.line < self.scroll)
                {
                    self.scroll = h.line;
                }
            }
            _ => {}
        }
        self.scroll = self.scroll.min(last);
        false
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        let pct = if self.doc.lines.len() <= self.height {
            100
        } else {
            (self.scroll + self.height) * 100 / self.doc.lines.len()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(FOCUS))
            .title(Span::styled(
                format!(" md · {} ", self.title),
                Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
            ))
            .title(
                Line::from(Span::styled(
                    format!(" {}% ", pct.min(100)),
                    Style::default().fg(DIM),
                ))
                .right_aligned(),
            );
        let inner = block.inner(area);
        f.render_widget(block, area);

        let toc_w = if inner.width >= TOC_FROM && !self.doc.headings.is_empty() {
            TOC
        } else {
            0
        };
        let text = Rect {
            x: inner.x + toc_w + u16::from(toc_w > 0) + 1,
            width: inner
                .width
                .saturating_sub(toc_w + u16::from(toc_w > 0) + 2)
                .min(100),
            ..inner
        };
        if text.width != self.width {
            self.width = text.width;
            self.doc = md::render(&self.src, text.width);
        }
        self.height = text.height as usize;
        let last = self
            .doc
            .lines
            .len()
            .saturating_sub(self.height.min(self.doc.lines.len()));
        self.scroll = self.scroll.min(last);

        if toc_w > 0 {
            let toc = Rect {
                width: toc_w,
                ..inner
            };
            self.draw_toc(f, toc);
            for y in inner.y..inner.y + inner.height {
                if let Some(c) = f.buffer_mut().cell_mut((inner.x + toc_w, y)) {
                    c.set_symbol("│").set_style(Style::default().fg(LINE));
                }
            }
        }
        let lines: Vec<Line> = self
            .doc
            .lines
            .iter()
            .skip(self.scroll)
            .take(self.height)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(lines), text);
    }

    fn draw_toc(&self, f: &mut Frame, area: Rect) {
        let current = self.current_heading();
        let mut lines = vec![Line::from(Span::styled(
            " Contents",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        ))];
        let room = area.height.saturating_sub(1) as usize;
        let first = current.unwrap_or(0).saturating_sub(room.saturating_sub(1));
        for (i, h) in self.doc.headings.iter().enumerate().skip(first).take(room) {
            let on = Some(i) == current;
            let indent = "  ".repeat(usize::from(h.level.saturating_sub(1)).min(3));
            let w = (area.width as usize).saturating_sub(indent.len() + 2);
            let text: String = if h.text.chars().count() > w {
                h.text
                    .chars()
                    .take(w.saturating_sub(1))
                    .chain(['…'])
                    .collect()
            } else {
                h.text.clone()
            };
            let mut line = Line::from(Span::styled(
                format!(" {indent}{text}"),
                if on {
                    Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(if h.level <= 2 { FG } else { DIM })
                },
            ));
            if on {
                line = line.style(Style::default().bg(PANEL));
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), area);
    }
}
