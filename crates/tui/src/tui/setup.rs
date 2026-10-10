//! The setup step of a project's first worktree (addendum 10): the user
//! builds the layout, picks the agent and checks the setup, and jw writes
//! `.jw.toml` from it (REQ-115–117).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect as TRect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::actions::Project;
use crate::core::registry::Entry;
use crate::init::{Draft, ProjectSetup};
use crate::layout::{Dir, Node, Rect};
use crate::theme::p;

/// The builder's height in rows, borders included.
const BUILDER_ROWS: u16 = 11;
/// The smallest box the builder still draws with its label.
const MIN_BOX: (u16, u16) = (8, 3);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    Layout,
    Agent,
    Setup,
}

/// One setup line: a command to run, or an env file to copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub env: bool,
    pub text: String,
    pub on: bool,
}

/// `␣` on a box: what runs in it. Row 3 is a typed command.
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    row: usize,
    text: String,
}

const PICKS: [&str; 3] = ["editor", "agent", "shell"];

pub struct Setup {
    pub from: Entry,
    pub project: Project,
    /// The worktree's name, from the step before.
    pub name: String,
    pub tree: Node,
    /// The selected box, in leaf order.
    pub sel: usize,
    section: Section,
    pick: Option<Pick>,
    pub codex: bool,
    pub items: Vec<Item>,
    row: usize,
    editing: bool,
    /// Why the last key did nothing.
    note: Option<String>,
}

/// What a key did to the step.
pub enum Done {
    Stay,
    /// `esc`: back to the name.
    Back,
    Submit,
}

impl Setup {
    pub fn new(from: Entry, project: Project, name: String, draft: &Draft) -> Self {
        let items = draft
            .setup
            .iter()
            .map(|t| Item {
                env: false,
                text: t.clone(),
                on: true,
            })
            .chain(draft.env.iter().map(|e| Item {
                env: true,
                text: e.file.clone(),
                on: true,
            }))
            .collect();
        Self {
            from,
            project,
            name,
            tree: Node::default_tree(),
            sel: 0,
            section: Section::Layout,
            pick: None,
            codex: false,
            items,
            row: 0,
            editing: false,
            note: None,
        }
    }

    /// What `↵` writes.
    pub fn choice(&self) -> ProjectSetup {
        let on = |env: bool| {
            self.items
                .iter()
                .filter(|i| i.on && i.env == env && !i.text.trim().is_empty())
                .map(|i| i.text.trim().to_string())
                .collect()
        };
        ProjectSetup {
            setup: on(false),
            env: on(true),
            agent: self.agent().into(),
            layout: self.tree.clone(),
        }
    }

