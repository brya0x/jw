//! The area of what runs (spec REQ-601): at the right of a conversation wide enough for it, what
//! the engine runs while the person works, in three sections: the subagents (a line for each,
//! `name (model) → what it does`, and its time and tokens at the right), the shells Claude runs in
//! the background and the monitors (and the loops that repeat). Under them, in sections, what the
//! one that is lit is: what a subagent was asked and its steps, what a shell or a monitor runs. It
//! is open until it is closed (`^B`, or a click on the agents of the footer) and has the keys
//! while it is open: `↑` `↓` walk every line, `←` `→` jump a section, `x` stops, `⇥` gives the keys
//! to the field and `esc` closes it.

use std::cell::Cell;

use contract::conversation::{Conversation, Item, Task, TaskKind, TaskState};

use super::{Grid, Rgb, Style, clip, look};

/// A pane at least this many columns wide inside holds the area; a narrower one lists the tasks
/// over the conversation instead (`/tasks`).
pub const MIN_ROOM: usize = 100;
/// Columns of the area, the line that separates it included, in a panel `iw` wide inside: more
/// room for what each one does when the panel has it.
pub fn width(iw: usize) -> usize {
    if iw >= 150 { 60 } else { 46 }
}

/// Rows of the prompt and of what a task said, at most.
const PROMPT_ROWS: usize = 3;
const SAID_ROWS: usize = 4;
/// Lines of what a shell or a monitor printed, at most (REQ-607).
const TAIL_ROWS: usize = 6;
/// Lines of one section, at most: the newest.
const SECTION_ROWS: usize = 6;

type Rect = (usize, usize, usize, usize);

/// The three kinds of what runs, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Agents,
    Shells,
    Monitors,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Agents, Section::Shells, Section::Monitors];

    /// The section a task is in: what the engine calls a monitor, a loop and what it does not
    /// name are watching or repeating; a `Bash` is a shell.
    pub fn of(kind: TaskKind) -> Section {
        match kind {
            TaskKind::Agent => Section::Agents,
            TaskKind::Bash => Section::Shells,
            TaskKind::Monitor | TaskKind::Loop | TaskKind::Other => Section::Monitors,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::Agents => "subagentes",
            Section::Shells => "shells",
            Section::Monitors => "monitores",
        }
    }

    /// How a task of this section is called in a list that mixes them.
    pub fn word(self) -> &'static str {
        match self {
            Section::Agents => "subagente",
            Section::Shells => "shell",
            Section::Monitors => "monitor",
        }
    }
}

/// How many run of each kind: what the footer says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub agents: usize,
    pub shells: usize,
    pub monitors: usize,
}

impl Counts {
    pub fn total(self) -> usize {
        self.agents + self.shells + self.monitors
    }
}

pub fn counts(conv: &Conversation) -> Counts {
    let mut c = Counts::default();
    for t in conv.tasks.iter().filter(|t| t.state == TaskState::Run) {
        match Section::of(t.kind) {
            Section::Agents => c.agents += 1,
            Section::Shells => c.shells += 1,
            Section::Monitors => c.monitors += 1,
        }
    }
    c
}

/// What the area remembers: whether it is open, whether the keys go to it and which task is lit.
#[derive(Debug, Default, Clone)]
pub struct State {
    pub open: bool,
    pub focus: bool,
    sel: Option<String>,
    /// Whether the pane is wide enough for the area, as the last frame found it.
    room: Cell<bool>,
}

/// A line of the list: the heading of a section, or a task.
#[derive(Debug, Clone, Copy)]
enum Line<'a> {
    Head(Section, usize),
    Row(&'a Task),
}

/// The tasks as the area lists them: by section, the newest first in each.
pub fn rows(conv: &Conversation) -> Vec<&Task> {
    Section::ALL
        .iter()
        .flat_map(|s| {
            conv.tasks
                .iter()
                .rev()
                .filter(move |t| Section::of(t.kind) == *s)
                .take(SECTION_ROWS)
        })
        .collect()
}

