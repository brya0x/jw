//! The programs a new panel can start: the list `config.toml` holds under the config directory
//! and the menu `N` opens over it (prototype: `docs/prototypes/tui-levels.html`, spec REQ-239 to
//! REQ-246). The client reads the file and never writes it. A missing file, or one with no
//! `[[program]]`, is the built-in list; a file that does not read keeps the list the client had
//! and says why. The daemon never sees any of it: a pick is a `new_pane` with a `program`.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crossterm::event::KeyCode;
use serde::Deserialize;
use toml::Spanned;

use super::{Grid, Rect, Style, clip, look};

/// The letters the menu itself answers to; an entry cannot take one as its key.
const RESERVED: [char; 3] = ['j', 'k', 'n'];
/// How wide the menu is, at most.
const WIDTH: usize = 66;
/// The columns, inside the box, where a row's name and its command start.
const NAME_AT: usize = 8;
const COMMAND_AT: usize = 28;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    pub name: String,
    /// The program and its arguments, as `new_pane` takes them.
    pub command: Vec<String>,
    /// The letter that opens it from the menu: one the menu does not use and no earlier entry has.
    pub key: Option<char>,
}

/// The list the menu shows, and why the file was not read when it was not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Programs {
    list: Vec<Program>,
    error: Option<String>,
}

impl Default for Programs {
    fn default() -> Programs {
        Programs {
            list: built_in(),
            error: None,
        }
    }
}

fn built_in() -> Vec<Program> {
    vec![Program {
        name: "claude".into(),
        command: vec!["claude".into()],
        key: Some('c'),
    }]
}

#[derive(Deserialize)]
struct Doc {
    #[serde(default)]
    program: Vec<Spanned<Entry>>,
}

#[derive(Deserialize)]
struct Entry {
    name: Option<String>,
    command: Option<Vec<String>>,
    key: Option<String>,
}

impl Programs {
    pub fn path() -> PathBuf {
        identity::config_dir().join("config.toml")
    }

    /// What the file says now: the list, or why it cannot be read.
    pub fn read() -> Result<Vec<Program>, String> {
        Programs::read_from(&Programs::path())
    }

    fn read_from(path: &Path) -> Result<Vec<Program>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Programs::parse(&text),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(built_in()),
            Err(e) => Err(format!("no se pudo leer config.toml: {e}")),
        }
    }

    /// The entries of `text`, or the first thing wrong with it and the line it is on. A key that
    /// is not one lowercase letter, or that the menu or an earlier entry has, is no key.
    pub fn parse(text: &str) -> Result<Vec<Program>, String> {
        let doc: Doc = toml::from_str(text).map_err(|e| match e.span() {
            Some(span) => format!("línea {}: {}", line_of(text, span.start), e.message()),
            None => e.message().to_owned(),
        })?;
        let mut taken = RESERVED.to_vec();
        let mut list = Vec::new();
        for entry in doc.program {
            let at = header_line(text, entry.span().start);
            let entry = entry.into_inner();
            let name = entry
                .name
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| format!("línea {at}: [[program]] sin «name»"))?;
            let command = entry
                .command
                .filter(|c| c.first().is_some_and(|p| !p.trim().is_empty()))
                .ok_or_else(|| format!("línea {at}: [[program]] sin «command»"))?;
            let key = entry
                .key
                .as_deref()
                .and_then(one_letter)
                .filter(|k| !taken.contains(k));
            taken.extend(key);
            list.push(Program { name, command, key });
        }
        Ok(if list.is_empty() { built_in() } else { list })
    }

    /// What reading the file gave: a new list, or the reason the one we have stays.
    pub fn apply(&mut self, read: Result<Vec<Program>, String>) {
        match read {
            Ok(list) => {
                self.list = list;
                self.error = None;
            }
            Err(why) => self.error = Some(why),
        }
    }

    /// The menu's rows: the shell, then the programs. Row 0 is the shell.
    fn rows(&self) -> usize {
        self.list.len() + 1
    }

    fn pick(&self, row: usize) -> Pick {
        match row.checked_sub(1).and_then(|i| self.list.get(i)) {
            Some(p) => Pick::Program(p.command.clone()),
            None => Pick::Shell,
        }
    }

    fn row_of_key(&self, c: char) -> Option<usize> {
        if c == 'n' {
            return Some(0);
        }
        self.list
            .iter()
            .position(|p| p.key == Some(c))
            .map(|i| i + 1)
    }
}

