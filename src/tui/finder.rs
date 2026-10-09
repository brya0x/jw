//! The pickers (docs/specs/rust-tui.md, Interfaces → Pickers): one modal
//! with a query, a list and a preview, and one fuzzy scorer. `^␣ o` browses
//! folders one directory at a time.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::theme::p;

/// How well `query` matches `text`, and where: every query character must
/// appear in order, case aside. Consecutive matches and matches at the start
/// of a word score more; shorter texts win ties.
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut at = Vec::new();
    let mut score = 0;
    let mut i = 0;
    for q in query.chars().map(|c| c.to_ascii_lowercase()) {
        while i < chars.len() && chars[i].to_ascii_lowercase() != q {
            i += 1;
        }
        if i == chars.len() {
            return None;
        }
        score += 1;
        if at.last() == Some(&(i.wrapping_sub(1))) {
            score += 3;
        }
        if i == 0 || matches!(chars[i - 1], '/' | '-' | '_' | '.' | ' ') {
            score += 2;
        }
        at.push(i);
        i += 1;
    }
    Some((score * 100 - chars.len() as i32, at))
}

/// What picking a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Open this folder as a workspace.
    Dir(PathBuf),
    /// Make this folder, then open it.
    Create(PathBuf),
}

#[derive(Debug, Clone)]
struct Item {
    mark: &'static str,
    label: String,
    hint: String,
    pick: Pick,
    /// A folder the browser can go into.
    into: Option<PathBuf>,
}

/// What a key did.
pub enum Outcome {
    Stay,
    Cancel,
    Pick(Pick),
}

pub struct Finder {
    title: String,
    cwd: PathBuf,
    /// Folders already open as workspaces, marked `●`.
    open: HashSet<PathBuf>,
    query: String,
    sel: usize,
    items: Vec<Item>,
    /// The rows shown: an index into `items` and the matched characters.
    shown: Vec<(usize, Vec<usize>)>,
}

impl Finder {
    /// The folder browser, starting in `cwd`.
    pub fn folders(cwd: PathBuf, open: HashSet<PathBuf>) -> Self {
        let mut f = Self {
            title: "Open a folder".into(),
            cwd,
            open,
            query: String::new(),
            sel: 0,
            items: Vec::new(),
            shown: Vec::new(),
        };
        f.list();
        f
    }

    fn list(&mut self) {
        self.items.clear();
        let here = self.cwd.clone();
        self.items.push(Item {
            mark: ".",
            label: "open this folder".into(),
            hint: String::new(),
            pick: Pick::Dir(here.clone()),
            into: None,
        });
        for name in subdirs(&here) {
            let path = here.join(&name);
            let git = path.join(".git").exists();
            self.items.push(Item {
                mark: if self.open.contains(&path) {
                    "●"
                } else {
                    "▸"
                },
                label: format!("{name}/"),
                hint: if git { "git".into() } else { String::new() },
                pick: Pick::Dir(path.clone()),
                into: Some(path),
            });
        }
        self.filter();
    }

