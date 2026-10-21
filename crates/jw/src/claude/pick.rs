//! `claude --resume` with no session id, or with a search term: the conversations of the folder to
//! choose from, in the pane's own terminal (spec REQ-547 and REQ-548). The engine serves a resume
//! of a session it is given; choosing one is what Claude's own screen did, and here it is done
//! before the conversation opens, so the conversation is jwn's and not Claude's.

use std::collections::BTreeMap;
use std::io::{Write, stdout};

use contract::conversation::SavedConversation;
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};

/// A session id: what `--resume` serves as it is.
fn is_session_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Take `-r` or `--resume` out of `argv` when it names no session: it was typed alone, or with a
/// search term. The result is the term (none when it was alone); `None` when there is nothing to
/// choose, because there is no such flag or it carries an id.
pub fn take_search(argv: &mut Vec<String>) -> Option<Option<String>> {
    let at = argv
        .iter()
        .position(|a| a == "-r" || a == "--resume" || a.starts_with("--resume="))?;
    if let Some(inline) = argv[at].strip_prefix("--resume=") {
        if is_session_id(inline) {
            return None;
        }
        let term = inline.to_owned();
        argv.remove(at);
        return Some(Some(term).filter(|t| !t.is_empty()));
    }
    match argv.get(at + 1).filter(|v| !v.starts_with('-')) {
        Some(v) if is_session_id(v) => None,
        Some(v) => {
            let term = v.clone();
            argv.drain(at..=at + 1);
            Some(Some(term))
        }
        None => {
            argv.remove(at);
            Some(None)
        }
    }
}

/// The conversations whose name has the term in it, whatever its case.
pub fn matching<'a>(
    all: &'a [SavedConversation],
    term: Option<&str>,
) -> Vec<&'a SavedConversation> {
    let term = term.map(str::to_lowercase);
    all.iter()
        .filter(|c| {
            term.as_deref()
                .is_none_or(|t| c.name.to_lowercase().contains(t))
        })
        .collect()
}

/// How long ago, in words that fit a row.
pub fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..=59 => "ahora".into(),
        60..=3599 => format!("hace {} min", s / 60),
        3600..=86_399 => format!("hace {} h", s / 3600),
        _ => format!("hace {} d", s / 86_400),
    }
}

/// The lines to draw: a heading, then the rows from the one the window starts at, `selected`
/// marked, and the keys. At most `height` lines.
pub fn lines(
    list: &[&SavedConversation],
    selected: usize,
    term: Option<&str>,
    height: usize,
    now_ms: u64,
    width: usize,
) -> Vec<String> {
    let heading = match term {
        Some(t) => format!("conversaciones de esta carpeta con «{t}»"),
        None => "conversaciones de esta carpeta".to_owned(),
    };
    let room = height.saturating_sub(3).max(1);
    let first = selected.saturating_sub(room - 1);
    let mut out = vec![heading, String::new()];
    for (i, c) in list.iter().enumerate().skip(first).take(room) {
        let when = ago(now_ms, c.at);
        let name_room = width.saturating_sub(when.chars().count() + 6).max(8);
        let name: String = c.name.chars().take(name_room).collect();
        let pad = " ".repeat(name_room.saturating_sub(name.chars().count()));
        out.push(format!(
            "{} {name}{pad}  {when}",
            if i == selected { "▸" } else { " " }
        ));
    }
    out.push("↑↓ elegir · ↵ retomar · esc salir".to_owned());
    out.truncate(height.max(1));
    out
}

/// What a key does to the list.
#[derive(Debug, PartialEq, Eq)]
pub enum Move {
    To(usize),
    Choose,
    Leave,
}