/// The lines of the list: a heading for each section that has tasks, then them.
fn lines(conv: &Conversation) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for s in Section::ALL {
        let of: Vec<&Task> = rows(conv)
            .into_iter()
            .filter(|t| Section::of(t.kind) == s)
            .collect();
        if of.is_empty() {
            continue;
        }
        let all = conv
            .tasks
            .iter()
            .filter(|t| Section::of(t.kind) == s)
            .count();
        out.push(Line::Head(s, all));
        out.extend(of.into_iter().map(Line::Row));
    }
    out
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
    /// first of the list.
    pub fn lit<'a>(&self, conv: &'a Conversation) -> Option<&'a Task> {
        let tasks = rows(conv);
        self.sel
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id).copied())
            .or_else(|| tasks.iter().find(|t| t.state == TaskState::Run).copied())
            .or_else(|| tasks.first().copied())
    }

    fn light(&mut self, conv: &Conversation, at: usize) {
        self.sel = rows(conv).get(at).map(|t| t.id.clone());
    }

    fn at(&self, conv: &Conversation) -> usize {
        let tasks = rows(conv);
        self.lit(conv)
            .and_then(|l| tasks.iter().position(|t| t.id == l.id))
            .unwrap_or(0)
    }

    /// Light the task above or below the lit one, across the sections.
    pub fn step(&mut self, conv: &Conversation, down: bool) {
        let at = self.at(conv);
        let last = rows(conv).len().saturating_sub(1);
        self.light(
            conv,
            if down {
                (at + 1).min(last)
            } else {
                at.saturating_sub(1)
            },
        );
    }

    /// Light the first task, or the last.
    pub fn edge(&mut self, conv: &Conversation, last: bool) {
        let n = rows(conv).len();
        self.light(conv, if last { n.saturating_sub(1) } else { 0 });
    }

    /// Light the first task of the next section or, going back, of this one if the lit is not
    /// its first, else of the previous.
    pub fn jump(&mut self, conv: &Conversation, forward: bool) {
        let tasks = rows(conv);
        let Some(here) = self.lit(conv).map(|t| Section::of(t.kind)) else {
            return;
        };
        let first_of = |s: Section| tasks.iter().position(|t| Section::of(t.kind) == s);
        let order: Vec<Section> = Section::ALL
            .into_iter()
            .filter(|s| first_of(*s).is_some())
            .collect();
        let Some(i) = order.iter().position(|s| *s == here) else {
            return;
        };
        let at = self.at(conv);
        let target = if forward {
            order.get(i + 1).copied()
        } else if first_of(here) != Some(at) {
            Some(here)
        } else {
            i.checked_sub(1).map(|j| order[j])
        };
        if let Some(at) = target.and_then(first_of) {
            self.light(conv, at);
        }
    }

    /// A click at row `y` of the area whose top is `top`: light the task there, if it is one.
    pub fn click(&mut self, conv: &Conversation, top: usize, y: usize) -> bool {
        let Some(row) = y.checked_sub(top + 1) else {
            return false;
        };
        match lines(conv).get(row) {
            Some(Line::Row(t)) => {
                self.sel = Some(t.id.clone());
                self.focus = true;
                true
            }
            _ => false,
        }
    }
}

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
    match (t.state, Section::of(t.kind)) {
        // A monitor that runs does not turn: it watches.
        (TaskState::Run, Section::Monitors) => (
            if t.kind == TaskKind::Loop {
                '↻'
            } else {
                '◉'
            },
            look::yellow(),
        ),
        (TaskState::Run, _) => (
            ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(now_ms / 100 % 10) as usize],
            look::yellow(),
        ),
        (TaskState::Done, _) => ('✓', look::green()),
        (TaskState::Failed, _) => ('✗', look::red()),
        (TaskState::Stopped, _) => ('■', look::dim()),
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
    if t.kind != TaskKind::Agent {
        return None;
    }
    let id = t.item.as_deref()?;
    conv.items
        .iter()
        .rev()
        .find(|i| matches!(i, Item::Agent { id: a, .. } if a == id))
}

