//! Drawing: the sidebar of workspaces, a header, the current workspace's
//! splits, the status bar and the leader's key popup.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect as TRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::{App, PaneView, SIDEBAR};
use crate::layout::Rect;
use crate::theme::p;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.buffer_mut()
        .set_style(area, Style::default().bg(p().bg).fg(p().fg));

    sidebar(f, app);
    header(f, app);
    status(f, app);

    let rects = app.pane_rects();
    if rects.is_empty() {
        empty(f, app);
    }
    for (id, rect) in rects {
        let r = trect(rect);
        let focused = app.focus == Some(id);
        match app.views.get_mut(&id) {
            Some(super::View::Diff(d)) => {
                d.focused = focused;
                d.draw(f, r);
            }
            Some(super::View::Md(m)) => {
                m.focused = focused;
                m.draw(f, r);
            }
            None => pane(f, app, r, app.pane_title(id), app.panes.get(&id), focused),
        }
    }
    if let Some(m) = &app.modal {
        m.draw(f);
    }
    if let Some(finder) = &app.finder {
        finder.draw(f);
    }
    if app.which {
        which(f, app);
    }
}

fn trect(r: Rect) -> TRect {
    TRect::new(r.x, r.y, r.w, r.h)
}

fn sidebar(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(0, 0, SIDEBAR.min(area.width), area.height.saturating_sub(1));
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(p().line))
        .style(Style::default().bg(p().panel));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let dim = Style::default().fg(p().dim);
    let w = inner.width as usize;
    let hint = format!("{} a sessions ", app.leader.label());
    let title = format!(" {}", crate::session::current());
    let pad = w.saturating_sub(title.chars().count() + hint.chars().count());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(title, dim.add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(pad)),
            Span::styled(hint, dim),
        ]),
        Line::default(),
    ];
    let rows = inner.height.saturating_sub(4) as usize;
    let current = app
        .rows
        .iter()
        .position(|r| app.current().is_some_and(|c| c.id == r.entry.id))
        .unwrap_or(0);
    let first = current.saturating_sub(rows.saturating_sub(1));
    for (i, row) in app.rows.iter().enumerate().skip(first).take(rows) {
        let e = &row.entry;
        let open = app.is_open(&e.id);
        let active = app.current().is_some_and(|c| c.id == e.id);
        let number = if i < 9 {
            (i + 1).to_string()
        } else {
            " ".into()
        };
        let mut name = Style::default().fg(if open { p().fg } else { p().dim });
        if active {
            name = name.fg(p().blue).add_modifier(Modifier::BOLD);
        }
        let mut spans = vec![Span::styled(format!(" {number} "), dim)];
        if row.child {
            spans.push(Span::styled("  ↳", dim));
        }
        spans.push(Span::styled(
            if open { "● " } else { "○ " },
            Style::default().fg(if open { p().green } else { p().dim }),
        ));
        let used = 3 + if row.child { 3 } else { 0 } + 2;
        let room = w.saturating_sub(used + 1 + app.marks(&e.id).len() * 2);
        let label: String = if e.name.chars().count() > room {
            e.name
                .chars()
                .take(room.saturating_sub(1))
                .chain(['…'])
                .collect()
        } else {
            e.name.clone()
        };
        spans.push(Span::styled(label.clone(), name));
        let marks = app.marks(&e.id);
        if !marks.is_empty() {
            let used = used + label.chars().count();
            let text: Vec<Span> = marks
                .iter()
                .map(|m| Span::styled(format!("{m}"), Style::default().fg(mark_color(*m))))
                .collect();
            let width = marks.len() * 2 - 1;
            spans.push(Span::raw(
                " ".repeat(w.saturating_sub(used + width + 1).max(1)),
            ));
            for (i, t) in text.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(t);
            }
        }
        let mut line = Line::from(spans);
        if active {
            line = line.style(Style::default().bg(p().sel));
        }
        lines.push(line);
    }
    if app.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {} o opens a folder", app.leader.label()),
            dim,
        )));
    }
    let list = TRect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    f.render_widget(Paragraph::new(lines), list);
    let legend = TRect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2);
    let sym = |m: char, what: &str| {
        [
            Span::styled(format!(" {m} "), Style::default().fg(mark_color(m))),
            Span::styled(what.to_string(), dim),
        ]
    };
    let mut a = Vec::new();
    for (m, what) in [('●', "open"), ('○', "closed"), ('✻', "agent")] {
        a.extend(sym(m, what));
    }
    let mut b = Vec::new();
    for (m, what) in [('?', "waiting"), ('⚡', "dev"), ('⚑', "merged")] {
        b.extend(sym(m, what));
    }
    f.render_widget(Paragraph::new(vec![Line::from(a), Line::from(b)]), legend);
}

