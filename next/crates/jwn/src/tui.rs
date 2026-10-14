//! `jwn tui`: the terminal client, laid out like the jw TUI it replaces (prototype:
//! `docs/prototypes/tui.html`). A sidebar of workspaces with the marks of what waits, a row
//! of facts on top, the open workspace's panes as rounded boxes, a status bar with the mode.
//! `a` lists what waits. It draws the same documents the phone draws and keeps its own seen
//! record. `View` is pure (documents in, a character grid out) so it is tested without a
//! terminal; `run` only moves bytes between the socket, the keyboard and the screen.

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::time::{Duration, Instant};

use contract::{
    Attention, AttentionKind, Blocks, BlocksMode, Cmd, DiffScope, Node, Pane, Program, Session,
    Workspace, WorkspaceKind,
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
mod look;
mod manage;
mod open;
mod phone;
mod settings;
mod shell;
mod side;

pub use changes::Ask;
use changes::{Changes, Docs, Outcome, Src};
use contract::ErrorBody;
use manage::{Act, Manage, Named, Target};
use settings::Settings;
use wire::ClientError;

type Rgb = contract::themes::Rgb;

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
                        // The screen paints the theme's own ground, whatever the terminal's colours are.
                        let (f, b) = (st.fg.unwrap_or(look::fg()), st.bg.unwrap_or(look::bg()));
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
        AttentionKind::Permission => ("!", look::red()),
        AttentionKind::Question => ("?", look::yellow()),
        AttentionKind::Plan => ("◆", look::magenta()),
        AttentionKind::CiFailed => ("✗", look::red()),
        AttentionKind::ShellDone if a.exit_code.unwrap_or(0) != 0 => ("✗", look::red()),
        AttentionKind::ShellDone => ("❯", look::green()),
        AttentionKind::Screen => ("…", look::dim()),
        AttentionKind::AgentDone => ("✓", look::green()),
        AttentionKind::Merged => ("⚑", look::cyan()),
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
        c if c == look::green() || c == look::dim() || c == look::cyan() => look::dim(),
        c => c,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Workspaces,
    Avisos,
}

