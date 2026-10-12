//! The pickers (docs/specs/rust-tui.md, Interfaces → Pickers): one modal
//! with a query, a list and a preview, and one fuzzy scorer. `^␣ o` browses
//! folders one directory at a time; `^␣ ␣` lists the workspaces and `^␣ /`
//! the files of the current one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::anim;
use crate::connectors::github::PrSummary;
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
    /// Go to this workspace, by id.
    Ws(String),
    /// Open this file of the current workspace, by its path in it.
    File(String),
    /// Go to this session (REQ-63).
    Session(String),
    /// Start a session with this name in the current workspace's folder.
    NewSession(String),
    /// Ask for a new name for this session.
    RenameSession(String),
    /// `ctrl-d` in the session picker (REQ-276).
    DeleteSession(String),
    /// Review this pull request (REQ-165).
    Review(Box<PrSummary>),
}

/// One workspace for the switcher, in the order to list it (most recent
/// first).
pub struct WsItem {
    pub id: String,
    pub label: String,
    pub open: bool,
    pub hint: String,
    pub preview: Vec<Line<'static>>,
}

enum Kind {
    Folders {
        cwd: PathBuf,
        /// Folders already open as workspaces, marked `●`.
        open: HashSet<PathBuf>,
    },
    Workspaces,
    Files {
        root: PathBuf,
    },
    Sessions,
    /// The open pull requests; `loading` until gh answers (REQ-161).
    Reviews {
        loading: bool,
        /// Only the PRs whose review is asked of the user (REQ-179).
        requested: bool,
    },
}

#[derive(Debug, Clone)]
struct Item {
    mark: &'static str,
    label: String,
    hint: String,
    pick: Pick,
    /// A folder the browser can go into.
    into: Option<PathBuf>,
    /// What the right side shows, when the item brings it.
    preview: Vec<Line<'static>>,
}

/// What a key did.
pub enum Outcome {
    Stay,
    Cancel,
    Pick(Pick),
    /// `m` in the PR picker: load the list again, `true` for the PRs
    /// requested of the user (REQ-179).
    Reload(bool),
    /// `o` in the PR picker: open this url in the browser (REQ-180).
    Browse(String),
}

pub struct Finder {
    title: String,
    kind: Kind,
    query: String,
    sel: usize,
    items: Vec<Item>,
    /// The rows shown: an index into `items` and the matched characters.
    shown: Vec<(usize, Vec<usize>)>,
    /// The highlighted row on its way to `sel` (REQ-144).
    band: anim::Tween,
    band_to: usize,
}

impl Finder {
    /// The folder browser, starting in `cwd`.
    pub fn folders(cwd: PathBuf, open: HashSet<PathBuf>) -> Self {
        Self::new("Open a folder", Kind::Folders { cwd, open }, Vec::new())
    }

    /// The switcher (REQ-41): every workspace, most recent first.
    pub fn workspaces(list: Vec<WsItem>) -> Self {
        let items = list
            .into_iter()
            .map(|w| Item {
                mark: if w.open { "●" } else { "○" },
                label: w.label,
                hint: w.hint,
                pick: Pick::Ws(w.id),
                into: None,
                preview: w.preview,
            })
            .collect();
        Self::new("Switch workspace", Kind::Workspaces, items)
    }

    /// Every session, the one on screen last; `id` is the session's name.
    pub fn sessions(list: Vec<WsItem>) -> Self {
        let items = list
            .into_iter()
            .map(|s| Item {
                mark: if s.open { "●" } else { "○" },
                label: s.label,
                hint: s.hint,
                pick: Pick::Session(s.id),
                into: None,
                preview: s.preview,
            })
            .collect();
        Self::new("Sessions", Kind::Sessions, items)
    }

