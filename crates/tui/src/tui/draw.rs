//! Drawing: the sidebar of workspaces, a header, the current workspace's
//! splits, the status bar and the leader's key popup.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Alignment, Rect as TRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::anim;
use super::select::Sel;
use super::{App, PaneView, SIDEBAR, key};
use crate::layout::Rect;
use crate::theme::p;

pub fn draw(f: &mut Frame, app: &mut App) {
    let now = std::time::Instant::now();
    app.tick_motion(now);
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
    for &(id, rect) in &rects {
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
            None => pane(
                f,
                app,
                r,
                app.pane_title(id),
                app.panes.get(&id),
                app.selection(id),
                focused,
            ),
        }
    }
    if let Some((old, new, ease)) = app.focus_fade {
        let t = ease.value(now);
        for &(id, rect) in &rects {
            if Some(id) == new {
                ease_border(f.buffer_mut(), trect(rect), p().line, p().blue, t);
            } else if Some(id) == old {
                ease_border(f.buffer_mut(), trect(rect), p().blue, p().line, t);
            }
        }
    }
    banner(f, app);
    if let Some((enter, dir)) = app.switch {
        let header = TRect::new(SIDEBAR, 0, area.width.saturating_sub(SIDEBAR), 1);
        enter_transition(
            f.buffer_mut(),
            header,
            trect(app.stage()),
            enter.value(now),
            dir,
        );
    }
    let base = app.popup.map(|_| f.buffer_mut().clone());
    if let Some(m) = &app.modal {
        m.draw(f);
    }
    if let Some(finder) = &app.finder {
        finder.draw(f);
    }
    if let Some(s) = &app.settings {
        s.draw(f, &app.leader.label());
    }
    if app.which {
        which(f, app);
    }
    if let (Some(base), Some(fade)) = (base, app.popup) {
        dissolve(f.buffer_mut(), &base, fade.value(now));
    }
}

/// REQ-145: a pane's border, already drawn in its final colour `to`, shown
/// `t` of the way from `from`. The title's text is left as it is.
fn ease_border(buf: &mut Buffer, r: TRect, from: Color, to: Color, t: f32) {
    if r.width < 2 || r.height < 2 {
        return;
    }
    let right = r.right() - 1;
    let bottom = r.bottom() - 1;
    let edge = (r.left()..=right)
        .flat_map(|x| [(x, r.y), (x, bottom)])
        .chain((r.top()..=bottom).flat_map(|y| [(r.x, y), (right, y)]));
    for pos in edge {
        let cell = &mut buf[pos];
        if cell.fg == to && matches!(cell.symbol(), "─" | "│" | "╭" | "╮" | "╰" | "╯") {
            cell.fg = anim::lerp(from, to, t);
        }
    }
}

/// REQ-144: what an overlay drew fades in over what was under it. `a` is 0
/// at the start; the text changes over at the half.
fn dissolve(buf: &mut Buffer, base: &Buffer, a: f32) {
    let blend = |under: Color, over: Color| match (under, over) {
        (Color::Rgb(..), Color::Rgb(..)) => anim::lerp(under, over, a),
        _ if a < 0.5 => under,
        _ => over,
    };
    for (cell, under) in buf.content.iter_mut().zip(&base.content) {
        if cell == under {
            continue;
        }
        let (fg, bg) = (blend(under.fg, cell.fg), blend(under.bg, cell.bg));
        if a < 0.5 {
            cell.set_symbol(under.symbol());
        }
        cell.fg = fg;
        cell.bg = bg;
    }
}

/// `✓ Copied to clipboard`, on the top edge of the pane the copy came from
/// (REQ-135).
fn banner(f: &mut Frame, app: &App) {
    let Some((id, n, _)) = app.copied else {
        return;
    };
    let Some(rect) = app
        .pane_rects()
        .into_iter()
        .find(|(p, _)| *p == id)
        .map(|(_, r)| trect(r))
    else {
        return;
    };
    let text = banner_text(n);
    let w = (text.chars().count() as u16).min(rect.width.saturating_sub(2));
    if w == 0 {
        return;
    }
    let r = TRect::new(rect.x + rect.width - 1 - w, rect.y, w, 1);
    f.render_widget(
        Paragraph::new(text).style(
            Style::default()
                .bg(p().green)
                .fg(p().bg)
                .add_modifier(Modifier::BOLD),
        ),
        r,
    );
}