/// Where the person is: the workspaces of the sidebar, the panels of the open workspace, or
/// typing in a panel. What a key does is what the level is about (spec REQ-169 to REQ-176).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Workspaces,
    Panels,
    Terminal,
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
    /// Close what the level is about: the highlighted workspace, or the focused panel.
    Close,
    /// The focused panel fills the area.
    Full,
    /// The keys of the level.
    Help,
    /// The mode and the theme.
    Settings,
    /// A worktree of the project the person is in.
    Worktree,
    /// The phone link: its box.
    Phone,
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
    /// Close a pane.
    ClosePane(String),
    /// Name a workspace or a panel (empty: the automatic name).
    Rename {
        target: Named,
        name: String,
    },
    /// Open a folder as a workspace with a shell; with `create`, make the folder first.
    OpenFolder {
        path: std::path::PathBuf,
        create: bool,
    },
    /// Make a worktree of `repo` on a new `branch` from `from`, in the folder `folder` of
    /// `<repo>-wt`, and open it with a shell.
    CreateWorktree {
        repo: std::path::PathBuf,
        branch: String,
        from: String,
        folder: String,
    },
    /// Close a workspace, and with `remove_worktree` remove its worktree. The daemon never
    /// forces: git refuses a worktree with changes.
    RemoveWorkspace {
        workspace: String,
        remove_worktree: bool,
    },
    /// Read the workspace's uncommitted changes, to know whether its worktree may go.
    Check(String),
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
    /// The question being asked before a pane or a workspace is closed.
    manage: Option<Manage>,
    /// The `uncommitted` node `manage` reads a worktree's changes from, while it is open.
    check_node: Option<String>,
    /// What the last command did, in the status bar until the next key.
    flash: String,
    /// The look the person chose, and whether the machine is dark (what `system` follows).
    settings: Settings,
    os_dark: bool,
    /// The settings changed here and are not written yet.
    save: bool,
    /// The folder browser and the worktree box, while one is open.
    picker: Option<open::Picker>,
    worktree: Option<open::Worktree>,
    /// A workspace made by the client's own request, to open when it shows up in the session
    /// (or at the time given, in `now_ms`, if it does not).
    goto: Option<(String, u64)>,
    /// The focused panel fills the area, and the box of the keys of the level is open.
    full: bool,
    help: bool,
    /// The phone link, and whether its box is open, how many phones are paired, whether the
    /// loop should start the thread that turns it on, and what to tell that thread.
    phone: phone::State,
    phone_open: bool,
    phone_paired: usize,
    phone_start: bool,
    phone_says: Vec<phone::Say>,
    /// When the pairing question appeared, in `now_ms`: a key typed before it was read is not an answer.
    ask_at: u64,
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
            manage: None,
            check_node: None,
            flash: String::new(),
            settings: Settings::default(),
            os_dark: true,
            save: false,
            picker: None,
            worktree: None,
            goto: None,
            full: false,
            help: false,
            phone: phone::State::Off,
            phone_open: false,
            phone_paired: 0,
            phone_start: false,
            phone_says: Vec::new(),
            ask_at: 0,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The settings file changed (or was read for the first time).
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    pub fn set_os_dark(&mut self, dark: bool) {
        self.os_dark = dark;
        if matches!(self.manage, Some(Manage::Settings { .. })) {
            self.manage = Some(self.settings_question());
        }
    }

    /// The settings question, on the theme in force among those of the kind in force (the
    /// mode picks the kind, so changing it brings the list of the other kind).
    fn settings_question(&self) -> Manage {
        let dark = self.settings.is_dark(self.os_dark);
        let chosen = &self.settings.theme(self.os_dark).id;
        Manage::Settings {
            cursor: contract::themes::of_kind(dark)
                .position(|t| t.id == *chosen)
                .unwrap_or(0),
            shown: contract::themes::of_kind(dark).count(),
        }
    }

    /// A change of the settings waits to be written: the file must not be read over it.
    pub fn unsaved(&self) -> bool {
        self.save
    }

    /// The settings the person just changed, to write.
    pub fn take_save(&mut self) -> Option<Settings> {
        std::mem::take(&mut self.save).then(|| self.settings.clone())
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
        self.advance_manage();
        self.follow_goto();
        self.clamp();
        let gone = self.blk.as_ref().is_some_and(|(pane, id)| {
            !self
                .blocks_of(pane)
                .is_some_and(|b| b.blocks.iter().any(|k| k.id == *id))
        });
        if gone {
            self.blk = None;
        }
    }

    /// A workspace or a pane that was closed leaves the highlight on its neighbour.
    fn clamp(&mut self) {
        let workspaces = self.session.as_ref().map_or(0, |s| s.workspaces.len());
        self.selected = self.selected.min(workspaces.saturating_sub(1));
        if let Some(panes) = self.shown().map(|w| w.panes.len()) {
            self.focus = self.focus.min(panes.saturating_sub(1));
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
        if self.opened.as_deref() != Some(ws) {
            self.full = false;
        }
        self.opened = Some(ws.to_owned());
        self.typing = false;
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

    /// Where the person is.
    fn level(&self) -> Level {
        if self.typing_pane().is_some() {
            Level::Terminal
        } else if self.opened.is_some() && self.opened == self.selected_id() {
            Level::Panels
        } else {
            Level::Workspaces
        }
    }

    /// The rectangles of the panels: while one is shown full, it has the whole area and the
    /// others none.
    fn layout(&self, n: usize, area: Rect) -> Vec<Rect> {
        let mut rects = pane_rects(n, area);
        if self.full && self.level() != Level::Workspaces {
            for (i, r) in rects.iter_mut().enumerate() {
                *r = if i == self.focus { area } else { (0, 0, 0, 0) };
            }
        }
        rects
    }

    /// Move the highlight of the sidebar to the next or the previous workspace.
    fn select_workspace(&mut self, forward: bool) {
        let n = self.session.as_ref().map_or(0, |s| s.workspaces.len());
        self.typing = false;
        self.full = false;
        self.selected = if forward {
            (self.selected + 1).min(n.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        self.opened = None;
        self.focus = 0;
    }

    /// Focus the next or the previous panel of the open workspace.
    fn go_panel(&mut self, forward: bool) {
        let Some(id) = self.opened.clone() else {
            return;
        };
        let panes = self.workspaces.get(&id).map_or(0, |d| d.panes.len());
        if panes == 0 {
            return;
        }
        let next = if forward {
            (self.focus + 1).min(panes - 1)
        } else {
            self.focus.saturating_sub(1)
        };
        self.open(&id, next);
    }

    /// The pane the keyboard goes to: the focused pane of the opened workspace, while typing.
    fn typing_pane(&self) -> Option<String> {
        if !self.typing || self.mode != Mode::Workspaces || self.changes.is_some() {
            return None;
        }
        let ws = self.workspaces.get(self.opened.as_deref()?)?;
        Some(ws.panes.get(self.focus)?.id.clone())
    }

    /// What runs in a pane, when something does: a program in its shell, or a command still
    /// going (`None` when the shell waits at its prompt).
    fn busy(&self, pane: &str) -> Option<String> {
        let p = self.panes.get(pane)?;
        if let Some(program) = &p.running {
            return Some(program_name(program));
        }
        let blocks = self.blocks.get(p.blocks.as_ref()?)?;
        let last = blocks.blocks.last().filter(|k| k.running)?;
        Some(clip(
            last.cmd.split_whitespace().next().unwrap_or(&last.cmd),
            24,
        ))
    }

    /// `x`: close the focused pane at once when its shell is idle, ask when something runs.
    fn ask_close_pane(&mut self) -> After {
        let Some(ws) = self.shown() else {
            return After::Continue;
        };
        let Some(pane) = ws.panes.get(self.focus) else {
            self.flash = "no hay panel enfocado".into();
            return After::Continue;
        };
        let (id, title) = (pane.id.clone(), pane.title.clone());
        match self.busy(&id) {
            None => After::ClosePane(id),
            Some(what) => {
                self.manage = Some(Manage::Pane {
                    pane: id,
                    title,
                    what,
                });
                After::Continue
            }
        }
    }

    /// The folders of the workspaces that are open.
    fn open_paths(&self) -> Vec<std::path::PathBuf> {
        self.session
            .iter()
            .flat_map(|s| s.workspaces.iter())
            .map(|w| std::path::PathBuf::from(&w.path))
            .collect()
    }

    /// The project folder of the highlighted workspace (a worktree's repository, or the
    /// folder itself), with its name, kind and branch.
    fn project_of_selected(
        &self,
    ) -> Option<(String, std::path::PathBuf, WorkspaceKind, Option<String>)> {
        let w = self.session.as_ref()?.workspaces.get(self.selected)?;
        let project = self
            .workspaces
            .get(&w.id)
            .and_then(|d| d.project.clone())
            .map_or_else(
                || std::path::PathBuf::from(&w.path),
                std::path::PathBuf::from,
            );
        Some((w.name.clone(), project, w.kind, w.branch.clone()))
    }

    /// `o`: the folder browser, beside the project the person is in (else at home).
    fn open_picker(&mut self) {
        let dir = self
            .project_of_selected()
            .and_then(|(_, project, _, _)| project.parent().map(std::path::Path::to_path_buf))
            .filter(|d| d.is_dir())
            .unwrap_or_else(daemon::browse::home);
        self.picker = Some(open::Picker::new(dir));
    }

    /// `w`: the box of a worktree of the project the person is in.
    fn open_worktree_box(&mut self) {
        let Some((name, project, kind, branch)) = self.project_of_selected() else {
            return;
        };
        let (repo, from) = match (kind, branch) {
            (WorkspaceKind::Worktree, Some(branch)) => (Some(project), Some(branch)),
            _ => (
                open::git_out(&project, &["rev-parse", "--show-toplevel"])
                    .map(std::path::PathBuf::from),
                open::git_out(&project, &["rev-parse", "--abbrev-ref", "HEAD"]),
            ),
        };
        let Some(repo) = repo else {
            self.flash = format!("«{name}» no es un repositorio git: w necesita uno");
            return;
        };
        self.worktree = Some(open::Worktree::new(
            repo,
            from.unwrap_or_else(|| "HEAD".into()),
            self.open_paths(),
        ));
    }

    /// A key in the folder browser.
    fn picker_key(&mut self, ev: KeyEvent) -> After {
        let paths = self.open_paths();
        let is_open = |p: &std::path::Path| paths.iter().any(|o| o == p);
        let Some(picker) = self.picker.as_mut() else {
            return After::Continue;
        };
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match picker.press(ev.code, ctrl, &is_open) {
            open::Pick::Stay => After::Continue,
            open::Pick::Close => {
                self.picker = None;
                After::Continue
            }
            open::Pick::Open { path, create } => {
                self.picker = None;
                // Already open: go to it.
                let there = self.session.as_ref().and_then(|s| {
                    s.workspaces
                        .iter()
                        .position(|w| std::path::Path::new(&w.path) == path)
                        .map(|i| (i, s.workspaces[i].id.clone()))
                });
                if let Some((i, id)) = there {
                    self.selected = i;
                    self.open(&id, 0);
                    self.flash = "ya estaba abierta: voy a ella".into();
                    return After::Continue;
                }
                After::OpenFolder { path, create }
            }
        }
    }

    /// A key in the worktree box.
    fn worktree_key(&mut self, ev: KeyEvent) -> After {
        let Some(t) = self.worktree.as_mut() else {
            return After::Continue;
        };
        match t.press(ev.code, ev.modifiers.contains(KeyModifiers::CONTROL)) {
            open::Make::Stay => After::Continue,
            open::Make::Close => {
                self.worktree = None;
                After::Continue
            }
            open::Make::Create {
                repo,
                branch,
                from,
                folder,
            } => {
                self.worktree = None;
                After::CreateWorktree {
                    repo,
                    branch,
                    from,
                    folder,
                }
            }
        }
    }

    /// Open this workspace as soon as the daemon shows it: the client asked for it to be made.
    pub fn goto(&mut self, workspace: &str) {
        self.goto = Some((workspace.to_owned(), self.now_ms + 8_000));
    }

    /// The workspace the client asked for, once it is in the session: selected and opened.
    fn follow_goto(&mut self) {
        let Some((id, until)) = self.goto.clone() else {
            return;
        };
        let at = self
            .session
            .as_ref()
            .and_then(|s| s.workspaces.iter().position(|w| w.id == id));
        if let Some(i) = at {
            self.selected = i;
            self.open(&id, 0);
            self.goto = None;
        } else if self.now_ms > until {
            self.goto = None;
        }
    }

    /// `r`: ask for a new name for the highlighted workspace, or for the focused panel.
    fn ask_rename(&mut self, level: Level) {
        let (target, title, text) = match level {
            Level::Workspaces => {
                let Some(w) = self
                    .session
                    .as_ref()
                    .and_then(|s| s.workspaces.get(self.selected))
                else {
                    return;
                };
                (
                    Named::Workspace(w.id.clone()),
                    "renombrar el workspace".to_owned(),
                    shown_name(&w.name, w.kind, w.branch.as_deref()).to_owned(),
                )
            }
            _ => {
                let Some(p) = self.shown().and_then(|w| w.panes.get(self.focus)) else {
                    return;
                };
                (
                    Named::Pane(p.id.clone()),
                    "nombrar el panel".to_owned(),
                    p.title.clone(),
                )
            }
        };
        self.manage = Some(Manage::Rename {
            target,
            title,
            text,
        });
    }

    /// `x` at the workspace level: ask before the highlighted workspace is closed.
    fn ask_close_workspace(&mut self) {
        let Some(w) = self
            .session
            .as_ref()
            .and_then(|s| s.workspaces.get(self.selected))
        else {
            return;
        };
        let doc = self.workspaces.get(&w.id);
        let running = doc
            .into_iter()
            .flat_map(|d| d.panes.iter())
            .filter_map(|p| self.busy(&p.id))
            .collect();
        let target = Target {
            workspace: w.id.clone(),
            name: self.label(&w.id),
            path: w.path.clone(),
            branch: w.branch.clone(),
            worktree: w.kind == WorkspaceKind::Worktree,
            panes: doc.map_or(w.pane_count as usize, |d| d.panes.len()),
            running,
        };
        self.manage = Some(Manage::Workspace(target));
    }

    /// A key while a question is open: it answers it, and is never a pane's.
    fn manage_key(&mut self, ev: KeyEvent) -> After {
        // A chord is not a letter of a name: it answers nothing.
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return After::Continue;
        }
        let Some(question) = self.manage.take() else {
            return After::Continue;
        };
        let (next, act) = question.press(ev.code);
        self.manage = next;
        if self.manage.is_none() {
            self.surface_ask();
        }
        if !self.manage.as_ref().is_some_and(Manage::is_check) {
            self.drop_check();
        }
        match act {
            None => After::Continue,
            Some(Act::ClosePane(pane)) => After::ClosePane(pane),
            Some(Act::Check(workspace)) => After::Check(workspace),
            Some(Act::Rename { target, name }) => After::Rename { target, name },
            Some(Act::Theme(at)) => {
                let kind = self.settings.is_dark(self.os_dark);
                if let Some(t) = contract::themes::of_kind(kind).nth(at) {
                    self.settings.choose(self.os_dark, &t.id);
                    self.save = true;
                }
                After::Continue
            }
            Some(Act::StepMode(forward)) => {
                self.settings.mode = self.settings.mode.step(forward);
                self.save = true;
                self.manage = Some(self.settings_question());
                After::Continue
            }
            Some(Act::RemoveWorkspace {
                workspace,
                remove_worktree,
            }) => After::RemoveWorkspace {
                workspace,
                remove_worktree,
            },
        }
    }

    /// `m`: show the phone box, turning the link on when it is off.
    fn open_phone(&mut self) {
        if self.manage.is_some() {
            return;
        }
        if matches!(self.phone, phone::State::Off | phone::State::Failed(_)) {
            self.phone = phone::State::Starting;
            self.phone_start = true;
        }
        self.phone_open = true;
    }

    /// A key in the phone box. While a phone asks to pair only `y` accepts, and only once the
    /// question has been on the screen for a moment; any other key refuses.
    fn phone_key(&mut self, ev: KeyEvent) -> After {
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return After::Continue;
        }
        match (&self.phone, ev.code) {
            (phone::State::Asking { .. }, KeyCode::Char('y')) => {
                if self.now_ms >= self.ask_at + ANSWER_DELAY_MS {
                    self.phone_says.push(phone::Say::Answer(true));
                }
            }
            (phone::State::Asking { .. }, _) => self.phone_says.push(phone::Say::Answer(false)),
            (phone::State::Failed(_), _) => {
                self.phone = phone::State::Off;
                self.phone_open = false;
            }
            (phone::State::Ready(link), KeyCode::Char('c')) => {
                let uri = link.uri.clone();
                self.flash = "enlace copiado".into();
                return After::Copy(uri);
            }
            (phone::State::Ready(_), KeyCode::Char('s')) => {
                self.phone_says.push(phone::Say::Stop);
                self.phone = phone::State::Off;
                self.phone_open = false;
                self.flash = "el enlace con el teléfono está apagado".into();
            }
            (_, KeyCode::Esc) => self.phone_open = false,
            _ => {}
        }
        After::Continue
    }

    /// What the thread of the phone link reported.
    pub fn phone_event(&mut self, event: phone::Event) {
        use phone::{Event, State};
        let link = match &self.phone {
            State::Ready(l) | State::Asking { link: l, .. } => Some(l.clone()),
            _ => None,
        };
        match (event, link) {
            // The link again, with the tunnel in it: a phone asking stays asked.
            (Event::Ready(link), Some(_)) => {
                let opened = link.tunnel.clone();
                self.phone = match std::mem::replace(&mut self.phone, State::Off) {
                    State::Asking { name, code, .. } => State::Asking { link, name, code },
                    _ => State::Ready(link),
                };
                if opened.is_some() && !self.phone_open {
                    self.flash = "túnel listo: el teléfono ya puede entrar desde fuera · m".into();
                }
            }
            (Event::Ready(link), None) => {
                self.phone = State::Ready(link);
                if !self.phone_open {
                    self.flash = "enlace con el teléfono listo · m".into();
                }
            }
            (Event::Failed(why), _) => {
                self.phone = State::Failed(why);
                self.phone_open = self.manage.is_none();
                if !self.phone_open {
                    self.flash = "no se pudo encender el enlace con el teléfono · m".into();
                }
            }
            (Event::Ask { name, code }, Some(link)) => {
                self.phone = State::Asking { link, name, code };
                self.ask_at = self.now_ms;
                self.surface_ask();
            }
            (Event::Answered { yes }, Some(link)) => {
                let name = match &self.phone {
                    State::Asking { name, .. } => name.clone(),
                    _ => String::new(),
                };
                self.phone = State::Ready(link);
                self.flash = if yes {
                    format!("emparejado «{name}»")
                } else {
                    format!("rechazado «{name}»")
                };
            }
            (Event::Paired(n), _) => self.phone_paired = n,
            (Event::Off, _) => {
                self.phone = State::Off;
                self.phone_open = false;
            }
            (Event::Ask { .. } | Event::Answered { .. }, None) => {}
        }
    }

    /// The pairing question opens its box by itself, unless another question is open: a key
    /// meant for that one must not answer this.
    fn surface_ask(&mut self) {
        if matches!(self.phone, phone::State::Asking { .. }) && self.manage.is_none() {
            self.phone_open = true;
            self.typing = false;
        } else if matches!(self.phone, phone::State::Asking { .. }) {
            self.flash = "un teléfono pide emparejarse · m".into();
        }
    }

    /// The loop starts the thread of the link when this says so.
    pub fn take_phone_start(&mut self) -> bool {
        std::mem::take(&mut self.phone_start)
    }

    pub fn take_phone_says(&mut self) -> Vec<phone::Say> {
        std::mem::take(&mut self.phone_says)
    }

    /// The link was turned off: the thread is done.
    pub fn phone_is_off(&self) -> bool {
        self.phone == phone::State::Off
    }

    /// Stop watching the node a worktree's changes were read from.
    fn drop_check(&mut self) {
        if let Some(node) = self.check_node.take() {
            self.asks.push(Ask::Close(node));
        }
    }

    /// The daemon made the node of the worktree's changes; its document will follow.
    pub fn check_made(&mut self, node: &str) {
        if self.manage.as_ref().is_some_and(Manage::is_check) {
            self.check_node = Some(node.to_owned());
            self.asks.push(Ask::Watch(node.to_owned()));
        } else {
            self.asks.push(Ask::Close(node.to_owned()));
        }
    }

    /// The daemon refused something the person asked: say why, and leave things as they are.
    pub fn manage_failed(&mut self, title: &str, text: &str) {
        self.drop_check();
        self.manage = Some(Manage::Failed {
            title: title.to_owned(),
            text: text.to_owned(),
        });
    }

    pub fn flash(&mut self, text: &str) {
        self.flash = text.to_owned();
    }

    /// The question reads the changes as they arrive.
    fn advance_manage(&mut self) {
        let Some(Manage::Checking(target)) = &self.manage else {
            return;
        };
        if let Some(diff) = self.check_node.as_ref().and_then(|n| self.diffs.get(n)) {
            self.manage = Some(Manage::with_changes(target.clone(), diff));
        }
    }

    /// A key at the keyboard: typed into the pane, or (after the leader, or when no pane is
    /// open) a command of the client.
    pub fn press_key(&mut self, ev: KeyEvent) -> After {
        self.flash.clear();
        if self.help {
            self.help = false;
            return After::Continue;
        }
        if self.picker.is_some() {
            return self.picker_key(ev);
        }
        if self.worktree.is_some() {
            return self.worktree_key(ev);
        }
        if self.phone_open {
            return self.phone_key(ev);
        }
        if self.manage.is_some() {
            return self.manage_key(ev);
        }
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
        let (_, _, w, h) = *self.layout(n, panes_rect(cols, rows)).get(self.focus)?;
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
        let level = self.level();
        match (self.mode, key) {
            (_, Key::Quit) => return After::Quit,
            // `esc` goes up a level; at the first there is nowhere to go.
            (Mode::Workspaces, Key::Back) => {
                if level == Level::Panels {
                    self.opened = None;
                    self.full = false;
                    self.blk = None;
                }
            }
            (Mode::Avisos, Key::Back | Key::Avisos) => self.mode = Mode::Workspaces,
            (Mode::Workspaces, Key::Avisos) => {
                self.mode = Mode::Avisos;
                self.avisos_selected = 0;
            }
            (Mode::Workspaces, Key::Help) => self.help = true,
            (Mode::Workspaces, Key::Down) if level == Level::Workspaces => {
                self.select_workspace(true)
            }
            (Mode::Workspaces, Key::Up) if level == Level::Workspaces => {
                self.select_workspace(false)
            }
            (Mode::Workspaces, Key::Down | Key::Right) if level == Level::Panels => {
                self.go_panel(true)
            }
            (Mode::Workspaces, Key::Up | Key::Left) if level == Level::Panels => {
                self.go_panel(false)
            }
            (Mode::Workspaces, Key::Enter) => match level {
                Level::Workspaces => {
                    if let Some(id) = self.selected_id() {
                        self.open(&id, self.focus);
                    }
                }
                Level::Panels => {
                    let panes = self
                        .opened
                        .as_ref()
                        .and_then(|id| self.workspaces.get(id))
                        .map_or(0, |d| d.panes.len());
                    self.typing = panes > 0;
                }
                Level::Terminal => {}
            },
            (Mode::Workspaces, Key::Full) if level == Level::Panels => self.full = !self.full,
            (Mode::Workspaces, Key::Repeat) if level != Level::Terminal => self.ask_rename(level),
            (Mode::Workspaces, Key::CopyOut) if level != Level::Terminal => self.open_picker(),
            (Mode::Workspaces, Key::Worktree) if level != Level::Terminal => {
                self.open_worktree_box()
            }
            (Mode::Workspaces, Key::Close) => match level {
                Level::Workspaces => self.ask_close_workspace(),
                Level::Panels => return self.ask_close_pane(),
                Level::Terminal => {}
            },
            (Mode::Workspaces, Key::Changes) => {
                if let Some(id) = self.selected_id() {
                    self.typing = false;
                    self.blk = None;
                    self.changes = Some(Changes::open(&id));
                }
            }
            (Mode::Workspaces, Key::NewPane) if level == Level::Panels => {
                if let Some(id) = self.selected_id() {
                    return After::NewPane(id);
                }
            }
            (Mode::Workspaces, Key::Settings) => self.manage = Some(self.settings_question()),
            (Mode::Workspaces, Key::Phone) => self.open_phone(),
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
                    // An attention is a call to answer: the keyboard goes to the pane.
                    self.typing = self
                        .workspaces
                        .get(&ws)
                        .is_some_and(|d| !d.panes.is_empty());
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
        if self.help {
            self.help = false;
            return After::Continue;
        }
        if self.manage.is_some()
            || self.phone_open
            || self.picker.is_some()
            || self.worktree.is_some()
        {
            return After::Continue;
        }
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
        let row = self
            .session
            .as_ref()
            .filter(|_| x + 1 < SIDEBAR && y >= 2 && y < rows.saturating_sub(5))
            .and_then(|s| side::rows(&s.workspaces).into_iter().nth(y - 2));
        if let Some(side::Side::Workspace(i)) = row {
            let same = self.selected == i;
            self.selected = i;
            if let Some(id) = self.selected_id() {
                self.open(&id, if same { self.focus } else { 0 });
            }
            return After::Continue;
        }
        if let Some(doc) = self.shown() {
            let id = doc.head.id.clone();
            let rects = self.layout(doc.panes.len(), panes_rect(cols, rows));
            if let Some(i) = rects.iter().position(|r| inside(*r, x, y)) {
                let pane = doc.panes[i].id.clone();
                self.open(&id, i);
                // A click in a panel starts typing in it, unless it picks a block.
                self.typing = true;
                return self.click_pane(&pane, rects[i], x, y);
            }
        }
        After::Continue
    }

    /// The wheel over the sidebar (or the avisos list) moves the highlight; over a pane drawn
    /// as blocks it scrolls them.
    pub fn scroll(&mut self, down: bool, x: usize, y: usize, cols: usize, rows: usize) -> After {
        if self.help
            || self.manage.is_some()
            || self.phone_open
            || self.picker.is_some()
            || self.worktree.is_some()
        {
            return After::Continue;
        }
        if self.changes.is_some() {
            if x >= SIDEBAR {
                if let Some(c) = self.changes.as_mut() {
                    c.wheel(down, x, (cols, rows));
                }
                return After::Continue;
            }
            self.close_changes();
        }
        if self.mode == Mode::Avisos {
            return self.press(if down { Key::Down } else { Key::Up });
        }
        if x < SIDEBAR {
            self.select_workspace(down);
            return After::Continue;
        }
        let Some(doc) = self.shown() else {
            return After::Continue;
        };
        let rects = self.layout(doc.panes.len(), panes_rect(cols, rows));
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
        look::set(
            self.settings.palette(self.os_dark),
            self.settings.is_dark(self.os_dark),
        );
        let mut g = Grid::new(cols, rows);
        let Some(session) = &self.session else {
            g.put(
                1,
                0,
                &format!("conectando… {}", self.status),
                Style::fg(look::dim()),
            );
            return g;
        };
        let dim = Style::fg(look::dim());
        // The sidebar and its border, blue while the sidebar is where the person is.
        let divider = if self.mode == Mode::Workspaces && self.level() == Level::Workspaces {
            look::blue()
        } else {
            look::line()
        };
        for y in 0..rows.saturating_sub(1) {
            g.put(SIDEBAR - 1, y, "│", Style::fg(divider));
        }
        g.put(1, 0, &clip(&session.name, 12), dim.bold());
        for (k, row) in side::rows(&session.workspaces).iter().enumerate() {
            let y = 2 + k;
            if y >= rows.saturating_sub(5) {
                break;
            }
            let side::Side::Workspace(i) = *row else {
                if let side::Side::Head(head) = row {
                    g.put_over(2, y, &clip(head, SIDEBAR - 4), dim);
                }
                continue;
            };
            let w = &session.workspaces[i];
            let worktree = w.kind == WorkspaceKind::Worktree;
            let on = i == self.selected && self.mode == Mode::Workspaces;
            let un = self.unseen(&w.attention);
            if on {
                g.paint_bg(0, y, SIDEBAR - 1, look::sel());
            }
            let name = if worktree {
                shown_name(&w.name, w.kind, w.branch.as_deref()).to_owned()
            } else {
                w.name.clone()
            };
            // A worktree is drawn four columns in, under its root (REQ-223).
            let (dot, text) = if worktree { (6, 8) } else { (2, 4) };
            let name_style = if on {
                Style::fg(look::blue()).bold()
            } else if w.pane_count == 0 {
                dim
            } else {
                Style::fg(look::fg())
            };
            let number = if i < 9 {
                (i + 1).to_string()
            } else {
                " ".into()
            };
            g.put_over(0, y, &number, dim);
            if worktree {
                g.put_over(4, y, "↳", dim);
            }
            g.put_over(
                dot,
                y,
                if w.pane_count > 0 { "●" } else { "○" },
                if w.pane_count > 0 {
                    Style::fg(look::green())
                } else {
                    dim
                },
            );
            g.put_over(text, y, &clip(&name, SIDEBAR - text - 6), name_style);
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
        // The phone link, while it is on, above the legend.
        if self.phone.is_on() {
            g.put(
                1,
                rows.saturating_sub(5),
                "● teléfono · m",
                Style::fg(look::magenta()),
            );
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
            let title = shown_name(&ws.name, ws.kind, ws.branch.as_deref()).to_owned();
            g.put(x, 0, &title, Style::fg(look::blue()).bold());
            x += title.chars().count() + 2;
            if ws.kind == WorkspaceKind::Worktree {
                g.put(
                    x,
                    0,
                    &format!("⎇ {}", ws.branch.clone().unwrap_or_default()),
                    Style::fg(look::magenta()),
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
            g.cells[(rows - 1) * cols + x] = (' ', Style::default().on(look::panel()));
        }
        let level = self.level();
        let diff_status = self.changes.as_ref().map(|c| c.status(&self.docs()));
        let (chip, bg, hint) = match (&self.manage, &diff_status) {
            (None, _) if self.picker.is_some() => (
                " ABRIR ",
                look::cyan(),
                "↑↓ mover · → entrar · ← subir · ~ casa · ↵ abrir · esc cerrar",
            ),
            (None, _) if self.worktree.is_some() => {
                (" WORKTREE ", look::cyan(), "↵ crear · esc cancelar")
            }
            (Some(Manage::Failed { .. }), _) => (" ERROR ", look::red(), self.manage_hint()),
            (Some(Manage::Settings { .. }), _) => (" AJUSTES ", look::blue(), self.manage_hint()),
            (None, _) if self.phone_open => match &self.phone {
                phone::State::Asking { .. } => {
                    (" PREGUNTA ", look::yellow(), phone::hint(&self.phone))
                }
                phone::State::Failed(_) => (" ERROR ", look::red(), phone::hint(&self.phone)),
                _ => (" TELÉFONO ", look::magenta(), phone::hint(&self.phone)),
            },
            (Some(_), _) => (" PREGUNTA ", look::yellow(), self.manage_hint()),
            (None, Some((hint, _))) => (" DIFF ", look::cyan(), hint.as_str()),
            (None, None) => match (self.mode, level) {
                (Mode::Avisos, _) => (" AVISOS ", look::yellow(), "j/k mover · ↵ ir · esc volver"),
                (_, Level::Terminal) => (
                    " TERMINAL ",
                    look::green(),
                    "escribes en el panel · ^␣ para salir",
                ),
                (_, Level::Panels) if self.blk.is_some() => (
                    " BLOQUE ",
                    look::magenta(),
                    "c comando · o salida · r repetir · j/k mover · esc soltar",
                ),
                (_, Level::Panels) => (
                    " PANEL ",
                    look::magenta(),
                    "h/l panel · ↵ escribir · n nuevo · r nombrar · f completa · x cerrar · esc volver · ? teclas",
                ),
                (_, Level::Workspaces) => (
                    " WORKSPACE ",
                    look::blue(),
                    "j/k mover · ↵ entrar · r renombrar · o carpeta · w worktree · x cerrar · d cambios · m teléfono · ? teclas · q salir",
                ),
            },
        };
        g.put(
            0,
            rows - 1,
            chip,
            Style {
                fg: Some(look::panel()),
                bg: Some(bg),
                bold: true,
                ..Style::default()
            },
        );
        let (hint, hint_style) = match self.flash.as_str() {
            "" => (hint, dim.on(look::panel())),
            flash => (flash, Style::fg(look::green()).on(look::panel())),
        };
        g.put(
            chip.chars().count() + 1,
            rows - 1,
            &clip(hint, cols.saturating_sub(chip.chars().count() + 14)),
            hint_style,
        );
        let waiting = self.avisos().len();
        let mut right = cols;
        if waiting > 0 {
            let t = format!("avisos {waiting}");
            right = cols.saturating_sub(t.chars().count() + 1);
            g.put(
                right,
                rows - 1,
                &t,
                Style::fg(look::yellow()).bold().on(look::panel()),
            );
        }
        if let Some((_, Some((seen, total)))) = &diff_status {
            let t = format!("{seen}/{total} vistos");
            let tone = if seen == total {
                look::green()
            } else {
                look::dim()
            };
            g.put(
                right.saturating_sub(t.chars().count() + 2),
                rows - 1,
                &t,
                Style::fg(tone).on(look::panel()),
            );
        }
        if self.mode == Mode::Avisos {
            self.avisos_box(&mut g, (px, 1, pw, rows.saturating_sub(2)));
        }
        if let Some(p) = &self.picker {
            let paths = self.open_paths();
            open::draw_picker(&mut g, (px, 1, pw, rows.saturating_sub(2)), p, &|d| {
                paths.iter().any(|o| o == d)
            });
        }
        if let Some(t) = &self.worktree {
            open::draw_worktree(&mut g, (px, 1, pw, rows.saturating_sub(2)), t);
        }
        if self.help {
            self.help_box(&mut g, (px, 1, pw, rows.saturating_sub(2)));
        }
        if self.phone_open {
            phone::draw(
                &mut g,
                (px, 1, pw, rows.saturating_sub(2)),
                &self.phone,
                self.phone_paired,
            );
        }
        if let Some(question) = &self.manage {
            question.draw(
                &mut g,
                (px, 1, pw, rows.saturating_sub(2)),
                &self.settings,
                self.os_dark,
            );
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
                "este workspace no tiene paneles · ↵ y luego n abre uno",
                Style::fg(look::dim()),
            );
            return;
        }
        let rects = self.layout(n, (x, y, w, h));
        let opened = self.opened.as_deref() == Some(doc.head.id.as_str());
        for (i, (summary, rect)) in doc.panes.iter().zip(rects).enumerate() {
            if rect.2 == 0 {
                continue;
            }
            let focused = opened && i == self.focus;
            let un = self.unseen(&summary.attention);
            let badge = un.first().map(|a| {
                (
                    format!("{} {}", mark(a).0, what(a)),
                    Style::fg(tone(a)).bold(),
                )
            });
            let title_style = if focused {
                Style::fg(look::blue()).bold()
            } else {
                Style::fg(look::fg()).bold()
            };
            g.boxed(
                rect,
                &summary.title,
                if focused { look::blue() } else { look::line() },
                title_style,
                badge,
            );
            let (bx, by, bw, bh) = rect;
            let Some(p) = self.panes.get(&summary.id) else {
                skeleton(g, rect);
                continue;
            };
            if let Some(b) = self.blocks_of(&summary.id) {
                self.blocks_view(g, b, p, rect, focused, branch);
                continue;
            }
            // The shell has blocks that have not arrived: the screen would show for a moment
            // and then give way to them, so it waits.
            if p.blocks
                .as_ref()
                .is_some_and(|id| !self.blocks.contains_key(id))
            {
                skeleton(g, rect);
                continue;
            }
            for (r, line) in p.screen.lines.iter().take(bh.saturating_sub(2)).enumerate() {
                g.put(
                    bx + 2,
                    by + 1 + r,
                    &clip(line, bw.saturating_sub(4)),
                    Style::fg(look::fg()),
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
                g.paint_bg(ix, y, iw, look::sel());
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
        g.put(ix, ey, &"─".repeat(iw), Style::fg(look::line()));
        let last = b.blocks.last();
        if let Some(k) = last.filter(|k| k.running) {
            g.put(
                ix,
                ey + 1,
                &format!(
                    "◌ {}",
                    clip(&k.cmd, iw.saturating_sub(STOP.chars().count() + 4))
                ),
                Style::fg(look::yellow()),
            );
            g.put(
                ix + iw - STOP.chars().count(),
                ey + 1,
                STOP,
                Style::fg(look::red()).bold(),
            );
        } else {
            let folder = shell::short(&p.cwd);
            g.put(ix, ey + 1, &clip(&folder, iw), Style::fg(look::blue()));
            if let Some(branch) = branch {
                let at = ix + folder.chars().count() + 2;
                g.put(
                    at,
                    ey + 1,
                    &clip(&format!("⎇ {branch}"), (ix + iw).saturating_sub(at)),
                    Style::fg(look::magenta()),
                );
            }
        }
        let cur = p.screen.cursor;
        if let Some(line) = p.screen.lines.get(cur.row as usize) {
            g.put(ix, ey + 2, &clip(line, iw), Style::fg(look::fg()));
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
                    g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(look::panel()));
                }
            }
        }
        g.boxed(
            (bx, by, bw, bh),
            "avisos",
            look::yellow(),
            Style::fg(look::fg()).bold(),
            None,
        );
        // The box leaves the panel background under its border characters.
        if items.is_empty() {
            g.put(
                bx + 3,
                by + 2,
                "Nada espera.",
                Style::fg(look::dim()).on(look::panel()),
            );
        }
        for (i, (_, name, a)) in items.iter().enumerate() {
            let row = by + 2 + i;
            let bg = if i == self.avisos_selected {
                look::sel()
            } else {
                look::panel()
            };
            g.paint_bg(bx + 1, row, bw - 2, bg);
            let (glyph, colour) = mark(a);
            g.put_over(bx + 3, row, glyph, Style::fg(colour).bold());
            g.put_over(
                bx + 6,
                row,
                &clip(name, bw.saturating_sub(24)),
                Style::fg(look::fg()).bold(),
            );
            let t = what(a);
            g.put_over(bx + bw - 3 - t.chars().count(), row, &t, Style::fg(tone(a)));
        }
    }

    /// The keys of the level, as a box.
    fn help_box(&self, g: &mut Grid, (x, y, w, _): Rect) {
        let (title, keys): (&str, &[(&str, &str)]) = match self.level() {
            Level::Workspaces => ("teclas · workspace", WORKSPACE_KEYS),
            Level::Panels => ("teclas · panel", PANEL_KEYS),
            Level::Terminal => ("teclas · terminal", TERMINAL_KEYS),
        };
        let bw = 62.min(w.saturating_sub(2));
        let bh = keys.len() + 4;
        let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 3);
        for r in 0..bh {
            for c in 0..bw {
                if by + r < g.rows && bx + c < g.cols {
                    g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(look::panel()));
                }
            }
        }
        g.boxed(
            (bx, by, bw, bh),
            title,
            look::blue(),
            Style::fg(look::fg()).bold(),
            None,
        );
        for (i, (key, what)) in keys.iter().enumerate() {
            let at = by + 2 + i;
            g.put(
                bx + 3,
                at,
                key,
                Style::fg(look::blue()).bold().on(look::panel()),
            );
            g.put(
                bx + 14,
                at,
                &clip(what, bw.saturating_sub(17)),
                Style::fg(look::fg()).on(look::panel()),
            );
        }
    }

    fn manage_hint(&self) -> &'static str {
        self.manage.as_ref().map_or("", Manage::hint)
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
/// The keys of each level, for the box `?` opens. Only what the client does is listed.
const WORKSPACE_KEYS: &[(&str, &str)] = &[
    ("j k", "mover por los workspaces"),
    ("↵", "entrar al workspace"),
    ("r", "renombrar el workspace"),
    ("o", "abrir una carpeta"),
    ("w", "worktree nuevo del proyecto"),
    ("x", "cerrar el workspace (y quitar su worktree)"),
    ("d", "ver sus cambios"),
    ("a", "avisos"),
    ("m", "enlace con el teléfono"),
    (",", "ajustes"),
    ("q", "salir (los paneles siguen)"),
];
const PANEL_KEYS: &[(&str, &str)] = &[
    ("h j k l", "cambiar de panel"),
    ("↵", "escribir en el panel"),
    ("n", "panel nuevo"),
    ("r", "nombrar el panel"),
    ("f", "el panel a pantalla completa"),
    ("x", "cerrar el panel"),
    ("o w", "abrir una carpeta · worktree nuevo"),
    ("d a m ,", "cambios · avisos · teléfono · ajustes"),
    ("esc", "volver a los workspaces"),
];
const TERMINAL_KEYS: &[(&str, &str)] = &[
    ("todo", "va al programa, esc y ctrl incluidos"),
    ("^␣", "salir al nivel de paneles"),
];

/// How long a pairing question is on the screen before `y` answers it.
const ANSWER_DELAY_MS: u64 = 800;
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

/// What a workspace is called on the screen: a worktree that has the name it got when it was
/// made (`repo: branch`) shows its branch, and a name the person chose shows as chosen.
fn shown_name<'a>(name: &'a str, kind: WorkspaceKind, branch: Option<&'a str>) -> &'a str {
    match (kind, branch) {
        (WorkspaceKind::Worktree, Some(branch))
            if name.strip_suffix(branch).is_some_and(|p| p.ends_with(": ")) =>
        {
            branch
        }
        _ => name,
    }
}

/// What a pane's box shows before its document arrives: grey bars where the lines will be.
fn skeleton(g: &mut Grid, (x, y, w, _): Rect) {
    let inner = w.saturating_sub(4);
    for (i, share) in [55, 80, 40, 65, 30].into_iter().enumerate() {
        let bar = "░".repeat((inner * share / 100).max(1));
        g.put(x + 2, y + 2 + i * 2, &bar, Style::fg(look::line()));
    }
}

/// The name a running program is shown by: its command without the folders.
fn program_name(program: &Program) -> String {
    program
        .argv
        .first()
        .map_or("un programa", |a| base(a))
        .to_owned()
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
        KeyCode::Char('x') => Key::Close,
        KeyCode::Char('f') => Key::Full,
        KeyCode::Char('?') => Key::Help,
        KeyCode::Char(',') => Key::Settings,
        KeyCode::Char('m') => Key::Phone,
        KeyCode::Char('w') => Key::Worktree,
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

/// How often the settings file is read again, and the machine asked whether it is dark.
const SETTINGS_EVERY: Duration = Duration::from_secs(2);
const APPEARANCE_EVERY: Duration = Duration::from_secs(5);

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

/// Read a worktree's uncommitted changes for the question that asks to remove it. A node the
/// daemon still holds from an earlier look is returned as it was and is only read again while
/// somebody watches it, so it is read now and the copy this client kept is dropped: the
/// answer decides what is offered.
fn check_changes(view: &mut View, client: &mut Client, workspace: String) {
    const FAILED: &str = "No se pudo mirar los cambios";
    let cmd = Cmd::OpenDiff {
        workspace,
        scope: DiffScope::Uncommitted,
        commit: None,
    };
    let node = match client.cmd_within(cmd, ASK_WAIT) {
        Ok(Ok(result)) => result
            .as_ref()
            .and_then(|r| r["node"].as_str().map(str::to_owned)),
        Ok(Err(e)) => return view.manage_failed(FAILED, &e.message),
        Err(e) => return view.manage_failed(FAILED, &e.to_string()),
    };
    let Some(node) = node else {
        return view.manage_failed(FAILED, "el daemon no dio el nodo");
    };
    match client.cmd_within(Cmd::RefreshDiff { node: node.clone() }, ASK_WAIT) {
        Ok(Ok(_)) => {
            client.nodes.remove(&node);
            view.check_made(&node);
        }
        Ok(Err(e)) => view.manage_failed(FAILED, &e.message),
        Err(e) => view.manage_failed(FAILED, &e.to_string()),
    }
}

/// A workspace the daemon made for the client: give it a shell, and open it when it appears.
fn shown_in_a_pane(view: &mut View, client: &mut Client, reply: Reply, failed: &str) {
    match reply {
        Ok(Ok(result)) => {
            if let Some(ws) = result.as_ref().and_then(|r| r["workspace"].as_str()) {
                match client.cmd(Cmd::NewPane {
                    workspace: ws.to_owned(),
                    program: None,
                }) {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => view.manage_failed("No se pudo abrir un panel", &e.message),
                    Err(e) => view.manage_failed("No se pudo abrir un panel", &e.to_string()),
                }
                view.goto(ws);
            }
        }
        Ok(Err(e)) => view.manage_failed(failed, &e.message),
        Err(e) => view.manage_failed(failed, &e.to_string()),
    }
}

/// Show how the daemon answered a close or a removal: `done` in the status bar, or why not.
fn told(view: &mut View, failed: &str, done: &str, reply: Reply) {
    match reply {
        Ok(Ok(_)) => view.flash(done),
        Ok(Err(e)) => view.manage_failed(failed, &e.message),
        Err(e) => view.manage_failed(failed, &e.to_string()),
    }
}

pub fn run() -> Result<(), String> {
    let mut view = View::new("conectando");
    let mut client = connect(&mut view)?;
    // `JWN_TUI_DUMP`: also write each frame, without colours, to this file (for tests).
    let dump = std::env::var_os(identity::env_var("TUI_DUMP"));
    let _term = Terminal::enter().map_err(|e| e.to_string())?;
    let mut last: Vec<String> = Vec::new();
    view.set_settings(Settings::load());
    view.set_os_dark(settings::os_is_dark());
    let mut phone_worker: Option<phone::Worker> = None;
    let (mut settings_read, mut appearance_read) = (Instant::now(), Instant::now());
    loop {
        // The settings file may be changed by another terminal; the machine's appearance by
        // the person, at any time.
        if settings_read.elapsed() >= SETTINGS_EVERY && !view.unsaved() {
            settings_read = Instant::now();
            view.set_settings(Settings::load());
        }
        if view.settings().mode == settings::Mode::System
            && appearance_read.elapsed() >= APPEARANCE_EVERY
        {
            appearance_read = Instant::now();
            view.set_os_dark(settings::os_is_dark());
        }
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
        if view.take_phone_start() {
            phone_worker = Some(phone::Worker::spawn());
        }
        if let Some(worker) = &phone_worker {
            while let Ok(event) = worker.events.try_recv() {
                view.phone_event(event);
            }
            for say in view.take_phone_says() {
                worker.say(say);
            }
            if view.phone_is_off() {
                phone_worker = None;
            }
        }
        if let Some(chosen) = view.take_save() {
            if let Err(e) = chosen.save() {
                view.flash(&format!("no se pudo guardar settings.json: {e}"));
            }
            settings_read = Instant::now();
        }
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
                After::OpenFolder { path, create } => {
                    if create && let Err(e) = std::fs::create_dir(&path) {
                        view.manage_failed("No se pudo crear la carpeta", &e.to_string());
                    } else {
                        let reply = client.cmd(Cmd::OpenFolder {
                            path: path.to_string_lossy().into_owned(),
                        });
                        shown_in_a_pane(
                            &mut view,
                            &mut client,
                            reply,
                            "No se pudo abrir la carpeta",
                        );
                    }
                }
                After::CreateWorktree {
                    repo,
                    branch,
                    from,
                    folder,
                } => {
                    let reply = client.cmd_within(
                        Cmd::CreateWorktree {
                            repo: repo.to_string_lossy().into_owned(),
                            branch,
                            base: Some(from),
                            folder: Some(folder),
                        },
                        ASK_WAIT,
                    );
                    shown_in_a_pane(
                        &mut view,
                        &mut client,
                        reply,
                        "No se pudo crear el worktree",
                    );
                }
                After::Rename { target, name } => {
                    let cmd = match target {
                        Named::Workspace(workspace) => Cmd::RenameWorkspace { workspace, name },
                        Named::Pane(pane) => Cmd::RenamePane { pane, name },
                    };
                    let reply = client.cmd(cmd);
                    told(&mut view, "No se pudo renombrar", "renombrado", reply);
                }
                After::ClosePane(pane) => {
                    let reply = client.cmd(Cmd::ClosePane { pane });
                    told(
                        &mut view,
                        "No se pudo cerrar el panel",
                        "panel cerrado",
                        reply,
                    );
                }
                After::RemoveWorkspace {
                    workspace,
                    remove_worktree,
                } => {
                    let reply = client.cmd(Cmd::RemoveWorkspace {
                        workspace,
                        remove_worktree,
                    });
                    if remove_worktree {
                        told(
                            &mut view,
                            "No se pudo quitar el worktree",
                            "worktree quitado · la rama se queda",
                            reply,
                        );
                    } else {
                        told(
                            &mut view,
                            "No se pudo cerrar el workspace",
                            "workspace cerrado · la carpeta se queda",
                            reply,
                        );
                    }
                }
                After::Check(workspace) => check_changes(&mut view, &mut client, workspace),
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
        assert!(first.starts_with("1   ↳ ● feat/login"), "{first}");
        assert!(
            first[..28].contains('!'),
            "permission is the most urgent: {first}"
        );
        assert!(t.lines().nth(3).unwrap().starts_with("  carpetas"), "{t}");
        assert!(t.lines().nth(4).unwrap().starts_with("2 ● notes"), "{t}");
        assert!(
            !t.contains("sessions"),
            "the sessions of the old jw are not here: {t}"
        );
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
        assert!(
            v.plain(180, 44)
                .lines()
                .last()
                .unwrap()
                .contains("WORKSPACE")
        );
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
        assert!(
            v.plain(120, 30)
                .lines()
                .last()
                .unwrap()
                .contains("WORKSPACE")
        );
        v.press(Key::Enter);
        assert!(v.avisos().len() < before, "opening cleared something");
        assert!(v.plain(120, 30).lines().last().unwrap().contains("PANEL"));
        v.press(Key::Enter);
        assert!(
            v.plain(120, 30)
                .lines()
                .last()
                .unwrap()
                .contains("TERMINAL")
        );
    }

    #[test]
    fn moving_the_highlight_only_previews() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        v.press(Key::Down);
        assert_eq!(v.avisos().len(), before, "a preview marks nothing seen");
        assert!(
            v.plain(120, 30)
                .lines()
                .last()
                .unwrap()
                .contains("WORKSPACE")
        );
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
        // The second workspace is y = 4: `carpetas` is the row between (y = 3), and a click on it does nothing.
        v.click(5, 3, 120, 30);
        assert_eq!(v.selected, 0, "a heading is not a workspace");
        assert!(v.opened.is_none());
        v.click(5, 4, 120, 30);
        assert_eq!(v.selected, 1);
        assert!(v.opened.is_some(), "a click opens, like Enter");
        assert!(v.avisos().len() <= before);
        assert!(v.plain(120, 30).lines().last().unwrap().contains("PANEL"));
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
        assert!(
            v.typing_pane().is_none(),
            "opening a workspace is the panel level"
        );
        v.press(Key::Enter);
        let pane = v.typing_pane().expect("Enter on a panel starts typing");
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
        v.press(Key::Enter);
        assert!(v.typing_pane().is_some());
        assert_eq!(
            v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            After::Continue
        );
        assert!(v.typing_pane().is_none());
        assert!(v.plain(120, 30).lines().last().unwrap().contains("PANEL"));
        // Now `j` moves to the next panel, as at the panel level.
        let focus = v.focus;
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.focus, focus + 1);
        // Enter goes back to typing in that panel.
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
        assert_eq!(
            v.take_resize(120, 30),
            None,
            "the panel level types nothing"
        );
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
        assert_eq!(
            v.press(Key::NewPane),
            After::Continue,
            "not at the workspace level"
        );
        v.press(Key::Enter);
        assert!(matches!(v.press(Key::NewPane), After::NewPane(_)));
    }

    #[test]
    fn back_leaves_avisos_and_then_quits() {
        let mut v = fixture_view();
        v.press(Key::Avisos);
        assert_eq!(v.press(Key::Back), After::Continue);
        assert_eq!(
            v.press(Key::Back),
            After::Continue,
            "esc at the first level goes nowhere"
        );
        assert_eq!(v.press(Key::Quit), After::Quit);
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
    /// `x` at the workspace level: up from the panel level `idle_view` is at, then close.
    fn close_workspace(v: &mut View) -> After {
        v.press(Key::Back);
        tap(v, 'x')
    }

    fn tap(v: &mut View, c: char) -> After {
        v.press_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    /// The first workspace opened, with the keyboard back at the client, and nothing running.
    fn idle_view() -> View {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.typing = false;
        for p in v.panes.values_mut() {
            p.running = None;
        }
        for b in v.blocks.values_mut() {
            for k in &mut b.blocks {
                k.running = false;
            }
        }
        v
    }

    fn uncommitted(clean: bool) -> (String, contract::Diff) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-diff-uncommitted.json")).unwrap(),
        )
        .unwrap();
        let Node::Diff(mut d) = serde_json::from_value(doc["node"].clone()).unwrap() else {
            panic!("not a diff");
        };
        if clean {
            d.files.clear();
        }
        (d.head.id.clone(), d)
    }

    #[test]
    fn x_closes_an_idle_pane_at_once() {
        let mut v = idle_view();
        let pane = v.shown().unwrap().panes[v.focus].id.clone();
        assert_eq!(tap(&mut v, 'x'), After::ClosePane(pane));
        assert!(v.manage.is_none());
    }

    #[test]
    fn x_asks_first_when_a_program_or_a_command_runs() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.typing = false;
        // The first pane runs nvim.
        assert_eq!(tap(&mut v, 'x'), After::Continue);
        assert!(matches!(&v.manage, Some(Manage::Pane { what, .. }) if what == "nvim"));
        let t = v.plain(110, 30);
        assert!(t.contains("nvim sigue corriendo en este panel."), "{t}");
        assert!(t.contains(" PREGUNTA "), "{t}");
        // Anything but y leaves it.
        assert_eq!(tap(&mut v, 'n'), After::Continue);
        assert!(v.manage.is_none());
        // The second runs `cargo watch`, a command of its shell.
        v.press(Key::Right);
        v.typing = false;
        assert_eq!(tap(&mut v, 'x'), After::Continue);
        assert!(matches!(&v.manage, Some(Manage::Pane { what, .. }) if what == "cargo"));
        let pane = v.shown().unwrap().panes[1].id.clone();
        assert_eq!(tap(&mut v, 'y'), After::ClosePane(pane));
        assert!(v.manage.is_none());
    }

    #[test]
    fn x_at_the_workspace_level_is_about_the_workspace_never_a_pane() {
        let mut v = idle_view();
        v.opened = None;
        assert_eq!(v.level(), Level::Workspaces);
        assert_eq!(tap(&mut v, 'x'), After::Continue);
        assert!(
            matches!(v.manage, Some(Manage::Workspace(_))),
            "the workspace is asked about"
        );
    }

    #[test]
    fn the_focus_moves_to_a_neighbour_when_a_pane_goes() {
        let mut v = idle_view();
        v.press(Key::Right);
        assert_eq!(v.focus, 1);
        let ws = v.selected_id().unwrap();
        v.workspaces.get_mut(&ws).unwrap().panes.pop();
        v.clamp();
        assert_eq!(v.focus, 0, "the pane before it");
        v.session.as_mut().unwrap().workspaces.clear();
        v.clamp();
        assert_eq!(v.selected, 0, "no workspace left");
    }

    #[test]
    fn x_closes_a_workspace_without_its_worktree_and_asks_first() {
        let mut v = idle_view();
        assert_eq!(close_workspace(&mut v), After::Continue);
        let t = v.plain(120, 30);
        assert!(t.contains("Se terminan sus 2 paneles."), "{t}");
        assert!(
            t.contains("El worktree y su carpeta se quedan en el disco."),
            "{t}"
        );
        assert!(t.contains("d quitar el worktree"), "{t}");
        let ws = v.selected_id().unwrap();
        assert_eq!(
            tap(&mut v, 'y'),
            After::RemoveWorkspace {
                workspace: ws,
                remove_worktree: false
            }
        );
        assert!(v.manage.is_none());
    }

    #[test]
    fn a_folder_does_not_offer_to_remove_a_worktree() {
        let mut v = idle_view();
        v.press(Key::Back);
        v.press(Key::Down);
        tap(&mut v, 'x');
        let t = v.plain(120, 30);
        assert!(t.contains("La carpeta se queda en el disco."), "{t}");
        assert!(!t.contains("quitar el worktree"), "{t}");
        assert_eq!(tap(&mut v, 'd'), After::Continue, "d does nothing here");
        assert!(matches!(v.manage, Some(Manage::Workspace(_))));
    }

    #[test]
    fn a_worktree_with_changes_lists_them_and_cannot_be_removed() {
        let mut v = idle_view();
        close_workspace(&mut v);
        let ws = v.selected_id().unwrap();
        assert_eq!(tap(&mut v, 'd'), After::Check(ws));
        assert!(matches!(v.manage, Some(Manage::Checking(_))));
        v.check_made("node");
        assert!(v.take_asks().contains(&Ask::Watch("node".into())));
        let (_, diff) = uncommitted(false);
        v.diffs.insert("node".into(), diff);
        v.advance_manage();
        let t = v.plain(120, 30);
        assert!(t.contains("No se puede quitar"), "{t}");
        assert!(t.contains("Tiene 3 archivos sin commitear:"), "{t}");
        assert!(t.contains("no se borra nada a la fuerza"), "{t}");
        assert!(t.contains("c cerrar solo el workspace"), "{t}");
        // `y` does not remove it; `c` closes the workspace alone and lets go of the node.
        assert_eq!(tap(&mut v, 'y'), After::Continue);
        let ws = v.selected_id().unwrap();
        assert_eq!(
            tap(&mut v, 'c'),
            After::RemoveWorkspace {
                workspace: ws,
                remove_worktree: false
            }
        );
        assert_eq!(v.take_asks(), vec![Ask::Close("node".into())]);
    }

    #[test]
    fn a_clean_worktree_is_removed_on_y_and_esc_goes_back() {
        let mut v = idle_view();
        close_workspace(&mut v);
        tap(&mut v, 'd');
        v.check_made("node");
        let (_, diff) = uncommitted(true);
        v.diffs.insert("node".into(), diff);
        v.advance_manage();
        let t = v.plain(120, 30);
        assert!(
            t.contains("No hay cambios sin commitear. La rama feat/login se queda."),
            "{t}"
        );
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(v.manage, Some(Manage::Workspace(_))));
        assert!(v.take_asks().contains(&Ask::Close("node".into())));
        tap(&mut v, 'd');
        v.check_made("node2");
        let (_, diff) = uncommitted(true);
        v.diffs.insert("node2".into(), diff);
        v.advance_manage();
        let ws = v.selected_id().unwrap();
        assert_eq!(
            tap(&mut v, 'y'),
            After::RemoveWorkspace {
                workspace: ws,
                remove_worktree: true
            }
        );
    }

    #[test]
    fn a_refusal_is_shown_and_the_workspace_stays() {
        let mut v = idle_view();
        close_workspace(&mut v);
        tap(&mut v, 'd');
        v.check_made("node");
        v.manage_failed("No se pudo quitar el worktree", "contains modified files");
        let t = v.plain(120, 30);
        assert!(t.contains("No se pudo quitar el worktree"), "{t}");
        assert!(t.contains("contains modified files"), "{t}");
        assert!(t.contains(" ERROR "), "{t}");
        assert_eq!(
            v.take_asks(),
            vec![Ask::Watch("node".into()), Ask::Close("node".into())]
        );
        tap(&mut v, 'q');
        assert!(v.manage.is_none());
        assert!(v.session.is_some());
    }

    #[test]
    fn while_a_question_is_open_keys_and_clicks_do_not_reach_the_panes() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.typing = false;
        close_workspace(&mut v);
        assert_eq!(tap(&mut v, 'j'), After::Continue);
        assert_eq!(v.selected, 0, "the highlight did not move");
        assert_eq!(v.click(5, 3, 110, 30), After::Continue);
        assert!(v.manage.is_some());
    }
    #[test]
    fn comma_opens_the_settings_and_enter_chooses_a_theme_to_save() {
        let mut v = idle_view();
        assert_eq!(tap(&mut v, ','), After::Continue);
        let t = v.plain(120, 30);
        assert!(t.contains("ajustes · tema"), "{t}");
        assert!(t.contains("[sistema]"), "{t}");
        assert!(t.contains("❯ ● One"), "{t}");
        assert!(t.contains(" AJUSTES "), "{t}");
        assert!(v.take_save().is_none(), "opening changes nothing");
        tap(&mut v, 'j');
        tap(&mut v, 'j');
        assert_eq!(v.take_save(), None);
        v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let saved = v.take_save().expect("a choice is written");
        assert_eq!(saved.dark, "tokyo-night");
        assert_eq!(saved.light, "one-light", "the light choice is left alone");
        assert!(v.take_save().is_none(), "once");
        v.plain(120, 30);
        assert_eq!(
            look::bg(),
            contract::themes::get("tokyo-night", true).palette.bg
        );
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(v.manage.is_none());
    }

    #[test]
    fn the_mode_changes_the_colours_the_frame_is_painted_with() {
        let mut v = idle_view();
        tap(&mut v, ',');
        tap(&mut v, 'm');
        assert_eq!(v.settings().mode, settings::Mode::Dark);
        tap(&mut v, 'm');
        assert_eq!(v.settings().mode, settings::Mode::Light);
        assert!(v.unsaved(), "the file must not be read over it");
        let light = contract::themes::get("one-light", false).palette;
        let frame = v.grid(100, 20).rows_ansi().join("");
        let (r, g, b) = light.bg;
        assert!(
            frame.contains(&format!("48;2;{r};{g};{b}m")),
            "the light ground"
        );
        let (r, g, b) = contract::themes::get("one-dark", true).palette.bg;
        assert!(
            !frame.contains(&format!("48;2;{r};{g};{b}m")),
            "no dark ground left"
        );
        tap(&mut v, 'h');
        assert_eq!(v.settings().mode, settings::Mode::Dark);
    }

    #[test]
    fn system_follows_the_machine_until_the_person_picks_a_mode() {
        let mut v = idle_view();
        v.set_os_dark(false);
        v.plain(100, 20);
        assert_eq!(
            look::bg(),
            contract::themes::get("one-light", false).palette.bg
        );
        v.set_os_dark(true);
        v.plain(100, 20);
        assert_eq!(
            look::bg(),
            contract::themes::get("one-dark", true).palette.bg
        );
    }

    #[test]
    fn every_theme_draws_the_frame_with_its_own_ground() {
        let mut v = idle_view();
        for t in contract::themes::all() {
            let mut s = Settings {
                mode: if t.dark {
                    settings::Mode::Dark
                } else {
                    settings::Mode::Light
                },
                ..Settings::default()
            };
            s.choose(true, &t.id);
            v.set_settings(s);
            let rows = v.grid(100, 20).rows_plain();
            assert_eq!(rows.len(), 20, "{}", t.id);
            assert_eq!(look::bg(), t.palette.bg, "{}", t.id);
        }
    }

    #[test]
    fn the_selector_lists_only_the_kind_in_force_and_each_kind_keeps_its_choice() {
        let mut v = idle_view();
        tap(&mut v, ',');
        tap(&mut v, 'm');
        assert_eq!(v.settings().mode, settings::Mode::Dark);
        let t = v.plain(120, 30);
        for name in [
            "One Dark",
            "Catppuccin Mocha",
            "Tokyo Night",
            "Gruvbox Dark",
            "Solarized Dark",
        ] {
            assert!(t.contains(name), "{name}: {t}");
        }
        assert!(t.contains("Temas oscuros"), "{t}");
        assert!(!t.contains("One Light") && !t.contains("Tokyo Day"), "{t}");
        // Pick Gruvbox Dark, go to light mode: only the light ones, and the light choice is
        // still the default.
        for _ in 0..3 {
            tap(&mut v, 'j');
        }
        v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(v.settings().dark, "gruvbox-dark");
        tap(&mut v, 'm');
        assert_eq!(v.settings().mode, settings::Mode::Light);
        let t = v.plain(120, 30);
        assert!(t.contains("Temas claros") && t.contains("Tokyo Day"), "{t}");
        assert!(
            !t.contains("Gruvbox Dark") && !t.contains("One Dark"),
            "{t}"
        );
        assert!(
            t.contains("❯ ● One Light"),
            "the light choice, not the dark one: {t}"
        );
        // Choose in light mode; the dark choice stays.
        tap(&mut v, 'j');
        v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(v.settings().light, "catppuccin-latte");
        assert_eq!(v.settings().dark, "gruvbox-dark");
        // `system` shows the kind the machine is in.
        tap(&mut v, 'm');
        assert_eq!(v.settings().mode, settings::Mode::System);
        v.set_os_dark(false);
        let t = v.plain(120, 30);
        assert!(t.contains("Temas claros (la máquina está en claro)"), "{t}");
        v.set_os_dark(true);
        let t = v.plain(120, 30);
        assert!(
            t.contains("Temas oscuros (la máquina está en oscuro)"),
            "{t}"
        );
        assert!(t.contains("❯ ● Gruvbox Dark"), "{t}");
    }
    #[test]
    fn a_shell_whose_blocks_have_not_arrived_does_not_flash_its_screen() {
        let mut v = idle_view();
        v.press(Key::Enter);
        let pane = v.shown().unwrap().panes[1].id.clone();
        let blocks = v.panes[&pane].blocks.clone().expect("a pane with blocks");
        v.panes.get_mut(&pane).unwrap().screen.lines[0] = "SCREEN-TEXT".into();
        v.blocks.remove(&blocks);
        let t = v.plain(120, 30);
        assert!(t.contains("░░░"), "grey bars stand for the lines: {t}");
        assert!(
            !t.contains("SCREEN-TEXT"),
            "the screen must not show first: {t}"
        );
    }
    fn link() -> phone::Link {
        phone::Link {
            uri: "jwn://192.168.1.20:7871/t0kenabc?fp=ab".into(),
            lan: Some("192.168.1.20:7871".into()),
            tunnel: None,
            pending: false,
        }
    }

    fn ready_view() -> View {
        let mut v = idle_view();
        tap(&mut v, 'm');
        v.phone_event(phone::Event::Ready(link()));
        v
    }

    #[test]
    fn m_opens_the_box_and_asks_the_loop_to_turn_the_link_on_once() {
        let mut v = idle_view();
        assert_eq!(tap(&mut v, 'm'), After::Continue);
        assert!(v.take_phone_start());
        assert!(!v.take_phone_start(), "once");
        let t = v.plain(120, 30);
        assert!(t.contains("Encendiendo el enlace"), "{t}");
        assert!(t.contains(" TELÉFONO "), "{t}");
        // Closing it leaves the link opening; `m` shows it again without a second start.
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!v.phone_open);
        tap(&mut v, 'm');
        assert!(v.phone_open && !v.take_phone_start());
    }

    #[test]
    fn a_ready_link_shows_the_qr_the_addresses_and_that_there_is_no_tunnel() {
        let mut v = ready_view();
        let t = v.plain(140, 50);
        assert!(t.contains("Escanea el código con la app"), "{t}");
        assert!(t.contains("jwn://192.168.1.20:7871/t0kenabc"), "{t}");
        assert!(t.contains("red local   192.168.1.20:7871"), "{t}");
        assert!(t.contains("sin túnel: solo desde tu red"), "{t}");
        assert!(t.contains("ningún teléfono emparejado todavía"), "{t}");
        assert!(t.contains('█') || t.contains('▀'), "the QR: {t}");
        v.phone_event(phone::Event::Paired(2));
        assert!(v.plain(140, 50).contains("2 teléfonos emparejados"));
        // A terminal too small for the QR still shows the link and says so.
        let t = v.plain(100, 24);
        assert!(t.contains("agranda la terminal para ver el QR"), "{t}");
        assert!(t.contains("jwn://192.168.1.20"), "{t}");
    }

    #[test]
    fn esc_leaves_the_link_on_and_the_sidebar_says_so() {
        let mut v = ready_view();
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!v.phone_open && v.phone.is_on());
        assert!(v.plain(120, 30).contains("● teléfono · m"));
        assert!(
            v.take_phone_says().is_empty(),
            "closing the box does not turn it off"
        );
    }

    #[test]
    fn c_copies_the_link_and_s_turns_it_off() {
        let mut v = ready_view();
        assert_eq!(tap(&mut v, 'c'), After::Copy(link().uri));
        assert!(v.phone_open);
        assert_eq!(tap(&mut v, 's'), After::Continue);
        assert_eq!(v.take_phone_says(), vec![phone::Say::Stop]);
        assert!(!v.phone_open && v.phone_is_off());
        assert!(!v.plain(120, 30).contains("● teléfono"));
    }

    #[test]
    fn a_phone_asking_opens_the_box_by_itself_and_only_y_after_a_moment_accepts() {
        let mut v = ready_view();
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        v.set_now(1_000);
        v.phone_event(phone::Event::Ask {
            name: "Pixel 8".into(),
            code: "482 913".into(),
        });
        assert!(v.phone_open, "even though it was closed");
        let t = v.plain(120, 30);
        assert!(t.contains("¿Emparejar «Pixel 8»?"), "{t}");
        assert!(t.contains("482 913"), "{t}");
        assert!(t.contains(" PREGUNTA "), "{t}");
        // A `y` typed before the question could be read is not an answer.
        tap(&mut v, 'y');
        assert!(v.take_phone_says().is_empty());
        v.set_now(2_000);
        tap(&mut v, 'y');
        assert_eq!(v.take_phone_says(), vec![phone::Say::Answer(true)]);
        // Anything else refuses, at once.
        tap(&mut v, 'x');
        assert_eq!(v.take_phone_says(), vec![phone::Say::Answer(false)]);
        v.phone_event(phone::Event::Answered { yes: true });
        assert!(
            matches!(v.phone, phone::State::Ready(_)),
            "back to the link"
        );
        assert_eq!(v.flash, "emparejado «Pixel 8»");
    }

    #[test]
    fn a_question_that_is_open_is_not_answered_by_the_pairing_one() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.typing = false;
        v.phone = phone::State::Ready(link());
        close_workspace(&mut v);
        assert!(v.manage.is_some());
        v.set_now(5_000);
        v.phone_event(phone::Event::Ask {
            name: "Pixel 8".into(),
            code: "111 222".into(),
        });
        assert!(!v.phone_open, "the `y` meant for closing must not pair");
        assert!(v.flash.contains("pide emparejarse"));
        v.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            v.manage.is_none() && v.phone_open,
            "now it is the pairing question's turn"
        );
    }

    #[test]
    fn a_failure_is_a_red_box_with_the_reason_and_any_key_leaves() {
        let mut v = idle_view();
        tap(&mut v, 'm');
        v.phone_event(phone::Event::Failed("address in use".into()));
        let t = v.plain(120, 30);
        assert!(
            t.contains("No se pudo encender el enlace con el teléfono:"),
            "{t}"
        );
        assert!(t.contains("address in use"), "{t}");
        assert!(t.contains(" ERROR "), "{t}");
        tap(&mut v, 'q');
        assert!(!v.phone_open && v.phone_is_off());
        // And `m` tries again.
        tap(&mut v, 'm');
        assert!(v.take_phone_start());
    }

    #[test]
    fn keys_and_clicks_stay_in_the_box_while_it_is_open() {
        let mut v = ready_view();
        assert_eq!(tap(&mut v, 'j'), After::Continue);
        assert_eq!(v.selected, 0);
        assert_eq!(v.click(5, 3, 110, 30), After::Continue);
        assert!(v.phone_open);
    }
    #[test]
    fn enter_goes_in_a_level_and_esc_comes_out_and_the_chip_says_where() {
        let mut v = fixture_view();
        let chip = |v: &View| v.plain(120, 30).lines().last().unwrap().to_owned();
        assert_eq!(v.level(), Level::Workspaces);
        assert!(chip(&v).contains(" WORKSPACE "));
        v.press(Key::Enter);
        assert_eq!(v.level(), Level::Panels);
        assert!(chip(&v).contains(" PANEL "));
        v.press(Key::Enter);
        assert_eq!(v.level(), Level::Terminal);
        assert!(chip(&v).contains(" TERMINAL "));
        // `esc` is the program's at the last level; `^␣` is the way out.
        assert!(matches!(
            v.press_key(plain(KeyCode::Esc)),
            After::Write { .. }
        ));
        assert_eq!(v.level(), Level::Terminal);
        v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(v.level(), Level::Panels);
        v.press_key(plain(KeyCode::Esc));
        assert_eq!(v.level(), Level::Workspaces);
        assert_eq!(
            v.press_key(plain(KeyCode::Esc)),
            After::Continue,
            "nowhere further up"
        );
    }

    #[test]
    fn j_and_k_move_the_workspaces_at_the_first_level_and_the_panels_at_the_second() {
        let mut v = fixture_view();
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!((v.selected, v.level()), (1, Level::Workspaces));
        v.press_key(plain(KeyCode::Char('k')));
        assert_eq!(v.selected, 0);
        v.press_key(plain(KeyCode::Enter));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            (v.selected, v.focus),
            (0, 1),
            "the next panel, not the next workspace"
        );
        v.press_key(plain(KeyCode::Char('h')));
        assert_eq!(v.focus, 0);
        // `h` and `l` do nothing at the first level.
        v.press_key(plain(KeyCode::Esc));
        v.press_key(plain(KeyCode::Char('l')));
        assert_eq!((v.selected, v.level()), (0, Level::Workspaces));
    }

    #[test]
    fn x_closes_what_the_level_is_about() {
        let mut v = idle_view();
        let pane = v.shown().unwrap().panes[v.focus].id.clone();
        assert_eq!(
            tap(&mut v, 'x'),
            After::ClosePane(pane),
            "panel level: the panel"
        );
        v.press(Key::Back);
        tap(&mut v, 'x');
        assert!(
            matches!(v.manage, Some(Manage::Workspace(_))),
            "workspace level: the workspace"
        );
        v.press_key(plain(KeyCode::Esc));
        // `X` is not a key any more.
        assert_eq!(tap(&mut v, 'X'), After::Continue);
        assert!(v.manage.is_none());
    }

    #[test]
    fn n_makes_a_panel_only_at_the_panel_level_and_f_fills_the_area_with_one() {
        let mut v = idle_view();
        assert!(matches!(v.press(Key::NewPane), After::NewPane(_)));
        let area = panes_rect(120, 30);
        let two = v.layout(2, area);
        assert_ne!(two[0], area, "two panels share the area");
        v.press(Key::Full);
        let full = v.layout(2, area);
        assert_eq!(full[v.focus], area);
        assert!(
            full.iter()
                .enumerate()
                .all(|(i, r)| i == v.focus || r.2 == 0)
        );
        let t = v.plain(120, 30);
        assert_eq!(
            t.matches("╭─ zsh").count(),
            1,
            "only the focused panel is drawn: {t}"
        );
        // Back at the workspaces it is not full any more.
        v.press(Key::Back);
        assert!(!v.full);
        assert_eq!(v.layout(2, area), two);
        // And `f` does nothing there.
        v.press(Key::Full);
        assert!(!v.full);
    }

    #[test]
    fn a_click_on_a_row_opens_the_workspace_and_a_click_on_a_panel_starts_typing() {
        let mut v = fixture_view();
        v.click(5, 2, 120, 30);
        assert_eq!(v.level(), Level::Panels);
        v.click(SIDEBAR + 10, 5, 120, 30);
        assert_eq!(v.level(), Level::Terminal);
        // The wheel over the sidebar moves the workspaces whatever the level.
        v.scroll(true, 5, 5, 120, 30);
        assert_eq!((v.selected, v.level()), (1, Level::Workspaces));
    }

    #[test]
    fn question_mark_lists_the_keys_of_the_level_and_any_key_closes_it() {
        let mut v = fixture_view();
        v.press_key(plain(KeyCode::Char('?')));
        let t = v.plain(120, 30);
        assert!(t.contains("teclas · workspace"), "{t}");
        assert!(t.contains("entrar al workspace"), "{t}");
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.selected, 0, "the key only closed the box");
        v.press(Key::Enter);
        v.press_key(plain(KeyCode::Char('?')));
        let t = v.plain(120, 30);
        assert!(
            t.contains("teclas · panel") && t.contains("a pantalla completa"),
            "{t}"
        );
        v.press_key(plain(KeyCode::Esc));
        v.press(Key::Enter);
        v.press_key(plain(KeyCode::Char('?')));
        assert_eq!(v.level(), Level::Terminal, "`?` is typed into the program");
        assert!(!v.help);
    }
    #[test]
    fn a_panel_shown_full_is_not_full_in_the_next_workspace_and_a_click_closes_the_keys() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.press(Key::Full);
        assert!(v.full);
        // Another workspace opened by a click, an attention or `j` + `↵` starts as usual.
        v.click(5, 4, 120, 30);
        assert!(!v.full, "opening another workspace");
        v.full = true;
        v.select_workspace(true);
        assert!(!v.full, "moving the highlight to another workspace");
        v.press(Key::Enter);
        v.press(Key::Back);
        // A click while the box of keys is open closes it and does nothing else.
        v.press_key(plain(KeyCode::Char('?')));
        assert!(v.help);
        v.click(5, 3, 120, 30);
        assert!(!v.help);
        assert_eq!(v.level(), Level::Workspaces);
    }
    #[test]
    fn r_asks_for_the_name_of_the_workspace_at_the_first_level_prefilled_with_what_it_shows() {
        let mut v = fixture_view();
        assert_eq!(tap(&mut v, 'r'), After::Continue);
        let t = v.plain(120, 30);
        assert!(t.contains("renombrar el workspace"), "{t}");
        assert!(
            t.contains("❯ feat/login▏"),
            "a worktree shows its branch: {t}"
        );
        assert!(t.contains(" PREGUNTA "), "{t}");
        // Typing edits it, `backspace` takes a letter away, a chord is no letter.
        for c in ['-', '2'] {
            tap(&mut v, c);
        }
        v.press_key(plain(KeyCode::Backspace));
        v.press_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(v.plain(120, 30).contains("❯ feat/login-▏"));
        let ws = v.selected_id().unwrap();
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Rename {
                target: Named::Workspace(ws),
                name: "feat/login-".into()
            }
        );
        assert!(v.manage.is_none());
    }

    #[test]
    fn r_at_the_panel_level_names_the_panel_and_esc_changes_nothing() {
        let mut v = idle_view();
        let pane = v.shown().unwrap().panes[v.focus].id.clone();
        let title = v.shown().unwrap().panes[v.focus].title.clone();
        tap(&mut v, 'r');
        assert!(v.plain(120, 30).contains("nombrar el panel"));
        assert!(v.plain(120, 30).contains(&format!("❯ {title}▏")));
        for c in "tests".chars() {
            tap(&mut v, c);
        }
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Rename {
                target: Named::Pane(pane),
                name: format!("{title}tests")
            }
        );
        tap(&mut v, 'r');
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.manage.is_none());
        // An empty name is sent as it is: the daemon brings the automatic one back.
        tap(&mut v, 'r');
        for _ in 0..title.chars().count() {
            v.press_key(plain(KeyCode::Backspace));
        }
        assert!(matches!(
            v.press_key(plain(KeyCode::Enter)),
            After::Rename { name, .. } if name.is_empty()
        ));
    }

    #[test]
    fn r_is_the_programs_while_typing_and_the_blocks_while_one_is_selected() {
        let mut v = idle_view();
        v.press(Key::Enter);
        assert!(
            matches!(tap(&mut v, 'r'), After::Write { .. }),
            "typed into the shell"
        );
        assert!(v.manage.is_none());
    }

    #[test]
    fn a_worktree_with_a_name_of_its_own_shows_it_and_the_default_shows_its_branch() {
        let mut v = fixture_view();
        let t = v.plain(120, 30);
        assert!(t.contains("↳ ● feat/login"), "{t}");
        let ws = v.selected_id().unwrap();
        for node in v.workspaces.values_mut() {
            if node.head.id == ws {
                node.name = "login fix".into();
            }
        }
        v.session.as_mut().unwrap().workspaces[0].name = "login fix".into();
        let t = v.plain(120, 30);
        assert!(t.contains("↳ ● login fix"), "{t}");
        assert_eq!(
            shown_name("login fix", WorkspaceKind::Worktree, Some("feat/login")),
            "login fix"
        );
        assert_eq!(
            shown_name(
                "app: feat/login",
                WorkspaceKind::Worktree,
                Some("feat/login")
            ),
            "feat/login"
        );
        assert_eq!(shown_name("x: y", WorkspaceKind::Folder, None), "x: y");
    }
    #[test]
    fn the_link_over_the_network_shows_first_and_the_tunnel_joins_it_when_it_is_up() {
        let mut v = idle_view();
        tap(&mut v, 'm');
        let mut first = link();
        first.pending = true;
        v.phone_event(phone::Event::Ready(first.clone()));
        let t = v.plain(140, 50);
        assert!(t.contains("red local   192.168.1.20:7871"), "{t}");
        assert!(t.contains("túnel       abriendo…"), "{t}");
        assert!(!t.contains("sin túnel"), "it is coming, not missing: {t}");
        // A phone asks meanwhile: the tunnel arriving does not lose the question.
        v.set_now(1_000);
        v.phone_event(phone::Event::Ask {
            name: "Pixel 8".into(),
            code: "123 456".into(),
        });
        let mut up = first.clone();
        up.pending = false;
        up.tunnel = Some("quiet-river.trycloudflare.com".into());
        up.uri = format!("{}&t=quiet-river.trycloudflare.com", first.uri);
        v.phone_event(phone::Event::Ready(up.clone()));
        assert!(
            matches!(&v.phone, phone::State::Asking { link, name, .. } if link.tunnel.is_some() && name == "Pixel 8"),
            "{:?}",
            v.phone
        );
        v.phone_event(phone::Event::Answered { yes: false });
        let t = v.plain(140, 50);
        assert!(
            t.contains("túnel       quiet-river.trycloudflare.com"),
            "{t}"
        );
        assert!(!t.contains("abriendo…"), "{t}");
        assert!(v.phone.is_on());
    }
    /// A folder tree to open from: `root/{app (git, with a worktree), notes}`.
    fn disk(name: &str) -> std::path::PathBuf {
        let root =
            std::path::PathBuf::from(format!("/tmp/jwn-tui-open-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["app", "notes"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(root.join("app"))
                    .args(&args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        root
    }

    /// The fixture view with its two workspaces on that disk: the first (the worktree) of
    /// `app`, the second the folder `notes`.
    fn view_on(root: &std::path::Path) -> View {
        let mut v = fixture_view();
        let s = v.session.as_mut().unwrap();
        s.workspaces[0].path = root.join("app-wt/login").to_string_lossy().into_owned();
        s.workspaces[1].path = root.join("notes").to_string_lossy().into_owned();
        let id = s.workspaces[0].id.clone();
        v.workspaces.get_mut(&id).unwrap().project =
            Some(root.join("app").to_string_lossy().into_owned());
        v
    }

    #[test]
    fn o_opens_the_browser_beside_the_project_and_enter_opens_the_folder_in_a_pane() {
        let root = disk("o");
        let mut v = view_on(&root);
        assert_eq!(tap(&mut v, 'o'), After::Continue);
        let t = v.plain(120, 30);
        assert!(t.contains("abrir una carpeta"), "{t}");
        assert!(t.contains("abrir esta carpeta"), "{t}");
        assert!(
            t.contains("repositorio git") || t.contains("no es un repositorio git"),
            "{t}"
        );
        assert!(t.contains(" ABRIR "), "{t}");
        // It starts beside the project (`app`), so `app/` and `notes/` are in the list.
        assert!(t.contains("app/") && t.contains("notes/"), "{t}");
        // Down to `app/` (git), then Enter: the daemon is asked to open it.
        v.press_key(plain(KeyCode::Down));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::OpenFolder {
                path: root.join("app"),
                create: false
            }
        );
        assert!(v.picker.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_already_open_is_gone_to_and_a_new_name_is_created() {
        let root = disk("o2");
        let mut v = view_on(&root);
        tap(&mut v, 'o');
        // `notes` is the second workspace: Enter on it selects it, asks the daemon nothing.
        v.press_key(plain(KeyCode::Down));
        v.press_key(plain(KeyCode::Down));
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert_eq!(v.selected, 1);
        assert_eq!(v.level(), Level::Panels);
        assert!(v.flash.contains("ya estaba abierta"));
        // A name that is not a folder: its row makes it.
        tap(&mut v, 'o');
        for c in "fresh".chars() {
            tap(&mut v, c);
        }
        v.press_key(plain(KeyCode::Down));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::OpenFolder {
                path: root.join("fresh"),
                create: true
            }
        );
        tap(&mut v, 'o');
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.picker.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn w_makes_a_worktree_of_the_project_and_only_of_a_repository() {
        let root = disk("w");
        let mut v = view_on(&root);
        // The first workspace is a worktree of `app`, on `feat/login`.
        assert_eq!(tap(&mut v, 'w'), After::Continue);
        let t = v.plain(120, 30);
        assert!(t.contains("worktree nuevo de app"), "{t}");
        assert!(t.contains("nombre    ws-1▏"), "{t}");
        assert!(t.contains("rama      feat/ws-1"), "{t}");
        assert!(t.contains("desde     feat/login"), "{t}");
        assert!(t.contains("app-wt/ws-1"), "{t}");
        assert!(t.contains(" WORKTREE "), "{t}");
        // Typing replaces the suggestion, and a bad name says why.
        tap(&mut v, 'X');
        assert!(v.plain(120, 30).contains("nombre no válido"));
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        v.press_key(plain(KeyCode::Backspace));
        for c in "fix".chars() {
            tap(&mut v, c);
        }
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::CreateWorktree {
                repo: root.join("app"),
                branch: "feat/fix".into(),
                from: "feat/login".into(),
                folder: "fix".into()
            }
        );
        assert!(v.worktree.is_none());
        // A folder that is no repository has no worktrees to make.
        v.press(Key::Back);
        v.press(Key::Down);
        tap(&mut v, 'w');
        assert!(v.worktree.is_none());
        assert!(v.flash.contains("no es un repositorio git"), "{}", v.flash);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_workspace_the_client_asked_for_is_opened_when_it_shows_up() {
        let mut v = fixture_view();
        let id = v.session.as_ref().unwrap().workspaces[1].id.clone();
        v.set_now(1_000);
        v.goto(&id);
        v.sync(std::iter::empty());
        assert!(v.goto.is_some(), "no session yet: it waits");
        // The session is there again with it in: it is selected and opened at the panel level.
        let mut v = fixture_view();
        v.set_now(1_000);
        v.goto(&id);
        let ws = v.session.clone().unwrap();
        v.session = Some(ws);
        v.follow_goto();
        assert_eq!((v.selected, v.level()), (1, Level::Panels));
        assert!(v.goto.is_none());
        // One that never comes is given up on.
        let mut v = fixture_view();
        v.set_now(1_000);
        v.goto("nope");
        v.set_now(20_000);
        v.follow_goto();
        assert!(v.goto.is_none());
    }

    #[test]
    fn keys_and_clicks_stay_in_the_browser_and_the_box() {
        let root = disk("guard");
        let mut v = view_on(&root);
        tap(&mut v, 'o');
        assert_eq!(v.click(5, 3, 120, 30), After::Continue);
        assert!(v.picker.is_some());
        assert_eq!(v.scroll(true, 5, 5, 120, 30), After::Continue);
        v.press_key(plain(KeyCode::Esc));
        tap(&mut v, 'w');
        assert_eq!(v.click(5, 3, 120, 30), After::Continue);
        assert!(v.worktree.is_some());
        // `o` and `w` are the program's at the terminal level.
        let mut v = view_on(&root);
        v.press(Key::Enter);
        v.press(Key::Enter);
        assert!(matches!(tap(&mut v, 'o'), After::Write { .. }));
        assert!(matches!(tap(&mut v, 'w'), After::Write { .. }));
        let _ = std::fs::remove_dir_all(root);
    }
}