    fn agent(&self) -> &'static str {
        if self.codex { "codex" } else { "claude" }
    }

    pub fn key(&mut self, k: KeyEvent) -> Done {
        self.note = None;
        if let Some(pick) = &mut self.pick {
            match k.code {
                KeyCode::Esc => self.pick = None,
                KeyCode::Up => pick.row = pick.row.saturating_sub(1),
                KeyCode::Down => pick.row = (pick.row + 1).min(3),
                KeyCode::Enter => {
                    let run = match pick.row {
                        3 => pick.text.trim().to_string(),
                        r => PICKS[r].to_string(),
                    };
                    if !run.is_empty() {
                        if let Some(leaf) = nth_leaf(&mut self.tree, self.sel) {
                            leaf.run = Some(run);
                        }
                        self.pick = None;
                    }
                }
                KeyCode::Backspace if pick.row == 3 => {
                    pick.text.pop();
                }
                KeyCode::Char(c) if pick.row == 3 && pick.text.chars().count() < 60 => {
                    pick.text.push(c)
                }
                _ => {}
            }
            return Done::Stay;
        }
        if self.editing {
            let item = &mut self.items[self.row];
            match k.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.editing = false;
                    if item.text.trim().is_empty() {
                        self.items.remove(self.row);
                        self.row = self.row.min(self.items.len().saturating_sub(1));
                    }
                }
                KeyCode::Backspace => {
                    item.text.pop();
                }
                KeyCode::Char(c) => item.text.push(c),
                _ => {}
            }
            return Done::Stay;
        }
        match k.code {
            KeyCode::Esc => return Done::Back,
            KeyCode::Enter => return Done::Submit,
            KeyCode::Tab => {
                self.section = match self.section {
                    Section::Layout => Section::Agent,
                    Section::Agent => Section::Setup,
                    Section::Setup => Section::Layout,
                }
            }
            KeyCode::BackTab => {
                self.section = match self.section {
                    Section::Layout => Section::Setup,
                    Section::Agent => Section::Layout,
                    Section::Setup => Section::Agent,
                }
            }
            _ => match self.section {
                Section::Layout => self.layout_key(k),
                Section::Agent => {
                    if matches!(
                        k.code,
                        KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l' | ' ')
                    ) {
                        self.codex = !self.codex;
                    }
                }
                Section::Setup => self.setup_key(k),
            },
        }
        Done::Stay
    }

    /// REQ-116: select, move, split, close, resize, and what runs.
    fn layout_key(&mut self, k: KeyEvent) {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let dir = match k.code {
            KeyCode::Left | KeyCode::Char('h' | 'H') => Some((-1, 0)),
            KeyCode::Right | KeyCode::Char('l' | 'L') => Some((1, 0)),
            KeyCode::Up | KeyCode::Char('k' | 'K') => Some((0, -1)),
            KeyCode::Down | KeyCode::Char('j' | 'J') => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = dir {
            let to = neighbour(&self.tree, self.sel, dx, dy);
            let swap = shift || matches!(k.code, KeyCode::Char('H' | 'J' | 'K' | 'L'));
            if swap && to != self.sel {
                let runs = self.tree.leaves();
                let (a, b) = (runs[self.sel].run.clone(), runs[to].run.clone());
                if let Some(l) = nth_leaf(&mut self.tree, self.sel) {
                    l.run = Some(b);
                }
                if let Some(l) = nth_leaf(&mut self.tree, to) {
                    l.run = Some(a);
                }
            }
            self.sel = to;
            return;
        }
        match k.code {
            KeyCode::Char(c @ ('|' | '-')) => {
                let dir = if c == '|' { Dir::Right } else { Dir::Down };
                let mut next = self.tree.clone();
                if let Some(leaf) = nth_leaf(&mut next, self.sel) {
                    let old = std::mem::take(leaf);
                    *leaf = Node::split(dir, 0.5, old, Node::leaf("shell"));
                }
                if fits(&next) {
                    self.tree = next;
                    self.sel += 1;
                } else {
                    self.note = Some("No room for another pane there.".into());
                }
            }
            KeyCode::Char('x') => {
                if self.tree.run.is_some() {
                    self.note = Some("A layout needs one pane at least.".into());
                } else {
                    remove_leaf(&mut self.tree, self.sel);
                    self.sel = self.sel.min(self.tree.leaves().len() - 1);
                }
            }
            KeyCode::Char(c @ ('<' | '>')) => {
                resize(&mut self.tree, self.sel, c == '>');
            }
            KeyCode::Char(' ') => {
                let run = self.tree.leaves()[self.sel].run.clone();
                self.pick = Some(match PICKS.iter().position(|p| *p == run) {
                    Some(row) => Pick {
                        row,
                        text: String::new(),
                    },
                    None => Pick { row: 3, text: run },
                });
            }
            _ => {}
        }
    }

    fn setup_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.row = self.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.row = (self.row + 1).min(self.items.len().saturating_sub(1))
            }
            KeyCode::Char(' ') => {
                if let Some(i) = self.items.get_mut(self.row) {
                    i.on = !i.on;
                }
            }
            KeyCode::Char('e') if !self.items.is_empty() => self.editing = true,
            KeyCode::Char('a') => {
                let at = self.items.iter().filter(|i| !i.env).count();
                self.items.insert(
                    at,
                    Item {
                        env: false,
                        text: String::new(),
                        on: true,
                    },
                );
                self.row = at;
                self.editing = true;
            }
            _ => {}
        }
    }

    pub fn draw(&self, f: &mut Frame) {
        let dim = Style::default().fg(p().dim);
        let blue = Style::default().fg(p().blue);
        let head = |s: Section, label: &str| {
            let on = self.section == s;
            Span::styled(
                format!(" {} {label}", if on { "▸" } else { " " }),
                if on { blue } else { dim },
            )
        };
        let n = self.tree.leaves().len();
        let top = vec![
            Line::from(Span::styled(
                " First worktree in this project. jw saves these answers in .jw.toml.",
                dim,
            )),
            Line::default(),
            Line::from(vec![
                head(Section::Layout, "Layout"),
                Span::styled(
                    format!("        {n} pane{}", if n == 1 { "" } else { "s" }),
                    dim,
                ),
            ]),
        ];

        let mut bottom = Vec::new();
        if let Some(pick) = &self.pick {
            let rows = [
                ("editor ", "plain nvim".to_string()),
                ("agent  ", self.agent().to_string()),
                ("shell  ", "your shell".to_string()),
                (
                    "command",
                    if pick.row == 3 {
                        format!("{}▏", pick.text)
                    } else {
                        "type one, e.g. pnpm dev".into()
                    },
                ),
            ];
            for (i, (k, v)) in rows.iter().enumerate() {
                let on = pick.row == i;
                bottom.push(Line::from(vec![
                    Span::styled(if on { "   › " } else { "     " }, blue),
                    Span::styled(format!("{k} "), if on { blue } else { dim }),
                    Span::raw(v.clone()),
                ]));
            }
        }
        if let Some(note) = &self.note {
            bottom.push(Line::from(Span::styled(
                format!(" {note}"),
                Style::default().fg(p().yellow),
            )));
        }
        bottom.push(Line::default());
        let choice = |on: bool, s: &str| {
            if on {
                Span::styled(format!("(●) {s}   "), blue)
            } else {
                Span::styled(format!("( ) {s}   "), dim)
            }
        };
        bottom.push(Line::from(vec![
            head(Section::Agent, "Agent    "),
            choice(!self.codex, "claude"),
            choice(self.codex, "codex"),
        ]));
        bottom.push(Line::default());
        bottom.push(Line::from(vec![
            head(Section::Setup, "Setup"),
            Span::styled(
                "        detected from the lockfiles and ignored .env files",
                dim,
            ),
        ]));
        if self.items.is_empty() {
            bottom.push(Line::from(Span::styled(
                "     nothing found; a adds a command",
                dim,
            )));
        }
        for (i, it) in self.items.iter().enumerate() {
            let at = self.section == Section::Setup && self.row == i;
            let text = if at && self.editing {
                format!("{}▏", it.text)
            } else {
                it.text.clone()
            };
            let mut line = Line::from(vec![
                Span::raw("   "),
                if it.on {
                    Span::styled("[x] ", Style::default().fg(p().green))
                } else {
                    Span::styled("[ ] ", dim)
                },
                Span::styled(if it.env { "copy " } else { "run  " }, dim),
                if it.on {
                    Span::raw(text)
                } else {
                    Span::styled(text, dim)
                },
            ]);
            if at {
                line = line.style(Style::default().bg(p().sel));
            }
            bottom.push(line);
        }
        bottom.push(Line::default());
        let [what, leave] = self.hints();
        bottom.push(keys(&what));
        bottom.push(keys(&leave));

        let area = f.area();
        let w = 84.min(area.width.saturating_sub(4));
        let h = (top.len() as u16 + BUILDER_ROWS + bottom.len() as u16 + 2).min(area.height);
        let r = TRect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height.saturating_sub(h) / 3,
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(blue)
            .title(Span::styled(
                format!(" Set up {} ", self.project.name),
                blue.add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(p().panel).fg(p().fg));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let top_h = (top.len() as u16).min(inner.height);
        f.render_widget(
            Paragraph::new(top),
            TRect::new(inner.x, inner.y, inner.width, top_h),
        );
        let bh = BUILDER_ROWS.min(inner.height.saturating_sub(top_h));
        let builder = TRect::new(
            inner.x + 2,
            inner.y + top_h,
            inner.width.saturating_sub(4),
            bh,
        );
        self.draw_builder(f, builder);
        let rest = inner.height.saturating_sub(top_h + bh);
        f.render_widget(
            Paragraph::new(bottom),
            TRect::new(inner.x, inner.y + top_h + bh, inner.width, rest),
        );
    }

    fn draw_builder(&self, f: &mut Frame, area: TRect) {
        let on = self.section == Section::Layout;
        let frame = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if on { p().blue } else { p().line }));
        let inner = frame.inner(area);
        f.render_widget(frame, area);
        let rects = self.tree.rects(Rect {
            x: inner.x,
            y: inner.y,
            w: inner.width,
            h: inner.height,
        });
        let agent = self.agent();
        for (i, (leaf, r)) in self.tree.leaves().iter().zip(rects).enumerate() {
            let sel = on && i == self.sel;
            let (kind, what) = label(&leaf.run, agent);
            let style = if sel {
                Style::default().fg(p().fg).bg(p().sel)
            } else {
                Style::default().fg(p().dim)
            };
            let b = Block::default()
                .borders(Borders::ALL)
                .border_type(if sel {
                    BorderType::Rounded
                } else {
                    BorderType::Plain
                })
                .border_style(Style::default().fg(if sel { p().blue } else { p().line }))
                .title(Span::styled(
                    format!("{}", i + 1),
                    Style::default().fg(p().dim),
                ))
                .style(style);
            let box_r = TRect::new(r.x, r.y, r.w, r.h);
            let text_r = b.inner(box_r);
            f.render_widget(b, box_r);
            let mut lines = vec![Line::from(Span::styled(
                kind,
                if sel {
                    Style::default().fg(p().blue).add_modifier(Modifier::BOLD)
                } else {
                    style.add_modifier(Modifier::BOLD)
                },
            ))];
            if let Some(w) = what {
                lines.push(Line::from(w));
            }
            let pad = text_r.height.saturating_sub(lines.len() as u16) / 2;
            let text_r = TRect::new(text_r.x, text_r.y + pad, text_r.width, text_r.height - pad);
            f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), text_r);
        }
    }

    /// The key hints: what the section does, then how to leave it.
    fn hints(&self) -> [Vec<(&'static str, &'static str)>; 2] {
        if self.pick.is_some() {
            return [
                vec![("↑↓", "pick"), ("↵", "put it in the box")],
                vec![("esc", "cancel")],
            ];
        }
        if self.editing {
            return [vec![("type", "the line")], vec![("↵", "done")]];
        }
        let leave = vec![
            ("tab", "next section"),
            ("↵", "save and create"),
            ("esc", "back"),
        ];
        let what = match self.section {
            Section::Layout => vec![
                ("←↑↓→", "box"),
                ("⇧ arrow", "move it"),
                ("| -", "split"),
                ("x", "close"),
                ("< >", "size"),
                ("␣", "what runs"),
            ],
            Section::Agent => vec![("← →", "claude or codex")],
            Section::Setup => vec![("↑↓", "line"), ("␣", "on/off"), ("e", "edit"), ("a", "add")],
        };
        [what, leave]
    }
}

