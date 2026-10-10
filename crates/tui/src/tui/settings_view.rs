//! The settings screen, `^␣ ,` (REQ-67): the leader, the theme, the keys
//! after the leader and the themes, each change written to `settings.json`
//! at once. Prototype v7's settings (docs/specs/rust-tui.md, addendum 3).

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::settings::{self, ACTIONS, Settings};
use crate::theme::{self, p};

use super::keys::Leader;

const PAGES: [&str; 3] = ["general", "keys", "themes"];
const MODES: [&str; 3] = ["system", "dark", "light"];

pub enum Outcome {
    Stay,
    Close,
    /// Settings changed: apply them.
    Changed,
    /// Open this file in nvim.
    Edit(PathBuf),
}

#[derive(Default)]
pub struct SettingsView {
    page: usize,
    sel: usize,
    /// The next key is the new binding of the selected row.
    listen: bool,
    /// The last thing that happened, and whether it was an error.
    msg: Option<(String, bool)>,
}

/// One row of a page.
struct Row {
    id: String,
    label: String,
    value: Vec<Span<'static>>,
    hint: String,
    group: Option<&'static str>,
}

impl SettingsView {
    pub fn new() -> Self {
        Self::default()
    }

    /// While a key is awaited, the status bar says so.
    pub fn listening(&self) -> bool {
        self.listen
    }

    fn rows(&self) -> Vec<Row> {
        let s = settings::get();
        let val = |v: String| vec![Span::styled(v, Style::default().fg(p().green))];
        match PAGES[self.page] {
            "general" => vec![
                Row {
                    id: "leader".into(),
                    label: "leader".into(),
                    value: val(s.leader.clone().unwrap_or_else(|| "C-Space".into())),
                    hint: "↵ then press it".into(),
                    group: None,
                },
                Row {
                    id: "theme".into(),
                    label: "theme".into(),
                    value: val(s.theme_mode().into()),
                    hint: "←→ system · dark · light".into(),
                    group: None,
                },
                Row {
                    id: "dark".into(),
                    label: "dark theme".into(),
                    value: val(s.dark_theme().into()),
                    hint: "←→".into(),
                    group: None,
                },
                Row {
                    id: "light".into(),
                    label: "light theme".into(),
                    value: val(s.light_theme().into()),
                    hint: "←→".into(),
                    group: None,
                },
                Row {
                    id: "delay".into(),
                    label: "keys show after".into(),
                    value: val(format!("{} ms", s.which_delay().as_millis())),
                    hint: "←→".into(),
                    group: None,
                },
            ],
            "keys" => ACTIONS
                .iter()
                .map(|(group, action, what, default)| {
                    let key = s.key(action);
                    Row {
                        id: format!("key:{action}"),
                        label: (*what).into(),
                        value: val(settings::label(&key)),
                        hint: if key == *default {
                            String::new()
                        } else {
                            format!("default {} · r resets", settings::label(default))
                        },
                        group: Some(group),
                    }
                })
                .collect(),
            _ => theme::names()
                .into_iter()
                .map(|name| {
                    let (value, kind) = match theme::named(&name) {
                        Ok(pal) => (swatches(&pal), if pal.dark { "dark" } else { "light" }),
                        Err(_) => (
                            vec![Span::styled("doesn't load", Style::default().fg(p().red))],
                            "",
                        ),
                    };
                    let mut hint = Vec::new();
                    if s.dark_theme() == name {
                        hint.push("dark ✓");
                    }
                    if s.light_theme() == name {
                        hint.push("light ✓");
                    }
                    if hint.is_empty() {
                        hint.push(kind);
                    }
                    Row {
                        id: format!("theme:{name}"),
                        label: name,
                        value,
                        hint: hint.join(" "),
                        group: None,
                    }
                })
                .collect(),
        }
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.msg = Some((text.into(), error));
    }