fn banner_text(n: usize) -> String {
    format!(
        " ✓ Copied to clipboard · {n} character{} ",
        if n == 1 { "" } else { "s" }
    )
}

/// REQ-136: the header and the panes fade in, and the panes travel in from
/// the side the move came from. It works on the cells already drawn, so every
/// pane and viewer takes part without knowing. `t` is 0 at the start, 1 done.
fn enter_transition(buf: &mut Buffer, header: TRect, stage: TRect, t: f32, dir: i8) {
    let bg = p().bg;
    let off = ((1.0 - t) * f32::from(anim::SLIDE_CELLS)).round() as usize;
    let width = usize::from(stage.width);
    let mut blank = Cell::default();
    blank.set_style(Style::default().bg(bg).fg(p().fg));
    for y in stage.top()..stage.bottom() {
        if off > 0 && width > 0 {
            let row: Vec<Cell> = (0..width)
                .map(|i| buf[(stage.x + i as u16, y)].clone())
                .collect();
            for i in 0..width {
                let src = if dir >= 0 {
                    i.checked_sub(off)
                } else {
                    (i + off < width).then_some(i + off)
                };
                buf[(stage.x + i as u16, y)] =
                    src.map_or_else(|| blank.clone(), |s| row[s].clone());
            }
        }
        for x in stage.left()..stage.right() {
            fade(&mut buf[(x, y)], t, bg);
        }
    }
    for x in header.left()..header.right() {
        fade(&mut buf[(x, header.y)], t, bg);
    }
}

/// A cell's text colour blended in from its own background; colours that are
/// not true colour are left as they are.
fn fade(cell: &mut Cell, t: f32, bg: Color) {
    let ground = if matches!(cell.bg, Color::Rgb(..)) {
        cell.bg
    } else {
        bg
    };
    if matches!((ground, cell.fg), (Color::Rgb(..), Color::Rgb(..))) {
        cell.fg = anim::lerp(ground, cell.fg, t);
    }
}

fn trect(r: Rect) -> TRect {
    TRect::new(r.x, r.y, r.w, r.h)
}

fn sidebar(f: &mut Frame, app: &App) {
    let area = f.area();
    let r = TRect::new(0, 0, SIDEBAR.min(area.width), area.height.saturating_sub(1));
    let edge = if app.walking { p().blue } else { p().line };
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(edge))
        .style(Style::default().bg(p().panel));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let dim = Style::default().fg(p().dim);
    let w = inner.width as usize;
    let hint = format!("{} {} sessions ", app.leader.label(), key("sessions"));
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
    let first = app.sidebar_first(rows);
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
        let marks = app.marks(&e.id);
        let used = 3 + if row.child { 3 } else { 0 } + 2;
        let room = w.saturating_sub(used + 1 + marks.len() * 2);
        let label: String = if e.name.chars().count() > room {
            e.name
                .chars()
                .take(room.saturating_sub(1))
                .chain(['…'])
                .collect()
        } else {
            e.name.clone()
        };
        let used = used + label.chars().count();
        spans.push(Span::styled(label, name));
        if !marks.is_empty() {
            let width = marks.len() * 2 - 1;
            spans.push(Span::raw(
                " ".repeat(w.saturating_sub(used + width + 1).max(1)),
            ));
            for (i, m) in marks.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                let shown = match (m, app.spinner) {
                    ('✻', Some(glyph)) => glyph,
                    _ => m,
                };
                spans.push(Span::styled(
                    shown.to_string(),
                    Style::default().fg(mark_color(m)),
                ));
            }
        }
        let mut line = Line::from(spans);
        if app.band == Some(i) {
            line = line.style(Style::default().bg(p().sel));
        }
        lines.push(line);
    }
    if app.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {} {} opens a folder", app.leader.label(), key("open")),
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
    // The git of where the focused pane is, like a shell prompt; the
    // workspace's own until the daemon says.
    let here = app.focus.and_then(|f| app.here.get(&f));
    let (branch, home) = match here {
        Some(h) => match &h.git {
            Some((root, branch)) => {
                if !h.home {
                    let repo = root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    spans.push(Span::styled(
                        format!("  {repo}"),
                        Style::default().fg(p().fg),
                    ));
                }
                (branch.clone(), h.home)
            }
            None => (String::new(), false),
        },
        None => (e.branch.clone(), true),
    };
    if branch.is_empty() {
        spans.push(Span::styled("  not a git repo", dim));
    } else {
        spans.push(Span::styled(
            format!("  {branch}"),
            Style::default().fg(p().magenta),
        ));
    }
    if home && !folder && !s.base.is_empty() {
        spans.push(Span::styled(format!("  from {}", s.base), dim));
    }
    if let Some(pr) = app.prs.get(&e.id).filter(|_| home) {
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
    if app.layout_changed() {
        spans.push(Span::styled(
            format!(
                "  layout changed · {} {} saves it",
                app.leader.label(),
                key("layout")
            ),
            Style::default().fg(p().yellow),
        ));
    }
    if !folder {
        for (svc, port) in s.vars().ports {
            spans.push(Span::styled(format!("  {svc} :{port}"), dim));
        }
    }
    let path =
        super::finder::tilde(here.map_or(std::path::Path::new(&e.path), |h| h.cwd.as_path()));
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

/// What the status bar is for right now: its chip and its hint (REQ-140).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    Set,
    SetKey,
    Confirm,
    Leader,
    Move,
    Diff,
    Md,
    Term,
}

