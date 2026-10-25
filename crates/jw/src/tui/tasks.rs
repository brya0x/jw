//! The tasks view of the terminal client (N180f2; REQ-943 to REQ-945, REQ-947, REQ-951 to REQ-953,
//! REQ-958): a screen of this client that takes the place of the workspace's panels while it is
//! open, drawn from the `tasks` node alone. It owns every key while it is shown, so `L`, `d`, `u`
//! and `b` are its own and not a swap, the diff or a close. It asks the daemon through `Ask`s that
//! the loop serves, and holds no list of its own.

use contract::tasks::{StopKind, TaskOp, TaskPhase, Tasks};
use crossterm::event::KeyCode;

use super::{Grid, Rect, Style, clip, look, shell};

/// How long before the node is asked for again when it is gone while the row says there is a list
/// (REQ-958).
const ASK_AGAIN_MS: u64 = 3000;

/// What the view wants the daemon to do, for the loop to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    Open(String),
    Edit {
        workspace: String,
        op: TaskOp,
        id: String,
        text: Option<String>,
    },
    LoopOn {
        workspace: String,
        max: u32,
    },
    LoopOff(String),
}

/// What a key came to for the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    /// Close the view and read the list's file in the viewer (`e`).
    File(String),
}

/// The line the view is asking for: the reason of a block, or the laps of a loop.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Input {
    Reason { id: String, text: String },
    Laps { text: String },
}

#[derive(Debug)]
pub struct TasksView {
    workspace: String,
    cursor: usize,
    input: Option<Input>,
    asks: Vec<Ask>,
    asked_at: Option<u64>,
}

/// A piece of a row: where it starts, its text and its style; and a row: the task it is the main line of, if any.
type Part = (usize, String, Style);
type Line = (Option<usize>, Vec<Part>);

/// The tasks in the order they are shown: by level, and in the order of the file within one.
fn order(doc: &Tasks) -> Vec<usize> {
    let mut at: Vec<usize> = (0..doc.tasks.len()).collect();
    at.sort_by_key(|i| (doc.tasks[*i].level, *i));
    at
}

impl TasksView {
    pub fn new(workspace: &str) -> TasksView {
        TasksView {
            workspace: workspace.to_owned(),
            cursor: 0,
            input: None,
            asks: Vec::new(),
            asked_at: None,
        }
    }

    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// The node is not there while the row says there is a list: ask for it, and again if it goes (REQ-958).
    pub fn want(&mut self, have: bool, listed: bool, now_ms: u64) {
        if have {
            self.asked_at = None;
            return;
        }
        if listed && self.asked_at.is_none_or(|t| now_ms >= t + ASK_AGAIN_MS) {
            self.asked_at = Some(now_ms);
            self.asks.push(Ask::Open(self.workspace.clone()));
        }
    }

    pub fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// What the line at the foot says it is waiting for, and the keys.
    pub fn hint(&self) -> String {
        match &self.input {
            Some(Input::Reason { .. }) => "↵ bloquea la tarea · esc cancela".into(),
            Some(Input::Laps { .. }) => "↵ activa el bucle · esc cancela".into(),
            None => {
                "j k · d hecha · b bloquea · u reabre · L bucle · e archivo · T esc vuelve".into()
            }
        }
    }

