//! `jwn tui`: the terminal client, laid out like the jw TUI it replaces (prototype:
//! `docs/prototypes/tui.html`). A sidebar of workspaces with the marks of what waits, a row
//! of facts on top, the open workspace's panes as rounded boxes, a status bar with the mode.
//! `a` lists what waits. It draws the same documents the phone draws and keeps its own seen
//! record. `View` is pure (documents in, a character grid out) so it is tested without a
//! terminal; `run` only moves bytes between the socket, the keyboard and the screen.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use std::io::{Write, stdout};
use std::time::{Duration, Instant};

use contract::{
    Attention, AttentionKind, Blocks, BlocksMode, Cmd, DiffScope, FileKind, FileListing,
    FoundFiles, GitMark, Node, Pane, Program, Session, SplitDir, Workspace, WorkspaceKind,
};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use wire::Client;

mod agents;
mod changes;
mod clip;
mod conversation;
mod diff_lines;
mod field;
mod fileview;
mod fold;
mod look;
mod manage;
mod markdown;
mod older;
mod open;
mod panels;
mod phone;
mod programs;
mod project;
mod prs;
mod select;
mod settings;
mod shell;
mod side;
mod split;
mod tree;
mod viewer;

pub use changes::Ask;
use changes::{Changes, Docs, Outcome, Src};
use contract::ErrorBody;
use conversation::{Act as ConvAct, Ui as ConvUi};
use field::Field;
use manage::{Act, Manage, Named, Target};
use programs::{Launcher, Pick, Programs};
use settings::Settings;
use wire::ClientError;

type Rgb = contract::themes::Rgb;

const SIDEBAR: usize = 28;

/// What the viewer over the panes shows: a document as blocks, or a text file as lines.
enum Viewing {
    Doc(Box<viewer::Viewer>),
    Text(fileview::FileTabs),
}

impl Viewing {
    fn doc(&self) -> Option<&viewer::Viewer> {
        match self {
            Viewing::Doc(v) => Some(v),
            Viewing::Text(_) => None,
        }
    }

    fn doc_mut(&mut self) -> Option<&mut viewer::Viewer> {
        match self {
            Viewing::Doc(v) => Some(v),
            Viewing::Text(_) => None,
        }
    }

    fn scroll_by(&mut self, up: bool, n: usize) {
        match self {
            Viewing::Doc(v) => v.scroll_by(up, n),
            Viewing::Text(v) => v.scroll_by(up, n),
        }
    }