    fn filter(&mut self) {
        let q = self.query.trim();
        let mut shown: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| q.is_empty() || it.into.is_some())
            .filter_map(|(i, it)| {
                let (score, at) = fuzzy(q, &it.label)?;
                Some((if q.is_empty() { -(i as i32) } else { score }, i, at))
            })
            .collect();
        shown.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.shown = shown.into_iter().map(|(_, i, at)| (i, at)).collect();
        let exists = self
            .items
            .iter()
            .any(|it| it.label.trim_end_matches('/') == q);
        if !q.is_empty() && !exists && crate::actions::valid_name(q) {
            self.items.push(Item {
                mark: "+",
                label: format!("create {q}/"),
                hint: "new".into(),
                pick: Pick::Create(self.cwd.join(q)),
                into: None,
            });
            self.shown.push((self.items.len() - 1, Vec::new()));
        }
        self.sel = self.sel.min(self.shown.len().saturating_sub(1));
    }

    fn current(&self) -> Option<&Item> {
        self.shown.get(self.sel).map(|(i, _)| &self.items[*i])
    }

    fn go(&mut self, dir: PathBuf, select: Option<PathBuf>) {
        self.cwd = dir;
        self.query.clear();
        self.sel = 0;
        self.list();
        if let Some(s) = select
            && let Some(i) = self
                .shown
                .iter()
                .position(|(i, _)| self.items[*i].into.as_ref() == Some(&s))
        {
            self.sel = i;
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => {
                if let Some(it) = self.current() {
                    return Outcome::Pick(it.pick.clone());
                }
            }
            KeyCode::Down => self.sel = (self.sel + 1).min(self.shown.len().saturating_sub(1)),
            KeyCode::Char('n') if ctrl => {
                self.sel = (self.sel + 1).min(self.shown.len().saturating_sub(1))
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => {
                if let Some(dir) = self.current().and_then(|it| it.into.clone()) {
                    self.go(dir, None);
                }
            }
            KeyCode::Left => self.up(),
            KeyCode::Backspace if self.query.is_empty() => self.up(),
            KeyCode::Backspace => {
                self.query.pop();
                self.list();
            }
            KeyCode::Char('~') if self.query.is_empty() => {
                if let Some(home) = std::env::var_os("HOME") {
                    self.go(PathBuf::from(home), None);
                }
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.list();
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.sel = 0;
                self.list();
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn up(&mut self) {
        let from = self.cwd.clone();
        if let Some(parent) = from.parent() {
            self.go(parent.to_path_buf(), Some(from));
        }
    }

    pub fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let w = 100.min(area.width.saturating_sub(4));
        let h = 24.min(area.height.saturating_sub(2));
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height.saturating_sub(h) / 3,
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(p().blue))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(p().panel).fg(p().fg));
        let inner = block.inner(r);
        f.render_widget(block, r);
        if inner.height < 5 {
            return;
        }

        let dim = Style::default().fg(p().dim);
        let path = tilde(&self.cwd);
        let mut crumbs = Vec::new();
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 || path.starts_with('/') {
                crumbs.push(Span::styled(" / ", dim));
            }
            crumbs.push(if i + 1 == parts.len() {
                Span::styled(
                    part.to_string(),
                    Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(part.to_string())
            });
        }
        let top = Rect { height: 1, ..inner };
        f.render_widget(
            Paragraph::new(Line::from(crumbs)),
            Rect {
                x: top.x + 1,
                ..top
            },
        );
        let input = Line::from(vec![
            Span::styled(" › ", Style::default().fg(p().blue)),
            if self.query.is_empty() {
                Span::styled("type to filter this folder…", dim)
            } else {
                Span::raw(self.query.clone())
            },
            Span::styled("▏", Style::default().fg(p().blue)),
        ]);
        f.render_widget(
            Paragraph::new(input),
            Rect {
                y: top.y + 1,
                ..top
            },
        );

        let body = Rect {
            y: inner.y + 3,
            height: inner.height.saturating_sub(4),
            ..inner
        };
        let list_w = body.width * 45 / 100;
        let list = Rect {
            width: list_w,
            ..body
        };
        let prev = Rect {
            x: body.x + list_w + 1,
            width: body.width.saturating_sub(list_w + 1),
            ..body
        };
        let rows = list.height as usize;
        let first = self.sel.saturating_sub(rows.saturating_sub(1));
        let mut lines = Vec::new();
        for (n, (i, at)) in self.shown.iter().enumerate().skip(first).take(rows) {
            let it = &self.items[*i];
            let on = n == self.sel;
            let mut spans = vec![Span::styled(format!(" {} ", it.mark), dim)];
            for (ci, c) in it.label.chars().enumerate() {
                let hit = at.contains(&ci);
                spans.push(Span::styled(
                    c.to_string(),
                    if hit {
                        Style::default().fg(p().yellow).add_modifier(Modifier::BOLD)
                    } else if on {
                        Style::default().fg(p().blue)
                    } else {
                        Style::default()
                    },
                ));
            }
            if !it.hint.is_empty() {
                spans.push(Span::styled(format!("  {}", it.hint), dim));
            }
            let mut line = Line::from(spans);
            if on {
                line = line.style(Style::default().bg(p().sel));
            }
            lines.push(line);
        }
        if self.shown.is_empty() {
            lines.push(Line::from(Span::styled(" Nothing matches.", dim)));
        }
        f.render_widget(Paragraph::new(lines), list);
        for y in body.y..body.y + body.height {
            if let Some(c) = f.buffer_mut().cell_mut((prev.x.saturating_sub(1), y)) {
                c.set_symbol("│").set_style(Style::default().fg(p().line));
            }
        }
        f.render_widget(Paragraph::new(self.preview()), prev);

        let foot = Line::from(vec![
            Span::styled(
                " ↑↓",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" move  ", dim),
            Span::styled(
                "→",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" go in  ", dim),
            Span::styled(
                "←",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" go up  ", dim),
            Span::styled(
                "~",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" home  ", dim),
            Span::styled(
                "↵",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" open as a workspace  ", dim),
            Span::styled(
                "esc",
                Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" close", dim),
        ]);
        f.render_widget(
            Paragraph::new(foot),
            Rect {
                y: inner.bottom().saturating_sub(1),
                height: 1,
                ..inner
            },
        );
    }

    fn preview(&self) -> Vec<Line<'static>> {
        let dim = Style::default().fg(p().dim);
        let Some(it) = self.current() else {
            return Vec::new();
        };
        let (dir, new) = match &it.pick {
            Pick::Dir(d) => (d.clone(), false),
            Pick::Create(d) => (d.clone(), true),
        };
        let mut l = vec![Line::from(Span::styled(
            tilde(&dir),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        if new {
            l.push(Line::default());
            l.push(Line::from(Span::styled(
                "A new, empty folder. It opens with one shell.",
                dim,
            )));
            return l;
        }
        let git = crate::folders::repo(&dir).is_some();
        l.push(Line::from(if git {
            Span::styled("git repository", Style::default().fg(p().green))
        } else {
            Span::styled("not a git repository", dim)
        }));
        if self.open.contains(&dir) {
            l.push(Line::from(Span::styled(
                "● open: ↵ goes there",
                Style::default().fg(p().blue),
            )));
        }
        l.push(Line::default());
        let mut names: Vec<String> = subdirs(&dir).into_iter().map(|d| d + "/").collect();
        names.extend(files(&dir));
        if names.is_empty() {
            l.push(Line::from(Span::styled("empty", dim)));
        }
        for n in names.into_iter().take(14) {
            l.push(Line::from(Span::raw(n)));
        }
        l
    }
}

/// The folders in `dir`, without hidden ones, sorted by name.
pub fn subdirs(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.') && e.path().is_dir()).then_some(name)
        })
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

fn files(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.') && !e.path().is_dir()).then_some(name)
        })
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// A path with the home directory as `~`.
pub fn tilde(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if home.len() > 1 && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn fuzzy_ranks_tight_and_word_start_matches_first() {
        assert!(fuzzy("xyz", "kanvas").is_none());
        let (_, at) = fuzzy("kv", "kanvas").unwrap();
        assert_eq!(at, [0, 3]);
        let mut names = ["mctekk-web", "kanvas", "jitsubai", "kanvas-old"];
        names.sort_by_key(|n| -fuzzy("kan", n).map_or(-1000, |s| s.0));
        assert_eq!(names[..2], ["kanvas", "kanvas-old"]);
        assert!(fuzzy("web", "mctekk-web").unwrap().0 > fuzzy("web", "wxexb").unwrap().0);
    }

    #[test]
    fn the_browser_lists_goes_in_and_up_and_offers_create() {
        let d = tempfile::tempdir().unwrap();
        for name in ["beta", "alpha", ".hidden"] {
            std::fs::create_dir(d.path().join(name)).unwrap();
        }
        std::fs::write(d.path().join("file.txt"), "").unwrap();
        let root = d.path().to_path_buf();
        let mut f = Finder::folders(root.clone(), HashSet::new());
        let labels: Vec<&str> = f
            .shown
            .iter()
            .map(|(i, _)| f.items[*i].label.as_str())
            .collect();
        assert_eq!(labels, ["open this folder", "alpha/", "beta/"]);

        f.key(key(KeyCode::Down));
        f.key(key(KeyCode::Right));
        assert_eq!(f.cwd, root.join("alpha"));
        f.key(key(KeyCode::Left));
        assert_eq!(f.cwd, root);
        assert_eq!(
            f.current().unwrap().label,
            "alpha/",
            "back on the folder it left"
        );

        for c in "gamma".chars() {
            f.key(key(KeyCode::Char(c)));
        }
        match f.key(key(KeyCode::Enter)) {
            Outcome::Pick(Pick::Create(p)) => assert_eq!(p, root.join("gamma")),
            _ => panic!("want create"),
        }
        f.key(key(KeyCode::Backspace));
        for _ in 0..4 {
            f.key(key(KeyCode::Backspace));
        }
        for c in "bet".chars() {
            f.key(key(KeyCode::Char(c)));
        }
        match f.key(key(KeyCode::Enter)) {
            Outcome::Pick(Pick::Dir(p)) => assert_eq!(p, root.join("beta")),
            _ => panic!("want beta"),
        }
    }
}