pub fn key(code: KeyCode, ctrl: bool, selected: usize, len: usize) -> Move {
    let last = len.saturating_sub(1);
    match code {
        KeyCode::Down | KeyCode::Char('j') => Move::To((selected + 1).min(last)),
        KeyCode::Up | KeyCode::Char('k') => Move::To(selected.saturating_sub(1)),
        KeyCode::Home | KeyCode::Char('g') => Move::To(0),
        KeyCode::End | KeyCode::Char('G') => Move::To(last),
        KeyCode::Enter => Move::Choose,
        KeyCode::Esc | KeyCode::Char('q') => Move::Leave,
        KeyCode::Char('c') | KeyCode::Char('d') if ctrl => Move::Leave,
        _ => Move::To(selected),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Show the conversations of this folder and return the session the person chooses; none when they
/// leave, or when there are none to choose from.
pub fn choose(term: Option<&str>) -> Result<Option<String>, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let all = daemon::saved_conversations(&cwd, Some(&env));
    let list = matching(&all, term);
    if list.is_empty() {
        eprintln!(
            "{} claude: no hay conversaciones{} en esta carpeta",
            identity::get().name,
            term.map_or_else(String::new, |t| format!(" con «{t}»"))
        );
        return Ok(None);
    }
    let _raw = super::Raw::on().ok_or("the terminal cannot be used raw")?;
    let mut out = stdout();
    let mut selected = 0;
    loop {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let _ = execute!(out, MoveTo(0, 0), Clear(ClearType::All));
        for line in lines(
            &list,
            selected,
            term,
            usize::from(rows),
            now_ms(),
            usize::from(cols),
        ) {
            let _ = write!(out, "{line}\r\n");
        }
        let _ = out.flush();
        let Event::Key(k) = event::read().map_err(|e| e.to_string())? else {
            continue;
        };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        match key(
            k.code,
            k.modifiers.contains(KeyModifiers::CONTROL),
            selected,
            list.len(),
        ) {
            Move::To(at) => selected = at,
            Move::Choose => {
                let _ = execute!(out, MoveTo(0, 0), Clear(ClearType::All));
                return Ok(list.get(selected).map(|c| c.id.clone()));
            }
            Move::Leave => {
                let _ = execute!(out, MoveTo(0, 0), Clear(ClearType::All));
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "9c97ae9c-41ad-424b-a6a5-7e72a36cd0d5";

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    fn saved(id: &str, name: &str, at: u64) -> SavedConversation {
        SavedConversation {
            id: id.into(),
            name: name.into(),
            at,
        }
    }

    #[test]
    fn a_resume_with_no_session_or_with_a_search_term_is_a_choice_and_one_with_an_id_is_not() {
        let mut a = argv(&["--resume"]);
        assert_eq!(take_search(&mut a), Some(None));
        assert!(
            a.is_empty(),
            "the flag is taken out: the choice puts the session in"
        );
        let mut a = argv(&["-r", "--model", "opus"]);
        assert_eq!(
            take_search(&mut a),
            Some(None),
            "a flag after it is not its value"
        );
        assert_eq!(a, ["--model", "opus"]);
        let mut a = argv(&["--model", "opus", "--resume", "login"]);
        assert_eq!(take_search(&mut a), Some(Some("login".into())));
        assert_eq!(a, ["--model", "opus"]);
        let mut a = argv(&["--resume=login"]);
        assert_eq!(take_search(&mut a), Some(Some("login".into())));
        for with_id in [
            argv(&["--resume", ID]),
            argv(&["-r", ID, "--model", "x"]),
            argv(&[&format!("--resume={ID}")]),
        ] {
            let mut a = with_id.clone();
            assert_eq!(take_search(&mut a), None, "{with_id:?}");
            assert_eq!(a, with_id, "an id is left for the engine to serve");
        }
        let mut none = argv(&["--model", "opus", "hola"]);
        assert_eq!(take_search(&mut none), None);
    }

    #[test]
    fn the_search_finds_names_with_the_term_in_them_whatever_their_case() {
        let all = [
            saved("1", "Arregla el Login", 5),
            saved("2", "otra cosa", 4),
        ];
        assert_eq!(matching(&all, None).len(), 2);
        assert_eq!(matching(&all, Some("login"))[0].id, "1");
        assert!(matching(&all, Some("nada")).is_empty());
    }

    #[test]
    fn the_rows_say_how_long_ago_and_mark_the_chosen_one_and_the_window_follows_it() {
        assert_eq!(ago(10_000_000, 10_000_000), "ahora");
        assert_eq!(ago(10_000_000, 10_000_000 - 5 * 60_000), "hace 5 min");
        assert_eq!(ago(100_000_000, 100_000_000 - 3 * 3_600_000), "hace 3 h");
        assert_eq!(ago(900_000_000, 900_000_000 - 2 * 86_400_000), "hace 2 d");
        let all: Vec<SavedConversation> = (0..10)
            .map(|i| saved(&i.to_string(), &format!("c{i}"), 0))
            .collect();
        let list: Vec<&SavedConversation> = all.iter().collect();
        let l = lines(&list, 0, None, 8, 60_000, 40);
        assert_eq!(l.len(), 8);
        assert!(
            l[2].starts_with("▸ c0") && l[2].ends_with("hace 1 min"),
            "{l:?}"
        );
        let l = lines(&list, 9, Some("c"), 8, 60_000, 40);
        assert!(l[0].contains("«c»"));
        assert!(
            l.iter().any(|r| r.starts_with("▸ c9")),
            "the window scrolled to the chosen one: {l:?}"
        );
        assert!(
            l.iter()
                .all(|r| !r.starts_with("▸ c0") && !r.contains(" c0 "))
        );
    }

    #[test]
    fn the_keys_move_choose_and_leave() {
        assert_eq!(key(KeyCode::Down, false, 0, 3), Move::To(1));
        assert_eq!(
            key(KeyCode::Down, false, 2, 3),
            Move::To(2),
            "not past the last"
        );
        assert_eq!(key(KeyCode::Up, false, 0, 3), Move::To(0));
        assert_eq!(key(KeyCode::Char('j'), false, 1, 3), Move::To(2));
        assert_eq!(key(KeyCode::End, false, 0, 3), Move::To(2));
        assert_eq!(key(KeyCode::Enter, false, 1, 3), Move::Choose);
        assert_eq!(key(KeyCode::Esc, false, 1, 3), Move::Leave);
        assert_eq!(key(KeyCode::Char('c'), true, 1, 3), Move::Leave);
        assert_eq!(key(KeyCode::Char('x'), false, 1, 3), Move::To(1));
    }
}
