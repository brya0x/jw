//! `jwn tui`: the terminal client, laid out like the jw TUI it replaces (prototype:
//! `docs/prototypes/tui.html`). A sidebar of workspaces with the marks of what waits, a row
//! of facts on top, the open workspace's panes as rounded boxes, a status bar with the mode.
//! `a` lists what waits. It draws the same documents the phone draws and keeps its own seen
//! record. `View` is pure (documents in, a character grid out) so it is tested without a
//! terminal; `run` only moves bytes between the socket, the keyboard and the screen.

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::time::Duration;

use contract::{Attention, AttentionKind, Cmd, Node, Pane, Session, Workspace, WorkspaceKind};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use wire::Client;

/// The One Dark palette of the mobile prototype, as 24-bit colours.
type Rgb = (u8, u8, u8);
const FG: Rgb = (0xab, 0xb2, 0xbf);
const BG: Rgb = (0x28, 0x2c, 0x34);
const PANEL: Rgb = (0x21, 0x25, 0x2b);
const DIM: Rgb = (0x5c, 0x63, 0x70);
const LINE: Rgb = (0x3e, 0x44, 0x51);
const SEL: Rgb = (0x2c, 0x31, 0x3a);
const BLUE: Rgb = (0x61, 0xaf, 0xef);
const GREEN: Rgb = (0x98, 0xc3, 0x79);
const YELLOW: Rgb = (0xe5, 0xc0, 0x7b);
const RED: Rgb = (0xe0, 0x6c, 0x75);
const MAGENTA: Rgb = (0xc6, 0x78, 0xdd);
const CYAN: Rgb = (0x56, 0xb6, 0xc2);

const SIDEBAR: usize = 28;

#[derive(Clone, Copy, PartialEq, Default)]
struct Style {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    reverse: bool,
}

impl Style {
    fn fg(c: Rgb) -> Style {
        Style {
            fg: Some(c),
            ..Style::default()
        }
    }

    fn bold(mut self) -> Style {
        self.bold = true;
        self
    }

    fn on(mut self, bg: Rgb) -> Style {
        self.bg = Some(bg);
        self
    }
}