    fn hint(&self) -> String {
        match self {
            Viewing::Doc(v) => v.hint(),
            Viewing::Text(v) => v.hint(),
        }
    }
}

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

    /// Light the cells `x0..x1` of row `y` the way a selection is lit (REQ-308).
    fn light(&mut self, x0: usize, x1: usize, y: usize) {
        let (lit, text) = look::selection();
        self.tint(x0, x1, y, lit, text);
    }

    /// Put `text` on `lit` in the cells `x0..x1` of row `y`, keeping what they say.
    fn tint(&mut self, x0: usize, x1: usize, y: usize, lit: Rgb, text: Rgb) {
        for x in x0..x1 {
            if y < self.rows && x < self.cols {
                let style = &mut self.cells[y * self.cols + x].1;
                style.bg = Some(lit);
                style.fg = Some(text);
                style.reverse = false;
            }
        }
    }

    /// Blank a box on the panel colour, before a popup is drawn over what is under it.
    fn clear_box(&mut self, (x, y, w, h): (usize, usize, usize, usize)) {
        for r in 0..h {
            for c in 0..w {
                if y + r < self.rows && x + c < self.cols {
                    self.cells[(y + r) * self.cols + x + c] =
                        (' ', Style::default().on(look::panel()));
                }
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
pub(crate) const URGENCY: [AttentionKind; 8] = [
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
    /// The menu of programs a new panel can start (`N`).
    NewProgram,
    /// The open pull requests of the project, to review one (`R`).
    Review,
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
    /// The settings of the project the person is in.
    Project,
    /// Push the highlighted worktree's branch.
    Sync,
    /// A new panel beside the focused one, to its right (`|`) or below it (`-`).
    SplitRow,
    SplitCol,
    /// The next panel in the order they were made.
    Next,
    /// Show the file tree, give it the keys, or close it.
    Tree,
    /// Swap the focused panel with the one on this side.
    Swap(split::Side),
    /// Make the focused panel wider or taller (`true`), narrower or lower, by moving a split.
    Resize(SplitDir, bool),
    /// The phone link: its box.
    Phone,
    /// With a block selected: copy its command, copy its output, run it again.
    CopyCmd,
    CopyOut,
    Repeat,
    Back,
    Quit,
}

/// Columns of the file tree, as wide as the sidebar.
const TREE_W: usize = SIDEBAR;
/// How long the daemon is waited for to list a folder or search.
const FILES_WAIT: Duration = Duration::from_secs(30);

/// What the file tree asks the daemon for, with the workspace it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileAsk {
    List {
        workspace: String,
        path: String,
    },
    Find {
        workspace: String,
        query: String,
    },
    /// The files that hold what follows an `@` in a conversation's line.
    Mention {
        workspace: String,
        query: String,
        node: String,
    },
    /// The items of a conversation that came before the time `before` (REQ-567).
    Older {
        node: String,
        before: u64,
    },
    /// The conversations Claude saved for the workspace's folder, for `/resume`.
    Saved {
        workspace: String,
        node: String,
    },
}

/// What the list of pull requests asks the daemon: its content, or the review of one.
pub enum PrsAsk {
    List { workspace: String, requested: bool },
    Review { workspace: String, pr: u64 },
}

/// The daemon's answer to a `PrsAsk`.
pub struct PrsReply {
    ask: PrsAsk,
    reply: Reply,
}

/// The daemon's answer to a `FileAsk`.
pub struct FilesReply {
    ask: FileAsk,
    reply: Reply,
}

/// A drag that began in a pane's list of blocks, with the button still down.
#[derive(Debug)]
struct Drag {
    pane: String,
    /// The cell it began on: a press that never moved is the click it was.
    start: (usize, usize),
    anchor: select::Point,
    /// How wide the lines of the list are, for the text that is copied.
    width: usize,
    moved: bool,
    /// Past the top (`true`) or the bottom of the list, since when: the list scrolls while it is.
    scrolling: Option<(bool, u64)>,
}

/// Where the list of blocks of a pane is on the screen.
struct ListGeom {
    /// Left of the cells of the list, and where its text begins.
    left: usize,
    text_x: usize,
    /// The rows the lines are on, from `top` up to `bottom` (not included).
    top: usize,
    bottom: usize,
    /// The first line shown, and the width the lines are built for.
    start: usize,
    width: usize,
}

/// How often a drag past the edge of a list scrolls it.
const DRAG_SCROLL_MS: u64 = 60;
/// Two presses of the left button on one cell this close (ms) are a double click (REQ-637).
const DOUBLE_CLICK_MS: u64 = 400;

/// Where a new panel goes: it splits this one, along this way.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Place {
    pub beside: Option<String>,
    pub dir: SplitDir,
}

#[derive(PartialEq, Eq, Debug)]
pub enum After {
    Continue,
    Quit,
    /// Ask the daemon for a new pane in this workspace, where `Place` says.
    NewPane(String, Place),
    /// Swap two panes of the open workspace.
    Swap(String, String),
    /// Move the split around a pane so that it gets `by` thousandths more of the way `dir`.
    Grow {
        pane: String,
        dir: SplitDir,
        by: i16,
    },
    /// Ask the daemon for a new pane that starts this program (the program and its arguments).
    NewProgram {
        workspace: String,
        program: Vec<String>,
        place: Place,
    },
    /// Bytes for a pane's shell: what was typed.
    Write {
        pane: String,
        data: String,
    },
    /// Text for the clipboard (OSC 52).
    Copy(String),
    /// Something the person asked of a conversation.
    Conv(ConvAct),
    /// Read the Markdown file `path` of a workspace and show it in the viewer.
    ReadMarkdown {
        workspace: String,
        path: String,
    },
    /// Read the text file `path` of a workspace and show it in the viewer of text (REQ-615);
    /// `size` is what the tree knows of its weight, for the notice of a binary file, and `keep`
    /// reads it again where the person was.
    ReadFile {
        workspace: String,
        path: String,
        size: Option<u64>,
        keep: bool,
    },
    /// Read the plan that waits as `ask` in the conversation `node` and show it in the viewer.
    ReadPlan {
        node: String,
        ask: String,
    },
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
    /// Put `set` in the file `to` of the settings of `repo` and read them again.
    SaveProject {
        repo: std::path::PathBuf,
        to: contract::ConfigFrom,
        set: contract::ProjectEdit,
    },
    /// Make a worktree of `repo` on a new `branch` from `from`, in the folder `folder` of
    /// `<repo>-wt`, and open it with a shell or `claude` as `open` says.
    CreateWorktree {
        repo: std::path::PathBuf,
        branch: String,
        from: String,
        folder: String,
        open: contract::OpenKind,
    },
    /// Open these worktrees of `repo` that exist as workspaces, with no panel.
    OpenWorktrees {
        repo: std::path::PathBuf,
        paths: Vec<String>,
    },
    /// Close a workspace, and with `remove_worktree` remove its worktree. The daemon never
    /// forces: git refuses a worktree with changes.
    RemoveWorkspace {
        workspace: String,
        remove_worktree: bool,
        force: bool,
    },
    /// Read the workspace's uncommitted changes, to know whether its worktree may go.
    Check(String),
    /// Push the worktree's branch.
    Sync {
        workspace: String,
    },
    /// Stop what the daemon runs for this workspace (REQ-604).
    CancelOp {
        workspace: String,
    },
}

pub struct View {
    session: Option<Session>,
    workspaces: HashMap<String, Workspace>,
    panes: HashMap<String, Pane>,
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
    /// The conversation nodes by node id, and what this terminal holds about each.
    convs: HashMap<String, contract::conversation::Conversation>,
    conv_ui: HashMap<String, ConvUi>,
    /// What an answer asked for, to do with the next events.
    then: Option<After>,
    /// The epoch and revision each document held here was read at: what the client holds, and what
    /// the seen record counts from.
    read: HashMap<String, (String, u64)>,
    /// The block of each pane whose program has painted something on the normal screen: once it has, the pane
    /// never says it is starting again for that block, whatever the program clears.
    painted: HashMap<String, u64>,
    /// Each pane whose running program has left the alternate screen: its block, and when this client saw it leave
    /// (the shell's own screen is back under the program until the command ends).
    left_alt: HashMap<String, (u64, u64)>,
    /// The selected block: its pane and its id.
    blk: Option<(String, u64)>,
    /// How many lines each pane's blocks are scrolled up from the newest.
    scroll: HashMap<String, usize>,
    /// What a drag of the mouse covers in a pane's list, and the drag while the button is down.
    selection: Option<select::Selection>,
    drag: Option<Drag>,
    /// Where and when the last press of the left button was, to tell a double click (REQ-637).
    last_press: Option<(usize, usize, u64)>,
    /// The pane whose conversation a drag began in, with the button still down (the drag itself
    /// and what it covers are the conversation's own: `conversation::Ui`).
    conv_drag: Option<String>,
    /// The read-only screen over the panes, when one is open: a Markdown document or a text file.
    viewer: Option<Viewing>,
    /// The tabs of text files that a Markdown view opened from the viewer is over: they come back
    /// when it closes (REQ-640).
    tabs_behind: Option<fileview::FileTabs>,
    /// The file tree: whether its column is shown, whether the keys are in it, and what each
    /// workspace's holds.
    tree_on: bool,
    tree_focus: bool,
    trees: HashMap<String, tree::Tree>,
    file_asks: Vec<FileAsk>,
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
    /// The programs `config.toml` offers a new panel, and the menu over them while it is open.
    programs: Programs,
    launcher: Option<Launcher>,
    /// The folder browser and the worktree box, while one is open.
    picker: Option<open::Picker>,
    worktree: Option<open::Worktree>,
    /// The list of pull requests to review, while it is open, and what it waits to ask (REQ-587).
    prs: Option<prs::Prs>,
    prs_asks: Vec<PrsAsk>,
    /// The diff of a review, beside its agent (REQ-589), and whether it has the focus.
    rv: Option<Changes>,
    rv_focus: bool,
    /// `d` opened the diff beside the panels of every workspace, and `z` made it fill the area
    /// (REQ-590).
    diff_open: bool,
    diff_zoom: bool,
    /// The project screen, while it is open (REQ-520).
    project: Option<project::Project>,
    /// Repositories whose `.jw.toml` the worktree box and the project screen wait to hear of.
    project_asks: Vec<std::path::PathBuf>,
    /// The repositories whose worktrees the box asks the daemon to list.
    worktree_asks: Vec<std::path::PathBuf>,
    /// A workspace made by the client's own request, to open when it shows up in the session
    /// (or at the time given, in `now_ms`, if it does not).
    goto: Option<(String, u64)>,
    /// A panel this client asked for, to focus when it appears in its workspace, and when it was
    /// asked: one that never shows up is forgotten.
    new_pane: Option<(String, u64)>,
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
            convs: HashMap::new(),
            conv_ui: HashMap::new(),
            then: None,
            read: HashMap::new(),
            painted: HashMap::new(),
            left_alt: HashMap::new(),
            blk: None,
            scroll: HashMap::new(),
            selection: None,
            drag: None,
            last_press: None,
            conv_drag: None,
            viewer: None,
            tabs_behind: None,
            tree_on: false,
            tree_focus: false,
            trees: HashMap::new(),
            file_asks: Vec::new(),
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
            programs: Programs::default(),
            launcher: None,
            picker: None,
            worktree: None,
            prs: None,
            prs_asks: Vec::new(),
            rv: None,
            rv_focus: false,
            diff_open: false,
            diff_zoom: false,
            project: None,
            project_asks: Vec::new(),
            worktree_asks: Vec::new(),
            goto: None,
            full: false,
            new_pane: None,
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

    /// `config.toml` was read: the list the menu offers, or why the one it has stays.
    pub fn set_programs(&mut self, read: Result<Vec<programs::Program>, String>) {
        self.programs.apply(read);
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
        if self.size != (cols, rows) {
            self.size = (cols, rows);
            self.sync_review_diff();
        }
    }

    /// The pull request the open workspace reviews, when it does.
    fn review_pr(&self) -> Option<u64> {
        self.shown()?.review.as_ref().map(|r| r.pr)
    }

    /// Whether the area has room for the diff beside the panels: the panels keep two fifths of it
    /// (at least 40 columns) and the diff gets at least 30.
    fn side_room(&self) -> bool {
        let w = self.size.0.saturating_sub(SIDEBAR + 1);
        w >= (w * 2 / 5).max(40) + 30
    }

    /// The columns the diff takes at the right of the open workspace, when it is beside the
    /// panels: the workspace of a review always has it, any other when `d` opened it; three fifths
    /// of the area, all of it when zoomed, and none when the area is too narrow (the diff is then
    /// the screen of `d`).
    fn diff_w(&self) -> usize {
        let wanted = self.review_pr().is_some() || (self.diff_open && self.shown().is_some());
        if !wanted || self.mode != Mode::Workspaces || !self.side_room() {
            return 0;
        }
        let w = self.size.0.saturating_sub(SIDEBAR + 1);
        if self.diff_zoom {
            w
        } else {
            w - (w * 2 / 5).max(40)
        }
    }

    /// Where the diff of a review is drawn.
    fn diff_rect(&self) -> Rect {
        let (cols, rows) = self.size;
        let w = self.diff_w();
        (cols.saturating_sub(w), 1, w, rows.saturating_sub(2))
    }

    /// The diff of a review exists while its workspace is the one shown, in the place it has, and
    /// is let go when another is (its nodes stop being read).
    fn sync_review_diff(&mut self) {
        let id = self.shown().map(|d| d.head.id.clone());
        match id.filter(|_| self.diff_w() > 0) {
            Some(id) => {
                self.tree_on = false;
                self.tree_focus = false;
                if self.rv.as_ref().is_none_or(|c| c.workspace() != id) {
                    if let Some(old) = self.rv.take() {
                        self.asks.extend(old.close());
                    }
                    self.rv = Some(if self.review_pr().is_some() {
                        Changes::open_general(&id)
                    } else {
                        Changes::open(&id)
                    });
                    self.leave_review_diff();
                }
                let area = self.diff_rect();
                let docs = Docs {
                    diffs: &self.diffs,
                    commits: &self.commit_lists,
                };
                if let Some(c) = self.rv.as_mut() {
                    c.set_area(area);
                    c.update(&docs, self.size, self.now_ms);
                }
            }
            None => {
                if let Some(old) = self.rv.take() {
                    self.asks.extend(old.close());
                }
                self.leave_review_diff();
            }
        }
    }

    /// The diff no longer has the keyboard: not as typing that would go to a panel by surprise.
    fn leave_review_diff(&mut self) {
        if self.rv_focus {
            self.typing = false;
        }
        self.rv_focus = false;
    }

    /// The viewer that is open: the screen of `d`, or the diff of a review.
    fn viewer_mut(&mut self) -> Option<&mut Changes> {
        match self.changes.as_mut() {
            Some(c) => Some(c),
            None => self.rv.as_mut(),
        }
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
        if let Some(c) = self.viewer_mut() {
            asks.extend(c.take_asks());
        }
        asks
    }

    /// The daemon made the node the viewer asked for.
    pub fn changes_made(&mut self, src: Src, node: &str) {
        if let Some(c) = self.viewer_mut() {
            c.made(src, node);
        }
    }

    pub fn changes_failed(&mut self, src: Src, why: &str) {
        if let Some(c) = self.viewer_mut() {
            c.failed(src, why);
        }
    }

    pub fn changes_flash(&mut self, text: &str) {
        if let Some(c) = self.viewer_mut() {
            c.flash(text);
        }
    }

    /// A key while the diff of a review has the keyboard: it is the viewer's; the leader and `esc`
    /// give the keyboard back to the client's commands.
    fn rv_key(&mut self, ev: KeyEvent) -> After {
        if is_leader(&ev) || ev.modifiers.contains(KeyModifiers::CONTROL) {
            if is_leader(&ev) {
                self.typing = false;
            }
            return After::Continue;
        }
        match ev.code {
            // `d` closes the diff that `d` opened; in a review, where it stays, it gives the keys back.
            KeyCode::Char('d') => {
                self.typing = false;
                if self.review_pr().is_none() {
                    self.diff_open = false;
                    self.diff_zoom = false;
                    self.rv_focus = false;
                }
                self.sync_review_diff();
                return After::Continue;
            }
            KeyCode::Char('z') => {
                self.diff_zoom = !self.diff_zoom;
                self.sync_review_diff();
                return After::Continue;
            }
            _ => {}
        }
        let docs = Docs {
            diffs: &self.diffs,
            commits: &self.commit_lists,
        };
        let size = self.size;
        let Some(c) = self.rv.as_mut() else {
            return After::Continue;
        };
        let outcome = c.press(ev.code, &docs, size);
        let after = match outcome {
            Outcome::Close => {
                self.typing = false;
                After::Continue
            }
            Outcome::Reference(text) => self.refer(text),
            Outcome::Stay => After::Continue,
        };
        self.sync_review_diff();
        after
    }

    /// Lines of the diff were picked: where they are goes in the field of the agent of the open
    /// workspace, to be asked about, and the keyboard goes there; nothing is sent. With no agent
    /// it is copied (REQ-592).
    fn refer(&mut self, text: String) -> After {
        let Some((at, node)) = self.agent() else {
            self.flash = "no hay un agente aquí: la referencia quedó copiada".into();
            return After::Copy(text);
        };
        self.write_reference(at, node, &text)
    }

    /// The pane of the agent a reference goes to, and its conversation: the focused one of the
    /// open workspace, else the first (REQ-592).
    fn agent(&self) -> Option<(usize, String)> {
        let d = self.shown()?;
        let at = |i: usize| Some((i, self.conv_node(&d.panes.get(i)?.id)?));
        at(self.focus).or_else(|| (0..d.panes.len()).find_map(at))
    }

    /// Put `text` in the field of the agent at pane `at` and give the keyboard to it; nothing is sent.
    fn write_reference(&mut self, at: usize, node: String, text: &str) -> After {
        self.conv_ui.entry(node).or_default().editor.insert(text);
        self.focus = at;
        self.rv_focus = false;
        self.typing = true;
        self.flash = "referencia escrita en el campo del agente; nada se mandó".into();
        After::Continue
    }

    /// Lines `from` to `to` (from 1) of the file `path` of `workspace` were picked in the viewer of
    /// text: the reference is written as the `@` picker writes a path, from the folder the agent
    /// runs in (REQ-631, REQ-639); with no agent it is copied.
    fn refer_lines(&mut self, workspace: &str, path: String, from: usize, to: usize) -> After {
        let range = if to > from {
            format!("#L{from}-{to} ")
        } else {
            format!("#L{from} ")
        };
        let Some((at, node)) = self.agent() else {
            self.flash = "no hay un agente aquí: la referencia quedó copiada".into();
            return After::Copy(format!("@{path}{range}"));
        };
        let path = self
            .written_from_the_conversations_folder(workspace, &node, vec![path])
            .remove(0);
        let after = self.write_reference(at, node, &format!("@{path}{range}"));
        self.flash.push_str(" · esc cierra el visor");
        after
    }

    /// A key while the diff is open: it is the viewer's, never a pane's.
    fn changes_key(&mut self, ev: KeyEvent) -> After {
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return After::Continue;
        }
        let Some(c) = self.changes.as_mut() else {
            return After::Continue;
        };
        let docs = Docs {
            diffs: &self.diffs,
            commits: &self.commit_lists,
        };
        match c.press(ev.code, &docs, self.size) {
            Outcome::Close => self.close_changes(),
            // The screen of `d` has no agent beside it: the reference is copied.
            Outcome::Reference(text) => {
                self.flash = "referencia copiada".into();
                self.update_changes();
                return After::Copy(text);
            }
            Outcome::Stay => self.update_changes(),
        }
        After::Continue
    }

    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_owned();
    }

    /// Bring what is drawn up to date with the documents the client holds (spec REQ-269). Reading a document costs what it weighs, and the blocks of one pane weigh megabytes,
    /// so only a document whose epoch or revision is not the one already read is read again, and what
    /// the client no longer holds goes.
    pub fn sync<'a>(&mut self, nodes: impl Iterator<Item = &'a serde_json::Value>) {
        let mut held: HashSet<&str> = HashSet::new();
        for doc in nodes {
            let Some(id) = doc["id"].as_str() else {
                continue;
            };
            held.insert(id);
            let stamp = (
                doc["epoch"].as_str().unwrap_or_default(),
                doc["rev"].as_u64().unwrap_or_default(),
            );
            if self
                .read
                .get(id)
                .is_some_and(|(epoch, rev)| (epoch.as_str(), *rev) == stamp)
            {
                continue;
            }
            let Ok(node) = Node::deserialize(doc) else {
                continue;
            };
            let id = contract::node_id(&node).clone();
            self.read.insert(id.clone(), (stamp.0.to_owned(), stamp.1));
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
                Node::Conversation(c) => {
                    // The older items that were asked for stay before the node's (REQ-567).
                    let ui = self.conv_ui.entry(id.clone()).or_default();
                    let eff = ui.older.merged(self.convs.get(&id), *c, stamp.0);
                    ui.sync(&eff);
                    self.convs.insert(id, eff);
                }
            }
        }
        let gone: Vec<String> = self
            .read
            .keys()
            .filter(|id| !held.contains(id.as_str()))
            .cloned()
            .collect();
        for id in &gone {
            self.read.remove(id);
            self.workspaces.remove(id);
            self.panes.remove(id);
            self.blocks.remove(id);
            self.convs.remove(id);
            self.conv_ui.remove(id);
            self.diffs.remove(id);
            self.commit_lists.remove(id);
            if self.session.as_ref().is_some_and(|s| s.head.id == *id) {
                self.session = None;
            }
        }
        self.note_painted();
        self.note_left_alt();
        self.update_changes();
        self.sync_review_diff();
        self.advance_manage();
        self.follow_goto();
        self.clamp();
        // The folders of the open workspace's tree are asked for before the tree is shown (REQ-636).
        self.ask_folders();
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
        if let Some((id, asked)) = &self.new_pane {
            let at = self
                .shown()
                .and_then(|d| d.panes.iter().position(|p| p.id == *id));
            if let Some(at) = at {
                self.focus = at;
                self.new_pane = None;
            } else if self.now_ms > asked + NEW_PANE_WAIT_MS {
                self.new_pane = None;
            }
        }
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
        ids.extend(
            self.shown()
                .iter()
                .flat_map(|w| w.panes.iter())
                .filter_map(|p| self.panes.get(&p.id)?.conversation.clone()),
        );
        ids.retain(|id| !self.read.contains_key(id));
        ids
    }

    /// Remember the panes whose running program has painted something (see `painted`).
    fn note_painted(&mut self) {
        self.painted.retain(|pane, _| self.panes.contains_key(pane));
        for (id, p) in &self.panes {
            let Some(b) = blocks_node(&self.blocks, p) else {
                continue;
            };
            if let Some(k) = running_block(b).filter(|k| shell::has_ink(&k.out)) {
                self.painted.insert(id.clone(), k.id);
            }
        }
    }

    /// Remember when a program left the alternate screen and is still running (see `left_alt`).
    fn note_left_alt(&mut self) {
        self.left_alt
            .retain(|pane, _| self.panes.contains_key(pane));
        for (id, p) in &self.panes {
            let leaving = blocks_node(&self.blocks, p)
                .and_then(running_full)
                .filter(|k| k.full && !p.screen.alt);
            match leaving {
                Some(k) => {
                    let since = self.left_alt.get(id).filter(|(block, _)| *block == k.id);
                    let since = since.map_or(self.now_ms, |(_, at)| *at);
                    self.left_alt.insert(id.clone(), (k.id, since));
                }
                None => {
                    self.left_alt.remove(id);
                }
            }
        }
    }

    /// The program that used the alternate screen, left it and has not ended (spec REQ-264): the
    /// pane's screen is the shell's own again, as it was before the program. A program that stays so
    /// for `CLOSING_MAX_MS` is running something else on purpose and its screen is shown.
    fn closing(&self, pane: &str) -> Option<String> {
        let p = self.panes.get(pane)?;
        let k = running_full(blocks_node(&self.blocks, p)?).filter(|k| k.full && !p.screen.alt)?;
        let (block, since) = self.left_alt.get(pane)?;
        (*block == k.id && self.now_ms.saturating_sub(*since) < CLOSING_MAX_MS)
            .then(|| shell::program_name(&k.cmd))
    }

    /// The program a pane's blocks say has the screen and has not painted anything yet (spec
    /// REQ-259): it draws on the normal screen, so the pane's own screen would still be the shell's
    /// as it was. A program that has not painted in `LOADING_MAX_MS` is not waited for.
    fn starting(&self, pane: &str) -> Option<String> {
        let p = self.panes.get(pane)?;
        let k = running_full(blocks_node(&self.blocks, p)?).filter(|k| !k.full)?;
        let waiting = !shell::has_ink(&k.out)
            && self.painted.get(pane) != Some(&k.id)
            && self.now_ms.saturating_sub(k.started) < LOADING_MAX_MS;
        waiting.then(|| shell::program_name(&k.cmd))
    }

    /// The pane's blocks, when it is drawn as blocks (a program with the whole screen, or a
    /// shell without the integration, is drawn as its screen).
    fn blocks_of(&self, pane: &str) -> Option<&Blocks> {
        blocks_node(&self.blocks, self.panes.get(pane)?).filter(|b| b.mode == BlocksMode::Blocks)
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
        let rev = self.read.get(id).map(|(_, rev)| *rev).unwrap_or_else(|| {
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
        self.rv_focus = false;
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
        if self.typing_pane().is_some() || self.rv_typing() {
            Level::Terminal
        } else if self.opened.is_some() && self.opened == self.selected_id() {
            Level::Panels
        } else {
            Level::Workspaces
        }
    }

    /// The rectangles of the panels, in the order of `doc.panes`: those of the daemon's layout, or
    /// the client's own arrangement when the daemon keeps none. While one is shown full, it has
    /// the whole area and the others none.
    fn layout(&self, doc: &Workspace, area: Rect) -> Vec<Rect> {
        let mut rects = match &doc.layout {
            Some(tree) => {
                let at = split::rects(tree, area);
                doc.panes
                    .iter()
                    .map(|p| at.get(p.id.as_str()).copied().unwrap_or((0, 0, 0, 0)))
                    .collect()
            }
            None => pane_rects(doc.panes.len(), area),
        };
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

    /// Where a new panel goes: beside the focused one, along `dir` or, when none is given, along
    /// the longer side of the focused panel.
    fn place(&self, dir: Option<SplitDir>) -> Place {
        let doc = self.shown();
        let beside = doc
            .and_then(|d| d.panes.get(self.focus))
            .map(|p| p.id.clone());
        let dir = dir.unwrap_or_else(|| {
            let (cols, rows) = self.size;
            doc.and_then(|d| {
                self.layout(d, self.panes_space(cols, rows))
                    .get(self.focus)
                    .copied()
            })
            .map_or(SplitDir::Row, split::auto_dir)
        });
        Place { beside, dir }
    }

    /// The place in the open workspace of the panel on this side of the focused one.
    fn neighbour_toward(&self, side: split::Side) -> Option<usize> {
        let doc = self.shown()?;
        let (cols, rows) = self.size;
        let at = split::rects(doc.layout.as_ref()?, self.panes_space(cols, rows));
        let id = split::neighbour(&at, &doc.panes.get(self.focus)?.id, side)?;
        doc.panes.iter().position(|p| p.id == id)
    }

    /// `H` `J` `K` `L`: ask to swap the focused panel with the one on that side.
    fn swap_toward(&mut self, side: split::Side) -> After {
        let Some(doc) = self.shown() else {
            return After::Continue;
        };
        let pair = self.neighbour_toward(side).and_then(|to| {
            Some((
                doc.panes.get(self.focus)?.id.clone(),
                doc.panes.get(to)?.id.clone(),
            ))
        });
        match pair {
            Some((a, b)) => After::Swap(a, b),
            None => {
                self.flash("no hay un panel en esa dirección");
                After::Continue
            }
        }
    }

    /// `<` `>` `{` `}`: ask to move the split around the focused panel by a twentieth.
    fn resize_focused(&mut self, dir: SplitDir, bigger: bool) -> After {
        let Some(pane) = self.shown().and_then(|d| d.panes.get(self.focus)) else {
            return After::Continue;
        };
        After::Grow {
            pane: pane.id.clone(),
            dir,
            by: if bigger { RESIZE_STEP } else { -RESIZE_STEP },
        }
    }

    /// Focus the panel on this side of the focused one, when there is one.
    fn focus_toward(&mut self, side: split::Side) {
        // The diff of a review is the panel at the right of its agent.
        if self.rv.is_some() {
            if self.rv_focus {
                if side == split::Side::Left {
                    self.rv_focus = false;
                }
                return;
            }
            if side == split::Side::Right && self.neighbour_toward(side).is_none() {
                self.rv_focus = true;
                return;
            }
        }
        if self.shown().is_some_and(|d| d.layout.is_none()) {
            self.go_panel(matches!(side, split::Side::Right | split::Side::Down));
        } else if let Some(to) = self.neighbour_toward(side) {
            self.focus = to;
        }
    }

    /// The panel the client just asked for takes the focus as soon as it shows up.
    pub fn focus_new(&mut self, reply: &Reply) {
        if let Ok(Ok(Some(r))) = reply {
            self.new_pane = r["pane"].as_str().map(|id| (id.to_owned(), self.now_ms));
        }
    }

    /// Focus the next or the previous panel of the open workspace.
    fn go_panel(&mut self, forward: bool) {
        let Some(id) = self.opened.clone() else {
            return;
        };
        let panes = self.workspaces.get(&id).map_or(0, |d| d.panes.len());
        if self.rv.is_some() {
            if self.rv_focus {
                self.rv_focus = self.rv_focus && forward;
                return;
            }
            if forward && self.focus + 1 >= panes {
                self.rv_focus = true;
                return;
            }
        }
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

    /// The accent of the conversation the keyboard is in, and whether it asks something.
    fn focused_conversation(&self) -> Option<(Rgb, bool)> {
        let pane = self.typing_pane()?;
        let conv = self.conv_of(self.panes.get(&pane)?)?;
        Some((conversation::accent_rgb(conv.accent), conv.ask.is_some()))
    }

    /// The keyboard is in the diff of a review.
    fn rv_typing(&self) -> bool {
        self.typing && self.rv_focus && self.rv.is_some() && self.mode == Mode::Workspaces
    }

    /// The pane the keyboard goes to: the focused pane of the opened workspace, while typing.
    fn typing_pane(&self) -> Option<String> {
        if !self.typing
            || self.mode != Mode::Workspaces
            || self.changes.is_some()
            || self.rv_focus && self.rv.is_some()
        {
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
        let last = running_block(blocks_node(&self.blocks, p)?)?;
        Some(clip(&shell::program_name(&last.cmd), 24))
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

    /// `R`: the open pull requests of the project the person is in.
    fn open_prs(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        self.typing = false;
        self.prs = Some(prs::Prs::new(id.clone()));
        self.prs_asks.push(PrsAsk::List {
            workspace: id,
            requested: false,
        });
    }

    fn prs_key(&mut self, ev: &KeyEvent) -> After {
        let Some(p) = self.prs.as_mut() else {
            return After::Continue;
        };
        let workspace = p.workspace.clone();
        match p.press(ev) {
            prs::Act::Nothing => {}
            prs::Act::Close => self.prs = None,
            prs::Act::Reload { requested } => {
                p.loading();
                self.prs_asks.push(PrsAsk::List {
                    workspace,
                    requested,
                });
            }
            prs::Act::Review(pr) => {
                p.making(pr, "trayendo la rama y haciendo el worktree…");
                self.prs_asks.push(PrsAsk::Review { workspace, pr });
            }
            prs::Act::Cancel => return After::CancelOp { workspace },
            prs::Act::Browse(url) => {
                self.flash = match open_url(&url) {
                    Ok(()) => format!("abriendo {url} en el navegador"),
                    Err(why) => format!("no se pudo abrir el navegador: {why}"),
                };
            }
        }
        After::Continue
    }

    /// What the list of pull requests waits to ask, once.
    pub fn take_prs_asks(&mut self) -> Vec<PrsAsk> {
        std::mem::take(&mut self.prs_asks)
    }

    /// The daemon's answer to the list or to the review: the list shows the pull requests, or is
    /// closed and the status bar says why `gh` could not; a review that was made is opened.
    pub fn prs_replied(&mut self, answer: PrsReply) {
        match (answer.ask, reply_value(answer.reply)) {
            (PrsAsk::List { workspace, .. }, reply) => {
                let Some(p) = self.prs.as_mut().filter(|p| p.workspace == workspace) else {
                    return;
                };
                match reply.and_then(|v| {
                    serde_json::from_value::<contract::PrList>(v).map_err(|e| e.to_string())
                }) {
                    Ok(list) => p.set_list(Ok(list.prs)),
                    Err(why) => {
                        self.prs = None;
                        self.flash = format!("no se pudo listar los PR: {why}");
                    }
                }
            }
            (PrsAsk::Review { workspace, .. }, Ok(made)) => {
                if self.prs.as_ref().is_some_and(|p| p.workspace == workspace) {
                    self.prs = None;
                }
                if let Some(id) = made["workspace"].as_str() {
                    self.goto(id);
                }
                self.flash = if made["dirty"].as_bool() == Some(true) {
                    "esa review tiene cambios tuyos: se dejó como estaba".into()
                } else if made["existed"].as_bool() == Some(true) {
                    "la review ya existía: traje la última versión del PR".into()
                } else {
                    "review lista: el agente espera tus preguntas".into()
                };
            }
            (PrsAsk::Review { .. }, Err(why)) => {
                if let Some(p) = self.prs.as_mut() {
                    p.stop_making();
                }
                self.flash = format!("no se hizo nada: {why}");
            }
        }
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
        self.project_asks.push(repo.clone());
        self.worktree = Some(open::Worktree::new(
            repo,
            from.unwrap_or_else(|| "HEAD".into()),
            self.open_paths(),
        ));
    }

    /// The repositories whose settings the worktree box asks the daemon for, once.
    pub fn take_project_asks(&mut self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut self.project_asks)
    }

    /// The repositories whose worktrees the box wants listed.
    pub fn take_worktree_asks(&mut self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut self.worktree_asks)
    }

    /// The daemon's answer to `list_worktrees` for `repo`, for the box if it is still open on it.
    pub fn worktrees_replied(&mut self, repo: &std::path::Path, reply: Reply) {
        let list = reply_value(reply).and_then(|v| {
            serde_json::from_value::<Vec<contract::WorktreeInfo>>(v["worktrees"].clone())
                .map_err(|e| e.to_string())
        });
        if let Some(t) = self.worktree.as_mut().filter(|t| t.repo == repo) {
            t.set_existing(list);
        }
    }

    /// What opening worktrees did: `opened` of them, and why the others did not. The box says it
    /// and lists them again, to show which are open now.
    pub fn worktrees_opened(&mut self, repo: &std::path::Path, opened: usize, failed: &[String]) {
        let Some(t) = self.worktree.as_mut().filter(|t| t.repo == repo) else {
            return;
        };
        let said = match (opened, failed.first()) {
            (0, Some(why)) => (why.clone(), false),
            (_, Some(why)) => (
                format!("abiertos {opened}; {} no: {why}", failed.len()),
                false,
            ),
            (1, None) => ("abierto, sin setup ni copiar archivos".to_owned(), true),
            (n, None) => (format!("abiertos {n}, sin setup ni copiar archivos"), true),
        };
        t.say(said.0, said.1);
        self.worktree_asks.push(repo.to_path_buf());
    }

    /// The daemon's answer to `project_config` for `repo`: it goes to the worktree box if that
    /// is still open and for the same repository.
    pub fn project_replied(&mut self, repo: &std::path::Path, reply: Reply) {
        let info = reply_value(reply).and_then(|v| {
            serde_json::from_value::<contract::ProjectInfo>(v).map_err(|e| e.to_string())
        });
        if let Some(p) = self.project.as_mut().filter(|p| p.repo == repo) {
            p.set_info(info);
        } else if let Some(t) = self.worktree.as_mut().filter(|t| t.repo == repo) {
            t.set_config(info);
        }
    }

    /// `p`: the settings of the project the person is in; only a repository has them.
    fn open_project_box(&mut self) {
        let Some((name, project, kind, _)) = self.project_of_selected() else {
            return;
        };
        let top = match kind {
            WorkspaceKind::Worktree => Some(project),
            _ => open::git_out(&project, &["rev-parse", "--show-toplevel"])
                .map(std::path::PathBuf::from),
        };
        let Some(repo) = top else {
            self.flash = format!("«{name}» no es un repositorio git: p necesita uno");
            return;
        };
        self.project_asks.push(repo.clone());
        self.project = Some(project::Project::new(repo));
    }

    /// What saving the settings answered: it is told on the screen, which reads them again.
    pub fn project_saved(&mut self, repo: &std::path::Path, reply: Reply) {
        let Some(p) = self.project.as_mut().filter(|p| p.repo == repo) else {
            return;
        };
        let saved = reply_value(reply).map(|v| {
            (
                v["path"].as_str().unwrap_or_default().to_owned(),
                v["shadowed"].as_bool().unwrap_or(false),
            )
        });
        // A save that failed keeps what was typed; one that worked reads the settings again.
        if saved.is_ok() {
            self.project_asks.push(repo.to_path_buf());
        }
        p.saved(saved);
    }

    /// A key in the project screen.
    fn project_key(&mut self, ev: KeyEvent) -> After {
        let Some(p) = self.project.as_mut() else {
            return After::Continue;
        };
        match p.press(ev.code) {
            project::Act::Stay => After::Continue,
            project::Act::Close => {
                self.project = None;
                After::Continue
            }
            project::Act::Save { repo, to, set } => After::SaveProject { repo, to, set },
        }
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
        let made = t.press(ev.code, ev.modifiers.contains(KeyModifiers::CONTROL));
        if t.wants_list() {
            self.worktree_asks.push(t.repo.clone());
        }
        match made {
            open::Make::Stay => After::Continue,
            open::Make::Close => {
                self.worktree = None;
                After::Continue
            }
            open::Make::OpenExisting { repo, paths } => After::OpenWorktrees { repo, paths },
            open::Make::Create {
                repo,
                branch,
                from,
                folder,
                open,
            } => {
                self.worktree = None;
                After::CreateWorktree {
                    repo,
                    branch,
                    from,
                    folder,
                    open,
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

    /// `r`: ask for a new name for the highlighted worktree (a folder is a directory on disk and
    /// is not renamed), or for the focused panel.
    fn ask_rename(&mut self, level: Level) {
        let (target, title, text) = match level {
            Level::Workspaces => {
                let Some(w) = self
                    .session
                    .as_ref()
                    .and_then(|s| s.workspaces.get(self.selected))
                    .filter(|w| w.kind == WorkspaceKind::Worktree)
                else {
                    return;
                };
                (
                    Named::Workspace(w.id.clone()),
                    "renombrar el worktree".to_owned(),
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

    /// `s`: push the highlighted worktree's branch; a folder has none of its own to push.
    fn ask_sync(&mut self) -> After {
        let Some((id, kind, branch)) = self
            .session
            .as_ref()
            .and_then(|s| s.workspaces.get(self.selected))
            .map(|w| (w.id.clone(), w.kind, w.branch.clone()))
        else {
            return After::Continue;
        };
        let Some(branch) = branch.filter(|_| kind == WorkspaceKind::Worktree) else {
            let name = self.label(&id);
            self.flash(&format!("«{name}» es una carpeta: s sube un worktree"));
            return After::Continue;
        };
        self.manage = Some(Manage::Syncing {
            workspace: id.clone(),
            branch,
            cancelling: false,
        });
        After::Sync { workspace: id }
    }

    /// The daemon answered the push: say what went up, or why it did not.
    pub fn sync_done(&mut self, reply: Reply) {
        let Some(Manage::Syncing {
            branch, cancelling, ..
        }) = self.manage.take_if(|m| matches!(m, Manage::Syncing { .. }))
        else {
            return;
        };
        match reply {
            // It was stopped because the person said so: not a failure to explain.
            Ok(Err(e)) if cancelling && e.message == "cancelado" => {
                self.flash(&format!("subida de {branch} cancelada"));
            }
            Ok(Ok(result)) => self.flash(&sync_said(&branch, result.as_ref())),
            Ok(Err(e)) => self.manage_failed(&format!("No se pudo subir {branch}"), &e.message),
            Err(e) => self.manage_failed(&format!("No se pudo subir {branch}"), &e.to_string()),
        }
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
            unpushed: 0,
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
            Some(Act::Cancel(workspace)) => After::CancelOp { workspace },
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
                force,
            }) => After::RemoveWorkspace {
                workspace,
                remove_worktree,
                force,
            },
        }
    }

    /// A key while the menu of programs is open: it moves, picks or cancels, and is never a pane's.
    fn launcher_key(&mut self, ev: KeyEvent) -> After {
        // A chord is not a letter of the menu: it answers nothing.
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return After::Continue;
        }
        let Some(menu) = self.launcher.take() else {
            return After::Continue;
        };
        let (next, pick) = menu.press(ev.code, &self.programs);
        self.launcher = next;
        self.started(pick)
    }

    /// What the menu started, in the workspace the keys are on.
    fn started(&mut self, pick: Option<Pick>) -> After {
        let (Some(pick), Some(workspace)) = (pick, self.selected_id()) else {
            return After::Continue;
        };
        match pick {
            Pick::Shell => After::NewPane(workspace, self.place(None)),
            Pick::Program(program) => After::NewProgram {
                workspace,
                program,
                place: self.place(None),
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

    /// How many commits of the worktree's branch no remote has, for the question to say they stay.
    pub fn check_unpushed(&mut self, n: u64) {
        if let Some(Manage::Checking(target)) = &mut self.manage {
            target.unpushed = n;
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
        self.selection = None;
        self.unlight_conversations();
        if self.help {
            self.help = false;
            return After::Continue;
        }
        if self.viewer.is_some() {
            if self.tree_on && self.tree_focus {
                return self.tree_key(ev);
            }
            return self.viewer_key(ev);
        }
        if self.prs.is_some() {
            return self.prs_key(&ev);
        }
        if self.picker.is_some() {
            return self.picker_key(ev);
        }
        if self.project.is_some() {
            return self.project_key(ev);
        }
        if self.worktree.is_some() {
            return self.worktree_key(ev);
        }
        if self.launcher.is_some() {
            return self.launcher_key(ev);
        }
        if self.phone_open {
            return self.phone_key(ev);
        }
        if self.manage.is_some() {
            return self.manage_key(ev);
        }
        if self.changes.is_some() {
            return self.changes_key(ev);
        }
        if self.rv_typing() {
            return self.rv_key(ev);
        }
        if self.tree_focus && self.tree_on {
            return self.tree_key(ev);
        }
        let Some(pane) = self.typing_pane() else {
            return key_of(ev.code).map_or(After::Continue, |k| self.press(k));
        };
        if is_leader(&ev) {
            self.typing = false;
            return After::Continue;
        }
        if let Some(node) = self.conv_node(&pane) {
            return self.conv_key(&node, &ev);
        }
        match bytes_of(&ev) {
            Some(data) => After::Write { pane, data },
            None => After::Continue,
        }
    }

    /// A key for the conversation of the pane the keyboard is in: `^V` reads a picture here, the
    /// rest is the conversation's own.
    fn conv_key(&mut self, node: &str, ev: &KeyEvent) -> After {
        let Some(conv) = self.convs.get(node) else {
            return After::Continue;
        };
        let ui = self.conv_ui.entry(node.to_owned()).or_default();
        if let Some(p) = self.panes.get(&conv.pane) {
            ui.context(&p.cwd);
        }
        if ev.modifiers.contains(KeyModifiers::CONTROL)
            && ev.code == KeyCode::Char('v')
            && conv.ask.is_none()
            && !ui.has_pop()
        {
            match clip::image() {
                Ok(image) => ui.attach(image),
                Err(e) => self.flash = e,
            }
            return After::Continue;
        }
        let act = ui.key(conv, ev);
        if let Some(says) = ui.take_says() {
            self.flash = says;
        }
        self.ask_mentions(node);
        self.ask_saved(node);
        self.conv_after(act)
    }

    /// The conversations `/resume` lists, asked of the daemon once.
    fn ask_saved(&mut self, node: &str) {
        let Some(workspace) = self.opened.clone() else {
            return;
        };
        if self
            .conv_ui
            .get_mut(node)
            .is_some_and(ConvUi::take_saved_ask)
        {
            self.file_asks.push(FileAsk::Saved {
                workspace,
                node: node.to_owned(),
            });
        }
    }

    /// Paths of the workspace as the conversation's engine reads them: from the folder it runs in
    /// (REQ-631). Unchanged while the node does not say where it runs.
    fn written_from_the_conversations_folder(
        &self,
        workspace: &str,
        node: &str,
        paths: Vec<String>,
    ) -> Vec<String> {
        let root = self
            .session
            .as_ref()
            .and_then(|s| s.workspaces.iter().find(|w| w.id == workspace))
            .map(|w| w.path.as_str());
        match (root, self.convs.get(node).and_then(|c| c.cwd.as_deref())) {
            (Some(root), Some(cwd)) => paths
                .iter()
                .map(|p| shell::from_folder(root, cwd, p))
                .collect(),
            _ => paths,
        }
    }

    /// The files an `@` in the conversation's line calls for, asked once for each fragment.
    fn ask_mentions(&mut self, node: &str) {
        let Some(workspace) = self.opened.clone() else {
            return;
        };
        if let Some(query) = self.conv_ui.get_mut(node).and_then(ConvUi::want_files) {
            self.file_asks.push(FileAsk::Mention {
                workspace,
                query,
                node: node.to_owned(),
            });
        }
    }

    fn conv_after(&mut self, act: Option<ConvAct>) -> After {
        match act {
            Some(ConvAct::Copy(text)) => After::Copy(text),
            Some(ConvAct::Open(path)) => match self.opened.clone() {
                Some(workspace) => self.open_memory_file(workspace, &path.to_string_lossy()),
                None => After::Continue,
            },
            Some(ConvAct::ViewPlan { node, ask }) => After::ReadPlan { node, ask },
            Some(act) => After::Conv(act),
            None => After::Continue,
        }
    }

    /// Whether a box of the client is open over the panes, so that what is typed goes to it.
    fn modal(&self) -> bool {
        self.help
            || self.picker.is_some()
            || self.worktree.is_some()
            || self.project.is_some()
            || self.launcher.is_some()
            || self.phone_open
            || self.manage.is_some()
            || self.changes.is_some()
            || (self.tree_focus && self.tree_on)
    }

    /// Text pasted (the terminal says it is a paste and not keys): into the conversation's line,
    /// or typed into whatever has the keyboard, a line break being Enter.
    pub fn paste(&mut self, text: &str) -> After {
        self.flash.clear();
        if self.viewer.is_some() {
            return After::Continue;
        }
        if self.modal() {
            // What a key would have done, the first that asks the client for something ending the paste.
            for c in text.chars().filter(|c| *c != '\r') {
                let code = if c == '\n' {
                    KeyCode::Enter
                } else {
                    KeyCode::Char(c)
                };
                let after = self.press_key(KeyEvent::new(code, KeyModifiers::NONE));
                if after != After::Continue {
                    return after;
                }
            }
            return After::Continue;
        }
        let Some(pane) = self.typing_pane() else {
            return After::Continue;
        };
        if let Some(node) = self.conv_node(&pane) {
            self.conv_ui.entry(node.clone()).or_default().paste(text);
            self.ask_mentions(&node);
            return After::Continue;
        }
        After::Write {
            pane,
            data: text.replace("\r\n", "\r").replace('\n', "\r"),
        }
    }

    /// The size the typing pane's box has, when it differs from what was last asked: a shell
    /// should wrap where its box does.
    pub fn take_resize(&mut self, cols: usize, rows: usize) -> Option<(String, u16, u16)> {
        let pane = self.typing_pane()?;
        let doc = self.workspaces.get(self.opened.as_deref()?)?;
        let (_, _, w, h) = *self
            .layout(doc, self.panes_space(cols, rows))
            .get(self.focus)?;
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
            (Mode::Workspaces, Key::Next) if level == Level::Panels => self.go_panel(true),
            (Mode::Workspaces, Key::Tree) if level != Level::Terminal => self.toggle_tree(),
            (Mode::Workspaces, Key::Swap(side)) if level == Level::Panels => {
                return self.swap_toward(side);
            }
            (Mode::Workspaces, Key::Resize(dir, bigger)) if level == Level::Panels => {
                return self.resize_focused(dir, bigger);
            }
            (Mode::Workspaces, Key::Left) if level == Level::Panels => {
                self.focus_toward(split::Side::Left)
            }
            (Mode::Workspaces, Key::Right) if level == Level::Panels => {
                self.focus_toward(split::Side::Right)
            }
            (Mode::Workspaces, Key::Up) if level == Level::Panels => {
                self.focus_toward(split::Side::Up)
            }
            (Mode::Workspaces, Key::Down) if level == Level::Panels => {
                self.focus_toward(split::Side::Down)
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
                    self.typing = panes > 0 || self.rv.is_some();
                }
                Level::Terminal => {}
            },
            (Mode::Workspaces, Key::Full) if level == Level::Panels => self.full = !self.full,
            (Mode::Workspaces, Key::Repeat) if level != Level::Terminal => self.ask_rename(level),
            (Mode::Workspaces, Key::CopyOut) if level != Level::Terminal => self.open_picker(),
            (Mode::Workspaces, Key::Sync) if level != Level::Terminal => return self.ask_sync(),
            (Mode::Workspaces, Key::Worktree) if level != Level::Terminal => {
                self.open_worktree_box()
            }
            (Mode::Workspaces, Key::Project) if level != Level::Terminal => self.open_project_box(),
            (Mode::Workspaces, Key::Review) if level != Level::Terminal => self.open_prs(),
            (Mode::Workspaces, Key::Close) => match level {
                Level::Workspaces => self.ask_close_workspace(),
                Level::Panels => return self.ask_close_pane(),
                Level::Terminal => {}
            },
            // In a review the diff is already beside the agent: `d` gives it the keys.
            (Mode::Workspaces, Key::Changes) if self.rv.is_some() => {
                self.opened = self.selected_id();
                self.rv_focus = true;
                self.typing = true;
            }
            // Elsewhere it opens beside the panels when there is room, and as a screen when not.
            (Mode::Workspaces, Key::Changes)
                if self.side_room() && self.selected_id().is_some() =>
            {
                self.opened = self.selected_id();
                self.blk = None;
                self.diff_open = true;
                self.sync_review_diff();
                self.rv_focus = true;
                self.typing = true;
            }
            (Mode::Workspaces, Key::Changes) => {
                if let Some(id) = self.selected_id() {
                    self.typing = false;
                    self.blk = None;
                    self.changes = Some(Changes::open(&id));
                }
            }
            (Mode::Workspaces, Key::NewPane | Key::SplitRow | Key::SplitCol)
                if level == Level::Panels =>
            {
                if let Some(id) = self.selected_id() {
                    let dir = match key {
                        Key::SplitRow => Some(SplitDir::Row),
                        Key::SplitCol => Some(SplitDir::Col),
                        _ => None,
                    };
                    // A daemon from before layouts would put the panel where it likes, not where
                    // `|` or `-` says: better to say so than to leave it below.
                    if dir.is_some()
                        && self
                            .shown()
                            .is_some_and(|d| !d.panes.is_empty() && d.layout.is_none())
                    {
                        self.flash("el daemon es anterior al layout: jwn stop y abre jwn otra vez");
                        return After::Continue;
                    }
                    return After::NewPane(id, self.place(dir));
                }
            }
            (Mode::Workspaces, Key::NewProgram) if level == Level::Panels => {
                if self.selected_id().is_some() {
                    self.launcher = Some(Launcher::default());
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
        // The mark of a restart is no block to select.
        let next = if forward {
            (at + 1..b.blocks.len()).find(|i| !b.blocks[*i].restart)
        } else {
            (0..at).rev().find(|i| !b.blocks[*i].restart)
        };
        self.blk = Some((pane, b.blocks[next.unwrap_or(at)].id));
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
        if let Some(node) = self.conv_node(pane) {
            return self.conv_click(&node, rect, x, y);
        }
        let Some(b) = self.blocks_of(pane) else {
            return After::Continue;
        };
        let (ix, _, iw, _) = inner(rect);
        let Some(g) = self.list_geom(pane, rect) else {
            return After::Continue;
        };
        let on_marks = y == g.bottom + Field::FIRST_TEXT;
        let running = running_block(b).is_some();
        if (g.top..g.bottom).contains(&y) && (ix..ix + iw).contains(&x) {
            let line = shell::window_lines(&b.blocks, self.now_ms, g.width, g.start + y - g.top, 1);
            if let Some(id) = line.first().and_then(|l| l.block) {
                self.blk = Some((pane.to_owned(), id));
                self.typing = false;
            }
        } else if on_marks
            && running
            && (field::stop_at(ix, iw)..field::stop_at(ix, iw) + STOP.chars().count()).contains(&x)
        {
            return After::Write {
                pane: pane.to_owned(),
                data: "\x03".into(),
            };
        } else if on_marks && x == field::send_at(ix, iw) {
            return After::Write {
                pane: pane.to_owned(),
                data: "\r".into(),
            };
        }
        After::Continue
    }

    /// Let go of what a drag covers in every conversation.
    fn unlight_conversations(&mut self) {
        self.conv_drag = None;
        for ui in self.conv_ui.values_mut() {
            ui.unlight();
        }
    }

    /// The conversation a drag is held in and the rectangle of its pane, while there is one.
    fn conv_drag_at(&mut self) -> Option<(String, Rect)> {
        let pane = self.conv_drag.clone()?;
        let (cols, rows) = self.size;
        let found = self.conv_node(&pane).zip(self.pane_rect(&pane, cols, rows));
        if found.is_none() {
            self.conv_drag = None;
        }
        found
    }

    /// The button was let go in a conversation: a click, or the text a drag covered to copy.
    fn release_conversation(&mut self, pane: &str, cols: usize, rows: usize) -> After {
        let Some((node, rect)) = self.conv_node(pane).zip(self.pane_rect(pane, cols, rows)) else {
            return After::Continue;
        };
        let room = self.conv_inner(&node, rect);
        let released = match (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
            (Some(conv), Some(ui)) => ui.release(conv, room),
            _ => return After::Continue,
        };
        if let Some(says) = self.conv_ui.get_mut(&node).and_then(ConvUi::take_says) {
            self.flash = says;
        }
        match released {
            conversation::Released::Click(x, y) => self.click(x, y, cols, rows),
            conversation::Released::Copy(text) => After::Copy(text),
            conversation::Released::Nothing => After::Continue,
        }
    }

    // ---- the Markdown viewer (REQ-469 to REQ-478) ----

    /// Show a document in the viewer; the keys and the mouse are its until it closes.
    pub fn open_viewer(
        &mut self,
        doc: contract::markdown::MarkdownDoc,
        source: viewer::Source,
        workspace: String,
    ) {
        // A plan is answered the ways the question's box has.
        let answers = match &source {
            viewer::Source::Plan { node, ask } => self
                .convs
                .get(node)
                .and_then(|c| c.ask.as_ref())
                .filter(|a| a.id == *ask)
                .map(|a| {
                    a.options
                        .iter()
                        .map(|o| (o.key.clone(), o.label.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            viewer::Source::File => Vec::new(),
        };
        if let Some(Viewing::Text(tabs)) = self.viewer.take() {
            self.tabs_behind = Some(tabs);
        }
        self.tree_focus = false;
        self.viewer = Some(Viewing::Doc(Box::new(viewer::Viewer::new(
            doc, source, answers, workspace,
        ))));
    }

    /// Close the viewer: the tabs of text files a Markdown view was over come back, or nothing is
    /// shown (REQ-640).
    fn close_viewer(&mut self) {
        self.viewer = self.tabs_behind.take().map(Viewing::Text);
    }

    /// Put `view` in the tabs of the viewer: the ones that show, the ones a Markdown view is over,
    /// or new ones.
    fn open_tab(&mut self, view: fileview::FileView, keep: bool) {
        let tabs = match self.viewer.take() {
            Some(Viewing::Text(tabs)) => Some(tabs),
            Some(Viewing::Doc(_)) | None => self.tabs_behind.take(),
        };
        let tabs = match tabs {
            Some(mut tabs) => {
                tabs.open(view, keep);
                tabs
            }
            None => fileview::FileTabs::new(view),
        };
        self.viewer = Some(Viewing::Text(tabs));
        // The keys are the viewer's, whoever had them when the file came.
        self.tree_focus = false;
    }

    /// Show a text file in the viewer of text; `keep` leaves the person where they were reading
    /// when it is the same file read again (REQ-616).
    pub fn open_text(
        &mut self,
        workspace: String,
        rel: String,
        doc: contract::text::TextDoc,
        keep: bool,
    ) {
        self.open_tab(fileview::FileView::new(workspace, doc).at(rel), keep);
    }

    /// Show why the daemon would not read a file: it is binary (REQ-613).
    pub fn open_refused(&mut self, workspace: String, path: String, size: Option<u64>) {
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let weight = size.map_or_else(String::new, |n| format!(" · {}", fileview::size_text(n)));
        let why = format!("binario{weight} · no se puede mostrar");
        self.open_tab(
            fileview::FileView::refused(workspace, path, name, why),
            false,
        );
    }

    /// Whether the plan the viewer shows still waits for an answer.
    fn viewer_plan_waits(&self) -> bool {
        match self
            .viewer
            .as_ref()
            .and_then(Viewing::doc)
            .map(|v| &v.source)
        {
            Some(viewer::Source::Plan { node, ask }) => self
                .convs
                .get(node)
                .and_then(|c| c.ask.as_ref())
                .is_some_and(|a| a.id == *ask),
            _ => false,
        }
    }

    fn viewer_act(&mut self, act: viewer::Act) -> After {
        match act {
            viewer::Act::Nothing => After::Continue,
            viewer::Act::Close => {
                self.close_viewer();
                After::Continue
            }
            viewer::Act::Copy(text) => After::Copy(text),
            viewer::Act::Source(path) => {
                let workspace = self
                    .viewer
                    .as_ref()
                    .and_then(Viewing::doc)
                    .map(|v| v.workspace.clone())
                    .unwrap_or_default();
                After::ReadFile {
                    workspace,
                    path,
                    size: None,
                    keep: false,
                }
            }
            viewer::Act::Answer(key) => {
                let Some(viewer::Source::Plan { node, ask }) = self
                    .viewer
                    .as_ref()
                    .and_then(Viewing::doc)
                    .map(|v| v.source.clone())
                else {
                    return After::Continue;
                };
                if !self.viewer_plan_waits() {
                    self.flash("ese plan ya no espera respuesta");
                    return After::Continue;
                }
                self.close_viewer();
                // Keeping planning takes an instruction: the box asks for it (REQ-595).
                if key == "no" {
                    self.conv_ui.entry(node).or_default().refuse(&ask);
                    return After::Continue;
                }
                After::Conv(ConvAct::Answer {
                    node,
                    ask,
                    key,
                    text: None,
                })
            }
        }
    }

    fn viewer_key(&mut self, ev: KeyEvent) -> After {
        let (cols, rows) = self.size;
        let rect = self.panes_space(cols, rows);
        match self.viewer.as_mut() {
            Some(Viewing::Doc(v)) => {
                let act = v.key(&ev, rect);
                if let Some(says) = v.take_says() {
                    self.flash = says;
                }
                self.viewer_act(act)
            }
            Some(Viewing::Text(v)) => {
                let act = v.key(&ev, rect);
                if let Some(says) = v.take_says() {
                    self.flash = says;
                }
                self.text_act(act)
            }
            None => After::Continue,
        }
    }

    /// What a key of the viewer of text asked of the client.
    fn text_act(&mut self, act: fileview::Act) -> After {
        let Some(Viewing::Text(tabs)) = self.viewer.as_ref() else {
            return After::Continue;
        };
        let v = tabs.current();
        let workspace = v.workspace.clone();
        let path = v.path().to_owned();
        let rel = v.rel().to_owned();
        match act {
            fileview::Act::Nothing => After::Continue,
            fileview::Act::Refer { path, from, to } => self.refer_lines(&workspace, path, from, to),
            fileview::Act::Close => {
                self.close_viewer();
                After::Continue
            }
            fileview::Act::Tree => {
                self.tree_on = true;
                self.tree_focus = true;
                self.typing = false;
                self.ask_folders();
                After::Continue
            }
            fileview::Act::Copy(text) => After::Copy(text),
            fileview::Act::Reread => After::ReadFile {
                workspace,
                path: rel,
                size: None,
                keep: true,
            },
            fileview::Act::Rendered => After::ReadMarkdown { workspace, path },
        }
    }

    /// The button was let go over the viewer: the click it was, or the text a drag covered.
    fn viewer_release(&mut self, cols: usize, rows: usize) -> After {
        let rect = self.panes_space(cols, rows);
        if let Some(Viewing::Text(t)) = self.viewer.as_mut() {
            let copy = t.release();
            if let Some(says) = t.take_says() {
                self.flash = says;
            }
            return copy.map_or(After::Continue, After::Copy);
        }
        let Some(v) = self.viewer.as_mut().and_then(Viewing::doc_mut) else {
            return After::Continue;
        };
        let released = v.release(rect);
        if let Some(says) = v.take_says() {
            self.flash = says;
        }
        match released {
            viewer::Released::Click(x, y) => self.click(x, y, cols, rows),
            viewer::Released::Copy(text) => After::Copy(text),
            viewer::Released::Nothing => After::Continue,
        }
    }

    /// A click in the conversation of a pane: its title, an item, the field, the footer, the box that asks.
    fn conv_click(&mut self, node: &str, rect: Rect, x: usize, y: usize) -> After {
        let Some(conv) = self.convs.get(node) else {
            return After::Continue;
        };
        let ui = self.conv_ui.entry(node.to_owned()).or_default();
        if let Some(p) = self.panes.get(&conv.pane) {
            ui.context(&p.cwd);
        }
        if ui.click_title(conv, rect, x, y) {
            return After::Continue;
        }
        let full = inner(rect);
        let (room, area) = agents::split(full, ui.agents.fits(full.2));
        // A click in the area of the agents lights a task; it is not the conversation's.
        if let Some((ax, ay, ..)) = area
            && x >= ax
        {
            ui.agents.click(conv, ay, y);
            return After::Continue;
        }
        let act = ui.click(conv, room, x, y);
        if let Some(says) = ui.take_says() {
            self.flash = says;
        }
        self.ask_saved(node);
        self.conv_after(act)
    }

    /// A left click at cell (`x`, `y`) of a `cols` × `rows` screen. A workspace row opens it,
    /// a pane's box focuses it, `avisos N` in the status bar opens the list, an item of the
    /// list goes there, and a click outside the list closes it.
    pub fn click(&mut self, x: usize, y: usize, cols: usize, rows: usize) -> After {
        if self.help {
            self.help = false;
            return After::Continue;
        }
        if self.viewer.is_some() {
            let rect = self.panes_space(cols, rows);
            let act = self
                .viewer
                .as_mut()
                .and_then(Viewing::doc_mut)
                .map_or(viewer::Act::Nothing, |v| v.click(rect, x, y));
            return self.viewer_act(act);
        }
        if let Some(menu) = self.launcher {
            let px = SIDEBAR + 1;
            let area = (px, 1, cols.saturating_sub(px), rows.saturating_sub(2));
            let (next, pick) = menu.click(area, &self.programs, x, y);
            self.launcher = next;
            return self.started(pick);
        }
        if self.manage.is_some()
            || self.phone_open
            || self.picker.is_some()
            || self.worktree.is_some()
            || self.project.is_some()
        {
            return After::Continue;
        }
        if self.rv.is_some() && self.changes.is_none() {
            if x >= self.diff_rect().0 {
                self.opened = self.selected_id();
                self.rv_focus = true;
                self.typing = true;
                let docs = Docs {
                    diffs: &self.diffs,
                    commits: &self.commit_lists,
                };
                let size = self.size;
                if let Some(c) = self.rv.as_mut() {
                    c.click(x, y, &docs, size);
                }
                self.sync_review_diff();
                return After::Continue;
            }
            self.rv_focus = false;
        }
        if self.changes.is_none() {
            if self.tree_on && x + TREE_W >= cols {
                return self.tree_click(y, rows);
            }
            self.tree_focus = false;
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
            let rect = avisos_rect(waiting, self.panes_space(cols, rows));
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
            let rects = self.layout(doc, self.panes_space(cols, rows));
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
        if let Some(v) = self.viewer.as_mut() {
            v.scroll_by(!down, 3);
            return After::Continue;
        }
        if self.help
            || self.manage.is_some()
            || self.launcher.is_some()
            || self.phone_open
            || self.picker.is_some()
            || self.worktree.is_some()
            || self.project.is_some()
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
        if self.rv.is_some() && x >= self.diff_rect().0 {
            let size = self.size;
            if let Some(c) = self.rv.as_mut() {
                c.wheel(down, x, size);
            }
            return After::Continue;
        }
        if self.tree_on && x + TREE_W >= cols {
            let room = Self::tree_room(rows);
            if let Some((_, t)) = self.tree_mut() {
                let most = t.rows().len().saturating_sub(room);
                t.scroll = if down {
                    (t.scroll + SCROLL_STEP).min(most)
                } else {
                    t.scroll.saturating_sub(SCROLL_STEP)
                };
            }
            return After::Continue;
        }
        if x < SIDEBAR {
            self.select_workspace(down);
            return After::Continue;
        }
        let Some(doc) = self.shown() else {
            return After::Continue;
        };
        let rects = self.layout(doc, self.panes_space(cols, rows));
        let Some(i) = rects.iter().position(|r| inside(*r, x, y)) else {
            return After::Continue;
        };
        let pane = doc.panes[i].id.clone();
        self.scroll_pane(&pane, rects[i], !down, SCROLL_STEP);
        After::Continue
    }

    /// How many columns the file tree takes from the panels' area.
    fn tree_w(&self) -> usize {
        if self.tree_on { TREE_W } else { 0 }
    }

    /// The area the panels share, less the file tree's column when it is shown.
    fn panes_space(&self, cols: usize, rows: usize) -> Rect {
        let (x, y, w, h) = panes_rect(cols, rows);
        (x, y, w.saturating_sub(self.tree_w()), h)
    }

    /// Rows of the file tree that show entries: under its title and above its legend.
    fn tree_room(rows: usize) -> usize {
        rows.saturating_sub(6)
    }

    fn tree_mut(&mut self) -> Option<(String, &mut tree::Tree)> {
        let ws = self.selected_id()?;
        let t = self.trees.entry(ws.clone()).or_default();
        Some((ws, t))
    }

    /// Ask for the folders of the open workspace's tree that nobody has asked for.
    fn ask_folders(&mut self) {
        let Some((workspace, t)) = self.tree_mut() else {
            return;
        };
        let asks: Vec<FileAsk> = t
            .wanted()
            .into_iter()
            .map(|path| FileAsk::List {
                workspace: workspace.clone(),
                path,
            })
            .collect();
        self.file_asks.extend(asks);
    }

    /// Ask again for what the tree already holds, as it was left: what is listed stays on the screen
    /// until the answer arrives.
    fn refresh_folders(&mut self) {
        let Some((workspace, t)) = self.tree_mut() else {
            return;
        };
        let asks: Vec<FileAsk> = t
            .stale()
            .into_iter()
            .map(|path| FileAsk::List {
                workspace: workspace.clone(),
                path,
            })
            .collect();
        self.file_asks.extend(asks);
    }

    /// `t`: show the file tree and give it the keys; with it shown, the keys; with them, close it.
    fn toggle_tree(&mut self) {
        if self.rv.is_some() {
            self.flash = "la review ya tiene su diff al lado".into();
            return;
        }
        if self.tree_on && self.tree_focus {
            self.tree_on = false;
            self.tree_focus = false;
        } else {
            self.tree_on = true;
            self.tree_focus = true;
            self.typing = false;
            self.ask_folders();
            self.refresh_folders();
        }
    }

    /// What the tree asks the daemon for, once.
    pub fn take_file_asks(&mut self) -> Vec<FileAsk> {
        std::mem::take(&mut self.file_asks)
    }

    /// The daemon listed the saved conversations: the list of `/resume` fills, or says why not.
    fn saved_replied(&mut self, node: &str, reply: Reply) {
        let list = reply_value(reply).and_then(|v| {
            serde_json::from_value::<contract::conversation::ConversationList>(v)
                .map_err(|e| e.to_string())
        });
        let (Some(conv), Some(ui)) = (self.convs.get(node), self.conv_ui.get_mut(node)) else {
            return;
        };
        match list {
            Ok(l) => {
                let act = ui.listed(&l.conversations, conv);
                if let Some(says) = ui.take_says() {
                    self.flash = says;
                }
                self.then = Some(self.conv_after(act));
            }
            Err(why) => {
                ui.list_failed(&format!("no se pudo listar las conversaciones: {why}"));
                if let Some(says) = ui.take_says() {
                    self.flash = says;
                }
            }
        }
    }

    /// What an answer that came on its own asks the client to do (`/continue` taking the newest).
    pub fn take_then(&mut self) -> Option<After> {
        self.then.take().filter(|a| *a != After::Continue)
    }

    /// The daemon answered a folder or a search.
    pub fn files_replied(&mut self, r: FilesReply) {
        let FilesReply { ask, reply } = r;
        if let FileAsk::Mention {
            workspace,
            query,
            node,
        } = &ask
        {
            let found = reply_value(reply)
                .and_then(|v| serde_json::from_value::<FoundFiles>(v).map_err(|e| e.to_string()));
            if let Ok(f) = found {
                let paths = self.written_from_the_conversations_folder(workspace, node, f.paths);
                if let Some(ui) = self.conv_ui.get_mut(node) {
                    ui.found(query, paths);
                }
            }
            return;
        }
        if let FileAsk::Saved { node, .. } = &ask {
            self.saved_replied(node, reply);
            return;
        }
        if let FileAsk::Older { node, before } = &ask {
            self.older_replied(node, *before, reply);
            return;
        }
        let (FileAsk::List { workspace, .. }
        | FileAsk::Find { workspace, .. }
        | FileAsk::Mention { workspace, .. }
        | FileAsk::Saved { workspace, .. }) = &ask
        else {
            return;
        };
        let Some(t) = self.trees.get_mut(workspace) else {
            return;
        };
        let value = reply_value(reply);
        match ask {
            FileAsk::List { path, .. } => {
                let dir = match value.and_then(|v| {
                    serde_json::from_value::<FileListing>(v).map_err(|e| e.to_string())
                }) {
                    Ok(l) => tree::Dir::Ready {
                        entries: l.entries,
                        total: l.total,
                    },
                    Err(why) => tree::Dir::Failed(why),
                };
                t.listed(&path, dir);
            }
            FileAsk::Find { query, .. } => {
                // Only the answer for what is typed now counts.
                if t.filter.as_deref() == Some(query.as_str())
                    && let Ok(f) = value.and_then(|v| {
                        serde_json::from_value::<FoundFiles>(v).map_err(|e| e.to_string())
                    })
                {
                    t.found = Some(tree::Found {
                        query,
                        paths: f.paths,
                        cut: f.cut,
                    });
                    t.sel = 0;
                    t.scroll = 0;
                }
            }
            // Answered above.
            FileAsk::Mention { .. } | FileAsk::Saved { .. } | FileAsk::Older { .. } => {}
        }
    }

    /// The conversations that are at the top of their list and have older items to ask for ask
    /// for them (REQ-567).
    fn ask_older(&mut self) {
        for (id, ui) in &mut self.conv_ui {
            if let Some(conv) = self.convs.get(id)
                && let Some(before) = ui.older.ask(conv)
            {
                self.file_asks.push(FileAsk::Older {
                    node: id.clone(),
                    before,
                });
            }
        }
    }

    /// The daemon sent the older items asked for with `before`, or why not: they go before the
    /// others, and what is held by the number of an item moves with them.
    fn older_replied(&mut self, node: &str, before: u64, reply: Reply) {
        let result = reply_value(reply)
            .ok()
            .and_then(|v| serde_json::from_value::<contract::conversation::OlderItems>(v).ok());
        let (Some(conv), Some(ui)) = (self.convs.get_mut(node), self.conv_ui.get_mut(node)) else {
            return;
        };
        if let Some(added) = ui.older.replied(before, result, conv)
            && added > 0
        {
            ui.shifted(added);
        }
    }

    /// Leave the tree's keys, as opening a file does.
    fn leave_tree(&mut self) {
        self.tree_focus = false;
        self.typing = false;
        if let Some((_, t)) = self.tree_mut() {
            t.filter = None;
            t.found = None;
        }
    }

    /// Open a file of the tree: a `.md` in the Markdown viewer, any other in the viewer of text
    /// (REQ-615).
    fn open_file(&mut self, path: &str, size: Option<u64>) -> After {
        let Some(workspace) = self.selected_id() else {
            return After::Continue;
        };
        self.leave_tree();
        if is_markdown(path) {
            return After::ReadMarkdown {
                workspace,
                path: path.to_owned(),
            };
        }
        After::ReadFile {
            workspace,
            path: path.to_owned(),
            size,
            keep: false,
        }
    }

    /// A file of a `/memory` row: in the viewer when it is in the workspace, a `.md` as Markdown;
    /// outside it the daemon does not read it (OPEN-17), so its path is copied (REQ-641).
    fn open_memory_file(&mut self, workspace: String, path: &str) -> After {
        let root = self
            .session
            .as_ref()
            .and_then(|s| s.workspaces.iter().find(|w| w.id == workspace))
            .map(|w| w.path.trim_end_matches('/').to_owned());
        let inside = root.is_some_and(|r| path.starts_with(&format!("{r}/")));
        if !inside {
            self.flash = "ese archivo está fuera del workspace: su ruta quedó copiada".into();
            return After::Copy(path.to_owned());
        }
        if is_markdown(path) {
            return After::ReadMarkdown {
                workspace,
                path: path.to_owned(),
            };
        }
        After::ReadFile {
            workspace,
            path: path.to_owned(),
            size: None,
            keep: false,
        }
    }

    /// A key while the keys are in the file tree.
    fn tree_key(&mut self, ev: KeyEvent) -> After {
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            return After::Continue;
        }
        let rows_shown = self.size.1;
        let Some((workspace, t)) = self.tree_mut() else {
            return After::Continue;
        };
        let rows = t.rows();
        let at = rows.get(t.sel).filter(|r| !r.note).cloned();
        let mut after = After::Continue;
        let mut ask_find: Option<String> = None;
        let mut reread = false;
        let mut flash = None;
        if let Some(text) = t.filter.clone() {
            match ev.code {
                KeyCode::Esc => {
                    t.filter = None;
                    t.found = None;
                    t.sel = 0;
                }
                KeyCode::Enter => {
                    if let Some(r) = at {
                        return self.open_file(&r.path, r.size);
                    }
                }
                KeyCode::Down => t.sel += 1,
                KeyCode::Up => t.sel = t.sel.saturating_sub(1),
                KeyCode::Backspace => {
                    let mut text = text;
                    text.pop();
                    ask_find = Some(text.clone());
                    t.filter = Some(text);
                }
                KeyCode::Char(c) => {
                    let text = format!("{text}{c}");
                    ask_find = Some(text.clone());
                    t.filter = Some(text);
                }
                _ => {}
            }
        } else {
            match ev.code {
                KeyCode::Char('j') | KeyCode::Down => t.sel += 1,
                KeyCode::Char('k') | KeyCode::Up => t.sel = t.sel.saturating_sub(1),
                KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                    if let Some(r) = at {
                        if r.kind == FileKind::Dir {
                            t.toggle(&r.path);
                        } else {
                            return self.open_file(&r.path, r.size);
                        }
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    if let Some(r) = at {
                        if r.kind == FileKind::Dir && r.open {
                            t.toggle(&r.path);
                        } else if let Some(i) = rows
                            .iter()
                            .position(|q| q.path == tree::Tree::parent_of(&r.path) && !q.note)
                        {
                            t.sel = i;
                        }
                    }
                }
                KeyCode::Char('.') => {
                    t.hidden = !t.hidden;
                    t.sel = 0;
                    flash = Some(if t.hidden {
                        "archivos ocultos visibles"
                    } else {
                        "archivos ocultos escondidos"
                    });
                }
                KeyCode::Char('/') => {
                    t.filter = Some(String::new());
                    t.found = None;
                    t.sel = 0;
                }
                KeyCode::Char('y') => {
                    if let Some(r) = at {
                        flash = Some("ruta copiada");
                        after = After::Copy(r.path);
                    }
                }
                KeyCode::Char('r') => reread = true,
                KeyCode::Char('t') => {
                    self.tree_on = false;
                    self.tree_focus = false;
                    return After::Continue;
                }
                KeyCode::Esc => {
                    self.tree_focus = false;
                    return After::Continue;
                }
                _ => {}
            }
        }
        if reread {
            t.forget();
        }
        let len = t.rows().len();
        t.clamp(len, Self::tree_room(rows_shown));
        if let Some(query) = ask_find {
            if query.trim().is_empty() {
                t.found = None;
            } else {
                self.file_asks.push(FileAsk::Find { workspace, query });
            }
        }
        self.ask_folders();
        if let Some(f) = flash {
            self.flash(f);
        }
        if reread {
            self.flash("leído de nuevo del PC");
        }
        after
    }

    /// A click in the file tree's column: a row is lit, a folder opens or closes, a file opens.
    fn tree_click(&mut self, y: usize, rows: usize) -> After {
        self.tree_focus = true;
        self.typing = false;
        if !(2..rows.saturating_sub(4)).contains(&y) {
            return After::Continue;
        }
        let Some((_, t)) = self.tree_mut() else {
            return After::Continue;
        };
        let at = t.scroll + y - 2;
        let Some(r) = t.rows().get(at).filter(|r| !r.note).cloned() else {
            return After::Continue;
        };
        t.sel = at;
        if r.kind == FileKind::Dir {
            t.toggle(&r.path);
            self.ask_folders();
            After::Continue
        } else {
            self.open_file(&r.path, r.size)
        }
    }

    /// The rows of the file tree on the screen, and what git says of each, at the right of the panels.
    fn draw_tree(&self, g: &mut Grid, cols: usize, rows: usize) {
        let Some(t) = self.selected_id().and_then(|ws| self.trees.get(&ws)) else {
            return;
        };
        let x0 = cols.saturating_sub(TREE_W);
        let dim = Style::fg(look::dim());
        let divider = if self.tree_focus {
            look::blue()
        } else {
            look::line()
        };
        for y in 0..rows.saturating_sub(1) {
            g.put(x0, y, "│", Style::fg(divider));
        }
        g.put(x0 + 2, 0, "archivos", dim.bold());
        match &t.filter {
            Some(text) => g.put(
                x0 + 2,
                1,
                &clip(&format!("/ {text}▏"), TREE_W - 3),
                Style::fg(look::yellow()).bold(),
            ),
            None if t.hidden => g.put(x0 + 2, 1, "ocultos visibles", dim),
            None => {}
        }
        let all = t.rows();
        let room = Self::tree_room(rows);
        for (k, r) in all.iter().skip(t.scroll).take(room).enumerate() {
            let y = 2 + k;
            let on = t.scroll + k == t.sel;
            if on && !r.note {
                g.paint_bg(x0 + 1, y, TREE_W - 1, look::sel());
            }
            let lead = x0 + 2 + r.depth * 2;
            if r.note {
                g.put_over(lead, y, &clip(&r.name, TREE_W - 4 - r.depth * 2), dim);
                continue;
            }
            let (glyph, glyph_style) = match r.kind {
                FileKind::Dir if r.open => ("▾", Style::fg(look::blue())),
                FileKind::Dir => ("▸", Style::fg(look::blue())),
                FileKind::Link => ("↗", dim),
                FileKind::File => ("·", dim),
            };
            g.put_over(lead, y, glyph, glyph_style);
            let mut style = if r.ignored {
                dim
            } else {
                Style::fg(look::fg())
            };
            if on {
                style = if self.tree_focus {
                    Style::fg(look::blue()).bold()
                } else {
                    style.bold()
                };
            }
            let name = if r.kind == FileKind::Dir {
                format!("{}/", r.name)
            } else {
                r.name.clone()
            };
            g.put_over(
                lead + 2,
                y,
                &clip(&name, TREE_W.saturating_sub(9 + r.depth * 2)),
                style,
            );
            let (mark, mark_style) = match (r.git, r.ignored) {
                (Some(GitMark::Conflict), _) => ("!", Style::fg(look::red()).bold()),
                (Some(GitMark::Modified), _) => ("M", Style::fg(look::yellow()).bold()),
                (Some(GitMark::Deleted), _) => ("D", Style::fg(look::red()).bold()),
                (Some(GitMark::Added), _) => ("A", Style::fg(look::green()).bold()),
                (Some(GitMark::Untracked), _) => ("?", Style::fg(look::green()).bold()),
                (None, true) => ("◌", dim),
                (None, false) => ("", dim),
            };
            g.put_over(cols.saturating_sub(3), y, mark, mark_style);
        }
        if all.is_empty() {
            let text = if t.filter.is_some() {
                "escribe para buscar"
            } else {
                "carpeta vacía"
            };
            g.put(x0 + 2, 2, text, dim);
        }
        g.put(x0 + 2, rows.saturating_sub(4), "M cambiado · ? nuevo", dim);
        g.put(x0 + 2, rows.saturating_sub(3), "◌ ignorado por git", dim);
        let count = match &t.filter {
            Some(_) => format!(
                "{} {}",
                all.len(),
                if all.len() == 1 {
                    "coincidencia"
                } else {
                    "coincidencias"
                }
            ),
            None => format!("{} / {}", (t.sel + 1).min(all.len()), all.len()),
        };
        g.put(x0 + 2, rows.saturating_sub(2), &count, dim);
    }

    /// The rectangle of a pane of the open workspace on a `cols` × `rows` screen.
    fn pane_rect(&self, pane: &str, cols: usize, rows: usize) -> Option<Rect> {
        let doc = self.shown()?;
        let at = doc.panes.iter().position(|p| p.id == pane)?;
        let rect = *self.layout(doc, self.panes_space(cols, rows)).get(at)?;
        (rect.2 > 0).then_some(rect)
    }

    fn list_geom(&self, pane: &str, rect: Rect) -> Option<ListGeom> {
        let b = self.blocks_of(pane)?;
        let (ix, iy, iw, ih) = inner(rect);
        let doc = self.panes.get(pane)?;
        let lh = list_h(ih, field_of(doc, b, iw).band_rows());
        let total = shell::total_lines(&b.blocks);
        Some(ListGeom {
            left: ix,
            text_x: ix + 2,
            top: iy + gap(total, lh),
            bottom: iy + lh,
            start: window_start(total, lh, self.scroll.get(pane).copied().unwrap_or(0)),
            width: iw.saturating_sub(2),
        })
    }

    /// A press of the left button at (`x`, `y`): `true` when it is the second on the same cell within
    /// `DOUBLE_CLICK_MS` of the first (REQ-637). A third press begins a count again.
    pub fn double_press(&mut self, x: usize, y: usize) -> bool {
        let double = self.last_press.is_some_and(|(lx, ly, at)| {
            (lx, ly) == (x, y) && self.now_ms.saturating_sub(at) <= DOUBLE_CLICK_MS
        });
        self.last_press = (!double).then_some((x, y, self.now_ms));
        double
    }

    /// The press that began a drag was the second of a double click: the drag covers the word it
    /// is on, so the release copies it (REQ-637).
    pub fn widen_to_word(&mut self) {
        if self.viewer.is_some() {
            let (cols, rows) = self.size;
            let rect = self.panes_space(cols, rows);
            if let Some(v) = self.viewer.as_mut().and_then(Viewing::doc_mut) {
                v.widen_to_word(rect);
            }
            return;
        }
        if let Some((node, rect)) = self.conv_drag_at() {
            let room = self.conv_inner(&node, rect);
            if let (Some(conv), Some(ui)) = (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
                ui.widen_to_word(conv, room);
            }
            return;
        }
        let Some(d) = self.drag.as_mut() else {
            return;
        };
        let (pane, anchor, width) = (d.pane.clone(), d.anchor, d.width);
        let Some(text) = self.blocks_of(&pane).and_then(|b| {
            shell::window_lines(&b.blocks, self.now_ms, width, anchor.line, 1)
                .first()
                .map(select::text_of)
        }) else {
            return;
        };
        let Some((a, b)) = select::word_span(anchor.line, &text, anchor.col) else {
            return;
        };
        if let Some(d) = self.drag.as_mut() {
            d.anchor = a;
            d.moved = true;
        }
        self.selection = Some(select::Selection {
            pane,
            anchor: a,
            head: b,
        });
    }

    /// A double click on the text of a conversation's field selects the word it is on (REQ-637);
    /// `true` when it did.
    pub fn select_word_in_field(&mut self, x: usize, y: usize, cols: usize, rows: usize) -> bool {
        if self.mode != Mode::Workspaces || self.viewer.is_some() {
            return false;
        }
        let Some(doc) = self.shown() else {
            return false;
        };
        let rects = self.layout(doc, self.panes_space(cols, rows));
        let Some(i) = rects.iter().position(|r| inside(*r, x, y)) else {
            return false;
        };
        let pane = doc.panes[i].id.clone();
        let Some(node) = self.conv_node(&pane) else {
            return false;
        };
        let room = self.conv_inner(&node, rects[i]);
        match (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
            (Some(conv), Some(ui)) => ui.select_word_in_field(conv, room, x, y),
            _ => false,
        }
    }

    /// A press on a command or an output line of a pane's list begins a drag (REQ-304); any other
    /// press is a click. Either way, what was selected is let go of. `true` when a drag began.
    pub fn press_select(&mut self, x: usize, y: usize, cols: usize, rows: usize) -> bool {
        self.selection = None;
        self.drag = None;
        self.unlight_conversations();
        if self.viewer.is_some() {
            let rect = self.panes_space(cols, rows);
            return match self.viewer.as_mut() {
                Some(Viewing::Doc(v)) => v.press(rect, x, y),
                Some(Viewing::Text(t)) => t.press(rect, x, y),
                None => false,
            };
        }
        if self.help
            || self.manage.is_some()
            || self.launcher.is_some()
            || self.phone_open
            || self.picker.is_some()
            || self.worktree.is_some()
            || self.project.is_some()
            || self.changes.is_some()
            || self.mode != Mode::Workspaces
        {
            return false;
        }
        let Some(doc) = self.shown() else {
            return false;
        };
        let rects = self.layout(doc, self.panes_space(cols, rows));
        let Some(i) = rects.iter().position(|r| inside(*r, x, y)) else {
            return false;
        };
        let pane = doc.panes[i].id.clone();
        if let Some(node) = self.conv_node(&pane) {
            let room = self.conv_inner(&node, rects[i]);
            let began = match (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
                (Some(conv), Some(ui)) => ui.press(conv, room, x, y),
                _ => false,
            };
            if began {
                self.conv_drag = Some(pane);
            }
            return began;
        }
        let Some(g) = self.list_geom(&pane, rects[i]) else {
            return false;
        };
        let (_, _, iw, _) = inner(rects[i]);
        if !(g.top..g.bottom).contains(&y) || !(g.left..g.left + iw).contains(&x) {
            return false;
        }
        let line = g.start + (y - g.top);
        let on_text = self.blocks_of(&pane).is_some_and(|b| {
            shell::window_lines(&b.blocks, self.now_ms, g.width, line, 1)
                .first()
                .is_some_and(|l| matches!(l.kind, shell::Kind::Command | shell::Kind::Output))
        });
        if !on_text {
            return false;
        }
        self.drag = Some(Drag {
            pane,
            start: (x, y),
            anchor: select::Point {
                line,
                col: x.saturating_sub(g.text_x),
            },
            width: g.width,
            moved: false,
            scrolling: None,
        });
        true
    }

    /// The pointer moved with the button down: the selection goes from where the drag began to
    /// the line and column it is on, or, past the top or the bottom of the list, to its first or
    /// last line, which scrolls (REQ-305).
    pub fn drag_to(&mut self, x: usize, y: usize) {
        if self.viewer.is_some() {
            let (cols, rows) = self.size;
            let (rect, now) = (self.panes_space(cols, rows), self.now_ms);
            match self.viewer.as_mut() {
                Some(Viewing::Doc(v)) => v.drag_to(rect, (x, y), now),
                Some(Viewing::Text(t)) => t.drag_to(rect, x, y),
                None => {}
            }
            return;
        }
        if let Some((node, rect)) = self.conv_drag_at() {
            let now = self.now_ms;
            let room = self.conv_inner(&node, rect);
            if let (Some(conv), Some(ui)) = (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
                ui.drag_to(conv, room, (x, y), now);
            }
            return;
        }
        let Some(d) = self.drag.as_ref() else {
            return;
        };
        let (cols, rows) = self.size;
        let pane = d.pane.clone();
        let Some(g) = self
            .pane_rect(&pane, cols, rows)
            .and_then(|r| self.list_geom(&pane, r))
        else {
            self.drag = None;
            return;
        };
        let last = g.start + (g.bottom - g.top).saturating_sub(1);
        let (head, edge) = if y < g.top {
            (
                select::Point {
                    line: g.start,
                    col: 0,
                },
                Some(true),
            )
        } else if y >= g.bottom {
            (
                select::Point {
                    line: last,
                    col: usize::from(u16::MAX),
                },
                Some(false),
            )
        } else {
            (
                select::Point {
                    line: g.start + (y - g.top),
                    col: x.saturating_sub(g.text_x),
                },
                None,
            )
        };
        let now = self.now_ms;
        let Some(d) = self.drag.as_mut() else {
            return;
        };
        d.moved |= head != d.anchor;
        d.scrolling = match (edge, d.scrolling) {
            (Some(up), Some((was, since))) if up == was => Some((up, since)),
            (Some(up), _) => Some((up, now)),
            (None, _) => None,
        };
        let anchor = d.anchor;
        self.selection = Some(select::Selection { pane, anchor, head });
    }

    /// While a drag is held past the edge of a list, scroll it a line every so often and keep the
    /// selection at its edge.
    pub fn drag_tick(&mut self) {
        if self.viewer.is_some() {
            let (cols, rows) = self.size;
            let (rect, now) = (self.panes_space(cols, rows), self.now_ms);
            if let Some(v) = self.viewer.as_mut().and_then(Viewing::doc_mut) {
                v.drag_tick(rect, now);
            }
            return;
        }
        if let Some((node, rect)) = self.conv_drag_at() {
            let now = self.now_ms;
            let room = self.conv_inner(&node, rect);
            if let (Some(conv), Some(ui)) = (self.convs.get(&node), self.conv_ui.get_mut(&node)) {
                ui.drag_tick(conv, room, now);
            }
            return;
        }
        let Some(Drag {
            pane,
            scrolling: Some((up, since)),
            ..
        }) = self.drag.as_ref()
        else {
            return;
        };
        if self.now_ms < since + DRAG_SCROLL_MS {
            return;
        }
        let (pane, up) = (pane.clone(), *up);
        let (cols, rows) = self.size;
        let Some(rect) = self.pane_rect(&pane, cols, rows) else {
            return;
        };
        self.scroll_pane(&pane, rect, up, 1);
        let Some(g) = self.list_geom(&pane, rect) else {
            return;
        };
        let line = if up {
            g.start
        } else {
            g.start + (g.bottom - g.top).saturating_sub(1)
        };
        let col = if up { 0 } else { usize::from(u16::MAX) };
        if let (Some(d), Some(sel)) = (self.drag.as_mut(), self.selection.as_mut()) {
            d.scrolling = Some((up, self.now_ms));
            sel.head = select::Point { line, col };
        }
    }

    /// The button was let go: a drag that never moved is the click it was; one that did copies what it
    /// covers (REQ-306) and leaves it lit until the next press or key.
    pub fn release(&mut self, cols: usize, rows: usize) -> After {
        if self.viewer.is_some() {
            return self.viewer_release(cols, rows);
        }
        if let Some(pane) = self.conv_drag.take() {
            return self.release_conversation(&pane, cols, rows);
        }
        let Some(d) = self.drag.take() else {
            return After::Continue;
        };
        if !d.moved {
            self.selection = None;
            return self.click(d.start.0, d.start.1, cols, rows);
        }
        let Some(text) = self.selection.as_ref().and_then(|sel| {
            let b = self.blocks_of(&sel.pane)?;
            Some(select::selected_text(&b.blocks, self.now_ms, d.width, sel))
        }) else {
            return After::Continue;
        };
        if text.is_empty() {
            self.selection = None;
            return After::Continue;
        }
        self.flash(&conversation::copied(&text));
        After::Copy(text)
    }

    /// Move a pane's list `by` lines up (`true`) or down, within what there is to see.
    fn scroll_pane(&mut self, pane: &str, rect: Rect, up: bool, by: usize) {
        if let Some(node) = self.conv_node(pane) {
            if let Some(ui) = self.conv_ui.get_mut(&node) {
                ui.scroll_by(up, by);
            }
            return;
        }
        let Some(b) = self.blocks_of(pane) else {
            return;
        };
        let (_, _, iw, ih) = inner(rect);
        let Some(doc) = self.panes.get(pane) else {
            return;
        };
        let total = shell::total_lines(&b.blocks);
        let most = total.saturating_sub(list_h(ih, field_of(doc, b, iw).band_rows()));
        let at = self.scroll.entry(pane.to_owned()).or_insert(0);
        *at = if up {
            (*at + by).min(most)
        } else {
            at.saturating_sub(by)
        };
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
            // A review says which pull request it is, beside the marks (REQ-588).
            let tag = w.review.as_ref().map(|r| format!("⎇#{}", r.pr));
            let tag_w = tag.as_ref().map_or(0, |t| t.chars().count() + 1);
            g.put_over(
                text,
                y,
                &clip(&name, (SIDEBAR - text - 6).saturating_sub(tag_w)),
                name_style,
            );
            if let Some(t) = &tag {
                g.put_over(SIDEBAR - 6 - tag_w + 1, y, t, Style::fg(look::cyan()));
            }
            if let Some(a) = un.first() {
                let (glyph, colour) = mark(a);
                g.put_over(SIDEBAR - 5, y, glyph, Style::fg(colour).bold());
                if un.len() > 1 {
                    g.put_over(SIDEBAR - 4, y, &un.len().to_string(), dim);
                }
            }
            // Claude works in it: a spinner beside the marks (REQ-579).
            if w.working {
                g.put_over(
                    SIDEBAR - 3,
                    y,
                    shell::spin(self.now_ms),
                    Style::fg(look::blue()),
                );
            }
        }
        if session.workspaces.is_empty() {
            g.put(1, 2, "^␣ o abre una carpeta", dim);
        }
        for (i, line) in [
            "! permiso  ? pregunta",
            "◆ plan ❯ comando ⠋ trabaja",
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
            let path_at = cols.saturating_sub(path.chars().count() + 1 + self.tree_w());
            g.put(path_at, 0, path, dim);
            self.panes_area(
                &mut g,
                doc,
                ws.branch.as_deref(),
                (
                    px,
                    1,
                    pw.saturating_sub(self.tree_w()),
                    rows.saturating_sub(2),
                ),
            );
            if self.tree_on {
                self.draw_tree(&mut g, cols, rows);
            }
            if let Some(c) = &self.rv {
                c.draw_focused(
                    &mut g,
                    &self.docs(),
                    &self.label(c.workspace()),
                    self.now_ms,
                    self.rv_focus,
                );
            }
        }

        if let Some(c) = &self.changes {
            c.draw(
                &mut g,
                &self.docs(),
                &self.label(c.workspace()),
                self.now_ms,
            );
        }

        match &self.viewer {
            Some(Viewing::Doc(v)) => v.draw(
                &mut g,
                self.panes_space(cols, rows),
                self.viewer_plan_waits(),
            ),
            Some(Viewing::Text(v)) => v.draw(&mut g, self.panes_space(cols, rows)),
            None => {}
        }

        // The status bar.
        for x in 0..cols {
            g.cells[(rows - 1) * cols + x] = (' ', Style::default().on(look::panel()));
        }
        let level = self.level();
        let conv_focus = self.focused_conversation();
        let diff_status = self
            .changes
            .as_ref()
            .or_else(|| self.rv.as_ref().filter(|_| self.rv_typing()))
            .map(|c| c.status(&self.docs()));
        let viewer_hint = self.viewer.as_ref().map(Viewing::hint);
        let (chip, bg, hint) = match (&self.manage, &diff_status) {
            _ if viewer_hint.is_some() => (
                " LEER ",
                look::cyan(),
                viewer_hint.as_deref().unwrap_or_default(),
            ),
            (None, _) if self.launcher.is_some() => (
                " PANEL ",
                look::magenta(),
                "j/k mover · ↵ abrir · letra directa · esc cancelar",
            ),
            (None, _) if self.picker.is_some() => (
                " ABRIR ",
                look::cyan(),
                "↑↓ mover · → entrar · ← subir · ~ casa · ↵ abrir · esc cerrar",
            ),
            (None, _) if self.worktree.is_some() => {
                (" WORKTREE ", look::cyan(), "↵ crear · esc cancelar")
            }
            (None, _) if self.project.is_some() => (
                " PROYECTO ",
                look::cyan(),
                "↑↓ campo · ↵ editar · g dónde guarda · s guardar · esc cerrar",
            ),
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
                (_, Level::Terminal) if let Some((accent, asking)) = conv_focus => {
                    if asking {
                        (
                            " PERMISO ",
                            accent,
                            "1 2 3 · y a n · ↑↓ ↵ responden · ^␣ salir",
                        )
                    } else {
                        (
                            " CLAUDE ",
                            accent,
                            "↵ envía · / comandos · esc interrumpe · ⇧tab modo · ^O abre · ^V imagen · clic en un ítem y c copia · ^␣ salir",
                        )
                    }
                }
                (_, Level::Terminal) => (
                    " TERMINAL ",
                    look::green(),
                    "escribes en el panel · ^␣ para salir",
                ),
                _ if self.tree_on && self.tree_focus => (
                    " ARCHIVOS ",
                    look::cyan(),
                    "j/k mover · ↵ abrir · h cerrar · / buscar · . ocultos · y ruta · r leer · esc volver · t cerrar",
                ),
                (_, Level::Panels) if self.blk.is_some() => (
                    " BLOQUE ",
                    look::magenta(),
                    "c comando · o salida · r repetir · j/k mover · esc soltar",
                ),
                (_, Level::Panels) => (
                    " PANEL ",
                    look::magenta(),
                    "hjkl panel · HJKL mover · ↵ escribir · n | - nuevo · < > { } tamaño · t archivos · x cerrar · esc volver · ? teclas",
                ),
                (_, Level::Workspaces) => (
                    " WORKSPACE ",
                    look::blue(),
                    "j/k mover · ↵ entrar · r renombrar · o carpeta · w worktree · p proyecto · s subir · x cerrar · d cambios · m teléfono · ? teclas · q salir",
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
        if let Some(p) = &self.prs {
            p.draw(&mut g, (px, 1, pw, rows.saturating_sub(2)), self.now_ms);
        }
        if let Some(p) = &self.project {
            project::draw(&mut g, (px, 1, pw, rows.saturating_sub(2)), p);
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
        if let Some(menu) = &self.launcher {
            menu.draw(&mut g, (px, 1, pw, rows.saturating_sub(2)), &self.programs);
        }
        if let Some(question) = &self.manage {
            question.draw(
                &mut g,
                (px, 1, pw, rows.saturating_sub(2)),
                &self.settings,
                self.os_dark,
                self.now_ms,
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
        let rects = self.layout(doc, (x, y, w, h));
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
            let conv = self.panes.get(&summary.id).and_then(|p| self.conv_of(p));
            let accent = conv.map(|c| conversation::accent_rgb(c.accent));
            let title_style = match (accent, focused) {
                (Some(a), _) => Style::fg(a).bold(),
                (None, true) => Style::fg(look::blue()).bold(),
                (None, false) => Style::fg(look::fg()).bold(),
            };
            let border = match (accent, focused) {
                (Some(a), true) => a,
                (None, true) => look::blue(),
                _ => look::line(),
            };
            g.boxed(
                rect,
                &conv.map_or_else(|| summary.title.clone(), conversation::title),
                border,
                title_style,
                badge,
            );
            let (bx, by, bw, bh) = rect;
            let Some(p) = self.panes.get(&summary.id) else {
                skeleton(g, rect);
                continue;
            };
            if p.conversation.is_some() {
                match conv {
                    Some(c) => self.conv_view(g, c, p, rect, focused, branch),
                    None => skeleton(g, rect),
                }
                continue;
            }
            if let Some(b) = self.blocks_of(&summary.id) {
                self.blocks_view(g, b, p, rect, focused, branch);
                continue;
            }
            let loading = self
                .closing(&summary.id)
                .map(|name| (shell::Phase::Closing, name))
                .or_else(|| {
                    self.starting(&summary.id)
                        .map(|name| (shell::Phase::Starting, name))
                });
            if let Some((phase, name)) = loading {
                shell::loading(g, (bx, by, bw, bh), phase, &name, self.now_ms);
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
            let (width, height) = (bw.saturating_sub(4), bh.saturating_sub(2));
            if p.screen.spans.is_empty() {
                for (r, line) in p.screen.lines.iter().take(height).enumerate() {
                    g.put(
                        bx + 2,
                        by + 1 + r,
                        &clip(line, width),
                        Style::fg(look::fg()),
                    );
                }
            } else {
                for (r, row) in p.screen.spans.iter().take(height).enumerate() {
                    let mut x = 0;
                    for span in row {
                        if x >= width {
                            break;
                        }
                        let text = clip(&span.t, width - x);
                        g.put(bx + 2 + x, by + 1 + r, &text, shell::span_style(span));
                        x += text.chars().count();
                    }
                }
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

    /// The node of the conversation a pane runs, whether it has arrived or not.
    fn conv_node(&self, pane: &str) -> Option<String> {
        self.panes.get(pane)?.conversation.clone()
    }

    /// The conversation of a pane, once its node has arrived.
    fn conv_of(&self, pane: &Pane) -> Option<&contract::conversation::Conversation> {
        self.convs.get(pane.conversation.as_ref()?)
    }

    /// A pane that runs `claude`: the conversation, with the pane's folder and branch as chips.
    fn conv_view(
        &self,
        g: &mut Grid,
        conv: &contract::conversation::Conversation,
        p: &Pane,
        rect: Rect,
        focused: bool,
        branch: Option<&str>,
    ) {
        let Some(ui) = self.conv_ui.get(&conv.head.id) else {
            return;
        };
        // The folder the engine runs in; the pane's own moves with the shell (REQ-630).
        let folder = conv.cwd.as_deref().unwrap_or(&p.cwd);
        let chips: Vec<(String, Rgb)> = std::iter::once((shell::short(folder), look::dim()))
            .chain(branch.map(|br| (format!("⎇ {br}"), look::magenta())))
            .collect();
        let full = inner(rect);
        let (room, area) = agents::split(full, ui.agents.fits(full.2));
        ui.draw(g, conv, room, &chips, focused, self.now_ms);
        if let Some(area) = area {
            agents::draw(
                g,
                conv,
                &ui.agents,
                area,
                self.now_ms,
                conversation::accent_rgb(conv.accent),
            );
        }
    }

    /// The rectangle a conversation takes in its pane's: all of it, or what is left when the
    /// area of the agents is open at its right (REQ-601).
    fn conv_inner(&self, node: &str, rect: Rect) -> Rect {
        let full = inner(rect);
        let shown = self
            .conv_ui
            .get(node)
            .is_some_and(|ui| ui.agents.fits(full.2));
        agents::split(full, shown).0
    }

    /// A shell pane as blocks: the list, newest at the bottom, and the field under it (the chips
    /// and the line the shell holds, in a band of their own, `field.rs`).
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
        let field = field_of(p, b, iw);
        let lh = list_h(ih, field.band_rows());
        let total = shell::total_lines(&b.blocks);
        let scrolled = self.scroll.get(&p.head.id).copied().unwrap_or(0);
        let start = window_start(total, lh, scrolled);
        let lines = shell::window_lines(&b.blocks, self.now_ms, iw - 2, start, lh);
        let picked = self
            .blk
            .as_ref()
            .filter(|(pane, _)| *pane == p.head.id)
            .map(|(_, id)| *id);
        let top = iy + gap(total, lh);
        let lit_by = self.selection.as_ref().filter(|s| s.pane == p.head.id);
        for (r, line) in lines.iter().enumerate() {
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
            // What a drag covers: the theme's most visible colour (REQ-308).
            let covered = lit_by
                .filter(|_| line.kind != shell::Kind::Other)
                .and_then(|s| s.on_line(start + r, select::len_of(line)));
            if let Some((from, to)) = covered {
                g.light(ix + 2 + from, (ix + 2 + to).min(ix + iw), y);
            }
        }
        let running = running_block(b);
        let chips: Vec<(String, Rgb)> = match running {
            Some(k) => vec![(format!("◌ {}", k.cmd), look::yellow())],
            // The folder the shell is in, and that a Claude opened here starts in it (REQ-633).
            None => std::iter::once((
                format!("{} · claude abre aquí", shell::short(&p.cwd)),
                look::dim(),
            ))
            .chain(branch.map(|br| (format!("⎇ {br}"), look::magenta())))
            .collect(),
        };
        field.draw(g, (ix, iy + lh, iw), &chips, running.is_some(), focused);
    }

    fn avisos_box(&self, g: &mut Grid, (x, y, w, h): Rect) {
        let items = self.avisos();
        let (bx, by, bw, bh) = avisos_rect(items.len(), (x, y, w, h));
        g.clear_box((bx, by, bw, bh));
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
        g.clear_box((bx, by, bw, bh));
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
    ("r", "renombrar el worktree"),
    ("o", "abrir una carpeta"),
    ("t", "archivos del workspace"),
    ("w", "worktrees del proyecto: nuevo o los que ya existen"),
    ("R", "revisar un pull request del proyecto"),
    ("p", "la configuración del proyecto"),
    ("s", "subir el worktree a origin"),
    ("x", "cerrar el workspace (y quitar su worktree)"),
    ("d", "ver sus cambios"),
    ("a", "avisos"),
    ("m", "enlace con el teléfono"),
    (",", "ajustes"),
    ("q", "salir (los paneles siguen)"),
];
const PANEL_KEYS: &[(&str, &str)] = &[
    ("h j k l", "ir al panel de ese lado"),
    ("tab", "el panel siguiente"),
    ("↵", "escribir en el panel"),
    ("n", "panel nuevo (por el lado más largo)"),
    ("| -", "panel nuevo a la derecha · debajo"),
    ("H J K L", "intercambiar con el panel de ese lado"),
    ("< >", "panel más estrecho · más ancho"),
    ("{ }", "panel más bajo · más alto"),
    ("N", "panel nuevo con un programa (^␣ N)"),
    ("r", "nombrar el panel"),
    ("f", "el panel a pantalla completa"),
    ("x", "cerrar el panel"),
    (
        "o w s t",
        "abrir una carpeta · worktree nuevo · subir el worktree · archivos",
    ),
    ("d a m ,", "cambios · avisos · teléfono · ajustes"),
    ("esc", "volver a los workspaces"),
];
const TERMINAL_KEYS: &[(&str, &str)] = &[
    ("todo", "va al programa, esc y ctrl incluidos"),
    ("^␣", "salir al nivel de paneles"),
];

/// How long a pairing question is on the screen before `y` answers it.
const ANSWER_DELAY_MS: u64 = 800;
/// What a pane's box holds: inside the border and its padding.
fn inner((x, y, w, h): Rect) -> Rect {
    (x + 2, y + 1, w.saturating_sub(4), h.saturating_sub(2))
}

/// Rows of the list of blocks in a box `h` rows tall under a field of `band` rows.
fn list_h(h: usize, band: usize) -> usize {
    h.saturating_sub(band)
}

/// How long a program that has the screen is waited for before the pane shows its screen as it is.
const LOADING_MAX_MS: u64 = 20_000;
/// How long a program that left the alternate screen is waited for to end before the pane shows its screen.
const CLOSING_MAX_MS: u64 = 3_000;

/// The blocks node of a pane, when it has one and it has arrived.
fn blocks_node<'a>(nodes: &'a HashMap<String, Blocks>, pane: &Pane) -> Option<&'a Blocks> {
    nodes.get(pane.blocks.as_ref()?)
}

/// The command running in a node that is in mode `full` (a program has the screen).
fn running_full(b: &Blocks) -> Option<&contract::Block> {
    running_block(b).filter(|_| b.mode == BlocksMode::Full)
}

/// The command a pane's blocks say is running: the last block, while it runs.
fn running_block(b: &Blocks) -> Option<&contract::Block> {
    b.blocks.last().filter(|k| k.running)
}

/// The field of a shell pane drawn as blocks: what its shell holds at the prompt, the line of its
/// screen when the daemon does not say, or nothing while a command runs.
fn field_of(p: &Pane, b: &Blocks, iw: usize) -> Field {
    if running_block(b).is_some() {
        return Field::new(None, None, iw);
    }
    let c = p.screen.cursor;
    let line = p
        .screen
        .lines
        .get(c.row as usize)
        .map(|l| (l.as_str(), c.col as usize));
    Field::new(p.input.as_deref(), line, iw)
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

/// Leave the alternate screen and raw mode even when drawing panics. `enhanced` is whether the
/// terminal was asked for the keyboard protocol that gives every key its modifiers (shift-enter).
struct Terminal {
    enhanced: bool,
}

impl Terminal {
    fn enter() -> std::io::Result<Terminal> {
        terminal::enable_raw_mode()?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            cursor::Hide,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        // Without it shift-enter arrives as enter. A terminal that does not speak the protocol
        // (kitty's, which Ghostty has) is left as it is.
        let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        Ok(Terminal { enhanced })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            stdout(),
            DisableBracketedPaste,
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
        KeyCode::Char('l') | KeyCode::Right => Key::Right,
        KeyCode::Tab => Key::Next,
        KeyCode::Char('t') => Key::Tree,
        KeyCode::Char('H') => Key::Swap(split::Side::Left),
        KeyCode::Char('L') => Key::Swap(split::Side::Right),
        KeyCode::Char('K') => Key::Swap(split::Side::Up),
        KeyCode::Char('J') => Key::Swap(split::Side::Down),
        KeyCode::Char('>') => Key::Resize(SplitDir::Row, true),
        KeyCode::Char('<') => Key::Resize(SplitDir::Row, false),
        KeyCode::Char('}') => Key::Resize(SplitDir::Col, true),
        KeyCode::Char('{') => Key::Resize(SplitDir::Col, false),
        KeyCode::Char('|') => Key::SplitRow,
        KeyCode::Char('-') => Key::SplitCol,
        KeyCode::Enter => Key::Enter,
        KeyCode::Char('a') => Key::Avisos,
        KeyCode::Char('n') => Key::NewPane,
        KeyCode::Char('N') => Key::NewProgram,
        KeyCode::Char('d') => Key::Changes,
        KeyCode::Char('x') => Key::Close,
        KeyCode::Char('f') => Key::Full,
        KeyCode::Char('?') => Key::Help,
        KeyCode::Char(',') => Key::Settings,
        KeyCode::Char('m') => Key::Phone,
        KeyCode::Char('w') => Key::Worktree,
        KeyCode::Char('R') => Key::Review,
        KeyCode::Char('p') => Key::Project,
        KeyCode::Char('s') => Key::Sync,
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

/// What a command answered, or why it did not: the daemon's message, the link's failure, or that
/// it gave no result when one was expected.
fn reply_value(reply: Reply) -> Result<serde_json::Value, String> {
    match reply {
        Ok(Ok(Some(v))) => Ok(v),
        Ok(Ok(None)) => Err("el daemon no contestó nada".to_owned()),
        Ok(Err(e)) => Err(e.message),
        Err(e) => Err(e.to_string()),
    }
}

/// What a push that worked says: the commits that went up and whether the branch is new there.
fn sync_said(branch: &str, result: Option<&serde_json::Value>) -> String {
    let pushed = result.and_then(|r| r["pushed"].as_u64()).unwrap_or(0);
    let new = result
        .and_then(|r| r["new_branch"].as_bool())
        .unwrap_or(false);
    match (pushed, new) {
        (0, _) => format!("{branch} · al día con origin, nada que subir"),
        (n, true) => format!("subido {branch} · {} · rama nueva en origin", commits(n)),
        (n, false) => format!("subido {branch} · {} a origin", commits(n)),
    }
}

fn commits(n: u64) -> String {
    if n == 1 {
        "1 commit".to_owned()
    } else {
        format!("{n} commits")
    }
}

/// How much of a split one press of `<` `>` `{` `}` moves, in thousandths.
const RESIZE_STEP: i16 = 50;

/// How long a panel the client asked for is waited for, to focus it.
const NEW_PANE_WAIT_MS: u64 = 5000;

/// List a folder or search in a thread of its own, with a connection of its own: git may take
/// as long as the repository is big, and the screen goes on meanwhile.
fn spawn_files(ask: FileAsk, tx: std::sync::mpsc::Sender<FilesReply>) {
    std::thread::spawn(move || {
        let cmd = match &ask {
            FileAsk::List { workspace, path } => Cmd::ListFiles {
                workspace: workspace.clone(),
                path: Some(path.clone()),
            },
            FileAsk::Find { workspace, query }
            | FileAsk::Mention {
                workspace, query, ..
            } => Cmd::FindFiles {
                workspace: workspace.clone(),
                query: query.clone(),
            },
            FileAsk::Saved { workspace, .. } => Cmd::ListConversations {
                workspace: workspace.clone(),
            },
            FileAsk::Older { node, before } => Cmd::ReadOlder {
                node: node.clone(),
                before: *before,
                count: older::PAGE,
            },
        };
        let reply = Client::connect(&identity::socket_path(), "jwn-tui-files")
            .and_then(|mut c| c.cmd_within(cmd, FILES_WAIT));
        let _ = tx.send(FilesReply { ask, reply });
    });
}

/// How long the list of pull requests is waited for, and the making of a review: `gh` and a fetch
/// take what the network takes.
const PRS_WAIT: Duration = Duration::from_secs(60);
const REVIEW_WAIT: Duration = Duration::from_secs(300);

/// Ask `gh`, through the daemon, in a thread of its own with a connection of its own: the screen
/// goes on (the spinner moves) while the network answers.
fn spawn_prs(ask: PrsAsk, tx: std::sync::mpsc::Sender<PrsReply>) {
    std::thread::spawn(move || {
        let (cmd, wait) = match &ask {
            PrsAsk::List {
                workspace,
                requested,
            } => (
                Cmd::ListPrs {
                    workspace: workspace.clone(),
                    requested: *requested,
                },
                PRS_WAIT,
            ),
            PrsAsk::Review { workspace, pr } => (
                Cmd::ReviewPr {
                    workspace: workspace.clone(),
                    pr: *pr,
                },
                REVIEW_WAIT,
            ),
        };
        let reply = Client::connect(&identity::socket_path(), "jwn-tui-prs")
            .and_then(|mut c| c.cmd_within(cmd, wait));
        let _ = tx.send(PrsReply { ask, reply });
    });
}

/// The browser, asked to open a pull request's address (only `http` and `https`).
fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("no es una dirección web".into());
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Reaped in a thread of its own: some openers stay as long as the browser does.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// How long a push is waited for.
const SYNC_WAIT: Duration = Duration::from_secs(120);

/// Push in a thread of its own, with a connection of its own, so the screen goes on (the spinner
/// moves, the other keys wait) while git runs. The reply comes back through the channel.
fn spawn_sync(workspace: String, tx: std::sync::mpsc::Sender<Reply>) {
    std::thread::spawn(move || {
        let reply = Client::connect(&identity::socket_path(), "jwn-tui-sync")
            .and_then(|mut c| c.cmd_within(Cmd::SyncWorkspace { workspace }, SYNC_WAIT));
        let _ = tx.send(reply);
    });
}

/// Ask the daemon to stop what it runs for `workspace`, with a connection of its own: the push or
/// the review that waits has its reply coming on another (REQ-604). The reply says nothing new.
fn spawn_cancel(workspace: String) {
    std::thread::spawn(move || {
        let _ = Client::connect(&identity::socket_path(), "jwn-tui-cancel")
            .and_then(|mut c| c.cmd_within(Cmd::Cancel { workspace }, ASK_WAIT));
    });
}

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
    // What is not pushed is only said, never needed: without it the question just says less.
    if let Ok(Ok(Some(r))) = client.cmd_within(
        Cmd::CheckWorkspace {
            workspace: workspace.clone(),
        },
        ASK_WAIT,
    ) {
        view.check_unpushed(r["ahead"].as_u64().unwrap_or(0));
    }
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
fn shown_in_a_pane(
    view: &mut View,
    client: &mut Client,
    reply: Reply,
    failed: &str,
    open: Option<contract::OpenKind>,
) {
    match reply {
        Ok(Ok(result)) => {
            if let Some(ws) = result.as_ref().and_then(|r| r["workspace"].as_str()) {
                // A workspace opens with the one panel its settings or the person's choice say
                // (a folder's, its repository's when it is its top level); when that cannot be
                // done it is one shell.
                let started = match reply_value(client.cmd_within(
                    Cmd::StartWorkspace {
                        workspace: ws.to_owned(),
                        open,
                    },
                    ASK_WAIT,
                )) {
                    Ok(_) => true,
                    Err(why) => {
                        view.manage_failed("No se pudieron abrir los paneles", &why);
                        false
                    }
                };
                if !started {
                    match client.cmd(Cmd::NewPane {
                        workspace: ws.to_owned(),
                        program: None,
                        beside: None,
                        dir: None,
                    }) {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => view.manage_failed("No se pudo abrir un panel", &e.message),
                        Err(e) => view.manage_failed("No se pudo abrir un panel", &e.to_string()),
                    }
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

/// `conversation_set` of one thing.
fn set(
    node: String,
    name: Option<String>,
    accent: Option<contract::conversation::Accent>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<contract::conversation::Mode>,
) -> Cmd {
    Cmd::ConversationSet {
        node,
        name,
        accent,
        model,
        effort,
        mode,
    }
}

/// What the person asked of a conversation, as the command the daemon takes; a refusal (an
/// unknown command, an engine that is gone) goes to the status bar.
fn conversation_cmd(view: &mut View, client: &mut Client, act: ConvAct) {
    let cmd = match act {
        ConvAct::Send { node, text, images } => Cmd::ConversationSend { node, text, images },
        ConvAct::Answer {
            node,
            ask,
            key,
            text,
        } => Cmd::ConversationAnswer {
            node,
            ask,
            key,
            text,
            by: Some("el terminal".into()),
        },
        ConvAct::Interrupt { node } => Cmd::ConversationInterrupt { node },
        ConvAct::StopTask { node, task } => Cmd::ConversationStopTask { node, task },
        ConvAct::Name { node, name } => set(node, Some(name), None, None, None, None),
        ConvAct::Color { node, accent } => set(node, None, Some(accent), None, None, None),
        ConvAct::Model { node, model } => set(node, None, None, Some(model), None, None),
        ConvAct::Effort { node, effort } => set(node, None, None, None, Some(effort), None),
        ConvAct::Mode { node, mode } => set(node, None, None, None, None, Some(mode)),
        ConvAct::Shell { node, command } => Cmd::ConversationShell { node, command },
        ConvAct::Resume { pane, id } => Cmd::OpenConversation {
            pane,
            resume: Some(id),
            args: Vec::new(),
            env: None,
        },
        ConvAct::Copy(_) | ConvAct::Open(_) | ConvAct::ViewPlan { .. } => return,
    };
    match client.cmd(cmd) {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => view.flash(&e.message),
        Err(e) => view.flash(&e.to_string()),
    }
}

/// A Markdown file or a plan, asked of the daemon.
fn read_doc(client: &mut Client, cmd: Cmd) -> Result<contract::markdown::MarkdownDoc, String> {
    match client.cmd(cmd) {
        Ok(Ok(Some(doc))) => serde_json::from_value(doc).map_err(|e| e.to_string()),
        Ok(Ok(None)) => Err("el daemon no respondió con un documento".into()),
        Ok(Err(e)) => Err(e.message),
        Err(e) => Err(e.to_string()),
    }
}

/// How long a text file is waited for: an older daemon does not know the command and never
/// answers it (REQ-619).
const FILE_WAIT: Duration = Duration::from_secs(10);

/// A text file asked of the daemon, and shown in the viewer of text, or why it is not.
fn read_file(
    view: &mut View,
    client: &mut Client,
    workspace: String,
    path: String,
    size: Option<u64>,
    keep: bool,
) {
    let cmd = Cmd::ReadFile {
        workspace: workspace.clone(),
        path: path.clone(),
    };
    match client.cmd_within(cmd, FILE_WAIT) {
        Ok(Ok(Some(doc))) => match serde_json::from_value(doc) {
            Ok(doc) => view.open_text(workspace, path, doc, keep),
            Err(e) => view.flash(&e.to_string()),
        },
        Ok(Ok(None)) => view.flash("el daemon no respondió con un archivo"),
        Ok(Err(e)) if e.code == contract::ErrorCode::Binary => {
            view.open_refused(workspace, path, size);
        }
        Ok(Err(e)) => view.flash(&e.message),
        Err(ClientError::Protocol(_)) => {
            view.flash("el daemon no responde a read_file: quizá haga falta actualizarlo");
        }
        Err(e) => view.flash(&e.to_string()),
    }
}

/// Whether the viewer reads the file: `.md` and `.markdown`, whatever the case (REQ-470).
fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| ["md", "markdown"].contains(&e.to_lowercase().as_str()))
}

/// Do what a key, a click or an answer asked for; true when the client is to quit.
fn do_after(
    after: After,
    view: &mut View,
    client: &mut Client,
    sync_tx: &std::sync::mpsc::Sender<Reply>,
) -> bool {
    match after {
        After::Quit => return true,
        After::Write { pane, data } => {
            let _ = client.send(Cmd::Write { pane, data });
        }
        After::Swap(a, b) => {
            let reply = client.cmd(Cmd::SwapPanes { a, b });
            told(
                view,
                "No se pudo intercambiar los paneles",
                "paneles intercambiados",
                reply,
            );
        }
        After::Grow { pane, dir, by } => {
            if let Ok(Err(_)) = client.cmd(Cmd::GrowPane { pane, dir, by }) {
                view.flash(match dir {
                    SplitDir::Row => "no hay división en horizontal para este panel",
                    SplitDir::Col => "no hay división en vertical para este panel",
                });
            }
        }
        After::NewPane(workspace, place) => {
            let reply = client.cmd(Cmd::NewPane {
                workspace,
                program: None,
                beside: place.beside,
                dir: Some(place.dir),
            });
            view.focus_new(&reply);
        }
        After::NewProgram {
            workspace,
            program,
            place,
        } => {
            let reply = client.cmd(Cmd::NewPane {
                workspace,
                program: Some(program),
                beside: place.beside,
                dir: Some(place.dir),
            });
            view.focus_new(&reply);
            match reply {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => view.manage_failed("No se pudo abrir el programa", &e.message),
                Err(e) => view.manage_failed("No se pudo abrir el programa", &e.to_string()),
            }
        }
        After::OpenFolder { path, create } => {
            if create && let Err(e) = std::fs::create_dir(&path) {
                view.manage_failed("No se pudo crear la carpeta", &e.to_string());
            } else {
                let reply = client.cmd(Cmd::OpenFolder {
                    path: path.to_string_lossy().into_owned(),
                });
                shown_in_a_pane(view, client, reply, "No se pudo abrir la carpeta", None);
            }
        }
        After::CreateWorktree {
            repo,
            branch,
            from,
            folder,
            open,
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
                view,
                client,
                reply,
                "No se pudo crear el worktree",
                Some(open),
            );
        }
        After::OpenWorktrees { repo, paths } => {
            let mut failed = Vec::new();
            let mut opened = 0;
            for path in paths {
                let reply = client.cmd_within(
                    Cmd::OpenWorktree {
                        repo: repo.to_string_lossy().into_owned(),
                        path,
                    },
                    ASK_WAIT,
                );
                match reply_value(reply) {
                    Ok(_) => opened += 1,
                    Err(why) => failed.push(why),
                }
            }
            view.worktrees_opened(&repo, opened, &failed);
        }
        After::SaveProject { repo, to, set } => {
            let reply = client.cmd_within(
                Cmd::EditProjectConfig {
                    repo: repo.to_string_lossy().into_owned(),
                    to,
                    set,
                },
                ASK_WAIT,
            );
            view.project_saved(&repo, reply);
        }
        After::Sync { workspace } => spawn_sync(workspace, sync_tx.clone()),
        After::CancelOp { workspace } => spawn_cancel(workspace),
        After::Rename { target, name } => {
            let cmd = match target {
                Named::Workspace(workspace) => Cmd::RenameWorkspace { workspace, name },
                Named::Pane(pane) => Cmd::RenamePane { pane, name },
            };
            let reply = client.cmd(cmd);
            told(view, "No se pudo renombrar", "renombrado", reply);
        }
        After::ClosePane(pane) => {
            let reply = client.cmd(Cmd::ClosePane { pane });
            told(view, "No se pudo cerrar el panel", "panel cerrado", reply);
        }
        After::RemoveWorkspace {
            workspace,
            remove_worktree,
            force,
        } => {
            let reply = client.cmd(Cmd::RemoveWorkspace {
                workspace,
                remove_worktree,
                force,
            });
            if remove_worktree {
                told(
                    view,
                    "No se pudo quitar el worktree",
                    "worktree quitado · la rama se queda",
                    reply,
                );
            } else {
                told(
                    view,
                    "No se pudo cerrar el workspace",
                    "workspace cerrado · la carpeta se queda",
                    reply,
                );
            }
        }
        After::Check(workspace) => check_changes(view, client, workspace),
        After::Conv(act) => conversation_cmd(view, client, act),
        After::ReadMarkdown { workspace, path } => {
            let cmd = Cmd::ReadMarkdown {
                workspace: workspace.clone(),
                path,
            };
            match read_doc(client, cmd) {
                Ok(doc) => view.open_viewer(doc, viewer::Source::File, workspace),
                Err(why) => view.flash(&why),
            }
        }
        After::ReadFile {
            workspace,
            path,
            size,
            keep,
        } => read_file(view, client, workspace, path, size, keep),
        After::ReadPlan { node, ask } => {
            let cmd = Cmd::ReadPlan {
                node: node.clone(),
                ask: ask.clone(),
            };
            match read_doc(client, cmd) {
                Ok(doc) => view.open_viewer(doc, viewer::Source::Plan { node, ask }, String::new()),
                Err(why) => view.flash(&why),
            }
        }
        After::Copy(text) => {
            let mut out = stdout();
            let _ = write!(out, "{}", shell::osc52(&text));
            let _ = out.flush();
        }
        After::Continue => {}
    }
    false
}

pub fn run() -> Result<(), String> {
    let mut view = View::new("conectando");
    let mut client = connect(&mut view)?;
    // `JWN_TUI_DUMP`: also write each frame, without colours, to this file (for tests).
    let dump = std::env::var_os(identity::env_var("TUI_DUMP"));
    let _term = Terminal::enter().map_err(|e| e.to_string())?;
    let mut last: Vec<String> = Vec::new();
    view.set_settings(Settings::load());
    view.set_programs(Programs::read());
    view.set_os_dark(settings::os_is_dark());
    let mut phone_worker: Option<phone::Worker> = None;
    let (sync_tx, sync_rx) = std::sync::mpsc::channel::<Reply>();
    let (files_tx, files_rx) = std::sync::mpsc::channel::<FilesReply>();
    let (prs_tx, prs_rx) = std::sync::mpsc::channel::<PrsReply>();
    let (mut settings_read, mut appearance_read) = (Instant::now(), Instant::now());
    let mut programs_read = Instant::now();
    loop {
        // The settings file may be changed by another terminal; the machine's appearance by
        // the person, at any time.
        if settings_read.elapsed() >= SETTINGS_EVERY && !view.unsaved() {
            settings_read = Instant::now();
            view.set_settings(Settings::load());
        }
        if programs_read.elapsed() >= SETTINGS_EVERY {
            programs_read = Instant::now();
            view.set_programs(Programs::read());
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
        view.drag_tick();
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
        for repo in view.take_worktree_asks() {
            let reply = client.cmd_within(
                Cmd::ListWorktrees {
                    repo: repo.to_string_lossy().into_owned(),
                },
                ASK_WAIT,
            );
            view.worktrees_replied(&repo, reply);
        }
        for repo in view.take_project_asks() {
            let reply = client.cmd_within(
                Cmd::ProjectConfig {
                    repo: repo.to_string_lossy().into_owned(),
                },
                ASK_WAIT,
            );
            view.project_replied(&repo, reply);
        }
        while let Ok(reply) = sync_rx.try_recv() {
            view.sync_done(reply);
        }
        while let Ok(reply) = files_rx.try_recv() {
            view.files_replied(reply);
        }
        while let Ok(answer) = prs_rx.try_recv() {
            view.prs_replied(answer);
        }
        for ask in view.take_prs_asks() {
            spawn_prs(ask, prs_tx.clone());
        }
        view.ask_older();
        for ask in view.take_file_asks() {
            spawn_files(ask, files_tx.clone());
        }
        if let Some(after) = view.take_then()
            && do_after(after, &mut view, &mut client, &sync_tx)
        {
            return Ok(());
        }
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
                Event::Paste(text) => view.paste(&text),
                Event::Mouse(m) => {
                    let (cols, rows) = (cols as usize, rows as usize);
                    let (x, y) = (m.column as usize, m.row as usize);
                    match m.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            let double = view.double_press(x, y);
                            if view.press_select(x, y, cols, rows) {
                                if double {
                                    view.widen_to_word();
                                }
                                After::Continue
                            } else if double && view.select_word_in_field(x, y, cols, rows) {
                                After::Continue
                            } else {
                                view.click(x, y, cols, rows)
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            view.drag_to(x, y);
                            After::Continue
                        }
                        MouseEventKind::Up(MouseButton::Left) => view.release(cols, rows),
                        MouseEventKind::ScrollUp => view.scroll(false, x, y, cols, rows),
                        MouseEventKind::ScrollDown => view.scroll(true, x, y, cols, rows),
                        _ => After::Continue,
                    }
                }
                _ => After::Continue,
            };
            if do_after(after, &mut view, &mut client, &sync_tx) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The shared fixtures as the documents a client holds: the same ones the phone draws.
    fn fixture_docs() -> Vec<serde_json::Value> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        [
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
        .collect()
    }

    /// The rectangles of the panels of the workspace shown.
    fn rects_of(v: &View, area: Rect) -> Vec<Rect> {
        v.layout(v.shown().unwrap(), area)
    }

    fn fixture_view() -> View {
        let mut v = View::new("fixtures");
        v.set_size(120, 30);
        v.sync(fixture_docs().iter());
        v
    }

    #[test]
    fn a_document_that_did_not_change_is_not_read_again() {
        let docs = fixture_docs();
        let mut v = View::new("fixtures");
        v.sync(docs.iter());
        // Something this view holds that a second reading of the same documents would undo.
        v.blocks.get_mut(BLOCKS).unwrap().mode = BlocksMode::Full;
        v.sync(docs.iter());
        assert_eq!(
            v.blocks[BLOCKS].mode,
            BlocksMode::Full,
            "the blocks were not read again"
        );
    }

    #[test]
    fn a_document_with_another_revision_or_another_epoch_is_read_again() {
        let mut docs = fixture_docs();
        let blocks = docs.len() - 1;
        let mut v = View::new("fixtures");
        v.sync(docs.iter());
        v.blocks.get_mut(BLOCKS).unwrap().mode = BlocksMode::Full;
        let rev = docs[blocks]["rev"].as_u64().unwrap();
        docs[blocks]["rev"] = (rev + 1).into();
        v.sync(docs.iter());
        assert_eq!(
            v.blocks[BLOCKS].mode,
            BlocksMode::Blocks,
            "a new revision is read"
        );
        v.blocks.get_mut(BLOCKS).unwrap().mode = BlocksMode::Full;
        docs[blocks]["epoch"] = "00000000-0000-4000-8000-0000000000ee".into();
        v.sync(docs.iter());
        assert_eq!(
            v.blocks[BLOCKS].mode,
            BlocksMode::Blocks,
            "a restarted daemon starts its revisions over: the same number is another document"
        );
    }

    #[test]
    fn what_the_client_no_longer_holds_goes_and_comes_back_when_it_does() {
        let docs = fixture_docs();
        let mut v = View::new("fixtures");
        v.sync(docs.iter());
        assert!(v.blocks.contains_key(BLOCKS) && v.read.contains_key(BLOCKS));
        v.sync(docs[..docs.len() - 1].iter());
        assert!(!v.blocks.contains_key(BLOCKS) && !v.read.contains_key(BLOCKS));
        assert!(
            v.wants().iter().any(|id| id == BLOCKS),
            "it is asked for again"
        );
        v.sync(docs.iter());
        assert!(v.blocks.contains_key(BLOCKS));
        v.sync([].iter());
        assert!(v.session.is_none() && v.panes.is_empty() && v.workspaces.is_empty());
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

    /// REQ-579: a workspace whose Claude works has a spinner beside its marks, which turns, and
    /// one that does not work has none.
    #[test]
    fn a_workspace_that_works_has_a_spinner_beside_its_marks() {
        let mut v = fixture_view();
        let spin = |v: &View| {
            let t = v.plain(110, 30);
            let rows: Vec<String> = t.lines().map(|l| l.chars().take(28).collect()).collect();
            (rows[2].clone(), rows[4].clone())
        };
        v.set_now(0);
        let (first, notes) = spin(&v);
        assert!(first.contains('!') && first.contains('⠋'), "{first}");
        assert!(!notes.contains('⠋'), "the other one does not work: {notes}");
        v.set_now(110);
        assert!(spin(&v).0.contains('⠙'), "it turns");
    }

    fn prs_list() -> serde_json::Value {
        serde_json::json!({"prs": [
            {"number": 142, "title": "Add rate limiting", "author": "maria", "branch": "maria/rl",
             "base": "main", "updated": "2026-10-19T10:00:00Z", "decision": "REVIEW_REQUIRED",
             "additions": 312, "deletions": 41, "body": "Adds a bucket.", "url": "https://x/pull/142",
             "files": [{"path": "src/limit.rs", "additions": 188, "deletions": 0}]},
            {"number": 139, "title": "Fix cursor", "author": "luis", "branch": "luis/c",
             "base": "main", "updated": "2026-10-18T10:00:00Z",
             "additions": 9, "deletions": 4}
        ]})
    }

    /// REQ-587 to REQ-589: `R` opens the list of pull requests, which is asked of the daemon and
    /// drawn; `↵` asks for the review and the workspace it makes is opened; a failure of `gh`
    /// closes the list and says why.
    #[test]
    fn r_lists_the_pull_requests_and_enter_makes_the_review_and_opens_it() {
        let mut v = fixture_view();
        let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(key_of(KeyCode::Char('R')), Some(Key::Review));
        v.press(Key::Review);
        assert!(v.plain(120, 30).contains("cargando los PR abiertos"));
        let asks = v.take_prs_asks();
        assert!(
            matches!(
                &asks[..],
                [PrsAsk::List {
                    requested: false,
                    ..
                }]
            ),
            "asks the daemon"
        );
        let ws = match &asks[0] {
            PrsAsk::List { workspace, .. } => workspace.clone(),
            PrsAsk::Review { .. } => unreachable!(),
        };
        v.prs_replied(PrsReply {
            ask: PrsAsk::List {
                workspace: ws.clone(),
                requested: false,
            },
            reply: Ok(Ok(Some(prs_list()))),
        });
        let t = v.plain(120, 30);
        for want in [
            "#142",
            "Add rate limiting",
            "maria",
            "+312",
            "sin revisar",
            "#139",
            "src/limit.rs",
        ] {
            assert!(t.contains(want), "{want:?} is not drawn:\n{t}");
        }
        // the filter, then the review
        v.press_key(key('l'));
        v.press_key(key('u'));
        let t = v.plain(120, 30);
        assert!(
            t.contains("Fix cursor") && !t.contains("Add rate limiting"),
            "the filter keeps luis's:\n{t}"
        );
        v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let asks = v.take_prs_asks();
        assert!(
            matches!(&asks[..], [PrsAsk::Review { pr: 139, .. }]),
            "{}",
            asks.len()
        );
        assert!(v.plain(120, 30).contains("#139 · trayendo la rama"));
        v.prs_replied(PrsReply {
            ask: PrsAsk::Review {
                workspace: ws.clone(),
                pr: 139,
            },
            reply: Ok(Ok(Some(
                serde_json::json!({"workspace": "w9", "existed": false, "dirty": false}),
            ))),
        });
        assert!(v.prs.is_none(), "the list is closed");
        assert!(
            v.goto.as_ref().is_some_and(|(id, _)| id == "w9"),
            "the review is opened when it shows"
        );
        assert!(v.flash.contains("review lista"));
    }

    /// A view whose first workspace reviews pull request 142.
    fn review_view() -> View {
        let mut docs = fixture_docs();
        for d in docs.iter_mut().filter(|d| d["type"] == "workspace") {
            d["review"] = serde_json::json!({"pr": 142, "base": "main"});
        }
        let mut v = View::new("fixtures");
        v.set_size(120, 30);
        v.sync(docs.iter());
        v
    }

    /// REQ-589: the workspace of a review is its agent and, beside it, the diff of the pull request
    /// against its base, which has the keyboard when it is asked for (`d`, `l`, a click) and gives it
    /// back to the commands with the leader.
    #[test]
    fn a_review_is_the_agent_and_the_diff_beside_it() {
        let mut v = review_view();
        assert!(v.rv.is_some(), "the diff is made with the workspace");
        let asks = v.take_asks();
        assert!(
            asks.iter().any(|a| matches!(
                a,
                Ask::OpenDiff {
                    scope: DiffScope::General,
                    commit: None,
                    ..
                }
            )),
            "it asks for the pull request's changes, no more: {asks:?}"
        );
        assert!(
            !asks.iter().any(|a| matches!(
                a,
                Ask::OpenDiff {
                    scope: DiffScope::Uncommitted,
                    ..
                }
            )),
            "{asks:?}"
        );
        v.press(Key::Enter);
        let t = v.plain(120, 30);
        assert!(
            t.contains("╭─ zsh") && t.contains("diff · contra"),
            "both are drawn:\n{t}"
        );
        let diff_x = v.diff_rect().0;
        assert!(diff_x > SIDEBAR + 30, "the agent keeps its share: {diff_x}");
        // the keyboard moves between them with the keys of the panels: the diff is the panel at the
        // right of the last one (the fixture's workspace has several)
        assert!(!v.rv_focus);
        for _ in 0..4 {
            v.press(Key::Right);
        }
        assert!(v.rv_focus, "l goes to the diff");
        v.press(Key::Left);
        assert!(!v.rv_focus, "h comes back");
        for _ in 0..4 {
            v.press(Key::Next);
        }
        assert!(v.rv_focus, "tab after the last panel is the diff");
        v.press(Key::Left);
        assert!(!v.rv_focus);
        // d gives the keys to the diff, and the leader gives them back
        v.press(Key::Changes);
        assert!(v.rv_focus && v.typing && v.rv_typing());
        assert!(v.changes.is_none(), "no screen of its own opens");
        v.press_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(!v.typing);
        // a click in the diff takes the keys, one in the agent gives them back to it
        let at = v.diff_rect();
        v.click(at.0 + 5, at.1 + 4, 120, 30);
        assert!(v.rv_focus && v.typing);
        v.click(SIDEBAR + 6, 6, 120, 30);
        assert!(!v.rv_focus);
        // the tree is not here: the review has its diff
        v.press_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        v.press(Key::Tree);
        assert!(!v.tree_on && v.flash.contains("diff al lado"));
    }

    /// REQ-590: `d` opens the diff beside the panels of any workspace, on the uncommitted changes,
    /// `d` closes it, and `z` makes it fill the area.
    #[test]
    fn d_opens_the_diff_beside_the_panels_of_any_workspace_and_z_fills_the_area() {
        let mut v = fixture_view();
        assert!(v.rv.is_none());
        v.press(Key::Changes);
        assert!(v.changes.is_none(), "no screen of its own: there is room");
        assert!(v.rv.is_some() && v.rv_focus && v.rv_typing());
        let asks = v.take_asks();
        assert!(
            matches!(
                &asks[..],
                [Ask::OpenDiff {
                    scope: DiffScope::Uncommitted,
                    ..
                }]
            ),
            "the uncommitted changes, the viewer's first view: {asks:?}"
        );
        v.changes_made(Src::Uncommitted, "n1");
        let _ = v.take_asks();
        let t = v.plain(120, 30);
        assert!(
            t.contains("╭─ zsh") && t.contains("diff · contra HEAD"),
            "both are drawn:\n{t}"
        );
        // the keys are the viewer's: 3 is the general view, which asks for it
        v.press_key(plain(KeyCode::Char('3')));
        assert!(
            v.take_asks().iter().any(|a| matches!(
                a,
                Ask::OpenDiff {
                    scope: DiffScope::General,
                    ..
                }
            )),
            "1 2 3 change the view"
        );
        // z fills the area, z gives it back
        let beside = v.diff_rect();
        v.press_key(plain(KeyCode::Char('z')));
        assert_eq!(v.diff_rect().0, SIDEBAR + 1, "all of the area");
        v.press_key(plain(KeyCode::Char('z')));
        assert_eq!(v.diff_rect(), beside);
        // d closes it, and the panels have their width back
        v.press_key(plain(KeyCode::Char('d')));
        assert!(v.rv.is_none() && !v.diff_open && !v.rv_focus && !v.typing);
        assert!(
            v.take_asks().contains(&Ask::Close("n1".into())),
            "its nodes are let go"
        );
        // in the workspace of a review d only gives the keys back
        let mut r = review_view();
        r.press(Key::Changes);
        r.press_key(plain(KeyCode::Char('d')));
        assert!(r.rv.is_some() && !r.typing, "the diff of a review stays");
    }

    /// REQ-592: where picked lines are goes in the field of the agent of the workspace and the keys
    /// go there; with no agent it is copied.
    #[test]
    fn a_reference_goes_in_the_agents_field_or_is_copied_when_there_is_none() {
        let mut v = fixture_view();
        v.press(Key::Enter);
        v.press(Key::Changes);
        assert_eq!(
            v.refer("@a.rs#L3 ".into()),
            After::Copy("@a.rs#L3 ".into()),
            "no conversation in the fixtures' panes"
        );
        assert!(v.flash.contains("copiada"));
        let mut docs = fixture_docs();
        for d in docs
            .iter_mut()
            .filter(|d| d["id"] == "00000000-0000-4000-8000-0000000000c1")
        {
            d["conversation"] = serde_json::json!("conv-1");
        }
        let mut v = View::new("fixtures");
        v.set_size(120, 30);
        v.sync(docs.iter());
        v.press(Key::Enter);
        v.press(Key::Changes);
        assert!(v.rv_typing());
        assert_eq!(v.refer("@a.rs#L3-5 ".into()), After::Continue);
        assert_eq!(v.conv_ui["conv-1"].editor.text(), "@a.rs#L3-5 ");
        assert!(!v.rv_focus && v.typing, "the keys are the agent's now");
    }

    /// The diff is let go with its workspace, and a terminal too narrow for both has the screen of `d`.
    #[test]
    fn the_diff_of_a_review_goes_with_it_and_a_narrow_terminal_has_none() {
        let mut v = review_view();
        let _ = v.take_asks();
        v.press(Key::Down);
        v.sync(fixture_docs().iter());
        v.set_size(120, 30);
        let _ = v.take_asks();
        v.set_size(70, 30);
        assert_eq!(v.diff_w(), 0, "no room for both");
        let mut w = review_view();
        w.set_size(70, 30);
        assert!(w.rv.is_none());
    }

    /// REQ-588: a workspace that reviews a pull request says which one, beside its marks.
    #[test]
    fn a_review_says_its_pull_request_in_the_sidebar() {
        let t = fixture_view().plain(110, 30);
        let row = |n: usize| -> String { t.lines().nth(n).unwrap().chars().take(28).collect() };
        assert!(row(4).contains("⎇#142"), "{}", row(4));
        assert!(!row(2).contains('⎇'), "{}", row(2));
    }

    #[test]
    fn a_gh_that_fails_closes_the_list_and_a_review_that_fails_keeps_it() {
        let mut v = fixture_view();
        v.press(Key::Review);
        let ws = match &v.take_prs_asks()[0] {
            PrsAsk::List { workspace, .. } => workspace.clone(),
            PrsAsk::Review { .. } => unreachable!(),
        };
        v.prs_replied(PrsReply {
            ask: PrsAsk::List {
                workspace: ws.clone(),
                requested: false,
            },
            reply: Ok(Err(ErrorBody {
                code: contract::ErrorCode::GitFailed,
                message: "gh: not logged in".into(),
            })),
        });
        assert!(v.prs.is_none());
        assert!(v.flash.contains("not logged in"), "{}", v.flash);
        // a list that is there and a review that fails
        v.press(Key::Review);
        let _ = v.take_prs_asks();
        v.prs_replied(PrsReply {
            ask: PrsAsk::List {
                workspace: ws.clone(),
                requested: false,
            },
            reply: Ok(Ok(Some(prs_list()))),
        });
        v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = v.take_prs_asks();
        v.prs_replied(PrsReply {
            ask: PrsAsk::Review {
                workspace: ws,
                pr: 142,
            },
            reply: Ok(Err(ErrorBody {
                code: contract::ErrorCode::GitFailed,
                message: "fetch failed".into(),
            })),
        });
        assert!(v.prs.is_some(), "the list stays");
        assert!(v.flash.contains("no se hizo nada: fetch failed"));
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

    /// Where `text` is in the frame: the column it starts at and its row.
    fn find(v: &View, text: &str) -> (usize, usize) {
        for (y, row) in v.grid(120, 30).rows_plain().iter().enumerate() {
            if let Some(i) = row.find(text) {
                return (row[..i].chars().count(), y);
            }
        }
        panic!("{text} is not on the screen");
    }

    #[test]
    fn a_drag_over_the_output_copies_what_it_covers_when_the_button_is_let_go() {
        let mut v = open_blocks();
        let (x, y) = find(&v, "On branch feat/login");
        assert!(
            v.press_select(x + 3, y, 120, 30),
            "a press on the output begins a drag"
        );
        v.drag_to(x + 9, y + 1);
        let after = v.release(120, 30);
        assert_eq!(
            after,
            After::Copy("branch feat/login\nnothing to".into()),
            "from where it began to where it ended"
        );
        assert!(
            v.plain(120, 30)
                .contains("copiado · 2 líneas · 28 caracteres")
        );
    }

    #[test]
    fn what_a_drag_covers_is_lit_in_the_most_visible_colour_of_the_theme_until_a_key_or_a_press() {
        let mut v = open_blocks();
        let (x, y) = find(&v, "On branch feat/login");
        v.press_select(x + 3, y, 120, 30);
        v.drag_to(x + 9, y + 1);
        v.release(120, 30);
        let (lit, text) = look::selection();
        let g = v.grid(120, 30);
        let cell = |g: &Grid, cx: usize, cy: usize| g.cells[cy * g.cols + cx].1;
        for (cx, cy) in [(x + 3, y), (x + 19, y), (x, y + 1), (x + 9, y + 1)] {
            let st = cell(&g, cx, cy);
            assert_eq!((st.bg, st.fg), (Some(lit), Some(text)), "{cx},{cy}");
        }
        for (cx, cy) in [(x + 2, y), (x + 10, y + 1), (x, y + 2)] {
            assert_ne!(cell(&g, cx, cy).bg, Some(lit), "{cx},{cy} is not covered");
        }
        // A key lets it go.
        v.press_key(plain(KeyCode::Esc));
        assert_ne!(cell(&v.grid(120, 30), x + 3, y).bg, Some(lit));
        // So does the next press.
        v.press_select(x + 3, y, 120, 30);
        v.drag_to(x + 9, y);
        assert!(v.selection.is_some());
        v.press_select(x, y + 1, 120, 30);
        assert!(v.selection.is_none());
    }

    #[test]
    fn a_press_that_never_moves_is_the_click_it_was() {
        let mut v = open_blocks();
        let (x, y) = find(&v, "On branch feat/login");
        assert!(v.press_select(x + 3, y, 120, 30));
        assert_eq!(v.release(120, 30), After::Continue, "nothing is copied");
        assert!(v.selection.is_none());
        assert_eq!(
            v.blk,
            Some((SHELL.into(), 1)),
            "the block under it is picked, as a click does"
        );
        // On what cannot be selected the press is the click at once: the sidebar, the folder line.
        assert!(!v.press_select(3, 3, 120, 30));
        let (fx, fy) = find(&v, "…/code/app-login");
        assert!(
            !v.press_select(fx + 1, fy, 120, 30),
            "the folder and time line is not text to select"
        );
    }

    #[test]
    fn a_drag_past_the_edge_of_the_list_scrolls_it_while_it_is_held() {
        let mut v = open_blocks();
        v.set_size(120, 16);
        let rows = v.grid(120, 16).rows_plain();
        let (y, row) = rows
            .iter()
            .enumerate()
            .find(|(_, r)| r.contains("❯ cargo watch"))
            .expect("the command at the bottom of the list");
        let x = row.find("cargo").map(|i| row[..i].chars().count()).unwrap();
        v.set_now(1_000);
        assert!(v.press_select(x, y, 120, 16));
        v.drag_to(x + 4, 1);
        let sel = v.selection.clone().unwrap();
        assert_eq!(sel.head.col, 0, "to the start of the first line shown");
        v.drag_tick();
        assert_eq!(
            v.scroll.get(SHELL),
            None,
            "not before it has been held a moment"
        );
        v.set_now(1_000 + DRAG_SCROLL_MS);
        v.drag_tick();
        assert_eq!(v.scroll.get(SHELL), Some(&1), "a line up");
        v.set_now(1_000 + 2 * DRAG_SCROLL_MS);
        v.drag_tick();
        assert_eq!(v.scroll.get(SHELL), Some(&2));
        assert!(matches!(v.release(120, 16), After::Copy(_)));
        // Back inside the list it stops.
        v.scroll.clear();
        assert!(v.press_select(x, y, 120, 16));
        v.drag_to(x + 4, 1);
        v.drag_to(x + 4, y);
        assert!(v.drag.as_ref().unwrap().scrolling.is_none());
    }

    fn file(
        name: &str,
        kind: FileKind,
        git: Option<GitMark>,
        ignored: bool,
    ) -> contract::FileEntry {
        contract::FileEntry {
            name: name.into(),
            kind,
            size: None,
            git,
            ignored,
        }
    }

    /// The daemon's answer for the listing of `path`.
    fn listing(ws: &str, path: &str, entries: Vec<contract::FileEntry>) -> FilesReply {
        let total = entries.len();
        FilesReply {
            ask: FileAsk::List {
                workspace: ws.into(),
                path: path.into(),
            },
            reply: Ok(Ok(Some(
                serde_json::to_value(FileListing {
                    path: path.into(),
                    entries,
                    total,
                })
                .unwrap(),
            ))),
        }
    }

    /// A view with the tree shown, the root answered: `src/`, `target/` (ignored), `Cargo.toml` (changed), `.gitignore`.
    fn tree_view() -> (View, String) {
        let mut v = fixture_view();
        let ws = v.selected_id().unwrap();
        assert_eq!(
            v.take_file_asks(),
            [FileAsk::List {
                workspace: ws.clone(),
                path: String::new()
            }],
            "the root was asked for before the tree was shown"
        );
        v.press(Key::Tree);
        assert!(
            v.take_file_asks().is_empty(),
            "showing it asks for nothing that is already on its way"
        );
        v.files_replied(listing(
            &ws,
            "",
            vec![
                file("src", FileKind::Dir, Some(GitMark::Modified), false),
                file("target", FileKind::Dir, None, true),
                file("Cargo.toml", FileKind::File, Some(GitMark::Modified), false),
                file(".gitignore", FileKind::File, None, false),
            ],
        ));
        (v, ws)
    }

    #[test]
    fn t_shows_the_tree_at_the_right_gives_it_the_keys_and_draws_the_folder_with_what_git_says() {
        let (v, _) = tree_view();
        assert!(v.tree_on && v.tree_focus);
        let text = v.plain(120, 30);
        for want in [
            "archivos",
            "▸ src/",
            "▸ target/",
            "· Cargo.toml",
            "M cambiado",
            "◌ ignorado",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        assert!(
            !text.contains(".gitignore"),
            "the hidden ones are not shown"
        );
        assert!(text.lines().last().unwrap().contains(" ARCHIVOS "));
        let (_, _, w, _) = panes_rect(120, 30);
        assert_eq!(
            v.panes_space(120, 30).2,
            w - TREE_W,
            "the panels give it room"
        );
        // The marks are at the right, in their colours.
        let g = v.grid(120, 30);
        let y = g
            .rows_plain()
            .iter()
            .position(|r| r.contains("Cargo.toml"))
            .unwrap();
        let mark = g.cells[y * 120 + 117];
        assert_eq!((mark.0, mark.1.fg), ('M', Some(look::yellow())));
    }

    /// REQ-636: the folder of the selected workspace is asked for before `t`, once, and showing the
    /// tree again asks once more without emptying what it shows.
    #[test]
    fn the_tree_is_preloaded_and_showing_it_again_refreshes_it_with_the_old_rows_on_screen() {
        let mut v = fixture_view();
        let ws = v.selected_id().unwrap();
        let root = FileAsk::List {
            workspace: ws.clone(),
            path: String::new(),
        };
        assert_eq!(v.take_file_asks(), std::slice::from_ref(&root));
        v.sync(fixture_docs().iter());
        assert!(v.take_file_asks().is_empty(), "once");
        v.files_replied(listing(
            &ws,
            "",
            vec![file("src", FileKind::Dir, None, false)],
        ));
        assert!(!v.tree_on, "nothing is shown yet");
        v.press(Key::Tree);
        assert_eq!(v.take_file_asks(), [root], "what was listed is asked again");
        let text = v.plain(120, 30);
        assert!(
            text.contains("▸ src/") && !text.contains("leyendo…"),
            "the rows of the earlier answer stay until the new one comes\n{text}"
        );
    }

    #[test]
    fn selecting_another_workspace_with_the_tree_shown_asks_for_that_workspaces_folder() {
        let (mut v, first) = tree_view();
        v.press_key(plain(KeyCode::Esc));
        v.press(Key::Back);
        v.press(Key::Down);
        let second = v.selected_id().unwrap();
        assert_ne!(first, second);
        v.sync(fixture_docs().iter());
        assert_eq!(
            v.take_file_asks(),
            [FileAsk::List {
                workspace: second,
                path: String::new()
            }]
        );
        v.sync(fixture_docs().iter());
        assert!(v.take_file_asks().is_empty(), "once");
    }

    #[test]
    fn j_k_move_enter_opens_a_folder_and_asks_for_it_and_e_opens_nothing() {
        let (mut v, ws) = tree_view();
        v.press_key(plain(KeyCode::Enter));
        assert_eq!(
            v.take_file_asks(),
            [FileAsk::List {
                workspace: ws.clone(),
                path: "src".into()
            }],
            "the folder is asked for when it opens, once"
        );
        assert!(v.plain(120, 30).contains("leyendo…"));
        v.files_replied(listing(
            &ws,
            "src",
            vec![file("main.rs", FileKind::File, None, false)],
        ));
        assert!(v.plain(120, 30).contains("▾ src/") && v.plain(120, 30).contains("main.rs"));
        v.press_key(plain(KeyCode::Enter));
        assert!(v.take_file_asks().is_empty(), "closing asks for nothing");
        v.press_key(plain(KeyCode::Char('j')));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            v.press_key(plain(KeyCode::Char('e'))),
            After::Continue,
            "no editor opens from the tree (N151)"
        );
        assert!(v.tree_focus && v.tree_on, "the keys stay in the tree");
    }

    #[test]
    fn h_closes_a_folder_or_goes_up_y_copies_the_path_and_dot_shows_the_hidden() {
        let (mut v, ws) = tree_view();
        v.press_key(plain(KeyCode::Enter));
        v.take_file_asks();
        v.files_replied(listing(
            &ws,
            "src",
            vec![file("main.rs", FileKind::File, None, false)],
        ));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            v.press_key(plain(KeyCode::Char('y'))),
            After::Copy("src/main.rs".into())
        );
        assert!(v.plain(120, 30).contains("ruta copiada"));
        v.press_key(plain(KeyCode::Char('h')));
        assert_eq!(v.trees[&ws].sel, 0, "up to the folder that holds it");
        v.press_key(plain(KeyCode::Char('h')));
        assert!(!v.plain(120, 30).contains("main.rs"), "and then it closes");
        v.press_key(plain(KeyCode::Char('.')));
        assert!(v.plain(120, 30).contains(".gitignore"));
        v.press_key(plain(KeyCode::Char('.')));
        assert!(!v.plain(120, 30).contains(".gitignore"));
    }

    #[test]
    fn a_search_asks_for_what_is_typed_and_only_what_is_typed_now_is_shown() {
        let (mut v, ws) = tree_view();
        v.press_key(plain(KeyCode::Char('/')));
        for c in ['t', 'o'] {
            v.press_key(plain(KeyCode::Char(c)));
        }
        assert_eq!(
            v.take_file_asks(),
            [
                FileAsk::Find {
                    workspace: ws.clone(),
                    query: "t".into()
                },
                FileAsk::Find {
                    workspace: ws.clone(),
                    query: "to".into()
                },
            ]
        );
        let found = |q: &str, paths: &[&str]| FilesReply {
            ask: FileAsk::Find {
                workspace: ws.clone(),
                query: q.into(),
            },
            reply: Ok(Ok(Some(
                serde_json::to_value(FoundFiles {
                    paths: paths.iter().map(|p| (*p).to_owned()).collect(),
                    cut: false,
                })
                .unwrap(),
            ))),
        };
        v.files_replied(found("t", &["a/other.rs"]));
        assert!(
            !v.plain(120, 30).contains("other.rs"),
            "the answer for «t» is old"
        );
        v.files_replied(found("to", &["src/auth/token.rs"]));
        let text = v.plain(120, 30);
        assert!(
            text.contains("/ to▏") && text.contains("src/auth/token.rs"),
            "{text}"
        );
        assert!(text.contains("1 coincidencia"), "{text}");
        let after = v.press_key(plain(KeyCode::Enter));
        assert!(
            matches!(&after, After::ReadFile { path, keep: false, .. } if path == "src/auth/token.rs"),
            "{after:?}"
        );
        assert!(v.trees[&ws].filter.is_none(), "opening ends the search");
        // esc ends it too, and a second one gives the keys back.
        let (mut v, _) = tree_view();
        v.press_key(plain(KeyCode::Char('/')));
        v.press_key(plain(KeyCode::Esc));
        assert!(v.tree_focus && v.trees.values().all(|t| t.filter.is_none()));
        v.press_key(plain(KeyCode::Esc));
        assert!(!v.tree_focus && v.tree_on);
        v.press(Key::Tree);
        assert!(v.tree_focus, "t gives the keys back");
        v.press_key(plain(KeyCode::Char('t')));
        assert!(!v.tree_on, "and closes it from inside");
    }

    #[test]
    fn a_click_in_the_tree_opens_a_folder_or_a_file_and_a_click_on_the_panels_gives_them_the_keys()
    {
        let (mut v, ws) = tree_view();
        assert_eq!(v.click(110, 2, 120, 30), After::Continue);
        assert_eq!(
            v.take_file_asks(),
            [FileAsk::List {
                workspace: ws.clone(),
                path: "src".into()
            }],
            "the first row is a folder: it opens"
        );
        // src, its note, target and then Cargo.toml.
        let after = v.click(110, 5, 120, 30);
        assert!(
            matches!(&after, After::ReadFile { path, .. } if path == "Cargo.toml"),
            "a file opens in the viewer: {after:?}"
        );
        v.press(Key::Tree);
        assert!(v.tree_focus);
        v.click(40, 8, 120, 30);
        assert!(!v.tree_focus, "a click on the panels takes the keys back");
        // The wheel scrolls the tree and not the panels.
        v.scroll(true, 110, 5, 120, 30);
        assert_eq!(v.trees[&ws].scroll, 0, "nothing to scroll in so few rows");
    }

    #[test]
    fn a_shell_pane_is_blocks_with_an_editor_under_them() {
        let t = open_blocks().plain(120, 30);
        assert!(t.contains("▌ ❯ git status"), "{t}");
        assert!(t.contains("salió 101 · 4.2s"), "{t}");
        assert!(t.contains("corriendo"), "{t}");
        assert!(t.contains("◌ cargo watch -x test"), "{t}");
        assert!(t.contains("va al programa"), "{t}");
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
        let last = rows.iter().position(|l| l.contains("[Running")).unwrap();
        let chips = rows
            .iter()
            .position(|l| l.contains("◌ cargo watch"))
            .unwrap();
        assert_eq!(
            chips - last,
            3,
            "the newest block ends, a free row of the list and one of the band, and the chips: {t}"
        );
    }

    /// The idle shell pane of the fixture: its last block ended, with what the shell holds.
    fn idle(input: Option<(&str, u32)>) -> View {
        let mut v = open_blocks();
        v.blocks
            .get_mut(BLOCKS)
            .unwrap()
            .blocks
            .last_mut()
            .unwrap()
            .running = false;
        v.panes.get_mut(SHELL).unwrap().input = input.map(|(text, cursor)| {
            Box::new(contract::PaneInput {
                text: text.into(),
                cursor,
            })
        });
        v
    }

    #[test]
    fn the_field_shows_what_the_shell_holds_after_a_green_prompt_and_asks_when_it_is_empty() {
        let t = idle(Some(("git sta", 7))).plain(120, 30);
        assert!(t.contains("❯ git sta"), "{t}");
        assert!(!t.contains("escribe un comando"), "{t}");
        let t = idle(Some(("", 0))).plain(120, 30);
        assert!(t.contains("❯ escribe un comando"), "{t}");
        assert!(t.contains("➤"), "the send mark: {t}");
        assert!(!t.contains("■ ^C"), "nothing runs to stop: {t}");
        assert!(
            t.contains("app-login"),
            "the folder is a chip over the field: {t}"
        );
    }

    #[test]
    fn a_line_that_wraps_makes_the_field_grow_and_the_list_give_way() {
        let short = idle(Some(("ls", 2))).plain(120, 30);
        let long_text = "x".repeat(200);
        let long = idle(Some((&long_text, 200))).plain(120, 30);
        let rows = |t: &str| t.lines().filter(|l| l.contains("xxxxxxxx")).count();
        assert_eq!(rows(&short), 0);
        assert!(rows(&long) >= 2, "the text wraps inside the field: {long}");
        let blocks = |t: &str| t.lines().filter(|l| l.contains("▌")).count();
        assert!(
            blocks(&long) < blocks(&short),
            "the list has fewer rows: {long}"
        );
    }

    #[test]
    fn without_the_daemons_input_the_line_of_the_screen_is_the_field() {
        let mut v = idle(None);
        let pane = v.panes.get_mut(SHELL).unwrap();
        pane.screen.lines[0] = "❯ ls -la".into();
        pane.screen.cursor.row = 0;
        pane.screen.cursor.col = 8;
        let t = v.plain(120, 30);
        assert!(t.contains("❯ ❯ ls -la") || t.contains("❯ ls -la"), "{t}");
    }

    /// The shell pane of the fixture with `claude` just started on the normal screen: the node is in
    /// mode `full`, the running block is not an alternate screen and has drawn nothing yet.
    fn claude_starting() -> View {
        let mut v = open_blocks();
        let node = v.blocks.get_mut(BLOCKS).unwrap();
        node.mode = BlocksMode::Full;
        let k = node.blocks.last_mut().unwrap();
        // A new command, as the fixture's block had painted already when it was loaded.
        k.id = 99;
        k.cmd = "claude".into();
        k.running = true;
        k.full = false;
        k.started = 1_000;
        k.out.clear();
        v.panes.get_mut(SHELL).unwrap().screen.lines[0] = "SCREEN-TEXT".into();
        v.set_now(1_500);
        v
    }

    fn paint(v: &mut View, text: &str) {
        let k = v.blocks.get_mut(BLOCKS).unwrap().blocks.last_mut().unwrap();
        k.out = vec![vec![contract::Span {
            t: text.into(),
            fg: None,
            bg: None,
            b: false,
            i: false,
            u: false,
        }]];
        v.note_painted();
    }

    #[test]
    fn a_program_that_has_the_screen_and_has_painted_nothing_is_starting_and_not_the_shells_old_screen()
     {
        let t = claude_starting().plain(120, 30);
        assert!(t.contains("iniciando claude…"), "{t}");
        assert!(t.contains("^C cancela"), "{t}");
        assert!(
            !t.contains("SCREEN-TEXT"),
            "the shell's screen as it was stays hidden: {t}"
        );
    }

    #[test]
    fn once_it_paints_its_screen_is_drawn_as_it_is_and_it_never_starts_again_for_that_command() {
        let mut v = claude_starting();
        paint(&mut v, "╭─ claude");
        let t = v.plain(120, 30);
        assert!(t.contains("SCREEN-TEXT") && !t.contains("iniciando"), "{t}");
        // It clears its screen for a moment: that is not starting over.
        let k = v.blocks.get_mut(BLOCKS).unwrap().blocks.last_mut().unwrap();
        k.out.clear();
        let t = v.plain(120, 30);
        assert!(t.contains("SCREEN-TEXT") && !t.contains("iniciando"), "{t}");
    }

    #[test]
    fn a_program_on_the_alternate_screen_or_one_that_never_paints_does_not_load_for_ever() {
        let mut v = claude_starting();
        v.blocks
            .get_mut(BLOCKS)
            .unwrap()
            .blocks
            .last_mut()
            .unwrap()
            .full = true;
        let t = v.plain(120, 30);
        assert!(
            t.contains("SCREEN-TEXT") && !t.contains("iniciando"),
            "an alternate screen is blank and its own: {t}"
        );
        let mut v = claude_starting();
        v.set_now(1_000 + LOADING_MAX_MS);
        let t = v.plain(120, 30);
        assert!(t.contains("SCREEN-TEXT") && !t.contains("iniciando"), "{t}");
    }

    /// The shell pane of the fixture with `nvim` just left: it used the alternate screen, left it, and the
    /// command has not ended, so the pane's screen is the shell's own again.
    fn nvim_closing() -> View {
        let mut v = open_blocks();
        let node = v.blocks.get_mut(BLOCKS).unwrap();
        node.mode = BlocksMode::Full;
        let k = node.blocks.last_mut().unwrap();
        k.id = 98;
        k.cmd = "nvim src/main.rs".into();
        k.running = true;
        k.full = true;
        k.out.clear();
        let pane = v.panes.get_mut(SHELL).unwrap();
        pane.screen.lines[0] = "SCREEN-TEXT".into();
        pane.screen.alt = false;
        v.set_now(5_000);
        v.note_left_alt();
        v
    }

    #[test]
    fn a_program_that_left_the_alternate_screen_and_has_not_ended_is_closing_not_the_shells_screen()
    {
        let t = nvim_closing().plain(120, 30);
        assert!(t.contains("cerrando nvim…"), "{t}");
        assert!(t.contains("esperando a que termine"), "{t}");
        assert!(
            !t.contains("SCREEN-TEXT"),
            "the shell's own screen stays hidden: {t}"
        );
    }

    #[test]
    fn a_program_on_the_alternate_screen_a_normal_one_or_one_that_stays_out_for_long_shows_its_screen()
     {
        let screen = |v: &View| {
            let t = v.plain(120, 30);
            assert!(!t.contains("cerrando"), "{t}");
            assert!(t.contains("SCREEN-TEXT"), "{t}");
        };
        let mut v = nvim_closing();
        v.panes.get_mut(SHELL).unwrap().screen.alt = true;
        v.note_left_alt();
        screen(&v);
        let mut v = nvim_closing();
        v.blocks
            .get_mut(BLOCKS)
            .unwrap()
            .blocks
            .last_mut()
            .unwrap()
            .full = false;
        v.note_left_alt();
        paint(&mut v, "its own output");
        screen(&v);
        let mut v = nvim_closing();
        v.set_now(5_000 + CLOSING_MAX_MS);
        screen(&v);
    }

    #[test]
    fn going_back_to_the_alternate_screen_starts_the_wait_over() {
        let mut v = nvim_closing();
        v.set_now(7_000);
        v.panes.get_mut(SHELL).unwrap().screen.alt = true;
        v.note_left_alt();
        v.set_now(9_000);
        v.panes.get_mut(SHELL).unwrap().screen.alt = false;
        v.note_left_alt();
        v.set_now(9_000 + CLOSING_MAX_MS - 1);
        assert!(
            v.plain(120, 30).contains("cerrando nvim…"),
            "the 3 s count from the second time"
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
    fn a_screen_with_colours_is_drawn_with_them_and_one_without_in_the_text_colour() {
        let span = |t: &str, fg, bg| contract::Span {
            t: t.into(),
            fg,
            bg,
            b: false,
            i: false,
            u: false,
        };
        let mut v = open_blocks();
        v.blocks.get_mut(BLOCKS).unwrap().mode = BlocksMode::Full;
        let pane = v.panes.get_mut(SHELL).unwrap();
        pane.screen.lines[0] = "local x".into();
        let plain = v.grid(120, 30);
        let (x, y) = find(&v, "local x");
        let at = |g: &Grid, dx: usize| g.cells[y * g.cols + x + dx].1;
        assert_eq!(at(&plain, 0).fg, Some(look::fg()), "text, as before");
        assert_eq!(at(&plain, 0).bg, None);

        let pane = v.panes.get_mut(SHELL).unwrap();
        pane.screen.alt = true;
        pane.screen.spans = vec![vec![
            span("local", Some(contract::Color::Index(1)), None),
            span(" ", None, None),
            span("x", None, Some(contract::Color::Rgb("#010203".into()))),
        ]];
        let g = v.grid(120, 30);
        assert!(g.rows_plain()[y].contains("local x"));
        assert_eq!(
            at(&g, 0).fg,
            Some(look::red()),
            "index 1 is the theme's red"
        );
        assert_eq!(at(&g, 5).fg, None, "no colour is the theme's own");
        assert_eq!(
            at(&g, 6).bg,
            Some((1, 2, 3)),
            "an RGB colour as it is asked"
        );
        assert_eq!(at(&g, 7).bg, None, "nothing spills past the row's spans");
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

    /// The cell of the first `mark` on the screen, to click it.
    fn cell_of(v: &View, mark: &str) -> (usize, usize) {
        let t = v.plain(120, 30);
        let (y, line) = t
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(mark))
            .unwrap_or_else(|| panic!("no {mark} on\n{t}"));
        (line[..line.find(mark).unwrap()].chars().count(), y)
    }

    #[test]
    fn the_stop_mark_sends_ctrl_c_and_the_send_mark_sends_the_line() {
        let mut v = open_blocks();
        let (x, y) = cell_of(&v, "■ ^C");
        assert_eq!(
            v.click(x + 1, y, 120, 30),
            After::Write {
                pane: SHELL.into(),
                data: "\x03".into()
            }
        );
        let (x, y) = cell_of(&v, "➤");
        assert_eq!(
            v.click(x, y, 120, 30),
            After::Write {
                pane: SHELL.into(),
                data: "\r".into()
            }
        );
        let mut idle = idle(Some(("", 0)));
        let (x, y) = cell_of(&idle, "➤");
        assert!(matches!(idle.click(x, y, 120, 30), After::Write { .. }));
        assert_eq!(
            idle.click(x - 4, y, 120, 30),
            After::Continue,
            "no stop mark when nothing runs"
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
        v.read.remove(BLOCKS);
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
    fn d_opens_the_diff_over_the_panes_when_there_is_no_room_beside_them_and_esc_closes_it_and_stops_watching()
     {
        let mut v = fixture_view();
        v.set_size(90, 44);
        assert!(!v.plain(90, 44).contains(" vs HEAD"));
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
        let screen = v.plain(90, 44);
        assert!(screen.contains(" vs HEAD"), "{screen}");
        assert!(screen.lines().last().unwrap().contains("DIFF"));
        assert!(screen.contains("leyendo los cambios"));

        v.changes_made(Src::Uncommitted, "the-node");
        assert_eq!(v.take_asks(), [Ask::Watch("the-node".into())]);
        // Keys are the viewer's, never a pane's.
        assert_eq!(v.press_key(plain(KeyCode::Char('x'))), After::Continue);
        v.press_key(plain(KeyCode::Esc));
        assert_eq!(v.take_asks(), [Ask::Close("the-node".into())]);
        assert!(!v.plain(90, 44).contains(" vs HEAD"));
        assert!(
            v.plain(90, 44)
                .lines()
                .last()
                .unwrap()
                .contains("WORKSPACE")
        );
    }

    #[test]
    fn clicking_the_sidebar_leaves_the_diff_and_a_click_in_it_picks_a_tab() {
        let mut v = fixture_view();
        v.set_size(90, 44);
        v.press(Key::Changes);
        v.take_asks();
        // The second tab starts after the first, which is ` 1 Sin commitear ` wide.
        v.click(SIDEBAR + 4 + 20, 2, 90, 44);
        let asks = v.take_asks();
        assert!(matches!(&asks[..], [Ask::OpenCommits { .. }]), "{asks:?}");
        v.click(3, 2, 90, 44);
        assert!(
            !v.plain(90, 44).contains(" vs HEAD"),
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
        // Now `l` moves to the panel on the right, as at the panel level.
        let focus = v.focus;
        v.press_key(plain(KeyCode::Char('l')));
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
        assert!(matches!(v.press(Key::NewPane), After::NewPane(..)));
    }

    const PROGRAMS: &str = "[[program]]\nname = \"claude\"\nkey = \"c\"\ncommand = [\"claude\"]\n\n[[program]]\nname = \"claude opus\"\nkey = \"o\"\ncommand = [\"claude\", \"--model\", \"opus\"]\n";

    fn menu_view() -> View {
        let mut v = fixture_view();
        v.set_programs(Programs::parse(PROGRAMS));
        v.press(Key::Enter);
        v
    }

    fn shift_n() -> KeyEvent {
        key(KeyCode::Char('N'), KeyModifiers::SHIFT)
    }

    #[test]
    fn shift_n_opens_the_menu_of_programs_at_the_panel_level_and_n_still_makes_a_shell() {
        let mut v = fixture_view();
        v.set_programs(Programs::parse(PROGRAMS));
        v.press_key(shift_n());
        assert!(v.launcher.is_none(), "not at the workspace level");
        v.press(Key::Enter);
        assert_eq!(v.press_key(shift_n()), After::Continue);
        assert!(v.launcher.is_some());
        let text = v.plain(120, 30);
        for want in ["panel nuevo", "shell", "claude opus", "claude --model opus"] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        v.press_key(plain(KeyCode::Esc));
        assert!(v.launcher.is_none());
        assert!(matches!(
            v.press_key(plain(KeyCode::Char('n'))),
            After::NewPane(..)
        ));
    }

    #[test]
    fn the_menu_starts_a_program_by_its_row_or_its_letter_in_the_workspace_the_keys_are_on() {
        let mut v = menu_view();
        let workspace = v.selected_id().unwrap();
        let place = v.place(None);
        assert!(place.beside.is_some(), "beside the focused panel");
        v.press_key(shift_n());
        v.press_key(plain(KeyCode::Char('j')));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::NewProgram {
                workspace: workspace.clone(),
                program: vec!["claude".into(), "--model".into(), "opus".into()],
                place: place.clone(),
            }
        );
        assert!(v.launcher.is_none(), "a pick closes the menu");
        v.press_key(shift_n());
        assert_eq!(
            v.press_key(plain(KeyCode::Char('c'))),
            After::NewProgram {
                workspace: workspace.clone(),
                program: vec!["claude".into()],
                place: place.clone(),
            }
        );
        v.press_key(shift_n());
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::NewPane(workspace, place),
            "the first row is the shell"
        );
    }

    #[test]
    fn while_the_menu_is_open_a_chord_and_the_mouse_wheel_do_nothing_and_a_click_picks() {
        let mut v = menu_view();
        v.press_key(shift_n());
        assert_eq!(
            v.press_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            After::Continue
        );
        assert!(v.launcher.is_some());
        v.scroll(true, 40, 10, 120, 30);
        assert!(v.launcher.is_some());
        let area = (SIDEBAR + 1, 1, 120 - SIDEBAR - 1, 28);
        let (bx, by, ..) = programs::frame(area, &v.programs);
        assert!(matches!(
            v.click(bx + 10, by + 3 + 1, 120, 30),
            After::NewProgram { .. }
        ));
        v.press_key(shift_n());
        assert_eq!(v.click(0, 0, 120, 30), After::Continue);
        assert!(v.launcher.is_none(), "a click outside closes it");
    }

    #[test]
    fn the_leader_then_shift_n_opens_the_menu_from_the_terminal_level() {
        let mut v = menu_view();
        v.press(Key::Enter);
        assert!(v.typing_pane().is_some());
        v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        v.press_key(shift_n());
        assert!(v.launcher.is_some());
        assert!(v.typing_pane().is_none(), "the keys are the menu's");
    }

    #[test]
    fn a_file_that_does_not_read_is_said_in_the_menu_and_the_list_stays() {
        let mut v = menu_view();
        v.set_programs(Programs::parse("[[program]]\nname = \"a\"\n"));
        v.press_key(shift_n());
        let text = v.plain(120, 30);
        assert!(
            text.contains("línea 1: [[program]] sin «command»"),
            "{text}"
        );
        assert!(text.contains("se sigue con la lista anterior"), "{text}");
        assert!(
            text.contains("claude opus"),
            "the list before stays: {text}"
        );
        v.set_programs(Programs::parse(PROGRAMS));
        assert!(!v.plain(120, 30).contains("se sigue con la lista anterior"));
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
        v.read
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
                remove_worktree: false,
                force: false
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
    fn a_worktree_with_changes_lists_them_and_asks_for_its_branch_before_removing_it_by_force() {
        let mut v = idle_view();
        close_workspace(&mut v);
        let ws = v.selected_id().unwrap();
        assert_eq!(tap(&mut v, 'd'), After::Check(ws.clone()));
        assert!(matches!(v.manage, Some(Manage::Checking(_))));
        v.check_unpushed(3);
        v.check_made("node");
        assert!(v.take_asks().contains(&Ask::Watch("node".into())));
        let (_, diff) = uncommitted(false);
        v.diffs.insert("node".into(), diff);
        v.advance_manage();
        let t = v.plain(120, 30);
        assert!(t.contains("se perdería trabajo"), "{t}");
        assert!(t.contains("Tiene 3 archivos sin commitear:"), "{t}");
        assert!(
            t.contains("3 commits sin subir se quedan en la rama feat/login."),
            "{t}"
        );
        assert!(t.contains("escribe el nombre de la rama"), "{t}");
        assert!(t.contains("❯ ▏  feat/login"), "{t}");
        // Typing is typing: `y`, `d` and `c` answer nothing, and `↵` waits for the name.
        for c in "yd".chars() {
            assert_eq!(tap(&mut v, c), After::Continue);
        }
        assert_eq!(
            v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            After::Continue
        );
        assert!(v.plain(120, 30).contains("espera a que el nombre coincida"));
        for _ in 0..2 {
            v.press_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        for c in "feat/login".chars() {
            tap(&mut v, c);
        }
        assert!(v.plain(120, 30).contains("quitar igual"));
        assert_eq!(
            v.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            After::RemoveWorkspace {
                workspace: ws,
                remove_worktree: true,
                force: true
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
        assert!(
            !t.contains("sin subir"),
            "nothing is said when all is pushed: {t}"
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
                remove_worktree: true,
                force: false
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

    /// The idle view with a third panel: `a` on the left, `b` over `c` on the right.
    fn three_panels() -> (View, [String; 3]) {
        let mut v = idle_view();
        let id = v.opened.clone().unwrap();
        let doc = v.workspaces.get_mut(&id).unwrap();
        let third = contract::PaneSummary {
            id: "third".into(),
            title: "zsh".into(),
            attention: vec![],
        };
        doc.panes.push(third);
        let [a, b, c] = [0, 1, 2].map(|i| doc.panes[i].id.clone());
        let pane = |id: &str| contract::Layout::Pane { pane: id.into() };
        doc.layout = Some(contract::Layout::Split {
            dir: SplitDir::Row,
            ratio: 500,
            a: Box::new(pane(&a)),
            b: Box::new(contract::Layout::Split {
                dir: SplitDir::Col,
                ratio: 500,
                a: Box::new(pane(&b)),
                b: Box::new(pane(&c)),
            }),
        });
        v.focus = 0;
        (v, [a, b, c])
    }

    #[test]
    fn hjkl_go_to_the_panel_on_that_side_and_nowhere_when_there_is_none() {
        let (mut v, _) = three_panels();
        v.press_key(plain(KeyCode::Char('l')));
        assert_eq!(v.focus, 1, "the upper of the two it touches");
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.focus, 2);
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.focus, 2, "nothing below");
        v.press_key(plain(KeyCode::Char('h')));
        assert_eq!(v.focus, 0);
        v.press_key(plain(KeyCode::Char('k')));
        assert_eq!(v.focus, 0);
    }

    #[test]
    fn shift_hjkl_ask_to_swap_with_the_panel_on_that_side() {
        let (mut v, [a, b, c]) = three_panels();
        let shift = |ch| key(KeyCode::Char(ch), KeyModifiers::SHIFT);
        assert_eq!(v.press_key(shift('L')), After::Swap(a.clone(), b.clone()));
        v.focus = 1;
        assert_eq!(v.press_key(shift('J')), After::Swap(b.clone(), c.clone()));
        assert_eq!(v.press_key(shift('H')), After::Swap(b, a));
        v.focus = 2;
        assert_eq!(
            v.press_key(shift('K')),
            After::Swap(c, v.shown().unwrap().panes[1].id.clone())
        );
        v.focus = 0;
        assert_eq!(v.press_key(shift('H')), After::Continue);
        assert!(
            v.plain(120, 30)
                .contains("no hay un panel en esa dirección")
        );
    }

    #[test]
    fn the_four_size_keys_ask_to_move_a_split_by_a_twentieth_toward_or_away_from_the_panel() {
        let (mut v, [a, ..]) = three_panels();
        let grow = |dir, by| After::Grow {
            pane: a.clone(),
            dir,
            by,
        };
        assert_eq!(
            v.press_key(plain(KeyCode::Char('>'))),
            grow(SplitDir::Row, 50)
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('<'))),
            grow(SplitDir::Row, -50)
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('}'))),
            grow(SplitDir::Col, 50)
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('{'))),
            grow(SplitDir::Col, -50)
        );
    }

    #[test]
    fn swapping_and_resizing_are_for_the_panel_level_only() {
        let mut v = fixture_view();
        for ch in ['H', '>', '{'] {
            assert_eq!(
                v.press_key(key(KeyCode::Char(ch), KeyModifiers::SHIFT)),
                After::Continue
            );
        }
    }

    #[test]
    fn the_daemons_layout_decides_where_the_panels_are() {
        let v = idle_view();
        let area = panes_rect(120, 30);
        let r = rects_of(&v, area);
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].1, r[0].3, r[1].1, r[1].3),
            (area.1, area.3, area.1, area.3)
        );
        assert_eq!(r[0].0 + r[0].2, r[1].0, "side by side, meeting");
        assert_eq!(r[0].2 + r[1].2, area.2);
    }

    #[test]
    fn a_workspace_with_no_layout_is_arranged_by_the_client_and_l_goes_to_the_next_panel() {
        let mut v = idle_view();
        let id = v.opened.clone().unwrap();
        v.workspaces.get_mut(&id).unwrap().layout = None;
        v.focus = 0;
        v.press(Key::Right);
        assert_eq!(v.focus, 1);
        assert_eq!(rects_of(&v, panes_rect(120, 30)).len(), 2);
    }

    #[test]
    fn bar_and_pipe_split_the_focused_panel_below_and_to_the_right_and_n_along_its_longer_side() {
        let mut v = idle_view();
        let first = v.shown().unwrap().panes[0].id.clone();
        let ws = v.selected_id().unwrap();
        let place = |dir| Place {
            beside: Some(first.clone()),
            dir,
        };
        assert_eq!(
            v.press_key(plain(KeyCode::Char('|'))),
            After::NewPane(ws.clone(), place(SplitDir::Row))
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('-'))),
            After::NewPane(ws.clone(), place(SplitDir::Col))
        );
        // A panel 45 wide and 28 high is tall: `n` splits it below. At 240 columns it is wide.
        assert_eq!(
            v.press_key(plain(KeyCode::Char('n'))),
            After::NewPane(ws.clone(), place(SplitDir::Col))
        );
        v.set_size(240, 30);
        assert_eq!(
            v.press_key(plain(KeyCode::Char('n'))),
            After::NewPane(ws, place(SplitDir::Row))
        );
    }

    #[test]
    fn a_daemon_with_no_layout_cannot_split_where_asked_and_the_person_is_told() {
        let mut v = idle_view();
        let id = v.opened.clone().unwrap();
        v.workspaces.get_mut(&id).unwrap().layout = None;
        for ch in ['|', '-'] {
            assert_eq!(v.press_key(plain(KeyCode::Char(ch))), After::Continue);
            assert!(v.plain(120, 30).contains("el daemon es anterior al layout"));
        }
        assert!(
            matches!(v.press_key(plain(KeyCode::Char('n'))), After::NewPane(..)),
            "n asks for a panel all the same: the daemon puts it where it does"
        );
    }

    #[test]
    fn the_panel_asked_for_takes_the_focus_when_it_appears() {
        let mut v = idle_view();
        let second = v.shown().unwrap().panes[1].id.clone();
        v.focus = 0;
        v.focus_new(&Ok(Ok(Some(serde_json::json!({ "pane": second })))));
        assert_eq!(v.focus, 0, "not before it is there to be focused");
        v.sync(fixture_docs().iter());
        assert_eq!(v.focus, 1);
        v.focus = 0;
        v.sync(fixture_docs().iter());
        assert_eq!(v.focus, 0, "only once");
        // One that never appears is forgotten, and does not take the focus when it does.
        v.set_now(1000);
        v.focus_new(&Ok(Ok(Some(serde_json::json!({ "pane": "later" })))));
        v.set_now(7000);
        v.sync(fixture_docs().iter());
        assert!(v.new_pane.is_none());
        v.focus_new(&Ok(Err(ErrorBody {
            code: contract::ErrorCode::BadRequest,
            message: "no".into(),
        })));
        assert!(v.new_pane.is_none());
    }

    #[test]
    fn tab_goes_to_the_next_panel_in_the_order_they_were_made() {
        let mut v = idle_view();
        v.focus = 0;
        assert_eq!(key_of(KeyCode::Tab), Some(Key::Next));
        v.press(Key::Next);
        assert_eq!(v.focus, 1);
    }

    #[test]
    fn j_and_k_move_the_workspaces_at_the_first_level_and_the_panels_at_the_second() {
        let mut v = fixture_view();
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!((v.selected, v.level()), (1, Level::Workspaces));
        v.press_key(plain(KeyCode::Char('k')));
        assert_eq!(v.selected, 0);
        v.press_key(plain(KeyCode::Enter));
        v.press_key(plain(KeyCode::Char('l')));
        assert_eq!(
            (v.selected, v.focus),
            (0, 1),
            "the panel on the right, not the next workspace"
        );
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.focus, 1, "nothing is below it");
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
        assert!(matches!(v.press(Key::NewPane), After::NewPane(..)));
        let area = panes_rect(120, 30);
        let two = rects_of(&v, area);
        assert_ne!(two[0], area, "two panels share the area");
        v.press(Key::Full);
        let full = rects_of(&v, area);
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
        assert_eq!(rects_of(&v, area), two);
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
        assert!(t.contains("renombrar el worktree"), "{t}");
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
    fn a_paste_into_a_box_of_the_client_types_into_it_and_a_line_break_is_enter() {
        let mut v = fixture_view();
        assert_eq!(tap(&mut v, 'r'), After::Continue);
        let ws = v.selected_id().unwrap();
        assert_eq!(
            v.paste("-x\r\nsobra"),
            After::Rename {
                target: Named::Workspace(ws),
                name: "feat/login-x".into()
            },
            "the Enter of the paste confirms and what follows it is not typed"
        );
        assert!(v.manage.is_none());
    }

    #[test]
    fn s_pushes_the_worktree_and_waits_with_a_box_that_takes_every_key() {
        let mut v = fixture_view();
        let ws = v.selected_id().unwrap();
        assert_eq!(tap(&mut v, 's'), After::Sync { workspace: ws });
        let t = v.plain(120, 30);
        assert!(t.contains("subir el worktree"), "{t}");
        assert!(t.contains("git push origin feat/login"), "{t}");
        assert!(t.contains("subiendo feat/login…"), "{t}");
        // Nothing else is done while git runs, and a second push is not started.
        for c in ['x', 'r', 's', 'j'] {
            assert_eq!(tap(&mut v, c), After::Continue);
        }
        assert!(matches!(v.manage, Some(Manage::Syncing { .. })));
    }

    /// REQ-604: `esc` while the push waits asks the daemon to stop it (once), the box says so, and
    /// the answer «cancelado» is said as a line and not explained as a failure.
    #[test]
    fn esc_stops_the_push_that_waits_and_what_it_answers_is_not_a_failure() {
        let mut v = fixture_view();
        let ws = v.selected_id().unwrap();
        assert_eq!(
            tap(&mut v, 's'),
            After::Sync {
                workspace: ws.clone()
            }
        );
        assert!(v.plain(120, 30).contains("esc cancela"));
        assert_eq!(
            v.press_key(plain(KeyCode::Esc)),
            After::CancelOp {
                workspace: ws.clone()
            }
        );
        let t = v.plain(120, 30);
        assert!(t.contains("cancelando feat/login…"), "{t}");
        assert_eq!(
            v.press_key(plain(KeyCode::Esc)),
            After::Continue,
            "asked once: the reply gives the screen back"
        );
        v.sync_done(Ok(Err(ErrorBody {
            code: contract::ErrorCode::GitFailed,
            message: "cancelado".into(),
        })));
        assert!(v.manage.is_none(), "no box of a failure");
        assert_eq!(v.flash, "subida de feat/login cancelada");
        // a push that failed for another reason is explained as before, whether or not esc was pressed
        tap(&mut v, 's');
        v.press_key(plain(KeyCode::Esc));
        v.sync_done(Ok(Err(ErrorBody {
            code: contract::ErrorCode::GitFailed,
            message: "rejected".into(),
        })));
        assert!(matches!(v.manage, Some(Manage::Failed { .. })));
    }

    #[test]
    fn what_a_push_says_when_it_worked() {
        let said = |pushed: u64, new: bool| {
            let mut v = fixture_view();
            tap(&mut v, 's');
            v.sync_done(Ok(Ok(Some(serde_json::json!({
                "branch": "feat/login", "pushed": pushed, "new_branch": new
            })))));
            assert!(v.manage.is_none());
            v.flash.clone()
        };
        assert_eq!(
            said(3, true),
            "subido feat/login · 3 commits · rama nueva en origin"
        );
        assert_eq!(said(1, false), "subido feat/login · 1 commit a origin");
        assert_eq!(
            said(0, false),
            "feat/login · al día con origin, nada que subir"
        );
    }

    #[test]
    fn a_push_that_failed_says_why_and_any_key_closes_it() {
        let mut v = fixture_view();
        tap(&mut v, 's');
        v.sync_done(Ok(Err(ErrorBody {
            code: contract::ErrorCode::GitFailed,
            message: "! [rejected] feat/login -> feat/login (non-fast-forward)".into(),
        })));
        let t = v.plain(120, 30);
        assert!(t.contains("No se pudo subir feat/login"), "{t}");
        assert!(t.contains("non-fast-forward"), "{t}");
        assert_eq!(tap(&mut v, 'x'), After::Continue);
        assert!(
            v.manage.is_none(),
            "any key closes it and does nothing else"
        );
    }

    #[test]
    fn s_on_a_folder_says_it_is_a_folder_and_pushes_nothing() {
        let mut v = fixture_view();
        v.session.as_mut().unwrap().workspaces[v.selected].kind = WorkspaceKind::Folder;
        assert_eq!(tap(&mut v, 's'), After::Continue);
        assert!(v.manage.is_none());
        assert!(
            v.plain(120, 30)
                .contains("es una carpeta: s sube un worktree")
        );
    }

    #[test]
    fn s_works_from_the_panel_level_too() {
        let mut v = idle_view();
        assert_eq!(v.level(), Level::Panels);
        assert!(matches!(tap(&mut v, 's'), After::Sync { .. }));
    }

    #[test]
    fn r_does_not_ask_for_the_name_of_a_folder() {
        let mut v = fixture_view();
        let ws = v.session.as_mut().unwrap();
        ws.workspaces[v.selected].kind = WorkspaceKind::Folder;
        assert_eq!(tap(&mut v, 'r'), After::Continue);
        assert!(v.manage.is_none(), "a folder is a directory on disk");
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

    /// What the daemon answers to `project_config`.
    fn project_reply(root: &std::path::Path, setup: Option<&[&str]>) -> Reply {
        Ok(Ok(Some(serde_json::json!({
            "source": setup.map(|_| root.join(".jw.toml")),
            "from": setup.map(|_| "repo"),
            "personal": root.join("projects/app.toml"),
            "root": root,
            "setup": setup.unwrap_or(&[]),
            "open": if setup.is_some() { "claude" } else { "shell" },
        }))))
    }

    #[test]
    fn the_box_shows_the_setup_and_the_panes_the_repository_asks_for_and_a_refusal() {
        let root = disk("wset");
        let mut v = view_on(&root);
        tap(&mut v, 'w');
        let _ = v.take_project_asks();
        v.project_replied(
            &root.join("app"),
            project_reply(
                &root.join("app-wt"),
                Some(&["bun install", "cp .env.example .env"]),
            ),
        );
        let t = v.plain(120, 30);
        assert!(t.contains("setup     bun install"), "{t}");
        assert!(t.contains("          cp .env.example .env"), "{t}");
        assert!(t.contains("[claude]") && t.contains("tras el setup"), "{t}");
        assert!(!t.contains("[shell]"), "{t}");
        assert!(t.contains("según el .jw.toml de app"), "{t}");
        // `tab` changes what this worktree opens with, and `↵` says it (REQ-510).
        v.press_key(plain(KeyCode::Tab));
        let t = v.plain(120, 30);
        assert!(t.contains("[shell]") && !t.contains("[claude]"), "{t}");
        // An answer for another repository is not this box's.
        v.project_replied(
            &root.join("other"),
            Ok(Err(contract::ErrorBody {
                code: contract::ErrorCode::BadRequest,
                message: "x".into(),
            })),
        );
        assert!(v.plain(120, 30).contains("abre      "));
        // A repository with no file: its panes and a line that says so.
        let mut v = view_on(&root);
        tap(&mut v, 'w');
        v.project_replied(&root.join("app"), project_reply(&root.join("app-wt"), None));
        let t = v.plain(120, 30);
        assert!(t.contains("[shell]"), "{t}");
        assert!(t.contains("sin configuración: como siempre"), "{t}");
        // The person's own file says so (REQ-488).
        let mut v = view_on(&root);
        tap(&mut v, 'w');
        v.project_replied(
            &root.join("app"),
            Ok(Ok(Some(serde_json::json!({
                "source": root.join("projects/app.toml"),
                "from": "personal",
                "personal": root.join("projects/app.toml"),
                "root": root.join("app-wt"),
                "setup": [],
                "open": "shell",
            })))),
        );
        assert!(v.plain(120, 30).contains("según tu configuración de app"));
        // The daemon's refusal is the box's whole content and ↵ does nothing.
        let mut v = view_on(&root);
        tap(&mut v, 'w');
        v.project_replied(
            &root.join("app"),
            Ok(Err(contract::ErrorBody {
                code: contract::ErrorCode::BadRequest,
                message: ".jw.toml, line 3: «{nme}» is no marker (use {name})".into(),
            })),
        );
        let t = v.plain(120, 30);
        assert!(t.contains(".jw.toml, line 3"), "{t}");
        assert!(!t.contains("y abrir"), "{t}");
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert!(v.worktree.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    fn worktree_infos(root: &std::path::Path) -> serde_json::Value {
        let at = |name: &str| root.join("app-wt").join(name).display().to_string();
        serde_json::json!({ "worktrees": [
            { "path": at("login"), "branch": "feat/login", "head": "4f2a9c1", "workspace": "w1" },
            { "path": at("fix-login"), "branch": "fix/login", "head": "9a8b7c6", "changes": 3 },
            { "path": at("bisect"), "head": "1d2e3f4", "changes": 0 },
            { "path": at("old"), "branch": "old", "head": "aa00bb1", "missing": true },
        ]})
    }

    /// REQ-533 to REQ-535: the box lists the worktrees git has on its second side, opens the
    /// highlighted one, the marked ones or every closed one, and says what it did.
    #[test]
    fn the_box_lists_the_worktrees_that_exist_and_opens_them_one_marked_or_all() {
        let root = disk("existing");
        let mut v = view_on(&root);
        let app = root.join("app");
        tap(&mut v, 'w');
        assert!(
            v.take_worktree_asks().is_empty(),
            "git reads the status of each worktree: the list waits for its side"
        );
        assert!(v.plain(120, 30).contains("existentes"));
        v.press_key(plain(KeyCode::Right));
        assert_eq!(
            v.take_worktree_asks(),
            std::slice::from_ref(&app),
            "the first time the side is shown the box asks for the list"
        );
        v.press_key(plain(KeyCode::Left));
        v.press_key(plain(KeyCode::Right));
        assert!(v.take_worktree_asks().is_empty(), "and only that time");
        assert!(v.plain(120, 30).contains("leyendo los worktrees…"));
        v.worktrees_replied(&app, Ok(Ok(Some(worktree_infos(&root)))));
        let t = v.plain(120, 30);
        assert!(
            t.contains("worktrees de app") && t.contains("[existentes 4]"),
            "{t}"
        );
        for want in [
            "feat/login",
            "abierto",
            "3 cambios",
            "(1d2e3f4)",
            "sin rama",
            "carpeta no existe",
            "2 cerrados",
        ] {
            assert!(t.contains(want), "{want}: {t}");
        }
        // The first row is open: ↵ says so and opens nothing; so does the one whose folder is gone.
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert!(v.plain(120, 30).contains("ya está abierto"));
        let path = |name: &str| root.join("app-wt").join(name).display().to_string();
        // ↵ opens the highlighted one and the box stays.
        v.press_key(plain(KeyCode::Down));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::OpenWorktrees {
                repo: app.clone(),
                paths: vec![path("fix-login")]
            }
        );
        assert!(v.worktree.is_some());
        // Marking two and ↵ opens both; marking what cannot be opened does nothing.
        tap(&mut v, ' ');
        v.press_key(plain(KeyCode::Down));
        tap(&mut v, ' ');
        v.press_key(plain(KeyCode::Down));
        tap(&mut v, ' ');
        assert!(v.plain(120, 30).contains("2 marcados"));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::OpenWorktrees {
                repo: app.clone(),
                paths: vec![path("fix-login"), path("bisect")]
            }
        );
        // `a` opens every closed one, but never a missing one or one that is open.
        assert_eq!(
            tap(&mut v, 'a'),
            After::OpenWorktrees {
                repo: app.clone(),
                paths: vec![path("fix-login"), path("bisect")]
            }
        );
        // What opening did is said, and the list is asked for again.
        let _ = v.take_worktree_asks();
        v.worktrees_opened(&app, 2, &[]);
        assert!(
            v.plain(120, 30)
                .contains("abiertos 2, sin setup ni copiar archivos")
        );
        assert_eq!(v.take_worktree_asks(), std::slice::from_ref(&app));
        v.worktrees_opened(&app, 1, &["no se pudo".to_owned()]);
        assert!(v.plain(120, 30).contains("abiertos 1; 1 no: no se pudo"));
        // ← goes back to the new worktree, with its own keys.
        v.press_key(plain(KeyCode::Left));
        assert!(v.plain(120, 30).contains("worktree nuevo de app"));
        // A refusal of the daemon is the whole content of the list.
        v.press_key(plain(KeyCode::Right));
        v.worktrees_replied(
            &app,
            Ok(Err(contract::ErrorBody {
                code: contract::ErrorCode::GitFailed,
                message: "not a git repository".into(),
            })),
        );
        assert!(v.plain(120, 30).contains("not a git repository"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn w_makes_a_worktree_of_the_project_and_only_of_a_repository() {
        let root = disk("w");
        let mut v = view_on(&root);
        // The first workspace is a worktree of `app`, on `feat/login`.
        assert_eq!(tap(&mut v, 'w'), After::Continue);
        // The box asks the daemon for the repository's settings and waits for them.
        assert_eq!(v.take_project_asks(), [root.join("app")]);
        let t = v.plain(120, 30);
        assert!(t.contains("worktree nuevo de app"), "{t}");
        assert!(t.contains("leyendo .jw.toml…"), "{t}");
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert!(v.worktree.is_some(), "↵ does nothing while it waits");
        v.project_replied(&root.join("app"), project_reply(&root.join("app-wt"), None));
        let t = v.plain(120, 30);
        assert!(t.contains("nombre    ws-1▏"), "{t}");
        assert!(t.contains("rama      ws-1"), "{t}");
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
                branch: "fix".into(),
                from: "feat/login".into(),
                folder: "fix".into(),
                open: contract::OpenKind::Shell,
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
    fn p_opens_the_settings_of_the_project_and_saves_what_changed_to_the_daemon() {
        let root = disk("p");
        let mut v = view_on(&root);
        assert_eq!(tap(&mut v, 'p'), After::Continue);
        assert_eq!(v.take_project_asks(), [root.join("app")]);
        let t = v.plain(120, 40);
        assert!(t.contains("proyecto · app"), "{t}");
        assert!(t.contains("leyendo la configuración"), "{t}");
        v.project_replied(
            &root.join("app"),
            project_reply(&root.join("app-wt"), Some(&["make deps"])),
        );
        let t = v.plain(120, 40);
        for shown in [
            "carpeta de worktrees",
            "make deps",
            "al abrir",
            "[claude]",
            "[repositorio]",
            "cambia el .jw.toml que ve el equipo",
            "sin cambios",
        ] {
            assert!(t.contains(shown), "{shown}\n{t}");
        }
        // The keys go to the screen while it is open: `al abrir` changes with →, and `s` saves.
        for _ in 0..4 {
            v.press_key(plain(KeyCode::Down));
        }
        v.press_key(plain(KeyCode::Right));
        assert!(v.plain(120, 40).contains("hay cambios sin guardar"));
        assert_eq!(
            v.press_key(plain(KeyCode::Char('s'))),
            After::SaveProject {
                repo: root.join("app"),
                to: contract::ConfigFrom::Repo,
                set: contract::ProjectEdit {
                    open: Some(contract::OpenKind::Shell),
                    ..Default::default()
                },
            }
        );
        // What the daemon answers is told, and the settings are read again.
        v.project_saved(
            &root.join("app"),
            Ok(Ok(Some(serde_json::json!({
                "path": root.join("app/.jw.toml"),
                "shadowed": false,
            })))),
        );
        assert_eq!(v.take_project_asks(), [root.join("app")]);
        assert!(
            v.plain(120, 40).contains("guardado en"),
            "{}",
            v.plain(120, 40)
        );
        // The new answer replaces the screen's values, so there is nothing left to lose: esc closes it.
        v.project_replied(
            &root.join("app"),
            project_reply(&root.join("app-wt"), Some(&["make deps"])),
        );
        // A save the daemon refused is told and keeps what was typed: nothing is read again.
        v.press_key(plain(KeyCode::Right));
        v.project_saved(
            &root.join("app"),
            Ok(Err(contract::ErrorBody {
                code: contract::ErrorCode::BadRequest,
                message: ".jw.toml, line 2: bad".into(),
            })),
        );
        assert!(v.take_project_asks().is_empty());
        let t = v.plain(120, 40);
        assert!(t.contains("no se pudo guardar: .jw.toml, line 2"), "{t}");
        assert!(t.contains("hay cambios sin guardar"), "{t}");
        v.press_key(plain(KeyCode::Right));
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.project.is_none());
        // A folder that is no repository has no settings of its own.
        v.press(Key::Back);
        v.press(Key::Down);
        tap(&mut v, 'p');
        assert!(v.project.is_none());
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

    /// A daemon of its own in `/tmp`, with a repository `app` that has one commit.
    fn daemon_with_repo(name: &str, jw: &str) -> (std::path::PathBuf, daemon::Daemon, Client) {
        let dir = std::path::PathBuf::from(format!("/tmp/jwn-tu-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.join("app"))
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(st.success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "i"]);
        std::fs::write(dir.join("app/.jw.toml"), jw).unwrap();
        let config = daemon::Config {
            socket: dir.join("s.sock"),
            state_dir: dir.join("state"),
            shell: Some("/bin/sh".into()),
            tcp: None,
            env: Vec::new(),
            blocks: true,
            link: daemon::LinkConfig::off(),
            hook_exe: None,
            gh: None,
            github_every: Duration::from_millis(100),
            conversation: false,
            projects: dir.join("projects"),
            chrome: false,
        };
        let d = daemon::Daemon::start(config.clone()).unwrap();
        let c = Client::connect(&config.socket, "test").unwrap();
        (dir, d, c)
    }

    fn panes_of(c: &mut Client, ws: &str) -> usize {
        c.open(ws).unwrap();
        assert!(
            c.wait_until(Duration::from_secs(5), |c| c.nodes.contains_key(ws))
                .unwrap()
        );
        c.nodes[ws]["panes"].as_array().unwrap().len()
    }

    /// REQ-610, REQ-613, REQ-621: through a real daemon, a text file opens in the viewer with its
    /// text, a binary one shows its refusal with the weight the tree knew, and a missing one says so.
    #[test]
    fn read_file_through_a_real_daemon_opens_the_viewer_the_refusal_or_the_daemons_reason() {
        let (dir, d, mut c) = daemon_with_repo("readfile", "");
        let repo = dir.join("app");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("src/main.rs"),
            "fn main() {\n    println!(\"hola\");\n}\n",
        )
        .unwrap();
        std::fs::write(repo.join("logo.png"), b"\x89PNG\0\0\0IHDR").unwrap();
        let reply = c.cmd(Cmd::OpenFolder {
            path: repo.to_string_lossy().into_owned(),
        });
        let ws = reply.unwrap().unwrap().unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut v = fixture_view();

        read_file(
            &mut v,
            &mut c,
            ws.clone(),
            "src/main.rs".into(),
            None,
            false,
        );
        let screen = v.plain(120, 30);
        assert!(
            screen.contains("▤ main.rs · 3 líneas") && screen.contains("println!(\"hola\");"),
            "{screen}"
        );
        assert!(
            matches!(&v.viewer, Some(Viewing::Text(t)) if t.current().workspace == ws && t.current().path().ends_with("src/main.rs"))
        );

        read_file(
            &mut v,
            &mut c,
            ws.clone(),
            "logo.png".into(),
            Some(48 * 1024),
            false,
        );
        assert!(
            v.plain(120, 30)
                .contains("binario · 48.0 KB · no se puede mostrar"),
            "{}",
            v.plain(120, 30)
        );

        v.viewer = None;
        read_file(&mut v, &mut c, ws, "no-hay.rs".into(), None, false);
        assert!(v.viewer.is_none());
        assert!(v.flash.contains("no file no-hay.rs"), "{}", v.flash);
        d.stop();
    }

    #[test]
    fn a_new_worktree_gets_the_panes_of_its_repository_and_when_that_fails_one_shell() {
        let (dir, d, mut c) = daemon_with_repo("start", "setup = [\"true\"]\n");
        let repo = dir.join("app");
        let make = |c: &mut Client, name: &str| {
            c.cmd(Cmd::CreateWorktree {
                repo: repo.to_string_lossy().into_owned(),
                branch: format!("feat/{name}"),
                base: None,
                folder: Some(name.to_owned()),
            })
        };
        let mut v = fixture_view();

        // With a setup the one panel runs it first (REQ-507).
        let reply = make(&mut c, "one");
        let ws = reply.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        shown_in_a_pane(&mut v, &mut c, reply, "x", None);
        assert_eq!(panes_of(&mut c, &ws), 1);
        assert!(v.manage.is_none(), "nothing failed");
        assert!(v.goto.is_some(), "and the client goes there");

        // A file that went bad after the worktree was made: the message is shown and the workspace still gets a shell.
        let reply = make(&mut c, "two");
        std::fs::write(repo.join(".jw.toml"), "setup = [").unwrap();
        let ws = reply.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        shown_in_a_pane(&mut v, &mut c, reply, "x", None);
        assert!(
            matches!(&v.manage, Some(Manage::Failed { text, .. }) if text.contains(".jw.toml")),
            "the message is shown"
        );
        assert_eq!(panes_of(&mut c, &ws), 1);

        // A folder that is the repository's top level has its settings but never their setup
        // (REQ-500): with no layout it is one shell.
        std::fs::write(repo.join(".jw.toml"), "setup = [\"true\"]\n").unwrap();
        v.manage = None;
        let reply = c.cmd(Cmd::OpenFolder {
            path: repo.to_string_lossy().into_owned(),
        });
        let ws = reply.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        shown_in_a_pane(&mut v, &mut c, reply, "x", None);
        assert_eq!(panes_of(&mut c, &ws), 1);
        assert!(v.manage.is_none(), "nothing failed");

        // A folder whose settings cannot be read: the message is shown and it is one shell
        // all the same (REQ-503).
        std::fs::write(repo.join(".jw.toml"), "[layout]\nrun = \n").unwrap();
        let plain = dir.join("plain-folder");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join(".jw.toml"), "[layout]\nrun = \n").unwrap();
        let reply = c.cmd(Cmd::OpenFolder {
            path: plain.to_string_lossy().into_owned(),
        });
        let ws = reply.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        shown_in_a_pane(&mut v, &mut c, reply, "x", None);
        assert!(
            matches!(&v.manage, Some(Manage::Failed { text, .. }) if text.contains(".jw.toml")),
            "the message is shown"
        );
        assert_eq!(panes_of(&mut c, &ws), 1);
        d.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    const CONV_PANE: &str = "00000000-0000-4000-8000-0000000000c1";
    const CONV: &str = "00000000-0000-4000-8000-0000000000e1";

    /// The shared fixtures with the first pane running a conversation; `idle` takes away the
    /// permission that waits in the fixture. The keyboard is in the conversation.
    fn conv_view(idle: bool) -> View {
        conv_view_older(idle, None)
    }

    /// The same, with a conversation whose node says where to ask for older items from.
    fn conv_view_older(idle: bool, older: Option<u64>) -> View {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let load = |n: &str| {
            serde_json::from_str::<serde_json::Value>(
                &std::fs::read_to_string(dir.join(n)).unwrap(),
            )
            .unwrap()["node"]
                .clone()
        };
        let mut docs = fixture_docs();
        docs.retain(|d| d["id"] != CONV_PANE);
        docs.push(load("snapshot-pane-conversation.json"));
        let mut conv = load("snapshot-conversation.json");
        if idle {
            conv["ask"] = serde_json::Value::Null;
            conv["status"] = "idle".into();
        }
        if let Some(at) = older {
            conv["older"] = at.into();
        }
        docs.push(conv);
        let mut v = View::new("fixtures");
        v.set_size(120, 40);
        v.sync(docs.iter());
        v.press(Key::Enter);
        v.press(Key::Enter);
        assert_eq!(v.typing_pane().as_deref(), Some(CONV_PANE));
        // The tree's folder was asked for as the workspace was selected (REQ-636); the tests below
        // count what the conversation asks.
        v.take_file_asks();
        v
    }

    /// REQ-567: a conversation scrolled to its top asks for the items before the first, once; the
    /// answer goes before the others and the row above them says where the conversation starts.
    #[test]
    fn a_conversation_at_its_top_asks_for_older_items_once_and_draws_them_before_the_others() {
        let mut v = conv_view_older(true, Some(1_792_109_990_000));
        v.ask_older();
        assert!(
            v.take_file_asks().is_empty(),
            "nothing was drawn, so nothing is at its top"
        );
        v.plain(120, 40);
        v.ask_older();
        assert!(
            v.take_file_asks().is_empty(),
            "the newest is in view, not the top"
        );
        for _ in 0..100 {
            v.scroll(false, 50, 10, 120, 40);
        }
        let screen = v.plain(120, 40);
        assert!(screen.contains("hay anteriores"), "{screen}");
        v.ask_older();
        let asks = v.take_file_asks();
        let ask = FileAsk::Older {
            node: CONV.into(),
            before: 1_792_109_990_000,
        };
        assert_eq!(asks, std::slice::from_ref(&ask));
        v.ask_older();
        assert!(v.take_file_asks().is_empty(), "one is out");
        let first_before = v.convs[CONV].first;
        let len_before = v.convs[CONV].items.len();
        v.files_replied(FilesReply {
            ask,
            reply: Ok(Ok(Some(serde_json::json!({"items": [
                {"t": "user", "text": "pregunta de antes"},
                {"t": "text", "text": "respuesta de antes"}
            ]})))),
        });
        assert_eq!(v.convs[CONV].items.len(), len_before + 2);
        assert_eq!(
            v.convs[CONV].first, first_before,
            "the number of the first item is the same"
        );
        // the list is longer now: it is drawn once to know by how much, and then scrolled
        v.plain(120, 40);
        for _ in 0..100 {
            v.scroll(false, 50, 10, 120, 40);
        }
        let screen = v.plain(120, 40);
        assert!(screen.contains("inicio de la conversación"), "{screen}");
        assert!(screen.contains("respuesta de antes"), "{screen}");
        v.ask_older();
        assert!(
            v.take_file_asks().is_empty(),
            "the start of the file was reached"
        );
    }

    #[test]
    fn a_pane_with_a_conversation_draws_it_with_its_name_and_asks_for_its_node() {
        let mut v = fixture_view();
        let mut docs = fixture_docs();
        docs.retain(|d| d["id"] != CONV_PANE);
        let pane: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../contract/fixtures/daemon/snapshot-pane-conversation.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let mut node = pane["node"].clone();
        node["rev"] = 99.into();
        docs.push(node);
        v.sync(docs.iter());
        v.press(Key::Enter);
        assert_eq!(
            v.wants(),
            [CONV],
            "the node of the conversation is asked for"
        );
        assert!(
            !v.plain(120, 40).contains("permiso · Edit"),
            "until it arrives nothing of it is drawn"
        );
        let v = conv_view(false);
        // Two rows taller than the conversation was: the message that waits is drawn too (REQ-549).
        let screen = v.plain(120, 42);
        assert!(
            screen.contains("claude · arregla el test que falla e…"),
            "{screen}"
        );
        assert!(screen.contains("▸ Leyó 1 archivo"), "{screen}");
        assert!(screen.contains("permiso · Edit"), "{screen}");
        assert!(v.wants().is_empty());
    }

    #[test]
    fn typed_in_a_conversation_goes_to_it_and_never_to_the_shell() {
        let mut v = conv_view(true);
        for c in "hola".chars() {
            assert_eq!(v.press_key(plain(KeyCode::Char(c))), After::Continue);
        }
        assert!(v.plain(120, 40).contains("❯ hola"));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Conv(ConvAct::Send {
                node: CONV.into(),
                text: "hola".into(),
                images: vec![]
            })
        );
        // the leader gives the keyboard back
        assert_eq!(
            v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            After::Continue
        );
        assert!(v.typing_pane().is_none());
    }

    #[test]
    fn a_permission_is_answered_from_the_keyboard_and_says_where_it_came_from() {
        let mut v = conv_view(false);
        assert_eq!(
            v.press_key(plain(KeyCode::Char('x'))),
            After::Continue,
            "no other key reaches the shell"
        );
        assert_eq!(
            v.press_key(plain(KeyCode::Char('y'))),
            After::Conv(ConvAct::Answer {
                node: CONV.into(),
                ask: "req-1".into(),
                key: "yes".into(),
                text: None,
            })
        );
        let bar = v.plain(120, 40);
        assert!(bar.contains(" PERMISO "), "{bar}");
        let v = conv_view(true);
        assert!(v.plain(120, 40).contains(" CLAUDE "));
    }

    #[test]
    fn a_paste_goes_into_the_conversations_line_and_a_shell_gets_its_lines_as_enters() {
        let mut v = conv_view(true);
        assert_eq!(v.paste("uno\r\ndos"), After::Continue);
        assert_eq!(v.conv_ui[CONV].editor.text(), "uno\ndos");
        // the second pane is a shell
        v.press_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        v.press(Key::Right);
        v.press(Key::Enter);
        let pane = v.typing_pane().unwrap();
        assert_ne!(pane, CONV_PANE);
        assert_eq!(
            v.paste("ls\nls -la"),
            After::Write {
                pane,
                data: "ls\rls -la".into()
            }
        );
    }

    #[test]
    fn the_wheel_scrolls_a_conversation_and_a_click_in_its_footer_opens_the_list() {
        let mut v = conv_view(true);
        v.set_size(120, 24);
        v.grid(120, 24);
        let (x, y) = find_in(&v, "sonnet-5.5", 24);
        assert_eq!(v.click(x + 1, y, 120, 24), After::Continue);
        assert!(v.conv_ui[CONV].has_pop());
        assert!(v.plain(120, 24).contains("fable"), "{}", v.plain(120, 24));
        v.press_key(plain(KeyCode::Esc));
        let (x, y) = find_in(&v, "Listo en 11 s", 24);
        v.scroll(false, x, y, 120, 24);
        v.scroll(true, x, y, 120, 24);
    }

    #[test]
    fn a_drag_in_a_conversation_copies_what_it_covers_and_a_key_or_a_press_lets_it_go() {
        let mut v = conv_view(true);
        let (x, y) = find_in(&v, "mirar el test", 40);
        assert!(v.press_select(x, y, 120, 40), "a drag begins in the list");
        v.drag_to(x + 12, y);
        let lit = |v: &View| {
            let g = v.grid(120, 40);
            g.cells[y * g.cols + x].1.bg == Some(look::selection().0)
        };
        assert!(lit(&v), "what it covers is lit");
        assert_eq!(v.release(120, 40), After::Copy("mirar el test".into()));
        assert!(v.flash.starts_with("copiado · 1 línea"), "{}", v.flash);
        assert!(lit(&v), "it stays lit");
        v.press_key(plain(KeyCode::Char('a')));
        assert!(!lit(&v), "a key lets go of it");
        // a press that does not move is a click: it picks the item
        assert!(v.press_select(x, y, 120, 40));
        assert_eq!(v.release(120, 40), After::Continue);
        assert!(
            v.grid(120, 40).rows_plain()[y].contains('▌'),
            "the click picked it"
        );
        // and a press outside the list is the click it always was
        assert!(!v.press_select(x, 39, 120, 40));
    }

    /// REQ-637: two presses on one cell, close in time, are a double click; a third begins again.
    #[test]
    fn two_presses_on_one_cell_within_the_time_are_a_double_click() {
        let mut v = conv_view(true);
        assert!(!v.double_press(5, 5), "the first");
        assert!(v.double_press(5, 5), "the second");
        assert!(!v.double_press(5, 5), "a third is the first of the next");
        assert!(!v.double_press(6, 5), "another cell");
        v.now_ms += DOUBLE_CLICK_MS + 1;
        assert!(!v.double_press(6, 5), "too late");
        v.now_ms += 10;
        assert!(v.double_press(6, 5));
    }

    /// REQ-637: a double click in a conversation's list copies the word.
    #[test]
    fn a_double_click_in_a_conversation_selects_the_word_and_copies_it() {
        let mut v = conv_view(true);
        let (x, y) = find_in(&v, "mirar el test", 40);
        let at = x + 10;
        assert!(!v.double_press(at, y));
        assert!(v.press_select(at, y, 120, 40));
        assert_eq!(
            v.release(120, 40),
            After::Continue,
            "the first press is a click"
        );
        assert!(v.double_press(at, y));
        assert!(v.press_select(at, y, 120, 40));
        v.widen_to_word();
        assert_eq!(v.release(120, 40), After::Copy("test".into()));
        assert!(v.flash.starts_with("copiado"), "{}", v.flash);
        let g = v.grid(120, 40);
        assert_eq!(
            g.cells[y * g.cols + x + 10].1.bg,
            Some(look::selection().0),
            "it is lit"
        );
    }

    /// REQ-637: the same in a terminal's list of blocks.
    #[test]
    fn a_double_click_on_the_output_of_a_terminal_copies_the_word() {
        let mut v = open_blocks();
        let (x, y) = find(&v, "On branch feat/login");
        assert!(!v.double_press(x + 5, y));
        assert!(v.press_select(x + 5, y, 120, 30));
        v.release(120, 30);
        assert!(v.double_press(x + 5, y));
        assert!(v.press_select(x + 5, y, 120, 30));
        v.widen_to_word();
        assert_eq!(v.release(120, 30), After::Copy("branch".into()));
    }

    /// REQ-637: a double click on a word of the field selects it, and what is typed next takes its place.
    #[test]
    fn a_double_click_in_the_field_selects_the_word_under_it() {
        let mut v = conv_view(true);
        for c in "hola ñandú mundo".chars() {
            v.press_key(plain(KeyCode::Char(c)));
        }
        let (x, y) = find_in(&v, "hola ñandú mundo", 40);
        let at = x + 7;
        assert!(
            !v.select_word_in_field(x + 4, y, 120, 40),
            "a space has no word"
        );
        assert!(v.select_word_in_field(at, y, 120, 40));
        v.press_key(plain(KeyCode::Char('X')));
        let screen = v.plain(120, 40);
        assert!(screen.contains("hola X mundo"), "{screen}");
        assert!(
            !v.select_word_in_field(x, 0, 120, 40),
            "above the field is not in it"
        );
    }

    fn plan_ask() -> contract::conversation::Ask {
        use contract::conversation::{Ask, AskKind, AskOption};
        let option = |key: &str, label: &str| AskOption {
            key: key.into(),
            label: label.into(),
        };
        Ask {
            id: "req-p".into(),
            kind: AskKind::Plan,
            tool: "ExitPlanMode".into(),
            title: "Plan listo".into(),
            command: None,
            diff: vec![],
            plan: vec!["# Plan".into(), "1. mirar".into()],
            options: vec![
                option("accept", "Aprobar y editar sin preguntar"),
                option("manual", "Aprobar, preguntando cada edición"),
                option("no", "Seguir planeando"),
            ],
            multi: false,
        }
    }

    fn plan_doc() -> contract::markdown::MarkdownDoc {
        use markdown::tests::{doc, head, para};
        let mut blocks = vec![head(1, "Plan: pruebas")];
        blocks.extend((0..40).map(|n| para(&format!("paso número {n} del plan"))));
        let mut d = doc(blocks);
        d.title = "Plan: pruebas".into();
        d.path = Some("/x/plan.md".into());
        d
    }

    fn text_doc(lines: usize) -> contract::text::TextDoc {
        contract::text::TextDoc {
            title: "main.rs".into(),
            path: "/x/src/main.rs".into(),
            size: 900,
            truncated: false,
            lines: (1..=lines)
                .map(|n| {
                    vec![contract::text::Run {
                        t: format!("let n = {n};"),
                        k: None,
                    }]
                })
                .collect(),
        }
    }

    /// REQ-615, REQ-616, REQ-620: a text file in the viewer has every key, the paste and the
    /// wheel, so none reaches the panel under it; `e` edits, `r` reads it again where it was, `esc`
    /// closes it and nothing is written.
    #[test]
    fn a_text_file_in_the_viewer_has_the_keys_the_paste_and_the_wheel_and_none_reaches_a_pane() {
        let (mut v, ws) = tree_view();
        v.open_text(ws.clone(), String::new(), text_doc(80), false);
        assert!(v.viewer.is_some());
        assert!(v.plain(120, 30).contains("▤ main.rs · 80 líneas"));
        assert!(
            v.plain(120, 30).contains("/ n N buscar"),
            "the status bar tells the keys"
        );
        for code in [KeyCode::Char('j'), KeyCode::Char('w'), KeyCode::Char('x')] {
            assert_eq!(v.press_key(plain(code)), After::Continue);
        }
        assert_eq!(
            v.paste("echo hola"),
            After::Continue,
            "a paste is swallowed"
        );
        v.scroll(true, 40, 10, 120, 30);
        v.click(40, 10, 120, 30);
        assert!(v.viewer.is_some(), "a click leaves it open");
        assert_eq!(
            v.press_key(plain(KeyCode::Char('r'))),
            After::ReadFile {
                workspace: ws.clone(),
                path: "/x/src/main.rs".into(),
                size: None,
                keep: true
            }
        );
        assert!(matches!(
            v.press_key(plain(KeyCode::Char('y'))),
            After::Copy(t) if t.starts_with("let n = 1;\nlet n = 2;")
        ));
        assert_eq!(
            v.press_key(plain(KeyCode::Char('e'))),
            After::Continue,
            "no editor opens from the viewer (N151)"
        );
        assert!(v.viewer.is_some(), "and it stays open");
        v.open_text(ws, String::new(), text_doc(5), false);
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.viewer.is_none());
    }

    /// REQ-617: `s` flips a `.md` between its rendered view and its source, both ways.
    #[test]
    fn s_flips_a_markdown_between_its_view_and_its_source() {
        let (mut v, ws) = tree_view();
        v.open_viewer(plan_doc(), viewer::Source::File, ws.clone());
        assert_eq!(
            v.press_key(plain(KeyCode::Char('s'))),
            After::ReadFile {
                workspace: ws.clone(),
                path: "/x/plan.md".into(),
                size: None,
                keep: false
            }
        );
        let mut doc = text_doc(3);
        doc.title = "plan.md".into();
        doc.path = "/x/plan.md".into();
        v.open_text(ws.clone(), String::new(), doc, false);
        assert_eq!(
            v.press_key(plain(KeyCode::Char('s'))),
            After::ReadMarkdown {
                workspace: ws,
                path: "/x/plan.md".into()
            }
        );
    }

    /// REQ-613: a binary file shows its refusal with the weight the tree knew.
    #[test]
    fn a_binary_file_shows_its_refusal_with_its_weight_and_only_esc_works() {
        let (mut v, ws) = tree_view();
        v.open_refused(ws, "assets/logo.png".into(), Some(48 * 1024));
        assert!(
            v.plain(120, 30)
                .contains("binario · 48.0 KB · no se puede mostrar"),
            "{}",
            v.plain(120, 30)
        );
        assert_eq!(v.press_key(plain(KeyCode::Char('j'))), After::Continue);
        assert!(v.viewer.is_some());
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.viewer.is_none());
    }

    #[test]
    fn v_on_a_plan_asks_for_it_whole_and_the_viewer_answers_it_and_closes() {
        let mut v = conv_view(true);
        v.convs.get_mut(CONV).unwrap().ask = Some(plan_ask());
        assert_eq!(
            v.press_key(plain(KeyCode::Char('v'))),
            After::ReadPlan {
                node: CONV.into(),
                ask: "req-p".into()
            },
            "REQ-469"
        );
        let source = viewer::Source::Plan {
            node: CONV.into(),
            ask: "req-p".into(),
        };
        v.open_viewer(plan_doc(), source.clone(), String::new());
        let screen = v.plain(120, 30);
        assert!(
            screen.contains("▤ plan · Plan: pruebas") && screen.contains("plan listo"),
            "{screen}"
        );
        assert!(
            screen.contains("1  Aprobar y editar sin preguntar"),
            "{screen}"
        );
        assert!(
            screen.contains(" LEER ") && screen.contains("esc cerrar"),
            "the keys are in the status bar: {screen}"
        );
        // every key is the viewer's, the conversation's line gets none
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.conv_ui[CONV].editor.text(), "");
        assert_eq!(
            v.viewer.as_ref().and_then(Viewing::doc).unwrap().source,
            source
        );
        // 1 answers it as the box does, and the viewer closes
        assert_eq!(
            v.press_key(plain(KeyCode::Char('1'))),
            After::Conv(ConvAct::Answer {
                node: CONV.into(),
                ask: "req-p".into(),
                key: "accept".into(),
                text: None,
            }),
            "REQ-475"
        );
        assert!(v.viewer.is_none());
        // answered elsewhere: it says so and keeps the document
        v.open_viewer(plan_doc(), source, String::new());
        v.convs.get_mut(CONV).unwrap().ask = None;
        assert_eq!(v.press_key(plain(KeyCode::Char('2'))), After::Continue);
        assert!(v.viewer.is_some());
        assert!(v.plain(120, 30).contains("ya no espera respuesta"));
        assert!(
            !v.plain(120, 30).contains("plan listo"),
            "the bar no longer says it waits"
        );
        // esc leaves things as they were
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.viewer.is_none());
        assert!(
            v.plain(120, 30).contains("claude · "),
            "the conversation is back"
        );
    }

    #[test]
    fn the_viewer_has_the_wheel_the_clicks_and_the_drag() {
        let mut v = conv_view(true);
        v.convs.get_mut(CONV).unwrap().ask = Some(plan_ask());
        v.open_viewer(
            plan_doc(),
            viewer::Source::Plan {
                node: CONV.into(),
                ask: "req-p".into(),
            },
            String::new(),
        );
        assert!(v.plain(120, 30).contains("paso número 0 del plan"));
        v.scroll(true, 60, 10, 120, 30);
        assert!(
            v.plain(120, 30).contains("paso número 1 del plan"),
            "the wheel moves three rows"
        );
        v.scroll(false, 60, 10, 120, 30);
        // a click on an answer answers (the row above the status bar)
        let (x, y) = find_in(&v, "Aprobar, preguntando", 30);
        assert_eq!(
            v.click(x + 2, y, 120, 30),
            After::Conv(ConvAct::Answer {
                node: CONV.into(),
                ask: "req-p".into(),
                key: "manual".into(),
                text: None,
            })
        );
        // a drag copies, and a key lets go
        v.open_viewer(plan_doc(), viewer::Source::File, String::new());
        let (x, y) = find_in(&v, "paso número 2", 30);
        assert!(v.press_select(x + 5, y, 120, 30));
        v.drag_to(x + 10, y);
        assert_eq!(v.release(120, 30), After::Copy("número".into()));
        assert!(v.flash.starts_with("copiado · 1 línea"), "{}", v.flash);
        // y copies it all, and e opens nothing (N151)
        let After::Copy(all) = v.press_key(plain(KeyCode::Char('y'))) else {
            panic!("a copy");
        };
        assert!(
            all.starts_with("# Plan: pruebas\n\npaso número 0 del plan"),
            "{all}"
        );
        assert_eq!(v.press_key(plain(KeyCode::Char('e'))), After::Continue);
        assert!(v.viewer.is_some(), "the viewer stays open");
    }

    #[test]
    fn a_markdown_file_of_the_tree_is_read_into_the_viewer_and_any_other_text_into_the_viewer_of_text()
     {
        let (mut v, ws) = tree_view();
        v.press_key(plain(KeyCode::Enter));
        v.take_file_asks();
        v.files_replied(listing(
            &ws,
            "src",
            vec![
                file("notas.MD", FileKind::File, None, false),
                file("main.rs", FileKind::File, None, false),
            ],
        ));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::ReadMarkdown {
                workspace: ws.clone(),
                path: "src/notas.MD".into()
            },
            "REQ-470, whatever the case of the extension"
        );
        assert!(!v.tree_focus, "the keys go back to the panels");
        v.tree_focus = true;
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            v.press_key(plain(KeyCode::Char('e'))),
            After::Continue,
            "e opens nothing (N151)"
        );
        assert!(v.tree_focus, "the keys stay in the tree");
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::ReadFile {
                workspace: ws,
                path: "src/main.rs".into(),
                size: None,
                keep: false
            },
            "REQ-615: any other file is read as text"
        );
        assert!(is_markdown("a/b.markdown") && is_markdown("README.md"));
        assert!(!is_markdown("md") && !is_markdown("a.mdx") && !is_markdown("Makefile"));
    }

    #[test]
    fn a_paste_while_the_viewer_is_open_goes_nowhere() {
        let mut v = conv_view(true);
        v.open_viewer(plan_doc(), viewer::Source::File, String::new());
        assert_eq!(v.paste("q hola"), After::Continue);
        assert!(
            v.viewer.is_some(),
            "a `q` that was pasted does not close it"
        );
        assert_eq!(
            v.conv_ui[CONV].editor.text(),
            "",
            "and the line behind gets nothing"
        );
    }

    #[test]
    fn a_plan_is_answered_the_ways_the_question_has_and_a_viewer_of_a_file_has_no_answers() {
        let mut v = conv_view(true);
        let mut ask = plan_ask();
        ask.options.truncate(2);
        v.convs.get_mut(CONV).unwrap().ask = Some(ask);
        v.open_viewer(
            plan_doc(),
            viewer::Source::Plan {
                node: CONV.into(),
                ask: "req-p".into(),
            },
            String::new(),
        );
        let screen = v.plain(120, 30);
        assert!(
            screen.contains("1  Aprobar y editar") && screen.contains("2  Aprobar, preguntando"),
            "{screen}"
        );
        assert!(
            !screen.contains("Seguir planeando"),
            "the box had two answers"
        );
        assert_eq!(v.press_key(plain(KeyCode::Char('3'))), After::Continue);
        v.open_viewer(plan_doc(), viewer::Source::File, String::new());
        assert!(!v.plain(120, 30).contains("Aprobar"));
    }

    fn find_in(v: &View, text: &str, rows: usize) -> (usize, usize) {
        for (y, row) in v.grid(120, rows).rows_plain().iter().enumerate() {
            if let Some(i) = row.find(text) {
                return (row[..i].chars().count(), y);
            }
        }
        panic!("{text} is not on the screen:\n{}", v.plain(120, rows));
    }

    #[test]
    fn an_at_in_the_conversation_asks_for_files_and_the_answer_lists_them() {
        let mut v = conv_view(true);
        for c in "ver @au".chars() {
            v.press_key(plain(KeyCode::Char(c)));
        }
        let asks = v.take_file_asks();
        assert_eq!(asks.len(), 2, "each fragment is asked once: a, au");
        assert_eq!(
            asks[1],
            FileAsk::Mention {
                workspace: v.opened.clone().unwrap(),
                query: "au".into(),
                node: CONV.into()
            }
        );
        v.files_replied(FilesReply {
            ask: asks[1].clone(),
            reply: Ok(Ok(Some(serde_json::json!({"paths": ["src/auth.rs"]})))),
        });
        assert!(v.plain(120, 40).contains("src/auth.rs"));
        v.press_key(plain(KeyCode::Tab));
        assert_eq!(v.conv_ui[CONV].editor.text(), "ver @src/auth.rs ");
    }

    /// REQ-631: the files `@` finds are written from the folder the engine runs in, `../` where
    /// they are above it.
    #[test]
    fn the_files_an_at_finds_are_written_from_the_folder_the_engine_runs_in() {
        let mut v = conv_view(true);
        let root = v.session.as_ref().unwrap().workspaces[0].path.clone();
        let id = v.session.as_ref().unwrap().workspaces[0].id.clone();
        v.opened = Some(id);
        v.convs.get_mut(CONV).unwrap().cwd = Some(format!("{root}/src"));
        for c in "ver @au".chars() {
            v.press_key(plain(KeyCode::Char(c)));
        }
        let asks = v.take_file_asks();
        v.files_replied(FilesReply {
            ask: asks[1].clone(),
            reply: Ok(Ok(Some(
                serde_json::json!({"paths": ["src/auth.rs", "docs/auth.md"]}),
            ))),
        });
        let screen = v.plain(120, 40);
        assert!(
            screen.contains("auth.rs") && screen.contains("../docs/auth.md"),
            "{screen}"
        );
        v.press_key(plain(KeyCode::Tab));
        assert_eq!(v.conv_ui[CONV].editor.text(), "ver @auth.rs ");
    }

    /// REQ-639: `↵` in the viewer of text writes the reference of the lines picked in the field of
    /// the agent of the workspace, from the folder the agent runs in, and sends nothing.
    #[test]
    fn enter_in_the_viewer_of_text_writes_the_reference_in_the_field_of_the_agent_and_sends_nothing()
     {
        let mut v = conv_view(true);
        let root = v.session.as_ref().unwrap().workspaces[0].path.clone();
        let ws = v.session.as_ref().unwrap().workspaces[0].id.clone();
        v.open_text(ws, "src/auth.rs".into(), text_doc(10), false);
        v.press_key(plain(KeyCode::Char('j')));
        v.press_key(plain(KeyCode::Char('v')));
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert_eq!(v.conv_ui[CONV].editor.text(), "@src/auth.rs#L2-3 ");
        assert!(v.flash.contains("nada se mandó"), "{}", v.flash);
        assert!(v.viewer.is_some(), "the viewer stays where it was");
        assert!(v.typing, "the keyboard is the agent's");
        // with nothing picked, the line of the cursor; written from the folder the agent runs in
        v.convs.get_mut(CONV).unwrap().cwd = Some(format!("{root}/docs"));
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert_eq!(
            v.conv_ui[CONV].editor.text(),
            "@src/auth.rs#L2-3 @../src/auth.rs#L3 "
        );
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert!(v.viewer.is_none(), "esc closes it and the field is there");
    }

    /// REQ-639: a workspace with no agent has the reference copied.
    #[test]
    fn a_reference_from_the_viewer_of_text_is_copied_when_the_workspace_has_no_agent() {
        let mut v = fixture_view();
        let ws = v.selected_id().unwrap();
        v.open_text(ws, "src/auth.rs".into(), text_doc(10), false);
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Copy("@src/auth.rs#L1 ".into())
        );
        assert!(v.flash.contains("no hay un agente"), "{}", v.flash);
    }

    /// REQ-639: the file is read again by the path it was asked by, and not by the canonical one, so
    /// that the reference of the new reading is the same.
    #[test]
    fn a_reread_asks_for_the_path_the_file_was_opened_by() {
        let mut v = conv_view(true);
        let ws = v.session.as_ref().unwrap().workspaces[0].id.clone();
        v.open_text(ws.clone(), "src/auth.rs".into(), text_doc(3), false);
        assert_eq!(
            v.press_key(plain(KeyCode::Char('r'))),
            After::ReadFile {
                workspace: ws,
                path: "src/auth.rs".into(),
                size: None,
                keep: true
            }
        );
    }

    fn doc_at(path: &str, lines: usize) -> contract::text::TextDoc {
        let mut d = text_doc(lines);
        d.path = path.into();
        d.title = path.rsplit('/').next().unwrap().into();
        d
    }

    fn tabs_of(v: &View) -> &fileview::FileTabs {
        match &v.viewer {
            Some(Viewing::Text(t)) => t,
            _ => panic!("the viewer of text is not open"),
        }
    }

    /// REQ-640: `t` in the viewer gives the keys to the tree beside it, and `↵` on a file of the tree
    /// is a new tab; `esc` gives the keys back.
    #[test]
    fn t_gives_the_keys_to_the_tree_beside_the_viewer_and_a_file_of_it_is_a_new_tab() {
        let (mut v, ws) = tree_view();
        v.open_text(
            ws.clone(),
            "src/a.rs".into(),
            doc_at("/x/src/a.rs", 20),
            false,
        );
        assert!(!v.tree_focus, "the keys are the viewer's");
        assert_eq!(v.press_key(plain(KeyCode::Char('t'))), After::Continue);
        assert!(v.tree_on && v.tree_focus, "and now the tree's");
        v.press_key(plain(KeyCode::Char('j')));
        assert_eq!(
            tabs_of(&v).current().cursor(),
            (0, 0),
            "j moved the tree, not the cursor"
        );
        v.press_key(plain(KeyCode::Char('j')));
        assert!(matches!(
            v.press_key(plain(KeyCode::Enter)),
            After::ReadFile { ref path, keep: false, .. } if path == "Cargo.toml"
        ));
        v.open_text(ws, "Cargo.toml".into(), doc_at("/x/Cargo.toml", 5), false);
        assert_eq!(tabs_of(&v).count(), 2);
        assert_eq!(tabs_of(&v).current().doc_title(), "Cargo.toml");
        assert!(!v.tree_focus, "the keys are the viewer's again");
        v.press_key(plain(KeyCode::Char('t')));
        assert!(v.tree_focus);
        v.press_key(plain(KeyCode::Esc));
        assert!(
            !v.tree_focus && v.viewer.is_some(),
            "esc gives the keys back and closes nothing"
        );
    }

    /// REQ-640: a Markdown view opened over the tabs gives them back when it closes.
    #[test]
    fn a_markdown_over_the_tabs_gives_them_back_when_it_closes() {
        let (mut v, ws) = tree_view();
        v.open_text(ws.clone(), "a.rs".into(), doc_at("/x/a.rs", 5), false);
        v.open_text(ws.clone(), "b.rs".into(), doc_at("/x/b.rs", 5), false);
        v.open_viewer(plan_doc(), viewer::Source::File, ws.clone());
        assert!(matches!(v.viewer, Some(Viewing::Doc(_))));
        assert!(v.tabs_behind.is_some());
        assert_eq!(v.press_key(plain(KeyCode::Esc)), After::Continue);
        assert_eq!(tabs_of(&v).count(), 2, "the tabs are back");
        assert!(v.tabs_behind.is_none());
        // a file read while the Markdown view is open (its `s`) joins them
        v.open_viewer(plan_doc(), viewer::Source::File, ws.clone());
        v.open_text(ws, "c.rs".into(), doc_at("/x/c.rs", 5), false);
        assert_eq!(tabs_of(&v).count(), 3);
        assert!(v.tabs_behind.is_none());
    }

    /// REQ-640: the reference is of the file of the tab that shows.
    #[test]
    fn the_reference_is_of_the_file_of_the_tab_that_shows() {
        let mut v = conv_view(true);
        let ws = v.session.as_ref().unwrap().workspaces[0].id.clone();
        v.open_text(
            ws.clone(),
            "src/a.rs".into(),
            doc_at("/x/src/a.rs", 9),
            false,
        );
        v.open_text(ws, "src/b.rs".into(), doc_at("/x/src/b.rs", 9), false);
        v.press_key(plain(KeyCode::Tab));
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        assert_eq!(v.conv_ui[CONV].editor.text(), "@src/a.rs#L1 ");
    }

    /// REQ-640: a click puts the cursor on a character of the text, a drag selects and copies, and a
    /// click on a name of the bar is that tab.
    #[test]
    fn the_mouse_moves_the_cursor_selects_and_changes_tab_in_the_viewer_of_text() {
        let (mut v, ws) = tree_view();
        v.open_text(ws.clone(), "a.rs".into(), doc_at("/x/a.rs", 6), false);
        v.open_text(ws, "b.rs".into(), doc_at("/x/b.rs", 6), false);
        let (x, y) = find(&v, "let n = 3;");
        assert!(
            v.press_select(x + 4, y, 120, 30),
            "a press on the text begins a drag"
        );
        assert_eq!(tabs_of(&v).current().cursor(), (2, 4));
        assert_eq!(
            v.release(120, 30),
            After::Continue,
            "a click copies nothing"
        );
        assert!(v.press_select(x + 4, y, 120, 30));
        v.drag_to(x + 8, y);
        assert_eq!(v.release(120, 30), After::Copy("n = 3".into()));
        let (bx, by) = find(&v, "a.rs");
        assert!(
            !v.press_select(bx + 1, by, 120, 30),
            "a name of the bar is no drag"
        );
        assert_eq!(tabs_of(&v).current().doc_title(), "a.rs");
    }

    fn type_line(v: &mut View, text: &str) {
        for c in text.chars() {
            v.press_key(plain(KeyCode::Char(c)));
        }
    }

    fn saved_reply(n: usize) -> Reply {
        let conversations: Vec<_> = (0..n)
            .map(|i| {
                serde_json::json!({
                    "id": format!("{i:08}-3d20-4cd2-9d13-485b39b26147"),
                    "name": format!("conversación {i}"),
                    "at": 1_792_110_000_000u64 - i as u64 * 60_000
                })
            })
            .collect();
        Ok(Ok(Some(
            serde_json::json!({ "conversations": conversations }),
        )))
    }

    #[test]
    fn resume_asks_the_daemon_for_the_saved_conversations_and_choosing_one_opens_it_in_the_pane() {
        let mut v = conv_view(true);
        type_line(&mut v, "/resume");
        assert_eq!(v.press_key(plain(KeyCode::Enter)), After::Continue);
        let asks = v.take_file_asks();
        let ws = v.opened.clone().unwrap();
        assert_eq!(
            asks,
            [FileAsk::Saved {
                workspace: ws,
                node: CONV.into()
            }]
        );
        assert!(v.plain(120, 40).contains("cargando…"));
        v.files_replied(FilesReply {
            ask: asks[0].clone(),
            reply: saved_reply(30),
        });
        let screen = v.plain(120, 40);
        assert!(
            screen.contains("retomar una conversación") && screen.contains("conversación 0"),
            "{screen}"
        );
        assert!(screen.contains("1 de 30"), "{screen}");
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Conv(ConvAct::Resume {
                pane: CONV_PANE.into(),
                id: "00000000-3d20-4cd2-9d13-485b39b26147".into()
            })
        );
        assert_eq!(v.take_then(), None);
    }

    #[test]
    fn continue_resumes_the_newest_without_a_key_and_a_failure_is_said() {
        let mut v = conv_view(true);
        type_line(&mut v, "/continue");
        v.press_key(plain(KeyCode::Enter));
        let asks = v.take_file_asks();
        v.files_replied(FilesReply {
            ask: asks[0].clone(),
            reply: saved_reply(3),
        });
        assert_eq!(
            v.take_then(),
            Some(After::Conv(ConvAct::Resume {
                pane: CONV_PANE.into(),
                id: "00000000-3d20-4cd2-9d13-485b39b26147".into()
            }))
        );
        // the daemon says no
        let mut v = conv_view(true);
        type_line(&mut v, "/resume");
        v.press_key(plain(KeyCode::Enter));
        let asks = v.take_file_asks();
        v.files_replied(FilesReply {
            ask: asks[0].clone(),
            reply: Err(ClientError::Protocol("sin conexión".into())),
        });
        assert!(
            !v.plain(120, 40).contains("cargando"),
            "the list does not wait for ever"
        );
        assert!(v.flash.contains("no se pudo listar"), "{}", v.flash);
    }

    /// REQ-641: a memory file of the workspace opens in the viewer, and one outside it is copied.
    #[test]
    fn a_memory_file_chosen_opens_in_the_viewer_or_is_copied_when_it_is_outside_the_workspace() {
        let mut v = conv_view(true);
        let dir = std::env::temp_dir().join(format!(
            "{}-mem-{}",
            identity::get().name,
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "x").unwrap();
        std::fs::write(dir.join("CLAUDE.local.md"), "y").unwrap();
        v.panes.get_mut(CONV_PANE).unwrap().cwd = dir.to_string_lossy().into_owned();
        let ws = v.session.as_ref().unwrap().workspaces[0].id.clone();
        v.opened = Some(ws.clone());
        type_line(&mut v, "/memory");
        v.press_key(plain(KeyCode::Enter));
        // the person's own files may be there too: narrow it to this one
        type_line(&mut v, "AGENTS");
        let file = dir.join("AGENTS.md").to_string_lossy().into_owned();
        // the workspace is somewhere else: the daemon would refuse it, so the path is copied
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::Copy(file.clone())
        );
        assert!(v.flash.contains("fuera del workspace"), "{}", v.flash);
        // the workspace is the folder: a `.md` opens as Markdown
        v.session.as_mut().unwrap().workspaces[0].path = dir.to_string_lossy().into_owned();
        v.press_key(plain(KeyCode::Esc));
        type_line(&mut v, "/memory");
        v.press_key(plain(KeyCode::Enter));
        type_line(&mut v, "AGENTS");
        assert_eq!(
            v.press_key(plain(KeyCode::Enter)),
            After::ReadMarkdown {
                workspace: ws,
                path: file
            }
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_closed_conversation_leaves_nothing_of_it_behind() {
        let mut v = conv_view(false);
        let docs: Vec<_> = fixture_docs();
        v.sync(docs.iter());
        assert!(!v.convs.contains_key(CONV) && !v.conv_ui.contains_key(CONV));
    }
}
