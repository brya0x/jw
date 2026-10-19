//! A pane that runs `claude` as the conversation the daemon holds (spec REQ-397 to REQ-415;
//! prototype `docs/prototypes/conversation.html`): the items from the oldest, the working line,
//! the field (or the box that asks), and the footer with the mode and the model. The node is
//! what the daemon says; what lives here is only how this terminal looks at it: the line being
//! typed, the scroll, the item picked, what is open, the list that is being chosen from.
//! Every colour is the theme's (REQ-415); the accent is one of its six.

use std::cell::Cell;
use std::collections::HashSet;

use contract::PaneInput;
use contract::conversation::{
    Accent, Ask, AskKind, CommandKind, CommandSource, ConvCommand, ConvStatus, Conversation,
    DiffLine, DiffSign, Held, Image, Item, Mode, SavedConversation, Tone, ToolState,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::field::{self, Field, Look};
use super::fold;
use super::older::{self, Older};
use super::panels::{self, Panel, RowAct};
use super::{Grid, Rgb, Style, clip, look, select};
use crate::claude::tools_word;

/// What the keyboard or the mouse asks the daemon for: the client maps each to its command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Send {
        node: String,
        text: String,
        images: Vec<Image>,
    },
    Answer {
        node: String,
        ask: String,
        key: String,
        /// What to do instead, written with the refusal of a plan (REQ-595).
        text: Option<String>,
    },
    Interrupt {
        node: String,
    },
    Name {
        node: String,
        name: String,
    },
    Color {
        node: String,
        accent: Accent,
    },
    Model {
        node: String,
        model: String,
    },
    Effort {
        node: String,
        effort: String,
    },
    Mode {
        node: String,
        mode: Mode,
    },
    Shell {
        node: String,
        command: String,
    },
    /// Stop a task the engine runs in the background.
    StopTask {
        node: String,
        task: String,
    },
    /// Retake a saved conversation in this pane.
    Resume {
        pane: String,
        id: String,
    },
    /// Open a file in an editor, in a new panel.
    Edit(std::path::PathBuf),
    /// Show the plan that waits, whole, in the Markdown viewer (REQ-469).
    ViewPlan {
        node: String,
        ask: String,
    },
    /// Text for the clipboard.
    Copy(String),
}

/// What the footer lists to choose from, in the order it shows them: the value sent and the
/// words shown.
type Choices = &'static [(&'static str, &'static str)];

const MODELS: Choices = &[
    ("fable", "fable"),
    ("opus", "opus"),
    ("sonnet", "sonnet"),
    ("haiku", "haiku"),
];
const EFFORTS: Choices = &[
    ("low", "low"),
    ("medium", "medium"),
    ("high", "high"),
    ("xhigh", "xhigh"),
    ("max", "max"),
];
const MODES: Choices = &[
    ("manual", "manual"),
    ("acceptEdits", "accept edits"),
    ("plan", "plan"),
    ("auto", "auto"),
];
/// The accents in the order `COLORS` lists them.
const ACCENTS: [Accent; 6] = [
    Accent::Blue,
    Accent::Green,
    Accent::Yellow,
    Accent::Red,
    Accent::Magenta,
    Accent::Cyan,
];
const COLORS: Choices = &[
    ("blue", "azul"),
    ("green", "verde"),
    ("yellow", "amarillo"),
    ("red", "rojo"),
    ("magenta", "magenta"),
    ("cyan", "cian"),
];

/// The most diff lines an edit shows before it is opened, and the most lines of output an opened item shows.
const DIFF_SHOWN: usize = 14;
const OPEN_SHOWN: usize = 60;
const SHELL_SHOWN: usize = 12;
/// Lines one page key moves the list.
const PAGE: usize = 10;
/// Files the `@` list shows at most.
const FILES_SHOWN: usize = 8;
/// Commands the `/` list shows at once.
const MENU_SHOWN: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Of {
    Model,
    Effort,
    Mode,
    Color,
}

impl Of {
    fn title(self) -> &'static str {
        match self {
            Of::Model => "modelo",
            Of::Effort => "esfuerzo",
            Of::Mode => "modo",
            Of::Color => "color",
        }
    }

    fn choices(self) -> Choices {
        match self {
            Of::Model => MODELS,
            Of::Effort => EFFORTS,
            Of::Mode => MODES,
            Of::Color => COLORS,
        }
    }
}

/// How many tasks the engine runs in the background: what the footer's gear counts.
fn running_tasks(conv: &Conversation) -> usize {
    conv.tasks
        .iter()
        .filter(|t| t.state == contract::conversation::TaskState::Run)
        .count()
}

/// A list of jwn's (`panels.rs`): the rows, the one lit, what is typed to narrow them, and, for
/// the conversations still to arrive, that it waits.
struct ListState {
    panel: Panel,
    at: usize,
    filter: String,
    loading: bool,
    /// `/continue`: take the first one that arrives.
    first: bool,
}

impl ListState {
    /// A list that is ready, with its first row lit and nothing typed.
    fn of(panel: Panel) -> ListState {
        ListState {
            panel,
            at: 0,
            filter: String::new(),
            loading: false,
            first: false,
        }
    }
}

enum Pop {
    Pick { of: Of, at: usize },
    Name(Editor),
    List(ListState),
}

/// The line being typed: characters, and where the cursor is among them.
#[derive(Default, Debug, Clone)]
pub struct Editor {
    chars: Vec<char>,
    cursor: usize,
    /// Where a selection began, when there is one: it covers what lies between this and the cursor.
    anchor: Option<usize>,
}