/// A screen of character cells; drawing writes cells, `ansi` turns them into escapes.
struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<(char, Style)>,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Grid {
        Grid {
            cols,
            rows,
            cells: vec![(' ', Style::default()); cols * rows],
        }
    }

    fn put(&mut self, x: usize, y: usize, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            if y < self.rows && x + i < self.cols {
                self.cells[y * self.cols + x + i] = (ch, style);
            }
        }
    }

    /// Put `text` keeping the background already under it.
    fn put_over(&mut self, x: usize, y: usize, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            if y < self.rows && x + i < self.cols {
                let under = self.cells[y * self.cols + x + i].1;
                self.cells[y * self.cols + x + i] = (
                    ch,
                    Style {
                        bg: under.bg,
                        ..style
                    },
                );
            }
        }
    }

    fn paint_bg(&mut self, x: usize, y: usize, w: usize, bg: Rgb) {
        for i in 0..w {
            if y < self.rows && x + i < self.cols {
                self.cells[y * self.cols + x + i].1.bg = Some(bg);
            }
        }
    }

    /// A rounded box; the title sits on the top border at the left, `badge` at the right.
    fn boxed(
        &mut self,
        (x, y, w, h): (usize, usize, usize, usize),
        title: &str,
        border: Rgb,
        title_style: Style,
        badge: Option<(String, Style)>,
    ) {
        if w < 4 || h < 2 {
            return;
        }
        let b = Style::fg(border);
        self.put(x, y, &format!("╭{}╮", "─".repeat(w - 2)), b);
        self.put(x, y + h - 1, &format!("╰{}╯", "─".repeat(w - 2)), b);
        for r in 1..h - 1 {
            self.put(x, y + r, "│", b);
            self.put(x + w - 1, y + r, "│", b);
        }
        self.put(
            x + 2,
            y,
            &format!(" {} ", clip(title, w.saturating_sub(6))),
            title_style,
        );
        if let Some((text, style)) = badge {
            let n = text.chars().count() + 2;
            if n + title.chars().count() + 6 < w {
                self.put(x + w - 2 - n, y, &format!(" {text} "), style);
            }
        }
    }

    fn rows_ansi(&self) -> Vec<String> {
        (0..self.rows)
            .map(|y| {
                let mut out = String::new();
                let mut cur: Option<Style> = None;
                for x in 0..self.cols {
                    let (ch, st) = self.cells[y * self.cols + x];
                    if cur != Some(st) {
                        out.push_str("\x1b[0m");
                        if st.bold {
                            out.push_str("\x1b[1m");
                        }
                        if st.reverse {
                            out.push_str("\x1b[7m");
                        }
                        // The screen paints its own One Dark ground, whatever the terminal's theme.
                        let (f, b) = (st.fg.unwrap_or(FG), st.bg.unwrap_or(BG));
                        out.push_str(&format!(
                            "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m",
                            f.0, f.1, f.2, b.0, b.1, b.2
                        ));
                        cur = Some(st);
                    }
                    out.push(ch);
                }
                out.push_str("\x1b[0m");
                out
            })
            .collect()
    }

    fn rows_plain(&self) -> Vec<String> {
        (0..self.rows)
            .map(|y| {
                self.cells[y * self.cols..(y + 1) * self.cols]
                    .iter()
                    .map(|c| c.0)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }
}

fn clip(text: &str, cols: usize) -> String {
    if text.chars().count() <= cols {
        return text.to_owned();
    }
    text.chars()
        .take(cols.saturating_sub(1))
        .chain(['…'])
        .collect()
}

/// Most urgent first: the order of the legend.
const URGENCY: [AttentionKind; 8] = [
    AttentionKind::Permission,
    AttentionKind::Question,
    AttentionKind::Plan,
    AttentionKind::CiFailed,
    AttentionKind::ShellDone,
    AttentionKind::Screen,
    AttentionKind::AgentDone,
    AttentionKind::Merged,
];

fn mark(a: &Attention) -> (&'static str, Rgb) {
    match a.kind {
        AttentionKind::Permission => ("!", RED),
        AttentionKind::Question => ("?", YELLOW),
        AttentionKind::Plan => ("◆", MAGENTA),
        AttentionKind::CiFailed => ("✗", RED),
        AttentionKind::ShellDone if a.exit_code.unwrap_or(0) != 0 => ("✗", RED),
        AttentionKind::ShellDone => ("❯", GREEN),
        AttentionKind::Screen => ("…", DIM),
        AttentionKind::AgentDone => ("✓", GREEN),
        AttentionKind::Merged => ("⚑", CYAN),
    }
}

fn what(a: &Attention) -> String {
    match a.kind {
        AttentionKind::Permission => "pide permiso".into(),
        AttentionKind::Question => "hace una pregunta".into(),
        AttentionKind::Plan => "plan listo".into(),
        AttentionKind::CiFailed => "CI falló".into(),
        AttentionKind::ShellDone => format!("salió {}", a.exit_code.unwrap_or(0)),
        AttentionKind::Screen => "espera en pantalla".into(),
        AttentionKind::AgentDone => "agente listo".into(),
        AttentionKind::Merged => "mergeado".into(),
    }
}

/// Only what needs a decision or went wrong is tinted; the rest stays dim.
fn tone(a: &Attention) -> Rgb {
    match mark(a).1 {
        c if c == GREEN || c == DIM || c == CYAN => DIM,
        c => c,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Workspaces,
    Avisos,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Avisos,
    NewPane,
    Back,
    Quit,
}

#[derive(PartialEq, Eq, Debug)]
pub enum After {
    Continue,
    Quit,
    /// Ask the daemon for a new pane in this workspace.
    NewPane(String),
}

fn rev_of(n: &Node) -> u64 {
    match n {
        Node::Session(s) => s.head.rev,
        Node::Workspace(w) => w.head.rev,
        Node::Pane(p) => p.head.rev,
    }
}

pub struct View {
    session: Option<Session>,
    workspaces: HashMap<String, Workspace>,
    panes: HashMap<String, Pane>,
    revs: HashMap<String, u64>,
    /// Node id -> revision this terminal had it open at. Never sent anywhere.
    seen: HashMap<String, u64>,
    mode: Mode,
    /// The highlighted workspace.
    selected: usize,
    avisos_selected: usize,
    /// The workspace opened with Enter, and which of its panes has focus. The highlight alone
    /// only previews.
    opened: Option<String>,
    focus: usize,
    status: String,
    /// The daemon epoch the seen record belongs to.
    epoch: Option<String>,
    /// What this terminal looked at and the daemon has not been told yet: (node, revision).
    outbox: Vec<(String, u64)>,
}

impl View {
    pub fn new(status: &str) -> View {
        View {
            session: None,
            workspaces: HashMap::new(),
            panes: HashMap::new(),
            revs: HashMap::new(),
            seen: HashMap::new(),
            mode: Mode::Workspaces,
            selected: 0,
            avisos_selected: 0,
            opened: None,
            focus: 0,
            status: status.to_owned(),
            epoch: None,
            outbox: Vec::new(),
        }
    }

    /// The looks to tell the daemon about, so every client stops showing them.
    pub fn take_outbox(&mut self) -> Vec<(String, u64)> {
        std::mem::take(&mut self.outbox)
    }

    /// Revisions only mean something inside one daemon epoch: a restarted daemon starts over,
    /// so what this terminal had seen is forgotten.
    pub fn set_epoch(&mut self, epoch: &str) {
        if self.epoch.as_deref() != Some(epoch) {
            self.epoch = Some(epoch.to_owned());
            self.seen.clear();
            self.opened = None;
            self.focus = 0;
        }
    }

    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_owned();
    }

    /// Replace what is drawn with the documents the client currently holds.
    pub fn sync<'a>(&mut self, nodes: impl Iterator<Item = &'a serde_json::Value>) {
        self.session = None;
        self.workspaces.clear();
        self.panes.clear();
        self.revs.clear();
        for doc in nodes {
            let Ok(node) = serde_json::from_value::<Node>(doc.clone()) else {
                continue;
            };
            let id = contract::node_id(&node).clone();
            self.revs.insert(id.clone(), rev_of(&node));
            match node {
                Node::Session(s) => self.session = Some(s),
                Node::Workspace(w) => {
                    self.workspaces.insert(id, w);
                }
                Node::Pane(p) => {
                    self.panes.insert(id, p);
                }
            }
        }
    }

    /// Nodes this view would draw but does not have yet.
    pub fn wants(&self) -> Vec<String> {
        let Some(s) = &self.session else {
            return vec![];
        };
        let mut ids: Vec<String> = s.workspaces.iter().map(|w| w.id.clone()).collect();
        if let Some(w) = self.shown() {
            ids.extend(w.panes.iter().map(|p| p.id.clone()));
        }
        ids.retain(|id| !self.revs.contains_key(id));
        ids
    }

    fn selected_id(&self) -> Option<String> {
        self.session
            .as_ref()?
            .workspaces
            .get(self.selected)
            .map(|w| w.id.clone())
    }

    /// The workspace on the right: the highlighted one.
    fn shown(&self) -> Option<&Workspace> {
        self.workspaces.get(&self.selected_id()?)
    }

    fn unseen(&self, list: &[Attention]) -> Vec<Attention> {
        let mut u: Vec<Attention> = list
            .iter()
            .filter(|a| a.rev > self.seen.get(&a.node).copied().unwrap_or(0))
            .cloned()
            .collect();
        u.sort_by_key(|a| URGENCY.iter().position(|k| *k == a.kind));
        u
    }

    fn mark_seen(&mut self, id: &str) {
        let rev = self.revs.get(id).copied().unwrap_or_else(|| {
            // Not opened yet: seen at the newest attention raised on it.
            self.session
                .iter()
                .flat_map(|s| s.workspaces.iter().flat_map(|w| w.attention.iter()))
                .filter(|a| a.node == id)
                .map(|a| a.rev)
                .max()
                .unwrap_or(0)
        });
        let e = self.seen.entry(id.to_owned()).or_insert(0);
        *e = (*e).max(rev);
        if rev > 0 {
            self.outbox.push((id.to_owned(), rev));
        }
    }

    fn avisos(&self) -> Vec<(String, String, Attention)> {
        let Some(s) = &self.session else {
            return vec![];
        };
        s.workspaces
            .iter()
            .flat_map(|w| {
                self.unseen(&w.attention)
                    .into_iter()
                    .map(|a| (w.id.clone(), self.label(&w.id), a))
            })
            .collect()
    }

    /// `project/branch` for a worktree, the name for a folder.
    fn label(&self, ws: &str) -> String {
        let Some(s) = &self.session else {
            return ws.to_owned();
        };
        let Some(w) = s.workspaces.iter().find(|w| w.id == ws) else {
            return ws.to_owned();
        };
        match (
            w.kind,
            self.workspaces.get(ws).and_then(|d| d.project.as_deref()),
        ) {
            (WorkspaceKind::Worktree, Some(p)) => {
                format!("{}/{}", base(p), w.branch.clone().unwrap_or_default())
            }
            _ => w.name.clone(),
        }
    }

    /// Open a workspace and focus one of its panes, marking both seen on this terminal.
    fn open(&mut self, ws: &str, focus: usize) {
        self.opened = Some(ws.to_owned());
        self.mark_seen(ws);
        let pane = self
            .workspaces
            .get(ws)
            .and_then(|d| d.panes.get(focus).or(d.panes.first()))
            .map(|p| p.id.clone());
        self.focus = focus.min(
            self.workspaces
                .get(ws)
                .map_or(0, |d| d.panes.len().saturating_sub(1)),
        );
        if let Some(p) = pane {
            self.mark_seen(&p);
        }
    }

    pub fn press(&mut self, key: Key) -> After {
        let n = self.session.as_ref().map_or(0, |s| s.workspaces.len());
        match (self.mode, key) {
            (_, Key::Quit) | (Mode::Workspaces, Key::Back) => return After::Quit,
            (Mode::Avisos, Key::Back | Key::Avisos) => self.mode = Mode::Workspaces,
            (Mode::Workspaces, Key::Avisos) => {
                self.mode = Mode::Avisos;
                self.avisos_selected = 0;
            }
            (Mode::Workspaces, Key::Down) => {
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
                self.opened = None;
                self.focus = 0;
            }
            (Mode::Workspaces, Key::Up) => {
                self.selected = self.selected.saturating_sub(1);
                self.opened = None;
                self.focus = 0;
            }
            (Mode::Workspaces, Key::Enter) => {
                if let Some(id) = self.selected_id() {
                    self.open(&id, 0);
                }
            }
            (Mode::Workspaces, Key::Left | Key::Right) => {
                if let Some(id) = self.selected_id() {
                    let panes = self.workspaces.get(&id).map_or(0, |d| d.panes.len());
                    if self.opened.as_deref() != Some(id.as_str()) {
                        self.open(&id, 0);
                    } else if panes > 0 {
                        let next = if key == Key::Right {
                            (self.focus + 1).min(panes - 1)
                        } else {
                            self.focus.saturating_sub(1)
                        };
                        self.open(&id, next);
                    }
                }
            }
            (Mode::Workspaces, Key::NewPane) => {
                if let Some(id) = self.selected_id() {
                    return After::NewPane(id);
                }
            }
            (Mode::Avisos, Key::Down) => {
                let last = self.avisos().len().saturating_sub(1);
                self.avisos_selected = (self.avisos_selected + 1).min(last);
            }
            (Mode::Avisos, Key::Up) => {
                self.avisos_selected = self.avisos_selected.saturating_sub(1)
            }
            (Mode::Avisos, Key::Enter) => {
                if let Some((ws, _, a)) = self.avisos().get(self.avisos_selected).cloned() {
                    // The attention names a pane, or the workspace itself.
                    let focus = self
                        .workspaces
                        .get(&ws)
                        .and_then(|d| d.panes.iter().position(|p| p.id == a.node))
                        .unwrap_or(0);
                    self.mode = Mode::Workspaces;
                    if let Some(i) = self
                        .session
                        .as_ref()
                        .and_then(|s| s.workspaces.iter().position(|w| w.id == ws))
                    {
                        self.selected = i;
                    }
                    self.open(&ws, focus);
                }
            }
            _ => {}
        }
        After::Continue
    }

    fn grid(&self, cols: usize, rows: usize) -> Grid {
        let mut g = Grid::new(cols, rows);
        let Some(session) = &self.session else {
            g.put(
                1,
                0,
                &format!("conectando… {}", self.status),
                Style::fg(DIM),
            );
            return g;
        };
        let dim = Style::fg(DIM);
        // The sidebar and its border.
        for y in 0..rows.saturating_sub(1) {
            g.put(SIDEBAR - 1, y, "│", Style::fg(LINE));
        }
        g.put(1, 0, &clip(&session.name, 12), dim.bold());
        g.put(SIDEBAR - 14, 0, "^␣ s sessions", dim);
        for (i, w) in session.workspaces.iter().enumerate() {
            let y = 2 + i;
            if y >= rows.saturating_sub(5) {
                break;
            }
            let on = i == self.selected && self.mode == Mode::Workspaces;
            let un = self.unseen(&w.attention);
            if on {
                g.paint_bg(0, y, SIDEBAR - 1, SEL);
            }
            let name = if w.kind == WorkspaceKind::Worktree {
                format!("↳ {}", w.branch.clone().unwrap_or_else(|| w.name.clone()))
            } else {
                w.name.clone()
            };
            let name_style = if on {
                Style::fg(BLUE).bold()
            } else if w.pane_count == 0 {
                dim
            } else {
                Style::fg(FG)
            };
            let number = if i < 9 {
                (i + 1).to_string()
            } else {
                " ".into()
            };
            g.put_over(0, y, &number, dim);
            g.put_over(
                2,
                y,
                if w.pane_count > 0 { "●" } else { "○" },
                if w.pane_count > 0 {
                    Style::fg(GREEN)
                } else {
                    dim
                },
            );
            g.put_over(4, y, &clip(&name, SIDEBAR - 10), name_style);
            if let Some(a) = un.first() {
                let (glyph, colour) = mark(a);
                g.put_over(SIDEBAR - 5, y, glyph, Style::fg(colour).bold());
                if un.len() > 1 {
                    g.put_over(SIDEBAR - 4, y, &un.len().to_string(), dim);
                }
            }
        }
        if session.workspaces.is_empty() {
            g.put(1, 2, "^␣ o abre una carpeta", dim);
        }
        for (i, line) in [
            "! permiso  ? pregunta",
            "◆ plan   ❯ comando",
            "✓ listo  ⚑ merge  ✗ falló",
        ]
        .iter()
        .enumerate()
        {
            g.put(1, rows.saturating_sub(4) + i, line, dim);
        }

        // The header row and the panes.
        let px = SIDEBAR + 1;
        let pw = cols.saturating_sub(px);
        if let Some(ws) = session.workspaces.get(self.selected) {
            let doc = self.workspaces.get(&ws.id);
            let mut x = px + 1;
            if let (WorkspaceKind::Worktree, Some(p)) =
                (ws.kind, doc.and_then(|d| d.project.as_deref()))
            {
                let project = format!("{}/", base(p));
                g.put(x, 0, &project, dim);
                x += project.chars().count();
            }
            let title = if ws.kind == WorkspaceKind::Worktree {
                ws.branch.clone().unwrap_or_else(|| ws.name.clone())
            } else {
                ws.name.clone()
            };
            g.put(x, 0, &title, Style::fg(BLUE).bold());
            x += title.chars().count() + 2;
            if ws.kind == WorkspaceKind::Worktree {
                g.put(
                    x,
                    0,
                    &format!("⎇ {}", ws.branch.clone().unwrap_or_default()),
                    Style::fg(MAGENTA),
                );
            }
            let path = &ws.path;
            g.put(cols.saturating_sub(path.chars().count() + 1), 0, path, dim);
            self.panes_area(&mut g, doc, (px, 1, pw, rows.saturating_sub(2)));
        }

        // The status bar.
        for x in 0..cols {
            g.cells[(rows - 1) * cols + x] = (' ', Style::default().on(PANEL));
        }
        let (chip, bg, hint) = match (self.mode, &self.opened) {
            (Mode::Avisos, _) => (" AVISOS ", YELLOW, "j/k mover · ↵ ir · esc volver"),
            (_, Some(_)) => (
                " TERM ",
                GREEN,
                "h/l panel · j/k workspace · n panel nuevo · a avisos · q salir",
            ),
            _ => (
                " MOVE ",
                BLUE,
                "j/k workspace · ↵ abrir · h/l panel · n panel nuevo · a avisos · q salir",
            ),
        };
        g.put(
            0,
            rows - 1,
            chip,
            Style {
                fg: Some(PANEL),
                bg: Some(bg),
                bold: true,
                reverse: false,
            },
        );
        g.put(
            chip.chars().count() + 1,
            rows - 1,
            &clip(hint, cols.saturating_sub(chip.chars().count() + 14)),
            dim.on(PANEL),
        );
        let waiting = self.avisos().len();
        if waiting > 0 {
            let t = format!("avisos {waiting}");
            g.put(
                cols.saturating_sub(t.chars().count() + 1),
                rows - 1,
                &t,
                Style::fg(YELLOW).bold().on(PANEL),
            );
        }
        if self.mode == Mode::Avisos {
            self.avisos_box(&mut g, (px, 1, pw, rows.saturating_sub(2)));
        }
        g
    }

    /// One pane fills the area; two sit side by side; from three, the first takes the left
    /// half and the rest stack on the right.
    fn panes_area(
        &self,
        g: &mut Grid,
        doc: Option<&Workspace>,
        (x, y, w, h): (usize, usize, usize, usize),
    ) {
        let Some(doc) = doc else { return };
        let n = doc.panes.len();
        if n == 0 {
            g.put(
                x + 2,
                y + 2,
                "este workspace no tiene paneles · n abre uno",
                Style::fg(DIM),
            );
            return;
        }
        let half = w / 2;
        let rects: Vec<(usize, usize, usize, usize)> = match n {
            1 => vec![(x, y, w, h)],
            2 => vec![(x, y, half, h), (x + half, y, w - half, h)],
            _ => {
                let rh = h / (n - 1);
                std::iter::once((x, y, half, h))
                    .chain((0..n - 1).map(|i| {
                        (
                            x + half,
                            y + i * rh,
                            w - half,
                            if i == n - 2 { h - i * rh } else { rh },
                        )
                    }))
                    .collect()
            }
        };
        let opened = self.opened.as_deref() == Some(doc.head.id.as_str());
        for (i, (summary, rect)) in doc.panes.iter().zip(rects).enumerate() {
            let focused = opened && i == self.focus;
            let un = self.unseen(&summary.attention);
            let badge = un.first().map(|a| {
                (
                    format!("{} {}", mark(a).0, what(a)),
                    Style::fg(tone(a)).bold(),
                )
            });
            let title_style = if focused {
                Style::fg(BLUE).bold()
            } else {
                Style::fg(FG).bold()
            };
            g.boxed(
                rect,
                &summary.title,
                if focused { BLUE } else { LINE },
                title_style,
                badge,
            );
            let (bx, by, bw, bh) = rect;
            let Some(p) = self.panes.get(&summary.id) else {
                g.put(bx + 2, by + 1, "abriendo el panel…", Style::fg(DIM));
                continue;
            };
            for (r, line) in p.screen.lines.iter().take(bh.saturating_sub(2)).enumerate() {
                g.put(
                    bx + 2,
                    by + 1 + r,
                    &clip(line, bw.saturating_sub(4)),
                    Style::fg(FG),
                );
            }
            let c = p.screen.cursor;
            if focused
                && c.visible
                && (c.row as usize) < bh.saturating_sub(2)
                && (c.col as usize) < bw.saturating_sub(4)
            {
                let at = (by + 1 + c.row as usize) * g.cols + bx + 2 + c.col as usize;
                g.cells[at].1.reverse = true;
            }
        }
    }

    fn avisos_box(&self, g: &mut Grid, (x, y, w, _): (usize, usize, usize, usize)) {
        let items = self.avisos();
        let bw = 56.min(w.saturating_sub(2));
        let bh = items.len().max(1) + 4;
        let bx = x + w.saturating_sub(bw) / 2;
        let by = y + 3;
        for r in 0..bh {
            for c in 0..bw {
                if by + r < g.rows && bx + c < g.cols {
                    g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(PANEL));
                }
            }
        }
        g.boxed(
            (bx, by, bw, bh),
            "avisos",
            YELLOW,
            Style::fg(FG).bold(),
            None,
        );
        // The box leaves the panel background under its border characters.
        if items.is_empty() {
            g.put(bx + 3, by + 2, "Nada espera.", Style::fg(DIM).on(PANEL));
        }
        for (i, (_, name, a)) in items.iter().enumerate() {
            let row = by + 2 + i;
            let bg = if i == self.avisos_selected {
                SEL
            } else {
                PANEL
            };
            g.paint_bg(bx + 1, row, bw - 2, bg);
            let (glyph, colour) = mark(a);
            g.put_over(bx + 3, row, glyph, Style::fg(colour).bold());
            g.put_over(
                bx + 6,
                row,
                &clip(name, bw.saturating_sub(24)),
                Style::fg(FG).bold(),
            );
            let t = what(a);
            g.put_over(bx + bw - 3 - t.chars().count(), row, &t, Style::fg(tone(a)));
        }
    }

    #[cfg(test)]
    fn plain(&self, cols: usize, rows: usize) -> String {
        self.grid(cols, rows).rows_plain().join("\n")
    }
}

