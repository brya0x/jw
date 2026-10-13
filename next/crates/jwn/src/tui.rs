//! `jwn tui`: the terminal client. Workspaces on the left with the marks of what waits, the
//! open workspace's panes as tabs, one pane's screen; `a` lists what waits across all of
//! them. It draws the same documents the phone draws and keeps its own seen record, like
//! the phone does. `View` is pure (documents in, text lines out) so it is tested without a
//! terminal; `run` only moves bytes between the socket, the keyboard and the screen.

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::time::Duration;

use contract::{Attention, AttentionKind, Node, Pane, Session, Workspace, WorkspaceKind};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use wire::Client;

/// The One Dark palette of the mobile prototype, as 24-bit colours.
type Rgb = (u8, u8, u8);
const DIM: Rgb = (0x5c, 0x63, 0x70);
const LINE: Rgb = (0x3e, 0x44, 0x51);
const BLUE: Rgb = (0x61, 0xaf, 0xef);
const GREEN: Rgb = (0x98, 0xc3, 0x79);
const YELLOW: Rgb = (0xe5, 0xc0, 0x7b);
const RED: Rgb = (0xe0, 0x6c, 0x75);
const MAGENTA: Rgb = (0xc6, 0x78, 0xdd);
const CYAN: Rgb = (0x56, 0xb6, 0xc2);
const SEL_BG: Rgb = (0x2c, 0x31, 0x3a);
const BAR_BG: Rgb = (0x21, 0x25, 0x2b);

fn fg(c: Rgb, text: &str) -> String {
    format!("\x1b[38;2;{};{};{}m{text}\x1b[39m", c.0, c.1, c.2)
}

fn bold(text: &str) -> String {
    format!("\x1b[1m{text}\x1b[22m")
}

/// Paint a whole line, padded to `cols`, on a background.
fn on_bg(c: Rgb, line: &str, cols: usize) -> String {
    let pad = cols.saturating_sub(width(line));
    format!(
        "\x1b[48;2;{};{};{}m{line}{}\x1b[49m",
        c.0,
        c.1,
        c.2,
        " ".repeat(pad)
    )
}

/// The width of `text` on screen, ignoring colour escapes.
fn width(text: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in text.chars() {
        match (in_escape, c) {
            (false, '\x1b') => in_escape = true,
            (true, 'm') => in_escape = false,
            (false, _) => n += 1,
            _ => {}
        }
    }
    n
}

/// Cut plain text to `cols` columns with an ellipsis.
fn clip(text: &str, cols: usize) -> String {
    if text.chars().count() <= cols {
        return text.to_owned();
    }
    text.chars()
        .take(cols.saturating_sub(1))
        .chain(['…'])
        .collect()
}

/// Cut a coloured line to `cols` visible columns, keeping its escapes.
fn cut(line: &str, cols: usize) -> String {
    let (mut out, mut n, mut in_escape) = (String::new(), 0, false);
    for c in line.chars() {
        match (in_escape, c) {
            (false, '\x1b') => in_escape = true,
            (true, 'm') => in_escape = false,
            (false, _) => {
                if n == cols {
                    break;
                }
                n += 1;
            }
            _ => {}
        }
        out.push(c);
    }
    out.push_str("\x1b[0m");
    out
}

fn pad_to(line: &str, cols: usize) -> String {
    format!("{line}{}", " ".repeat(cols.saturating_sub(width(line))))
}

fn strip(line: &str) -> String {
    let mut out = String::new();
    let mut in_escape = false;
    for c in line.chars() {
        match (in_escape, c) {
            (false, '\x1b') => in_escape = true,
            (true, 'm') => in_escape = false,
            (false, _) => out.push(c),
            _ => {}
        }
    }
    out
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
        AttentionKind::ShellDone => format!("comando terminó · salió {}", a.exit_code.unwrap_or(0)),
        AttentionKind::Screen => "espera en pantalla".into(),
        AttentionKind::AgentDone => "agente listo".into(),
        AttentionKind::Merged => "mergeado".into(),
    }
}