impl Editor {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The characters that are selected, from and to, when some are.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|a| *a != self.cursor)
            .map(|a| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
        self.anchor = None;
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.anchor = None;
        std::mem::take(&mut self.chars).into_iter().collect()
    }

    /// Write `text` at the cursor, in place of what is selected.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        for c in text.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// The `@fragment` the cursor ends: what follows an `@` that starts the line or a word.
    fn mention(&self) -> Option<String> {
        let before = &self.chars[..self.cursor];
        let at = before
            .iter()
            .rposition(|c| c.is_whitespace() || *c == '@')?;
        if before[at] != '@' || (at > 0 && !before[at - 1].is_whitespace()) {
            return None;
        }
        Some(before[at + 1..].iter().collect())
    }

    /// Put `text` in place of the `n` characters before the cursor.
    fn replace_before(&mut self, n: usize, text: &str) {
        self.anchor = None;
        let from = self.cursor.saturating_sub(n);
        self.chars.drain(from..self.cursor);
        self.cursor = from;
        self.insert(text);
    }

    /// Take out what is selected: whether something was.
    fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        self.anchor = None;
        let Some((from, to)) = sel else {
            return false;
        };
        self.chars.drain(from..to);
        self.cursor = from;
        true
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    fn line_start(&self, at: usize) -> usize {
        self.chars[..at]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.chars[at..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(self.chars.len(), |i| at + i)
    }

    /// One line up, keeping the column; false when this is the first line.
    fn up(&mut self) -> bool {
        self.anchor = None;
        let start = self.line_start(self.cursor);
        if start == 0 {
            return false;
        }
        let col = self.cursor - start;
        let (prev_end, prev_start) = (start - 1, self.line_start(start - 1));
        self.cursor = prev_start + col.min(prev_end - prev_start);
        true
    }

    fn down(&mut self) -> bool {
        self.anchor = None;
        let end = self.line_end(self.cursor);
        if end == self.chars.len() {
            return false;
        }
        let col = self.cursor - self.line_start(self.cursor);
        let next = end + 1;
        self.cursor = next + col.min(self.line_end(next) - next);
        true
    }

    /// Where the word before `at` starts: spaces are skipped, then the word.
    fn word_start(&self, mut at: usize) -> usize {
        while at > 0 && self.chars[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !self.chars[at - 1].is_whitespace() {
            at -= 1;
        }
        at
    }

    /// Where the word after `at` ends.
    fn word_end(&self, mut at: usize) -> usize {
        while at < self.chars.len() && self.chars[at].is_whitespace() {
            at += 1;
        }
        while at < self.chars.len() && !self.chars[at].is_whitespace() {
            at += 1;
        }
        at
    }

    /// The word before the cursor, or what is selected.
    fn word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.word_start(self.cursor);
        self.chars.drain(at..self.cursor);
        self.cursor = at;
    }

    /// Put the cursor at `to`, the selection growing from where it was when `select`, gone when
    /// not.
    fn go(&mut self, to: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
    }

    /// A key that moves one character, one word (with alt) or to the end of the line (home and end);
    /// with shift it selects what it crosses. A move with no shift over a selection goes to its edge.
    fn step(&mut self, ev: &KeyEvent) {
        let (alt, shift) = (
            ev.modifiers.contains(KeyModifiers::ALT),
            ev.modifiers.contains(KeyModifiers::SHIFT),
        );
        let edge = self.selection().filter(|_| !shift);
        let to = match (ev.code, alt, edge) {
            (KeyCode::Left, false, Some((from, _))) => from,
            (KeyCode::Right, false, Some((_, to))) => to,
            (KeyCode::Left, true, _) => self.word_start(self.cursor),
            (KeyCode::Right, true, _) => self.word_end(self.cursor),
            (KeyCode::Left, false, None) => self.cursor.saturating_sub(1),
            (KeyCode::Right, false, None) => (self.cursor + 1).min(self.chars.len()),
            (KeyCode::Home, ..) => self.line_start(self.cursor),
            _ => self.line_end(self.cursor),
        };
        self.go(to, shift);
    }

    fn kill_to_start(&mut self) {
        self.anchor = None;
        let start = self.line_start(self.cursor);
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    fn kill_to_end(&mut self) {
        self.anchor = None;
        let end = self.line_end(self.cursor);
        self.chars.drain(self.cursor..end);
    }

    /// The editing keys the line and the name field share.
    fn edit(&mut self, ev: &KeyEvent) -> bool {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        match ev.code {
            KeyCode::Char('a') if ctrl => self.go(self.line_start(self.cursor), false),
            KeyCode::Char('e') if ctrl => self.go(self.line_end(self.cursor), false),
            KeyCode::Char('w') if ctrl => self.word_back(),
            KeyCode::Char('u') if ctrl => self.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.kill_to_end(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert(&c.to_string()),
            // Many terminals send option+arrow as `esc b` and `esc f`, as readline has it.
            KeyCode::Char('b') if alt && !ctrl => self.go(self.word_start(self.cursor), false),
            KeyCode::Char('f') if alt && !ctrl => self.go(self.word_end(self.cursor), false),
            KeyCode::Backspace if alt => self.word_back(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => self.step(ev),
            _ => return false,
        }
        true
    }
}

/// What this terminal holds about one conversation.
#[derive(Default)]
pub struct Ui {
    pub editor: Editor,
    /// The suggestion that the person dismissed with esc (REQ-573).
    ghost_off: String,
    /// Another `claude` has the session open: nothing is written here (REQ-575).
    held: Option<Held>,
    /// The older items of the conversation, which `View::sync` keeps beside the node's (REQ-567).
    pub(super) older: Older,
    /// The time of the last frame drawn, for what counts up (a tool that runs); 0 before the first.
    now: Cell<u64>,
    /// Lines the list is scrolled up from the newest; `most` is what the last frame could scroll.
    scroll: usize,
    most: Cell<usize>,
    /// What a drag of the mouse covers in the list, and the drag while the button is down
    /// (REQ-461).
    sel: Option<select::Selection>,
    pull: Option<select::Pull>,
    /// The item picked (its number from the start of the conversation) and the items opened.
    picked: Option<u32>,
    open: HashSet<u32>,
    /// The runs of tools that are opened, by the number of their first item (REQ-558).
    unfolded: HashSet<u32>,
    /// The option of the question that is lit.
    ask_at: usize,
    /// «Seguir planeando» was chosen on the plan that waits (its id) and the instruction that goes
    /// with it is being written (REQ-595).
    refusing: Option<(String, Editor)>,
    pop: Option<Pop>,
    images: Vec<(u32, Image)>,
    /// How many pictures were pasted, which is the `n` of the next `[Imagen#n]`.
    pasted: u32,
    /// The files that hold what follows the `@` the line ends with, for which fragment, the
    /// one lit, the fragment last asked for and the one the person closed the list on.
    /// The command lit in the `/` list, and the line the person closed it on.
    menu_at: usize,
    menu_off: String,
    /// The options of a choice of several that are marked, and the ask they are for.
    picks: Vec<usize>,
    ask_id: String,
    /// The checklist is open: its lines, and not only how many are done.
    todos_open: bool,
    /// The folder the pane is in, for the lists that read the machine.
    cwd: String,
    /// The list of saved conversations waits for the daemon: the client asks once.
    want_saved: bool,
    files: Vec<String>,
    files_for: String,
    file_at: usize,
    asked: String,
    dismissed: String,
    /// A message was sent while the engine was dead: «retomando conversación…» until it answers.
    retaking: bool,
    says: String,
}

/// Rows the parts of a conversation pane take.
struct Geo {
    /// The rows of the strip of the checklist, at the top (none when there is no checklist).
    todo_rows: usize,
    /// Where the list goes, and how many rows it has.
    list: (usize, usize),
    /// The working line, when there is one.
    work: Option<usize>,
    /// Where the field (or the box that asks) starts, and its rows.
    band: (usize, usize),
    foot: usize,
}

struct Line {
    spans: Vec<(String, Style)>,
    bg: Option<Rgb>,
    item: u32,
    /// How many characters of the row are the mark that begins it (`❯ `, `● `, `  ⎿  `): what a
    /// selection copies does not have them.
    lead: usize,
    /// The row goes on from the one above it, which the width broke: copied, they are one line.
    soft: bool,
    /// The row says a run of tools folded (REQ-558): a click opens or closes it.
    fold: bool,
}

/// What a button let go of comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Released {
    /// It never moved: the click it was, at this cell.
    Click(usize, usize),
    /// It covered text: what to put on the clipboard.
    Copy(String),
    Nothing,
}

/// Where the lines of the list are on the screen.
struct ListRows {
    /// The rows they are on, from `top` up to `bottom` (not included).
    top: usize,
    bottom: usize,
    /// The first line shown.
    start: usize,
    most: usize,
}

/// What begins a row and is not what it says.
const MARKS: [&str; 7] = ["❯ ", "● ", "! ", "  ⎿  ", "     · ", "     ", "  "];

impl Line {
    fn text(&self) -> String {
        self.spans.iter().map(|(t, _)| t.as_str()).collect()
    }

    fn len(&self) -> usize {
        self.spans.iter().map(|(t, _)| t.chars().count()).sum()
    }
}

impl Ui {
    /// The conversation changed: what only a dead engine needed is forgotten.
    pub fn sync(&mut self, conv: &Conversation) {
        if !matches!(conv.status, ConvStatus::Dead | ConvStatus::Resuming) {
            self.retaking = false;
        }
        self.held.clone_from(&conv.held);
        // A run that is gone (its tools were trimmed) is not open any more; one that older items
        // made longer starts earlier, and stays open under its new first.
        let spans: Vec<(u32, u32)> = fold::runs(&conv.items)
            .iter()
            .map(|r| (conv.first + r.first as u32, conv.first + r.last as u32))
            .collect();
        self.unfolded = self
            .unfolded
            .iter()
            .filter_map(|k| {
                spans
                    .iter()
                    .find(|(a, b)| (a..=b).contains(&k))
                    .map(|(a, _)| *a)
            })
            .collect();
        // Each question is its own: what was lit or marked in the last one does not carry over.
        let id = conv.ask.as_ref().map_or("", |a| a.id.as_str());
        if self.ask_id != id {
            id.clone_into(&mut self.ask_id);
            self.ask_at = 0;
            self.picks.clear();
            self.refusing = None;
        }
    }

    /// Older items were added before the others: what is held by the number of an item (the one
    /// picked, the tools opened, the runs unfolded) moves with them, and a selection, which is of
    /// rows, is let go.
    pub(super) fn shifted(&mut self, by: usize) {
        let by = by as u32;
        self.picked = self.picked.map(|p| p + by);
        self.open = self.open.iter().map(|i| i + by).collect();
        self.unfolded = self.unfolded.iter().map(|i| i + by).collect();
        self.sel = None;
        self.pull = None;
    }

    /// A message for the status bar (a copy, a picture that was not there), once.
    pub fn take_says(&mut self) -> Option<String> {
        (!self.says.is_empty()).then(|| std::mem::take(&mut self.says))
    }

    /// The line was pasted: the text goes in where the cursor is.
    pub fn paste(&mut self, text: &str) {
        if matches!(self.pop, Some(Pop::Name(_))) || self.held.is_some() {
            return;
        }
        self.menu_at = 0;
        self.editor
            .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
    }

    /// A picture the clipboard held, now small: it goes with the next message.
    pub fn attach(&mut self, image: Image) {
        if self.held.is_some() {
            return;
        }
        self.pasted += 1;
        self.editor.insert(&format!("[Imagen#{}]", self.pasted));
        self.images.push((self.pasted, image));
    }

    /// The fragment after an `@` that the files have not been asked for yet (REQ-408).
    pub fn want_files(&mut self) -> Option<String> {
        let Some(fragment) = self.editor.mention().filter(|f| !f.is_empty()) else {
            self.files.clear();
            self.asked.clear();
            self.dismissed.clear();
            return None;
        };
        if fragment == self.asked {
            return None;
        }
        self.asked.clone_from(&fragment);
        Some(fragment)
    }

    /// The daemon found these files for `fragment`; they count only while it is still what the line ends with.
    pub fn found(&mut self, fragment: &str, paths: Vec<String>) {
        if self.editor.mention().as_deref() == Some(fragment) {
            self.files = paths.into_iter().take(FILES_SHOWN).collect();
            self.files_for = fragment.to_owned();
            self.file_at = 0;
        }
    }

    fn files_open(&self) -> bool {
        !self.files.is_empty()
            && self.editor.mention().as_deref() == Some(self.files_for.as_str())
            && self.dismissed != self.files_for
    }

    /// Put the file lit in the line, where the `@fragment` was.
    fn take_file(&mut self, at: usize) {
        let Some(path) = self.files.get(at).cloned() else {
            return;
        };
        let n = self.files_for.chars().count() + 1;
        self.editor.replace_before(n, &format!("@{path} "));
        self.files.clear();
        self.asked.clear();
    }

    /// The commands whose names hold the letters after the `/` the line is, the ones that begin
    /// with them first (REQ-416).
    fn menu<'a>(&self, conv: &'a Conversation) -> Vec<&'a ConvCommand> {
        let text = self.editor.text();
        let Some(typed) = text
            .strip_prefix('/')
            .filter(|t| !t.contains(char::is_whitespace) && self.menu_off != text)
        else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        let mut hits: Vec<(bool, usize, &ConvCommand)> = conv
            .commands
            .iter()
            .filter_map(|c| {
                let name = c.name.trim_start_matches('/').to_lowercase();
                name.contains(&typed)
                    .then(|| (!name.starts_with(&typed), source_look(c.source).rank, c))
            })
            .collect();
        hits.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
        hits.into_iter().map(|(_, _, c)| c).collect()
    }

    /// What the line starts with that has a colour of its own: a command that exists, in the
    /// colour of where it is defined, or the `!` of a shell line.
    fn lead(&self, conv: &Conversation) -> Option<(String, Rgb)> {
        let text = self.editor.text();
        if text.starts_with('!') && text.len() > 1 {
            return Some(("!".into(), look::green()));
        }
        let word = text
            .split_whitespace()
            .next()
            .filter(|w| w.starts_with('/'))?;
        let c = conv.commands.iter().find(|c| c.name == word)?;
        Some((word.to_owned(), source_look(c.source).colour))
    }

    /// The folder the pane is in.
    pub fn context(&mut self, cwd: &str) {
        if self.cwd != cwd {
            cwd.clone_into(&mut self.cwd);
        }
    }

    /// Whether the list of saved conversations has to be asked for, once.
    pub fn take_saved_ask(&mut self) -> bool {
        std::mem::take(&mut self.want_saved)
    }

    /// The saved conversations arrived (`/resume`); `/continue` takes the newest at once.
    pub fn listed(&mut self, saved: &[SavedConversation], conv: &Conversation) -> Option<Act> {
        let Some(Pop::List(l)) = &mut self.pop else {
            return None;
        };
        if !l.loading {
            return None;
        }
        let take_first = l.first;
        l.panel = panels::saved(saved);
        l.loading = false;
        if take_first {
            let act = l.panel.rows.first().map(|r| r.act.clone());
            self.pop = None;
            return match act {
                Some(act) => self.row_act(conv, act),
                None => {
                    self.says = "no hay conversaciones guardadas de esta carpeta".into();
                    None
                }
            };
        }
        None
    }

    /// The daemon could not list them.
    pub fn list_failed(&mut self, why: &str) {
        if matches!(&self.pop, Some(Pop::List(l)) if l.loading) {
            self.pop = None;
            self.says = why.to_owned();
        }
    }

    /// `/help`, `/status`, `/resume` and the other lists of jwn's: opened from what the node and the machine hold.
    fn open_panel(&mut self, conv: &Conversation, word: &str, arg: &str) -> Option<Act> {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let cwd = std::path::PathBuf::from(&self.cwd);
        let panel = match word {
            "help" => panels::help(conv),
            "status" => panels::status(conv, &self.cwd),
            "skills" => panels::skills(conv),
            "tasks" => panels::tasks(conv),
            "artifacts" => panels::artifacts(conv),
            "permissions" => panels::permissions(&cwd, home.as_deref()),
            "memory" => panels::memory(&cwd, home.as_deref()),
            "resume" | "continue" => {
                if word == "resume" && !arg.is_empty() {
                    return Some(Act::Resume {
                        pane: conv.pane.clone(),
                        id: arg.to_owned(),
                    });
                }
                self.want_saved = true;
                self.pop = Some(Pop::List(ListState {
                    loading: true,
                    first: word == "continue",
                    ..ListState::of(Panel {
                        title: "retomar una conversación".into(),
                        rows: Vec::new(),
                        empty: "cargando…".into(),
                    })
                }));
                return None;
            }
            _ => return None,
        };
        self.pop = Some(Pop::List(ListState::of(panel)));
        None
    }

    /// What choosing a row of a list does.
    fn row_act(&mut self, conv: &Conversation, act: RowAct) -> Option<Act> {
        match act {
            RowAct::Nothing => None,
            RowAct::Insert(text) => {
                self.editor.set(&text);
                None
            }
            RowAct::Resume(id) => Some(Act::Resume {
                pane: conv.pane.clone(),
                id,
            }),
            RowAct::Copy(text) => {
                self.says = copied(&text);
                Some(Act::Copy(text))
            }
            RowAct::Edit(path) => Some(Act::Edit(path)),
            RowAct::Stop(task) => {
                self.says = "deteniendo la tarea…".into();
                Some(Act::StopTask {
                    node: conv.head.id.clone(),
                    task,
                })
            }
        }
    }

    /// Whether a list or a field is open over the conversation.
    pub fn has_pop(&self) -> bool {
        self.pop.is_some()
    }

    /// Lines of the list, from the oldest, `w` columns wide.
    fn lines(&self, conv: &Conversation, w: usize) -> Vec<Line> {
        let mut out = Vec::new();
        // What is above the first item: older ones to load, or where the conversation starts.
        if let Some(row) = self.older.row(conv) {
            let text = row.text();
            let tone = match row {
                older::Row::Failed => look::red(),
                older::Row::Loading => look::yellow(),
                _ => look::dim(),
            };
            out.push(Line {
                spans: vec![(text.to_owned(), Style::fg(tone))],
                bg: None,
                item: older::TOP,
                lead: text.chars().count(),
                soft: false,
                fold: false,
            });
            out.push(Line {
                spans: vec![],
                bg: None,
                item: older::TOP,
                lead: 0,
                soft: false,
                fold: false,
            });
        }
        let mut runs = fold::runs(&conv.items).into_iter().peekable();
        let mut i = 0;
        let mut after_run = false;
        while i < conv.items.len() {
            let abs = conv.first + i as u32;
            let before = out.len();
            if let Some(run) = runs.next_if(|r| r.first == i) {
                let open = self.unfolded.contains(&abs);
                out.push(Line {
                    spans: vec![
                        (
                            format!("  {} ", if open { "▾" } else { "▸" }),
                            Style::fg(look::dim()),
                        ),
                        (run.title.clone(), Style::fg(look::dim())),
                    ],
                    bg: None,
                    item: abs,
                    lead: 4,
                    soft: false,
                    fold: true,
                });
                self.separate(conv, i, before, false, &mut out);
                if open {
                    for k in run.first..=run.last {
                        let at = conv.first + k as u32;
                        self.item_lines(conv, &conv.items[k], at, w, &mut out);
                    }
                }
                i = run.last + 1;
                after_run = true;
                continue;
            }
            self.item_lines(conv, &conv.items[i], abs, w, &mut out);
            self.separate(conv, i, before, after_run, &mut out);
            after_run = false;
            i += 1;
        }
        // What waits for the turn that runs comes after what was said, as a message not sent yet.
        let end = conv.first + conv.items.len() as u32;
        let dim = Style::fg(look::dim());
        for text in &conv.queue {
            out.push(Line {
                spans: vec![],
                bg: None,
                item: end,
                lead: 0,
                soft: false,
                fold: false,
            });
            for (k, (continues, row)) in wrap(text, w.saturating_sub(2), dim, dim)
                .into_iter()
                .enumerate()
            {
                let mark = if k == 0 { "⧖ " } else { "  " };
                let mut spans = vec![(mark.to_owned(), Style::fg(look::yellow()))];
                spans.extend(row);
                out.push(Line {
                    spans,
                    bg: None,
                    item: end,
                    lead: 2,
                    soft: continues,
                    fold: false,
                });
            }
        }
        out
    }

    /// A blank row before what was drawn from `before` on, unless it goes on from a tool, an agent or
    /// a note that the one above it is too (`i` is the item, or the first of a run). What comes
    /// after a run of folded tools (`after_run`) is not part of it, so it has its blank row.
    fn separate(
        &self,
        conv: &Conversation,
        i: usize,
        before: usize,
        after_run: bool,
        out: &mut Vec<Line>,
    ) {
        let Some(item) = conv.items.get(i) else {
            return;
        };
        let grouped = i > 0
            && !after_run
            && matches!(
                item,
                Item::Tool { .. } | Item::Agent { .. } | Item::Note { .. }
            )
            && matches!(
                conv.items[i - 1],
                Item::Tool { .. } | Item::Agent { .. } | Item::Note { .. }
            );
        if before > 0 && !grouped && out.len() > before {
            out.insert(
                before,
                Line {
                    spans: vec![],
                    bg: None,
                    item: conv.first + i as u32,
                    lead: 0,
                    soft: false,
                    fold: false,
                },
            );
        }
    }

    fn item_lines(
        &self,
        conv: &Conversation,
        item: &Item,
        abs: u32,
        w: usize,
        out: &mut Vec<Line>,
    ) {
        let accent = accent_rgb(conv.accent);
        let dim = Style::fg(look::dim());
        let plain = Style::fg(look::fg());
        // Set before a push: the row it makes goes on from the one above (see `Line::soft`).
        let soft = Cell::new(false);
        // Set before a push: how many characters past its mark the row is only how it is set.
        let set = Cell::new(0);
        let mut push = |spans: Vec<(String, Style)>, bg: Option<Rgb>| {
            let lead = spans
                .first()
                .filter(|(t, _)| MARKS.contains(&t.as_str()))
                .map_or(0, |(t, _)| t.chars().count())
                + set.replace(0);
            out.push(Line {
                spans,
                bg,
                item: abs,
                lead,
                soft: soft.replace(false),
                fold: false,
            });
        };
        // A tool that runs is drawn open: what it does and for how long (REQ-562).
        let running = matches!(
            item,
            Item::Tool {
                state: ToolState::Run,
                ..
            }
        );
        let open = running || self.open.contains(&abs);
        match item {
            Item::User { text, images } => {
                let rows = wrap(text, w.saturating_sub(2), plain, Style::fg(look::cyan()));
                for (k, (continues, row)) in rows.into_iter().enumerate() {
                    let mark = if k == 0 { "❯ " } else { "  " };
                    let mut spans = vec![(mark.to_owned(), Style::fg(accent).bold())];
                    spans.extend(row);
                    soft.set(continues);
                    push(spans, Some(look::sel()));
                }
                if *images > 0 {
                    push(
                        vec![(format!("  ▣ {images} imagen(es)"), dim)],
                        Some(look::sel()),
                    );
                }
            }
            Item::Text {
                blocks: Some(blocks),
                ..
            } => {
                for (k, row) in super::markdown::rows(blocks, w.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    let mark = if k == 0 { "● " } else { "  " };
                    let mut spans = vec![(mark.to_owned(), plain)];
                    spans.extend(row.spans);
                    soft.set(row.soft);
                    set.set(row.lead);
                    push(spans, row.bg);
                }
            }
            Item::Text { text, live, .. } => {
                let mut rows = wrap(text, w.saturating_sub(2), plain, Style::fg(look::cyan()));
                if *live && let Some((_, last)) = rows.last_mut() {
                    last.push(("▍".to_owned(), Style::fg(accent)));
                }
                for (k, (continues, row)) in rows.into_iter().enumerate() {
                    let mark = if k == 0 { "● " } else { "  " };
                    let mut spans = vec![(mark.to_owned(), plain)];
                    spans.extend(row);
                    soft.set(continues);
                    push(spans, None);
                }
            }
            Item::Tool {
                id,
                name,
                arg,
                state,
                summary,
                output,
                diff,
                url,
                by,
            } => {
                let mut head = vec![
                    ("● ".to_owned(), Style::fg(state_colour(*state))),
                    (name.clone(), plain.bold()),
                ];
                // A tool with nothing to say of its argument has no parentheses, and the advisor
                // says its model as Claude Code does: `Advising using Opus 5.5`.
                if name == "Advising" && !arg.is_empty() {
                    head.push((format!(" {arg}"), dim));
                } else if !arg.is_empty() {
                    head.push((
                        format!(
                            "({})",
                            clip(arg, w.saturating_sub(name.chars().count() + 6))
                        ),
                        dim,
                    ));
                }
                if let Some(by) = by {
                    let how = if by.starts_with("modo") {
                        "por"
                    } else {
                        "desde"
                    };
                    // What Claude asks is answered; what it does is allowed (REQ-405).
                    let did = if matches!(name.as_str(), "AskUserQuestion" | "ExitPlanMode") {
                        "respondida"
                    } else {
                        "permitido"
                    };
                    head.push((format!(" · {did} {how} {by}"), Style::fg(look::green())));
                }
                if *state == ToolState::Run
                    && let Some(r) = conv.running.iter().find(|r| &r.id == id)
                    && self.now.get() > 0
                {
                    let took = secs(self.now.get().saturating_sub(r.since));
                    head.push((format!(" · {took}"), dim));
                }
                push(head, None);
                if let Some(s) = summary {
                    push(
                        vec![
                            ("  ⎿  ".to_owned(), dim),
                            (clip(s, w.saturating_sub(5)), dim),
                        ],
                        None,
                    );
                } else if *state == ToolState::Denied {
                    push(
                        vec![
                            ("  ⎿  ".to_owned(), dim),
                            ("negado".to_owned(), Style::fg(look::red())),
                        ],
                        None,
                    );
                } else if running {
                    push(
                        vec![("  ⎿  ".to_owned(), dim), ("ejecutando…".to_owned(), dim)],
                        None,
                    );
                }
                if let Some(u) = url {
                    push(
                        vec![
                            ("  ⎿  ".to_owned(), dim),
                            (clip(u, w.saturating_sub(5)), {
                                let mut s = Style::fg(look::cyan());
                                s.underline = true;
                                s
                            }),
                        ],
                        None,
                    );
                }
                diff_lines(diff, open, w, &mut push);
                if open {
                    for line in output.iter().take(OPEN_SHOWN) {
                        push(
                            vec![
                                ("     ".to_owned(), dim),
                                (clip(line, w.saturating_sub(5)), dim),
                            ],
                            None,
                        );
                    }
                }
            }
            Item::Agent {
                name,
                description,
                state,
                tools,
                tokens,
                ms,
                last,
                steps,
                ..
            } => {
                push(
                    vec![
                        ("● ".to_owned(), Style::fg(state_colour(*state))),
                        ("Agent".to_owned(), plain.bold()),
                        (format!("({name}) "), dim),
                        (
                            clip(description, w.saturating_sub(name.chars().count() + 12)),
                            plain,
                        ),
                    ],
                    None,
                );
                let totals = format!(
                    "{} · {} tokens · {}",
                    tools_word(*tools),
                    kilo(*tokens),
                    secs(*ms)
                );
                let line = match (state, last) {
                    (ToolState::Run | ToolState::Ask, Some(l)) => format!("… {l} · {totals}"),
                    (ToolState::Run | ToolState::Ask, None) => format!("… {totals}"),
                    _ => totals,
                };
                push(
                    vec![
                        ("  ⎿  ".to_owned(), dim),
                        (clip(&line, w.saturating_sub(5)), dim),
                    ],
                    None,
                );
                if open {
                    for step in steps.iter().take(OPEN_SHOWN) {
                        push(
                            vec![
                                ("     · ".to_owned(), dim),
                                (clip(step, w.saturating_sub(7)), dim),
                            ],
                            None,
                        );
                    }
                }
            }
            Item::Shell {
                command,
                state,
                output,
            } => {
                push(
                    vec![
                        ("! ".to_owned(), Style::fg(state_colour(*state)).bold()),
                        (clip(command, w.saturating_sub(2)), Style::fg(look::green())),
                    ],
                    None,
                );
                let shown = if open { OPEN_SHOWN } else { SHELL_SHOWN };
                for line in output.iter().take(shown) {
                    push(
                        vec![
                            ("  ".to_owned(), dim),
                            (clip(line, w.saturating_sub(2)), plain),
                        ],
                        None,
                    );
                }
                if output.len() > shown {
                    push(
                        vec![(format!("  … {} líneas más · ^O", output.len() - shown), dim)],
                        None,
                    );
                }
            }
            Item::Out { lines } => {
                for (k, line) in lines.iter().enumerate() {
                    let mark = if k == 0 { "  ⎿  " } else { "     " };
                    push(
                        vec![
                            (mark.to_owned(), dim),
                            (clip(line, w.saturating_sub(5)), plain),
                        ],
                        None,
                    );
                }
            }
            Item::Note { text, tone } => {
                let colour = match tone {
                    Some(Tone::Error) => look::red(),
                    Some(Tone::Warn) => look::yellow(),
                    None => look::dim(),
                };
                push(
                    vec![
                        ("  ⎿  ".to_owned(), dim),
                        (clip(text, w.saturating_sub(5)), Style::fg(colour)),
                    ],
                    None,
                );
            }
            Item::Result { ms, tools, cost } => {
                push(
                    vec![(format!("✻ {}", done_line(*ms, *tools, *cost)), dim)],
                    None,
                );
            }
            Item::Divider { text, .. } => {
                let label = format!(" {text} ");
                let bar = w.saturating_sub(label.chars().count()) / 2;
                push(
                    vec![(
                        format!("{}{label}{}", "─".repeat(bar), "─".repeat(bar)),
                        dim,
                    )],
                    None,
                );
            }
        }
    }

    /// Why nothing is written here, when another `claude` has the session open (REQ-575): what
    /// the field says, short, and what the status bar says when something is typed, with its folder.
    fn held_why(held: &Held) -> (String, String) {
        let state = if held.busy {
            "trabaja en ella"
        } else {
            "la tiene abierta"
        };
        let place: Vec<&str> = held.cwd.rsplit('/').take(2).collect();
        let place = place.into_iter().rev().collect::<Vec<_>>().join("/");
        (
            format!(
                "solo lectura · otro claude (pid {}) {state} · ciérralo",
                held.pid
            ),
            format!(
                "otro claude (pid {}, en {place}) {state}: ciérralo para escribir en esta sesión",
                held.pid
            ),
        )
    }

    /// What Claude proposes, when it can be taken: the field is empty, nothing runs, waits or is
    /// open over it, and the person did not put it aside (REQ-572).
    fn ghost<'a>(&self, conv: &'a Conversation) -> Option<&'a str> {
        conv.suggestion.as_deref().filter(|s| {
            conv.held.is_none()
                && self.editor.text().is_empty()
                && conv.ask.is_none()
                && self.pop.is_none()
                && !self.working(conv)
                && self.ghost_off != *s
        })
    }

    fn working(&self, conv: &Conversation) -> bool {
        conv.status != ConvStatus::Idle && !(conv.status == ConvStatus::Dead && !self.retaking)
    }

    /// Another `claude` runs a turn in the transcript this conversation follows (REQ-565): the line
    /// of work is drawn, and there is nothing to interrupt.
    fn followed_work(&self, conv: &Conversation) -> bool {
        conv.following.is_some() && !self.working(conv)
    }

    fn geo(&self, conv: &Conversation, (iy, ih): (usize, usize), field_rows: usize) -> Geo {
        let foot = iy + ih.saturating_sub(1);
        let band_rows = conv
            .ask
            .as_ref()
            .map_or(field_rows, ask_rows)
            .min(ih.saturating_sub(2));
        let band_y = foot.saturating_sub(band_rows);
        let work =
            (self.working(conv) || self.followed_work(conv)).then(|| band_y.saturating_sub(1));
        // A blank row between the list and the line of work.
        let list_end = work.map_or(band_y, |w| w.saturating_sub(1));
        // The checklist's strip is one row, and its lines when it is open, as many as there is room for.
        let todo_rows = if conv.todos.is_empty() {
            0
        } else {
            1 + if self.todos_open {
                conv.todos.len().min(6)
            } else {
                0
            }
        }
        .min(list_end.saturating_sub(iy + 2));
        Geo {
            todo_rows,
            list: (iy + todo_rows, list_end.saturating_sub(iy + todo_rows)),
            work,
            band: (band_y, band_rows),
            foot,
        }
    }

    fn field(&self, iw: usize) -> Field {
        Field::selecting(
            &PaneInput {
                text: self.editor.text(),
                cursor: self.editor.cursor() as u32,
            },
            self.editor.selection(),
            iw,
        )
    }

    /// Draw the conversation in the pane whose inner box is `(ix, iy, iw, ih)`. `chips` are the
    /// pane's folder and branch; the messages that wait and the pictures go after them.
    pub fn draw(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        (ix, iy, iw, ih): (usize, usize, usize, usize),
        chips: &[(String, Rgb)],
        focused: bool,
        now_ms: u64,
    ) {
        if iw < 12 || ih < 8 {
            return;
        }
        self.now.set(now_ms);
        let field = self.field(iw);
        let geo = self.geo(conv, (iy, ih), field.band_rows());
        let lines = self.lines(conv, iw - 2);
        let rows = self.list_rows(&geo, lines.len());
        self.most.set(rows.most);
        self.older.set_at_top(self.scroll >= rows.most);
        let (start, top) = (rows.start, rows.top);
        let accent = accent_rgb(conv.accent);
        for (r, line) in lines
            .iter()
            .skip(start)
            .take(rows.bottom - rows.top)
            .enumerate()
        {
            let y = top + r;
            let picked = self.picked == Some(line.item) && !line.fold;
            if let Some(bg) = line.bg {
                g.paint_bg(ix, y, iw, bg);
            } else if picked {
                g.paint_bg(ix, y, iw, look::sel());
            }
            if picked {
                g.put_over(ix, y, "▌", Style::fg(accent));
            }
            let mut x = ix + 2;
            for (text, style) in &line.spans {
                let text = clip(text, (ix + iw).saturating_sub(x));
                g.put_over(x, y, &text, *style);
                x += text.chars().count();
            }
            // What a drag covers: the theme's most visible colour, as in a pane's list (REQ-308).
            if let Some((from, to)) = self
                .sel
                .as_ref()
                .and_then(|s| s.on_line(start + r, line.len()))
            {
                g.light(ix + 2 + from, (ix + 2 + to).min(ix + iw), y);
            }
        }
        self.draw_todos(g, conv, (ix, iy, iw), geo.todo_rows);
        if let Some(y) = geo.work {
            let spin = ['✻', '✶', '✷', '✸'][(now_ms / 250 % 4) as usize];
            let text = self.work_text(conv, now_ms);
            g.put(ix + 1, y, &format!("{spin} "), Style::fg(accent).bold());
            g.put(
                ix + 3,
                y,
                &clip(&text, iw.saturating_sub(4)),
                Style::fg(look::dim()),
            );
        }
        let (by, brows) = geo.band;
        if let Some(ask) = &conv.ask {
            self.draw_ask(g, ask, (ix, by, iw, brows));
        } else {
            let mut chips = chips.to_vec();
            if !conv.queue.is_empty() {
                chips.push((format!("en cola {}", conv.queue.len()), look::yellow()));
            }
            if !self.images.is_empty() {
                chips.push((format!("▣ {}", self.images.len()), look::cyan()));
            }
            let proposed = match &conv.held {
                Some(h) => Some(Ui::held_why(h).0),
                None => self.ghost(conv).map(|s| format!("{s}  tab")),
            };
            let look = Look {
                stop: self.working(conv).then_some("■ esc"),
                ask: proposed
                    .as_deref()
                    .unwrap_or("escribe un mensaje · / comandos"),
                lead: self.lead(conv),
            };
            field.draw_with(g, (ix, by, iw), &chips, focused, &look);
        }
        self.draw_foot(g, conv, (ix, geo.foot, iw));
        if let Some(Pop::List(l)) = &self.pop {
            let top = geo.list.0;
            self.draw_list(g, conv, l, (ix, top, iw, by.saturating_sub(top)), now_ms);
        } else if let Some(pop) = &self.pop {
            self.draw_pop(g, conv, pop, (ix, by, iw));
        } else if conv.ask.is_none() && self.files_open() {
            self.draw_files(g, conv, (ix, by, iw));
        } else if conv.ask.is_none() {
            self.draw_menu(g, conv, (ix, by, iw));
        }
    }

    fn work_text(&self, conv: &Conversation, now_ms: u64) -> String {
        if let (true, Some(began)) = (self.followed_work(conv), conv.following) {
            let what = if conv.running.is_empty() {
                "Pensando…"
            } else {
                "Trabajando…"
            };
            let mut parts = vec![secs(now_ms.saturating_sub(began))];
            if conv.tokens > 0 {
                parts.push(format!("↓ {} tokens", kilo(conv.tokens)));
            }
            return format!("{what} ({})", parts.join(" · "));
        }
        let what = match conv.status {
            ConvStatus::Resuming | ConvStatus::Dead => return "retomando conversación…".into(),
            ConvStatus::Thinking => "Pensando…",
            ConvStatus::Streaming => "Escribiendo…",
            ConvStatus::Tool => "Trabajando…",
            ConvStatus::Asking => "Espera tu respuesta",
            ConvStatus::Compacting => "Compactando…",
            ConvStatus::Idle => "",
        };
        let since = conv
            .since
            .map(|s| format!("{} · ", secs(now_ms.saturating_sub(s))))
            .unwrap_or_default();
        let tokens = if conv.tokens > 0 {
            format!("↓ {} tokens · ", kilo(conv.tokens))
        } else {
            String::new()
        };
        format!("{what} ({since}{tokens}esc interrumpe)")
    }

    fn draw_ask(&self, g: &mut Grid, ask: &Ask, (ix, y, iw, h): (usize, usize, usize, usize)) {
        let rect = (ix, y, iw, h);
        g.clear_box(rect);
        let title = match ask.kind {
            AskKind::Permission => format!("permiso · {}", ask.tool),
            AskKind::Question => "pregunta".to_owned(),
            AskKind::Plan => "plan".to_owned(),
        };
        g.boxed(
            rect,
            &title,
            look::yellow(),
            Style::fg(look::fg()).bold(),
            None,
        );
        let ground = look::panel();
        let (tx, tw) = (ix + 2, iw.saturating_sub(4));
        // The options sit at the bottom, where `click` counts them from; what the box is about
        // takes the rows above them and is cut when the pane is too short.
        let opts_at = (y + h).saturating_sub(2 + ask.options.len());
        let mut row = y + 1;
        let mut text_row = |g: &mut Grid, text: &str, style: Style| {
            if row >= opts_at {
                return;
            }
            if let Some(bg) = style.bg {
                g.paint_bg(tx, row, tw, bg);
            }
            g.put(tx, row, &clip(text, tw), style);
            row += 1;
        };
        text_row(g, &ask.title, Style::fg(look::fg()).bold().on(ground));
        if let Some(c) = &ask.command {
            text_row(g, &format!("$ {c}"), Style::fg(look::green()).on(ground));
        }
        for line in ask.plan.iter().take(8) {
            text_row(g, line, Style::fg(look::fg()).on(ground));
        }
        for d in ask.diff.iter().take(DIFF_SHOWN) {
            let (tint, fg) = tint_of(d.sign);
            text_row(
                g,
                &format!("{} {}", sign_char(d.sign), d.text),
                Style::fg(fg).on(tint),
            );
        }
        // «Seguir planeando»: the options give way to the line where the instruction is written.
        if let Some((_, editor)) = self.refusing.as_ref().filter(|(id, _)| *id == ask.id) {
            let (hint, written) = (
                "↵ envía (vacío: sigue planeando) · esc vuelve",
                editor.text(),
            );
            let lines = [
                (
                    "Seguir planeando · ¿qué sigue?  (opcional)".to_owned(),
                    look::yellow(),
                ),
                (format!("▸ {written}▏"), look::fg()),
            ];
            for (i, (text, colour)) in lines.iter().enumerate() {
                g.put(
                    tx,
                    opts_at + i,
                    &clip(text, tw),
                    Style::fg(*colour).on(ground),
                );
            }
            g.put(
                tx,
                y + h - 2,
                &clip(hint, tw),
                Style::fg(look::dim()).on(ground),
            );
            return;
        }
        let keys = keys_of(ask);
        for (i, o) in ask.options.iter().enumerate() {
            let at = opts_at + i;
            let lit = i == self.ask_at;
            let bg = if lit { look::sel() } else { ground };
            g.paint_bg(ix + 1, at, iw - 2, bg);
            let mark = match (ask.multi, self.picks.contains(&i)) {
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
                _ => "",
            };
            let n = format!("{}{} {mark}", if lit { "❯ " } else { "  " }, i + 1);
            g.put(tx, at, &n, Style::fg(look::yellow()).bold().on(bg));
            let label = clip(&o.label, tw.saturating_sub(n.chars().count() + 4));
            g.put(
                tx + n.chars().count(),
                at,
                &label,
                Style::fg(look::fg()).on(bg),
            );
            if let Some(k) = keys[i] {
                g.put(
                    (ix + iw).saturating_sub(3),
                    at,
                    &k.to_string(),
                    Style::fg(look::dim()).on(bg),
                );
            }
        }
        g.put(
            tx,
            y + h - 2,
            &clip(ask_hint(ask), tw),
            Style::fg(look::dim()).on(ground),
        );
    }

    /// The checklist Claude keeps, over the conversation: how many lines are done, and the lines when it is open (REQ-434).
    fn draw_todos(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        (ix, y, iw): (usize, usize, usize),
        rows: usize,
    ) {
        if rows == 0 {
            return;
        }
        let done = conv.todos.iter().filter(|t| t.done).count();
        let mark = if done == conv.todos.len() {
            "☑"
        } else {
            "☐"
        };
        g.paint_bg(ix, y, iw, look::panel());
        g.put(
            ix + 1,
            y,
            &format!("{mark} {done}/{} tareas", conv.todos.len()),
            Style::fg(look::fg()).on(look::panel()),
        );
        g.put(
            (ix + iw).saturating_sub(5),
            y,
            "^T",
            Style::fg(look::dim()).on(look::panel()),
        );
        for (i, t) in conv.todos.iter().take(rows - 1).enumerate() {
            g.paint_bg(ix, y + 1 + i, iw, look::panel());
            let style = Style::fg(if t.done { look::dim() } else { look::fg() }).on(look::panel());
            g.put(
                ix + 3,
                y + 1 + i,
                &clip(
                    &format!("{} {}", if t.done { "☑" } else { "☐" }, t.text),
                    iw.saturating_sub(5),
                ),
                style,
            );
        }
    }

    fn draw_foot(&self, g: &mut Grid, conv: &Conversation, (ix, y, iw): (usize, usize, usize)) {
        let (label, colour) = mode_look(conv.mode);
        g.put(ix + 1, y, label, Style::fg(colour));
        let running = running_tasks(conv);
        if running > 0 {
            g.put(
                ix + 2 + label.chars().count(),
                y,
                &format!("⚙ {running}"),
                Style::fg(look::dim()),
            );
        }
        let model = clip(&conv.model, iw.saturating_sub(label.chars().count() + 8));
        g.put(
            (ix + iw).saturating_sub(model.chars().count() + 1),
            y,
            &model,
            Style::fg(look::dim()),
        );
    }

    fn draw_menu(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        (ix, band_y, iw): (usize, usize, usize),
    ) {
        let menu = self.menu(conv);
        if menu.is_empty() {
            return;
        }
        let at = self.menu_at.min(menu.len() - 1);
        let shown = menu.len().min(MENU_SHOWN);
        let first = (at + 1).saturating_sub(shown);
        let h = shown + 2;
        let (x, y) = (ix, band_y.saturating_sub(h));
        g.clear_box((x, y, iw, h));
        g.boxed(
            (x, y, iw, h),
            "comandos",
            accent_rgb(conv.accent),
            Style::fg(look::fg()).bold(),
            Some((
                format!("{} de {}", at + 1, menu.len()),
                Style::fg(look::dim()),
            )),
        );
        let name_w = menu[first..first + shown]
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(0)
            .min(iw / 2);
        for (i, c) in menu[first..first + shown].iter().enumerate() {
            let lit = first + i == at;
            let bg = if lit { look::sel() } else { look::panel() };
            let row = y + 1 + i;
            g.paint_bg(x + 1, row, iw - 2, bg);
            let SourceLook { colour, tag, .. } = source_look(c.source);
            // One that only works in Claude's own screen is dimmed.
            let name_style = if c.kind == CommandKind::Screen {
                Style::fg(look::dim()).on(bg)
            } else {
                Style::fg(colour).bold().on(bg)
            };
            g.put(x + 2, row, &clip(&c.name, name_w), name_style);
            let tag_at = (x + iw).saturating_sub(tag.chars().count() + 3);
            g.put(tag_at, row, tag, Style::fg(colour).on(bg));
            let from = x + 4 + name_w;
            if let Some(d) = &c.description {
                g.put(
                    from,
                    row,
                    &clip(d, tag_at.saturating_sub(from + 1)),
                    Style::fg(look::dim()).on(bg),
                );
            }
        }
    }

    fn draw_files(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        (ix, band_y, iw): (usize, usize, usize),
    ) {
        let h = self.files.len() + 2;
        let (x, y) = (ix, band_y.saturating_sub(h));
        let w = iw;
        g.clear_box((x, y, w, h));
        g.boxed(
            (x, y, w, h),
            "archivos",
            accent_rgb(conv.accent),
            Style::fg(look::fg()).bold(),
            None,
        );
        for (i, path) in self.files.iter().enumerate() {
            let bg = if i == self.file_at {
                look::sel()
            } else {
                look::panel()
            };
            g.paint_bg(x + 1, y + 1 + i, w - 2, bg);
            g.put(
                x + 3,
                y + 1 + i,
                &clip(path, w.saturating_sub(6)),
                Style::fg(look::fg()).on(bg),
            );
        }
    }

    /// A list of jwn's, over the conversation: a box with what is typed to narrow it, and the rows.
    fn draw_list(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        l: &ListState,
        (x, y, w, h): (usize, usize, usize, usize),
        now_ms: u64,
    ) {
        if h < 5 {
            return;
        }
        g.clear_box((x, y, w, h));
        let rows = visible(l);
        let at = l.at.min(rows.len().saturating_sub(1));
        let badge = (!rows.is_empty()).then(|| {
            (
                format!("{} de {}", at + 1, rows.len()),
                Style::fg(look::dim()),
            )
        });
        g.boxed(
            (x, y, w, h),
            &l.panel.title,
            accent_rgb(conv.accent),
            Style::fg(look::fg()).bold(),
            badge,
        );
        let ground = look::panel();
        let filter = if l.filter.is_empty() {
            Style::fg(look::dim()).on(ground)
        } else {
            Style::fg(look::fg()).on(ground)
        };
        let typed = if l.filter.is_empty() {
            "escribe para filtrar".to_owned()
        } else {
            format!("{}▏", l.filter)
        };
        g.put(x + 2, y + 1, &format!("› {typed}"), filter);
        if rows.is_empty() {
            let text = if l.filter.is_empty() {
                l.panel.empty.as_str()
            } else {
                "Nada coincide."
            };
            g.put(
                x + 2,
                y + 3,
                &clip(text, w.saturating_sub(4)),
                Style::fg(look::dim()).on(ground),
            );
            return;
        }
        let capacity = h - 3;
        let first = (at + 1).saturating_sub(capacity);
        let two = rows.iter().any(|r| !r.detail.is_empty());
        let label_w = rows
            .iter()
            .map(|r| r.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(w / 3);
        for (i, r) in rows.iter().skip(first).take(capacity).enumerate() {
            let row = y + 2 + i;
            let lit = first + i == at;
            let bg = if lit { look::sel() } else { ground };
            g.paint_bg(x + 1, row, w - 2, bg);
            let mut right = String::new();
            if let Some(t) = r.at {
                right.push_str(&panels::ago(now_ms, t));
            }
            let tag = r.tag.as_ref().map(|(t, c)| (t.as_str(), *c));
            let tag_w = tag.map_or(0, |(t, _)| t.chars().count() + 2);
            let right_w = right.chars().count() + tag_w;
            let right_x = (x + w).saturating_sub(right_w + 2);
            let avail = right_x.saturating_sub(x + 4);
            let label = if two { label_w } else { avail };
            let label_style = Style::fg(look::fg()).on(bg);
            g.put(
                x + 2,
                row,
                &clip(&r.label, label.min(avail)),
                if lit { label_style.bold() } else { label_style },
            );
            if two && !r.detail.is_empty() {
                let from = x + 4 + label_w;
                g.put(
                    from,
                    row,
                    &clip(&r.detail, right_x.saturating_sub(from + 1)),
                    Style::fg(look::dim()).on(bg),
                );
            }
            if !right.is_empty() {
                g.put(right_x, row, &right, Style::fg(look::dim()).on(bg));
            }
            if let Some((t, c)) = tag {
                g.put(
                    (x + w).saturating_sub(t.chars().count() + 3),
                    row,
                    t,
                    Style::fg(c).on(bg),
                );
            }
        }
    }

    fn draw_pop(
        &self,
        g: &mut Grid,
        conv: &Conversation,
        pop: &Pop,
        (ix, band_y, iw): (usize, usize, usize),
    ) {
        let (title, rows) = match pop {
            Pop::Pick { of, .. } => (of.title(), of.choices().len()),
            Pop::Name(_) => ("nombre", 1),
            Pop::List(_) => return,
        };
        let h = rows + 2;
        let w = 34.min(iw);
        let (x, y) = (ix, band_y.saturating_sub(h));
        g.clear_box((x, y, w, h));
        g.boxed(
            (x, y, w, h),
            title,
            accent_rgb(conv.accent),
            Style::fg(look::fg()).bold(),
            None,
        );
        match pop {
            Pop::Pick { of, at } => {
                let current = current_of(conv, *of);
                for (i, (value, label)) in of.choices().iter().enumerate() {
                    let bg = if i == *at { look::sel() } else { look::panel() };
                    g.paint_bg(x + 1, y + 1 + i, w - 2, bg);
                    let mark = if *value == current { "●" } else { " " };
                    g.put(x + 2, y + 1 + i, mark, Style::fg(look::green()).on(bg));
                    g.put(x + 4, y + 1 + i, label, Style::fg(look::fg()).on(bg));
                }
            }
            Pop::List(_) => {}
            Pop::Name(e) => {
                let text = e.text();
                g.put(
                    x + 2,
                    y + 1,
                    &clip(&text, w.saturating_sub(4)),
                    Style::fg(look::fg()).on(look::panel()),
                );
                let at = (x + 2 + e.cursor()).min(x + w - 3);
                if y + 1 < g.rows && at < g.cols {
                    g.cells[(y + 1) * g.cols + at].1.reverse = true;
                }
            }
        }
    }

    // ---- keys ----

    pub fn key(&mut self, conv: &Conversation, ev: &KeyEvent) -> Option<Act> {
        let node = conv.head.id.clone();
        // A list opened over the box that asks (the whole plan) has the keys until it is closed.
        if matches!(self.pop, Some(Pop::List(_))) {
            return self.pop_key(conv, ev);
        }
        if let Some(ask) = &conv.ask {
            return self.ask_key(&node, ask, ev);
        }
        if self.pop.is_some() {
            return self.pop_key(conv, ev);
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        // Another `claude` has the session: it is read, not written (REQ-575). What changes the
        // field does nothing, whatever is open over it; what reads (the keys of a picked item that
        // copy, scrolling, opening what a tool printed) goes on.
        if let Some(h) = &conv.held {
            let edits = match ev.code {
                KeyCode::Char('c' | 't') if !ctrl && self.picked.is_some() => false,
                KeyCode::Char(c) => !ctrl || !matches!(c, 'o' | 't' | 'b' | 'c'),
                KeyCode::Enter
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Tab
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End => true,
                _ => false,
            };
            if edits {
                self.says = Ui::held_why(h).1;
                return None;
            }
        }
        let menu = self.menu(conv);
        if !menu.is_empty() {
            let at = self.menu_at.min(menu.len() - 1);
            match ev.code {
                KeyCode::Up => {
                    self.menu_at = at.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    self.menu_at = (at + 1).min(menu.len() - 1);
                    return None;
                }
                KeyCode::Tab => {
                    self.editor.set(&format!("{} ", menu[at].name));
                    self.menu_at = 0;
                    return None;
                }
                KeyCode::Enter => return self.run_command(conv, menu[at]),
                KeyCode::Esc => {
                    self.menu_off = self.editor.text();
                    return None;
                }
                _ => {}
            }
        }
        if self.files_open() {
            match ev.code {
                KeyCode::Up => {
                    self.file_at = self.file_at.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    self.file_at = (self.file_at + 1).min(self.files.len() - 1);
                    return None;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    self.take_file(self.file_at);
                    return None;
                }
                KeyCode::Esc => {
                    self.dismissed.clone_from(&self.files_for);
                    return None;
                }
                _ => {}
            }
        }
        if let Some(item) = self.picked {
            match ev.code {
                KeyCode::Char('c') if !ctrl => return self.copy(conv, item, false),
                KeyCode::Char('t') if !ctrl => return self.copy(conv, item, true),
                KeyCode::Esc => {
                    self.picked = None;
                    return None;
                }
                KeyCode::Up | KeyCode::Down => {
                    self.step_item(conv, item, ev.code == KeyCode::Down);
                    return None;
                }
                _ => self.picked = None,
            }
        }
        match ev.code {
            // What Claude proposes goes into the field to be edited or sent (REQ-572).
            KeyCode::Tab | KeyCode::Right if self.ghost(conv).is_some() => {
                if let Some(s) = self.ghost(conv).map(str::to_owned) {
                    self.editor.set(&s);
                }
                None
            }
            KeyCode::Esc if self.ghost(conv).is_some() => {
                self.ghost_off = self.ghost(conv).map(str::to_owned).unwrap_or_default();
                None
            }
            KeyCode::Enter if ev.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.insert("\n");
                None
            }
            KeyCode::Enter => self.submit(conv),
            KeyCode::Char('j') if ctrl => {
                self.editor.insert("\n");
                None
            }
            KeyCode::Char('c') if ctrl => {
                if self.working(conv) {
                    Some(Act::Interrupt { node })
                } else {
                    self.editor.take();
                    None
                }
            }
            KeyCode::Char('o') if ctrl => {
                self.toggle_open(conv);
                None
            }
            KeyCode::Char('t') if ctrl => {
                self.todos_open = !self.todos_open;
                None
            }
            KeyCode::Char('b') if ctrl => self.open_panel(conv, "tasks", ""),
            KeyCode::Esc if self.working(conv) => Some(Act::Interrupt { node }),
            KeyCode::BackTab => Some(Act::Mode {
                node,
                mode: next_mode(conv.mode),
            }),
            KeyCode::PageUp => {
                self.scroll_by(true, PAGE);
                None
            }
            KeyCode::PageDown => {
                self.scroll_by(false, PAGE);
                None
            }
            KeyCode::Up if !self.editor.up() => {
                self.scroll_by(true, 1);
                None
            }
            KeyCode::Down if !self.editor.down() => {
                self.scroll_by(false, 1);
                None
            }
            KeyCode::Up | KeyCode::Down => None,
            _ => {
                self.menu_at = 0;
                self.editor.edit(ev);
                None
            }
        }
    }

    fn toggle_pick(&mut self, i: usize) {
        match self.picks.iter().position(|p| *p == i) {
            Some(at) => {
                self.picks.remove(at);
            }
            None => self.picks.push(i),
        }
    }

    /// The marked options as the daemon's answer to a choice of several: their numbers, with commas.
    fn pick_key(&self) -> String {
        let mut marked = self.picks.clone();
        marked.sort_unstable();
        marked
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// An option of the question was chosen: it answers, except «Seguir planeando» of a plan, which
    /// first asks what to do next (REQ-595).
    fn choose(&mut self, node: &str, ask: &Ask, at: usize) -> Option<Act> {
        let key = ask.options.get(at)?.key.clone();
        if ask.kind == AskKind::Plan && key == "no" {
            self.refusing = Some((ask.id.clone(), Editor::default()));
            return None;
        }
        Some(Act::Answer {
            node: node.to_owned(),
            ask: ask.id.clone(),
            key,
            text: None,
        })
    }

    /// The viewer of the plan asked to keep planning: the box takes the instruction.
    pub fn refuse(&mut self, ask: &str) {
        self.refusing = Some((ask.to_owned(), Editor::default()));
    }

    /// The keys while the instruction for «seguir planeando» is written.
    fn refusing_key(&mut self, node: &str, ask: &Ask, ev: &KeyEvent) -> Option<Act> {
        let (_, editor) = self.refusing.as_mut()?;
        match ev.code {
            KeyCode::Esc => self.refusing = None,
            KeyCode::Enter => {
                let text = editor.text().trim().to_owned();
                self.refusing = None;
                return Some(Act::Answer {
                    node: node.to_owned(),
                    ask: ask.id.clone(),
                    key: "no".into(),
                    text: Some(text).filter(|t| !t.is_empty()),
                });
            }
            _ => {
                editor.edit(ev);
            }
        }
        None
    }

    fn ask_key(&mut self, node: &str, ask: &Ask, ev: &KeyEvent) -> Option<Act> {
        if self.refusing.as_ref().is_some_and(|(id, _)| *id == ask.id) {
            return self.refusing_key(node, ask, ev);
        }
        let n = ask.options.len();
        let by_key = |k: &str| ask.options.iter().position(|o| o.key == k);
        // The whole plan, when the box has room for a few lines of it only.
        if ask.kind == AskKind::Plan && ev.code == KeyCode::Char('v') {
            return Some(Act::ViewPlan {
                node: node.to_owned(),
                ask: ask.id.clone(),
            });
        }
        // A choice of several: a number marks or clears an option and `↵` sends the marked ones.
        if ask.multi {
            match ev.code {
                KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < n => {
                    self.toggle_pick(c as usize - '1' as usize);
                    return None;
                }
                KeyCode::Char(' ') => {
                    self.toggle_pick(self.ask_at.min(n.saturating_sub(1)));
                    return None;
                }
                KeyCode::Enter if !self.picks.is_empty() => {
                    return Some(Act::Answer {
                        node: node.to_owned(),
                        ask: ask.id.clone(),
                        key: self.pick_key(),
                        text: None,
                    });
                }
                _ => {}
            }
        }
        let at = match ev.code {
            KeyCode::Char(c @ '1'..='9') if !ask.multi => {
                Some(c as usize - '1' as usize).filter(|i| *i < n)
            }
            KeyCode::Char('y') => by_key("yes"),
            KeyCode::Char('a') => by_key("session"),
            KeyCode::Char('n') => by_key("no"),
            KeyCode::Up => {
                self.ask_at = self.ask_at.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                self.ask_at = (self.ask_at + 1).min(n.saturating_sub(1));
                None
            }
            KeyCode::Enter if !ask.multi => Some(self.ask_at).filter(|i| *i < n),
            _ => None,
        }?;
        self.choose(node, ask, at)
    }

    fn pop_key(&mut self, conv: &Conversation, ev: &KeyEvent) -> Option<Act> {
        let node = conv.head.id.clone();
        match self.pop.as_mut()? {
            Pop::Name(e) => match ev.code {
                KeyCode::Esc => self.pop = None,
                KeyCode::Enter => {
                    let name = e.text().trim().to_owned();
                    self.pop = None;
                    return (!name.is_empty()).then_some(Act::Name { node, name });
                }
                _ => {
                    e.edit(ev);
                }
            },
            Pop::List(l) => {
                let n = visible(l).len();
                match ev.code {
                    KeyCode::Esc => self.pop = None,
                    KeyCode::Up => l.at = l.at.saturating_sub(1),
                    KeyCode::Down => l.at = (l.at + 1).min(n.saturating_sub(1)),
                    KeyCode::PageUp => l.at = l.at.saturating_sub(PAGE),
                    KeyCode::PageDown => l.at = (l.at + PAGE).min(n.saturating_sub(1)),
                    KeyCode::Enter => {
                        let act = visible(l).get(l.at).map(|r| r.act.clone());
                        if let Some(act) = act {
                            self.pop = None;
                            return self.row_act(conv, act);
                        }
                    }
                    // `x` stops the task the row is on (REQ-433), and only where nothing is typed to narrow the list.
                    KeyCode::Char('x')
                        if l.filter.is_empty()
                            && visible(l)
                                .get(l.at)
                                .is_some_and(|r| matches!(r.act, RowAct::Stop(_))) =>
                    {
                        let act = visible(l).get(l.at).map(|r| r.act.clone());
                        if let Some(act) = act {
                            self.pop = None;
                            return self.row_act(conv, act);
                        }
                    }
                    KeyCode::Backspace => {
                        l.filter.pop();
                        l.at = 0;
                    }
                    KeyCode::Char(c) if !ev.modifiers.contains(KeyModifiers::CONTROL) => {
                        l.filter.push(c);
                        l.at = 0;
                    }
                    _ => {}
                }
            }
            Pop::Pick { of, at } => {
                let n = of.choices().len();
                match ev.code {
                    KeyCode::Esc => self.pop = None,
                    KeyCode::Up | KeyCode::Char('k') => *at = at.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => *at = (*at + 1).min(n - 1),
                    KeyCode::Enter => {
                        let act = choose(&node, *of, *at);
                        self.pop = None;
                        return act;
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Run a command chosen from the `/` list: it goes as the line, and the daemon refuses one
    /// that needs Claude's own screen (REQ-423).
    fn run_command(&mut self, conv: &Conversation, c: &ConvCommand) -> Option<Act> {
        self.menu_at = 0;
        self.editor.set(&c.name);
        self.submit(conv)
    }

    /// `↵`: send the line, or open the list a bare command stands for.
    fn submit(&mut self, conv: &Conversation) -> Option<Act> {
        if let Some(h) = &conv.held {
            self.says = Ui::held_why(h).1;
            return None;
        }
        let node = conv.head.id.clone();
        let text = self.editor.text();
        let line = text.trim();
        if line.is_empty() {
            return None;
        }
        if let Some(rest) = line.strip_prefix('/') {
            let (word, arg) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(w, a)| (w, a.trim()));
            if panels::is_panel(word) {
                self.editor.take();
                return self.open_panel(conv, word, arg);
            }
        }
        let bare = match line {
            "/model" => Some(Of::Model),
            "/effort" => Some(Of::Effort),
            "/mode" => Some(Of::Mode),
            "/color" => Some(Of::Color),
            _ => None,
        };
        if let Some(of) = bare {
            self.editor.take();
            self.open_pick(conv, of);
            return None;
        }
        if line == "/rename" {
            self.editor.take();
            self.open_name(conv);
            return None;
        }
        let line = line.to_owned();
        self.editor.take();
        self.scroll = 0;
        if let Some(command) = line
            .strip_prefix('!')
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            return Some(Act::Shell {
                node,
                command: command.to_owned(),
            });
        }
        if matches!(conv.status, ConvStatus::Dead | ConvStatus::Resuming) {
            self.retaking = true;
        }
        // Only the pictures whose mark is still in the line go.
        let images = std::mem::take(&mut self.images)
            .into_iter()
            .filter(|(n, _)| line.contains(&format!("[Imagen#{n}]")))
            .map(|(_, image)| image)
            .collect();
        Some(Act::Send {
            node,
            text: line,
            images,
        })
    }

    fn open_pick(&mut self, conv: &Conversation, of: Of) {
        let current = current_of(conv, of);
        let at = of
            .choices()
            .iter()
            .position(|(v, _)| *v == current)
            .unwrap_or(0);
        self.pop = Some(Pop::Pick { of, at });
    }

    fn copy(&mut self, conv: &Conversation, item: u32, turn: bool) -> Option<Act> {
        let text = if turn {
            turn_text(conv, item)
        } else {
            item_text(conv, item)
        };
        if text.is_empty() {
            return None;
        }
        self.says = copied(&text);
        Some(Act::Copy(text))
    }

    /// The item before or after the one picked.
    fn step_item(&mut self, conv: &Conversation, item: u32, down: bool) {
        let last = conv.first + conv.items.len() as u32;
        let runs = fold::runs(&conv.items);
        let mut next = item;
        loop {
            let stepped = if down {
                (next + 1).min(last.saturating_sub(1))
            } else {
                next.saturating_sub(1).max(conv.first)
            };
            if stepped == next {
                break;
            }
            next = stepped;
            // An item inside a run that is folded has no row to be picked at.
            if !self.hidden(conv, &runs, next) {
                self.picked = Some(next);
                return;
            }
        }
        // The end was reached inside a folded run: nothing to pick that way, so it stays.
        self.picked = Some(item);
    }

    /// Whether an item is in a run of tools that is folded, so no row of its own is drawn.
    fn hidden(&self, conv: &Conversation, runs: &[fold::Run], abs: u32) -> bool {
        let at = abs.wrapping_sub(conv.first) as usize;
        runs.iter().any(|r| {
            (r.first..=r.last).contains(&at)
                && !self.unfolded.contains(&(conv.first + r.first as u32))
        })
    }

    /// `^O`: open or close the output of the item picked, or of the last tool or agent.
    fn toggle_open(&mut self, conv: &Conversation) {
        let target = self.picked.or_else(|| {
            conv.items
                .iter()
                .rposition(|i| {
                    matches!(
                        i,
                        Item::Tool { .. } | Item::Agent { .. } | Item::Shell { .. }
                    )
                })
                .map(|i| conv.first + i as u32)
        });
        let Some(t) = target else {
            return;
        };
        // What is in a run that is folded opens by opening the run: the output comes after (REQ-558).
        let at = t.wrapping_sub(conv.first) as usize;
        if let Some(run) = fold::runs(&conv.items)
            .iter()
            .find(|r| (r.first..=r.last).contains(&at))
            .filter(|r| !self.unfolded.contains(&(conv.first + r.first as u32)))
        {
            self.unfolded.insert(conv.first + run.first as u32);
            return;
        }
        if !self.open.remove(&t) {
            self.open.insert(t);
        }
    }

    pub fn scroll_by(&mut self, up: bool, n: usize) {
        if let Some(Pop::List(l)) = &mut self.pop {
            let last = visible(l).len().saturating_sub(1);
            l.at = if up {
                l.at.saturating_sub(n)
            } else {
                (l.at + n).min(last)
            };
            return;
        }
        self.scroll = if up {
            (self.scroll + n).min(self.most.get())
        } else {
            self.scroll.saturating_sub(n)
        };
    }

    fn list_rows(&self, geo: &Geo, total: usize) -> ListRows {
        let (ly, lh) = geo.list;
        let most = total.saturating_sub(lh);
        ListRows {
            top: ly + lh.saturating_sub(total),
            bottom: ly + lh,
            start: most - self.scroll.min(most),
            most,
        }
    }

    // ---- selecting with the mouse (REQ-461 to REQ-464) ----

    /// Let go of what a drag covers, and of a drag.
    pub fn unlight(&mut self) {
        self.sel = None;
        self.pull = None;
    }

    /// A press on the list begins a drag; `true` when one began. The press is a click if the
    /// pointer never moves (see `release`).
    pub fn press(
        &mut self,
        conv: &Conversation,
        (ix, iy, iw, ih): (usize, usize, usize, usize),
        x: usize,
        y: usize,
    ) -> bool {
        self.unlight();
        if self.pop.is_some() || iw < 12 || ih < 8 {
            return false;
        }
        let geo = self.geo(conv, (iy, ih), self.field(iw).band_rows());
        let rows = self.list_rows(&geo, self.lines(conv, iw - 2).len());
        if !(rows.top..rows.bottom).contains(&y) || !(ix..ix + iw).contains(&x) {
            return false;
        }
        let anchor = select::Point {
            line: rows.start + (y - rows.top),
            col: x.saturating_sub(ix + 2),
        };
        self.pull = Some(select::Pull::new((x, y), anchor));
        true
    }

    /// The pointer moved with the button down: the drag covers from where it began to the line
    /// and column it is on, or, past the top or the bottom of the list, to its first or last
    /// line, which scrolls (REQ-462).
    pub fn drag_to(
        &mut self,
        conv: &Conversation,
        (ix, iy, iw, ih): (usize, usize, usize, usize),
        (x, y): (usize, usize),
        now_ms: u64,
    ) {
        if self.pull.is_none() || iw < 12 || ih < 8 {
            return;
        }
        let geo = self.geo(conv, (iy, ih), self.field(iw).band_rows());
        let rows = self.list_rows(&geo, self.lines(conv, iw - 2).len());
        let (head, edge) = if y < rows.top {
            (
                select::Point {
                    line: rows.start,
                    col: 0,
                },
                Some(true),
            )
        } else if y >= rows.bottom {
            (
                select::Point {
                    line: rows.start + (rows.bottom - rows.top).saturating_sub(1),
                    col: usize::from(u16::MAX),
                },
                Some(false),
            )
        } else {
            (
                select::Point {
                    line: rows.start + (y - rows.top),
                    col: x.saturating_sub(ix + 2),
                },
                None,
            )
        };
        let Some(pull) = self.pull.as_mut() else {
            return;
        };
        pull.to(head, edge, now_ms);
        self.sel = Some(select::Selection {
            pane: conv.head.id.clone(),
            anchor: pull.anchor,
            head,
        });
    }

    /// While a drag is held past the edge of the list, scroll it a line every so often and keep
    /// the drag at its edge.
    pub fn drag_tick(
        &mut self,
        conv: &Conversation,
        (_, iy, iw, ih): (usize, usize, usize, usize),
        now_ms: u64,
    ) {
        let Some(up) = self.pull.as_ref().and_then(|p| p.due(now_ms)) else {
            return;
        };
        if iw < 12 || ih < 8 {
            return;
        }
        self.scroll_by(up, 1);
        let geo = self.geo(conv, (iy, ih), self.field(iw).band_rows());
        let rows = self.list_rows(&geo, self.lines(conv, iw - 2).len());
        let (line, col) = if up {
            (rows.start, 0)
        } else {
            (
                rows.start + (rows.bottom - rows.top).saturating_sub(1),
                usize::from(u16::MAX),
            )
        };
        if let (Some(pull), Some(sel)) = (self.pull.as_mut(), self.sel.as_mut()) {
            pull.scrolled(now_ms);
            sel.head = select::Point { line, col };
        }
    }

    /// The button was let go: a drag that never moved is the click it was; one that did copies
    /// what it covers (REQ-463) and leaves it lit until the next press or key.
    pub fn release(
        &mut self,
        conv: &Conversation,
        (_, _, iw, _): (usize, usize, usize, usize),
    ) -> Released {
        let Some(pull) = self.pull.take() else {
            return Released::Nothing;
        };
        if !pull.moved {
            self.sel = None;
            return Released::Click(pull.start.0, pull.start.1);
        }
        let text = self
            .sel
            .as_ref()
            .map(|sel| selected_text(&self.lines(conv, iw.saturating_sub(2)), sel))
            .unwrap_or_default();
        if text.is_empty() {
            self.sel = None;
            return Released::Nothing;
        }
        self.says = copied(&text);
        Released::Copy(text)
    }

    // ---- the mouse ----

    /// A click at (`x`, `y`) in a pane whose box is `rect` (`(ix, iy, iw, ih)` its inner part).
    pub fn click(
        &mut self,
        conv: &Conversation,
        (ix, iy, iw, ih): (usize, usize, usize, usize),
        x: usize,
        y: usize,
    ) -> Option<Act> {
        if iw < 12 || ih < 8 {
            return None;
        }
        let node = conv.head.id.clone();
        let field = self.field(iw);
        let geo = self.geo(conv, (iy, ih), field.band_rows());
        // The strip of the checklist opens and closes with a click.
        if self.pop.is_none() && geo.todo_rows > 0 && y == iy {
            self.todos_open = !self.todos_open;
            return None;
        }
        if self.pop.is_none() && conv.ask.is_none() && self.files_open() {
            let h = self.files.len() + 2;
            let top = geo.band.0.saturating_sub(h);
            if y > top && y + 1 < top + h {
                self.take_file(y - top - 1);
            } else {
                self.dismissed.clone_from(&self.files_for);
            }
            return None;
        }
        if let Some(Pop::List(l)) = &mut self.pop {
            let top = geo.list.0;
            let h = geo.band.0.saturating_sub(top);
            let rows = visible(l);
            let at = l.at.min(rows.len().saturating_sub(1));
            let capacity = h.saturating_sub(3);
            let first = (at + 1).saturating_sub(capacity);
            if h >= 5
                && y >= top + 2
                && y < top + 2 + capacity
                && first + (y - top - 2) < rows.len()
            {
                let act = rows[first + y - top - 2].act.clone();
                self.pop = None;
                return self.row_act(conv, act);
            }
            self.pop = None;
            return None;
        }
        let menu = self.menu(conv);
        if self.pop.is_none() && conv.ask.is_none() && !menu.is_empty() {
            let shown = menu.len().min(MENU_SHOWN);
            let first = (self.menu_at.min(menu.len() - 1) + 1).saturating_sub(shown);
            let h = shown + 2;
            let top = geo.band.0.saturating_sub(h);
            if y > top && y + 1 < top + h {
                return self.run_command(conv, menu[first + y - top - 1]);
            }
        }
        if let Some(Pop::Pick { of, .. }) = &self.pop {
            let of = *of;
            let h = of.choices().len() + 2;
            let (px, py) = (ix, geo.band.0.saturating_sub(h));
            if (px..px + 34.min(iw)).contains(&x) && y > py && y + 1 < py + h {
                let act = choose(&node, of, y - py - 1);
                self.pop = None;
                return act;
            }
            self.pop = None;
            return None;
        }
        if self.pop.is_some() {
            return None;
        }
        if y == geo.foot {
            let (label, _) = mode_look(conv.mode);
            let left = ix + 1..ix + 1 + label.chars().count();
            let right = (ix + iw).saturating_sub(conv.model.chars().count() + 1)..ix + iw;
            let running = running_tasks(conv);
            let gear_at = ix + 2 + label.chars().count();
            if left.contains(&x) {
                self.open_pick(conv, Of::Mode);
            } else if running > 0 && (gear_at..gear_at + 3 + running.to_string().len()).contains(&x)
            {
                return self.open_panel(conv, "tasks", "");
            } else if right.contains(&x) {
                self.open_pick(conv, Of::Model);
            }
            return None;
        }
        if let Some(ask) = &conv.ask {
            let n = ask.options.len();
            let first = (geo.band.0 + geo.band.1).saturating_sub(2 + n);
            if y >= first && y < first + n {
                self.ask_at = y - first;
                if ask.multi {
                    self.toggle_pick(y - first);
                    return None;
                }
                return self.choose(&node, ask, y - first);
            }
            // A click on what a plan says opens it whole.
            if ask.kind == AskKind::Plan && y > geo.band.0 && y < first {
                return Some(Act::ViewPlan {
                    node,
                    ask: ask.id.clone(),
                });
            }
            return None;
        }
        let first_text = geo.band.0 + Field::FIRST_TEXT;
        if y == first_text && x == field::send_at(ix, iw) {
            return self.submit(conv);
        }
        if y == first_text
            && self.working(conv)
            && (field::stop_at(ix, iw)..field::stop_at(ix, iw) + 5).contains(&x)
        {
            return Some(Act::Interrupt { node });
        }
        let (ly, lh) = geo.list;
        if (ly..ly + lh).contains(&y) {
            let lines = self.lines(conv, iw - 2);
            let most = lines.len().saturating_sub(lh);
            let start = most - self.scroll.min(most);
            let top = ly + lh.saturating_sub(lines.len());
            let line = y.checked_sub(top).and_then(|r| lines.get(start + r));
            // The row above the first item: a tap asks again after a failure.
            if line.is_some_and(|l| l.item == older::TOP) {
                self.older.retry();
                return None;
            }
            // A row that says a run of tools opens or closes it, and picks nothing (REQ-558).
            if let Some(l) = line.filter(|l| l.fold) {
                if !self.unfolded.remove(&l.item) {
                    self.unfolded.insert(l.item);
                }
                // What was picked may have just lost its row.
                if let Some(p) = self.picked
                    && self.hidden(conv, &fold::runs(&conv.items), p)
                {
                    self.picked = None;
                }
                return None;
            }
            let hit = line.map(|l| l.item);
            // A message that waits is not an item: there is nothing of it to pick or copy.
            let hit = hit.filter(|i| ((i - conv.first) as usize) < conv.items.len());
            self.picked = hit;
            if let Some(i) = hit
                && let Some(item) = conv.items.get((i - conv.first) as usize)
                && matches!(
                    item,
                    Item::Tool { .. } | Item::Agent { .. } | Item::Shell { .. }
                )
                && !self.open.remove(&i)
            {
                self.open.insert(i);
            }
        }
        None
    }

    /// A click on the pane's title (`rect` is the pane's whole box): the name can be changed.
    pub fn click_title(
        &mut self,
        conv: &Conversation,
        (x, y, w, _): (usize, usize, usize, usize),
        cx: usize,
        cy: usize,
    ) -> bool {
        let len = title(conv).chars().count().min(w.saturating_sub(6)) + 2;
        if cy == y && cx >= x + 2 && cx < x + 2 + len {
            self.open_name(conv);
            return true;
        }
        false
    }

    /// The field for the conversation's name, with the one it has.
    fn open_name(&mut self, conv: &Conversation) {
        let mut name = Editor::default();
        name.set(&conv.name);
        self.pop = Some(Pop::Name(name));
    }
}

/// The rows of a list that hold what is typed to narrow it, wherever they hold it.
fn visible(l: &ListState) -> Vec<&panels::Row> {
    let typed = l.filter.to_lowercase();
    l.panel
        .rows
        .iter()
        .filter(|r| {
            typed.is_empty()
                || r.label.to_lowercase().contains(&typed)
                || r.detail.to_lowercase().contains(&typed)
                || r.tag
                    .as_ref()
                    .is_some_and(|(t, _)| t.to_lowercase().contains(&typed))
        })
        .collect()
}

/// The pane's title: what it runs and the conversation's name.
pub fn title(conv: &Conversation) -> String {
    format!("claude · {}", conv.name)
}

/// How a command's source is shown: its place in the list, the colour it is written in and the
/// word that says it.
struct SourceLook {
    rank: usize,
    colour: Rgb,
    tag: &'static str,
}

fn source_look(s: CommandSource) -> SourceLook {
    let (rank, colour, tag) = match s {
        CommandSource::Jwn => (0, look::blue(), "jwn"),
        CommandSource::Claude => (1, look::magenta(), "claude"),
        CommandSource::Project => (2, look::green(), "proyecto"),
        CommandSource::Skill => (3, look::yellow(), "skill"),
        CommandSource::Plugin => (4, look::cyan(), "plugin"),
    };
    SourceLook { rank, colour, tag }
}

pub fn accent_rgb(a: Accent) -> Rgb {
    match a {
        Accent::Blue => look::blue(),
        Accent::Green => look::green(),
        Accent::Yellow => look::yellow(),
        Accent::Red => look::red(),
        Accent::Magenta => look::magenta(),
        Accent::Cyan => look::cyan(),
    }
}

/// How a mode is written in the footer: dim for manual, magenta for accept edits, cyan for plan and yellow for auto (REQ-402).
fn mode_look(m: Mode) -> (&'static str, Rgb) {
    match m {
        Mode::Manual => ("manual", look::dim()),
        Mode::AcceptEdits => ("accept edits", look::magenta()),
        Mode::Plan => ("plan", look::cyan()),
        Mode::Auto => ("auto", look::yellow()),
    }
}

fn next_mode(m: Mode) -> Mode {
    match m {
        Mode::Manual => Mode::AcceptEdits,
        Mode::AcceptEdits => Mode::Plan,
        Mode::Plan => Mode::Auto,
        Mode::Auto => Mode::Manual,
    }
}

/// The value the conversation has now, as the list names it.
fn current_of(conv: &Conversation, of: Of) -> String {
    match of {
        Of::Model => MODELS
            .iter()
            .find(|(v, _)| conv.model.contains(v))
            .map_or(conv.model.clone(), |(v, _)| (*v).to_owned()),
        Of::Effort => conv.effort.clone(),
        Of::Mode => conv.mode.flag().to_owned(),
        Of::Color => ACCENTS
            .iter()
            .position(|a| *a == conv.accent)
            .map_or_else(String::new, |i| COLORS[i].0.to_owned()),
    }
}

fn choose(node: &str, of: Of, at: usize) -> Option<Act> {
    let (value, _) = *of.choices().get(at)?;
    let node = node.to_owned();
    Some(match of {
        Of::Model => Act::Model {
            node,
            model: value.to_owned(),
        },
        Of::Effort => Act::Effort {
            node,
            effort: value.to_owned(),
        },
        Of::Mode => Act::Mode {
            node,
            mode: Mode::from_flag(value)?,
        },
        Of::Color => Act::Color {
            node,
            accent: *ACCENTS.get(at)?,
        },
    })
}

fn state_colour(s: ToolState) -> Rgb {
    match s {
        ToolState::Run | ToolState::Ask => look::yellow(),
        ToolState::Ok => look::green(),
        ToolState::Denied | ToolState::Failed => look::red(),
    }
}

fn sign_char(s: DiffSign) -> char {
    match s {
        DiffSign::Add => '+',
        DiffSign::Del => '-',
        DiffSign::Same => ' ',
    }
}

/// A diff line's background and text colour, from the theme.
fn tint_of(s: DiffSign) -> (Rgb, Rgb) {
    match s {
        DiffSign::Add => (look::add_bg(), look::fg()),
        DiffSign::Del => (look::del_bg(), look::fg()),
        DiffSign::Same => (look::panel(), look::dim()),
    }
}

/// The shortcut letter each option of a permission has.
fn keys_of(ask: &Ask) -> Vec<Option<char>> {
    ask.options
        .iter()
        .map(|o| match o.key.as_str() {
            "yes" => Some('y'),
            "session" => Some('a'),
            "no" => Some('n'),
            _ => None,
        })
        .collect()
}

/// What keys answer the box that asks.
fn ask_hint(ask: &Ask) -> &'static str {
    match (ask.kind, ask.multi) {
        (AskKind::Plan, _) => "1 2 3 · v ver el plan · ↑↓ ↵",
        (_, true) => "1 2 3 marcan · ↵ envía lo marcado · ↑↓",
        (AskKind::Question, false) => "1 2 3 · ↑↓ ↵",
        _ => "1 2 3 · y a n · ↑↓ ↵",
    }
}

/// Rows the box that asks takes: its borders, its title, what it is about, the options and the hint.
fn ask_rows(a: &Ask) -> usize {
    3 + usize::from(a.command.is_some())
        + a.plan.len().min(8)
        + a.diff.len().min(DIFF_SHOWN)
        + a.options.len()
        + 1
}

fn diff_lines(
    diff: &[DiffLine],
    open: bool,
    w: usize,
    push: &mut impl FnMut(Vec<(String, Style)>, Option<Rgb>),
) {
    let shown = if open { OPEN_SHOWN } else { DIFF_SHOWN };
    for d in diff.iter().take(shown) {
        let (tint, fg) = tint_of(d.sign);
        let n = d.n.map_or("    ".to_owned(), |n| format!("{n:>4}"));
        let text = format!("{n} {} {}", sign_char(d.sign), d.text);
        push(
            vec![
                ("     ".to_owned(), Style::fg(fg).on(tint)),
                (clip(&text, w.saturating_sub(5)), Style::fg(fg).on(tint)),
            ],
            (d.sign != DiffSign::Same).then_some(tint),
        );
    }
    if diff.len() > shown {
        push(
            vec![(
                format!("     … {} líneas más · ^O", diff.len() - shown),
                Style::fg(look::dim()),
            )],
            None,
        );
    }
}

pub fn secs(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

fn kilo(n: u64) -> String {
    if n >= 1000 {
        // Whole tenths with a comma, rounded up from the half, as the phone's `convKilo` does.
        let tenths = (n + 50) / 100;
        format!("{},{} k", tenths / 10, tenths % 10)
    } else {
        n.to_string()
    }
}

fn usd(micro: u64) -> String {
    format!("${},{:02}", micro / 1_000_000, (micro % 1_000_000) / 10_000)
}

// ---- text ----

const CODE: u8 = 1;
const BOLD: u8 = 2;

/// `text` broken into rows of at most `width` columns, with `` `code` `` in `code` and `**bold**`
/// bold; a fenced block is code as it is. A row that continues a list item hangs under its text.
/// Each row says whether it goes on from the one before it, which the width broke.
fn wrap(text: &str, width: usize, plain: Style, code: Style) -> Vec<(bool, Vec<(String, Style)>)> {
    let width = width.max(4);
    let mut rows = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            rows.push((false, vec![(clip(line, width), Style::fg(look::dim()))]));
            continue;
        }
        if fenced {
            rows.push((false, vec![(clip(line, width), code)]));
            continue;
        }
        let tagged = tag(line);
        let hang = hang_of(line);
        for (k, row) in break_words(&tagged, width, hang).into_iter().enumerate() {
            rows.push((k > 0, spans_of(&row, plain, code)));
        }
    }
    if rows.is_empty() {
        rows.push((false, vec![]));
    }
    rows
}

/// The characters of a line with what they are: code or bold, the marks themselves dropped.
fn tag(line: &str) -> Vec<(char, u8)> {
    let heading = line.starts_with('#');
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let (mut code, mut bold) = (false, heading);
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => code = !code,
            '*' if !code && chars.get(i + 1) == Some(&'*') => {
                bold = !bold;
                i += 1;
            }
            c => out.push((c, if code { CODE } else { 0 } | if bold { BOLD } else { 0 })),
        }
        i += 1;
    }
    out
}