fn header(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(SIDEBAR, 0, area.width.saturating_sub(SIDEBAR), 1);
    let dim = Style::default().fg(p().dim);
    let Some(s) = &app.active else {
        f.render_widget(Paragraph::new(Span::styled(" no workspace open", dim)), r);
        return;
    };
    let e = &s.entry;
    let folder = crate::folders::is_folder(e);
    let mut spans = Vec::new();
    if !folder {
        spans.push(Span::styled(format!(" {}/", e.project), dim));
    } else {
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        e.name.clone(),
        Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
    ));
    if e.branch.is_empty() {
        spans.push(Span::styled("  not a git repo", dim));
    } else {
        spans.push(Span::styled(
            format!("  {}", e.branch),
            Style::default().fg(p().magenta),
        ));
    }
    if !folder && !s.base.is_empty() {
        spans.push(Span::styled(format!("  from {}", s.base), dim));
    }
    if let Some(pr) = app.prs.get(&e.id) {
        let color = if pr.state == "MERGED" {
            p().cyan
        } else {
            p().dim
        };
        spans.push(Span::styled(
            format!("  PR {}", pr.label()),
            Style::default().fg(color),
        ));
    }
    if !folder {
        for (svc, port) in s.vars().ports {
            spans.push(Span::styled(format!("  {svc} :{port}"), dim));
        }
    }
    let path = super::finder::tilde(std::path::Path::new(&e.path));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = (r.width as usize).saturating_sub(used + 1);
    if path.chars().count() + 2 <= room {
        spans.push(Span::raw(" ".repeat(room - path.chars().count())));
        spans.push(Span::styled(path, dim));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), r);
}

/// Each sidebar mark in its colour (REQ-51).
fn mark_color(m: char) -> ratatui::style::Color {
    match m {
        '●' => p().green,
        '✻' => p().magenta,
        '?' => p().yellow,
        '⚡' => p().green,
        '⚑' => p().cyan,
        _ => p().dim,
    }
}

