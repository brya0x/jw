//! Drawing: the sidebar of workspaces, a header, the current workspace's
//! splits, the status bar and the leader's key popup.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect as TRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::{App, PaneView, Row, SIDEBAR};
use crate::layout::Rect;

pub(super) const BG: Color = Color::Rgb(0x10, 0x23, 0x2a);
pub(super) const PANEL: Color = Color::Rgb(0x15, 0x2e, 0x37);
pub(super) const FG: Color = Color::Rgb(0xd5, 0xe3, 0xe6);
pub(super) const DIM: Color = Color::Rgb(0x5d, 0x7a, 0x82);
pub(super) const LINE: Color = Color::Rgb(0x2a, 0x47, 0x51);
pub(super) const FOCUS: Color = Color::Rgb(0xf0, 0xb4, 0x4c);
pub(super) const WORK: Color = Color::Rgb(0x7f, 0xc4, 0xff);
pub(super) const WAIT: Color = Color::Rgb(0xff, 0x8f, 0x7e);
pub(super) const DEV: Color = Color::Rgb(0x95, 0xde, 0x86);

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.buffer_mut()
        .set_style(area, Style::default().bg(BG).fg(FG));

    if !app.full {
        sidebar(f, app);
        header(f, app);
        status(f, app);
    }

    let view_area = trect(app.view_area());
    if let Some(view) = &mut app.view {
        match view {
            super::View::Diff(d) => d.draw(f, view_area),
            super::View::Md(m) => m.draw(f, view_area),
        }
        if let Some(m) = &app.modal {
            m.draw(f);
        }
        if app.which {
            which(f, app);
        }
        return;
    }
    let rects = app.pane_rects();
    if rects.is_empty() {
        empty(f, app);
    }
    for (role, rect) in rects {
        let r = trect(rect);
        let focused = app.focus.as_deref() == Some(role.as_str());
        let view = app.panes.values().find(|v| v.role == role);
        pane(f, app, r, &role, view, focused);
    }
    if let Some(m) = &app.modal {
        m.draw(f);
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
        .border_style(Style::default().fg(LINE))
        .style(Style::default().bg(PANEL));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let mut lines = vec![Line::from(vec![
        Span::styled(
            " jw",
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  workspaces", Style::default().fg(DIM)),
    ])];
    let mut n = 0;
    for (i, row) in app.rows.iter().enumerate() {
        match row {
            Row::Space(name) => {
                if i > 0 {
                    lines.push(Line::default());
                }
                let label = if name == crate::free::PROJECT {
                    crate::free::LABEL
                } else {
                    name.as_str()
                };
                lines.push(Line::from(Span::styled(
                    format!(" {label}"),
                    Style::default().fg(DIM).add_modifier(Modifier::BOLD),
                )));
            }
            Row::Stream(e) => {
                n += 1;
                let open = app.is_open(&e.id);
                let active = app.current().is_some_and(|c| c.id == e.id);
                let number = if n <= 9 { n.to_string() } else { " ".into() };
                let dot = if open {
                    Span::styled("●", Style::default().fg(DEV))
                } else {
                    Span::styled("○", Style::default().fg(DIM))
                };
                let mut name_style = Style::default().fg(if open { FG } else { DIM });
                if active {
                    name_style = name_style.fg(FOCUS).add_modifier(Modifier::BOLD);
                }
                let mut line = Line::from(vec![
                    Span::styled(format!(" {number} "), Style::default().fg(DIM)),
                    dot,
                    Span::raw(" "),
                    Span::styled(e.name.clone(), name_style),
                ]);
                if active {
                    line = line.style(Style::default().bg(LINE));
                }
                lines.push(line);
            }
        }
    }
    if app.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            " No workspaces yet",
            Style::default().fg(DIM),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn header(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(SIDEBAR, 0, area.width.saturating_sub(SIDEBAR), 1);
    let line = match &app.active {
        Some(s) => {
            let e = &s.entry;
            let ports: Vec<String> = s
                .vars()
                .ports
                .iter()
                .map(|(svc, p)| format!("{svc} :{p}"))
                .collect();
            if crate::free::is_free(e) {
                let line = Line::from(vec![
                    Span::styled(
                        format!(" {}/", crate::free::LABEL),
                        Style::default().fg(DIM),
                    ),
                    Span::styled(
                        e.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", e.path), Style::default().fg(DIM)),
                ]);
                f.render_widget(Paragraph::new(line), r);
                return;
            }
            let mut spans = vec![
                Span::styled(format!(" {}", e.project), Style::default().fg(DIM)),
                Span::styled("/", Style::default().fg(DIM)),
                Span::styled(
                    e.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", e.branch), Style::default().fg(WORK)),
                Span::styled(format!("  slot {}", e.slot), Style::default().fg(DIM)),
            ];
            if !ports.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", ports.join("  ")),
                    Style::default().fg(DIM),
                ));
            }
            Line::from(spans)
        }
        None => Line::from(Span::styled(" no stream open", Style::default().fg(DIM))),
    };
    f.render_widget(Paragraph::new(line), r);
}

fn status(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(0, area.height.saturating_sub(1), area.width, 1);
    let leader = app.leader.label();
    let (chip, color) = if app.leading() {
        (format!(" {leader} "), FOCUS)
    } else {
        (" TERM ".to_string(), WORK)
    };
    let key = |k: &str| Span::styled(format!(" {k}"), Style::default().fg(FOCUS));
    let txt = |t: &str| Span::styled(format!(" {t} "), Style::default().fg(DIM));
    let mut spans = vec![Span::styled(
        chip,
        Style::default()
            .bg(color)
            .fg(BG)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(job) = &app.busy {
        spans.push(Span::styled(
            format!("  ⟳ {job}"),
            Style::default().fg(WORK),
        ));
    }
    if let Some(msg) = &app.status {
        spans.push(Span::styled(format!("  {msg}"), Style::default().fg(WAIT)));
    } else if app.leading() {
        spans.extend([txt("one key"), key("esc"), txt("cancels")]);
    } else {
        spans.extend([
            key(&leader),
            txt("then a key ·"),
            key("1-9"),
            txt("workspace"),
            key("w"),
            txt("worktree"),
            key("d"),
            txt("changes"),
            key("?"),
            txt("all keys"),
        ]);
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL)),
        r,
    );
}

fn empty(f: &mut Frame, app: &App) {
    let stage = trect(app.stage());
    let leader = app.leader.label();
    let msg = if app.rows.is_empty() {
        "No workspaces yet.".to_string()
    } else {
        format!("{leader} then 1–9 opens a workspace from the sidebar.")
    };
    let y = stage.y + stage.height / 2;
    f.render_widget(
        Paragraph::new(Span::styled(msg, Style::default().fg(DIM))).alignment(Alignment::Center),
        TRect::new(stage.x, y, stage.width, 1),
    );
}

fn pane(f: &mut Frame, app: &App, r: TRect, role: &str, view: Option<&PaneView>, focused: bool) {
    let border = if focused {
        Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(LINE)
    };
    let mut title = vec![Span::styled(
        format!(" {role} "),
        if focused {
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        },
    )];
    if let Some(status) = view.and_then(|v| v.exited) {
        title.push(Span::styled(
            format!(" exited {status} "),
            Style::default().fg(WAIT),
        ));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(border)
        .title(Line::from(title));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let Some(view) = view else {
        f.render_widget(
            Paragraph::new(Span::styled("starting…", Style::default().fg(DIM))),
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
                .fg(color(cell.fgcolor(), FG))
                .bg(color(cell.bgcolor(), BG));
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
            &[("1-9", "workspace"), ("tab", "previous"), ("q", "detach")],
        ),
        (
            "worktree",
            &[
                ("w", "new"),
                ("s", "sync"),
                ("d", "changes"),
                ("X", "remove"),
            ],
        ),
        ("panes", &[("hjkl", "go"), ("f", "full"), ("x", "close")]),
    ];
    let rows = GROUPS.iter().map(|(_, k)| k.len()).max().unwrap_or(0) as u16;
    let col = 18u16;
    let area = f.area();
    let w = (col * GROUPS.len() as u16 + 2).min(area.width);
    let h = (rows + 3).min(area.height);
    let r = TRect::new(
        area.width.saturating_sub(w + 1),
        area.height.saturating_sub(h + 1),
        w,
        h,
    );
    f.render_widget(ratatui::widgets::Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(FOCUS))
        .title(Span::styled(
            format!(" {} then ", app.leader.label()),
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL).fg(FG));
    let inner = block.inner(r);
    f.render_widget(block, r);
    for (i, (title, keys)) in GROUPS.iter().enumerate() {
        let mut lines = vec![Line::from(Span::styled(
            *title,
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
        ))];
        for (k, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{k:<5}"), Style::default().fg(WORK)),
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