/// Columns a wrapped row of a list item hangs by: the indent and the bullet.
fn hang_of(line: &str) -> usize {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    let bullet = if rest.starts_with("- ") || rest.starts_with("* ") {
        2
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(". ") {
            digits + 2
        } else {
            0
        }
    };
    indent + bullet
}

fn break_words(cells: &[(char, u8)], width: usize, hang: usize) -> Vec<Vec<(char, u8)>> {
    let hang = hang.min(width / 2);
    let fresh = || vec![(' ', 0); hang];
    let mut rows: Vec<Vec<(char, u8)>> = vec![vec![]];
    let mut i = 0;
    while i < cells.len() {
        let mut end = i;
        while end < cells.len() && cells[end].0 != ' ' {
            end += 1;
        }
        let word_end = end;
        while end < cells.len() && cells[end].0 == ' ' {
            end += 1;
        }
        let (mut word, spaces) = (&cells[i..word_end], &cells[word_end..end]);
        i = end;
        let has_text = |r: &Vec<(char, u8)>| r.iter().any(|(c, _)| *c != ' ');
        if rows
            .last()
            .is_some_and(|r| r.len() + word.len() > width && has_text(r))
        {
            rows.push(fresh());
        }
        while let Some(row) = rows.last_mut() {
            let room = width.saturating_sub(row.len());
            if word.len() <= room || room == 0 {
                break;
            }
            row.extend_from_slice(&word[..room]);
            word = &word[room..];
            rows.push(fresh());
        }
        if let Some(row) = rows.last_mut() {
            row.extend_from_slice(word);
            row.extend_from_slice(spaces);
        }
    }
    for row in &mut rows {
        while row.last().is_some_and(|(c, _)| *c == ' ') {
            row.pop();
        }
    }
    rows
}

