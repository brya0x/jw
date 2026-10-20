//! The area of the agents (spec REQ-601): at the right of a conversation wide enough for it, the
//! subagents of the engine, newest first, one line for each (`name (model) → what it does`, and its
//! time and tokens at the right), and under them, in sections, what the one that is lit was asked,
//! the steps it took and what it does or said. It is open until it is closed (`^B`, or a click on
//! the agents of the footer), with agents or without them. What runs in the background and is
//! not a subagent (a shell, a monitor) is in `/tasks`.

use std::cell::Cell;

use contract::conversation::{Conversation, Item, Task, TaskKind, TaskState};

use super::{Grid, Rgb, Style, clip, look};

/// A pane at least this many columns wide inside holds the area; a narrower one lists the tasks
/// over the conversation instead (`/tasks`).
pub const MIN_ROOM: usize = 100;
/// Columns of the area, the line that separates it included, in a panel `iw` wide inside: more
/// room for what each agent does when the panel has it.
pub fn width(iw: usize) -> usize {
    if iw >= 150 { 60 } else { 46 }
}

/// Rows of the prompt and of what an agent said, at most.
const PROMPT_ROWS: usize = 3;
const SAID_ROWS: usize = 4;

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

/// The subagents as the area lists them: the newest first.
pub fn rows(conv: &Conversation) -> Vec<&Task> {
    conv.tasks
        .iter()
        .rev()
        .filter(|t| t.kind == TaskKind::Agent)
        .take(LIST_ROWS)
        .collect()
}

/// How many subagents run.
pub fn running(conv: &Conversation) -> usize {
    conv.tasks
        .iter()
        .filter(|t| t.kind == TaskKind::Agent && t.state == TaskState::Run)
        .count()
}

/// Rows of the list, at most.
const LIST_ROWS: usize = 8;