/// The mode the bar is in, the one it left, and when (REQ-140).
pub(super) struct ModeFade {
    pub mode: Mode,
    pub prev: Mode,
    pub since: Instant,
}

pub(super) fn mode_of(app: &App) -> Mode {
    let confirm = matches!(
        app.modal,
        Some(
            super::Modal::Close { .. }
                | super::Modal::ClosePane { .. }
                | super::Modal::Done { .. }
                | super::Modal::Rm { .. }
        )
    );
    if let Some(v) = &app.settings {
        if v.listening() {
            Mode::SetKey
        } else {
            Mode::Set
        }
    } else if confirm {
        Mode::Confirm
    } else if app.leading() {
        Mode::Leader
    } else if app.walking {
        Mode::Move
    } else {
        match app.focused_view() {
            Some(super::View::Diff(_)) => Mode::Diff,
            Some(super::View::Md(_)) => Mode::Md,
            None => Mode::Term,
        }
    }
}

fn chip(mode: Mode, leader: &str) -> (String, Color) {
    match mode {
        Mode::Set | Mode::SetKey => (" SET ".into(), p().magenta),
        Mode::Confirm => (" CONFIRM ".into(), p().red),
        Mode::Leader => (format!(" {leader} "), p().yellow),
        Mode::Move => (" MOVE ".into(), p().blue),
        Mode::Diff => (" DIFF ".into(), p().blue),
        Mode::Md => (" MD ".into(), p().blue),
        Mode::Term => (" TERM ".into(), p().green),
    }
}