    /// Writes the settings `change` makes, and says where.
    fn save(&mut self, change: impl FnOnce(&mut Settings)) -> Outcome {
        let mut s = (*settings::get()).clone();
        change(&mut s);
        match settings::save(s) {
            Ok(()) => {
                let path = settings::path().map(|p| super::finder::tilde(&p));
                self.say(format!("saved to {}", path.unwrap_or_default()), false);
                Outcome::Changed
            }
            Err(e) => {
                self.say(format!("{e:#}"), true);
                Outcome::Stay
            }
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let rows = self.rows();
        self.sel = self.sel.min(rows.len().saturating_sub(1));
        let Some(row) = rows.get(self.sel) else {
            return match k.code {
                KeyCode::Esc => Outcome::Close,
                _ => Outcome::Stay,
            };
        };
        let id = row.id.clone();
        if self.listen {
            self.listen = false;
            return self.bind(&id, k);
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Tab | KeyCode::BackTab => {
                let n = PAGES.len();
                self.page = if k.code == KeyCode::BackTab {
                    (self.page + n - 1) % n
                } else {
                    (self.page + 1) % n
                };
                self.sel = 0;
                self.msg = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel = (self.sel + 1).min(rows.len().saturating_sub(1))
            }
            KeyCode::Char('n') if ctrl => {
                self.sel = (self.sel + 1).min(rows.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Left | KeyCode::Char('h') => return self.step(&id, -1),
            KeyCode::Right | KeyCode::Char('l') => return self.step(&id, 1),
            KeyCode::Enter => {
                if id == "leader" || id.starts_with("key:") {
                    self.listen = true;
                    self.say("press the key · esc cancels", false);
                } else if let Some(name) = id.strip_prefix("theme:") {
                    return self.use_theme(name);
                } else {
                    return self.step(&id, 1);
                }
            }
            KeyCode::Char('r') if id.starts_with("key:") => {
                let action = id.trim_start_matches("key:").to_string();
                let default = settings::default_key(&action).unwrap_or_default();
                return self.bind_key(&action, default);
            }
            KeyCode::Char('e') if id.starts_with("theme:") => {
                let name = id.trim_start_matches("theme:");
                match own_file(theme::file(name).ok()) {
                    Some(path) => return Outcome::Edit(path),
                    None => self.say(format!("{name} is built in: c copies it to a file"), true),
                }
            }
            KeyCode::Char('c') if id.starts_with("theme:") => {
                let name = id.trim_start_matches("theme:");
                let names = theme::names();
                let to = (1..)
                    .map(|i| {
                        if i == 1 {
                            format!("my-{name}")
                        } else {
                            format!("my-{name}-{i}")
                        }
                    })
                    .find(|n| !names.contains(n))
                    .unwrap_or_default();
                match theme::copy(name, &to) {
                    Ok(path) => {
                        self.sel = theme::names().iter().position(|n| *n == to).unwrap_or(0);
                        self.say(
                            format!("wrote {} · e edits it", super::finder::tilde(&path)),
                            false,
                        );
                    }
                    Err(e) => self.say(format!("{e:#}"), true),
                }
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// The key pressed while listening becomes the leader or an action's.
    fn bind(&mut self, id: &str, k: KeyEvent) -> Outcome {
        if k.code == KeyCode::Esc {
            self.msg = None;
            return Outcome::Stay;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if id == "leader" {
            if alt {
                self.say("Alt belongs to the window manager: pick a Ctrl key", true);
                return Outcome::Stay;
            }
            let name = match k.code {
                KeyCode::Char(' ' | '@') if ctrl => "C-Space".to_string(),
                KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
                    format!("C-{}", c.to_ascii_lowercase())
                }
                _ => {
                    self.say("the leader needs Ctrl, so typing never sets it off", true);
                    return Outcome::Stay;
                }
            };
            if Leader::parse(&name).is_none() {
                self.say(format!("{name} can't be the leader"), true);
                return Outcome::Stay;
            }
            return self.save(|s| s.leader = (name != "C-Space").then_some(name));
        }
        let key = match k.code {
            _ if ctrl || alt => {
                self.say("after the leader it is one plain key, no Ctrl or Alt", true);
                return Outcome::Stay;
            }
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::Char(c) => c.to_string(),
            _ => {
                self.say("after the leader it is one plain key", true);
                return Outcome::Stay;
            }
        };
        let action = id.trim_start_matches("key:").to_string();
        self.bind_key(&action, &key)
    }

    fn bind_key(&mut self, action: &str, key: &str) -> Outcome {
        let mut s = (*settings::get()).clone();
        match s.bind(action, key) {
            Ok(swapped) => {
                let out = self.save(|new| *new = s);
                if let (Some(other), Some((msg, false))) = (swapped, &mut self.msg) {
                    let what = ACTIONS.iter().find(|a| a.1 == other).map_or(other, |a| a.2);
                    msg.push_str(&format!(" · swapped with {what}"));
                }
                out
            }
            Err(e) => {
                self.say(e, true);
                Outcome::Stay
            }
        }
    }

    /// ←→ on the general page.
    fn step(&mut self, id: &str, d: isize) -> Outcome {
        let cycle = |list: &[String], cur: &str| {
            let i = list.iter().position(|x| x == cur).unwrap_or(0) as isize;
            list[(i + d).rem_euclid(list.len() as isize) as usize].clone()
        };
        let s = settings::get();
        match id {
            "theme" => {
                let modes: Vec<String> = MODES.iter().map(|m| m.to_string()).collect();
                let next = cycle(&modes, s.theme_mode());
                self.save(|s| s.theme = (next != "system").then_some(next))
            }
            "dark" | "light" => {
                let names = theme::names();
                let cur = if id == "dark" {
                    s.dark_theme()
                } else {
                    s.light_theme()
                };
                let next = cycle(&names, cur);
                if let Err(e) = theme::named(&next) {
                    self.say(format!("{e:#}"), true);
                    return Outcome::Stay;
                }
                self.save(|s| {
                    if id == "dark" {
                        s.dark = Some(next);
                    } else {
                        s.light = Some(next);
                    }
                })
            }
            "delay" => {
                let ms = (s.which_delay().as_millis() as i64 + d as i64 * 100).clamp(0, 2000);
                self.save(|s| s.which_delay_ms = Some(ms as u64))
            }
            _ => Outcome::Stay,
        }
    }

    /// `↵` on a theme: the one for dark or light mode, as the theme is.
    fn use_theme(&mut self, name: &str) -> Outcome {
        match theme::named(name) {
            Ok(pal) => {
                let name = name.to_string();
                self.save(|s| {
                    if pal.dark {
                        s.dark = Some(name);
                    } else {
                        s.light = Some(name);
                    }
                })
            }
            Err(e) => {
                self.say(format!("{e:#}"), true);
                Outcome::Stay
            }
        }
    }

    pub fn draw(&self, f: &mut Frame, leader: &str) {
        let area = f.area();
        let w = 86.min(area.width.saturating_sub(4));
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
            .border_style(Style::default().fg(p().magenta))
            .title(Span::styled(
                " Settings ",
                Style::default()
                    .fg(p().magenta)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(p().panel).fg(p().fg));
        let inner = block.inner(r);
        f.render_widget(block, r);
        if inner.height < 6 || inner.width < 40 {
            return;
        }
        let dim = Style::default().fg(p().dim);

        // The pages, down the left.
        let nav_w = 12;
        let nav: Vec<Line> = PAGES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                if i == self.page {
                    Line::from(Span::styled(
                        format!(" {name:<10}"),
                        Style::default()
                            .fg(p().blue)
                            .bg(p().sel)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else {
                    Line::from(Span::styled(format!(" {name}"), dim))
                }
            })
            .collect();
        f.render_widget(
            Paragraph::new(nav),
            Rect::new(inner.x, inner.y + 1, nav_w, inner.height.saturating_sub(1)),
        );
        for y in inner.y..inner.bottom() {
            if let Some(c) = f.buffer_mut().cell_mut((inner.x + nav_w, y)) {
                c.set_symbol("│").set_style(Style::default().fg(p().line));
            }
        }

        let page = Rect::new(
            inner.x + nav_w + 2,
            inner.y,
            inner.width.saturating_sub(nav_w + 3),
            inner.height,
        );
        let title = match PAGES[self.page] {
            "general" => "General".to_string(),
            "keys" => format!("Keys after {leader}"),
            _ => "Themes".to_string(),
        };
        let mut lines = vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];
        let rows = self.rows();
        let label_w = 17;
        let mut group = None;
        for (i, row) in rows.iter().enumerate() {
            if row.group.is_some() && row.group != group {
                group = row.group;
                lines.push(Line::from(Span::styled(
                    row.group.unwrap_or_default(),
                    Style::default().fg(p().yellow).add_modifier(Modifier::BOLD),
                )));
            }
            let sel = i == self.sel;
            let mut spans = vec![Span::styled(
                format!(" {:<label_w$}", row.label),
                if sel {
                    Style::default().fg(p().blue)
                } else {
                    Style::default()
                },
            )];
            if sel && self.listen {
                spans.push(Span::styled(
                    "press a key…",
                    Style::default().fg(p().yellow),
                ));
            } else {
                spans.extend(row.value.iter().cloned());
            }
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let pad = (page.width as usize).saturating_sub(used + row.hint.chars().count() + 1);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(row.hint.clone(), dim));
            let mut line = Line::from(spans);
            if sel {
                line = line.style(Style::default().bg(p().sel));
            }
            lines.push(line);
        }
        if PAGES[self.page] == "keys" {
            lines.push(Line::from(Span::styled(
                "fixed",
                dim.add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(" 1–9 · hjkl · HJKL · ? · q", dim)));
        }
        // Keep the selected row on screen.
        let room = page.height.saturating_sub(2) as usize;
        let at = lines
            .iter()
            .position(|l| l.style.bg == Some(p().sel))
            .unwrap_or(0);
        let skip = at.saturating_sub(room.saturating_sub(1));
        let body: Vec<Line> = lines.into_iter().skip(skip).take(room).collect();
        f.render_widget(
            Paragraph::new(body),
            Rect::new(page.x, page.y, page.width, room as u16),
        );

        // Where it saves and what just happened; the keys of this page.
        let foot_y = inner.bottom().saturating_sub(1);
        let (msg, err) = self.msg.clone().unwrap_or_else(|| {
            let path = match PAGES[self.page] {
                "themes" => theme::user_dir()
                    .map(|d| format!("{}/", super::finder::tilde(&d)))
                    .unwrap_or_default(),
                _ => settings::path()
                    .map(|p| super::finder::tilde(&p))
                    .unwrap_or_default(),
            };
            (path, false)
        });
        let keys = match PAGES[self.page] {
            "keys" => "↵ rebind · r reset · tab page · esc",
            "themes" => "↵ use · e edit · c copy · tab page · esc",
            _ => "↵ change · ←→ value · tab page · esc",
        };
        // The page's keys are in the status bar too: a long message wins.
        let width = page.width as usize;
        let room = width.saturating_sub(keys.chars().count() + 2);
        let msg_style = Style::default().fg(if err { p().red } else { p().green });
        let line = if msg.chars().count() <= room {
            let pad = width.saturating_sub(msg.chars().count() + keys.chars().count());
            Line::from(vec![
                Span::styled(msg, msg_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(keys, dim),
            ])
        } else {
            Line::from(Span::styled(msg, msg_style))
        };
        f.render_widget(
            Paragraph::new(line),
            Rect::new(page.x, foot_y, page.width, 1),
        );
    }
}

/// The user's file of a theme, to edit; a built-in has none until copied.
fn own_file(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|p| p.exists())
}

/// A theme's main colours, side by side.
fn swatches(pal: &theme::Palette) -> Vec<Span<'static>> {
    [
        pal.bg,
        pal.panel,
        pal.fg,
        pal.blue,
        pal.green,
        pal.yellow,
        pal.red,
        pal.magenta,
        pal.cyan,
    ]
    .into_iter()
    .map(|c| Span::styled("  ", Style::default().bg(c)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn pages_and_rows() {
        let mut v = SettingsView::new();
        assert_eq!(v.rows()[0].id, "leader");
        v.key(key(KeyCode::Tab));
        assert_eq!(v.rows().len(), ACTIONS.len());
        assert_eq!(v.rows()[0].group, Some("go"));
        v.key(key(KeyCode::Tab));
        assert_eq!(v.rows()[0].id, "theme:one-dark");
        v.key(key(KeyCode::BackTab));
        v.key(key(KeyCode::BackTab));
        assert_eq!(v.page, 0);
        assert!(matches!(v.key(key(KeyCode::Esc)), Outcome::Close));
    }

    #[test]
    fn only_a_themes_own_file_is_edited() {
        let d = tempfile::tempdir().unwrap();
        let mine = d.path().join("mine.json");
        std::fs::write(&mine, "{}").unwrap();
        assert_eq!(own_file(Some(mine.clone())), Some(mine));
        assert_eq!(own_file(Some(d.path().join("tokyo-night.json"))), None);
        assert_eq!(own_file(None), None);
    }

    #[test]
    fn a_leader_needs_ctrl_and_never_alt() {
        let mut v = SettingsView::new();
        v.key(key(KeyCode::Enter));
        assert!(v.listening());
        assert!(matches!(v.key(key(KeyCode::Char('g'))), Outcome::Stay));
        assert!(v.msg.as_ref().is_some_and(|m| m.1), "an error");
        v.key(key(KeyCode::Enter));
        let alt = KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::ALT | KeyModifiers::CONTROL,
        );
        assert!(matches!(v.key(alt), Outcome::Stay));
        assert!(v.msg.as_ref().is_some_and(|m| m.0.contains("Alt")));
    }
}