    fn selected<'a>(&self, doc: &'a Tasks) -> Option<&'a contract::tasks::TaskItem> {
        order(doc).get(self.cursor).map(|i| &doc.tasks[*i])
    }

    fn edit(&mut self, doc: Option<&Tasks>, op: TaskOp) {
        if let Some(t) = doc.and_then(|d| self.selected(d)) {
            self.asks.push(Ask::Edit {
                workspace: self.workspace.clone(),
                op,
                id: t.id.clone(),
                text: None,
            });
        }
    }

    /// A key while the view is shown: every key is the view's (REQ-945); no key uses Alt.
    pub fn press(&mut self, doc: Option<&Tasks>, code: KeyCode) -> Outcome {
        if let Some(input) = self.input.take() {
            return self.type_in(input, code);
        }
        let last = doc.map_or(0, |d| d.tasks.len().saturating_sub(1));
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'T') => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.cursor = (self.cursor + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('d') => self.edit(doc, TaskOp::Done),
            KeyCode::Char('u') => self.edit(doc, TaskOp::Reopen),
            KeyCode::Char('b') => {
                if let Some(t) = doc.and_then(|d| self.selected(d)) {
                    self.input = Some(Input::Reason {
                        id: t.id.clone(),
                        text: String::new(),
                    });
                }
            }
            KeyCode::Char('L') => match doc {
                Some(d) if d.loop_.on => self.asks.push(Ask::LoopOff(self.workspace.clone())),
                Some(_) => self.input = Some(Input::Laps { text: "20".into() }),
                None => {}
            },
            KeyCode::Char('e') => {
                if let Some(d) = doc {
                    return Outcome::File(d.path.clone());
                }
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn type_in(&mut self, mut input: Input, code: KeyCode) -> Outcome {
        match (&mut input, code) {
            (_, KeyCode::Esc) => return Outcome::Stay,
            (Input::Reason { id, text }, KeyCode::Enter) => {
                if !text.trim().is_empty() {
                    self.asks.push(Ask::Edit {
                        workspace: self.workspace.clone(),
                        op: TaskOp::Block,
                        id: id.clone(),
                        text: Some(text.trim().to_owned()),
                    });
                    return Outcome::Stay;
                }
            }
            (Input::Laps { text }, KeyCode::Enter) => {
                let max = if text.is_empty() {
                    Some(20)
                } else {
                    text.parse::<u32>().ok()
                };
                if let Some(max) = max.filter(|m| (1..=99).contains(m)) {
                    self.asks.push(Ask::LoopOn {
                        workspace: self.workspace.clone(),
                        max,
                    });
                    return Outcome::Stay;
                }
            }
            (Input::Reason { text, .. }, KeyCode::Char(c)) => text.push(c),
            (Input::Laps { text }, KeyCode::Char(c)) if c.is_ascii_digit() && text.len() < 2 => {
                text.push(c)
            }
            (Input::Reason { text, .. } | Input::Laps { text }, KeyCode::Backspace) => {
                text.pop();
            }
            _ => {}
        }
        self.input = Some(input);
        Outcome::Stay
    }

    /// Draws the view in `rect`: the loop's line, the tasks by level with what each is and waits for, the
    /// blocked reasons, the notes of the final task, the problems, and the line being typed.
    pub fn draw(&self, g: &mut Grid, doc: Option<&Tasks>, (x, y, w, h): Rect, now_ms: u64) {
        let dim = Style::fg(look::dim());
        let put = |g: &mut Grid, row: usize, col: usize, text: &str, style: Style| {
            if row < h && col < w {
                g.put(x + col, y + row, &clip(text, w - col), style);
            }
        };
        let Some(doc) = doc else {
            put(g, 0, 1, "tareas", Style::fg(look::blue()).bold());
            put(g, 2, 1, "leyendo la lista…", dim);
            return;
        };
        let done = doc
            .tasks
            .iter()
            .filter(|t| t.state == TaskPhase::Done)
            .count();
        put(g, 0, 1, "tareas", Style::fg(look::blue()).bold());
        put(g, 0, 8, &format!("{done}/{}", doc.tasks.len()), dim);
        let (text, style) = loop_line(doc, now_ms);
        put(g, 1, 1, &text, style);
        put(g, 2, 1, &"─".repeat(w.saturating_sub(2)), dim);
        // the body is the rows that scroll; the line being typed is under it
        let foot = usize::from(self.input.is_some());
        let body_h = h.saturating_sub(3 + foot);
        let mut rows: Vec<Line> = Vec::new();
        let ord = order(doc);
        for (n, i) in ord.iter().enumerate() {
            let t = &doc.tasks[*i];
            let (glyph, gstyle) = match t.state {
                TaskPhase::Done => ("✓", Style::fg(look::green())),
                TaskPhase::Running => ("▶", Style::fg(look::blue()).bold()),
                TaskPhase::Ready => ("○", Style::default()),
                TaskPhase::Waiting => ("◌", dim),
                TaskPhase::Blocked => ("✗", Style::fg(look::yellow()).bold()),
            };
            let quiet = matches!(t.state, TaskPhase::Done | TaskPhase::Waiting);
            let right = if t.state == TaskPhase::Running {
                format!("{} en marcha", shell::spin(now_ms))
            } else if t.after.is_empty() {
                String::new()
            } else if t.after == ["*"] {
                "← todas".to_owned()
            } else {
                format!("← {}", t.after.join(", "))
            };
            let mut parts = vec![
                (1, format!("{:>2}", t.level), dim),
                (4, glyph.to_owned(), gstyle),
                (6, t.id.clone(), Style::default().bold()),
                (
                    7 + t.id.chars().count(),
                    clip(
                        &t.title,
                        w.saturating_sub(12 + right.chars().count() + t.id.chars().count()),
                    ),
                    if quiet { dim } else { Style::default() },
                ),
            ];
            if !right.is_empty() {
                let at = w.saturating_sub(right.chars().count() + 1);
                parts.push((
                    at,
                    right.clone(),
                    if t.state == TaskPhase::Running {
                        Style::fg(look::blue())
                    } else {
                        dim
                    },
                ));
            }
            rows.push((Some(n), parts));
            let under = |text: String, style: Style| (None, vec![(8, text, style)]);
            if let Some(why) = &t.why {
                rows.push(under(
                    format!("bloqueada: {why}"),
                    Style::fg(look::yellow()),
                ));
            }
            for note in &t.notes {
                rows.push(under(format!("· {note}"), Style::fg(look::green())));
            }
            if t.is_final && t.state == TaskPhase::Done && t.notes.is_empty() {
                rows.push(under(
                    "sin evidencia anotada".into(),
                    Style::fg(look::yellow()),
                ));
            }
            if t.auto {
                rows.push(under("se añade al archivo al activar el bucle".into(), dim));
            }
        }
        for p in &doc.problems {
            let line = p.line.map_or(String::new(), |l| format!(" (línea {l})"));
            rows.push((
                None,
                vec![(
                    1,
                    format!("problema{line}: {}", p.text),
                    Style::fg(look::yellow()),
                )],
            ));
        }
        let sel_row = rows
            .iter()
            .position(|(t, _)| *t == Some(self.cursor.min(ord.len().saturating_sub(1))))
            .unwrap_or(0);
        let top = if sel_row >= body_h {
            sel_row + 1 - body_h
        } else {
            0
        };
        for (r, (task, parts)) in rows.iter().enumerate().skip(top).take(body_h) {
            let row = 3 + r - top;
            if *task == Some(self.cursor) && !ord.is_empty() {
                g.light(x, x + w, y + row);
            }
            for (col, text, style) in parts {
                put(g, row, *col, text, *style);
            }
        }
        match &self.input {
            Some(Input::Reason { text, .. }) => put(
                g,
                h.saturating_sub(1),
                1,
                &format!("motivo: {text}▏"),
                Style::fg(look::yellow()),
            ),
            Some(Input::Laps { text }) => put(
                g,
                h.saturating_sub(1),
                1,
                &format!("vueltas como máximo: {text}▏"),
                Style::fg(look::yellow()),
            ),
            None => {}
        }
    }
}

/// What the loop is doing, in one line, and the colour that says it.
fn loop_line(doc: &Tasks, now_ms: u64) -> (String, Style) {
    let lp = &doc.loop_;
    if lp.on {
        let wait = if lp.waiting {
            " · espera tu respuesta"
        } else {
            ""
        };
        let at = lp
            .task
            .as_ref()
            .map(|t| format!(" · {t}"))
            .unwrap_or_default();
        let spin = if lp.waiting { "!" } else { shell::spin(now_ms) };
        let colour = if lp.waiting {
            look::red()
        } else {
            look::blue()
        };
        return (
            format!(
                "{spin} bucle en marcha · vuelta {}/{}{at}{wait}",
                lp.laps, lp.max
            ),
            Style::fg(colour),
        );
    }
    match &lp.stop {
        Some(s) if s.kind == StopKind::Done => (
            format!("✓ bucle terminado: {}", s.text),
            Style::fg(look::green()),
        ),
        Some(s) if matches!(s.kind, StopKind::Person | StopKind::Pane) => (
            format!("○ bucle parado: {}", s.text),
            Style::fg(look::dim()),
        ),
        Some(s) => (
            format!("? bucle parado: {}", s.text),
            Style::fg(look::yellow()),
        ),
        None => (
            "○ bucle apagado · L lo activa".into(),
            Style::fg(look::dim()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Tasks {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contract/fixtures/daemon")
            .join(name);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        match serde_json::from_value::<contract::Node>(doc["node"].clone()).unwrap() {
            contract::Node::Tasks(t) => t,
            other => panic!("not a tasks node: {other:?}"),
        }
    }

    fn text(v: &TasksView, doc: Option<&Tasks>, w: usize, h: usize) -> Vec<String> {
        let mut g = Grid::new(w, h);
        v.draw(&mut g, doc, (0, 0, w, h), 0);
        g.rows_plain()
    }

    /// REQ-944: the tasks by level with their state and what they wait for, the final task that is not
    /// in the file yet, the problems and the loop's line.
    #[test]
    fn the_view_shows_the_tasks_by_level_with_their_state_and_what_they_wait_for() {
        let doc = fixture("snapshot-tasks.json");
        let rows = text(&TasksView::new("w"), Some(&doc), 90, 24);
        assert!(
            rows[0].contains("tareas") && rows[0].contains("2/5"),
            "{}",
            rows[0]
        );
        assert!(rows[1].contains("bucle apagado"), "{}", rows[1]);
        let at = |id: &str| {
            rows.iter()
                .position(|r| r.contains(&format!(" {id} ")))
                .unwrap_or_else(|| panic!("{id} in {rows:#?}"))
        };
        assert!(rows[at("t1")].contains('✓') && rows[at("t1")].trim_start().starts_with('1'));
        assert!(
            rows[at("t3")].contains('○') && rows[at("t3")].contains("← t1, t2"),
            "{}",
            rows[at("t3")]
        );
        assert!(rows[at("t4")].contains('◌') && rows[at("t4")].contains("← t3, t9"));
        assert!(rows[at("fin")].contains("← todas"), "{}", rows[at("fin")]);
        assert!(
            at("t1") < at("t2")
                && at("t2") < at("t3")
                && at("t3") < at("t4")
                && at("t4") < at("fin"),
            "by level"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("se añade al archivo al activar el bucle"))
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("problema (línea 6): t4 waits for t9"))
        );
        // the cursor row is lit: the first task, in the order shown
        let mut g = Grid::new(90, 24);
        TasksView::new("w").draw(&mut g, Some(&doc), (0, 0, 90, 24), 0);
        assert_ne!(
            g.cells[3 * 90 + 5].1.bg,
            g.cells[4 * 90 + 5].1.bg,
            "the cursor is lit"
        );
    }

    /// REQ-944: a blocked task says why, the final task shows its notes (its evidence) and the loop says why it stopped.
    #[test]
    fn a_blocked_task_its_evidence_and_why_the_loop_stopped_are_there() {
        let doc = fixture("snapshot-tasks-stopped.json");
        let rows = text(&TasksView::new("w"), Some(&doc), 100, 24);
        assert!(
            rows[1].starts_with(" ? bucle parado: t3 blocked: needs STRIPE_KEY in .env"),
            "{}",
            rows[1]
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("bloqueada: needs STRIPE_KEY in .env"))
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("· cargo test: 212 ok, 2 fail"))
        );
        // done with no notes at all is said
        let mut done = fixture("snapshot-tasks.json");
        for t in &mut done.tasks {
            t.state = TaskPhase::Done;
            t.auto = false;
        }
        done.problems.clear();
        let rows = text(&TasksView::new("w"), Some(&done), 90, 24);
        assert!(
            rows.iter().any(|r| r.contains("sin evidencia anotada")),
            "{rows:#?}"
        );
        // a loop that is on says the lap, the task that runs, and that it waits for the person
        let mut on = fixture("snapshot-tasks.json");
        on.loop_.on = true;
        on.loop_.laps = 4;
        on.loop_.task = Some("t3".into());
        on.tasks[2].state = TaskPhase::Running;
        assert!(
            text(&TasksView::new("w"), Some(&on), 90, 24)[1]
                .contains("bucle en marcha · vuelta 4/20 · t3")
        );
        assert!(
            text(&TasksView::new("w"), Some(&on), 90, 24)
                .iter()
                .any(|r| r.contains("▶") && r.contains("en marcha"))
        );
        on.loop_.waiting = true;
        assert!(text(&TasksView::new("w"), Some(&on), 90, 24)[1].contains("espera tu respuesta"));
        // an end is good news
        let mut end = fixture("snapshot-tasks-stopped.json");
        end.loop_.stop = Some(contract::tasks::LoopStop {
            kind: StopKind::Done,
            text: "all 5 tasks done in 7 laps".into(),
        });
        assert!(
            text(&TasksView::new("w"), Some(&end), 100, 24)[1]
                .starts_with(" ✓ bucle terminado: all 5 tasks done")
        );
        assert!(text(&TasksView::new("w"), None, 60, 10)[2].contains("leyendo la lista"));
    }

    /// REQ-945: the keys of the view, and that it swallows the others.
    #[test]
    fn the_keys_of_the_view_ask_for_what_they_say_and_the_others_are_swallowed() {
        let doc = fixture("snapshot-tasks.json");
        let mut v = TasksView::new("w1");
        let press = |v: &mut TasksView, c: KeyCode| v.press(Some(&doc), c);
        // j moves down the order shown and stops at the end; k at the top stays
        assert_eq!(press(&mut v, KeyCode::Char('k')), Outcome::Stay);
        for _ in 0..9 {
            press(&mut v, KeyCode::Char('j'));
        }
        assert_eq!(v.cursor, 4);
        for _ in 0..9 {
            press(&mut v, KeyCode::Up);
        }
        assert_eq!(v.cursor, 0);
        // d and u act on the task under the cursor (t1 is the first in the order shown)
        press(&mut v, KeyCode::Char('j'));
        press(&mut v, KeyCode::Char('d'));
        press(&mut v, KeyCode::Char('u'));
        assert_eq!(
            v.take_asks(),
            [
                Ask::Edit {
                    workspace: "w1".into(),
                    op: TaskOp::Done,
                    id: "t2".into(),
                    text: None
                },
                Ask::Edit {
                    workspace: "w1".into(),
                    op: TaskOp::Reopen,
                    id: "t2".into(),
                    text: None
                },
            ]
        );
        // b asks the reason and sends it on ↵; an empty one is not sent, esc leaves it
        press(&mut v, KeyCode::Char('b'));
        assert!(v.hint().contains("bloquea la tarea"));
        press(&mut v, KeyCode::Enter);
        assert!(
            v.take_asks().is_empty() && v.input.is_some(),
            "a reason is needed"
        );
        for c in "no key".chars() {
            press(&mut v, KeyCode::Char(c));
        }
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Char('y'));
        press(&mut v, KeyCode::Enter);
        assert_eq!(
            v.take_asks(),
            [Ask::Edit {
                workspace: "w1".into(),
                op: TaskOp::Block,
                id: "t2".into(),
                text: Some("no key".into())
            }]
        );
        assert!(v.input.is_none());
        press(&mut v, KeyCode::Char('b'));
        press(&mut v, KeyCode::Char('x'));
        press(&mut v, KeyCode::Esc);
        assert!(v.input.is_none() && v.take_asks().is_empty());
        // L asks the laps with 20 in the line, only digits, at most two, and 1 to 99
        press(&mut v, KeyCode::Char('L'));
        press(&mut v, KeyCode::Enter);
        assert_eq!(
            v.take_asks(),
            [Ask::LoopOn {
                workspace: "w1".into(),
                max: 20
            }]
        );
        press(&mut v, KeyCode::Char('L'));
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Char('x'));
        press(&mut v, KeyCode::Char('7'));
        press(&mut v, KeyCode::Char('5'));
        press(&mut v, KeyCode::Char('9'));
        press(&mut v, KeyCode::Enter);
        assert_eq!(
            v.take_asks(),
            [Ask::LoopOn {
                workspace: "w1".into(),
                max: 75
            }]
        );
        press(&mut v, KeyCode::Char('L'));
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Char('0'));
        press(&mut v, KeyCode::Enter);
        assert!(
            v.take_asks().is_empty() && v.input.is_some(),
            "0 laps is not asked for"
        );
        press(&mut v, KeyCode::Esc);
        // with the loop on, L turns it off at once
        let mut on = doc.clone();
        on.loop_.on = true;
        v.press(Some(&on), KeyCode::Char('L'));
        assert_eq!(v.take_asks(), [Ask::LoopOff("w1".into())]);
        // e reads the file; T, esc and q leave; every other key is swallowed
        assert_eq!(
            press(&mut v, KeyCode::Char('e')),
            Outcome::File(doc.path.clone())
        );
        for c in [KeyCode::Char('T'), KeyCode::Esc, KeyCode::Char('q')] {
            assert_eq!(press(&mut v, c), Outcome::Close);
        }
        for c in [
            KeyCode::Char('x'),
            KeyCode::Char('n'),
            KeyCode::Tab,
            KeyCode::Char('w'),
            KeyCode::Enter,
        ] {
            assert_eq!(press(&mut v, c), Outcome::Stay);
        }
        assert!(v.take_asks().is_empty());
        // with no list read yet nothing is asked of a task
        let mut none = TasksView::new("w1");
        for c in "dubLe".chars() {
            none.press(None, KeyCode::Char(c));
        }
        assert!(none.take_asks().is_empty());
    }

    /// REQ-958: the node is asked for when it is not there while the row says there is a list, and again,
    /// not at every frame, when it goes.
    #[test]
    fn the_node_is_asked_for_again_when_it_goes_while_the_row_says_there_is_a_list() {
        let mut v = TasksView::new("w1");
        v.want(false, true, 0);
        assert_eq!(v.take_asks(), [Ask::Open("w1".into())]);
        v.want(false, true, 100);
        assert!(v.take_asks().is_empty(), "not at every frame");
        v.want(false, true, 3000);
        assert_eq!(v.take_asks(), [Ask::Open("w1".into())]);
        // it came: nothing, and when it goes again it is asked for at once
        v.want(true, true, 3100);
        v.want(false, true, 3200);
        assert_eq!(v.take_asks(), [Ask::Open("w1".into())]);
        // a row that says there is no list asks for nothing
        let mut none = TasksView::new("w1");
        none.want(false, false, 0);
        assert!(none.take_asks().is_empty());
    }

    #[test]
    fn frames_fit_the_terminal_at_every_size() {
        let doc = fixture("snapshot-tasks-stopped.json");
        for (w, h) in [(100, 30), (72, 20), (60, 12), (40, 8), (20, 5), (8, 3)] {
            let mut v = TasksView::new("w");
            v.press(Some(&doc), KeyCode::Char('b'));
            for rows in [
                text(&v, Some(&doc), w, h),
                text(&TasksView::new("w"), None, w, h),
            ] {
                assert_eq!(rows.len(), h);
                assert!(rows.iter().all(|r| r.chars().count() <= w), "{w}x{h}");
            }
        }
        // a list longer than the screen keeps the cursor in view
        let mut long = fixture("snapshot-tasks.json");
        let template = long.tasks[0].clone();
        for n in 0..30 {
            let mut t = template.clone();
            t.id = format!("x{n}");
            t.level = 6 + n;
            long.tasks.push(t);
        }
        let mut v = TasksView::new("w");
        for _ in 0..30 {
            v.press(Some(&long), KeyCode::Char('j'));
        }
        let rows = text(&v, Some(&long), 80, 10);
        assert!(rows.iter().any(|r| r.contains(" x24 ")), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains(" t1 ")));
    }
}