/// How a box names what runs in it.
fn label(run: &str, agent: &str) -> (String, Option<String>) {
    match run {
        "editor" => ("editor".into(), Some("nvim".into())),
        "agent" => ("agent".into(), Some(agent.into())),
        "shell" => ("shell".into(), None),
        cmd => ("command".into(), Some(cmd.into())),
    }
}

/// Six hints share a line here: a narrower gap than the modals'.
fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    super::modal::key_line(pairs, 2)
}

/// The builder's inner area, for fitting and moving.
const VIRTUAL: Rect = Rect {
    x: 0,
    y: 0,
    w: 76,
    h: BUILDER_ROWS - 2,
};

/// Whether every box of `t` still fits the builder with its label.
fn fits(t: &Node) -> bool {
    t.rects(VIRTUAL)
        .iter()
        .all(|r| r.w >= MIN_BOX.0 && r.h >= MIN_BOX.1)
}

/// The box beside box `from` towards `(dx, dy)`: the nearest whose edge
/// faces it, or `from` itself at the edge.
fn neighbour(t: &Node, from: usize, dx: i32, dy: i32) -> usize {
    let rs = t.rects(VIRTUAL);
    let s = rs[from];
    let c = |r: &Rect| (r.x as i32 * 2 + r.w as i32, r.y as i32 * 2 + r.h as i32);
    let (sx, sy) = c(&s);
    let mut best = (from, i32::MAX);
    for (i, r) in rs.iter().enumerate() {
        let faces = match (dx, dy) {
            (-1, _) => r.x + r.w <= s.x,
            (1, _) => r.x >= s.x + s.w,
            (_, -1) => r.y + r.h <= s.y,
            _ => r.y >= s.y + s.h,
        };
        if i == from || !faces {
            continue;
        }
        let (x, y) = c(r);
        let d = (x - sx).abs() + (y - sy).abs();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Leaf number `i` of `t`, in leaf order.
fn nth_leaf(t: &mut Node, i: usize) -> Option<&mut Node> {
    fn go<'a>(t: &'a mut Node, i: &mut usize) -> Option<&'a mut Node> {
        if t.run.is_some() {
            if *i == 0 {
                return Some(t);
            }
            *i -= 1;
            return None;
        }
        if let Some(a) = t.a.as_deref_mut()
            && let Some(l) = go(a, i)
        {
            return Some(l);
        }
        t.b.as_deref_mut().and_then(|b| go(b, i))
    }
    go(t, &mut { i })
}

