//! `jwn tui`: the terminal client, laid out like the jw TUI it replaces (prototype:
//! `docs/prototypes/tui.html`). A sidebar of workspaces with the marks of what waits, a row
//! of facts on top, the open workspace's panes as rounded boxes, a status bar with the mode.
//! `a` lists what waits. It draws the same documents the phone draws and keeps its own seen
//! record. `View` is pure (documents in, a character grid out) so it is tested without a
//! terminal; `run` only moves bytes between the socket, the keyboard and the screen.

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::time::Duration;

use contract::{
    Attention, AttentionKind, Blocks, BlocksMode, Cmd, Node, Pane, Session, Workspace,
    WorkspaceKind,
};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use wire::Client;

mod changes;
mod diff_lines;
mod shell;

pub use changes::Ask;
use changes::{Changes, Docs, Outcome, Src};
use contract::ErrorBody;
use wire::ClientError;

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
/// The diff: a line added or deleted, and the words inside it that changed.
const ADD_BG: Rgb = (0x2a, 0x3b, 0x2c);
const DEL_BG: Rgb = (0x40, 0x29, 0x2d);
const ADD_WORD: Rgb = (0x3f, 0x5a, 0x42);
const DEL_WORD: Rgb = (0x6a, 0x3a, 0x40);

const SIDEBAR: usize = 28;

