//! Turning crossterm key events back into the bytes a terminal would send,
//! for the pane that has focus (REQ-9), and parsing the leader key.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The key that leaves terminal mode, from `[tui] leader` ("C-Space", "C-g").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leader(char);

impl Leader {
    pub const DEFAULT: Self = Self(' ');

    pub fn parse(s: &str) -> Option<Self> {
        let key = s.strip_prefix("C-").or_else(|| s.strip_prefix("Ctrl-"))?;
        match key.to_ascii_lowercase().as_str() {
            "space" => Some(Self(' ')),
            k if k.len() == 1 && k.as_bytes()[0].is_ascii_lowercase() => {
                Some(Self(k.as_bytes()[0] as char))
            }
            _ => None,
        }
    }

    pub fn matches(self, k: &KeyEvent) -> bool {
        // Terminals send Ctrl-Space as NUL, which crossterm reports as
        // Ctrl+' ' or, on some, Ctrl+'@'.
        let c = match k.code {
            KeyCode::Char('@') if self.0 == ' ' => ' ',
            KeyCode::Char(c) => c.to_ascii_lowercase(),
            _ => return false,
        };
        k.modifiers.contains(KeyModifiers::CONTROL) && c == self.0
    }

    pub fn label(self) -> String {
        match self.0 {
            ' ' => "^␣".into(),
            c => format!("^{c}"),
        }
    }
}

/// The bytes for `k`. `app_cursor` is DECCKM: arrows send `ESC O x` instead
/// of `ESC [ x` when the app asked for it (vim, less).
pub fn encode(k: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let m = k.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    // xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let param =
        1 + u8::from(m.contains(KeyModifiers::SHIFT)) + 2 * u8::from(alt) + 4 * u8::from(ctrl);

    let csi = |final_: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{final_}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{n};{param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let mut out = match k.code {
        KeyCode::Char(c) if ctrl => match ctrl_byte(c) {
            Some(b) => vec![b],
            None => c.to_string().into_bytes(),
        },
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![if ctrl { 0x08 } else { 0x7f }],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let c = (b'P' + n - 1) as char;
            if param > 1 {
                format!("\x1b[1;{param}{c}").into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => Vec::new(),
    };
    // Alt prefixes ESC to plain keys; the CSI keys already carry it in `param`.
    if alt
        && matches!(
            k.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        out.insert(0, 0x1b);
    }
    out
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn plain_and_control_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(&key(KeyCode::Char('a'), none), false), b"a");
        assert_eq!(
            encode(&key(KeyCode::Char('ñ'), none), false),
            "ñ".as_bytes()
        );
        assert_eq!(
            encode(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            [3]
        );
        assert_eq!(
            encode(&key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(encode(&key(KeyCode::Enter, none), false), b"\r");
        assert_eq!(encode(&key(KeyCode::Backspace, none), false), [0x7f]);
    }

    #[test]
    fn arrows_follow_the_cursor_mode_and_modifiers() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(&key(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(encode(&key(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(
            encode(&key(KeyCode::Left, KeyModifiers::CONTROL), true),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode(&key(KeyCode::Delete, KeyModifiers::SHIFT), false),
            b"\x1b[3;2~"
        );
        assert_eq!(encode(&key(KeyCode::F(1), none), false), b"\x1bOP");
        assert_eq!(encode(&key(KeyCode::F(12), none), false), b"\x1b[24~");
    }

    #[test]
    fn leader() {
        let l = Leader::DEFAULT;
        assert!(l.matches(&key(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(l.matches(&key(KeyCode::Char('@'), KeyModifiers::CONTROL)));
        assert!(!l.matches(&key(KeyCode::Char(' '), KeyModifiers::NONE)));
        let g = Leader::parse("C-g").unwrap();
        assert!(g.matches(&key(KeyCode::Char('g'), KeyModifiers::CONTROL)));
        assert_eq!(Leader::parse("C-Space"), Some(Leader::DEFAULT));
        assert_eq!(Leader::parse("space"), None);
    }
}