/// The command of a shell or a monitor: what its tool call was given, when the item of that call
/// is still in the conversation. The engine's own words for the task (`label`) are a description.
fn command<'a>(conv: &'a Conversation, t: &Task) -> Option<&'a str> {
    let id = t.item.as_deref()?;
    conv.items.iter().rev().find_map(|i| match i {
        Item::Tool { id: i, arg, .. } if i == id && !arg.trim().is_empty() => Some(arg.as_str()),
        _ => None,
    })
}

/// What a line is called: the kind of subagent and the model it runs with (`Explore (haiku-5.5)`),
/// or the command a shell runs and what a monitor watches.
fn name(conv: &Conversation, t: &Task) -> String {
    match card(conv, t) {
        Some(Item::Agent {
            name,
            model: Some(m),
            ..
        }) => format!("{name} ({m})"),
        Some(Item::Agent { name, .. }) => name.clone(),
        _ if t.kind == TaskKind::Agent => "agente".to_owned(),
        _ => command(conv, t).unwrap_or(&t.label).to_owned(),
    }
}

/// What a subagent does: what the engine says it is doing, else its last step; when it ended, how.
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

/// Its time and tokens: `1:12 · 38k`; a shell and a monitor have only the time.
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

/// Draw the area in `(x, y, w, h)`: a line at its left, the title, the sections with a line for
/// each task and the detail of the one that is lit (REQ-601).
pub fn draw(
    g: &mut Grid,
    conv: &Conversation,
    state: &State,
    (x, y, w, h): Rect,
    now_ms: u64,
    accent: Rgb,
) {
    // The rows below the title and the hint do not fit a pane this short; `Grid::put` clips only to
    // the screen, so they would be written over the border and the rows around it.
    if h < 4 {
        return;
    }
    for r in 0..h {
        g.put(x, y + r, "│", Style::fg(look::dim()));
    }
    let tx = x + 2;
    let tw = w.saturating_sub(3);
    let bottom = y + h;
    let hint = bottom.saturating_sub(1);
    let running = counts(conv).total();
    // Bold when it has the keys, dim when the field has them.
    let title = if state.focus {
        Style::fg(accent).bold()
    } else {
        Style::fg(look::dim())
    };
    g.put(tx, y, "en marcha", title);
    let how_many = if running > 0 {
        format!(" · {running} corriendo")
    } else {
        " · nada".to_owned()
    };
    g.put(tx + 9, y, &how_many, Style::fg(look::dim()));
    let list = lines(conv);
    let lit = state.lit(conv).map(|t| t.id.clone());
    let mut row = y + 1;
    if list.is_empty() {
        g.put(tx, row, "nada corre ahora", Style::fg(look::dim()));
        row += 1;
    }
    for line in &list {
        if row >= hint {
            break;
        }
        match line {
            Line::Head(s, n) => {
                g.put(
                    tx,
                    row,
                    &format!("{} · {n}", s.title()),
                    Style::fg(look::dim()),
                );
            }
            Line::Row(t) => {
                let on = lit.as_deref() == Some(t.id.as_str());
                let (m, colour) = mark(t, now_ms);
                let right = numbers(conv, t, now_ms);
                let left_room = tw.saturating_sub(right.chars().count() + 1);
                let head = format!("{} {} {}", if on { '▸' } else { ' ' }, m, name(conv, t));
                let head = clip(&head, left_room);
                let used = head.chars().count();
                // Only a subagent says what it does in its line; a shell is its command.
                let arrow = if t.kind == TaskKind::Agent {
                    clip(
                        &format!(" → {}", doing(conv, t)),
                        left_room.saturating_sub(used),
                    )
                } else {
                    String::new()
                };
                if on {
                    g.paint_bg(tx - 1, row, tw + 1, look::sel());
                }
                let on_bg = |st: Style| if on { st.on(look::sel()) } else { st };
                let name_style = if on && state.focus {
                    Style::fg(look::fg()).bold()
                } else {
                    Style::fg(look::fg())
                };
                g.put(tx, row, &head, on_bg(name_style));
                g.put(tx + 2, row, &m.to_string(), on_bg(Style::fg(colour).bold()));
                g.put(tx + used, row, &arrow, on_bg(Style::fg(look::dim())));
                let at = tx + tw - right.chars().count();
                g.put(at, row, &right, on_bg(Style::fg(look::dim())));
            }
        }
        row += 1;
    }
    let Some(t) = state.lit(conv) else {
        return;
    };
    if row + 1 >= hint {
        return;
    }
    g.put(tx, row, &"─".repeat(tw), Style::fg(look::dim()));
    row += 1;
    let put = |g: &mut Grid, row: &mut usize, dx: usize, text: &str, st: Style| {
        if *row < hint {
            g.put(tx + dx, *row, &clip(text, tw.saturating_sub(dx)), st);
            *row += 1;
        }
    };
    let dim = Style::fg(look::dim());
    let plain = Style::fg(look::fg());
    match card(conv, t) {
        Some(Item::Agent {
            prompt,
            description,
            steps,
            ..
        }) => {
            put(g, &mut row, 0, description, plain.bold());
            let prompt_rows = prompt
                .as_deref()
                .map_or(Vec::new(), |p| wrap(p, tw.saturating_sub(2), PROMPT_ROWS));
            let said_rows = t
                .info
                .as_deref()
                .map_or(Vec::new(), |i| wrap(i, tw.saturating_sub(2), SAID_ROWS));
            let section = |n: usize| if n == 0 { 0 } else { n + 1 };
            let reserved = section(prompt_rows.len()) + section(said_rows.len());
            let room = hint.saturating_sub(row).saturating_sub(reserved);
            let shown = steps.len().min(room.saturating_sub(1));
            if !prompt_rows.is_empty() {
                put(g, &mut row, 0, "prompt", dim);
                for l in &prompt_rows {
                    put(g, &mut row, 2, l, plain);
                }
            }
            if shown > 0 {
                put(g, &mut row, 0, "pasos", dim);
                for (i, s) in steps[steps.len() - shown..].iter().enumerate() {
                    let last = steps.len() - shown + i + 1 == steps.len();
                    let (m, c) = if last && t.state == TaskState::Run {
                        (mark(t, now_ms).0, look::yellow())
                    } else {
                        ('✓', look::green())
                    };
                    let (tool, rest) = s.split_once(' ').unwrap_or((s.as_str(), ""));
                    if row < hint {
                        g.put(tx + 2, row, &m.to_string(), Style::fg(c));
                        g.put(tx + 4, row, tool, Style::fg(look::cyan()));
                        let at = 5 + tool.chars().count();
                        g.put(tx + at, row, &clip(rest, tw.saturating_sub(at)), plain);
                        row += 1;
                    }
                }
            }
            if !said_rows.is_empty() {
                put(
                    g,
                    &mut row,
                    0,
                    if t.state == TaskState::Run {
                        "ahora"
                    } else {
                        "dijo"
                    },
                    dim,
                );
                for l in &said_rows {
                    put(g, &mut row, 2, l, plain);
                }
            }
        }
        _ => {
            // A shell: its command whole, what it does now or how it ended. A monitor: what it
            // watches or repeats and what it last said.
            let (what, last) = match (Section::of(t.kind), t.kind) {
                (Section::Shells, _) => (
                    "comando",
                    if t.state == TaskState::Run {
                        "ahora"
                    } else {
                        "terminó"
                    },
                ),
                (_, TaskKind::Loop) => ("repite", "último"),
                _ => ("vigila", "último"),
            };
            let shown = command(conv, t).unwrap_or(&t.label);
            put(g, &mut row, 0, what, dim);
            for l in wrap(shown, tw.saturating_sub(2), PROMPT_ROWS) {
                put(g, &mut row, 2, &l, plain);
            }
            // What the engine says it does, when that is more than the command again; else how it is.
            let said = t
                .info
                .as_deref()
                .filter(|i| !i.trim().is_empty() && *i != shown)
                .or(Some(t.label.as_str()).filter(|l| *l != shown && t.state == TaskState::Run))
                .map_or_else(
                    || match t.state {
                        TaskState::Run => "en marcha".to_owned(),
                        TaskState::Done => "listo".to_owned(),
                        TaskState::Stopped => "detenido".to_owned(),
                        TaskState::Failed => "falló".to_owned(),
                    },
                    str::to_owned,
                );
            put(g, &mut row, 0, last, dim);
            for l in wrap(&said, tw.saturating_sub(2), SAID_ROWS) {
                put(g, &mut row, 2, &l, plain);
            }
            // What it printed last (REQ-607): when the room is short the oldest lines go first.
            if t.kind != TaskKind::Loop && row + 1 < hint {
                put(g, &mut row, 0, "salida", dim);
                if t.tail.is_empty() {
                    put(g, &mut row, 2, "sin salida todavía", dim);
                }
                let room = hint.saturating_sub(row).min(TAIL_ROWS);
                for l in &t.tail[t.tail.len().saturating_sub(room)..] {
                    put(g, &mut row, 2, l, dim);
                }
            }
        }
    }
    g.put(
        tx,
        hint,
        &clip("↑↓ ←→ · x detener · ⇥ campo · esc cierra", tw),
        dim,
    );
}

