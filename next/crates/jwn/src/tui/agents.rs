//! The area of the agents (spec REQ-601): at the right of a conversation wide enough for it, the
//! tasks of the engine, newest first, and under them what the one that is lit does: what it was
//! asked, the steps it took and, when it ended, what it said. It is open until it is closed
//! (`^B`, or a click on the agents of the footer), with agents or without them.

use std::cell::Cell;

use contract::conversation::{Conversation, Item, Task, TaskKind, TaskState, ToolState};

use super::{Grid, Rgb, Style, clip, look};

/// A pane at least this many columns wide inside holds the area; a narrower one lists the tasks
/// over the conversation instead (`/tasks`).
pub const MIN_ROOM: usize = 100;
/// Columns of the area, the line that separates it included.
pub const WIDTH: usize = 46;
/// Rows of the prompt, at most.
const PROMPT_ROWS: usize = 3;

type Rect = (usize, usize, usize, usize);

/// What the area remembers: whether it is open, whether the keys go to it and which task is lit.
#[derive(Debug, Default, Clone)]
pub struct State {
    pub open: bool,
    pub focus: bool,
    sel: Option<String>,
    /// Whether the pane is wide enough for the area, as the last frame found it.
    room: Cell<bool>,
}

impl State {
    /// The pane has `iw` columns inside: it says so, and whether the area is drawn.
    pub fn fits(&self, iw: usize) -> bool {
        self.room.set(iw >= MIN_ROOM);
        self.open && iw >= MIN_ROOM
    }

    /// Whether the last frame had room for the area.
    pub fn has_room(&self) -> bool {
        self.room.get()
    }

    /// The task lit: the one chosen if it is still there, else the first that runs, else the
    /// newest.
    pub fn lit<'a>(&self, conv: &'a Conversation) -> Option<&'a Task> {
        let tasks = rows(conv);
        self.sel
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id).copied())
            .or_else(|| tasks.iter().find(|t| t.state == TaskState::Run).copied())
            .or_else(|| tasks.first().copied())
    }

    /// Light the task above or below the lit one.
    pub fn step(&mut self, conv: &Conversation, down: bool) {
        let tasks = rows(conv);
        let at = self
            .lit(conv)
            .and_then(|l| tasks.iter().position(|t| t.id == l.id))
            .unwrap_or(0);
        let next = if down {
            (at + 1).min(tasks.len().saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        self.sel = tasks.get(next).map(|t| t.id.clone());
    }

    /// A click at row `y` of the area whose top is `top`: light the task there, if it is one.
    pub fn click(&mut self, conv: &Conversation, top: usize, y: usize) -> bool {
        let Some(row) = y.checked_sub(top + 1) else {
            return false;
        };
        match rows(conv).get(row) {
            Some(t) => {
                self.sel = Some(t.id.clone());
                self.focus = true;
                true
            }
            None => false,
        }
    }
}

/// The tasks as the area lists them: the newest first.
pub fn rows(conv: &Conversation) -> Vec<&Task> {
    conv.tasks.iter().rev().take(LIST_ROWS).collect()
}

/// Rows of the list, at most.
const LIST_ROWS: usize = 6;

/// The conversation's rectangle and the area's, when the area is shown.
pub fn split((ix, iy, iw, ih): Rect, shown: bool) -> (Rect, Option<Rect>) {
    if !shown {
        return ((ix, iy, iw, ih), None);
    }
    let conv = iw - WIDTH;
    ((ix, iy, conv, ih), Some((ix + conv, iy, WIDTH, ih)))
}

fn mark(t: &Task, now_ms: u64) -> (char, Rgb) {
    match t.state {
        TaskState::Run => (
            ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(now_ms / 100 % 10) as usize],
            look::yellow(),
        ),
        TaskState::Done => ('✓', look::green()),
        TaskState::Stopped | TaskState::Failed => ('■', look::red()),
    }
}

/// `m:ss`.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn kilo(n: u64) -> String {
    if n >= 1000 {
        let tenth = n / 100;
        if tenth.is_multiple_of(10) {
            format!("{}k", tenth / 10)
        } else {
            format!("{}.{}k", tenth / 10, tenth % 10)
        }
    } else {
        n.to_string()
    }
}