/// Only what needs a decision or went wrong is tinted; the rest stays dim.
fn line_colour(a: &Attention) -> Rgb {
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
    Enter,
    Avisos,
    Back,
    Quit,
}

#[derive(PartialEq, Eq, Debug)]
pub enum After {
    Continue,
    Quit,
}

fn rev_of(n: &Node) -> u64 {
    match n {
        Node::Session(s) => s.head.rev,
        Node::Workspace(w) => w.head.rev,
        Node::Pane(p) => p.head.rev,
    }
}

/// A row of the sidebar tree: a workspace, or one of its panes.
#[derive(Clone, PartialEq, Eq, Debug)]
struct RowRef {
    ws: String,
    pane: Option<String>,
}

pub struct View {
    session: Option<Session>,
    workspaces: HashMap<String, Workspace>,
    panes: HashMap<String, Pane>,
    revs: HashMap<String, u64>,
    /// Node id -> revision this terminal had it open at. Never sent anywhere.
    seen: HashMap<String, u64>,
    mode: Mode,
    /// The highlighted row of the tree.
    cursor: usize,
    avisos_selected: usize,
    /// The row opened with Enter; the highlight alone only previews.
    opened: Option<RowRef>,
    status: String,
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
            cursor: 0,
            avisos_selected: 0,
            opened: None,
            status: status.to_owned(),
        }
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

    /// Every workspace, each followed by its panes (once its document has arrived).
    fn rows(&self) -> Vec<RowRef> {
        let Some(s) = &self.session else {
            return vec![];
        };
        let mut rows = Vec::new();
        for w in &s.workspaces {
            rows.push(RowRef {
                ws: w.id.clone(),
                pane: None,
            });
            if let Some(doc) = self.workspaces.get(&w.id) {
                rows.extend(doc.panes.iter().map(|p| RowRef {
                    ws: w.id.clone(),
                    pane: Some(p.id.clone()),
                }));
            }
        }
        rows
    }

    /// The pane whose screen is on the right: the highlighted pane, or a workspace's first.
    fn shown_pane(&self) -> Option<String> {
        let row = self.rows().get(self.cursor).cloned()?;
        row.pane.or_else(|| {
            self.workspaces
                .get(&row.ws)?
                .panes
                .first()
                .map(|p| p.id.clone())
        })
    }

    /// Nodes this view would draw but does not have yet.
    pub fn wants(&self) -> Vec<String> {
        let Some(s) = &self.session else {
            return vec![];
        };
        let mut ids: Vec<String> = s.workspaces.iter().map(|w| w.id.clone()).collect();
        ids.extend(
            self.workspaces
                .values()
                .flat_map(|w| w.panes.iter().map(|p| p.id.clone())),
        );
        ids.retain(|id| !self.revs.contains_key(id));
        ids
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
                    .map(|a| (w.id.clone(), w.name.clone(), a))
            })
            .collect()
    }

    /// Open a row, marking its workspace and pane seen on this terminal.
    fn open_row(&mut self, row: RowRef) {
        self.mark_seen(&row.ws);
        let pane = row.pane.clone().or_else(|| {
            self.workspaces
                .get(&row.ws)?
                .panes
                .first()
                .map(|p| p.id.clone())
        });
        if let Some(p) = pane {
            self.mark_seen(&p);
        }
        self.opened = Some(row);
    }

    pub fn press(&mut self, key: Key) -> After {
        let rows = self.rows();
        match (self.mode, key) {
            (_, Key::Quit) => return After::Quit,
            (Mode::Workspaces, Key::Back) => return After::Quit,
            (Mode::Avisos, Key::Back | Key::Avisos) => self.mode = Mode::Workspaces,
            (Mode::Workspaces, Key::Avisos) => {
                self.mode = Mode::Avisos;
                self.avisos_selected = 0;
            }
            (Mode::Workspaces, Key::Down) => {
                self.cursor = (self.cursor + 1).min(rows.len().saturating_sub(1))
            }
            (Mode::Workspaces, Key::Up) => self.cursor = self.cursor.saturating_sub(1),
            (Mode::Workspaces, Key::Enter) => {
                if let Some(row) = rows.get(self.cursor).cloned() {
                    self.open_row(row);
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
                    let row = RowRef {
                        pane: (a.node != ws).then_some(a.node),
                        ws,
                    };
                    self.mode = Mode::Workspaces;
                    if let Some(i) = rows.iter().position(|r| *r == row) {
                        self.cursor = i;
                    }
                    self.open_row(row);
                }
            }
        }
        After::Continue
    }

    pub fn draw(&self, cols: usize, rows: usize) -> Vec<String> {
        let Some(session) = &self.session else {
            return vec![fg(DIM, &format!("conectando… {}", self.status))];
        };
        let side_w = 36.min(cols / 2);
        let main_w = cols.saturating_sub(side_w + 3);
        let body_h = rows.saturating_sub(2);
        let waiting = self.avisos().len();

        let title = format!(
            " {} {}",
            bold("jwn"),
            fg(DIM, &format!("· {}", session.name))
        );
        let right = fg(DIM, &format!("{}  rev {} ", self.status, session.head.rev));
        let gap = cols.saturating_sub(width(&title) + width(&right));
        let mut out = vec![on_bg(
            BAR_BG,
            &format!("{title}{}{right}", " ".repeat(gap)),
            cols,
        )];

        let side = self.sidebar(session, side_w, waiting);
        let main = match self.mode {
            Mode::Workspaces => self.main_pane(main_w),
            Mode::Avisos => self.avisos_list(main_w),
        };
        for i in 0..body_h {
            let l = side.get(i).cloned().unwrap_or_default();
            let r = main.get(i).cloned().unwrap_or_default();
            out.push(cut(
                &format!("{}{} {r}", pad_to(&l, side_w), fg(LINE, " │")),
                cols,
            ));
        }
        let (chip, hint) = match self.mode {
            Mode::Workspaces => (" NORMAL ", "j/k mover  ↵ abrir  a avisos  q salir"),
            Mode::Avisos => (" AVISOS ", "j/k mover  ↵ ir  esc volver"),
        };
        let chip_bg = if self.mode == Mode::Avisos {
            YELLOW
        } else {
            BLUE
        };
        let chip = format!(
            "\x1b[48;2;{};{};{}m\x1b[38;2;33;37;43m{}\x1b[39m\x1b[49m",
            chip_bg.0,
            chip_bg.1,
            chip_bg.2,
            bold(chip)
        );
        out.push(on_bg(BAR_BG, &format!("{chip} {}", fg(DIM, hint)), cols));
        out
    }

    /// The tree: each workspace, then its panes indented beneath it.
    fn sidebar(&self, session: &Session, w: usize, waiting: usize) -> Vec<String> {
        let label = format!("workspaces · {}", session.workspaces.len());
        let mut side = vec![format!(
            " {} {}",
            fg(DIM, &label),
            fg(
                LINE,
                &"─".repeat(w.saturating_sub(label.chars().count() + 3))
            )
        )];
        let rows = self.rows();
        let highlighted =
            |row: &RowRef| self.mode == Mode::Workspaces && rows.get(self.cursor) == Some(row);
        for ws in &session.workspaces {
            let un = self.unseen(&ws.attention);
            let (glyph, tone) = un.first().map(mark).unwrap_or((" ", DIM));
            let count = if un.len() > 1 {
                fg(DIM, &un.len().to_string())
            } else {
                String::new()
            };
            let name = clip(&ws.name, w.saturating_sub(7));
            let head = format!(
                "{}{count}",
                pad_to(
                    &format!(" {} {}", bold(&fg(tone, glyph)), bold(&name)),
                    w.saturating_sub(3)
                )
            );
            let detail = match ws.kind {
                WorkspaceKind::Worktree => format!("↳ {}", ws.branch.clone().unwrap_or_default()),
                WorkspaceKind::Folder => "carpeta".into(),
            };
            let mut lines = vec![
                head,
                format!("   {}", fg(DIM, &clip(&detail, w.saturating_sub(4)))),
            ];
            if let Some(a) = un.first() {
                lines.push(format!(
                    "   {}",
                    fg(line_colour(a), &clip(&what(a), w.saturating_sub(4)))
                ));
            }
            let on = highlighted(&RowRef {
                ws: ws.id.clone(),
                pane: None,
            });
            side.extend(
                lines
                    .into_iter()
                    .map(|l| if on { on_bg(SEL_BG, &l, w) } else { l }),
            );
            for p in self
                .workspaces
                .get(&ws.id)
                .map(|d| d.panes.as_slice())
                .unwrap_or_default()
            {
                let un = self.unseen(&p.attention);
                let m = un
                    .first()
                    .map(|a| format!(" {}", fg(mark(a).1, mark(a).0)))
                    .unwrap_or_default();
                let line = format!(
                    "   {} {}{m}",
                    fg(LINE, "└"),
                    clip(&p.title, w.saturating_sub(10))
                );
                let on = highlighted(&RowRef {
                    ws: ws.id.clone(),
                    pane: Some(p.id.clone()),
                });
                side.push(if on { on_bg(SEL_BG, &line, w) } else { line });
            }
        }
        if waiting > 0 {
            side.push(String::new());
            side.push(format!(
                " {}",
                fg(YELLOW, &format!("{waiting} esperando · a"))
            ));
        }
        side
    }

    fn main_pane(&self, w: usize) -> Vec<String> {
        let rows = self.rows();
        let Some(row) = rows.get(self.cursor) else {
            return vec![fg(DIM, "sin workspace")];
        };
        let Some(pane_id) = self.shown_pane() else {
            return vec![fg(DIM, "este workspace no tiene paneles")];
        };
        let Some(p) = self.panes.get(&pane_id) else {
            return vec![fg(DIM, "abriendo el panel…")];
        };
        let opened = self.opened.as_ref() == Some(row);
        let mut out = vec![format!(
            " {} {}",
            if opened {
                bold(&p.title)
            } else {
                fg(DIM, &p.title)
            },
            fg(
                DIM,
                &clip(&p.cwd, w.saturating_sub(p.title.chars().count() + 3))
            )
        )];
        out.push(
            fg(if opened { BLUE } else { LINE }, &"━".repeat(w.min(12)))
                + &fg(LINE, &"─".repeat(w.saturating_sub(12))),
        );
        let c = p.screen.cursor;
        for (r, line) in p.screen.lines.iter().enumerate() {
            let text = clip(line, w);
            if c.visible && r == c.row as usize {
                let chars: Vec<char> = format!("{text:<width$}", width = c.col as usize + 1)
                    .chars()
                    .collect();
                let col = c.col as usize;
                let before: String = chars[..col].iter().collect();
                let after: String = chars[col + 1..].iter().collect();
                out.push(format!("{before}\x1b[7m{}\x1b[27m{after}", chars[col]));
            } else {
                out.push(text);
            }
        }
        if !opened {
            out.push(String::new());
            out.push(fg(DIM, "vista previa · ↵ abre y marca visto"));
        }
        out
    }

    fn avisos_list(&self, w: usize) -> Vec<String> {
        let items = self.avisos();
        let mut out = vec![format!(
            "{} {}",
            fg(DIM, "avisos"),
            fg(LINE, &"─".repeat(w.saturating_sub(7)))
        )];
        if items.is_empty() {
            out.push(fg(
                DIM,
                "Nada espera. Cuando algo termine o pregunte, aparece aquí.",
            ));
            return out;
        }
        for (i, (_, ws, a)) in items.iter().enumerate() {
            let (g, tone) = mark(a);
            let line1 = format!(
                " {} {}",
                bold(&fg(tone, g)),
                bold(&clip(ws, w.saturating_sub(5)))
            );
            let line2 = format!(
                "   {}",
                fg(line_colour(a), &clip(&what(a), w.saturating_sub(4)))
            );
            for l in [line1, line2] {
                out.push(if i == self.avisos_selected {
                    on_bg(SEL_BG, &l, w)
                } else {
                    l
                });
            }
        }
        out
    }
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
        KeyCode::Enter => Key::Enter,
        KeyCode::Char('a') => Key::Avisos,
        KeyCode::Esc => Key::Back,
        KeyCode::Char('q') => Key::Quit,
        _ => return None,
    })
}