    /// The files of the workspace at `root` (REQ-42).
    pub fn files(title: String, root: PathBuf, files: Vec<String>) -> Self {
        let items = files
            .into_iter()
            .map(|f| Item {
                mark: if f.ends_with(".md") { "¶" } else { " " },
                hint: if f.ends_with(".md") {
                    "rendered".into()
                } else {
                    "nvim".into()
                },
                pick: Pick::File(f.clone()),
                label: f,
                into: None,
                preview: Vec::new(),
            })
            .collect();
        Self::new(&title, Kind::Files { root }, items)
    }

    /// The PR picker, loading until `set_reviews` brings the list (REQ-160).
    pub fn reviews(requested: bool) -> Self {
        let mut f = Self::new(
            "Review a pull request",
            Kind::Reviews {
                loading: true,
                requested,
            },
            Vec::new(),
        );
        f.retitle();
        f
    }

    /// Whether this is the PR picker and gh has not answered yet.
    pub fn loading_reviews(&self) -> bool {
        matches!(self.kind, Kind::Reviews { loading: true, .. })
    }

    /// Fills the PR picker. `false` when the list is not the one it asked
    /// for (the picker closed and another opened, or `m` was pressed since).
    pub fn set_reviews(&mut self, prs: Vec<PrSummary>, requested: bool) -> bool {
        let Kind::Reviews {
            loading,
            requested: asked,
        } = &mut self.kind
        else {
            return false;
        };
        if *asked != requested {
            return false;
        }
        *loading = false;
        self.items = prs.into_iter().map(review_item).collect();
        self.sel = 0;
        self.filter();
        true
    }

    fn retitle(&mut self) {
        if let Kind::Reviews { requested, .. } = self.kind {
            self.title = if requested {
                "Review a pull request · requested of me".into()
            } else {
                "Review a pull request".into()
            };
        }
    }

    fn new(title: &str, kind: Kind, items: Vec<Item>) -> Self {
        let mut f = Self {
            title: title.into(),
            kind,
            query: String::new(),
            sel: 0,
            items,
            shown: Vec::new(),
            band: anim::Tween::at(0.0, Instant::now()),
            band_to: 0,
        };
        f.list();
        f
    }

    /// The folder browser lists its directory; the other pickers keep their
    /// items and only filter. Typing only filters.
    fn list(&mut self) {
        let Kind::Folders { cwd, open } = &self.kind else {
            self.filter();
            return;
        };
        self.items.clear();
        let here = cwd.clone();
        self.items.push(Item {
            mark: ".",
            label: "open this folder".into(),
            hint: String::new(),
            pick: Pick::Dir(here.clone()),
            into: None,
            preview: Vec::new(),
        });
        for name in subdirs(&here) {
            let path = here.join(&name);
            let git = path.join(".git").exists();
            self.items.push(Item {
                mark: if open.contains(&path) { "●" } else { "▸" },
                label: format!("{name}/"),
                hint: if git { "git".into() } else { String::new() },
                pick: Pick::Dir(path.clone()),
                into: Some(path),
                preview: Vec::new(),
            });
        }
        self.filter();
    }