fn spans_of(row: &[(char, u8)], plain: Style, code: Style) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut last = None;
    for (c, flags) in row {
        if last == Some(*flags) {
            if let Some((s, _)) = out.last_mut() {
                s.push(*c);
            }
            continue;
        }
        let mut style = if flags & CODE != 0 { code } else { plain };
        if flags & BOLD != 0 {
            style = style.bold();
        }
        out.push((c.to_string(), style));
        last = Some(*flags);
    }
    out
}

// ---- what is copied ----

/// «copiado · 3 líneas · 120 caracteres».
pub fn copied(text: &str) -> String {
    let lines = text.lines().count();
    format!(
        "copiado · {lines} {} · {} caracteres",
        if lines == 1 { "línea" } else { "líneas" },
        text.chars().count()
    )
}

/// What a drag covers, as it reads: the marks that begin a row left out, the rows a width broke
/// joined into one line and the blank lines between items kept (REQ-463).
fn selected_text(lines: &[Line], sel: &select::Selection) -> String {
    let (a, b) = sel.ends();
    let mut out: Vec<String> = Vec::new();
    for at in a.line..=b.line {
        let Some(line) = lines.get(at) else {
            break;
        };
        let text: Vec<char> = line.text().chars().collect();
        let Some((from, to)) = sel.on_line(at, text.len()) else {
            if line.spans.is_empty() {
                out.push(String::new());
            }
            continue;
        };
        let from = from.max(line.lead);
        if to <= from {
            continue;
        }
        let piece: String = text[from..to].iter().collect();
        match out.last_mut() {
            Some(last) if line.soft && !last.is_empty() => {
                last.push(' ');
                last.push_str(piece.trim());
            }
            _ => out.push(piece.trim_end().to_owned()),
        }
    }
    while out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// An item as the text that is copied from it.
fn plain_of(item: &Item) -> String {
    match item {
        Item::User { text, .. } | Item::Text { text, .. } | Item::Note { text, .. } => text.clone(),
        Item::Tool {
            name,
            arg,
            summary,
            output,
            diff,
            ..
        } => {
            let mut v = vec![format!("{name}({arg})")];
            v.extend(summary.iter().cloned());
            v.extend(
                diff.iter()
                    .map(|d| format!("{} {}", sign_char(d.sign), d.text)),
            );
            v.extend(output.iter().cloned());
            v.join("\n")
        }
        Item::Agent {
            name,
            description,
            steps,
            ..
        } => std::iter::once(format!("Agent({name}) {description}"))
            .chain(steps.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n"),
        Item::Shell {
            command, output, ..
        } => std::iter::once(format!("! {command}"))
            .chain(output.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n"),
        Item::Out { lines } => lines.join("\n"),
        Item::Result { ms, tools, cost } => done_line(*ms, *tools, *cost),
        Item::Divider { text, .. } => text.clone(),
    }
}

/// What ends a turn says: how long, how many tools and, when it is known, what it cost (a turn read
/// from a transcript has no cost, REQ-552).
fn done_line(ms: u64, tools: u32, cost: u64) -> String {
    let mut line = format!("Listo en {} · {}", secs(ms), tools_word(tools));
    if cost > 0 {
        line.push_str(&format!(" · {}", usd(cost)));
    }
    line
}

fn item_text(conv: &Conversation, abs: u32) -> String {
    conv.items
        .get(abs.wrapping_sub(conv.first) as usize)
        .map(plain_of)
        .unwrap_or_default()
}

/// The whole turn the item belongs to: from the person's message to the next one.
fn turn_text(conv: &Conversation, abs: u32) -> String {
    let at = abs.wrapping_sub(conv.first) as usize;
    if at >= conv.items.len() {
        return String::new();
    }
    let start = conv.items[..=at]
        .iter()
        .rposition(|i| matches!(i, Item::User { .. }))
        .unwrap_or(0);
    let end = conv.items[at + 1..]
        .iter()
        .position(|i| matches!(i, Item::User { .. }))
        .map_or(conv.items.len(), |p| at + 1 + p);
    conv.items[start..end]
        .iter()
        .filter(|i| !matches!(i, Item::Divider { .. }))
        .map(plain_of)
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The conversation of the shared fixture: a permission waits for an edit.
    fn fixture() -> Conversation {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-conversation.json")).unwrap(),
        )
        .unwrap();
        serde_json::from_value(doc["node"].clone()).unwrap()
    }

    /// The same with nothing waiting and nothing running.
    fn idle() -> Conversation {
        let mut c = fixture();
        c.ask = None;
        c.status = ConvStatus::Idle;
        c.since = None;
        c
    }

    const INNER: (usize, usize, usize, usize) = (2, 1, 96, 58);

    fn frame(ui: &Ui, conv: &Conversation) -> (Grid, Vec<String>) {
        let mut g = Grid::new(100, 60);
        let chips = [("…/proj".to_owned(), look::dim())];
        ui.draw(&mut g, conv, INNER, &chips, true, 1_792_110_012_000);
        let rows = g.rows_plain();
        (g, rows)
    }

    /// A `Ui` with every run of tools open, to see the tools one by one.
    fn unfolded(conv: &Conversation) -> Ui {
        let mut ui = Ui::default();
        for run in fold::runs(&conv.items) {
            ui.unfolded.insert(conv.first + run.first as u32);
        }
        ui
    }

    fn row_of(rows: &[String], text: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("{text:?} is not drawn:\n{}", rows.join("\n")))
    }

    fn press(ui: &mut Ui, conv: &Conversation, code: KeyCode) -> Option<Act> {
        ui.key(conv, &KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_in(ui: &mut Ui, conv: &Conversation, text: &str) {
        for c in text.chars() {
            assert_eq!(press(ui, conv, KeyCode::Char(c)), None);
        }
    }

    #[test]
    fn each_item_is_drawn_as_the_prototype_has_it() {
        let conv = idle();
        let (_, rows) = frame(&unfolded(&conv), &conv);
        for text in [
            "❯ arregla el test que falla en auth::expired",
            "▣ 1 imagen(es)",
            "● Voy a mirar el test y el código que prueba.▍",
            "● Read(src/auth.rs)",
            "⎿  Leídas 142 líneas",
            "● Edit(src/auth.rs)",
            "  41 - ",
            "  41 + ",
            "● Artifact(Informe de pruebas)",
            "permitido desde el móvil",
            "https://claude.ai/artifact/6HbvGgivzfNgDnkzpDjf4Y",
            "● Agent(Explore) buscar los usos de verify",
            "3 herramientas · 22,0 k tokens · 6 s",
            "! git status",
            "nothing to commit, working tree clean",
            "⎿  contexto 12 % · 24k de 200k",
            "⎿  Interrumpido por ti",
            "✻ Listo en 11 s · 3 herramientas · $0,06",
            "el daemon se reinició · 20:10",
        ] {
            row_of(&rows, text);
        }
        assert!(
            !rows.iter().any(|r| r.contains("pub fn verify")),
            "the output of a tool is under a click"
        );
    }

    #[test]
    fn a_message_has_its_band_and_the_accent_marks_it() {
        let conv = idle();
        let (g, rows) = frame(&Ui::default(), &conv);
        let y = row_of(&rows, "❯ arregla");
        let mark = g.cells[y * g.cols + 4].1;
        assert_eq!(mark.fg, Some(look::magenta()), "the conversation's colour");
        assert_eq!(g.cells[y * g.cols + 30].1.bg, Some(look::sel()), "the band");
        let mut other = idle();
        other.accent = Accent::Cyan;
        let (g, _) = frame(&Ui::default(), &other);
        assert_eq!(g.cells[y * g.cols + 4].1.fg, Some(look::cyan()));
    }

    #[test]
    fn one_tool_is_not_herramientas_in_the_plural() {
        let mut conv = idle();
        conv.items = vec![
            serde_json::from_value(
                serde_json::json!({"t": "result", "ms": 3000, "tools": 1, "cost": 80000}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"t": "result", "ms": 1000, "tools": 0, "cost": 0}),
            )
            .unwrap(),
        ];
        conv.first = 0;
        let (_, rows) = frame(&Ui::default(), &conv);
        row_of(&rows, "Listo en 3 s · 1 herramienta · $0,08");
        row_of(&rows, "Listo en 1 s · 0 herramientas");
        assert!(!rows.iter().any(|r| r.contains("1 herramientas")));
    }

    #[test]
    fn a_tool_is_yellow_while_it_runs_green_when_it_worked_and_red_when_denied() {
        let mut conv = idle();
        conv.items = ["run", "ok", "denied", "failed"]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                serde_json::from_value(serde_json::json!({
                    "t": "tool", "id": format!("t{i}"), "name": "Bash", "arg": s, "state": s
                }))
                .unwrap()
            })
            .collect();
        conv.first = 0;
        let (g, rows) = frame(&unfolded(&conv), &conv);
        let dot = |arg: &str| {
            let y = row_of(&rows, &format!("Bash({arg})"));
            g.cells[y * g.cols + 4].1.fg
        };
        assert_eq!(dot("run"), Some(look::yellow()));
        assert_eq!(dot("ok"), Some(look::green()));
        assert_eq!(dot("denied"), Some(look::red()));
        assert_eq!(dot("failed"), Some(look::red()));
    }

    #[test]
    fn an_edit_shows_its_lines_tinted_from_the_theme() {
        let conv = idle();
        let (g, rows) = frame(&Ui::default(), &conv);
        let add = row_of(&rows, "claims.exp <= now");
        let del = row_of(&rows, "claims.exp < now");
        assert_eq!(g.cells[add * g.cols + 40].1.bg, Some(look::add_bg()));
        assert_eq!(g.cells[del * g.cols + 40].1.bg, Some(look::del_bg()));
    }

    #[test]
    fn the_footer_has_the_mode_at_the_left_and_the_model_at_the_right_and_nothing_else() {
        let mut conv = idle();
        let (g, rows) = frame(&Ui::default(), &conv);
        let foot = &rows[INNER.1 + INNER.3 - 1];
        assert!(foot.trim_start().starts_with("manual"), "{foot:?}");
        assert!(foot.trim_end().ends_with("sonnet-5.5"), "{foot:?}");
        let y = INNER.1 + INNER.3 - 1;
        assert_eq!(g.cells[y * g.cols + 3].1.fg, Some(look::dim()));
        assert!(foot.contains("⚙ 1"), "one task runs: {foot:?}");
        for (mode, text, colour) in [
            (Mode::AcceptEdits, "accept edits", look::magenta()),
            (Mode::Plan, "plan", look::cyan()),
            (Mode::Auto, "auto", look::yellow()),
        ] {
            conv.mode = mode;
            let (g, rows) = frame(&Ui::default(), &conv);
            assert!(rows[y].contains(text), "{mode:?}");
            assert_eq!(g.cells[y * g.cols + 3].1.fg, Some(colour), "{mode:?}");
        }
    }

    /// REQ-570: a consultation of the advisor is drawn as Claude Code draws it: `● Advising`, and
    /// under it what the advisor did.
    #[test]
    fn a_consultation_of_the_advisor_is_drawn_with_its_name_and_its_answer() {
        let mut conv = idle();
        conv.items = vec![Item::Tool {
            id: "srv1".into(),
            name: "Advising".into(),
            arg: String::new(),
            state: ToolState::Ok,
            summary: Some("✔ El asesor revisó la conversación y aplicará sus comentarios".into()),
            output: Vec::new(),
            diff: Vec::new(),
            url: None,
            by: None,
        }];
        let (_, rows) = frame(&Ui::default(), &conv);
        let joined = rows.join("\n");
        assert!(joined.contains("● Advising"), "{joined}");
        assert!(!joined.contains("Advising("), "{joined}");
        if let Item::Tool { arg, .. } = &mut conv.items[0] {
            *arg = "using Opus 5.5".into();
        }
        let (_, rows) = frame(&Ui::default(), &conv);
        assert!(
            rows.join("\n").contains("● Advising using Opus 5.5"),
            "{rows:?}"
        );
        assert!(!rows.join("\n").contains("Advising(using"), "{rows:?}");
        assert!(
            joined.contains("⎿  ✔ El asesor revisó la conversación"),
            "{joined}"
        );
    }

    #[test]
    fn a_tool_that_runs_is_drawn_open_with_its_time_and_the_working_line_says_the_tokens() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-conversation-running.json")).unwrap(),
        )
        .unwrap();
        let conv: Conversation = serde_json::from_value(doc["node"].clone()).unwrap();
        let (_, rows) = frame(&Ui::default(), &conv);
        row_of(&rows, "● Bash(cargo test) · 8 s");
        row_of(&rows, "⎿  ejecutando…");
        row_of(
            &rows,
            "Trabajando… (12 s · ↓ 1,3 k tokens · esc interrumpe)",
        );
        let mut done = conv.clone();
        done.running.clear();
        if let Item::Tool { state, summary, .. } = &mut done.items[1] {
            *state = ToolState::Ok;
            *summary = Some("Listo".into());
        }
        let (_, rows) = frame(&Ui::default(), &done);
        assert!(!rows.join("\n").contains("ejecutando"), "{rows:?}");
    }

    #[test]
    fn a_session_that_is_followed_shows_the_turn_of_the_other_claude_with_no_way_to_stop_it() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-conversation-following.json")).unwrap(),
        )
        .unwrap();
        let conv: Conversation = serde_json::from_value(doc["node"].clone()).unwrap();
        assert_eq!(conv.status, ConvStatus::Dead);
        let (_, rows) = frame(&Ui::default(), &conv);
        row_of(&rows, "Trabajando… (12 s · ↓ 1,3 k tokens)");
        row_of(&rows, "● Bash(cargo test) · 9 s");
        let joined = rows.join("\n");
        assert!(
            !joined.contains("esc interrumpe") && !joined.contains("■ esc"),
            "{joined}"
        );
        let mut done = conv.clone();
        done.following = None;
        done.running.clear();
        let (_, rows) = frame(&Ui::default(), &done);
        assert!(!rows.join("\n").contains("Trabajando…"));
    }

    #[test]
    fn the_working_line_says_what_claude_does_for_how_long_and_how_to_stop() {
        let mut conv = idle();
        conv.items = (0..60)
            .map(|n| Item::Text {
                text: format!("hola {n}"),
                live: false,
                blocks: None,
            })
            .collect();
        conv.first = 0;
        conv.queue.clear();
        conv.status = ConvStatus::Thinking;
        conv.since = Some(1_792_110_000_000);
        let (_, rows) = frame(&Ui::default(), &conv);
        let work = row_of(&rows, "Pensando… (12 s · esc interrumpe)");
        row_of(&rows, "■ esc");
        assert!(
            rows[work - 1].trim().is_empty(),
            "a blank row between the list and the line of work: {rows:#?}"
        );
        assert!(rows[work - 2].contains("hola 59"), "{rows:#?}");
        conv.status = ConvStatus::Idle;
        let (_, rows) = frame(&Ui::default(), &conv);
        assert!(!rows.iter().any(|r| r.contains("esc interrumpe")));
    }

    #[test]
    fn a_dead_engine_says_it_resumes_only_once_a_message_is_sent() {
        let mut conv = idle();
        conv.status = ConvStatus::Dead;
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        assert!(!rows.iter().any(|r| r.contains("retomando")));
        type_in(&mut ui, &conv, "sigue");
        assert!(matches!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send { .. })
        ));
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "retomando conversación…");
        conv.status = ConvStatus::Idle;
        ui.sync(&conv);
        let (_, rows) = frame(&ui, &conv);
        assert!(!rows.iter().any(|r| r.contains("retomando")));
    }

    #[test]
    fn the_field_has_the_chips_the_line_and_what_waits() {
        let mut conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "hola");
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "…/proj");
        row_of(&rows, "en cola 1");
        // The message that waits is in the list as one not sent yet (REQ-549).
        row_of(&rows, "⧖ y luego corre los tests");
        row_of(&rows, "❯ hola");
        assert!(rows.iter().any(|r| r.contains('➤')));
        conv.queue.clear();
        let (_, rows) = frame(&Ui::default(), &conv);
        row_of(&rows, "escribe un mensaje");
    }

    #[test]
    fn enter_sends_the_line_and_a_line_of_many_rows_goes_as_one_message() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "hola");
        ui.key(
            &conv,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        type_in(&mut ui, &conv, "adiós");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send {
                node: conv.head.id.clone(),
                text: "hola\nadiós".into(),
                images: vec![]
            })
        );
        assert_eq!(ui.editor.text(), "");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            None,
            "nothing to send"
        );
        ui.paste("uno\r\ndos\ntres");
        assert_eq!(
            ui.editor.text(),
            "uno\ndos\ntres",
            "a paste is not three Enters"
        );
    }

    #[test]
    fn esc_interrupts_only_what_runs_and_shift_tab_goes_round_the_modes() {
        let mut conv = idle();
        let mut ui = Ui::default();
        assert_eq!(press(&mut ui, &conv, KeyCode::Esc), None);
        conv.status = ConvStatus::Streaming;
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Esc),
            Some(Act::Interrupt {
                node: conv.head.id.clone()
            })
        );
        let mut seen = vec![];
        for mode in [Mode::Manual, Mode::AcceptEdits, Mode::Plan, Mode::Auto] {
            conv.mode = mode;
            match press(&mut ui, &conv, KeyCode::BackTab) {
                Some(Act::Mode { mode, .. }) => seen.push(mode),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            seen,
            [Mode::AcceptEdits, Mode::Plan, Mode::Auto, Mode::Manual]
        );
    }

    #[test]
    fn a_bare_command_opens_its_list_and_one_with_a_value_goes_to_the_daemon() {
        let conv = idle();
        let node = conv.head.id.clone();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/model");
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert!(ui.has_pop());
        let (_, rows) = frame(&ui, &conv);
        for m in ["modelo", "fable", "opus", "sonnet", "haiku"] {
            row_of(&rows, m);
        }
        // the one in force is lit, and ↓ ↵ chooses the next
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Model {
                node: node.clone(),
                model: "haiku".into()
            })
        );
        assert!(!ui.has_pop());
        type_in(&mut ui, &conv, "/effort");
        press(&mut ui, &conv, KeyCode::Enter);
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Effort {
                node: node.clone(),
                effort: "xhigh".into()
            })
        );
        type_in(&mut ui, &conv, "/mode");
        press(&mut ui, &conv, KeyCode::Enter);
        press(&mut ui, &conv, KeyCode::Down);
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Mode {
                node: node.clone(),
                mode: Mode::Plan
            })
        );
        type_in(&mut ui, &conv, "/color");
        press(&mut ui, &conv, KeyCode::Enter);
        let (_, rows) = frame(&ui, &conv);
        for c in ["azul", "verde", "amarillo", "rojo", "magenta", "cian"] {
            row_of(&rows, c);
        }
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Color {
                node: node.clone(),
                accent: Accent::Cyan
            })
        );
        type_in(&mut ui, &conv, "/effort low");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send {
                node,
                text: "/effort low".into(),
                images: vec![]
            })
        );
        type_in(&mut ui, &conv, "/model");
        press(&mut ui, &conv, KeyCode::Enter);
        assert_eq!(press(&mut ui, &conv, KeyCode::Esc), None);
        assert!(!ui.has_pop(), "esc lets the list go");
    }

    #[test]
    fn a_name_is_asked_for_in_a_field_and_the_title_opens_it() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/rename");
        press(&mut ui, &conv, KeyCode::Enter);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "nombre");
        row_of(&rows, "arregla el test que falla e…");
        for _ in 0..conv.name.chars().count() {
            press(&mut ui, &conv, KeyCode::Backspace);
        }
        type_in(&mut ui, &conv, "auth");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Name {
                node: conv.head.id.clone(),
                name: "auth".into()
            })
        );
        let mut ui = Ui::default();
        assert!(ui.click_title(&conv, (1, 0, 100, 60), 5, 0));
        assert!(ui.has_pop());
        assert!(!Ui::default().click_title(&conv, (1, 0, 100, 60), 5, 3));
    }

    #[test]
    fn a_line_that_starts_with_a_bang_is_a_shell_line() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "!git status");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Shell {
                node: conv.head.id.clone(),
                command: "git status".into()
            })
        );
    }

    #[test]
    fn while_a_permission_waits_only_its_keys_do_anything() {
        let conv = fixture();
        let node = conv.head.id.clone();
        let answer = |key: &str| {
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-1".into(),
                key: key.into(),
                text: None,
            })
        };
        let mut ui = Ui::default();
        let (g, rows) = frame(&ui, &conv);
        row_of(&rows, "permiso · Edit");
        row_of(&rows, "Editar src/auth.rs");
        row_of(&rows, "1 Sí");
        row_of(&rows, "2 Sí, y no volver a preguntar por ediciones");
        row_of(&rows, "3 No, y decirle a Claude qué hacer distinto");
        row_of(&rows, "1 2 3 · y a n · ↑↓ ↵");
        let add = row_of(&rows, "+ ");
        assert_eq!(
            g.cells[add * g.cols + 20].1.bg,
            Some(look::add_bg()),
            "the diff is tinted in the box too"
        );
        assert!(
            !rows.iter().any(|r| r.contains("escribe un mensaje")),
            "the box is where the field was"
        );
        for c in "hol x".chars() {
            assert_eq!(press(&mut ui, &conv, KeyCode::Char(c)), None);
        }
        assert_eq!(ui.editor.text(), "", "no other key reaches the field");
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('y')), answer("yes"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('a')), answer("session"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('n')), answer("no"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('1')), answer("yes"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('2')), answer("session"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('3')), answer("no"));
        assert_eq!(press(&mut ui, &conv, KeyCode::Char('4')), None);
        press(&mut ui, &conv, KeyCode::Down);
        press(&mut ui, &conv, KeyCode::Down);
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            answer("no"),
            "↓ stops at the last"
        );
        press(&mut ui, &conv, KeyCode::Up);
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), answer("session"));
        // a click on a row answers with it
        let y = row_of(&rows, "2 Sí, y no volver");
        assert_eq!(ui.click(&conv, INNER, 20, y), answer("session"));
    }

    #[test]
    fn in_a_short_pane_the_box_that_asks_keeps_its_options_inside_and_a_click_finds_them() {
        let mut conv = fixture();
        if let Some(ask) = conv.ask.as_mut() {
            ask.diff = (0..30)
                .map(|n| DiffLine {
                    n: Some(n),
                    sign: DiffSign::Add,
                    text: format!("línea {n}"),
                })
                .collect();
        }
        let node = conv.head.id.clone();
        let inner = (2, 1, 96, 14);
        let mut ui = Ui::default();
        let mut g = Grid::new(100, 16);
        ui.draw(&mut g, &conv, inner, &[], true, 0);
        let rows = g.rows_plain();
        let foot = inner.1 + inner.3 - 1;
        let third = row_of(&rows, "3 No, y decirle");
        assert!(
            third < foot,
            "the options end above the footer:\n{}",
            rows.join("\n")
        );
        assert!(
            rows[third + 1].contains("1 2 3 · y a n"),
            "the hint is under them"
        );
        for (n, key) in [
            ("1 Sí", "yes"),
            ("2 Sí, y no volver", "session"),
            ("3 No", "no"),
        ] {
            let y = row_of(&rows, n);
            assert_eq!(
                ui.click(&conv, inner, 20, y),
                Some(Act::Answer {
                    node: node.clone(),
                    ask: "req-1".into(),
                    key: key.into(),
                    text: None,
                }),
                "{n}"
            );
        }
    }

    #[test]
    fn a_click_picks_an_item_and_c_copies_it_t_the_turn_and_esc_lets_it_go() {
        let conv = idle();
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "Voy a mirar el test");
        assert_eq!(ui.click(&conv, INNER, 20, y), None);
        let (g, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "Voy a mirar el test");
        assert_eq!(g.cells[y * g.cols + 2].0, '▌', "a bar lights it");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Char('c')),
            Some(Act::Copy(
                "Voy a mirar el test y el código que prueba.".into()
            ))
        );
        assert_eq!(
            ui.take_says().as_deref(),
            Some("copiado · 1 línea · 43 caracteres")
        );
        match press(&mut ui, &conv, KeyCode::Char('t')) {
            Some(Act::Copy(t)) => {
                assert!(
                    t.starts_with("arregla el test que falla en auth::expired"),
                    "{t}"
                );
                assert!(
                    t.contains("Edit(src/auth.rs)") && t.contains("git status"),
                    "{t}"
                );
                assert!(
                    !t.contains("el daemon se reinició"),
                    "a divider is not the turn"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(ui.take_says().unwrap().starts_with("copiado · "));
        assert_eq!(press(&mut ui, &conv, KeyCode::Esc), None);
        let (g, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "Voy a mirar el test");
        assert_ne!(g.cells[y * g.cols + 2].0, '▌');
        // c is a letter again
        type_in(&mut ui, &conv, "c");
        assert_eq!(ui.editor.text(), "c");
    }

    /// The cell of `text` in the rows of a frame: its column and its row.
    fn at(rows: &[String], text: &str) -> (usize, usize) {
        let y = row_of(rows, text);
        let col = rows[y].find(text).map(|b| rows[y][..b].chars().count());
        (col.unwrap(), y)
    }

    fn lit(g: &Grid, x: usize, y: usize) -> bool {
        g.cells[y * g.cols + x].1.bg == Some(look::selection().0)
    }

    #[test]
    fn a_drag_over_the_list_copies_what_it_covers_without_the_marks_and_lights_it() {
        let conv = idle();
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let (x, y) = at(&rows, "mirar el test");
        assert!(ui.press(&conv, INNER, x, y));
        ui.drag_to(&conv, INNER, (x + 12, y), 0);
        let (g, _) = frame(&ui, &conv);
        assert!(lit(&g, x, y) && lit(&g, x + 12, y), "what it covers is lit");
        assert!(!lit(&g, x - 1, y) && !lit(&g, x + 13, y), "and no more");
        assert_eq!(
            ui.release(&conv, INNER),
            Released::Copy("mirar el test".into())
        );
        assert_eq!(
            ui.take_says().as_deref(),
            Some("copiado · 1 línea · 13 caracteres")
        );
        let (g, _) = frame(&ui, &conv);
        assert!(lit(&g, x, y), "it stays lit after the button is let go");
        // a drag that begins in the mark of a row does not copy the mark
        let (_, y) = at(&rows, "Voy a mirar el test");
        assert!(ui.press(&conv, INNER, INNER.0, y));
        ui.drag_to(&conv, INNER, (INNER.0 + 12, y), 0);
        assert_eq!(ui.release(&conv, INNER), Released::Copy("Voy a mir".into()));
        // a key lets go of it
        ui.unlight();
        let (g, _) = frame(&ui, &conv);
        assert!(!lit(&g, x, y));
    }

    #[test]
    fn a_press_that_never_moves_is_the_click_it_was_and_a_list_over_the_messages_takes_it_whole() {
        let conv = idle();
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let (x, y) = at(&rows, "mirar el test");
        assert!(ui.press(&conv, INNER, x, y));
        assert_eq!(ui.release(&conv, INNER), Released::Click(x, y));
        assert!(ui.sel.is_none());
        // a drag that returns to where it began copies nothing
        assert!(ui.press(&conv, INNER, x, y));
        ui.drag_to(&conv, INNER, (x + 3, y), 0);
        ui.drag_to(&conv, INNER, (x, y), 0);
        assert!(matches!(ui.release(&conv, INNER), Released::Copy(t) if t == "m"));
        // the footer and the field are not the list
        assert!(!ui.press(&conv, INNER, x, INNER.1 + INNER.3 - 1));
        // nor is anything while a list is open
        type_in(&mut ui, &conv, "/");
        ui.open_pick(&conv, Of::Model);
        assert!(!ui.press(&conv, INNER, x, y));
    }

    #[test]
    fn rows_the_width_broke_are_copied_as_one_line_and_the_blank_between_items_stays() {
        let mut conv = idle();
        let long = "palabra ".repeat(30);
        conv.items.push(Item::Text {
            text: long.trim().to_owned(),
            live: false,
            blocks: None,
        });
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let (x, y) = at(&rows, "palabra palabra");
        assert!(ui.press(&conv, INNER, x, y));
        ui.drag_to(&conv, INNER, (INNER.0 + INNER.2, y + 2), 0);
        assert_eq!(ui.release(&conv, INNER), Released::Copy(long.trim().into()));
        // from the item before it, the blank line between them is kept
        let (x, y) = at(&rows, "Voy a mirar el test");
        let (_, last) = at(&rows, "palabra palabra");
        assert!(last > y);
        assert!(ui.press(&conv, INNER, x, y));
        ui.drag_to(&conv, INNER, (INNER.0 + INNER.2, last), 0);
        match ui.release(&conv, INNER) {
            Released::Copy(t) => {
                assert!(t.starts_with("Voy a mirar el test"), "{t}");
                assert!(!t.contains('●') && !t.contains("  palabra"), "{t}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// REQ-567: when older items are added before the others, what is held by the number of an
    /// item moves with the items, and a selection is let go.
    #[test]
    fn what_is_held_by_the_number_of_an_item_moves_when_older_items_come_before() {
        let mut ui = Ui {
            picked: Some(4),
            ..Ui::default()
        };
        ui.open.insert(5);
        ui.unfolded.insert(7);
        ui.shifted(100);
        assert_eq!(ui.picked, Some(104));
        assert_eq!(ui.open.iter().copied().collect::<Vec<_>>(), [105]);
        assert_eq!(ui.unfolded.iter().copied().collect::<Vec<_>>(), [107]);
    }

    /// REQ-573: what Claude proposes is grey in the empty field and comes back whenever the field is
    /// empty again, Tab or → puts it in the field to be edited, Enter with the field empty sends
    /// nothing, and only Esc puts it aside.
    #[test]
    fn what_claude_proposes_is_taken_with_tab_put_aside_by_esc_and_back_when_the_field_is_empty() {
        let mut conv = idle();
        conv.suggestion = Some("revisa el diff antes de subirlo".into());
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "revisa el diff antes de subirlo  tab");
        assert!(
            press(&mut ui, &conv, KeyCode::Enter).is_none(),
            "Enter with an empty field sends nothing"
        );
        assert!(press(&mut ui, &conv, KeyCode::Tab).is_none());
        assert_eq!(ui.editor.text(), "revisa el diff antes de subirlo");
        let (_, rows) = frame(&ui, &conv);
        assert!(
            !rows.join("\n").contains("  tab"),
            "it is the person's text now"
        );
        assert!(matches!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send { .. })
        ));
        // Esc puts it aside; the same text does not come back, another one does
        let mut ui = Ui::default();
        press(&mut ui, &conv, KeyCode::Esc);
        assert!(
            !frame(&ui, &conv)
                .1
                .join("\n")
                .contains("revisa el diff antes de subirlo")
        );
        conv.suggestion = Some("otra cosa".into());
        row_of(&frame(&ui, &conv).1, "otra cosa  tab");
        // writing hides it, and it comes back when the field is empty again
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "x");
        assert!(!frame(&ui, &conv).1.join("\n").contains("otra cosa"));
        press(&mut ui, &conv, KeyCode::Backspace);
        row_of(&frame(&ui, &conv).1, "otra cosa  tab");
        // so does taking it and deleting it, or pasting and deleting
        press(&mut ui, &conv, KeyCode::Tab);
        assert_eq!(ui.editor.text(), "otra cosa");
        ui.editor.take();
        row_of(&frame(&ui, &conv).1, "otra cosa  tab");
        ui.paste("y");
        ui.editor.take();
        row_of(&frame(&ui, &conv).1, "otra cosa  tab");
        let mut ui = Ui::default();
        press(&mut ui, &conv, KeyCode::Right);
        assert_eq!(ui.editor.text(), "otra cosa");
        // nothing is proposed while Claude works or asks
        let mut busy = idle();
        busy.suggestion = Some("no".into());
        busy.status = ConvStatus::Thinking;
        assert!(
            !frame(&Ui::default(), &busy)
                .1
                .join("\n")
                .contains("no  tab")
        );
    }

    /// REQ-576: a session that another `claude` has open is read and not written: the field says
    /// who has it, what is typed, pasted or sent does nothing and says why, and what is read,
    /// scrolled and copied goes on.
    #[test]
    fn a_session_that_another_claude_has_open_is_read_and_not_written_in_the_terminal() {
        let mut conv = idle();
        conv.suggestion = Some("revisa el diff antes de subirlo".into());
        conv.held = Some(Held {
            pid: 93269,
            cwd: "/work/jw-wt/next-bootstrap".into(),
            busy: false,
        });
        let mut ui = Ui::default();
        ui.sync(&conv);
        let (_, rows) = frame(&ui, &conv);
        let joined = rows.join("\n");
        assert!(
            joined.contains("solo lectura · otro claude (pid 93269) la tiene abierta · ciérralo"),
            "{joined}"
        );
        assert!(
            !joined.contains("revisa el diff"),
            "no suggestion while it is held"
        );
        type_in(&mut ui, &conv, "hola");
        assert_eq!(ui.editor.text(), "", "nothing is written");
        assert!(
            ui.take_says()
                .is_some_and(|s| s.contains("pid 93269, en jw-wt/next-bootstrap"))
        );
        ui.paste("pegado");
        assert_eq!(ui.editor.text(), "");
        ui.key(
            &conv,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        assert_eq!(ui.editor.text(), "", "ctrl-j writes nothing either");
        assert!(press(&mut ui, &conv, KeyCode::Enter).is_none());
        // it is read: the list scrolls
        press(&mut ui, &conv, KeyCode::PageUp);
        // and when the other ends the field is the person's again
        conv.held = None;
        ui.sync(&conv);
        type_in(&mut ui, &conv, "hola");
        assert_eq!(ui.editor.text(), "hola");
        let mut busy = conv.clone();
        busy.held = Some(Held {
            pid: 1,
            cwd: "/a".into(),
            busy: true,
        });
        assert!(
            frame(&Ui::default(), &busy)
                .1
                .join("\n")
                .contains("trabaja en ella")
        );
    }

    #[test]
    fn a_text_with_blocks_is_drawn_as_blocks_and_its_marks_are_only_how_it_is_set() {
        use contract::markdown::{Inline, MdBlock};
        let t = |s: &str| {
            vec![Inline {
                t: s.into(),
                b: false,
                i: false,
                code: false,
                href: None,
            }]
        };
        let mut conv = idle();
        conv.items = vec![Item::Text {
            text: "x".into(),
            live: false,
            blocks: Some(vec![
                MdBlock::Para {
                    text: t("Ver el caso:"),
                },
                MdBlock::Quote {
                    blocks: vec![MdBlock::Para { text: t("citado") }],
                },
                MdBlock::Table {
                    head: vec![t("caso"), t("fin")],
                    rows: vec![vec![t("29"), t("vale")]],
                },
            ]),
        }];
        let ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let joined = rows.join("\n");
        assert!(joined.contains("● Ver el caso:"), "{joined}");
        assert!(joined.contains("▎ citado") && joined.contains("┌") && joined.contains("│ 29"));
        let lines = ui.lines(&conv, INNER.2);
        let quote = lines
            .iter()
            .find(|l| l.spans.iter().any(|(t, _)| t == "citado"))
            .expect("the quote row");
        assert_eq!(quote.lead, 4, "the mark and the bar are only how it is set");
    }

    #[test]
    fn a_drag_held_past_the_edge_scrolls_the_list_a_line_at_a_time() {
        let conv = idle();
        let mut ui = Ui::default();
        let small = (INNER.0, INNER.1, INNER.2, 14);
        let mut g = Grid::new(100, 60);
        ui.draw(&mut g, &conv, small, &[], true, 0);
        ui.scroll_by(true, 3);
        let mut g = Grid::new(100, 60);
        ui.draw(&mut g, &conv, small, &[], true, 0);
        let mid = small.1 + 3;
        assert!(ui.press(&conv, small, INNER.0 + 8, mid));
        ui.drag_to(&conv, small, (INNER.0 + 8, small.1 + small.3), 0);
        assert_eq!(ui.scroll, 3, "nothing moves before its time");
        ui.drag_tick(&conv, small, select::EDGE_MS);
        assert_eq!(ui.scroll, 2);
        ui.drag_tick(&conv, small, select::EDGE_MS + 1);
        assert_eq!(ui.scroll, 2, "a line each select::EDGE_MS");
        ui.drag_tick(&conv, small, 2 * select::EDGE_MS);
        assert_eq!(ui.scroll, 1);
        // back inside, it stops
        ui.drag_to(&conv, small, (INNER.0 + 8, mid), 3 * select::EDGE_MS);
        ui.drag_tick(&conv, small, 9 * select::EDGE_MS);
        assert_eq!(ui.scroll, 1);
    }

    #[test]
    fn a_click_on_a_tool_opens_its_output_and_ctrl_o_does_too() {
        let conv = idle();
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        // The tool is in a run that is folded: the row of the run opens it first (REQ-558).
        let fold_row = row_of(&rows, "▸ Leyó 1 archivo");
        ui.click(&conv, INNER, 10, fold_row);
        let (_, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "● Read(src/auth.rs)");
        ui.click(&conv, INNER, 10, y);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "pub fn verify(token: &str) -> bool {");
        ui.click(&conv, INNER, 10, y);
        let (_, rows) = frame(&ui, &conv);
        assert!(
            !rows.iter().any(|r| r.contains("pub fn verify")),
            "again closes it"
        );
        // the agent's steps
        let y = row_of(&rows, "● Agent(Explore)");
        ui.click(&conv, INNER, 10, y);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "· Grep verify");
        row_of(&rows, "· Read src/session.rs");
        ui.click(&conv, INNER, 10, y);
        // ^O opens the last tool, and closes it
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        let mut lots = idle();
        if let Some(Item::Shell { output, .. }) = lots
            .items
            .iter_mut()
            .find(|i| matches!(i, Item::Shell { .. }))
        {
            *output = (0..30).map(|n| format!("línea {n}")).collect();
        }
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &lots);
        assert!(!rows.iter().any(|r| r.contains("línea 20")));
        row_of(&rows, "líneas más");
        ui.key(&lots, &ctrl_o);
        let (_, rows) = frame(&ui, &lots);
        row_of(&rows, "línea 29");
        ui.key(&lots, &ctrl_o);
        let (_, rows) = frame(&ui, &lots);
        assert!(!rows.iter().any(|r| r.contains("línea 29")));
    }

    #[test]
    fn the_list_scrolls_with_the_page_keys_and_keeps_the_newest_at_the_bottom() {
        let conv = idle();
        let mut ui = Ui::default();
        let mut g = Grid::new(100, 22);
        let draw = |ui: &Ui, g: &mut Grid| {
            *g = Grid::new(100, 22);
            ui.draw(g, &conv, (2, 1, 96, 20), &[], true, 0);
            g.rows_plain()
        };
        let rows = draw(&ui, &mut g);
        assert!(
            rows.iter().any(|r| r.contains("el daemon se reinició")),
            "the newest is shown"
        );
        assert!(!rows.iter().any(|r| r.contains("arregla el test")));
        for _ in 0..8 {
            press(&mut ui, &conv, KeyCode::PageUp);
        }
        let rows = draw(&ui, &mut g);
        assert!(
            rows.iter().any(|r| r.contains("arregla el test")),
            "the oldest is reached"
        );
        ui.scroll_by(true, 1000);
        assert!(
            draw(&ui, &mut g)
                .iter()
                .any(|r| r.contains("arregla el test")),
            "it does not go past it"
        );
        for _ in 0..8 {
            press(&mut ui, &conv, KeyCode::PageDown);
        }
        assert!(
            draw(&ui, &mut g)
                .iter()
                .any(|r| r.contains("el daemon se reinició"))
        );
        // sending goes back to the newest
        ui.scroll_by(true, 5);
        type_in(&mut ui, &conv, "x");
        press(&mut ui, &conv, KeyCode::Enter);
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn a_picture_goes_with_the_message_only_while_its_mark_is_in_the_line() {
        let conv = idle();
        let image = |d: &str| Image {
            media_type: "image/jpeg".into(),
            data: d.into(),
        };
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "mira ");
        ui.attach(image("AAAA"));
        type_in(&mut ui, &conv, " y ");
        ui.attach(image("BBBB"));
        assert_eq!(ui.editor.text(), "mira [Imagen#1] y [Imagen#2]");
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "▣ 2");
        // the first mark is taken out
        for _ in 0..13 {
            press(&mut ui, &conv, KeyCode::Left);
        }
        ui.key(
            &conv,
            &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        match press(&mut ui, &conv, KeyCode::Enter) {
            Some(Act::Send { images, text, .. }) => {
                assert_eq!(images, vec![image("BBBB")], "{text}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_line_is_edited_with_the_cursor() {
        let mut e = Editor::default();
        e.insert("uno dos\ntres");
        assert_eq!(e.cursor(), 12);
        assert!(e.up());
        assert_eq!(e.cursor(), 4, "the column is kept, as far as the line goes");
        assert!(!e.up(), "the first line has nothing above");
        assert!(e.down());
        assert!(!e.down());
        e.edit(&KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        e.insert(">");
        e.edit(&KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        e.insert("<");
        assert_eq!(e.text(), "uno dos\n>tres<");
        e.word_back();
        assert_eq!(e.text(), "uno dos\n", "^W takes the word before the cursor");
        let mut e = Editor::default();
        e.set("abc def");
        e.kill_to_start();
        assert_eq!(e.text(), "");
        e.set("abc def");
        e.cursor = 3;
        e.kill_to_end();
        assert_eq!(e.text(), "abc");
        assert_eq!(e.take(), "abc");
        assert_eq!(e.cursor(), 0);
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// The keys of a text field that the terminal sends for option and shift: a word at a time,
    /// selected with shift, and gone with option-backspace.
    #[test]
    fn option_arrows_jump_a_word_with_shift_they_select_and_option_backspace_takes_it() {
        let (alt, both) = (KeyModifiers::ALT, KeyModifiers::ALT | KeyModifiers::SHIFT);
        let mut e = Editor::default();
        e.set("uno dos tres");
        e.edit(&key(KeyCode::Left, alt));
        assert_eq!(e.cursor(), 8, "to the start of the word before");
        e.edit(&key(KeyCode::Left, alt));
        assert_eq!(e.cursor(), 4);
        e.edit(&key(KeyCode::Right, alt));
        assert_eq!(e.cursor(), 7, "to the end of the word after");
        e.edit(&key(KeyCode::Char('b'), alt));
        assert_eq!(e.cursor(), 4, "esc b is the same key");
        e.edit(&key(KeyCode::Char('f'), alt));
        assert_eq!(e.cursor(), 7, "esc f too");
        assert_eq!(e.selection(), None);

        e.edit(&key(KeyCode::Right, both));
        assert_eq!(e.selection(), Some((7, 12)), "a word selected");
        e.edit(&key(KeyCode::Left, both));
        e.edit(&key(KeyCode::Left, both));
        assert_eq!(
            e.selection(),
            Some((4, 7)),
            "and taken back, and the one before it"
        );
        e.edit(&key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(
            (e.cursor(), e.selection()),
            (4, None),
            "a plain arrow goes to the edge"
        );

        e.edit(&key(KeyCode::Right, both));
        e.edit(&key(KeyCode::Char('X'), KeyModifiers::SHIFT));
        assert_eq!(e.text(), "uno X tres", "writing replaces the selection");
        e.edit(&key(KeyCode::Left, both));
        e.edit(&key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.text(), "uno  tres", "a selection goes with backspace");

        let mut e = Editor::default();
        e.set("uno dos tres");
        e.edit(&key(KeyCode::Backspace, alt));
        assert_eq!(
            e.text(),
            "uno dos ",
            "option-backspace takes the word before"
        );
        e.edit(&key(KeyCode::Left, both));
        e.edit(&key(KeyCode::Backspace, alt));
        assert_eq!(e.text(), "uno ", "a selection is what goes first");
    }

    #[test]
    fn text_wraps_by_words_keeps_code_apart_and_hangs_a_list() {
        let plain = Style::fg(look::fg());
        let code = Style::fg(look::cyan());
        let rows = |t: &str, w: usize| -> Vec<String> {
            wrap(t, w, plain, code)
                .into_iter()
                .map(|(_, r)| r.into_iter().map(|(s, _)| s).collect())
                .collect()
        };
        assert_eq!(
            rows("uno dos tres cuatro", 9),
            ["uno dos", "tres", "cuatro"]
        );
        assert_eq!(
            rows("abcdefghij", 4),
            ["abcd", "efgh", "ij"],
            "a word longer than a row is cut"
        );
        assert_eq!(rows("a\n\nb", 10), ["a", "", "b"], "a blank line stays");
        assert_eq!(
            rows("- uno dos tres cuatro", 12),
            ["- uno dos", "  tres", "  cuatro"]
        );
        let styled = wrap("usa `cargo test` ya", 40, plain, code);
        assert_eq!(
            styled[0].1,
            [
                ("usa ".to_owned(), plain),
                ("cargo test".to_owned(), code),
                (" ya".to_owned(), plain)
            ]
        );
        let fenced = wrap("```rust\nlet x = 1;\n```", 40, plain, code);
        assert_eq!(fenced[1].1, [("let x = 1;".to_owned(), code)]);
        let bold = wrap("**ojo** aquí", 40, plain, code);
        assert_eq!(bold[0].1[0], ("ojo".to_owned(), plain.bold()));
    }

    #[test]
    fn a_slash_lists_the_commands_that_hold_the_letters_the_ones_that_begin_with_them_first() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/");
        let all: Vec<&str> = ui.menu(&conv).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            all,
            [
                "/exit",
                "/tasks",
                "/context",
                "/doctor",
                "/deploy",
                "/jwn",
                "/vercel:deploy"
            ],
            "by where each is defined (jwn, claude, project, skill, plugin), then by name"
        );
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "comandos");
        row_of(&rows, "salir de la conversación");
        row_of(&rows, "publicar la rama en staging");
        row_of(&rows, "proyecto");
        row_of(&rows, "plugin");
        type_in(&mut ui, &conv, "de");
        let some: Vec<&str> = ui.menu(&conv).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            some,
            ["/deploy", "/vercel:deploy"],
            "begins with «de» first, then the ones that only hold it"
        );
        type_in(&mut ui, &conv, "x");
        assert!(ui.menu(&conv).is_empty(), "nothing holds it: no list");
        assert!(frame(&ui, &conv).1.iter().all(|r| !r.contains("comandos")));
        // a space ends the word, and so the list
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/model ");
        assert!(ui.menu(&conv).is_empty());
    }

    #[test]
    fn the_list_moves_with_the_arrows_completes_with_tab_runs_with_enter_and_closes_with_esc() {
        let conv = idle();
        let node = conv.head.id.clone();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/de");
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(press(&mut ui, &conv, KeyCode::Tab), None);
        assert_eq!(
            ui.editor.text(),
            "/vercel:deploy ",
            "tab completes the one lit"
        );
        assert!(ui.menu(&conv).is_empty());
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/de");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send {
                node: node.clone(),
                text: "/deploy".into(),
                images: vec![]
            }),
            "enter runs it"
        );
        assert_eq!(ui.editor.text(), "");
        // a command of jwn's that takes a value opens its list
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/exi");
        assert!(matches!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Send { text, .. }) if text == "/exit"
        ));
        // esc closes the list and the line stays; a key opens it again
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/co");
        assert_eq!(press(&mut ui, &conv, KeyCode::Esc), None);
        assert_eq!(ui.editor.text(), "/co");
        assert!(ui.menu(&conv).is_empty());
        type_in(&mut ui, &conv, "n");
        assert!(!ui.menu(&conv).is_empty());
        // ↑ at the top and ↓ at the bottom stay where they are
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/");
        press(&mut ui, &conv, KeyCode::Up);
        assert_eq!(ui.menu_at, 0);
        for _ in 0..20 {
            press(&mut ui, &conv, KeyCode::Down);
        }
        assert_eq!(ui.menu_at, 6);
        // typing again lights the first
        type_in(&mut ui, &conv, "d");
        assert_eq!(ui.menu_at, 0);
    }

    #[test]
    fn a_command_that_needs_claudes_screen_goes_to_the_daemon_which_refuses_it() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/doc");
        assert!(
            matches!(press(&mut ui, &conv, KeyCode::Enter), Some(Act::Send { text, .. }) if text == "/doctor"),
            "the daemon says where to go (REQ-423), the same for every client"
        );
        assert_eq!(ui.editor.text(), "");
    }

    #[test]
    fn a_click_on_a_row_of_the_list_runs_it_and_a_long_list_scrolls_with_the_one_lit() {
        let mut conv = idle();
        let node = conv.head.id.clone();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/de");
        let (_, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "/vercel:deploy");
        assert_eq!(
            ui.click(&conv, INNER, 20, y),
            Some(Act::Send {
                node,
                text: "/vercel:deploy".into(),
                images: vec![]
            })
        );
        conv.commands = (0..20)
            .map(|n| ConvCommand {
                name: format!("/c{n:02}"),
                source: CommandSource::Skill,
                kind: CommandKind::Prompt,
                description: None,
            })
            .collect();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/c");
        for _ in 0..12 {
            press(&mut ui, &conv, KeyCode::Down);
        }
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "/c12");
        assert!(
            !rows.iter().any(|r| r.contains("/c00")),
            "the window follows the one lit"
        );
        row_of(&rows, "13 de 20");
    }

    #[test]
    fn a_command_that_exists_is_written_in_the_colour_of_where_it_is_defined() {
        let conv = idle();
        let mut ui = Ui::default();
        let colour_of = |ui: &Ui, text: &str| {
            let (g, rows) = frame(ui, &conv);
            let y = row_of(&rows, text);
            let x = rows[y].find(text).unwrap();
            let x = rows[y][..x].chars().count();
            g.cells[y * g.cols + x].1.fg
        };
        type_in(&mut ui, &conv, "/exit");
        assert_eq!(colour_of(&ui, "/exit"), Some(look::blue()), "jwn's own");
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/context ");
        assert_eq!(
            colour_of(&ui, "/context"),
            Some(look::magenta()),
            "Claude's"
        );
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/deploy ahora");
        assert_eq!(
            colour_of(&ui, "/deploy"),
            Some(look::green()),
            "the project's"
        );
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/nada ");
        assert_eq!(
            colour_of(&ui, "/nada"),
            Some(look::fg()),
            "one that does not exist is plain"
        );
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "!ls");
        assert_eq!(colour_of(&ui, "!ls"), Some(look::green()), "a shell line");
    }

    fn saved(n: usize) -> Vec<SavedConversation> {
        (0..n)
            .map(|i| SavedConversation {
                id: format!("{i:08}-3d20-4cd2-9d13-485b39b26147"),
                name: format!("conversación número {i}"),
                at: 1_792_110_000_000 - i as u64 * 3_600_000,
            })
            .collect()
    }

    #[test]
    fn resume_asks_for_the_saved_conversations_once_and_draws_them_with_how_long_ago() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/resume");
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert!(ui.take_saved_ask(), "the daemon is asked");
        assert!(!ui.take_saved_ask(), "once");
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "retomar una conversación");
        row_of(&rows, "cargando…");
        assert_eq!(ui.listed(&saved(30), &conv), None);
        let (g, rows) = frame(&ui, &conv);
        row_of(&rows, "1 de 30");
        row_of(&rows, "conversación número 0");
        row_of(&rows, "ahora");
        row_of(&rows, "hace 2 h");
        row_of(&rows, "00000001");
        let y = row_of(&rows, "conversación número 0");
        assert_eq!(
            g.cells[y * g.cols + 20].1.bg,
            Some(look::sel()),
            "the first is lit"
        );
        // narrowing by what is typed, ↑↓ and ↵
        type_in(&mut ui, &conv, "número 2");
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "número 2");
        row_of(&rows, "1 de 11");
        assert!(!rows.iter().any(|r| r.contains("número 3")));
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Resume {
                pane: conv.pane.clone(),
                id: "00000020-3d20-4cd2-9d13-485b39b26147".into()
            }),
            "the second of those that hold «número 2»: 2, 20, 21…"
        );
        assert!(!ui.has_pop());
    }

    #[test]
    fn a_long_list_is_paged_with_the_keys_and_the_wheel_and_the_lit_row_stays_in_view() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/resume");
        press(&mut ui, &conv, KeyCode::Enter);
        ui.listed(&saved(40), &conv);
        press(&mut ui, &conv, KeyCode::PageDown);
        press(&mut ui, &conv, KeyCode::PageDown);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "21 de 40");
        row_of(&rows, "conversación número 20");
        ui.scroll_by(false, 3);
        row_of(&frame(&ui, &conv).1, "24 de 40");
        ui.scroll_by(true, 100);
        row_of(&frame(&ui, &conv).1, "1 de 40");
        for _ in 0..100 {
            press(&mut ui, &conv, KeyCode::Down);
        }
        row_of(&frame(&ui, &conv).1, "40 de 40");
        // backspace widens it again, esc closes
        type_in(&mut ui, &conv, "zz");
        row_of(&frame(&ui, &conv).1, "Nada coincide.");
        press(&mut ui, &conv, KeyCode::Backspace);
        press(&mut ui, &conv, KeyCode::Backspace);
        row_of(&frame(&ui, &conv).1, "1 de 40");
        press(&mut ui, &conv, KeyCode::Esc);
        assert!(!ui.has_pop());
    }

    #[test]
    fn a_click_on_a_row_of_the_list_resumes_it_and_a_click_outside_closes_the_list() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/resume");
        press(&mut ui, &conv, KeyCode::Enter);
        ui.listed(&saved(5), &conv);
        let (_, rows) = frame(&ui, &conv);
        let y = row_of(&rows, "conversación número 3");
        assert!(
            matches!(ui.click(&conv, INNER, 20, y), Some(Act::Resume { id, .. }) if id.starts_with("00000003"))
        );
        assert!(!ui.has_pop());
        press(&mut ui, &conv, KeyCode::Char('x'));
        type_in(&mut ui, &conv, "/resume");
        press(&mut ui, &conv, KeyCode::Enter);
        ui.listed(&saved(5), &conv);
        assert_eq!(ui.click(&conv, INNER, 20, INNER.1 + INNER.3 - 1), None);
        assert!(!ui.has_pop());
    }

    #[test]
    fn continue_takes_the_newest_at_once_and_resume_with_an_id_goes_straight_to_it() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/continue");
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert!(ui.take_saved_ask());
        assert_eq!(
            ui.listed(&saved(3), &conv),
            Some(Act::Resume {
                pane: conv.pane.clone(),
                id: "00000000-3d20-4cd2-9d13-485b39b26147".into()
            })
        );
        assert!(!ui.has_pop());
        // none saved
        type_in(&mut ui, &conv, "/continue");
        press(&mut ui, &conv, KeyCode::Enter);
        ui.take_saved_ask();
        assert_eq!(ui.listed(&[], &conv), None);
        assert!(ui.take_says().unwrap().contains("no hay conversaciones"));
        // an id
        type_in(&mut ui, &conv, "/resume abc-123");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Resume {
                pane: conv.pane.clone(),
                id: "abc-123".into()
            })
        );
        assert!(!ui.take_saved_ask());
        // the daemon could not
        type_in(&mut ui, &conv, "/resume");
        press(&mut ui, &conv, KeyCode::Enter);
        ui.list_failed("no se pudo");
        assert!(!ui.has_pop());
        assert_eq!(ui.take_says().as_deref(), Some("no se pudo"));
    }

    #[test]
    fn the_other_lists_open_from_the_node_and_what_choosing_a_row_does() {
        let conv = idle();
        let open = |word: &str| {
            let mut ui = Ui::default();
            ui.context("/work/app");
            type_in(&mut ui, &conv, word);
            assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None, "{word}");
            let (_, rows) = frame(&ui, &conv);
            (ui, rows)
        };
        let (_, rows) = open("/help");
        row_of(&rows, "ayuda");
        row_of(&rows, "enviar; en cola mientras Claude trabaja");
        let (_, rows) = open("/status");
        row_of(&rows, "estado");
        row_of(&rows, "sonnet-5.5");
        row_of(&rows, "/work/app");
        let (mut ui, rows) = open("/skills");
        row_of(&rows, "/deploy");
        row_of(&rows, "publicar la rama en staging");
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert_eq!(ui.editor.text(), "/deploy ", "a skill goes to the line");
        let (_, rows) = open("/tasks");
        row_of(&rows, "cargo build --release");
        row_of(&rows, "en curso");
        let (mut ui, rows) = open("/artifacts");
        row_of(&rows, "Informe de pruebas");
        assert_eq!(
            press(&mut ui, &conv, KeyCode::Enter),
            Some(Act::Copy(
                "https://claude.ai/artifact/6HbvGgivzfNgDnkzpDjf4Y".into()
            ))
        );
        let (_, rows) = open("/permissions");
        row_of(&rows, "permisos (solo lectura)");
        let (_, rows) = open("/memory");
        row_of(&rows, "memoria");
    }

    #[test]
    fn a_command_with_a_list_opens_from_the_slash_list_too() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "/tas");
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert!(ui.has_pop(), "the list, and nothing is sent to the engine");
    }

    /// The fixture's conversation asking a choice of several, or of one.
    fn asking(multi: bool) -> Conversation {
        let mut c = fixture();
        c.ask = Some(Ask {
            id: "req-9:0".into(),
            kind: AskKind::Question,
            tool: "AskUserQuestion".into(),
            title: "¿Qué lenguajes?".into(),
            command: None,
            diff: vec![],
            plan: vec!["Rust: sistemas".into()],
            options: ["Rust", "Kotlin", "Go"]
                .iter()
                .enumerate()
                .map(|(i, l)| contract::conversation::AskOption {
                    key: i.to_string(),
                    label: (*l).into(),
                })
                .collect(),
            multi,
        });
        c
    }

    #[test]
    fn a_choice_of_one_is_answered_by_its_number_and_one_of_several_is_marked_and_sent_with_enter()
    {
        let node = fixture().head.id.clone();
        let one = asking(false);
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &one);
        row_of(&rows, "pregunta");
        row_of(&rows, "¿Qué lenguajes?");
        row_of(&rows, "Rust: sistemas");
        row_of(&rows, "1 2 3 · ↑↓ ↵");
        assert_eq!(
            press(&mut ui, &one, KeyCode::Char('2')),
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-9:0".into(),
                key: "1".into(),
                text: None,
            })
        );
        // several: numbers mark, ↵ sends the marked ones as their numbers with commas
        let many = asking(true);
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &many);
        row_of(&rows, "[ ] Rust");
        row_of(&rows, "1 2 3 marcan · ↵ envía lo marcado");
        assert_eq!(
            press(&mut ui, &many, KeyCode::Enter),
            None,
            "nothing marked: nothing to send"
        );
        assert_eq!(press(&mut ui, &many, KeyCode::Char('3')), None);
        assert_eq!(press(&mut ui, &many, KeyCode::Char('1')), None);
        let (_, rows) = frame(&ui, &many);
        row_of(&rows, "[x] Rust");
        row_of(&rows, "[x] Go");
        row_of(&rows, "[ ] Kotlin");
        press(&mut ui, &many, KeyCode::Char('3'));
        press(&mut ui, &many, KeyCode::Char('2'));
        assert_eq!(
            press(&mut ui, &many, KeyCode::Enter),
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-9:0".into(),
                key: "0,1".into(),
                text: None,
            }),
            "3 was cleared, 2 marked: Rust and Kotlin"
        );
        // space marks the one that is lit, a click marks a row and a number that is no option does nothing
        let mut ui = Ui::default();
        press(&mut ui, &many, KeyCode::Down);
        press(&mut ui, &many, KeyCode::Char(' '));
        assert_eq!(press(&mut ui, &many, KeyCode::Char('9')), None);
        let (_, rows) = frame(&ui, &many);
        row_of(&rows, "[x] Kotlin");
        let y = row_of(&rows, "[ ] Go");
        assert_eq!(ui.click(&many, INNER, 20, y), None, "a click marks");
        assert_eq!(ui.picks, vec![1, 2]);
    }

    #[test]
    fn the_next_question_starts_with_nothing_lit_or_marked() {
        let many = asking(true);
        let mut ui = Ui::default();
        ui.sync(&many);
        press(&mut ui, &many, KeyCode::Char('1'));
        press(&mut ui, &many, KeyCode::Down);
        let mut next = asking(true);
        next.ask.as_mut().unwrap().id = "req-9:1".into();
        ui.sync(&next);
        assert_eq!((ui.picks.len(), ui.ask_at), (0, 0));
        ui.sync(&next);
        press(&mut ui, &next, KeyCode::Char('2'));
        ui.sync(&next);
        assert_eq!(
            ui.picks,
            vec![1],
            "the same question again keeps what is marked"
        );
    }

    #[test]
    fn a_plan_shows_its_first_lines_and_v_or_a_click_asks_for_all_of_it_in_the_viewer() {
        let node = fixture().head.id.clone();
        let mut c = fixture();
        c.ask = Some(Ask {
            id: "req-p".into(),
            kind: AskKind::Plan,
            tool: "ExitPlanMode".into(),
            title: "Plan listo".into(),
            command: None,
            diff: vec![],
            plan: (1..=20).map(|n| format!("paso {n}")).collect(),
            options: vec![
                contract::conversation::AskOption {
                    key: "accept".into(),
                    label: "Aprobar y editar sin preguntar".into(),
                },
                contract::conversation::AskOption {
                    key: "manual".into(),
                    label: "Aprobar, preguntando cada edición".into(),
                },
                contract::conversation::AskOption {
                    key: "no".into(),
                    label: "Seguir planeando".into(),
                },
            ],
            multi: false,
        });
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "plan · ");
        row_of(&rows, "paso 1");
        assert!(
            !rows.iter().any(|r| r.contains("paso 12")),
            "only the first lines fit the box"
        );
        row_of(&rows, "v ver el plan");
        // REQ-469: `v` asks for the plan whole in the viewer, which is the client's to open
        assert_eq!(
            press(&mut ui, &c, KeyCode::Char('v')),
            Some(Act::ViewPlan {
                node: node.clone(),
                ask: "req-p".into()
            })
        );
        assert!(
            !ui.has_pop(),
            "no list of the plan over the question any more"
        );
        // and so does a click on what the plan says, while a click on an answer answers
        let (_, rows) = frame(&ui, &c);
        let y = row_of(&rows, "paso 3");
        assert_eq!(
            ui.click(&c, INNER, 12, y),
            Some(Act::ViewPlan {
                node: node.clone(),
                ask: "req-p".into()
            })
        );
        // 2 approves asking for each edit
        assert_eq!(
            press(&mut ui, &c, KeyCode::Char('2')),
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-p".into(),
                key: "manual".into(),
                text: None,
            })
        );
        // «Seguir planeando» asks what to do next before it answers (REQ-595): nothing is sent yet
        assert_eq!(press(&mut ui, &c, KeyCode::Char('n')), None);
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "¿qué sigue?");
        row_of(&rows, "↵ envía (vacío: sigue planeando) · esc vuelve");
        // what is typed is the instruction, including the letters that are keys of the options
        for ch in "añade tests".chars() {
            assert_eq!(press(&mut ui, &c, KeyCode::Char(ch)), None);
        }
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "▸ añade tests");
        assert_eq!(
            press(&mut ui, &c, KeyCode::Enter),
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-p".into(),
                key: "no".into(),
                text: Some("añade tests".into()),
            })
        );
        // an empty line is the plain «seguir planeando», and esc goes back to the options
        press(&mut ui, &c, KeyCode::Char('3'));
        assert_eq!(
            press(&mut ui, &c, KeyCode::Enter),
            Some(Act::Answer {
                node: node.clone(),
                ask: "req-p".into(),
                key: "no".into(),
                text: None,
            })
        );
        press(&mut ui, &c, KeyCode::Down);
        press(&mut ui, &c, KeyCode::Down);
        press(&mut ui, &c, KeyCode::Enter);
        press(&mut ui, &c, KeyCode::Char('x'));
        assert_eq!(press(&mut ui, &c, KeyCode::Esc), None);
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "Aprobar y editar sin preguntar");
        assert!(!rows.iter().any(|r| r.contains("¿qué sigue?")));
        // the viewer's own «seguir planeando» leaves the box on that line
        ui.refuse("req-p");
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "¿qué sigue?");
    }

    #[test]
    fn the_checklist_is_a_strip_that_ctrl_t_and_a_click_open() {
        let c = idle();
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &c);
        let y = row_of(&rows, "☐ 1/2 tareas");
        assert_eq!(y, INNER.1, "at the top of the conversation");
        assert!(!rows.iter().any(|r| r.contains("Compilar")));
        let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        ui.key(&c, &ctrl_t);
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "☑ Buscar los usos de verify");
        row_of(&rows, "☐ Compilar");
        assert_eq!(ui.click(&c, INNER, 10, y), None);
        assert!(
            !frame(&ui, &c).1.iter().any(|r| r.contains("Compilar")),
            "a click closes it"
        );
        ui.click(&c, INNER, 10, y);
        row_of(&frame(&ui, &c).1, "☐ Compilar");
        // all done: ticked; none: no strip at all
        let mut done = idle();
        done.todos.iter_mut().for_each(|t| t.done = true);
        row_of(&frame(&Ui::default(), &done).1, "☑ 2/2 tareas");
        let mut none = idle();
        none.todos.clear();
        assert!(
            !frame(&Ui::default(), &none)
                .1
                .iter()
                .any(|r| r.contains("tareas"))
        );
    }

    #[test]
    fn the_tasks_list_has_the_newest_first_and_x_stops_the_one_that_runs() {
        let c = idle();
        let node = c.head.id.clone();
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        let mut ui = Ui::default();
        ui.key(&c, &ctrl_b);
        let (_, rows) = frame(&ui, &c);
        row_of(&rows, "segundo plano · x detiene");
        let first = row_of(&rows, "cargo build --release");
        let second = row_of(&rows, "revisar el CI cada 5 min");
        assert!(first < second, "the newest first");
        row_of(&rows, "en curso");
        assert_eq!(
            press(&mut ui, &c, KeyCode::Char('x')),
            Some(Act::StopTask {
                node: node.clone(),
                task: "bs02u3".into()
            })
        );
        assert!(!ui.has_pop());
        assert!(ui.take_says().unwrap().contains("deteniendo"));
        // the one that ended has nothing to stop, and what is typed narrows the list
        let mut ui = Ui::default();
        ui.key(&c, &ctrl_b);
        press(&mut ui, &c, KeyCode::Down);
        assert_eq!(press(&mut ui, &c, KeyCode::Char('x')), None);
        assert_eq!(press(&mut ui, &c, KeyCode::Enter), None);
        assert!(ui.has_pop(), "x on that row went to the filter");
        // ↵ stops the one that runs too, and a click on the footer's gear opens the list
        let mut ui = Ui::default();
        ui.key(&c, &ctrl_b);
        assert!(matches!(
            press(&mut ui, &c, KeyCode::Enter),
            Some(Act::StopTask { .. })
        ));
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &c);
        let y = rows
            .iter()
            .position(|r| r.contains("⚙ 1"))
            .expect("the gear in the footer");
        let x = rows[y]
            .find("⚙")
            .map(|b| rows[y][..b].chars().count())
            .unwrap();
        assert_eq!(ui.click(&c, INNER, x, y), None);
        assert!(ui.has_pop());
    }

    #[test]
    fn an_at_asks_for_files_and_tab_puts_the_path_in_the_line() {
        let conv = idle();
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "mira @s");
        assert_eq!(ui.want_files().as_deref(), Some("s"));
        assert_eq!(ui.want_files(), None, "asked once for each fragment");
        type_in(&mut ui, &conv, "rc");
        assert_eq!(ui.want_files().as_deref(), Some("src"));
        ui.found("s", vec!["stale.rs".into()]);
        let (_, rows) = frame(&ui, &conv);
        assert!(
            !rows.iter().any(|r| r.contains("stale.rs")),
            "an answer for what is no longer typed is dropped"
        );
        ui.found("src", vec!["src/auth.rs".into(), "src/session.rs".into()]);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "archivos");
        row_of(&rows, "src/auth.rs");
        press(&mut ui, &conv, KeyCode::Down);
        assert_eq!(press(&mut ui, &conv, KeyCode::Tab), None);
        assert_eq!(ui.editor.text(), "mira @src/session.rs ");
        let (_, rows) = frame(&ui, &conv);
        assert!(!rows.iter().any(|r| r.contains("archivos")));
        assert_eq!(ui.want_files(), None);
        // Enter takes the one lit and sends nothing; esc closes the list and the line stays
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "@au");
        ui.want_files();
        ui.found("au", vec!["src/auth.rs".into()]);
        assert_eq!(press(&mut ui, &conv, KeyCode::Enter), None);
        assert_eq!(ui.editor.text(), "@src/auth.rs ");
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "@au");
        ui.want_files();
        ui.found("au", vec!["src/auth.rs".into()]);
        press(&mut ui, &conv, KeyCode::Esc);
        assert_eq!(ui.editor.text(), "@au");
        assert!(!frame(&ui, &conv).1.iter().any(|r| r.contains("archivos")));
        // an address is not a mention, nor is a bare @
        let mut ui = Ui::default();
        type_in(&mut ui, &conv, "yo@correo @");
        assert_eq!(ui.want_files(), None);
    }

    #[test]
    fn a_pane_too_small_draws_nothing_and_a_click_in_it_does_nothing() {
        let conv = idle();
        let mut ui = Ui::default();
        let mut g = Grid::new(20, 6);
        ui.draw(&mut g, &conv, (1, 1, 10, 4), &[], true, 0);
        assert!(g.rows_plain().iter().all(String::is_empty));
        assert_eq!(ui.click(&conv, (1, 1, 10, 4), 3, 3), None);
    }

    /// REQ-558: consecutive tools are one row, and a click on it opens the tools one by one.
    #[test]
    fn consecutive_tools_are_one_row_that_a_click_opens_and_closes() {
        let mut conv = idle();
        let tool = |id: &str, name: &str, arg: &str, state: &str| -> Item {
            serde_json::from_value(serde_json::json!({
                "t": "tool", "id": id, "name": name, "arg": arg, "state": state
            }))
            .unwrap()
        };
        conv.items = vec![
            Item::Text {
                text: "Voy a mirar.".into(),
                live: false,
                blocks: None,
            },
            tool("a", "Grep", "verify", "ok"),
            tool("b", "Bash", "cargo test", "ok"),
            tool("c", "Bash", "git status", "failed"),
            tool("d", "Edit", "src/a.rs", "ok"),
            tool("e", "Bash", "sleep 9", "run"),
        ];
        conv.first = 0;
        let mut ui = Ui::default();
        let (_, rows) = frame(&ui, &conv);
        let at = row_of(&rows, "▸ Buscó 1 patrón, ejecutó 2 comandos");
        assert!(
            !rows.iter().any(|r| r.contains("Bash(cargo test)")),
            "folded: no row of its own"
        );
        let edit = row_of(&rows, "● Edit(src/a.rs)");
        row_of(&rows, "● Bash(sleep 9)");
        assert_eq!(
            edit,
            at + 2,
            "a blank row between the run and what follows it"
        );
        assert!(rows[at + 1].trim().is_empty(), "{rows:#?}");
        assert!(ui.click(&conv, INNER, 10, at).is_none());
        assert_eq!(ui.picked, None, "a row of a run picks nothing");
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "▾ Buscó 1 patrón, ejecutó 2 comandos");
        row_of(&rows, "● Grep(verify)");
        row_of(&rows, "● Bash(git status)");
        let at = row_of(&rows, "▾ Buscó");
        ui.click(&conv, INNER, 10, at);
        let (_, rows) = frame(&ui, &conv);
        row_of(&rows, "▸ Buscó");
        // ^O on a tool that is folded opens the run: the last tool is in it.
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        let mut short = conv.clone();
        short.items.truncate(3);
        let mut ui2 = Ui::default();
        ui2.key(&short, &ctrl_o);
        let (_, rows) = frame(&ui2, &short);
        row_of(&rows, "● Bash(cargo test)");
        // The arrows skip what has no row.
        ui.unfolded.clear();
        ui.step_item(&conv, 0, true);
        assert_eq!(
            ui.picked,
            Some(4),
            "from the text to the edit: the folded tools have no row"
        );
    }

    /// REQ-558: what was picked loses its pick when its run is folded, the fold row is not lit by a
    /// pick, the arrows do not stop inside a folded run, and a run that is gone is forgotten.
    #[test]
    fn folding_a_run_drops_a_pick_that_has_no_row_and_a_run_that_is_gone_is_forgotten() {
        let tool = |id: &str, name: &str| -> Item {
            serde_json::from_value(serde_json::json!({
                "t": "tool", "id": id, "name": name, "arg": id, "state": "ok"
            }))
            .unwrap()
        };
        let mut conv = idle();
        conv.items = vec![
            Item::Text {
                text: "a".into(),
                live: false,
                blocks: None,
            },
            tool("x", "Bash"),
            tool("y", "Bash"),
            tool("z", "Edit"),
            tool("w", "Bash"),
            tool("v", "Bash"),
        ];
        conv.first = 10;
        let mut ui = unfolded(&conv);
        ui.picked = Some(12);
        let (_, rows) = frame(&ui, &conv);
        let at = row_of(&rows, "▾ Ejecutó 2 comandos");
        // Folding the first run takes the row away from the item that was picked in it.
        ui.click(&conv, INNER, 10, at);
        assert_eq!(ui.picked, None);
        // Down from the text over a folded run goes to the edit, and at the end inside a folded run it stays.
        ui.step_item(&conv, 10, true);
        assert_eq!(ui.picked, Some(13));
        ui.unfolded.remove(&14);
        ui.step_item(&conv, 13, true);
        assert_eq!(
            ui.picked,
            Some(13),
            "the rest is a folded run: nothing to pick down there"
        );
        // The runs that were opened and are not a run any more are forgotten when the node arrives.
        ui.unfolded.insert(99);
        ui.sync(&conv);
        assert!(ui.unfolded.iter().all(|k| *k != 99));
    }
}