fn status(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(0, area.height.saturating_sub(1), area.width, 1);
    let leader = app.leader.label();
    let confirm = matches!(
        app.modal,
        Some(
            super::Modal::Close { .. }
                | super::Modal::ClosePane { .. }
                | super::Modal::Done { .. }
                | super::Modal::Rm { .. }
        )
    );
    let (chip, color) = if confirm {
        (" CONFIRM ".to_string(), p().red)
    } else if app.leading() {
        (format!(" {leader} "), p().yellow)
    } else {
        match app.focused_view() {
            Some(super::View::Diff(_)) => (" DIFF ".to_string(), p().blue),
            Some(super::View::Md(_)) => (" MD ".to_string(), p().blue),
            None => (" TERM ".to_string(), p().green),
        }
    };
    let key = |k: &str| {
        Span::styled(
            format!(" {k}"),
            Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
        )
    };
    let txt = |t: &str| Span::styled(format!(" {t} "), Style::default().fg(p().dim));
    let mut spans = vec![
        Span::styled(
            chip,
            Style::default()
                .bg(color)
                .fg(p().bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if let Some(job) = &app.busy {
        spans.push(Span::styled(
            format!(" ⟳ {job} "),
            Style::default().fg(p().cyan),
        ));
    }
    if confirm {
        spans.extend([key("y"), txt("yes ·"), key("esc"), txt("no")]);
    } else if app.leading() {
        spans.extend([txt("one key ·"), key("esc"), txt("cancels")]);
    } else {
        match app.focused_view() {
            Some(super::View::Diff(_)) => spans.extend([
                key("j k"),
                txt("file ·"),
                key("] ["),
                txt("change ·"),
                key("v"),
                txt("viewed ·"),
                key("t"),
                txt("layout ·"),
                key("↵"),
                txt("open ·"),
                key("q"),
                txt("close"),
            ]),
            Some(super::View::Md(_)) => spans.extend([
                key("j k ␣ b"),
                txt("scroll ·"),
                key("n N"),
                txt("heading ·"),
                key("q"),
                txt("close"),
            ]),
            None => spans.extend([
                key(&leader),
                txt("then a key ·"),
                key("␣"),
                txt("switch ·"),
                key("o"),
                txt("open ·"),
                key("w"),
                txt("worktree ·"),
                key("t"),
                txt("pane ·"),
                key("?"),
                txt("all keys"),
            ]),
        }
    }
    if let Some(msg) = &app.status {
        let color = match msg.tone {
            super::Tone::Done => p().green,
            super::Tone::Error => p().red,
            super::Tone::Info => p().yellow,
        };
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let text = format!("{} ", msg.text);
        let room = (r.width as usize).saturating_sub(used);
        let n = text.chars().count();
        if n <= room {
            spans.push(Span::raw(" ".repeat(room - n)));
            spans.push(Span::styled(text, Style::default().fg(color)));
        } else {
            // No room beside the hint: the message takes the bar.
            spans.truncate(2);
            spans.push(Span::styled(
                format!(" {}", msg.text),
                Style::default().fg(color),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p().panel)),
        r,
    );
}

fn empty(f: &mut Frame, app: &App) {
    let stage = trect(app.stage());
    let leader = app.leader.label();
    let msg = format!("{leader} o opens a folder · {leader} 1–9 a workspace from the sidebar");
    let y = stage.y + stage.height / 2;
    f.render_widget(
        Paragraph::new(Span::styled(msg, Style::default().fg(p().dim)))
            .alignment(Alignment::Center),
        TRect::new(stage.x, y, stage.width, 1),
    );
}

fn pane(
    f: &mut Frame,
    app: &App,
    r: TRect,
    (title, runs): (String, Option<String>),
    view: Option<&PaneView>,
    focused: bool,
) {
    let accent = if focused { p().blue } else { p().line };
    let mut spans = vec![Span::styled(
        format!(" {title} "),
        if focused {
            Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p().fg).add_modifier(Modifier::BOLD)
        },
    )];
    if let Some(runs) = runs {
        let room = (r.width as usize).saturating_sub(title.chars().count() + 8);
        let runs: String = if runs.chars().count() > room {
            runs.chars()
                .take(room.saturating_sub(1))
                .chain(['…'])
                .collect()
        } else {
            runs
        };
        if !runs.is_empty() {
            spans.push(Span::styled(
                format!("{runs} "),
                Style::default().fg(p().dim),
            ));
        }
    }
    let back = view.map_or(0, |v| v.parser.screen().scrollback());
    if back > 0 {
        spans.push(Span::styled(
            format!(" ↑ {back} lines · type to go back "),
            Style::default().fg(p().yellow),
        ));
    }
    if let Some(status) = view.and_then(|v| v.exited) {
        spans.push(Span::styled(
            format!(" exited {status} "),
            Style::default().fg(p().red),
        ));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(spans));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let Some(view) = view else {
        f.render_widget(
            Paragraph::new(Span::styled("starting…", Style::default().fg(p().dim))),
            inner,
        );
        return;
    };
    let screen = view.parser.screen();
    screen_to(f.buffer_mut(), screen, inner, app.leading() && !focused);
    if focused && !app.leading() && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < inner.height && col < inner.width {
            f.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
}

/// Copies a vt100 screen into the buffer. `dim` fades it, for panes that
/// aren't taking keys while navigating.
fn screen_to(buf: &mut Buffer, screen: &vt100::Screen, area: TRect, dim: bool) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut style = Style::default()
                .fg(color(cell.fgcolor(), p().fg))
                .bg(color(cell.bgcolor(), p().bg));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if dim {
                style = style.add_modifier(Modifier::DIM);
            }
            let contents = cell.contents();
            let symbol = if contents.is_empty() { " " } else { contents };
            if let Some(c) = buf.cell_mut((area.x + col, area.y + row)) {
                c.set_symbol(symbol).set_style(style);
            }
        }
    }
}

fn color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The leader's popup: every key, grouped go / worktree / panes (REQ-31).
fn which(f: &mut Frame, app: &App) {
    const GROUPS: [(&str, &[(&str, &str)]); 3] = [
        (
            "go",
            &[
                ("␣", "switch"),
                ("tab", "previous"),
                ("1-9", "workspace"),
                ("o", "open folder"),
                ("/", "open file"),
                ("a", "sessions"),
                ("q", "detach"),
            ],
        ),
        (
            "worktree",
            &[
                ("w", "new"),
                ("r", "rename"),
                ("s", "sync"),
                ("d", "changes"),
                ("X", "remove"),
            ],
        ),
        (
            "panes",
            &[
                ("hjkl", "go"),
                ("t", "new"),
                ("x", "close"),
                ("n", "name"),
                ("f", "full"),
                ("HJKL", "move"),
            ],
        ),
    ];
    let rows = GROUPS.iter().map(|(_, k)| k.len()).max().unwrap_or(0) as u16;
    let col = 17u16;
    let area = f.area();
    let w = (col * GROUPS.len() as u16 + 3).min(area.width);
    let h = (rows + 3).min(area.height);
    let r = TRect::new(
        area.width.saturating_sub(w + 1),
        area.height.saturating_sub(h + 2),
        w,
        h,
    );
    f.render_widget(ratatui::widgets::Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p().yellow))
        .title(Span::styled(
            format!(" {} then ", app.leader.label()),
            Style::default().fg(p().yellow).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(p().panel).fg(p().fg));
    let inner = block.inner(r);
    f.render_widget(block, r);
    for (i, (title, keys)) in GROUPS.iter().enumerate() {
        let mut lines = vec![Line::from(Span::styled(
            *title,
            Style::default().fg(p().yellow).add_modifier(Modifier::BOLD),
        ))];
        for (k, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{k:<5}"), Style::default().fg(p().blue)),
                Span::raw(*what),
            ]));
        }
        let x = inner.x + 1 + col * i as u16;
        let c = TRect::new(
            x,
            inner.y,
            col.min(inner.right().saturating_sub(x)),
            inner.height,
        );
        f.render_widget(Paragraph::new(lines), c);
    }
}