#[derive(Clone, Copy, PartialEq, Default, Debug)]
struct Style {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    italic: bool,
    underline: bool,
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
                        bg: style.bg.or(under.bg),
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
                        if st.italic {
                            out.push_str("\x1b[3m");
                        }
                        if st.underline {
                            out.push_str("\x1b[4m");
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
    /// Open the diff of the highlighted workspace.
    Changes,
    /// With a block selected: copy its command, copy its output, run it again.
    CopyCmd,
    CopyOut,
    Repeat,
    Back,
    Quit,
}

#[derive(PartialEq, Eq, Debug)]
pub enum After {
    Continue,
    Quit,
    /// Ask the daemon for a new pane in this workspace.
    NewPane(String),
    /// Bytes for a pane's shell: what was typed.
    Write {
        pane: String,
        data: String,
    },
    /// Text for the clipboard (OSC 52).
    Copy(String),
}

fn rev_of(n: &Node) -> u64 {
    match n {
        Node::Session(s) => s.head.rev,
        Node::Workspace(w) => w.head.rev,
        Node::Pane(p) => p.head.rev,
        Node::Blocks(b) => b.head.rev,
        Node::Diff(d) => d.head.rev,
        Node::Commits(c) => c.head.rev,
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
    /// The keyboard goes to the focused pane's shell; the leader (`^␣`) gives it back.
    typing: bool,
    /// The size last asked of the focused pane, so it is asked once.
    last_size: Option<(String, u16, u16)>,
    /// The blocks nodes, by node id.
    blocks: HashMap<String, Blocks>,
    /// The selected block: its pane and its id.
    blk: Option<(String, u64)>,
    /// How many lines each pane's blocks are scrolled up from the newest.
    scroll: HashMap<String, usize>,
    /// The clock the running blocks count from, in ms since the epoch.
    now_ms: u64,
    /// The diffs and lists of commits the daemon sent, by node id.
    diffs: HashMap<String, contract::Diff>,
    commit_lists: HashMap<String, contract::Commits>,
    /// The diff viewer, while it is open.
    changes: Option<Changes>,
    /// What the viewer asked for as it closed: to stop watching its nodes.
    asks: Vec<Ask>,
    /// The terminal's size as the loop last said it.
    size: (usize, usize),
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
            typing: false,
            last_size: None,
            blocks: HashMap::new(),
            blk: None,
            scroll: HashMap::new(),
            now_ms: 0,
            diffs: HashMap::new(),
            commit_lists: HashMap::new(),
            changes: None,
            asks: Vec::new(),
            size: (0, 0),
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

    pub fn set_now(&mut self, ms: u64) {
        self.now_ms = ms;
    }

    pub fn set_size(&mut self, cols: usize, rows: usize) {
        self.size = (cols, rows);
    }

    fn docs(&self) -> Docs<'_> {
        Docs {
            diffs: &self.diffs,
            commits: &self.commit_lists,
        }
    }

    /// The viewer follows the documents and the terminal.
    fn update_changes(&mut self) {
        if let Some(c) = self.changes.as_mut() {
            let docs = Docs {
                diffs: &self.diffs,
                commits: &self.commit_lists,
            };
            c.update(&docs, self.size, self.now_ms);
        }
    }

    fn close_changes(&mut self) {
        if let Some(c) = self.changes.take() {
            self.asks.extend(c.close());
        }
    }

    /// What the diff viewer wants the daemon to do, for the loop to send.
    pub fn take_asks(&mut self) -> Vec<Ask> {
        let mut asks = std::mem::take(&mut self.asks);
        if let Some(c) = self.changes.as_mut() {
            asks.extend(c.take_asks());
        }
        asks
    }

    /// The daemon made the node the viewer asked for.
    pub fn changes_made(&mut self, src: Src, node: &str) {
        if let Some(c) = self.changes.as_mut() {
            c.made(src, node);
        }
    }

    pub fn changes_failed(&mut self, src: Src, why: &str) {
        if let Some(c) = self.changes.as_mut() {
            c.failed(src, why);
        }
    }

    pub fn changes_flash(&mut self, text: &str) {
        if let Some(c) = self.changes.as_mut() {
            c.flash(text);
        }
    }

    /// A key while the diff is open: it is the viewer's, never a pane's.
    fn changes_key(&mut self, ev: KeyEvent) {
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        let Some(c) = self.changes.as_mut() else {
            return;
        };
        let docs = Docs {
            diffs: &self.diffs,
            commits: &self.commit_lists,
        };
        if c.press(ev.code, &docs, self.size) == Outcome::Close {
            self.close_changes();
        } else {
            self.update_changes();
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
        self.blocks.clear();
        self.diffs.clear();
        self.commit_lists.clear();
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
                Node::Blocks(b) => {
                    self.blocks.insert(id, b);
                }
                Node::Diff(d) => {
                    self.diffs.insert(id, d);
                }
                Node::Commits(c) => {
                    self.commit_lists.insert(id, c);
                }
            }
        }
        self.update_changes();
        let gone = self.blk.as_ref().is_some_and(|(pane, id)| {
            !self
                .blocks_of(pane)
                .is_some_and(|b| b.blocks.iter().any(|k| k.id == *id))
        });
        if gone {
            self.blk = None;
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
        ids.extend(
            self.shown()
                .iter()
                .flat_map(|w| w.panes.iter())
                .filter_map(|p| self.panes.get(&p.id)?.blocks.clone()),
        );
        ids.retain(|id| !self.revs.contains_key(id));
        ids
    }

    /// The pane's blocks, when it is drawn as blocks (a program with the whole screen, or a
    /// shell without the integration, is drawn as its screen).
    fn blocks_of(&self, pane: &str) -> Option<&Blocks> {
        let id = self.panes.get(pane)?.blocks.as_ref()?;
        self.blocks.get(id).filter(|b| b.mode == BlocksMode::Blocks)
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
        self.typing = true;
        self.blk = None;
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

    /// The pane the keyboard goes to: the focused pane of the opened workspace, while typing.
    fn typing_pane(&self) -> Option<String> {
        if !self.typing || self.mode != Mode::Workspaces || self.changes.is_some() {
            return None;
        }
        let ws = self.workspaces.get(self.opened.as_deref()?)?;
        Some(ws.panes.get(self.focus)?.id.clone())
    }

    /// A key at the keyboard: typed into the pane, or (after the leader, or when no pane is
    /// open) a command of the client.
    pub fn press_key(&mut self, ev: KeyEvent) -> After {
        if self.changes.is_some() {
            self.changes_key(ev);
            return After::Continue;
        }
        let Some(pane) = self.typing_pane() else {
            return key_of(ev.code).map_or(After::Continue, |k| self.press(k));
        };
        if is_leader(&ev) {
            self.typing = false;
            return After::Continue;
        }
        match bytes_of(&ev) {
            Some(data) => After::Write { pane, data },
            None => After::Continue,
        }
    }

    /// The size the typing pane's box has, when it differs from what was last asked: a shell
    /// should wrap where its box does.
    pub fn take_resize(&mut self, cols: usize, rows: usize) -> Option<(String, u16, u16)> {
        let pane = self.typing_pane()?;
        let n = self.workspaces.get(self.opened.as_deref()?)?.panes.len();
        let (_, _, w, h) = *pane_rects(n, panes_rect(cols, rows)).get(self.focus)?;
        let want = (
            pane,
            h.saturating_sub(2).max(1) as u16,
            w.saturating_sub(4).max(1) as u16,
        );
        (self.last_size.as_ref() != Some(&want)).then(|| {
            self.last_size = Some(want.clone());
            want
        })
    }

    pub fn press(&mut self, key: Key) -> After {
        let n = self.session.as_ref().map_or(0, |s| s.workspaces.len());
        if self.blk.is_some() && self.mode == Mode::Workspaces {
            match key {
                Key::Up | Key::Down => {
                    self.step_block(key == Key::Down);
                    return After::Continue;
                }
                Key::Back => {
                    self.blk = None;
                    return After::Continue;
                }
                Key::CopyCmd | Key::CopyOut | Key::Repeat => return self.block_action(key),
                _ => {}
            }
        }
        match (self.mode, key) {
            (_, Key::Quit) | (Mode::Workspaces, Key::Back) => return After::Quit,
            (Mode::Avisos, Key::Back | Key::Avisos) => self.mode = Mode::Workspaces,
            (Mode::Workspaces, Key::Avisos) => {
                self.mode = Mode::Avisos;
                self.avisos_selected = 0;
            }
            (Mode::Workspaces, Key::Down) => {
                self.typing = false;
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
                self.opened = None;
                self.focus = 0;
            }
            (Mode::Workspaces, Key::Up) => {
                self.typing = false;
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
            (Mode::Workspaces, Key::Changes) => {
                if let Some(id) = self.selected_id() {
                    self.typing = false;
                    self.blk = None;
                    self.changes = Some(Changes::open(&id));
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

    /// Move the selection to the next (or previous) block of its pane.
    fn step_block(&mut self, forward: bool) {
        let Some((pane, id)) = self.blk.clone() else {
            return;
        };
        let Some(b) = self.blocks_of(&pane) else {
            return;
        };
        let Some(at) = b.blocks.iter().position(|k| k.id == id) else {
            return;
        };
        let to = if forward {
            (at + 1).min(b.blocks.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        self.blk = Some((pane, b.blocks[to].id));
    }

    /// Copy the selected block's command or output, or run its command again.
    fn block_action(&mut self, key: Key) -> After {
        let Some((pane, id)) = self.blk.clone() else {
            return After::Continue;
        };
        let Some(b) = self
            .blocks_of(&pane)
            .and_then(|b| b.blocks.iter().find(|k| k.id == id))
        else {
            return After::Continue;
        };
        match key {
            Key::CopyCmd => After::Copy(b.cmd.clone()),
            Key::CopyOut => After::Copy(shell::output_text(b)),
            _ => {
                let data = format!("{}\r", b.cmd);
                self.blk = None;
                After::Write { pane, data }
            }
        }
    }

    /// A click inside a pane's box: a block selects, `■ ^C` stops the running command.
    fn click_pane(&mut self, pane: &str, rect: Rect, x: usize, y: usize) -> After {
        let Some(b) = self.blocks_of(pane) else {
            return After::Continue;
        };
        let (ix, iy, iw, ih) = inner(rect);
        let lh = list_h(ih);
        let lines = shell::block_lines(&b.blocks, self.now_ms, iw.saturating_sub(2));
        let top = iy + gap(lines.len(), lh);
        if (top..iy + lh).contains(&y) && (ix..ix + iw).contains(&x) {
            let start = window_start(lines.len(), lh, self.scroll.get(pane).copied().unwrap_or(0));
            if let Some(id) = lines.get(start + y - top).and_then(|l| l.block) {
                self.blk = Some((pane.to_owned(), id));
                self.typing = false;
            }
        } else if y == iy + lh + 1
            && b.blocks.last().is_some_and(|k| k.running)
            && x + STOP.chars().count() >= ix + iw
            && x < ix + iw
        {
            return After::Write {
                pane: pane.to_owned(),
                data: "\x03".into(),
            };
        }
        After::Continue
    }

    /// A left click at cell (`x`, `y`) of a `cols` × `rows` screen. A workspace row opens it,
    /// a pane's box focuses it, `avisos N` in the status bar opens the list, an item of the
    /// list goes there, and a click outside the list closes it.
    pub fn click(&mut self, x: usize, y: usize, cols: usize, rows: usize) -> After {
        if self.changes.is_some() {
            if x > SIDEBAR {
                if let Some(c) = self.changes.as_mut() {
                    let docs = Docs {
                        diffs: &self.diffs,
                        commits: &self.commit_lists,
                    };
                    c.click(x, y, &docs, (cols, rows));
                }
                self.update_changes();
                return After::Continue;
            }
            self.close_changes();
        }
        let waiting = self.avisos().len();
        if self.mode == Mode::Avisos {
            let rect = avisos_rect(waiting, panes_rect(cols, rows));
            if !inside(rect, x, y) {
                self.mode = Mode::Workspaces;
            } else if y >= rect.1 + 2
                && y < rect.1 + 2 + waiting
                && x > rect.0
                && x + 1 < rect.0 + rect.2
            {
                self.avisos_selected = y - rect.1 - 2;
                return self.press(Key::Enter);
            }
            return After::Continue;
        }
        let label = format!("avisos {waiting}");
        let start = cols.saturating_sub(label.chars().count() + 1);
        if waiting > 0 && y + 1 == rows && x >= start && x < start + label.chars().count() {
            return self.press(Key::Avisos);
        }
        let workspaces = self.session.as_ref().map_or(0, |s| s.workspaces.len());
        if x + 1 < SIDEBAR && y >= 2 && y < rows.saturating_sub(5) && y - 2 < workspaces {
            self.selected = y - 2;
            return self.press(Key::Enter);
        }
        if let Some(doc) = self.shown() {
            let id = doc.head.id.clone();
            let rects = pane_rects(doc.panes.len(), panes_rect(cols, rows));
            if let Some(i) = rects.iter().position(|r| inside(*r, x, y)) {
                let pane = doc.panes[i].id.clone();
                self.open(&id, i);
                return self.click_pane(&pane, rects[i], x, y);
            }
        }
        After::Continue
    }

    /// The wheel over the sidebar (or the avisos list) moves the highlight; over a pane drawn
    /// as blocks it scrolls them.
    pub fn scroll(&mut self, down: bool, x: usize, y: usize, cols: usize, rows: usize) -> After {
        if self.changes.is_some() {
            if x >= SIDEBAR {
                if let Some(c) = self.changes.as_mut() {
                    c.wheel(down, x, (cols, rows));
                }
                return After::Continue;
            }
            self.close_changes();
        }
        if self.mode == Mode::Avisos || x < SIDEBAR {
            return self.press(if down { Key::Down } else { Key::Up });
        }
        let Some(doc) = self.shown() else {
            return After::Continue;
        };
        let rects = pane_rects(doc.panes.len(), panes_rect(cols, rows));
        let Some(i) = rects.iter().position(|r| inside(*r, x, y)) else {
            return After::Continue;
        };
        let pane = doc.panes[i].id.clone();
        let Some(b) = self.blocks_of(&pane) else {
            return After::Continue;
        };
        let (_, _, iw, ih) = inner(rects[i]);
        let total = shell::block_lines(&b.blocks, self.now_ms, iw.saturating_sub(2)).len();
        let most = total.saturating_sub(list_h(ih));
        let at = self.scroll.entry(pane).or_insert(0);
        *at = if down {
            at.saturating_sub(SCROLL_STEP)
        } else {
            (*at + SCROLL_STEP).min(most)
        };
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
            self.panes_area(
                &mut g,
                doc,
                ws.branch.as_deref(),
                (px, 1, pw, rows.saturating_sub(2)),
            );
        }

        if let Some(c) = &self.changes {
            c.draw(
                &mut g,
                &self.docs(),
                &self.label(c.workspace()),
                self.now_ms,
            );
        }

        // The status bar.
        for x in 0..cols {
            g.cells[(rows - 1) * cols + x] = (' ', Style::default().on(PANEL));
        }
        let typing = self.typing_pane().is_some();
        let diff_status = self.changes.as_ref().map(|c| c.status(&self.docs()));
        let (chip, bg, hint) = match &diff_status {
            Some((hint, _)) => (" DIFF ", CYAN, hint.as_str()),
            None => match (self.mode, typing) {
                (Mode::Avisos, _) => (" AVISOS ", YELLOW, "j/k mover · ↵ ir · esc volver"),
                (_, false) if self.blk.is_some() => (
                    " BLOQUE ",
                    MAGENTA,
                    "c comando · o salida · r repetir · j/k mover · esc soltar",
                ),
                (_, true) => (" TERM ", GREEN, "escribes en el panel · ^␣ para moverte"),
                (_, false) => (
                    " MOVE ",
                    BLUE,
                    "j/k workspace · ↵ escribir · h/l panel · n panel nuevo · d cambios · a avisos · q salir",
                ),
            },
        };
        g.put(
            0,
            rows - 1,
            chip,
            Style {
                fg: Some(PANEL),
                bg: Some(bg),
                bold: true,
                ..Style::default()
            },
        );
        g.put(
            chip.chars().count() + 1,
            rows - 1,
            &clip(hint, cols.saturating_sub(chip.chars().count() + 14)),
            dim.on(PANEL),
        );
        let waiting = self.avisos().len();
        let mut right = cols;
        if waiting > 0 {
            let t = format!("avisos {waiting}");
            right = cols.saturating_sub(t.chars().count() + 1);
            g.put(right, rows - 1, &t, Style::fg(YELLOW).bold().on(PANEL));
        }
        if let Some((_, Some((seen, total)))) = &diff_status {
            let t = format!("{seen}/{total} vistos");
            let tone = if seen == total { GREEN } else { DIM };
            g.put(
                right.saturating_sub(t.chars().count() + 2),
                rows - 1,
                &t,
                Style::fg(tone).on(PANEL),
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
        branch: Option<&str>,
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
        let rects = pane_rects(n, (x, y, w, h));
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
            if let Some(b) = self.blocks_of(&summary.id) {
                self.blocks_view(g, b, p, rect, focused, branch);
                continue;
            }
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

    /// A shell pane as blocks: the list, newest at the bottom, and the editor under it (a rule,
    /// the chips, and the line the shell is editing, which is the cursor's line of its screen).
    fn blocks_view(
        &self,
        g: &mut Grid,
        b: &Blocks,
        p: &Pane,
        rect: Rect,
        focused: bool,
        branch: Option<&str>,
    ) {
        let (ix, iy, iw, ih) = inner(rect);
        if iw < 8 || ih < 4 {
            return;
        }
        let lh = list_h(ih);
        let lines = shell::block_lines(&b.blocks, self.now_ms, iw - 2);
        let scrolled = self.scroll.get(&p.head.id).copied().unwrap_or(0);
        let start = window_start(lines.len(), lh, scrolled);
        let picked = self
            .blk
            .as_ref()
            .filter(|(pane, _)| *pane == p.head.id)
            .map(|(_, id)| *id);
        let top = iy + gap(lines.len(), lh);
        for (r, line) in lines[start..].iter().take(lh).enumerate() {
            let y = top + r;
            if picked.is_some() && line.block == picked {
                g.paint_bg(ix, y, iw, SEL);
            }
            if let Some(bar) = line.bar {
                g.put_over(ix, y, "▌", Style::fg(bar));
            }
            let mut x = ix + 2;
            for (text, style) in &line.spans {
                let text = clip(text, (ix + iw).saturating_sub(x));
                g.put_over(x, y, &text, *style);
                x += text.chars().count();
            }
            if let Some((text, style)) = &line.right {
                let at = (ix + iw).saturating_sub(text.chars().count()).max(ix + 2);
                g.put_over(at, y, text, *style);
            }
        }
        let ey = iy + lh;
        g.put(ix, ey, &"─".repeat(iw), Style::fg(LINE));
        let last = b.blocks.last();
        if let Some(k) = last.filter(|k| k.running) {
            g.put(
                ix,
                ey + 1,
                &format!(
                    "◌ {}",
                    clip(&k.cmd, iw.saturating_sub(STOP.chars().count() + 4))
                ),
                Style::fg(YELLOW),
            );
            g.put(
                ix + iw - STOP.chars().count(),
                ey + 1,
                STOP,
                Style::fg(RED).bold(),
            );
        } else {
            let folder = shell::short(&p.cwd);
            g.put(ix, ey + 1, &clip(&folder, iw), Style::fg(BLUE));
            if let Some(branch) = branch {
                let at = ix + folder.chars().count() + 2;
                g.put(
                    at,
                    ey + 1,
                    &clip(&format!("⎇ {branch}"), (ix + iw).saturating_sub(at)),
                    Style::fg(MAGENTA),
                );
            }
        }
        let cur = p.screen.cursor;
        if let Some(line) = p.screen.lines.get(cur.row as usize) {
            g.put(ix, ey + 2, &clip(line, iw), Style::fg(FG));
        }
        if focused && cur.visible && (cur.col as usize) < iw {
            g.cells[(ey + 2) * g.cols + ix + cur.col as usize].1.reverse = true;
        }
    }

    fn avisos_box(&self, g: &mut Grid, (x, y, w, h): Rect) {
        let items = self.avisos();
        let (bx, by, bw, bh) = avisos_rect(items.len(), (x, y, w, h));
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

type Rect = (usize, usize, usize, usize);

/// The stop chip of a running command, at the right of the editor's chips row.
const STOP: &str = "■ ^C";
/// Lines one notch of the wheel moves a pane's blocks.
const SCROLL_STEP: usize = 3;
/// The editor under the blocks: a rule, the chips, the prompt line.
const EDITOR_ROWS: usize = 3;

/// What a pane's box holds: inside the border and its padding.
fn inner((x, y, w, h): Rect) -> Rect {
    (x + 2, y + 1, w.saturating_sub(4), h.saturating_sub(2))
}

/// Rows of the list of blocks in a box `h` rows tall.
fn list_h(h: usize) -> usize {
    h.saturating_sub(EDITOR_ROWS)
}

/// Empty rows above a list of `total` lines in `rows`: the list sits at the bottom, next to
/// the editor, like on the phone.
fn gap(total: usize, rows: usize) -> usize {
    rows.saturating_sub(total)
}

/// The first line of `total` shown in `rows`, with the newest at the bottom and the list
/// scrolled `up` lines from it.
fn window_start(total: usize, rows: usize, up: usize) -> usize {
    let most = total.saturating_sub(rows);
    most - up.min(most)
}

fn inside((x, y, w, h): Rect, px: usize, py: usize) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// The area the panes share: right of the sidebar, between the header and the status bar.
fn panes_rect(cols: usize, rows: usize) -> Rect {
    let x = SIDEBAR + 1;
    (x, 1, cols.saturating_sub(x), rows.saturating_sub(2))
}

/// One pane fills the area; two sit side by side; from three, the first takes the left half
/// and the rest stack on the right.
fn pane_rects(n: usize, (x, y, w, h): Rect) -> Vec<Rect> {
    let half = w / 2;
    match n {
        0 => vec![],
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
    }
}

/// Where the avisos box sits over the panes' area, for `items` rows.
fn avisos_rect(items: usize, (x, y, w, _): Rect) -> Rect {
    let bw = 56.min(w.saturating_sub(2));
    (x + w.saturating_sub(bw) / 2, y + 3, bw, items.max(1) + 4)
}

fn base(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

/// Leave the alternate screen and raw mode even when drawing panics.
struct Terminal;

impl Terminal {
    fn enter() -> std::io::Result<Terminal> {
        terminal::enable_raw_mode()?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            cursor::Hide,
            EnableMouseCapture
        )?;
        Ok(Terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            cursor::Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

/// `^␣` (Ctrl-Space), the key that gives the keyboard back to the client.
fn is_leader(ev: &KeyEvent) -> bool {
    ev.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(ev.code, KeyCode::Char(' ') | KeyCode::Char('@'))
}

/// What a key sends to a shell: the bytes a terminal would.
fn bytes_of(ev: &KeyEvent) -> Option<String> {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let body = match ev.code {
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            ((c.to_ascii_lowercase() as u8 - b'a' + 1) as char).to_string()
        }
        KeyCode::Char(c) if ctrl && matches!(c, '[' | '\\' | ']' | '^' | '_') => {
            ((c as u8 & 0x1f) as char).to_string()
        }
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Up => "\x1b[A".into(),
        KeyCode::Down => "\x1b[B".into(),
        KeyCode::Right => "\x1b[C".into(),
        KeyCode::Left => "\x1b[D".into(),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        _ => return None,
    };
    Some(if alt { format!("\x1b{body}") } else { body })
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
        KeyCode::Char('d') => Key::Changes,
        KeyCode::Char('c') => Key::CopyCmd,
        KeyCode::Char('o') => Key::CopyOut,
        KeyCode::Char('r') => Key::Repeat,
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

/// How long the diff viewer waits for the daemon to read git for it.
const ASK_WAIT: Duration = Duration::from_secs(20);

type Reply = Result<Result<Option<serde_json::Value>, ErrorBody>, ClientError>;

/// Tell the viewer how the daemon answered a request for a node.
fn answer(view: &mut View, src: Src, reply: Reply) {
    match reply {
        Ok(Ok(result)) => match result.as_ref().and_then(|r| r["node"].as_str()) {
            Some(node) => view.changes_made(src, node),
            None => view.changes_failed(src, "el daemon no dio el nodo"),
        },
        Ok(Err(e)) => view.changes_failed(src, &e.message),
        Err(e) => view.changes_failed(src, &e.to_string()),
    }
}

/// Send what the diff viewer asked for. A node it asked to be made is opened as soon as the
/// reply names it.
fn serve_asks(view: &mut View, client: &mut Client) {
    loop {
        let asks = view.take_asks();
        if asks.is_empty() {
            return;
        }
        for ask in asks {
            match ask {
                Ask::OpenDiff {
                    src,
                    workspace,
                    scope,
                    commit,
                } => {
                    let cmd = Cmd::OpenDiff {
                        workspace,
                        scope,
                        commit,
                    };
                    answer(view, src, client.cmd_within(cmd, ASK_WAIT));
                }
                Ask::OpenCommits { workspace } => {
                    let cmd = Cmd::OpenCommits { workspace };
                    answer(view, Src::Commits, client.cmd_within(cmd, ASK_WAIT));
                }
                Ask::Watch(node) => {
                    let _ = client.open(&node);
                }
                Ask::Refresh(node) => {
                    if let Ok(Err(e)) = client.cmd_within(Cmd::RefreshDiff { node }, ASK_WAIT) {
                        view.changes_flash(&e.message);
                    }
                }
                Ask::SetViewed { node, path, viewed } => {
                    let _ = client.send(Cmd::SetViewed { node, path, viewed });
                }
                Ask::Close(node) => {
                    let _ = client.close(&node);
                }
            }
        }
    }
}

pub fn run() -> Result<(), String> {
    let mut view = View::new("conectando");
    let mut client = connect(&mut view)?;
    // `JWN_TUI_DUMP`: also write each frame, without colours, to this file (for tests).
    let dump = std::env::var_os(identity::env_var("TUI_DUMP"));
    let _term = Terminal::enter().map_err(|e| e.to_string())?;
    let mut last: Vec<String> = Vec::new();
    loop {
        view.set_now(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
        );
        let (cols, rows) = terminal::size().map_err(|e| e.to_string())?;
        view.set_size(cols as usize, rows as usize);
        view.sync(client.nodes.values());
        for id in view.wants() {
            let _ = client.open(&id);
        }
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
        if let Some((pane, rows, cols_)) = view.take_resize(cols as usize, rows as usize) {
            let _ = client.send(Cmd::Resize {
                pane,
                rows,
                cols: cols_,
            });
        }
        for (node, rev) in view.take_outbox() {
            let _ = client.cmd(Cmd::MarkSeen { node, rev });
        }
        serve_asks(&mut view, &mut client);
        while event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
            let after = match event::read().map_err(|e| e.to_string())? {
                Event::Key(ev) if ev.kind == KeyEventKind::Press => view.press_key(ev),
                Event::Mouse(m) => {
                    let (cols, rows) = (cols as usize, rows as usize);
                    let (x, y) = (m.column as usize, m.row as usize);
                    match m.kind {
                        MouseEventKind::Down(MouseButton::Left) => view.click(x, y, cols, rows),
                        MouseEventKind::ScrollUp => view.scroll(false, x, y, cols, rows),
                        MouseEventKind::ScrollDown => view.scroll(true, x, y, cols, rows),
                        _ => After::Continue,
                    }
                }
                _ => After::Continue,
            };
            match after {
                After::Quit => return Ok(()),
                After::Write { pane, data } => {
                    let _ = client.send(Cmd::Write { pane, data });
                }
                After::NewPane(workspace) => {
                    let _ = client.cmd(Cmd::NewPane {
                        workspace,
                        program: None,
                    });
                }
                After::Copy(text) => {
                    let mut out = stdout();
                    let _ = write!(out, "{}", shell::osc52(&text));
                    let _ = out.flush();
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
            "snapshot-blocks.json",
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

    const BLOCKS: &str = "00000000-0000-4000-8000-0000000000d1";
    const SHELL: &str = "00000000-0000-4000-8000-0000000000c2";

    fn open_blocks() -> View {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v
    }

    #[test]
    fn a_shell_pane_is_blocks_with_an_editor_under_them() {
        let t = open_blocks().plain(120, 30);
        assert!(t.contains("▌ ❯ git status"), "{t}");
        assert!(t.contains("salió 101 · 4.2s"), "{t}");
        assert!(t.contains("corriendo"), "{t}");
        assert!(
            t.contains("── ◌ ") || t.contains("◌ cargo watch -x test"),
            "{t}"
        );
        assert!(
            t.contains("■ ^C"),
            "the running command can be stopped: {t}"
        );
        assert!(
            t.contains("$ cargo test"),
            "a pane with no blocks stays a screen: {t}"
        );
    }

    #[test]
    fn a_short_list_of_blocks_sits_at_the_bottom_next_to_the_editor() {
        let t = open_blocks().plain(120, 30);
        let rows: Vec<&str> = t.lines().collect();
        let cmd = rows
            .iter()
            .position(|l| l.contains("❯ cargo watch"))
            .unwrap();
        let rule = cmd
            + rows[cmd..]
                .iter()
                .position(|l| l.contains("────────────"))
                .unwrap();
        assert_eq!(
            rule - cmd,
            3,
            "the newest block ends right above the editor: {t}"
        );
    }

    #[test]
    fn a_program_with_the_whole_screen_is_drawn_as_its_screen() {
        let mut v = open_blocks();
        v.blocks.get_mut(BLOCKS).unwrap().mode = BlocksMode::Full;
        let t = v.plain(120, 30);
        assert!(!t.contains("❯ git status") && !t.contains("■ ^C"), "{t}");
    }

    #[test]
    fn the_editor_shows_the_folder_and_branch_when_nothing_runs() {
        let mut v = open_blocks();
        v.blocks.get_mut(BLOCKS).unwrap().blocks.pop();
        let t = v.plain(120, 30);
        assert!(!t.contains("■ ^C"), "{t}");
        assert!(t.contains("⎇ feat/login"), "{t}");
    }

    #[test]
    fn a_click_selects_a_block_and_c_o_r_act_on_it() {
        let mut v = open_blocks();
        assert_eq!(v.click(80, 8, 120, 30), After::Continue);
        assert!(v.plain(120, 30).lines().last().unwrap().contains("BLOQUE"));
        assert_eq!(v.press(Key::CopyCmd), After::Copy("git status".into()));
        assert_eq!(
            v.press(Key::CopyOut),
            After::Copy("On branch feat/login\nnothing to commit, working tree clean".into())
        );
        assert_eq!(
            v.press(Key::Repeat),
            After::Write {
                pane: SHELL.into(),
                data: "git status\r".into()
            }
        );
        assert!(v.blk.is_none(), "running it again lets go");
    }

    #[test]
    fn with_a_block_selected_j_k_move_between_blocks_and_esc_lets_go() {
        let mut v = open_blocks();
        v.click(80, 8, 120, 30);
        v.press(Key::Down);
        assert_eq!(v.blk, Some((SHELL.into(), 2)));
        v.press(Key::Down);
        v.press(Key::Down);
        assert_eq!(v.blk, Some((SHELL.into(), 3)), "stops at the newest");
        v.press(Key::Up);
        assert_eq!(v.blk, Some((SHELL.into(), 2)));
        assert_eq!(v.press(Key::Back), After::Continue, "esc does not quit");
        assert!(v.blk.is_none());
        assert_eq!(v.press(Key::CopyCmd), After::Continue, "nothing to copy");
    }

    #[test]
    fn the_stop_chip_sends_ctrl_c() {
        let mut v = open_blocks();
        assert_eq!(
            v.click(116, 26, 120, 30),
            After::Write {
                pane: SHELL.into(),
                data: "\x03".into()
            }
        );
    }

    #[test]
    fn the_wheel_over_a_pane_scrolls_its_blocks() {
        let mut v = open_blocks();
        let before = v.plain(120, 16);
        assert!(!before.contains("❯ git status"), "{before}");
        for _ in 0..4 {
            v.scroll(false, 100, 5, 120, 16);
        }
        assert!(v.plain(120, 16).contains("❯ git status"));
        for _ in 0..8 {
            v.scroll(true, 100, 5, 120, 16);
        }
        assert_eq!(v.plain(120, 16), before, "back at the newest");
    }

    #[test]
    fn the_blocks_node_of_a_shown_pane_is_asked_for() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.revs.remove(BLOCKS);
        assert!(v.wants().contains(&BLOCKS.to_owned()), "{:?}", v.wants());
    }

    #[test]
    fn a_selection_whose_block_is_gone_is_dropped() {
        let mut v = open_blocks();
        v.click(80, 8, 120, 30);
        assert!(v.blk.is_some());
        v.blocks.get_mut(BLOCKS).unwrap().blocks.remove(0);
        let docs: Vec<serde_json::Value> = vec![];
        v.sync(docs.iter());
        assert!(
            v.blk.is_none(),
            "nothing left to act on, so keys are the client's again"
        );
    }

    #[test]
    fn the_window_keeps_the_newest_at_the_bottom() {
        assert_eq!(window_start(5, 10, 0), 0);
        assert_eq!(gap(5, 10), 5, "a short list sits at the bottom");
        assert_eq!(gap(30, 10), 0);
        assert_eq!(window_start(30, 10, 0), 20);
        assert_eq!(window_start(30, 10, 4), 16);
        assert_eq!(window_start(30, 10, 99), 0);
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
    fn d_opens_the_diff_over_the_panes_and_esc_closes_it_and_stops_watching() {
        let mut v = fixture_view();
        v.set_size(180, 44);
        assert!(!v.plain(180, 44).contains(" vs HEAD"));
        v.press(Key::Changes);
        let asks = v.take_asks();
        assert!(
            matches!(
                &asks[..],
                [Ask::OpenDiff {
                    src: Src::Uncommitted,
                    ..
                }]
            ),
            "{asks:?}"
        );
        let screen = v.plain(180, 44);
        assert!(screen.contains(" vs HEAD"), "{screen}");
        assert!(screen.lines().last().unwrap().contains("DIFF"));
        assert!(screen.contains("leyendo los cambios"));

        v.changes_made(Src::Uncommitted, "the-node");
        assert_eq!(v.take_asks(), [Ask::Watch("the-node".into())]);
        // Keys are the viewer's, never a pane's.
        assert_eq!(v.press_key(plain(KeyCode::Char('x'))), After::Continue);
        v.press_key(plain(KeyCode::Esc));
        assert_eq!(v.take_asks(), [Ask::Close("the-node".into())]);
        assert!(!v.plain(180, 44).contains(" vs HEAD"));
        assert!(v.plain(180, 44).lines().last().unwrap().contains("MOVE"));
    }

    #[test]
    fn clicking_the_sidebar_leaves_the_diff_and_a_click_in_it_picks_a_tab() {
        let mut v = fixture_view();
        v.set_size(180, 44);
        v.press(Key::Changes);
        v.take_asks();
        // The second tab starts after the first, which is ` 1 Sin commitear ` wide.
        v.click(SIDEBAR + 4 + 20, 2, 180, 44);
        let asks = v.take_asks();
        assert!(matches!(&asks[..], [Ask::OpenCommits { .. }]), "{asks:?}");
        v.click(3, 2, 180, 44);
        assert!(
            !v.plain(180, 44).contains(" vs HEAD"),
            "the sidebar closes it"
        );
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
    fn clicking_a_workspace_row_opens_it() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        // The second row of the sidebar is y = 3 (row 1 of the list is y = 2).
        v.click(5, 3, 120, 30);
        assert_eq!(v.selected, 1);
        assert!(v.opened.is_some(), "a click opens, like Enter");
        assert!(v.avisos().len() <= before);
        assert!(v.plain(120, 30).lines().last().unwrap().contains("TERM"));
    }

    #[test]
    fn clicking_a_pane_box_focuses_it() {
        let mut v = fixture_view();
        v.click(5, 2, 120, 30); // the first workspace, two panes side by side
        let panes_x = SIDEBAR + 1;
        let (w, h) = (120 - panes_x, 28);
        v.click(panes_x + w - 3, 1 + h / 2, 120, 30); // inside the right box
        assert_eq!(v.focus, 1);
        v.click(panes_x + 3, 1 + h / 2, 120, 30);
        assert_eq!(v.focus, 0);
    }

    #[test]
    fn clicking_avisos_in_the_status_bar_opens_the_list_and_an_item_goes_there() {
        let mut v = fixture_view();
        let waiting = v.avisos().len();
        let label = format!("avisos {waiting}");
        v.click(120 - label.len() - 1 + 2, 29, 120, 30);
        assert_eq!(v.mode, Mode::Avisos);
        let rect = avisos_rect(waiting, panes_rect(120, 30));
        v.click(rect.0 + 6, rect.1 + 2, 120, 30);
        assert_eq!(v.mode, Mode::Workspaces);
        assert!(v.opened.is_some());
        assert!(v.avisos().len() < waiting, "going there cleared it");
    }

    #[test]
    fn clicking_outside_the_avisos_list_closes_it() {
        let mut v = fixture_view();
        v.press(Key::Avisos);
        v.click(2, 20, 120, 30);
        assert_eq!(v.mode, Mode::Workspaces);
        assert!(v.opened.is_none(), "closing opens nothing");
    }

    #[test]
    fn the_wheel_over_the_sidebar_moves_the_highlight_and_elsewhere_does_nothing() {
        let mut v = fixture_view();
        v.scroll(true, 5, 3, 120, 30);
        assert_eq!(v.selected, 1);
        v.scroll(false, 5, 3, 120, 30);
        assert_eq!(v.selected, 0);
        v.scroll(true, 80, 3, 120, 30);
        assert_eq!(v.selected, 0, "the wheel over a pane is not ours");
    }

    #[test]
    fn clicks_on_empty_space_change_nothing() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        v.click(5, 25, 120, 30); // below the workspace rows
        v.click(0, 0, 120, 30); // the header
        assert_eq!(v.selected, 0);
        assert!(v.opened.is_none());
        assert_eq!(v.avisos().len(), before);
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::NONE)
    }

    #[test]
    fn once_a_pane_is_open_the_keyboard_goes_to_its_shell() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        let pane = v.typing_pane().expect("opening a workspace starts typing");
        assert_eq!(
            v.press_key(plain(KeyCode::Char('x'))),
            After::Write {
                pane: pane.clone(),
                data: "x".into()
            }
        );
        // The client's own keys are the shell's now, `j` and `q` included.
        assert_eq!(
            v.press_key(plain(KeyCode::Char('j'))),
            After::Write {
                pane: pane.clone(),
                data: "j".into()
            }
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('q'))),
            After::Write {
                pane: pane.clone(),
                data: "q".into()
            }
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Write {
                pane: pane.clone(),
                data: "\r".into()
            }
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Esc)),
            After::Write {
                pane,
                data: "\x1b".into()
            }
        );
        assert!(v.plain(120, 30).lines().last().unwrap().contains("TERM"));
    }

    #[test]
    fn keys_become_the_bytes_a_terminal_sends() {
        let b = |c, m| bytes_of(&key(c, m));
        assert_eq!(
            b(KeyCode::Char('c'), KeyModifiers::CONTROL).as_deref(),
            Some("\x03")
        );
        assert_eq!(
            b(KeyCode::Char('d'), KeyModifiers::CONTROL).as_deref(),
            Some("\x04")
        );
        assert_eq!(
            b(KeyCode::Char('b'), KeyModifiers::ALT).as_deref(),
            Some("\x1bb")
        );
        assert_eq!(
            b(KeyCode::Backspace, KeyModifiers::NONE).as_deref(),
            Some("\x7f")
        );
        assert_eq!(b(KeyCode::Tab, KeyModifiers::NONE).as_deref(), Some("\t"));
        assert_eq!(
            b(KeyCode::Up, KeyModifiers::NONE).as_deref(),
            Some("\x1b[A")
        );
        assert_eq!(
            b(KeyCode::Left, KeyModifiers::NONE).as_deref(),
            Some("\x1b[D")
        );
        assert_eq!(
            b(KeyCode::Delete, KeyModifiers::NONE).as_deref(),
            Some("\x1b[3~")
        );
        assert_eq!(
            b(KeyCode::Char('é'), KeyModifiers::NONE).as_deref(),
            Some("é")
        );
        assert_eq!(
            b(KeyCode::F(5), KeyModifiers::NONE),
            None,
            "keys with no bytes send nothing"
        );
    }

    #[test]
    fn the_leader_gives_the_keyboard_back_to_the_client() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        assert_eq!(
            v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            After::Continue
        );
        assert!(v.typing_pane().is_none());
        assert!(v.plain(120, 30).lines().last().unwrap().contains("MOVE"));
        // Now `j` moves, as before.
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.selected, 1);
        // Enter goes back to typing in the workspace it opens.
        v.press_key(plain(KeyCode::Enter));
        assert!(v.typing_pane().is_some());
    }

    #[test]
    fn before_a_pane_is_open_keys_are_the_clients() {
        let mut v = fixture_view();
        assert!(v.typing_pane().is_none());
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.selected, 1);
        assert_eq!(v.press_key(plain(KeyCode::Char('q'))), After::Quit);
    }

    #[test]
    fn the_pane_is_asked_to_fit_its_box_once() {
        let mut v = fixture_view();
        assert_eq!(v.take_resize(120, 30), None, "nothing is typed into yet");
        v.press(Key::Enter);
        let (pane, rows, cols) = v.take_resize(120, 30).expect("the focused box's size");
        assert_eq!(Some(pane), v.typing_pane());
        let (_, _, w, h) = pane_rects(2, panes_rect(120, 30))[0];
        assert_eq!((rows, cols), ((h - 2) as u16, (w - 4) as u16));
        assert_eq!(v.take_resize(120, 30), None, "asked once");
        assert!(
            v.take_resize(100, 30).is_some(),
            "a smaller terminal is a new size"
        );
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
