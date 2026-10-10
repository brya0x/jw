//! Turning crossterm key events back into the bytes a terminal would send,
//! for the pane that has focus (REQ-9), and parsing the leader key.

use crate::kitty::Kitty;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

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

/// What a pane's parser tracks beyond its screen: its program's kitty
/// keyboard flags (REQ-92). Only the daemon answers queries.
#[derive(Default)]
pub struct Flags(pub Kitty);

impl vt100::Callbacks for Flags {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        self.0.csi(screen.alternate_screen(), i1, params, c);
    }
}

/// The bytes for `k`. `app_cursor` is DECCKM: arrows send `ESC O x` instead
/// of `ESC [ x` when the app asked for it (vim, less). `kitty` is the pane's
/// keyboard flags: with disambiguate (1), modified Enter, Tab and Backspace,
/// and Esc, go as `CSI code;mods u` (REQ-95).
pub fn encode(k: &KeyEvent, app_cursor: bool, kitty: u8) -> Vec<u8> {
    let m = k.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    // xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let param =
        1 + u8::from(m.contains(KeyModifiers::SHIFT)) + 2 * u8::from(alt) + 4 * u8::from(ctrl);

    if kitty & 1 != 0 {
        let code = match k.code {
            KeyCode::Esc => Some(27),
            KeyCode::Enter if param > 1 => Some(13),
            KeyCode::Tab if param > 1 => Some(9),
            KeyCode::Backspace if param > 1 => Some(127),
            _ => None,
        };
        if let Some(code) = code {
            return if param > 1 {
                format!("\x1b[{code};{param}u")
            } else {
                format!("\x1b[{code}u")
            }
            .into_bytes();
        }
    }

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

/// The bytes a terminal sends for a mouse event at `(col, row)` (1-based,
/// inside the pane), for an app that asked for the mouse; `None` when its
/// mode doesn't report this kind of event or the place can't be encoded.
pub fn encode_mouse(
    m: &MouseEvent,
    (col, row): (u16, u16),
    mode: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode as Mode;
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, release) = match m.kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) if mode != Mode::Press => (button(b), true),
        MouseEventKind::Drag(b) if matches!(mode, Mode::ButtonMotion | Mode::AnyMotion) => {
            (button(b) + 32, false)
        }
        MouseEventKind::Moved if mode == Mode::AnyMotion => (3 + 32, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        _ => return None,
    };
    let mods = m.modifiers;
    let code = code
        + 4 * u16::from(mods.contains(KeyModifiers::SHIFT))
        + 8 * u16::from(mods.contains(KeyModifiers::ALT))
        + 16 * u16::from(mods.contains(KeyModifiers::CONTROL));
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{code};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        ),
        other => {
            // The X10 forms can't say which button went up.
            let code = if release { 3 } else { code };
            let mut out = b"\x1b[M".to_vec();
            out.push(u8::try_from(32 + code).ok()?);
            for v in [col, row] {
                let v = u32::from(v) + 32;
                if other == vt100::MouseProtocolEncoding::Utf8 {
                    let mut b = [0; 4];
                    out.extend_from_slice(char::from_u32(v)?.encode_utf8(&mut b).as_bytes());
                } else {
                    out.push(u8::try_from(v).ok()?);
                }
            }
            Some(out)
        }
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
        assert_eq!(encode(&key(KeyCode::Char('a'), none), false, 0), b"a");
        assert_eq!(
            encode(&key(KeyCode::Char('ñ'), none), false, 0),
            "ñ".as_bytes()
        );
        assert_eq!(
            encode(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false, 0),
            [3]
        );
        assert_eq!(
            encode(&key(KeyCode::Char('x'), KeyModifiers::ALT), false, 0),
            b"\x1bx"
        );
        assert_eq!(encode(&key(KeyCode::Enter, none), false, 0), b"\r");
        assert_eq!(encode(&key(KeyCode::Backspace, none), false, 0), [0x7f]);
    }

    #[test]
    fn arrows_follow_the_cursor_mode_and_modifiers() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(&key(KeyCode::Up, none), false, 0), b"\x1b[A");
        assert_eq!(encode(&key(KeyCode::Up, none), true, 0), b"\x1bOA");
        assert_eq!(
            encode(&key(KeyCode::Left, KeyModifiers::CONTROL), true, 0),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode(&key(KeyCode::Delete, KeyModifiers::SHIFT), false, 0),
            b"\x1b[3;2~"
        );
        assert_eq!(encode(&key(KeyCode::F(1), none), false, 0), b"\x1bOP");
        assert_eq!(encode(&key(KeyCode::F(12), none), false, 0), b"\x1b[24~");
    }

    #[test]
    fn modified_enter_needs_the_kitty_flags() {
        let shift = KeyModifiers::SHIFT;
        let none = KeyModifiers::NONE;
        // REQ-96: without them, what a terminal sends.
        assert_eq!(encode(&key(KeyCode::Enter, shift), false, 0), b"\r");
        assert_eq!(encode(&key(KeyCode::Esc, none), false, 0), [0x1b]);
        // REQ-95: with disambiguate.
        assert_eq!(encode(&key(KeyCode::Enter, shift), false, 1), b"\x1b[13;2u");
        assert_eq!(
            encode(&key(KeyCode::Enter, KeyModifiers::CONTROL), false, 5),
            b"\x1b[13;5u"
        );
        assert_eq!(encode(&key(KeyCode::Enter, none), false, 1), b"\r");
        assert_eq!(
            encode(&key(KeyCode::Tab, KeyModifiers::CONTROL), false, 1),
            b"\x1b[9;5u"
        );
        assert_eq!(encode(&key(KeyCode::BackTab, shift), false, 1), b"\x1b[Z");
        assert_eq!(
            encode(&key(KeyCode::Backspace, KeyModifiers::ALT), false, 1),
            b"\x1b[127;3u"
        );
        assert_eq!(encode(&key(KeyCode::Esc, none), false, 1), b"\x1b[27u");
        assert_eq!(encode(&key(KeyCode::Char('a'), none), false, 1), b"a");
        // Flags without disambiguate change nothing.
        assert_eq!(encode(&key(KeyCode::Enter, shift), false, 8), b"\r");
    }

    #[test]
    fn pane_parser_tracks_the_flags() {
        let mut p = vt100::Parser::new_with_callbacks(4, 10, 0, Flags::default());
        p.process(b"\x1b[>1u");
        assert_eq!(p.callbacks().0.flags(false), 1);
        p.process(b"\x1b[?1049h\x1b[>5u");
        assert_eq!(p.callbacks().0.flags(true), 5);
        p.process(b"\x1b[?1049l\x1b[<u");
        assert_eq!(p.callbacks().0.flags(false), 0);
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

    #[test]
    fn mouse_encodings() {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use vt100::{MouseProtocolEncoding as E, MouseProtocolMode as M};
        let ev = |kind| MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let down = ev(MouseEventKind::Down(MouseButton::Left));
        assert_eq!(
            encode_mouse(&down, (3, 4), M::PressRelease, E::Sgr).unwrap(),
            b"\x1b[<0;3;4M"
        );
        let up = ev(MouseEventKind::Up(MouseButton::Left));
        assert_eq!(
            encode_mouse(&up, (3, 4), M::PressRelease, E::Sgr).unwrap(),
            b"\x1b[<0;3;4m"
        );
        assert_eq!(
            encode_mouse(&up, (3, 4), M::PressRelease, E::Default).unwrap(),
            b"\x1b[M##$"
        );
        assert!(encode_mouse(&up, (3, 4), M::Press, E::Sgr).is_none());
        let drag = ev(MouseEventKind::Drag(MouseButton::Left));
        assert!(encode_mouse(&drag, (1, 1), M::PressRelease, E::Sgr).is_none());
        assert_eq!(
            encode_mouse(&drag, (1, 1), M::ButtonMotion, E::Sgr).unwrap(),
            b"\x1b[<32;1;1M"
        );
        let wheel = ev(MouseEventKind::ScrollDown);
        assert_eq!(
            encode_mouse(&wheel, (2, 2), M::Press, E::Sgr).unwrap(),
            b"\x1b[<65;2;2M"
        );
        assert!(encode_mouse(&down, (300, 1), M::Press, E::Default).is_none());
    }
}