fn base(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

/// Leave the alternate screen and raw mode even when drawing panics.
struct Terminal;

impl Terminal {
    fn enter() -> std::io::Result<Terminal> {
        terminal::enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(Terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(stdout(), cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn key_of(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char('j') | KeyCode::Down => Key::Down,
        KeyCode::Char('k') | KeyCode::Up => Key::Up,
        KeyCode::Char('h') | KeyCode::Left => Key::Left,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Char('a') => Key::Avisos,
        KeyCode::Char('n') => Key::NewPane,
        KeyCode::Esc => Key::Back,
        KeyCode::Char('q') => Key::Quit,
        _ => return None,
    })
}

fn connect(view: &mut View) -> Result<Client, String> {
    let socket = identity::socket_path();
    let mut client = Client::connect(&socket, "jwn-tui")
        .map_err(|e| format!("cannot reach {}: {e}", socket.display()))?;
    let root = client.root.clone();
    client.open(&root).map_err(|e| e.to_string())?;
    view.set_epoch(&client.epoch);
    view.set_status("en vivo");
    Ok(client)
}

pub fn run() -> Result<(), String> {
    let mut view = View::new("conectando");
    let mut client = connect(&mut view)?;
    // `JWN_TUI_DUMP`: also write each frame, without colours, to this file (for tests).
    let dump = std::env::var_os(identity::env_var("TUI_DUMP"));
    let _term = Terminal::enter().map_err(|e| e.to_string())?;
    let mut last: Vec<String> = Vec::new();
    loop {
        view.sync(client.nodes.values());
        for id in view.wants() {
            let _ = client.open(&id);
        }
        let (cols, rows) = terminal::size().map_err(|e| e.to_string())?;
        let grid = view.grid(cols as usize, rows as usize);
        let frame = grid.rows_ansi();
        if frame != last {
            let mut out = stdout();
            for (i, line) in frame.iter().enumerate() {
                let _ = execute!(out, cursor::MoveTo(0, i as u16));
                let _ = write!(out, "{line}");
            }
            let _ = out.flush();
            if let Some(path) = &dump {
                let _ = std::fs::write(path, grid.rows_plain().join("\n"));
            }
            last = frame;
        }
        // A lost daemon is waited for, not fatal: the screen stays and says so.
        if client.poll(Duration::from_millis(40)).is_err() {
            view.set_status("sin conexión, reintentando");
            loop {
                if let Ok(c) = connect(&mut view) {
                    client = c;
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                if event::poll(Duration::ZERO).map_err(|e| e.to_string())?
                    && matches!(
                        event::read().map_err(|e| e.to_string())?,
                        Event::Key(KeyEvent {
                            code: KeyCode::Char('q'),
                            ..
                        })
                    )
                {
                    return Ok(());
                }
            }
        }
        for (node, rev) in view.take_outbox() {
            let _ = client.cmd(Cmd::MarkSeen { node, rev });
        }
        while event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
            let Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read().map_err(|e| e.to_string())?
            else {
                continue;
            };
            let Some(key) = key_of(code) else { continue };
            match view.press(key) {
                After::Quit => return Ok(()),
                After::NewPane(workspace) => {
                    let _ = client.cmd(Cmd::NewPane {
                        workspace,
                        program: None,
                    });
                }
                After::Continue => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A view of the shared fixtures: the same documents the phone draws.
    fn fixture_view() -> View {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let docs: Vec<serde_json::Value> = [
            "snapshot-session-attention.json",
            "snapshot-workspace.json",
            "snapshot-workspace-notes.json",
            "snapshot-pane.json",
            "snapshot-pane-prompt.json",
            "snapshot-pane-agent.json",
        ]
        .iter()
        .map(|n| {
            serde_json::from_str::<serde_json::Value>(
                &std::fs::read_to_string(dir.join(n)).unwrap(),
            )
            .unwrap()["node"]
                .clone()
        })
        .collect();
        let mut v = View::new("fixtures");
        v.sync(docs.iter());
        v
    }

    #[test]
    fn the_sidebar_numbers_workspaces_and_puts_the_most_urgent_mark_at_the_right() {
        let t = fixture_view().plain(110, 30);
        let first = t.lines().nth(2).unwrap();
        assert!(
            first.starts_with("1 ● ↳ login") || first.starts_with("1 ● ↳ feat/login"),
            "{first}"
        );
        assert!(
            first[..28].contains('!'),
            "permission is the most urgent: {first}"
        );
        assert!(t.lines().nth(3).unwrap().starts_with("2 ● notes"), "{t}");
        assert!(t.contains("^␣ s sessions"), "{t}");
    }

    #[test]
    fn panes_are_rounded_boxes_with_their_attention_in_the_border() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        let t = v.plain(120, 30);
        assert!(t.contains("╭─ zsh"), "{t}");
        assert!(t.contains("╭─ zsh") && t.contains("╰"), "{t}");
        assert!(
            t.contains("$ cargo test"),
            "the first pane's screen is in its box: {t}"
        );
        assert!(!t.contains("└ "), "no tree rows, no tabs: {t}");
    }

    #[test]
    fn two_panes_sit_side_by_side() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        let t = v.plain(120, 30);
        let top = t.lines().nth(1).unwrap();
        assert_eq!(top.matches('╭').count(), 2, "{top}");
    }

    #[test]
    fn the_header_names_the_project_and_branch() {
        let mut v = fixture_view();
        v.workspaces
            .get_mut("00000000-0000-4000-8000-0000000000b1")
            .unwrap()
            .project = Some("/home/u/code/app".into());
        let t = v.plain(120, 30);
        assert!(t.lines().next().unwrap().contains("app/"), "{t}");
        assert!(t.lines().next().unwrap().contains("⎇ feat/login"), "{t}");
    }

    #[test]
    fn enter_opens_marks_seen_and_the_status_bar_shows_the_mode() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        assert!(v.plain(120, 30).lines().last().unwrap().contains("MOVE"));
        v.press(Key::Enter);
        assert!(v.avisos().len() < before, "opening cleared something");
        assert!(v.plain(120, 30).lines().last().unwrap().contains("TERM"));
    }

    #[test]
    fn moving_the_highlight_only_previews() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        v.press(Key::Down);
        assert_eq!(v.avisos().len(), before, "a preview marks nothing seen");
        assert!(v.plain(120, 30).lines().last().unwrap().contains("MOVE"));
    }

    #[test]
    fn right_focuses_the_next_pane_and_marks_it_seen() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        let focused = v.focus;
        v.press(Key::Right);
        assert_eq!(v.focus, focused + 1);
    }

    #[test]
    fn avisos_is_a_box_over_the_panes_and_enter_goes_to_the_named_pane() {
        let mut v = fixture_view();
        v.press(Key::Avisos);
        let listed = v.plain(120, 30);
        assert!(listed.contains("╭─ avisos"), "{listed}");
        assert!(listed.contains("pide permiso"), "{listed}");
        assert!(listed.lines().last().unwrap().contains("AVISOS"));
        let waiting = v.avisos().len();
        v.press(Key::Enter);
        assert_eq!(v.mode, Mode::Workspaces);
        assert!(
            v.avisos().len() < waiting,
            "going there cleared the attention"
        );
        assert!(v.opened.is_some());
    }

    #[test]
    fn a_new_daemon_epoch_forgets_what_was_seen() {
        let mut v = fixture_view();
        v.set_epoch("epoch-one");
        v.press(Key::Enter);
        let after_open = v.avisos().len();
        v.set_epoch("epoch-one");
        assert_eq!(
            v.avisos().len(),
            after_open,
            "the same epoch keeps the record"
        );
        v.set_epoch("epoch-two");
        assert!(
            v.avisos().len() > after_open,
            "a restarted daemon's revisions start over"
        );
        assert!(v.opened.is_none());
    }

    #[test]
    fn looking_is_told_to_the_daemon_so_every_client_clears_it() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        let told = v.take_outbox();
        assert!(!told.is_empty(), "opening a workspace queues what was seen");
        assert!(told.iter().all(|(_, rev)| *rev > 0));
        assert!(v.take_outbox().is_empty(), "each look is told once");
    }

    #[test]
    fn n_asks_for_a_pane_in_the_highlighted_workspace() {
        let mut v = fixture_view();
        assert!(matches!(v.press(Key::NewPane), After::NewPane(_)));
    }

    #[test]
    fn back_leaves_avisos_and_then_quits() {
        let mut v = fixture_view();
        v.press(Key::Avisos);
        assert_eq!(v.press(Key::Back), After::Continue);
        assert_eq!(v.press(Key::Back), After::Quit);
    }

    #[test]
    fn a_pane_not_opened_yet_is_asked_for() {
        let mut v = fixture_view();
        v.panes.clear();
        v.revs
            .retain(|k, _| !k.ends_with("c1") && !k.ends_with("c2"));
        assert!(!v.wants().is_empty());
    }

    #[test]
    fn frames_fit_the_terminal_at_two_sizes() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        for (c, r) in [(80, 24), (120, 32)] {
            let rows = v.grid(c, r).rows_plain();
            assert_eq!(rows.len(), r);
            assert!(rows.iter().all(|l| l.chars().count() <= c));
        }
    }
}