#[cfg(test)]
mod tests {
    use contract::conversation::ToolState;

    use super::*;

    fn task(id: &str, kind: TaskKind, state: TaskState, started: u64) -> Task {
        Task {
            id: id.into(),
            kind,
            label: format!("label {id}"),
            state,
            started,
            info: None,
            item: None,
            tail: Vec::new(),
        }
    }

    /// One of each kind, two shells and a loop, oldest first as the node keeps them.
    fn mixed() -> Conversation {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.items = vec![agent_card(
            "c1",
            &["Glob crates/**", "Read lib.rs", "Grep fn pair"],
        )];
        let mut agent = task("ag", TaskKind::Agent, TaskState::Run, 1);
        agent.item = Some("c1".into());
        agent.info = Some("Buscando el puerto del puente".into());
        let mut done = task("ag0", TaskKind::Agent, TaskState::Done, 0);
        done.label = "revisar N123".into();
        let mut cargo = task("sh1", TaskKind::Bash, TaskState::Run, 2);
        cargo.label = "cargo test -p jwn".into();
        cargo.info = Some("compilando jwn".into());
        let mut build = task("sh2", TaskKind::Bash, TaskState::Failed, 3);
        build.label = "npm run build".into();
        let mut tail = task("mo1", TaskKind::Monitor, TaskState::Run, 4);
        tail.label = "tail -f app.log | grep ERROR".into();
        tail.info = Some("último aviso hace 2 min".into());
        let mut ci = task("mo2", TaskKind::Loop, TaskState::Run, 5);
        ci.label = "revisar el CI cada 5 min".into();
        conv.tasks = vec![done, agent, cargo, build, tail, ci];
        conv
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

    fn ids(conv: &Conversation) -> Vec<&str> {
        rows(conv).iter().map(|t| t.id.as_str()).collect()
    }

    fn frame(conv: &Conversation, state: &State, w: usize, h: usize) -> (Grid, Vec<String>) {
        let mut g = Grid::new(w + 10, h + 2);
        draw(&mut g, conv, state, (10, 1, w, h), 1_000_000, look::blue());
        let rows = g.rows_plain();
        (g, rows)
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
        assert_eq!(
            (width(120), width(150)),
            (46, 60),
            "wider when there is room"
        );
    }

    /// REQ-601: three sections, in this order, the newest first in each; a loop is a monitor.
    #[test]
    fn the_tasks_are_listed_by_section_the_newest_first_in_each() {
        let conv = mixed();
        assert_eq!(ids(&conv), ["ag", "ag0", "sh2", "sh1", "mo2", "mo1"]);
        assert_eq!(
            counts(&conv),
            Counts {
                agents: 1,
                shells: 1,
                monitors: 2
            },
            "only what runs counts"
        );
        assert_eq!(counts(&conv).total(), 4);
        // a section keeps its newest few
        let mut many = mixed();
        many.tasks = (0..9)
            .map(|n| task(&format!("s{n}"), TaskKind::Bash, TaskState::Done, n))
            .collect();
        assert_eq!(ids(&many).len(), SECTION_ROWS);
        assert_eq!(ids(&many)[0], "s8");
        // what the engine does not name is a monitor
        many.tasks = vec![task("o", TaskKind::Other, TaskState::Run, 1)];
        assert_eq!(Section::of(many.tasks[0].kind), Section::Monitors);
    }

    /// REQ-601: the lit line is the one chosen, else the first that runs, else the first; `↑` `↓`
    /// walk all the lines across the sections, `←` `→` jump a section, `g` `G` go to the ends.
    #[test]
    fn the_keys_walk_the_lines_across_the_sections_and_jump_from_one_to_the_next() {
        let conv = mixed();
        let mut s = State::default();
        assert_eq!(s.lit(&conv).unwrap().id, "ag", "the first that runs");
        s.step(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "ag0");
        s.step(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "sh2", "across into the shells");
        s.jump(&conv, true);
        assert_eq!(
            s.lit(&conv).unwrap().id,
            "mo2",
            "→ the first of the next section"
        );
        s.jump(&conv, true);
        assert_eq!(
            s.lit(&conv).unwrap().id,
            "mo2",
            "there is no section after the last"
        );
        s.step(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "mo1");
        s.jump(&conv, false);
        assert_eq!(
            s.lit(&conv).unwrap().id,
            "mo2",
            "← goes to the start of this section first"
        );
        s.jump(&conv, false);
        assert_eq!(
            s.lit(&conv).unwrap().id,
            "sh2",
            "and then to the section before"
        );
        s.edge(&conv, true);
        assert_eq!(s.lit(&conv).unwrap().id, "mo1");
        s.edge(&conv, false);
        assert_eq!(s.lit(&conv).unwrap().id, "ag");
        s.step(&conv, false);
        assert_eq!(s.lit(&conv).unwrap().id, "ag", "it stops at the first");
        // a section with nothing is skipped, and with nothing at all nothing moves
        let mut no_shells = mixed();
        no_shells.tasks.retain(|t| t.kind != TaskKind::Bash);
        let mut s = State::default();
        s.jump(&no_shells, true);
        assert_eq!(
            s.lit(&no_shells).unwrap().id,
            "mo2",
            "the shells have none: on to the monitors"
        );
        no_shells.tasks.clear();
        s.jump(&no_shells, true);
        s.step(&no_shells, true);
        s.edge(&no_shells, true);
        assert!(s.lit(&no_shells).is_none());
        // the one chosen is gone: the first that runs again
        let mut gone = mixed();
        let mut s = State::default();
        s.edge(&gone, true);
        gone.tasks.retain(|t| t.id != "mo1");
        assert_eq!(s.lit(&gone).unwrap().id, "ag");
    }

    /// A click counts the headings of the sections, which are not tasks.
    #[test]
    fn a_click_on_a_line_lights_it_and_gives_the_keys_to_the_area() {
        let conv = mixed();
        // y 5 is the title; 6 `subagentes`, 7 ag, 8 ag0, 9 `shells`, 10 sh2, 11 sh1
        let mut s = State::default();
        assert!(!s.click(&conv, 5, 5), "the title is not a line");
        assert!(!s.click(&conv, 5, 6), "nor is the heading of a section");
        assert!(s.click(&conv, 5, 8));
        assert_eq!((s.lit(&conv).unwrap().id.as_str(), s.focus), ("ag0", true));
        assert!(!s.click(&conv, 5, 9), "the heading of the shells");
        assert!(s.click(&conv, 5, 11));
        assert_eq!(s.lit(&conv).unwrap().id, "sh1");
        assert!(!s.click(&conv, 5, 40), "past the list");
    }

    /// REQ-601: the three sections with a line for each, and the detail of the lit one by kind.
    #[test]
    fn the_area_draws_the_sections_and_the_detail_of_the_lit_line_by_kind() {
        let conv = mixed();
        let mut state = State {
            open: true,
            focus: true,
            ..State::default()
        };
        let (_, rows) = frame(&conv, &state, 60, 30);
        let text = rows.join("\n");
        assert!(rows[1].contains("en marcha · 4 corriendo"), "{text}");
        for want in ["subagentes · 2", "shells · 2", "monitores · 2"] {
            assert!(text.contains(want), "{want:?} not drawn:\n{text}");
        }
        let line = |s: &str| {
            rows.iter()
                .find(|r| r.contains(s))
                .unwrap_or_else(|| panic!("{s:?}:\n{text}"))
        };
        assert!(
            line("Explore (haiku-5.5)").contains("→ Buscando el pue"),
            "{}",
            line("Explore")
        );
        assert!(
            line("Explore").trim_end().ends_with("16:39 · 38.4k"),
            "time and tokens at the right"
        );
        assert!(line("Explore").contains('▸'), "the first that runs is lit");
        assert!(
            line("✓ agente").contains("→ listo"),
            "a subagent that ended says so"
        );
        assert!(
            line("cargo test -p jwn").trim_end().ends_with("16:39")
                && !line("cargo test").contains('→'),
            "a shell is its command and its time"
        );
        assert!(line("npm run build").contains('✗'), "one that failed");
        assert!(
            line("tail -f app.log").contains('◉') && line("revisar el CI").contains('↻'),
            "a monitor watches and a loop repeats"
        );
        assert!(
            text.find("subagentes").unwrap() < text.find("shells").unwrap()
                && text.find("shells").unwrap() < text.find("monitores").unwrap()
        );
        // the detail of the subagent that is lit
        for want in [
            "mapear el puente",
            "prompt",
            "  Busca cómo el daemon",
            "pasos",
            "Glob",
            "Grep",
            "ahora",
            "  Buscando el puerto",
        ] {
            assert!(text.contains(want), "{want:?} not drawn:\n{text}");
        }
        // of a shell
        state.step(&conv, true);
        state.step(&conv, true);
        let (_, rows) = frame(&conv, &state, 60, 30);
        let text = rows.join("\n");
        for want in ["comando", "  npm run build", "terminó", "  falló"] {
            assert!(text.contains(want), "shell: {want:?} not drawn:\n{text}");
        }
        // of a monitor and of a loop
        state.edge(&conv, true);
        let (_, rows) = frame(&conv, &state, 60, 30);
        let text = rows.join("\n");
        for want in [
            "vigila",
            "  tail -f app.log | grep ERROR",
            "último",
            "  último aviso hace 2 min",
        ] {
            assert!(text.contains(want), "monitor: {want:?} not drawn:\n{text}");
        }
        state.step(&conv, false);
        let (_, rows) = frame(&conv, &state, 60, 30);
        assert!(rows.join("\n").contains("repite"), "a loop repeats");
        assert!(rows.iter().all(|r| r.chars().count() <= 70));
    }

    /// What a shell or a monitor printed last is under what it does, the newest line last; a loop
    /// has none, and a short room loses the oldest lines first (REQ-607).
    #[test]
    fn a_shell_and_a_monitor_show_the_tail_of_their_output() {
        let mut conv = mixed();
        for t in &mut conv.tasks {
            match t.id.as_str() {
                "sh1" => {
                    t.tail = (1..=8).map(|i| format!("línea {i}")).collect::<Vec<_>>()[2..].to_vec()
                }
                "mo1" => t.tail = vec!["ERROR timeout en /sync".into()],
                _ => {}
            }
        }
        let mut state = State {
            open: true,
            focus: true,
            ..State::default()
        };
        // the shell with output: the three steps from the lit subagent reach `cargo test`
        for _ in 0..3 {
            state.step(&conv, true);
        }
        let (_, rows) = frame(&conv, &state, 60, 30);
        let text = rows.join("\n");
        let at = |s: &str| {
            rows.iter()
                .position(|r| r.contains(s))
                .unwrap_or_else(|| panic!("{s:?}:\n{text}"))
        };
        assert!(
            at("ahora") < at("salida")
                && at("salida") < at("  línea 3")
                && at("  línea 3") < at("  línea 8"),
            "{text}"
        );
        assert!(
            !text.contains("línea 2"),
            "six lines at most, the newest:\n{text}"
        );
        // the one that printed nothing says so
        state.step(&conv, false);
        let (_, rows) = frame(&conv, &state, 60, 30);
        assert!(
            rows.join("\n").contains("  sin salida todavía"),
            "{}",
            rows.join("\n")
        );
        // a monitor
        state.edge(&conv, true);
        let (_, rows) = frame(&conv, &state, 60, 30);
        assert!(
            rows.join("\n").contains("  ERROR timeout en /sync"),
            "{}",
            rows.join("\n")
        );
        // a loop has no output of its own
        state.step(&conv, false);
        let (_, rows) = frame(&conv, &state, 60, 30);
        assert!(!rows.join("\n").contains("salida"), "{}", rows.join("\n"));
        // a short room keeps the newest lines
        let mut state = State {
            open: true,
            focus: true,
            ..State::default()
        };
        for _ in 0..3 {
            state.step(&conv, true);
        }
        let (_, rows) = frame(&conv, &state, 60, 21);
        let text = rows.join("\n");
        assert!(
            text.contains("línea 8") && !text.contains("línea 3"),
            "{text}"
        );
    }

    /// A shell is its command, which the item of its tool call has; the engine's words for it are
    /// a description, shown as what it does while it runs, and not again when it ended.
    #[test]
    fn a_shell_is_its_command_from_its_tool_call_and_the_description_is_what_it_does() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.items = vec![
            serde_json::from_value(serde_json::json!({
                "t": "tool", "id": "toolu_b", "name": "Bash", "arg": "sleep 25; echo listo", "state": "run"
            }))
            .unwrap(),
        ];
        let mut sh = task("sh", TaskKind::Bash, TaskState::Run, 1);
        sh.label = "Sleep 25 seconds then echo listo".into();
        sh.item = Some("toolu_b".into());
        conv.tasks = vec![sh];
        let state = State {
            open: true,
            ..State::default()
        };
        let (_, rows) = frame(&conv, &state, 60, 20);
        let text = rows.join("\n");
        assert!(
            rows.iter()
                .any(|r| r.contains("sleep 25; echo listo") && r.contains('⠋')),
            "{text}"
        );
        assert!(
            text.contains("comando")
                && text.contains("ahora")
                && text.contains("  Sleep 25 seconds then echo listo"),
            "{text}"
        );
        // ended: the description is not said again as if it were news
        conv.tasks[0].state = TaskState::Done;
        let (_, rows) = frame(&conv, &state, 60, 20);
        let text = rows.join("\n");
        assert!(
            text.contains("terminó") && text.contains("  listo"),
            "{text}"
        );
        assert!(!text.contains("Sleep 25 seconds"), "{text}");
        // with no tool call in the conversation, the description is all there is
        conv.items.clear();
        let (_, rows) = frame(&conv, &state, 60, 20);
        assert!(
            rows.iter()
                .any(|r| r.contains("Sleep 25 seconds then echo listo"))
        );
    }

    /// The title is bold when the area has the keys and dim when the field has them.
    #[test]
    fn the_title_says_who_has_the_keys() {
        let conv = mixed();
        let mut state = State {
            open: true,
            focus: true,
            ..State::default()
        };
        let (g, _) = frame(&conv, &state, 60, 20);
        let at = |g: &Grid| g.cells[g.cols + 12].1;
        assert_eq!(at(&g).fg, Some(look::blue()));
        assert!(at(&g).bold);
        state.focus = false;
        let (g, _) = frame(&conv, &state, 60, 20);
        assert_eq!(at(&g).fg, Some(look::dim()));
    }

    /// REQ-601: nothing runs, and the area says so and keeps its place.
    #[test]
    fn with_no_task_the_area_says_nothing_runs() {
        let mut conv = crate::tui::conversation::tests_fixture();
        conv.tasks.clear();
        let (_, rows) = frame(
            &conv,
            &State {
                open: true,
                ..State::default()
            },
            46,
            10,
        );
        let text = rows.join("\n");
        assert!(
            text.contains("en marcha · nada") && text.contains("nada corre ahora"),
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