/// Removes leaf number `i`; its sibling takes the split's place. A tree
/// that is one leaf stays as it is.
fn remove_leaf(t: &mut Node, i: usize) {
    let (Some(a), Some(b)) = (t.a.as_deref_mut(), t.b.as_deref_mut()) else {
        return;
    };
    let na = a.leaves().len();
    let (target, in_b, idx) = if i < na {
        (a, false, i)
    } else {
        (b, true, i - na)
    };
    if target.run.is_some() {
        if let Some(keep) = if in_b { t.a.take() } else { t.b.take() } {
            *t = *keep;
        }
        return;
    }
    remove_leaf(target, idx)
}

/// Moves the split that holds leaf `i` by 0.1, within 0.2–0.8, so that box
/// grows (`grow`) or shrinks.
fn resize(t: &mut Node, i: usize, grow: bool) {
    let (Some(a), Some(b)) = (t.a.as_deref_mut(), t.b.as_deref_mut()) else {
        return;
    };
    let na = a.leaves().len();
    let in_a = i < na;
    let child = if in_a { &*a } else { &*b };
    if child.run.is_some() {
        let r = t.ratio.unwrap_or(0.5);
        let step = if grow == in_a { 0.1 } else { -0.1 };
        t.ratio = Some(((r + step).clamp(0.2, 0.8) * 10.0).round() / 10.0);
        return;
    }
    if in_a {
        resize(a, i, grow)
    } else {
        resize(b, i - na, grow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(t: &Node) -> Vec<String> {
        t.leaves().into_iter().map(|l| l.run).collect()
    }

    /// REQ-116: split, move, resize and close on the tree the builder edits.
    #[test]
    fn the_builder_edits_its_tree() {
        let mut t = Node::leaf("shell");
        *nth_leaf(&mut t, 0).unwrap() =
            Node::split(Dir::Right, 0.5, Node::leaf("editor"), Node::leaf("agent"));
        assert_eq!(runs(&t), ["editor", "agent"]);
        assert_eq!(neighbour(&t, 0, 1, 0), 1);
        assert_eq!(neighbour(&t, 1, 1, 0), 1);
        assert_eq!(neighbour(&t, 1, -1, 0), 0);

        resize(&mut t, 0, true);
        assert_eq!(t.ratio, Some(0.6));
        resize(&mut t, 1, true);
        resize(&mut t, 1, true);
        assert_eq!(t.ratio, Some(0.4));
        for _ in 0..9 {
            resize(&mut t, 0, false);
        }
        assert_eq!(t.ratio, Some(0.2));

        let b = nth_leaf(&mut t, 1).unwrap();
        let old = std::mem::take(b);
        *b = Node::split(Dir::Down, 0.5, old, Node::leaf("shell"));
        assert_eq!(runs(&t), ["editor", "agent", "shell"]);
        assert_eq!(neighbour(&t, 1, 0, 1), 2);
        assert_eq!(neighbour(&t, 2, -1, 0), 0);
        assert!(fits(&t));

        remove_leaf(&mut t, 0);
        assert_eq!(runs(&t), ["agent", "shell"]);
        assert_eq!(t.split, Some(Dir::Down));
        remove_leaf(&mut t, 1);
        assert_eq!(t, Node::leaf("agent"));
    }

    fn step() -> Setup {
        let project = Project {
            name: "demo".into(),
            repo: crate::connectors::git::Repo {
                root: "/r".into(),
                common_dir: "/r/.git".into(),
                remote: String::new(),
            },
            cfg: Default::default(),
        };
        let draft = Draft {
            setup: vec!["pnpm install".into()],
            env: vec![crate::init::DraftEnv {
                file: ".env.local".into(),
                localhost: vec![],
            }],
            ..Draft::default()
        };
        Setup::new(Entry::default(), project, "ws-1".into(), &draft)
    }

    fn press(s: &mut Setup, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '↑' => KeyCode::Up,
                '↓' => KeyCode::Down,
                '↵' => KeyCode::Enter,
                '⇥' => KeyCode::Tab,
                c => KeyCode::Char(c),
            };
            s.key(KeyEvent::new(code, KeyModifiers::NONE));
            let mut t =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
            t.draw(|f| s.draw(f)).unwrap();
        }
    }

    /// REQ-115–117 through the keys: build agent | editor over a command,
    /// turn the env file off, pick codex.
    #[test]
    fn the_step_builds_what_the_keys_say() {
        let mut s = step();
        press(&mut s, "| ↑↵h ↑↑↵L- ↓pnpm dev↵<");
        assert_eq!(runs(&s.tree), ["agent", "editor", "pnpm dev"]);
        press(&mut s, "⇥→⇥↓ ");
        let c = s.choice();
        assert_eq!(c.agent, "claude");
        assert_eq!((c.setup, c.env), (vec!["pnpm install".to_string()], vec![]));
        press(&mut s, "⇥⇥ ");
        assert_eq!(s.choice().agent, "codex");
    }

    #[test]
    fn tiny_boxes_dont_fit() {
        let mut t = Node::leaf("shell");
        for _ in 0..4 {
            let n = t.leaves().len() - 1;
            let l = nth_leaf(&mut t, n).unwrap();
            let old = std::mem::take(l);
            *l = Node::split(Dir::Down, 0.5, old, Node::leaf("shell"));
        }
        assert!(!fits(&t));
    }
}