/// What to do next in `mode`: the keys that matter, in the user's bindings.
fn hint(mode: Mode, leader: &str) -> Vec<Span<'static>> {
    let k = |k: &str| {
        Span::styled(
            format!(" {k}"),
            Style::default().fg(p().fg).add_modifier(Modifier::BOLD),
        )
    };
    let t = |t: &str| Span::styled(format!(" {t} "), Style::default().fg(p().dim));
    match mode {
        Mode::SetKey => vec![t("press the new key ·"), k("esc"), t("cancels")],
        Mode::Set => vec![
            k("tab"),
            t("page ·"),
            k("↑↓"),
            t("move ·"),
            k("↵"),
            t("change ·"),
            k("←→"),
            t("value ·"),
            k("esc"),
            t("close"),
        ],
        Mode::Confirm => vec![k("y"), t("yes ·"), k("esc"), t("no")],
        Mode::Leader => vec![
            k(&key("switch")),
            t("switch ·"),
            k(&key("open")),
            t("open ·"),
            k(&key("new")),
            t("worktree ·"),
            k(&key("pane")),
            t("pane ·"),
            k("hjkl"),
            t("focus ·"),
            k("?"),
            t("all keys ·"),
            k("esc"),
            t("cancels"),
        ],
        Mode::Move => vec![
            k("↑↓"),
            t("workspace ·"),
            k("←→"),
            t("project ·"),
            k("↵"),
            t("back to the pane"),
        ],
        Mode::Diff => vec![
            k("j k"),
            t("file ·"),
            k("] ["),
            t("change ·"),
            k("v"),
            t("viewed ·"),
            k("t"),
            t("layout ·"),
            k("↵"),
            t("open ·"),
            k("q"),
            t("close"),
        ],
        Mode::Md => vec![
            k("j k ␣ b"),
            t("scroll ·"),
            k("n N"),
            t("heading ·"),
            k("q"),
            t("close"),
        ],
        Mode::Term => vec![
            k(leader),
            t("then a key ·"),
            k(&key("switch")),
            t("switch ·"),
            k(&key("open")),
            t("open ·"),
            k(&key("new")),
            t("worktree ·"),
            k(&key("pane")),
            t("pane ·"),
            k(&key("sessions")),
            t("sessions ·"),
            k("?"),
            t("all keys"),
        ],
    }
}