fn one_letter(key: &str) -> Option<char> {
    let mut chars = key.chars();
    chars
        .next()
        .filter(|c| c.is_ascii_lowercase() && chars.next().is_none())
}

/// The 1-based line of byte `at` in `text`.
fn line_of(text: &str, at: usize) -> usize {
    text.get(..at).map_or(1, |t| t.matches('\n').count() + 1)
}

/// The line of the `[[program]]` an entry starts at: the span of an entry may begin at its first
/// key, and the person looks for the header.
fn header_line(text: &str, at: usize) -> usize {
    let line = line_of(text, at);
    text.lines()
        .enumerate()
        .take(line)
        .filter(|(_, l)| l.trim_start().starts_with("[[program]]"))
        .last()
        .map_or(line, |(i, _)| i + 1)
}

/// What the menu started.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pick {
    Shell,
    Program(Vec<String>),
}

/// The menu `N` opens: `sel` is the row the keys are on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Launcher {
    sel: usize,
}

impl Launcher {
    /// The menu after `code`, and what it started. `None` closes it.
    pub fn press(self, code: KeyCode, programs: &Programs) -> (Option<Launcher>, Option<Pick>) {
        let at = |sel: usize| Some(Launcher { sel });
        let last = programs.rows() - 1;
        match code {
            KeyCode::Esc => (None, None),
            KeyCode::Char('j') | KeyCode::Down => (at((self.sel + 1).min(last)), None),
            KeyCode::Char('k') | KeyCode::Up => (at(self.sel.saturating_sub(1)), None),
            KeyCode::Enter => (None, Some(programs.pick(self.sel.min(last)))),
            KeyCode::Char(c) => match programs.row_of_key(c) {
                Some(row) => (None, Some(programs.pick(row))),
                None => (Some(self), None),
            },
            _ => (Some(self), None),
        }
    }

    /// What a click at (`x`, `y`) asks of the menu drawn over `area`: a row starts it; outside the
    /// box closes it; elsewhere in the box does nothing.
    pub fn click(
        self,
        area: Rect,
        programs: &Programs,
        x: usize,
        y: usize,
    ) -> (Option<Launcher>, Option<Pick>) {
        let (bx, by, bw, bh) = frame(area, programs);
        if x < bx || x >= bx + bw || y < by || y >= by + bh {
            return (None, None);
        }
        match y.checked_sub(by + 3).filter(|row| *row < programs.rows()) {
            Some(row) => (None, Some(programs.pick(row))),
            None => (Some(self), None),
        }
    }

    /// Draw the menu over `area`, near its top and centred.
    pub fn draw(&self, g: &mut Grid, area: Rect, programs: &Programs) {
        let (bx, by, bw, bh) = frame(area, programs);
        g.clear_box((bx, by, bw, bh));
        g.boxed(
            (bx, by, bw, bh),
            "panel nuevo",
            look::blue(),
            Style::fg(look::fg()).bold(),
            None,
        );
        let ground = look::panel();
        let inner = bw.saturating_sub(6);
        g.put(
            bx + 3,
            by + 2,
            &clip(
                "el panel arranca un shell y corre el programa primero",
                inner,
            ),
            Style::fg(look::dim()).on(ground),
        );
        let shell = ("n".to_owned(), "shell".to_owned(), "$SHELL".to_owned());
        let rows = std::iter::once(shell).chain(programs.list.iter().map(|p| {
            (
                p.key.map(String::from).unwrap_or_default(),
                p.name.clone(),
                p.command.join(" "),
            )
        }));
        for (i, (key, name, command)) in rows.enumerate() {
            let y = by + 3 + i;
            let on = i == self.sel;
            let ground = if on { look::sel() } else { look::panel() };
            g.put(
                bx + 1,
                y,
                &" ".repeat(bw.saturating_sub(2)),
                Style::default().on(ground),
            );
            let name_style = match (on, i) {
                (true, _) => Style::fg(look::blue()).bold(),
                (false, 0) => Style::fg(look::fg()).bold(),
                _ => Style::fg(look::fg()),
            };
            if on {
                g.put(bx + 3, y, "▸", Style::fg(look::blue()).bold().on(ground));
            }
            g.put(bx + 5, y, &key, Style::fg(look::yellow()).bold().on(ground));
            g.put(
                bx + NAME_AT,
                y,
                &clip(&name, COMMAND_AT - NAME_AT - 2),
                name_style.on(ground),
            );
            g.put(
                bx + COMMAND_AT,
                y,
                &clip(&command, bw.saturating_sub(COMMAND_AT + 4)),
                Style::fg(look::dim()).on(ground),
            );
        }
        let mut y = by + 3 + programs.rows() + 1;
        if let Some(why) = &programs.error {
            g.put(
                bx + 3,
                y,
                &clip(&format!("config.toml · {why}"), inner),
                Style::fg(look::red()).on(ground),
            );
            g.put(
                bx + 3,
                y + 1,
                "se sigue con la lista anterior",
                Style::fg(look::dim()).on(ground),
            );
            y += 2;
        }
        let dim = Style::fg(look::dim()).on(ground);
        g.put(
            bx + 3,
            y,
            &clip("↵ abrir · la letra abre directo · esc cancelar", inner),
            dim,
        );
        g.put(
            bx + 3,
            y + 1,
            &clip(
                &format!("se edita en ~/.config/{}/config.toml", identity::get().name),
                inner,
            ),
            dim,
        );
    }
}