    fn filter(&mut self) {
        let q = self.query.trim();
        let folders = matches!(self.kind, Kind::Folders { .. });
        let reviews = self.reviewing();
        self.items
            .retain(|it| !matches!(it.pick, Pick::Create(_) | Pick::NewSession(_)));
        let mut shown: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| q.is_empty() || !folders || it.into.is_some())
            .filter_map(|(i, it)| {
                // A PR is found by number, title, author or branch (REQ-163);
                // only the label is highlighted.
                let text = if reviews {
                    format!("{} {}", it.label, it.hint)
                } else {
                    it.label.clone()
                };
                let (score, at) = fuzzy(q, &text)?;
                Some((if q.is_empty() { -(i as i32) } else { score }, i, at))
            })
            .collect();
        shown.sort_by_key(|s| std::cmp::Reverse(s.0));
        self.shown = shown.into_iter().map(|(_, i, at)| (i, at)).collect();
        let exists = self
            .items
            .iter()
            .any(|it| it.label.trim_end_matches('/') == q);
        if let Kind::Folders { cwd, .. } = &self.kind
            && !q.is_empty()
            && !exists
            && crate::actions::valid_name(q)
        {
            self.items.push(Item {
                mark: "+",
                label: format!("create {q}/"),
                hint: "new".into(),
                pick: Pick::Create(cwd.join(q)),
                into: None,
                preview: Vec::new(),
            });
            self.shown.push((self.items.len() - 1, Vec::new()));
        }
        if matches!(self.kind, Kind::Sessions) && !exists && crate::session::valid(q) {
            self.items.push(Item {
                mark: "+",
                label: format!("create {q}"),
                hint: "new".into(),
                pick: Pick::NewSession(q.to_string()),
                into: None,
                preview: vec![
                    Line::from(Span::styled(
                        format!("new session {q}"),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Starts with one workspace in this folder,",
                        Style::default().fg(p().dim),
                    )),
                    Line::from(Span::styled(
                        "holding one shell, no layout.",
                        Style::default().fg(p().dim),
                    )),
                ],
            });
            self.shown.push((self.items.len() - 1, Vec::new()));
        }
        self.sel = self.sel.min(self.shown.len().saturating_sub(1));
    }

    fn current(&self) -> Option<&Item> {
        self.shown.get(self.sel).map(|(i, _)| &self.items[*i])
    }

    fn go(&mut self, dir: PathBuf, select: Option<PathBuf>) {
        let Kind::Folders { cwd, .. } = &mut self.kind else {
            return;
        };
        *cwd = dir;
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
            KeyCode::Char('r') if ctrl && matches!(self.kind, Kind::Sessions) => {
                if let Some(Pick::Session(name)) = self.current().map(|it| it.pick.clone()) {
                    return Outcome::Pick(Pick::RenameSession(name));
                }
            }
            KeyCode::Char('d') if ctrl && matches!(self.kind, Kind::Sessions) => {
                if let Some(Pick::Session(name)) = self.current().map(|it| it.pick.clone()) {
                    return Outcome::Pick(Pick::DeleteSession(name));
                }
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => {
                if let Some(dir) = self.current().and_then(|it| it.into.clone()) {
                    self.go(dir, None);
                }
            }
            KeyCode::Left if self.browsing() => self.up(),
            KeyCode::Backspace if self.query.is_empty() && self.browsing() => self.up(),
            KeyCode::Backspace => {
                self.query.pop();
                self.filter();
            }
            KeyCode::Char('~') if self.query.is_empty() && self.browsing() => {
                if let Some(home) = std::env::var_os("HOME") {
                    self.go(PathBuf::from(home), None);
                }
            }
            KeyCode::Char('m') if !ctrl && self.query.is_empty() && self.reviewing() => {
                if let Kind::Reviews { loading, requested } = &mut self.kind {
                    *requested = !*requested;
                    *loading = true;
                    let now = *requested;
                    self.items.clear();
                    self.sel = 0;
                    self.retitle();
                    self.filter();
                    return Outcome::Reload(now);
                }
            }
            KeyCode::Char('o') if !ctrl && self.query.is_empty() && self.reviewing() => {
                if let Some(Pick::Review(pr)) = self.current().map(|it| it.pick.clone()) {
                    return Outcome::Browse(pr.url);
                }
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.filter();
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.sel = 0;
                self.filter();
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn reviewing(&self) -> bool {
        matches!(self.kind, Kind::Reviews { .. })
    }

    fn browsing(&self) -> bool {
        matches!(self.kind, Kind::Folders { .. })
    }

    fn up(&mut self) {
        let Kind::Folders { cwd, .. } = &self.kind else {
            return;
        };
        let from = cwd.clone();
        if let Some(parent) = from.parent() {
            self.go(parent.to_path_buf(), Some(from));
        }
    }

    /// The box, its list and its preview, for a terminal of `area`.
    fn areas(area: Rect) -> (Rect, Rect, Rect) {
        let w = 100.min(area.width.saturating_sub(4));
        let h = 24.min(area.height.saturating_sub(2));
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height.saturating_sub(h) / 3,
            w,
            h,
        );
        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
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
        (r, list, prev)
    }

    /// The first row the list shows, keeping the selection in sight.
    fn first(&self, rows: usize) -> usize {
        self.sel.saturating_sub(rows.saturating_sub(1))
    }

    /// The wheel moves the selection.
    pub fn move_by(&mut self, by: isize) {
        self.sel = self
            .sel
            .saturating_add_signed(by)
            .min(self.shown.len().saturating_sub(1));
    }

    /// A click on a row picks it; anywhere else does nothing.
    pub fn click(&mut self, x: u16, y: u16, area: Rect) -> Option<Pick> {
        let (_, list, _) = Self::areas(area);
        if x < list.x || x >= list.x + list.width || y < list.y || y >= list.y + list.height {
            return None;
        }
        let i = self.first(list.height as usize) + usize::from(y - list.y);
        let (item, _) = self.shown.get(i)?;
        self.sel = i;
        Some(self.items[*item].pick.clone())
    }

    /// Moves the highlighted row toward the selection, sliding when
    /// animations are on (REQ-144). Whether it is still moving.
    pub fn glide(&mut self, now: Instant, animate: bool) -> bool {
        if self.band_to != self.sel {
            self.band_to = self.sel;
            if animate {
                self.band.retarget(self.sel as f32, anim::PICK_SLIDE, now);
            } else {
                self.band = anim::Tween::at(self.sel as f32, now);
            }
        }
        !self.band.done(now)
    }

    /// The row the highlight is on at `now`.
    fn band_row(&self, now: Instant) -> usize {
        self.band.value(now).round().max(0.0) as usize
    }

    pub fn draw(&self, f: &mut Frame) {
        let now = Instant::now();
        let area = f.area();
        let (r, list, prev) = Self::areas(area);
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
        let path = match &self.kind {
            Kind::Folders { cwd, .. } => tilde(cwd),
            Kind::Files { root } => tilde(root),
            Kind::Workspaces | Kind::Sessions | Kind::Reviews { .. } => String::new(),
        };
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
                Span::styled(
                    match self.kind {
                        Kind::Folders { .. } => "type to filter this folder…",
                        Kind::Workspaces => "project or worktree…",
                        Kind::Files { .. } => "file name…",
                        Kind::Sessions => "session name, or a new one…",
                        Kind::Reviews { .. } => "number, title, author or branch…",
                    },
                    dim,
                )
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
        let rows = list.height as usize;
        let first = self.first(rows);
        let mut lines = Vec::new();
        for (n, (i, at)) in self.shown.iter().enumerate().skip(first).take(rows) {
            let it = &self.items[*i];
            let on = n == self.sel;
            let band = n == self.band_row(now);
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
            if band {
                line = line.style(Style::default().bg(p().sel));
            }
            lines.push(line);
        }
        if self.shown.is_empty() {
            lines.push(Line::from(Span::styled(
                match self.kind {
                    Kind::Reviews { loading: true, .. } => " Loading open pull requests…",
                    Kind::Reviews { requested, .. } if self.query.is_empty() => {
                        if requested {
                            " No review is requested of you."
                        } else {
                            " No open pull requests."
                        }
                    }
                    _ => " Nothing matches.",
                },
                dim,
            )));
        }
        f.render_widget(Paragraph::new(lines), list);
        for y in body.y..body.y + body.height {
            if let Some(c) = f.buffer_mut().cell_mut((prev.x.saturating_sub(1), y)) {
                c.set_symbol("│").set_style(Style::default().fg(p().line));
            }
        }
        f.render_widget(Paragraph::new(self.preview()), prev);

        let bold = Style::default().fg(p().fg).add_modifier(Modifier::BOLD);
        let pairs: &[(&str, &str)] = match self.kind {
            Kind::Folders { .. } => &[
                ("↑↓", "move"),
                ("→", "go in"),
                ("←", "go up"),
                ("~", "home"),
                ("↵", "open as a workspace"),
                ("esc", "close"),
            ],
            Kind::Sessions => &[
                ("↑↓", "move"),
                ("↵", "open"),
                ("ctrl-r", "rename"),
                ("ctrl-d", "delete"),
                ("esc", "close"),
            ],
            Kind::Reviews { requested, .. } => &[
                ("↑↓", "move"),
                ("↵", "review"),
                (
                    "m",
                    if requested {
                        "all open"
                    } else {
                        "requested of me"
                    },
                ),
                ("o", "browser"),
                ("esc", "close"),
            ],
            _ => &[("↑↓", "move"), ("↵", "open"), ("esc", "close")],
        };
        let mut foot = Vec::new();
        for (k, what) in pairs {
            foot.push(Span::styled(format!(" {k}"), bold));
            foot.push(Span::styled(format!(" {what} "), dim));
        }
        let foot = Line::from(foot);
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
        if !it.preview.is_empty() {
            return it.preview.clone();
        }
        let (dir, new) = match &it.pick {
            Pick::Dir(d) => (d.clone(), false),
            Pick::Create(d) => (d.clone(), true),
            Pick::File(f) => {
                let Kind::Files { root } = &self.kind else {
                    return Vec::new();
                };
                return file_head(&root.join(f));
            }
            Pick::Ws(_)
            | Pick::Session(_)
            | Pick::NewSession(_)
            | Pick::RenameSession(_)
            | Pick::DeleteSession(_)
            | Pick::Review(_) => {
                return Vec::new();
            }
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
        // What the list's `git` says: drawn every frame, so no git here.
        let git = dir.join(".git").exists();
        l.push(Line::from(if git {
            Span::styled("git repository", Style::default().fg(p().green))
        } else {
            Span::styled("not a git repository", dim)
        }));
        if matches!(&self.kind, Kind::Folders { open, .. } if open.contains(&dir)) {
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

/// One open PR as a row, with its preview: the branches, the size, the
/// description and the files (REQ-164). Built once, not every frame.
fn review_item(pr: PrSummary) -> Item {
    let dim = Style::default().fg(p().dim);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mark = if pr.is_draft {
        "◌"
    } else {
        match pr.review.as_str() {
            "APPROVED" => "✓",
            "CHANGES_REQUESTED" => "±",
            _ => " ",
        }
    };
    let mut size = vec![
        Span::styled(format!("+{}", pr.additions), Style::default().fg(p().green)),
        Span::raw(" "),
        Span::styled(format!("−{}", pr.deletions), Style::default().fg(p().red)),
        Span::styled(format!(" in {} files", pr.files), dim),
    ];
    if pr.is_draft {
        size.push(Span::styled("  draft", Style::default().fg(p().yellow)));
    }
    let when = pr.updated_at.get(..10).unwrap_or_default();
    let mut preview = vec![
        Line::from(Span::styled(format!("#{} {}", pr.number, pr.title), bold)),
        Line::from(vec![
            Span::styled(pr.branch.clone(), Style::default().fg(p().magenta)),
            Span::styled(format!(" → {} · {when}", pr.base), dim),
        ]),
        Line::from(size),
    ];
    let verdict = match pr.review.as_str() {
        "APPROVED" => Some(("approved", p().green)),
        "CHANGES_REQUESTED" => Some(("changes requested", p().red)),
        _ => None,
    };
    if let Some((text, colour)) = verdict {
        preview.push(Line::from(Span::styled(text, Style::default().fg(colour))));
    }
    preview.push(Line::default());
    if pr.body.trim().is_empty() {
        preview.push(Line::from(Span::styled("no description", dim)));
    }
    preview.extend(
        pr.body
            .lines()
            .take(12)
            .map(|l| Line::from(l.replace('\t', "    "))),
    );
    if !pr.paths.is_empty() {
        preview.push(Line::default());
        preview.push(Line::from(Span::styled("files", dim)));
        preview.extend(
            pr.paths
                .iter()
                .take(10)
                .map(|f| Line::from(format!("  {f}"))),
        );
        if pr.paths.len() > 10 {
            preview.push(Line::from(Span::styled(
                format!("  +{} more", pr.paths.len() - 10),
                dim,
            )));
        }
    }
    Item {
        mark,
        label: format!("#{} {}", pr.number, pr.title),
        hint: format!("@{}  {}", pr.author, pr.branch),
        pick: Pick::Review(Box::new(pr)),
        into: None,
        preview,
    }
}

/// The first lines of a text file, for the preview. Only its start is read:
/// the preview is drawn every frame.
fn file_head(path: &Path) -> Vec<Line<'static>> {
    use std::io::Read;
    let dim = Style::default().fg(p().dim);
    let read = std::fs::File::open(path).and_then(|f| {
        let mut bytes = Vec::new();
        f.take(64 * 1024).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    match read {
        Ok(bytes) if bytes.iter().take(4096).any(|b| *b == 0) => {
            vec![Line::from(Span::styled("binary file", dim))]
        }
        Ok(bytes) => String::from_utf8_lossy(&bytes)
            .lines()
            .take(30)
            .map(|l| Line::from(l.replace('\t', "    ")))
            .collect(),
        Err(e) => vec![Line::from(Span::styled(e.to_string(), dim))],
    }
}

/// A workspace's files for `^␣ /`: what git tracks or would (untracked,
/// not ignored), else a walk that skips hidden folders, capped at `cap`.
pub fn list_files(root: &Path, cap: usize) -> Vec<String> {
    let git = std::process::Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard"])
        .current_dir(root)
        .output();
    if let Ok(out) = git
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .lines()
            .take(cap)
            .map(str::to_string)
            .collect();
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            // Not through symlinks, as git doesn't: a link to a parent loops.
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.display().to_string());
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
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
        Ok(home) => tilde_in(s, &home),
        Err(_) => s,
    }
}

/// `s` with `home` as `~` when it is `home` or under it: `/home/bob` is not
/// under `/home/bo`.
fn tilde_in(s: String, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match s.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
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
        assert!(matches!(&f.kind, Kind::Folders { cwd, .. } if *cwd == root.join("alpha")));
        f.key(key(KeyCode::Left));
        assert!(matches!(&f.kind, Kind::Folders { cwd, .. } if *cwd == root));
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

    #[test]
    fn tilde_takes_home_only_as_a_whole_folder() {
        let t = |s: &str, home: &str| tilde_in(s.to_string(), home);
        assert_eq!(t("/home/bo/x", "/home/bo"), "~/x");
        assert_eq!(t("/home/bo", "/home/bo/"), "~");
        assert_eq!(t("/home/bob/x", "/home/bo"), "/home/bob/x");
        assert_eq!(t("/x", "/"), "/x");
        assert_eq!(t("/x", ""), "/x");
    }

    #[test]
    fn the_preview_reads_a_repo_without_git() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("repo")).unwrap();
        std::fs::write(d.path().join("repo/.git"), "gitdir: /elsewhere").unwrap();
        let mut f = Finder::folders(d.path().to_path_buf(), HashSet::new());
        f.key(key(KeyCode::Down));
        let text: Vec<String> = f.preview().iter().map(|l| l.to_string()).collect();
        assert_eq!(text[1], "git repository", "{text:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_walk_does_not_follow_a_link_back_up() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("a")).unwrap();
        std::os::unix::fs::symlink(d.path(), d.path().join("a/up")).unwrap();
        std::fs::write(d.path().join("a/f"), "").unwrap();
        let mut got = list_files(d.path(), 100);
        got.sort();
        assert_eq!(got, ["a/f", "a/up"]);
    }

    #[test]
    fn the_file_list_walks_when_there_is_no_git() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("docs")).unwrap();
        std::fs::create_dir_all(d.path().join(".hidden")).unwrap();
        std::fs::write(d.path().join("docs/a.md"), "# a").unwrap();
        std::fs::write(d.path().join("b.rs"), "").unwrap();
        std::fs::write(d.path().join(".hidden/c"), "").unwrap();
        let mut got = list_files(d.path(), 100);
        got.sort();
        assert_eq!(got, ["b.rs", "docs/a.md"]);
        assert_eq!(list_files(d.path(), 1).len(), 1);

        let mut f = Finder::files("t".into(), d.path().to_path_buf(), got);
        for c in "amd".chars() {
            f.key(key(KeyCode::Char(c)));
        }
        match f.key(key(KeyCode::Enter)) {
            Outcome::Pick(Pick::File(p)) => assert_eq!(p, "docs/a.md"),
            _ => panic!("want the md"),
        }
    }

    fn pr(n: u64, title: &str, author: &str, branch: &str) -> PrSummary {
        PrSummary {
            number: n,
            title: title.into(),
            author: author.into(),
            branch: branch.into(),
            base: "main".into(),
            updated_at: "2026-10-09T10:00:00Z".into(),
            body: "Does a thing.\n\nSecond paragraph.".into(),
            paths: vec!["a.rs".into(), "b.rs".into()],
            files: 2,
            additions: 3,
            deletions: 1,
            ..PrSummary::default()
        }
    }

    fn picker() -> Finder {
        let mut f = Finder::reviews(false);
        let list = vec![
            pr(42, "Add magic-link sign-in", "ana", "feat/auth-flow"),
            pr(41, "Cache the sidebar", "luis", "perf/cache"),
            PrSummary {
                is_draft: true,
                ..pr(39, "WIP layout ratio", "bryan", "feat/ratio")
            },
        ];
        assert!(f.set_reviews(list, false));
        f
    }

    fn typed(f: &mut Finder, s: &str) {
        for c in s.chars() {
            f.key(key(KeyCode::Char(c)));
        }
    }

    fn labels(f: &Finder) -> Vec<String> {
        f.shown
            .iter()
            .map(|(i, _)| f.items[*i].label.clone())
            .collect()
    }

    fn screen(f: &Finder) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|fr| f.draw(fr)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// REQ-161: loading shows a row, takes `esc`, and picks nothing; then the
    /// PRs come in the order gh gave them.
    #[test]
    fn the_pr_picker_loads_then_lists_what_it_got() {
        let mut f = Finder::reviews(false);
        assert!(f.loading_reviews() && f.shown.is_empty());
        assert!(screen(&f).contains("Loading open pull requests"));
        assert!(matches!(f.key(key(KeyCode::Enter)), Outcome::Stay));
        assert!(matches!(f.key(key(KeyCode::Esc)), Outcome::Cancel));

        let f = picker();
        assert!(!f.loading_reviews());
        assert_eq!(
            labels(&f),
            [
                "#42 Add magic-link sign-in",
                "#41 Cache the sidebar",
                "#39 WIP layout ratio"
            ]
        );
    }

    #[test]
    fn a_list_for_the_other_scope_is_ignored() {
        let mut f = Finder::reviews(false);
        assert!(!f.set_reviews(vec![pr(1, "x", "a", "b")], true));
        assert!(f.loading_reviews() && f.shown.is_empty());
        assert!(!Finder::workspaces(Vec::new()).set_reviews(Vec::new(), false));
    }

    /// REQ-163: a PR is found by number, title, author or branch.
    #[test]
    fn the_pr_picker_filters_by_number_title_author_and_branch() {
        for (query, want) in [
            ("link", "#42"),
            ("41", "#41"),
            ("luis", "#41"),
            ("ratio", "#39"),
            ("auth-flow", "#42"),
        ] {
            let mut f = picker();
            typed(&mut f, query);
            let first = labels(&f).into_iter().next();
            assert!(
                first.is_some_and(|l| l.starts_with(want)),
                "{query} should find {want}"
            );
        }
        let mut f = picker();
        typed(&mut f, "zzz");
        assert!(labels(&f).is_empty());
        assert!(screen(&f).contains("Nothing matches"));
    }

    /// REQ-179: `m` with an empty query flips the scope and asks again;
    /// with a query it is just a letter.
    #[test]
    fn m_flips_the_scope_and_asks_for_the_list_again() {
        let mut f = picker();
        assert!(matches!(
            f.key(key(KeyCode::Char('m'))),
            Outcome::Reload(true)
        ));
        assert!(f.loading_reviews() && f.shown.is_empty());
        assert!(f.title.contains("requested of me"));
        assert!(matches!(
            f.key(key(KeyCode::Char('m'))),
            Outcome::Reload(false)
        ));
        assert!(!f.title.contains("requested"));

        let mut f = picker();
        typed(&mut f, "a");
        assert!(matches!(f.key(key(KeyCode::Char('m'))), Outcome::Stay));
        assert_eq!(f.query, "am");
    }

    /// REQ-180: `o` with an empty query browses to the selected PR.
    #[test]
    fn o_opens_the_selected_prs_url() {
        let mut f = Finder::reviews(false);
        f.set_reviews(
            vec![PrSummary {
                url: "https://github.com/o/r/pull/42".into(),
                ..pr(42, "Add", "ana", "b")
            }],
            false,
        );
        match f.key(key(KeyCode::Char('o'))) {
            Outcome::Browse(url) => assert_eq!(url, "https://github.com/o/r/pull/42"),
            _ => panic!("expected Browse"),
        }
        typed(&mut f, "a");
        assert!(matches!(f.key(key(KeyCode::Char('o'))), Outcome::Stay));
    }

    /// REQ-165: `↵` picks the selected PR whole.
    #[test]
    fn enter_picks_the_selected_pr() {
        let mut f = picker();
        f.key(key(KeyCode::Down));
        match f.key(key(KeyCode::Enter)) {
            Outcome::Pick(Pick::Review(pr)) => {
                assert_eq!((pr.number, pr.base.as_str()), (41, "main"))
            }
            _ => panic!("expected a review pick"),
        }
    }

    fn plain(lines: &[Line]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// REQ-164: the preview has the description and the files; a draft says so.
    #[test]
    fn the_pr_preview_shows_the_description_and_the_files() {
        let f = picker();
        let text = plain(&f.preview());
        for want in [
            "#42 Add magic-link sign-in",
            "feat/auth-flow",
            "Does a thing.",
            "a.rs",
            "b.rs",
            "+3",
            "−1",
        ] {
            assert!(text.contains(want), "{want} missing from {text}");
        }
        let shown = screen(&f);
        assert!(shown.contains("@ana") && shown.contains("requested of me"));
        let mut d = picker();
        d.sel = 2;
        assert!(plain(&d.preview()).contains("draft"));
    }

    #[test]
    fn ctrl_d_in_the_session_picker_asks_to_delete_the_selected_one() {
        let ws = |id: &str| WsItem {
            id: id.into(),
            label: id.into(),
            open: false,
            hint: String::new(),
            preview: Vec::new(),
        };
        let mut f = Finder::sessions(vec![ws("dev"), ws("olv")]);
        f.key(key(KeyCode::Down));
        let ctrl_d = KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(KeyCode::Char('d'))
        };
        match f.key(ctrl_d) {
            Outcome::Pick(Pick::DeleteSession(name)) => assert_eq!(name, "olv"),
            _ => panic!("want DeleteSession"),
        }
        let mut other = Finder::folders(std::env::temp_dir(), HashSet::new());
        assert!(!matches!(other.key(ctrl_d), Outcome::Pick(_)));
    }
}