pub fn run() -> Result<(), String> {
    let socket = identity::socket_path();
    let mut client = Client::connect(&socket, "jwn-tui")
        .map_err(|e| format!("cannot reach {}: {e}", socket.display()))?;
    let root = client.root.clone();
    client.open(&root).map_err(|e| e.to_string())?;
    let mut view = View::new("en vivo");
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
        let frame = view.draw(cols as usize, rows as usize);
        if frame != last {
            let mut out = stdout();
            let _ = execute!(out, terminal::Clear(terminal::ClearType::All));
            for (i, line) in frame.iter().enumerate() {
                let _ = execute!(out, cursor::MoveTo(0, i as u16));
                let _ = write!(out, "{line}");
            }
            let _ = out.flush();
            if let Some(path) = &dump {
                let plain: Vec<String> = frame.iter().map(|l| strip(l)).collect();
                let _ = std::fs::write(path, plain.join("\n"));
            }
            last = frame;
        }
        client
            .poll(Duration::from_millis(40))
            .map_err(|e| e.to_string())?;
        while event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
            let Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read().map_err(|e| e.to_string())?
            else {
                continue;
            };
            if let Some(key) = key_of(code)
                && view.press(key) == After::Quit
            {
                return Ok(());
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

    fn text(v: &View) -> String {
        v.draw(110, 30)
            .iter()
            .map(|l| strip(l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_sidebar_shows_the_most_urgent_unseen_mark_and_what_it_is() {
        let t = text(&fixture_view());
        assert!(t.contains("app: feat/login"), "{t}");
        assert!(
            t.contains("pide permiso"),
            "the top attention of the first workspace: {t}"
        );
        assert!(t.contains("comando terminó · salió 2"), "{t}");
        assert!(t.contains("esperando"), "{t}");
    }

    #[test]
    fn panes_are_rows_of_the_tree_not_tabs() {
        let t = text(&fixture_view());
        assert!(
            t.contains("└ zsh"),
            "the first workspace's panes sit under it: {t}"
        );
        assert!(t.contains("└ claude"), "{t}");
    }

    #[test]
    fn enter_opens_the_workspace_and_marks_it_seen_on_this_terminal_only() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        v.press(Key::Enter);
        let t = text(&v);
        assert!(v.avisos().len() < before, "opening cleared something");
        assert!(
            t.contains("$ cargo test"),
            "the first pane's screen is drawn: {t}"
        );
    }

    #[test]
    fn moving_the_highlight_onto_a_pane_only_previews_its_screen() {
        let mut v = fixture_view();
        let before = v.avisos().len();
        v.press(Key::Down);
        v.press(Key::Down);
        assert_eq!(v.avisos().len(), before, "a preview marks nothing seen");
        let t = text(&v);
        assert!(t.contains("vista previa"), "{t}");
        assert!(t.contains("$ git status"), "the second pane's screen: {t}");
    }

    #[test]
    fn avisos_lists_what_waits_and_enter_goes_to_the_named_pane() {
        let mut v = fixture_view();
        v.press(Key::Avisos);
        let listed = text(&v);
        assert!(
            listed.contains("AVISOS") && listed.contains("pide permiso"),
            "{listed}"
        );
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
    fn frames_fit_the_terminal() {
        let v = fixture_view();
        for line in v.draw(80, 24) {
            assert!(
                width(&line) <= 80,
                "{} wide: {}",
                width(&line),
                strip(&line)
            );
        }
    }
}