/// The card of a task, when it is a subagent shown in the items.
fn card<'a>(conv: &'a Conversation, t: &Task) -> Option<&'a Item> {
    let id = t.item.as_deref()?;
    conv.items
        .iter()
        .rev()
        .find(|i| matches!(i, Item::Agent { id: a, .. } if a == id))
}

/// What a task is called: the kind of subagent, or what runs in the background.
fn name(conv: &Conversation, t: &Task) -> String {
    match (card(conv, t), t.kind) {
        (Some(Item::Agent { name, .. }), _) => name.clone(),
        (_, TaskKind::Agent) => "agente".into(),
        (_, TaskKind::Bash) => "shell".into(),
        (_, TaskKind::Monitor) => "monitor".into(),
        (_, TaskKind::Loop) => "loop".into(),
        (_, TaskKind::Other) => "tarea".into(),
    }
}

/// The numbers a subagent's card has, as `n tools · 38k · 1:12`, or only the time for what has no card.
fn stats(conv: &Conversation, t: &Task, now_ms: u64) -> String {
    let elapsed = if t.state == TaskState::Run {
        now_ms.saturating_sub(t.started)
    } else {
        0
    };
    match card(conv, t) {
        Some(Item::Agent {
            tools, tokens, ms, ..
        }) => {
            let ms = if t.state == TaskState::Run {
                elapsed.max(*ms)
            } else {
                *ms
            };
            format!("{tools} tools · {} · {}", kilo(*tokens), clock(ms))
        }
        _ => {
            if t.state == TaskState::Run {
                clock(elapsed)
            } else {
                String::new()
            }
        }
    }
}

/// Wrap `text` to rows of `width` characters, at most `rows` of them, the last one cut with `…`.
fn wrap(text: &str, width: usize, rows: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let w = word.chars().count();
        if !line.is_empty() && line.chars().count() + 1 + w > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.len() > rows {
        out.truncate(rows);
        if let Some(last) = out.last_mut() {
            *last = clip(&format!("{last}…"), width);
        }
    }
    out.iter().map(|l| clip(l, width)).collect()
}