/// The conversation's rectangle and the area's, when the area is shown.
pub fn split((ix, iy, iw, ih): Rect, shown: bool) -> (Rect, Option<Rect>) {
    if !shown {
        return ((ix, iy, iw, ih), None);
    }
    let area = width(iw);
    let conv = iw - area;
    ((ix, iy, conv, ih), Some((ix + conv, iy, area, ih)))
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

/// The kind of subagent and the model it runs with: `Explore (haiku-5.5)`.
fn name(conv: &Conversation, t: &Task) -> String {
    match card(conv, t) {
        Some(Item::Agent {
            name,
            model: Some(m),
            ..
        }) => format!("{name} ({m})"),
        Some(Item::Agent { name, .. }) => name.clone(),
        _ => "agente".to_owned(),
    }
}

/// What it does: what the engine says it is doing, else its last step; when it ended, how.
fn doing(conv: &Conversation, t: &Task) -> String {
    match t.state {
        TaskState::Run => t
            .info
            .clone()
            .or_else(|| match card(conv, t) {
                Some(Item::Agent { steps, .. }) => steps.last().cloned(),
                _ => None,
            })
            .unwrap_or_else(|| "empezando".to_owned()),
        TaskState::Done => "listo".to_owned(),
        TaskState::Stopped => "detenido".to_owned(),
        TaskState::Failed => "falló".to_owned(),
    }
}

/// Its time and tokens: `1:12 · 38k`.
fn numbers(conv: &Conversation, t: &Task, now_ms: u64) -> String {
    let elapsed = now_ms.saturating_sub(t.started);
    match card(conv, t) {
        Some(Item::Agent { tokens, ms, .. }) => {
            let ms = if t.state == TaskState::Run {
                elapsed.max(*ms)
            } else {
                *ms
            };
            format!("{} · {}", clock(ms), kilo(*tokens))
        }
        _ if t.state == TaskState::Run => clock(elapsed),
        _ => String::new(),
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

/// Draw the area in `(x, y, w, h)`: a line at its left, the title, a line for each subagent and
/// the detail of the one that is lit in sections (REQ-601).
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
    let bottom = y + h;
    let running = running(conv);
    g.put(tx, y, "subagentes", Style::fg(accent).bold());
    let how_many = if running > 0 {
        format!(" · {running} corriendo")
    } else {
        " · ninguno".to_owned()
    };
    g.put(tx + 10, y, &how_many, Style::fg(look::dim()));
    let tasks = rows(conv);
    let lit = state.lit(conv).map(|t| t.id.clone());
    let mut row = y + 1;
    if tasks.is_empty() {
        g.put(tx, row, "ninguno ahora", Style::fg(look::dim()));
        row += 1;
    }
    for t in &tasks {
        let on = lit.as_deref() == Some(t.id.as_str());
        let (m, colour) = mark(t, now_ms);
        let right = numbers(conv, t, now_ms);
        let left_room = tw.saturating_sub(right.chars().count() + 1);
        let head = format!("{} {} {}", if on { '▸' } else { ' ' }, m, name(conv, t));
        let head = clip(&head, left_room);
        let used = head.chars().count();
        let arrow = format!(" → {}", doing(conv, t));
        let arrow = clip(&arrow, left_room.saturating_sub(used));
        if on {
            g.paint_bg(tx - 1, row, tw + 1, look::sel());
        }
        let on_bg = |st: Style| if on { st.on(look::sel()) } else { st };
        g.put(tx, row, &head, on_bg(Style::fg(look::fg()).bold()));
        g.put(tx + 2, row, &m.to_string(), on_bg(Style::fg(colour).bold()));
        g.put(tx + used, row, &arrow, on_bg(Style::fg(look::dim())));
        let at = tx + tw - right.chars().count();
        g.put(at, row, &right, on_bg(Style::fg(look::dim())));
        row += 1;
    }
    let Some(t) = state.lit(conv) else {
        return;
    };
    g.put(tx, row, &"─".repeat(tw), Style::fg(look::dim()));
    row += 1;
    let hint = row_hint(bottom);
    let card = card(conv, t);
    let (prompt, description, steps) = match card {
        Some(Item::Agent {
            prompt,
            description,
            steps,
            ..
        }) => (prompt.as_deref(), description.as_str(), steps.as_slice()),
        _ => (None, t.label.as_str(), &[][..]),
    };
    let prompt_rows = prompt.map_or(Vec::new(), |p| wrap(p, tw.saturating_sub(2), PROMPT_ROWS));
    let said_rows = t
        .info
        .as_deref()
        .map_or(Vec::new(), |i| wrap(i, tw.saturating_sub(2), SAID_ROWS));
    let section = |n: usize| if n == 0 { 0 } else { n + 1 };
    let reserved = 1 + section(prompt_rows.len()) + section(said_rows.len());
    let room = hint.saturating_sub(row).saturating_sub(reserved);
    let shown = steps.len().min(room.saturating_sub(1));
    let put = |g: &mut Grid, row: &mut usize, dx: usize, text: &str, st: Style| {
        if *row < hint {
            g.put(tx + dx, *row, &clip(text, tw.saturating_sub(dx)), st);
        }
    };
    put(g, &mut row, 0, description, Style::fg(look::fg()).bold());
    row += 1;
    if !prompt_rows.is_empty() {
        put(g, &mut row, 0, "prompt", Style::fg(look::dim()));
        row += 1;
        for l in &prompt_rows {
            put(g, &mut row, 2, l, Style::fg(look::fg()));
            row += 1;
        }
    }
    if shown > 0 {
        put(g, &mut row, 0, "pasos", Style::fg(look::dim()));
        row += 1;
        for (i, s) in steps[steps.len() - shown..].iter().enumerate() {
            let last = steps.len() - shown + i + 1 == steps.len();
            let (m, c) = if last && t.state == TaskState::Run {
                (mark(t, now_ms).0, look::yellow())
            } else {
                ('✓', look::green())
            };
            let (tool, rest) = s.split_once(' ').unwrap_or((s.as_str(), ""));
            put(g, &mut row, 2, &m.to_string(), Style::fg(c));
            put(g, &mut row, 4, tool, Style::fg(look::cyan()));
            let at = 5 + tool.chars().count();
            put(g, &mut row, at, rest, Style::fg(look::fg()));
            row += 1;
        }
    }
    if !said_rows.is_empty() {
        let title = if t.state == TaskState::Run {
            "ahora"
        } else {
            "dijo"
        };
        put(g, &mut row, 0, title, Style::fg(look::dim()));
        row += 1;
        for l in &said_rows {
            put(g, &mut row, 2, l, Style::fg(look::fg()));
            row += 1;
        }
    }
    g.put(
        tx,
        hint,
        &clip("^B cierra · ⇥ elegir · x detener", tw),
        Style::fg(look::dim()),
    );
}

/// The row of the keys at the foot of the area.
fn row_hint(bottom: usize) -> usize {
    bottom.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use contract::conversation::ToolState;

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
        assert_eq!(conv, (2, 1, 120 - width(120), 40));
        assert_eq!(area, Some((2 + 120 - width(120), 1, width(120), 40)));
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
            model: Some("haiku-5.5".into()),
        }
    }

    /// REQ-601: a line for each subagent, `name (model) → what it does` and its time and tokens at
    /// the right; what is not a subagent is not there; the detail of the lit one is in sections.
    #[test]
    fn the_area_has_a_line_for_each_subagent_and_the_detail_of_the_lit_one_in_sections() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.items = vec![agent_card(
            "c1",
            &["Glob crates/**", "Read lib.rs", "Grep fn pair"],
        )];
        let mut run = task("t1", TaskState::Run, Some("c1"));
        run.info = Some("Buscando el puerto del puente".into());
        let mut done = task("t0", TaskState::Done, None);
        done.started = 0;
        let mut bash = task("t2", TaskState::Run, None);
        bash.kind = TaskKind::Bash;
        bash.label = "cargo test -p jwn".into();
        conv.tasks = vec![done, run, bash];
        let state = State {
            open: true,
            ..State::default()
        };
        let mut g = Grid::new(80, 24);
        draw(
            &mut g,
            &conv,
            &state,
            (10, 1, 60, 22),
            1_000_000,
            look::blue(),
        );
        let rows = g.rows_plain();
        let text = rows.join("\n");
        assert!(rows[1].contains("subagentes · 1 corriendo"), "{text}");
        assert!(
            !text.contains("cargo test") && !text.contains("shell"),
            "a shell is not a subagent:\n{text}"
        );
        let line = rows
            .iter()
            .find(|r| r.contains("Explore (haiku-5.5)"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(line.contains("→ Buscando el pue"), "{line:?}");
        assert!(
            line.trim_end().ends_with("16:40 · 38.4k"),
            "time and tokens at the right: {line:?}"
        );
        assert!(line.contains('▸'), "the one that runs is lit");
        assert!(
            rows.iter()
                .any(|r| r.contains("agente") && r.contains("→ listo")),
            "the one that ended says so:\n{text}"
        );
        for want in [
            "mapear el puente",
            "prompt",
            "  Busca cómo el daemon",
            "pasos",
            "Glob",
            "Grep",
            "ahora",
            "  Buscando el puerto",
            "^B cierra",
        ] {
            assert!(text.contains(want), "{want:?} not drawn:\n{text}");
        }
        assert!(
            text.find("prompt").unwrap() < text.find("pasos").unwrap()
                && text.find("pasos").unwrap() < text.find("ahora").unwrap(),
            "in that order:\n{text}"
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 80));
        // a narrower line gives the end of what it does up before the numbers
        let mut g = Grid::new(60, 24);
        draw(
            &mut g,
            &conv,
            &state,
            (10, 1, 46, 22),
            1_000_000,
            look::blue(),
        );
        let line = g
            .rows_plain()
            .into_iter()
            .find(|r| r.contains("Explore"))
            .unwrap();
        assert!(
            line.contains('…') && line.trim_end().ends_with("38.4k"),
            "{line:?}"
        );
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
            (10, 1, width(120), 10),
            0,
            look::blue(),
        );
        let text = g.rows_plain().join("\n");
        assert!(
            text.contains("subagentes · ninguno") && text.contains("ninguno ahora"),
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