/// `spans` with their text colour `alpha` of the way up from the bar's own
/// background.
fn faded(spans: Vec<Span<'static>>, alpha: f32) -> Vec<Span<'static>> {
    if alpha >= 1.0 {
        return spans;
    }
    spans
        .into_iter()
        .map(|mut s| {
            if let Some(fg) = s.style.fg {
                s.style.fg = Some(anim::lerp(p().panel, fg, alpha));
            }
            s
        })
        .collect()
}

fn status(f: &mut Frame, app: &App) {
    let now = Instant::now();
    let area = f.area();
    let r = TRect::new(0, area.height.saturating_sub(1), area.width, 1);
    let leader = app.leader.label();
    let cur = mode_of(app);
    let (prev, since) = app.mode.as_ref().map_or((cur, Duration::MAX), |m| {
        (m.prev, now.saturating_duration_since(m.since))
    });
    let (chip, color) = chip(cur, &leader);
    let from = chip_color(prev);
    let color = anim::lerp(
        from,
        color,
        anim::ease_out(anim::progress(since, anim::CHIP_EASE)),
    );
    let (new, alpha) = anim::crossfade(since);
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
    let shown = if new { cur } else { prev };
    spans.extend(faded(hint(shown, &leader), alpha));
    if let Some(msg) = &app.status {
        let (visible, entered) = if anim::fades() {
            anim::message(now.saturating_duration_since(msg.at), super::STATUS_FOR)
        } else {
            (1.0, 1.0)
        };
        let tone = match msg.tone {
            super::Tone::Done => p().green,
            super::Tone::Error => p().red,
            super::Tone::Info => p().yellow,
        };
        let color = anim::lerp(p().panel, tone, visible);
        let travel = ((1.0 - entered) * f32::from(anim::MESSAGE_CELLS)).round() as usize;
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let text = format!("{} ", msg.text);
        let room = (r.width as usize).saturating_sub(used);
        let n = text.chars().count();
        if visible > 0.0 && n <= room {
            let travel = travel.min(n);
            spans.push(Span::raw(" ".repeat(room - n + travel)));
            spans.push(Span::styled(
                text.chars().take(n - travel).collect::<String>(),
                Style::default().fg(color),
            ));
        } else if visible > 0.0 {
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

fn chip_color(mode: Mode) -> Color {
    chip(mode, "").1
}

fn empty(f: &mut Frame, app: &App) {
    let stage = trect(app.stage());
    let leader = app.leader.label();
    let msg = format!(
        "{leader} {} opens a folder · {leader} 1–9 a workspace from the sidebar",
        key("open")
    );
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
    sel: Option<&Sel>,
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
    let sel = sel.map(|s| (s, s.base(screen.scrollback())));
    screen_to(
        f.buffer_mut(),
        screen,
        inner,
        app.leading() && !focused,
        sel,
    );
    if focused && !app.leading() && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < inner.height && col < inner.width {
            f.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
}

/// Copies a vt100 screen into the buffer. `dim` fades it, for panes that
/// aren't taking keys while navigating; `sel` is the selection and the line
/// on the first row, which it paints.
fn screen_to(
    buf: &mut Buffer,
    screen: &vt100::Screen,
    area: TRect,
    dim: bool,
    sel: Option<(&Sel, usize)>,
) {
    for row in 0..area.height {
        let picked = sel.and_then(|(s, base)| s.cols_on(base + usize::from(row), area.width));
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
            if picked.is_some_and(|(from, to)| (from..to).contains(&col)) {
                style = style
                    .remove_modifier(Modifier::REVERSED)
                    .fg(p().bg)
                    .bg(p().blue);
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

/// The leader's popup: every key, grouped go / worktree / panes (REQ-31),
/// with the keys the settings give them.
fn which(f: &mut Frame, app: &App) {
    let fixed: [(&str, &[(&str, &str)]); 3] = [
        (
            "go",
            &[("1-9", "workspace"), ("↑↓←→", "move"), ("q", "detach")],
        ),
        ("worktree", &[]),
        ("panes", &[("hjkl", "go"), ("HJKL", "move")]),
    ];
    let groups: Vec<(&str, Vec<(String, &str)>)> = fixed
        .iter()
        .map(|(g, extra)| {
            let mut keys: Vec<(String, &str)> = crate::settings::ACTIONS
                .iter()
                .filter(|a| a.0 == *g)
                .map(|a| (key(a.1), a.2))
                .collect();
            keys.extend(extra.iter().map(|(k, w)| (k.to_string(), *w)));
            (*g, keys)
        })
        .collect();
    let rows = groups.iter().map(|(_, k)| k.len()).max().unwrap_or(0) as u16;
    let col = 17u16;
    let area = f.area();
    let w = (col * groups.len() as u16 + 3).min(area.width);
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
    for (i, (title, keys)) in groups.iter().enumerate() {
        let mut lines = vec![Line::from(Span::styled(
            *title,
            Style::default().fg(p().yellow).add_modifier(Modifier::BOLD),
        ))];
        for (k, what) in keys {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::select::Pos;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn every_mode_has_a_hint_and_the_leader_names_the_useful_keys() {
        for mode in [
            Mode::Set,
            Mode::SetKey,
            Mode::Confirm,
            Mode::Leader,
            Mode::Move,
            Mode::Diff,
            Mode::Md,
            Mode::Term,
        ] {
            assert!(!hint(mode, "^␣").is_empty(), "{mode:?}");
        }
        let leader = text(&hint(Mode::Leader, "^␣"));
        for want in ["switch", "open", "worktree", "hjkl", "all keys", "esc"] {
            assert!(leader.contains(want), "{leader}");
        }
        assert!(text(&hint(Mode::Term, "^␣")).starts_with(" ^␣ then a key"));
    }

    #[test]
    fn a_hint_fades_toward_the_bar_and_a_full_one_is_untouched() {
        let spans = hint(Mode::Confirm, "^␣");
        assert_eq!(faded(spans.clone(), 1.0), spans);
        let gone = faded(spans, 0.0);
        assert!(gone.iter().all(|s| s.style.fg == Some(p().panel)));
    }

    fn staged() -> (Buffer, TRect, TRect) {
        let area = TRect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let style = Style::default()
            .bg(Color::Rgb(0, 0, 0))
            .fg(Color::Rgb(200, 200, 200));
        for y in 0..3 {
            buf.set_string(0, y, "abcdefghijkl", style);
        }
        (buf, TRect::new(0, 0, 12, 1), TRect::new(0, 1, 12, 2))
    }

    #[test]
    fn a_border_eases_between_the_two_accents_and_leaves_the_title_alone() {
        let area = TRect::new(0, 0, 6, 3);
        let mut buf = Buffer::empty(area);
        let to = Color::Rgb(100, 100, 100);
        let from = Color::Rgb(0, 0, 0);
        let style = Style::default().fg(to);
        buf.set_string(0, 0, "╭ab──╮", style);
        buf.set_string(0, 1, "│    │", style);
        buf.set_string(0, 2, "╰────╯", style);
        ease_border(&mut buf, area, from, to, 0.5);
        let half = Color::Rgb(50, 50, 50);
        assert_eq!(buf[(0, 0)].fg, half);
        assert_eq!(buf[(5, 1)].fg, half);
        assert_eq!(buf[(2, 2)].fg, half);
        assert_eq!(buf[(1, 0)].fg, to, "title text keeps its own colour");
        let mut done = Buffer::empty(area);
        done.set_string(0, 0, "╭ab──╮", style);
        ease_border(&mut done, area, from, to, 1.0);
        assert_eq!(done[(0, 0)].fg, to);
    }

    #[test]
    fn a_popup_dissolves_in_over_what_was_under_it() {
        let (base, _, _) = staged();
        let mut over = base.clone();
        let style = Style::default()
            .bg(Color::Rgb(100, 100, 100))
            .fg(Color::Rgb(255, 255, 255));
        over.set_string(2, 1, "XY", style);
        let mut shown = over.clone();
        dissolve(&mut shown, &base, 1.0);
        assert_eq!(shown, over, "done: the overlay as drawn");
        let mut shown = over.clone();
        dissolve(&mut shown, &base, 0.0);
        assert_eq!(shown, base, "start: nothing yet");
        let mut shown = over.clone();
        dissolve(&mut shown, &base, 0.5);
        assert_eq!(shown[(2, 1)].symbol(), "X");
        assert_eq!(shown[(2, 1)].bg, Color::Rgb(50, 50, 50));
        assert_eq!(shown[(0, 1)], base[(0, 1)], "cells the overlay left alone");
    }

    #[test]
    fn a_finished_transition_leaves_the_frame_as_drawn() {
        let (mut buf, header, stage) = staged();
        let before = buf.clone();
        enter_transition(&mut buf, header, stage, 1.0, 1);
        assert_eq!(buf, before);
    }

    #[test]
    fn the_panes_enter_from_the_side_the_move_came_from() {
        let (mut buf, header, stage) = staged();
        enter_transition(&mut buf, header, stage, 0.0, 1);
        let n = usize::from(anim::SLIDE_CELLS);
        let row: String = (0..12).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert_eq!(
            &row[..n],
            " ".repeat(n),
            "moving down: content comes in from the right"
        );
        assert_eq!(&row[n..], &"abcdefghijkl"[..12 - n]);
        assert_eq!(
            buf[(5, 0)].symbol(),
            "f",
            "the header only fades, it does not move"
        );

        let (mut buf, header, stage) = staged();
        enter_transition(&mut buf, header, stage, 0.0, -1);
        let row: String = (0..12).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert_eq!(&row[..12 - n], &"abcdefghijkl"[n..]);
    }

    #[test]
    fn text_fades_in_from_its_own_background() {
        let (mut buf, header, stage) = staged();
        enter_transition(&mut buf, header, stage, 0.0, 1);
        assert_eq!(
            buf[(6, 0)].fg,
            Color::Rgb(0, 0, 0),
            "invisible at the start"
        );
        let (mut buf, header, stage) = staged();
        enter_transition(&mut buf, header, stage, 0.5, 1);
        assert_eq!(buf[(6, 0)].fg, Color::Rgb(100, 100, 100));
    }

    #[test]
    fn the_selection_is_painted_on_its_cells_only() {
        let mut parser = vt100::Parser::new(2, 8, 10);
        parser.process(b"one\r\ntwo");
        let at = |line, col| Pos { line, col };
        let sel = Sel::new(1, &mut parser, at(0, 1), at(1, 0));
        let area = TRect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        screen_to(
            &mut buf,
            parser.screen(),
            area,
            false,
            Some((&sel, sel.base(0))),
        );
        let painted = |x, y| buf[(x, y)].bg == p().blue;
        assert!(!painted(0, 0), "before the start");
        assert!(painted(1, 0) && painted(7, 0), "the rest of the first row");
        assert!(painted(0, 1), "up to the head, inclusive");
        assert!(!painted(1, 1), "after the head");
    }

    #[test]
    fn the_banner_counts_characters() {
        assert!(banner_text(1).contains("1 character "));
        assert!(banner_text(12).contains("12 characters "));
    }
}