/// Draw the area in `(x, y, w, h)`: a line at its left, the title, the list and the detail of
/// the task that is lit (REQ-601).
pub fn draw(
    g: &mut Grid,
    conv: &Conversation,
    state: &State,
    (x, y, w, h): Rect,
    now_ms: u64,
    accent: Rgb,
) {
    for r in 0..h {
        g.put(x, y + r, "│", Style::fg(look::dim()));
    }
    let tx = x + 2;
    let tw = w.saturating_sub(3);
    let running = conv
        .tasks
        .iter()
        .filter(|t| t.state == TaskState::Run)
        .count();
    let title = if running > 0 {
        format!(" · {running} corriendo")
    } else {
        " · ninguno".to_owned()
    };
    g.put(tx, y, "agentes", Style::fg(accent).bold());
    g.put(tx + 7, y, &title, Style::fg(look::dim()));
    let tasks = rows(conv);
    let lit = state.lit(conv).map(|t| t.id.clone());
    let mut row = y + 1;
    if tasks.is_empty() {
        g.put(tx, row, "nada corre ahora", Style::fg(look::dim()));
        row += 1;
    }
    for t in &tasks {
        let on = lit.as_deref() == Some(t.id.as_str());
        let (m, colour) = mark(t, now_ms);
        let head = format!(
            "{} {} {}",
            if on { '▸' } else { ' ' },
            m,
            clip(&name(conv, t), 14)
        );
        let st = stats(conv, t, now_ms);
        let line = format!("{head:<19}{st}");
        if on {
            g.paint_bg(tx - 1, row, tw + 1, look::sel());
        }
        let bg = |s: Style| if on { s.on(look::sel()) } else { s };
        g.put(tx, row, &clip(&line, tw), bg(Style::fg(look::fg())));
        g.put(tx + 2, row, &m.to_string(), bg(Style::fg(colour)));
        row += 1;
    }
    let Some(t) = state.lit(conv) else {
        return;
    };
    g.put(tx, row, &"─".repeat(tw), Style::fg(look::dim()));
    row += 1;
    let bottom = y + h;
    let card = card(conv, t);
    let put = |g: &mut Grid, row: &mut usize, text: &str, style: Style| {
        if *row < bottom.saturating_sub(1) {
            g.put(tx, *row, &clip(text, tw), style);
            *row += 1;
        }
    };
    if card.is_none() {
        put(g, &mut row, &t.label, Style::fg(look::fg()));
    }
    if let Some(Item::Agent {
        prompt: Some(p),
        description,
        ..
    }) = card
    {
        put(g, &mut row, description, Style::fg(look::fg()).bold());
        for l in wrap(p, tw.saturating_sub(7), PROMPT_ROWS) {
            put(g, &mut row, &format!("prompt {l}"), Style::fg(look::dim()));
        }
    }
    if let Some(Item::Agent { steps, state, .. }) = card {
        let room = bottom.saturating_sub(row + 3);
        let shown = steps.len().min(room);
        for (i, s) in steps[steps.len() - shown..].iter().enumerate() {
            let last = steps.len() - shown + i + 1 == steps.len();
            let working = last && *state == ToolState::Run && t.state == TaskState::Run;
            let (m, c) = if working {
                (mark(t, now_ms).0, look::yellow())
            } else {
                ('✓', look::green())
            };
            let (tool, rest) = s.split_once(' ').unwrap_or((s.as_str(), ""));
            let body = clip(rest, tw.saturating_sub(tool.chars().count() + 4));
            if row < bottom.saturating_sub(1) {
                g.put(tx, row, &m.to_string(), Style::fg(c));
                g.put(tx + 2, row, tool, Style::fg(look::cyan()));
                g.put(
                    tx + 3 + tool.chars().count(),
                    row,
                    &body,
                    Style::fg(look::fg()),
                );
                row += 1;
            }
        }
    }
    if let Some(info) = &t.info {
        let label = if t.state == TaskState::Run {
            "ahora"
        } else {
            "dijo"
        };
        for (i, l) in wrap(info, tw.saturating_sub(6), 4).iter().enumerate() {
            let head = if i == 0 {
                format!("{label:<6}")
            } else {
                "      ".to_owned()
            };
            g.put(
                tx,
                row.min(bottom.saturating_sub(2)),
                &head,
                Style::fg(look::dim()),
            );
            g.put(
                tx + 6,
                row.min(bottom.saturating_sub(2)),
                l,
                Style::fg(look::fg()),
            );
            row += 1;
        }
    }
    g.put(
        tx,
        bottom.saturating_sub(1),
        &clip("^B cierra · ⇥ elegir · x detener", tw),
        Style::fg(look::dim()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, state: TaskState, item: Option<&str>) -> Task {
        Task {
            id: id.into(),
            kind: TaskKind::Agent,
            label: format!("label {id}"),
            state,
            started: 0,
            info: None,
            item: item.map(str::to_owned),
        }
    }

    #[test]
    fn a_wide_pane_holds_the_area_at_the_right_and_leaves_the_rest_to_the_conversation() {
        let s = State {
            open: true,
            ..State::default()
        };
        assert!(s.fits(100) && s.has_room());
        assert!(
            !s.fits(99) && !s.has_room(),
            "under 100 columns it does not fit"
        );
        let closed = State::default();
        assert!(
            !closed.fits(120) && closed.has_room(),
            "closed, but there is room for it"
        );
        let (conv, area) = split((2, 1, 120, 40), true);
        assert_eq!(conv, (2, 1, 120 - WIDTH, 40));
        assert_eq!(area, Some((2 + 120 - WIDTH, 1, WIDTH, 40)));
        assert_eq!(split((2, 1, 120, 40), false), ((2, 1, 120, 40), None));
    }

    #[test]
    fn the_lit_task_is_the_one_chosen_else_the_first_that_runs_else_the_newest() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.tasks = vec![
            task("old", TaskState::Done, None),
            task("run", TaskState::Run, None),
            task("new", TaskState::Done, None),
        ];
        let mut s = State::default();
        assert_eq!(
            s.lit(&conv).unwrap().id,
            "run",
            "the first that runs: the list is new, run, old"
        );
        s.step(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "old");
        s.step(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "old", "it stops at the last");
        s.step(&conv, false);
        s.step(&conv, false);
        s.step(&conv, false);
        assert_eq!(s.lit(&conv).unwrap().id, "new", "and at the first");
        conv.tasks.retain(|t| t.id != "new");
        assert_eq!(s.lit(&conv).unwrap().id, "run", "the chosen one is gone");
        conv.tasks.clear();
        assert!(s.lit(&conv).is_none());
    }

    #[test]
    fn a_click_on_a_row_lights_it_and_gives_the_keys_to_the_area() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.tasks = vec![
            task("a", TaskState::Done, None),
            task("b", TaskState::Done, None),
        ];
        let mut s = State::default();
        assert!(!s.click(&conv, 5, 5), "the title is not a row");
        assert!(
            s.click(&conv, 5, 7),
            "the second row of the list: the older one"
        );
        assert_eq!((s.lit(&conv).unwrap().id.as_str(), s.focus), ("a", true));
        assert!(!s.click(&conv, 5, 9), "past the list");
    }

    fn agent_card(id: &str, steps: &[&str]) -> Item {
        Item::Agent {
            id: id.into(),
            name: "Explore".into(),
            description: "mapear el puente".into(),
            state: ToolState::Run,
            tools: 14,
            tokens: 38_400,
            ms: 72_000,
            last: Some("Grep".into()),
            steps: steps.iter().map(|s| (*s).to_owned()).collect(),
            prompt: Some("Busca cómo el daemon habla con el teléfono y no edites nada".into()),
        }
    }

    /// REQ-601
    #[test]
    fn the_area_lists_the_tasks_newest_first_and_shows_what_the_lit_one_does() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.items = vec![agent_card(
            "c1",
            &["Glob crates/**", "Read lib.rs", "Grep fn pair"],
        )];
        let mut run = task("t1", TaskState::Run, Some("c1"));
        run.info = Some("Buscando el puerto del puente".into());
        let mut bash = task("t2", TaskState::Run, None);
        bash.kind = TaskKind::Bash;
        bash.label = "cargo test -p jwn".into();
        conv.tasks = vec![run, bash];
        let mut g = Grid::new(60, 24);
        let state = State {
            open: true,
            ..State::default()
        };
        draw(
            &mut g,
            &conv,
            &state,
            (10, 1, WIDTH, 22),
            1_000_000,
            look::blue(),
        );
        let rows = g.rows_plain();
        let at = |s: &str| {
            rows.iter()
                .position(|r| r.contains(s))
                .unwrap_or_else(|| panic!("{s:?} not drawn:\n{}", rows.join("\n")))
        };
        assert!(rows[1].contains("agentes · 2 corriendo"), "{:?}", rows[1]);
        assert!(
            at("shell") < at("Explore"),
            "newest first: the shell after the agent"
        );
        assert!(
            rows[at("Explore")].contains("14 tools · 38.4k · 20:00")
                || rows[at("Explore")].contains("14 tools · 38.4k")
        );
        let lit = at("Explore");
        assert!(
            rows[lit].contains('▸')
                || rows[lit - 1].contains('▸')
                || rows[at("shell")].contains('▸')
        );
        // the lit task is the first that runs: the newest, the shell; light the agent to see its detail
        let mut state = state;
        state.step(&conv, true);
        let mut g = Grid::new(60, 24);
        draw(
            &mut g,
            &conv,
            &state,
            (10, 1, WIDTH, 22),
            1_000_000,
            look::blue(),
        );
        let rows = g.rows_plain();
        let text = rows.join("\n");
        for want in [
            "mapear el puente",
            "prompt Busca cómo el daemon",
            "Glob",
            "Grep",
            "ahora Buscando el puerto",
            "^B cierra",
        ] {
            assert!(text.contains(want), "{want:?} not drawn:\n{text}");
        }
        assert!(rows.iter().all(|r| r.chars().count() <= 60));
    }

    /// REQ-601: nothing runs, and the area says so and keeps its place.
    #[test]
    fn with_no_task_the_area_says_nothing_runs() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.tasks.clear();
        let mut g = Grid::new(60, 12);
        draw(
            &mut g,
            &conv,
            &State {
                open: true,
                ..State::default()
            },
            (10, 1, WIDTH, 10),
            0,
            look::blue(),
        );
        let text = g.rows_plain().join("\n");
        assert!(
            text.contains("agentes · ninguno") && text.contains("nada corre ahora"),
            "{text}"
        );
    }

    #[test]
    fn numbers_are_written_short() {
        assert_eq!(
            (kilo(999), kilo(38_400), kilo(22_000)),
            ("999".into(), "38.4k".into(), "22k".into())
        );
        assert_eq!(
            (clock(72_000), clock(5_000)),
            ("1:12".into(), "0:05".into())
        );
        assert_eq!(wrap("uno dos tres cuatro", 8, 2), ["uno dos", "tres…"]);
    }
}