/// The box: its place and size inside `area`, tall enough for the rows, the error and the footer.
pub(super) fn frame((x, y, w, _): Rect, programs: &Programs) -> Rect {
    let lines = 1 + programs.rows() + 1 + if programs.error.is_some() { 2 } else { 0 } + 2;
    let bw = WIDTH.min(w.saturating_sub(2)).max(20);
    (x + w.saturating_sub(bw) / 2, y + 2, bw, lines + 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"# programs
[[program]]
name = "claude"
key = "c"
command = ["claude"]

[[program]]
name = "claude opus"
key = "o"
command = ["claude", "--model", "opus"]

[[program]]
name = "tests"
command = ["cargo", "test"]
"#;

    fn programs(text: &str) -> Programs {
        let mut p = Programs::default();
        p.apply(Programs::parse(text));
        p
    }

    #[test]
    fn the_entries_come_in_the_files_order_with_their_commands_and_keys() {
        let list = Programs::parse(FILE).unwrap();
        let seen: Vec<_> = list
            .iter()
            .map(|p| (p.name.as_str(), p.key, p.command.join(" ")))
            .collect();
        assert_eq!(
            seen,
            [
                ("claude", Some('c'), "claude".to_owned()),
                ("claude opus", Some('o'), "claude --model opus".to_owned()),
                ("tests", None, "cargo test".to_owned()),
            ]
        );
    }

    #[test]
    fn no_file_or_no_entries_is_the_built_in_claude_and_entries_replace_it() {
        assert_eq!(Programs::parse("").unwrap(), built_in());
        assert_eq!(Programs::parse("# nothing yet\n").unwrap(), built_in());
        let dir = std::env::temp_dir().join(format!("jwn-programs-{}", std::process::id()));
        assert_eq!(Programs::read_from(&dir.join("none.toml")), Ok(built_in()));
        std::fs::create_dir_all(&dir).unwrap();
        let unreadable = Programs::read_from(&dir).unwrap_err();
        assert!(
            unreadable.starts_with("no se pudo leer config.toml: "),
            "{unreadable}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let only: Vec<_> = Programs::parse("[[program]]\nname = \"v\"\ncommand = [\"nvim\"]\n")
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(
            only,
            ["v"],
            "claude is not added to a list of the person's own"
        );
    }

    #[test]
    fn a_key_the_menu_uses_a_repeat_or_a_bad_one_is_no_key() {
        let entry =
            |key: &str| format!("[[program]]\nname = \"a\"\nkey = \"{key}\"\ncommand = [\"a\"]\n");
        let text = [
            entry("j"),
            entry("n"),
            entry("c"),
            entry("c"),
            entry("C"),
            entry("cc"),
            entry(""),
        ]
        .concat();
        let keys: Vec<_> = Programs::parse(&text)
            .unwrap()
            .iter()
            .map(|p| p.key)
            .collect();
        assert_eq!(keys, [None, None, Some('c'), None, None, None, None]);
    }

    #[test]
    fn what_is_wrong_is_said_with_the_line_of_its_program() {
        let no_command =
            "[[program]]\nname = \"a\"\ncommand = [\"a\"]\n\n[[program]]\nname = \"b\"\n";
        assert_eq!(
            Programs::parse(no_command),
            Err("línea 5: [[program]] sin «command»".to_owned())
        );
        let no_name = "[[program]]\ncommand = [\"a\"]\n";
        assert_eq!(
            Programs::parse(no_name),
            Err("línea 1: [[program]] sin «name»".to_owned())
        );
        let empty = "[[program]]\nname = \"a\"\ncommand = []\n";
        assert_eq!(
            Programs::parse(empty),
            Err("línea 1: [[program]] sin «command»".to_owned())
        );
        let bad = Programs::parse("[[program]]\nname = \"a\"\ncommand = \"claude\"\n").unwrap_err();
        assert!(bad.starts_with("línea 3: "), "{bad}");
        let broken = Programs::parse("[[program]\n").unwrap_err();
        assert!(broken.starts_with("línea 1: "), "{broken}");
    }

    #[test]
    fn a_file_that_does_not_read_keeps_the_list_and_says_why_until_it_reads() {
        let mut p = programs(FILE);
        let before = p.list.clone();
        p.apply(Programs::parse("[[program]]\nname = \"b\"\n"));
        assert_eq!(p.list, before);
        assert_eq!(
            p.error.as_deref(),
            Some("línea 1: [[program]] sin «command»")
        );
        p.apply(Programs::parse(FILE));
        assert_eq!(p.error, None);
    }

    #[test]
    fn the_keys_move_pick_and_cancel() {
        let p = programs(FILE);
        let menu = Launcher::default();
        let (menu, _) = menu.press(KeyCode::Char('j'), &p);
        let (menu, _) = menu.unwrap().press(KeyCode::Down, &p);
        assert_eq!(menu, Some(Launcher { sel: 2 }));
        let (_, pick) = menu.unwrap().press(KeyCode::Enter, &p);
        assert_eq!(
            pick,
            Some(Pick::Program(vec![
                "claude".into(),
                "--model".into(),
                "opus".into()
            ]))
        );
        let (menu, _) = Launcher { sel: 3 }.press(KeyCode::Char('j'), &p);
        assert_eq!(menu, Some(Launcher { sel: 3 }), "stops at the last row");
        let (menu, _) = Launcher::default().press(KeyCode::Char('k'), &p);
        assert_eq!(menu, Some(Launcher::default()), "stops at the shell");
        assert_eq!(
            Launcher::default().press(KeyCode::Enter, &p),
            (None, Some(Pick::Shell))
        );
        assert_eq!(Launcher::default().press(KeyCode::Esc, &p), (None, None));
        assert_eq!(
            Launcher::default().press(KeyCode::Char('o'), &p).1,
            Some(Pick::Program(vec![
                "claude".into(),
                "--model".into(),
                "opus".into()
            ]))
        );
        assert_eq!(
            Launcher { sel: 2 }.press(KeyCode::Char('n'), &p),
            (None, Some(Pick::Shell)),
            "n is the shell, as outside the menu"
        );
        assert_eq!(
            Launcher { sel: 2 }.press(KeyCode::Char('z'), &p),
            (Some(Launcher { sel: 2 }), None),
            "a letter nobody has leaves the menu as it is"
        );
    }

    #[test]
    fn a_row_shorter_list_cannot_leave_the_menu_on_a_row_that_is_gone() {
        let p = programs(FILE);
        let (_, pick) = Launcher { sel: 9 }.press(KeyCode::Enter, &p);
        assert_eq!(
            pick,
            Some(Pick::Program(vec!["cargo".into(), "test".into()]))
        );
    }

    #[test]
    fn a_click_on_a_row_starts_it_outside_closes_and_inside_elsewhere_keeps_it() {
        let p = programs(FILE);
        let area = (29, 1, 91, 28);
        let (bx, by, ..) = frame(area, &p);
        let menu = Launcher::default();
        assert_eq!(
            menu.click(area, &p, bx + 10, by + 3 + 2,),
            (
                None,
                Some(Pick::Program(vec![
                    "claude".into(),
                    "--model".into(),
                    "opus".into()
                ]))
            )
        );
        assert_eq!(
            menu.click(area, &p, bx + 10, by + 3),
            (None, Some(Pick::Shell))
        );
        assert_eq!(menu.click(area, &p, bx + 10, by + 1), (Some(menu), None));
        assert_eq!(menu.click(area, &p, 0, 0), (None, None));
    }
}
