//! Erasing a word in a text field (REQ-670 to REQ-673): what the terminals send for
//! option-backspace and ctrl-backspace, said once as «backspace with alt», and the one rule of
//! what a word is, shared by the composer and the fields that are a `String`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The key as the rest of the client reads it: every encoding of «erase a word» becomes
/// backspace with alt only, which is what `esc DEL` is and what the composer and the shell
/// already take; anything else is left as it came.
///
/// The encodings (crossterm 0.29 decodes `esc DEL` as backspace+alt, `CSI 127;3u` as the same,
/// `esc BS` as `h`+ctrl+alt and `CSI 8;3u` as the character `\x08`+alt):
/// - backspace with alt, or with ctrl, or with both (`esc DEL`, `CSI 127;3u`, `127;5u`, `127;7u`);
/// - `h` with ctrl and alt (`esc BS`: ctrl-backspace is `BS`, and alt puts `esc` before it);
/// - the character `\x08` with alt or ctrl (`CSI 8;3u`, `8;5u`, `8;7u`); with no modifier it is
///   a plain backspace.
///
/// A plain ctrl-h is not touched: with the keyboard protocol it is `h` and ctrl, and it is not
/// a word.
pub fn canonical(ev: KeyEvent) -> KeyEvent {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let (code, mods) = match ev.code {
        KeyCode::Backspace if alt || ctrl => (KeyCode::Backspace, KeyModifiers::ALT),
        KeyCode::Char('h') if alt && ctrl => (KeyCode::Backspace, KeyModifiers::ALT),
        KeyCode::Char('\x08') if alt || ctrl => (KeyCode::Backspace, KeyModifiers::ALT),
        KeyCode::Char('\x08') => (KeyCode::Backspace, ev.modifiers),
        _ => return ev,
    };
    KeyEvent::new_with_kind(code, mods, ev.kind)
}

/// Is this (canonical) key «erase the word before»?
pub fn is_word(ev: &KeyEvent) -> bool {
    ev.code == KeyCode::Backspace && ev.modifiers.contains(KeyModifiers::ALT)
}

/// Where the word before `at` starts: spaces are skipped, then the word.
pub fn word_start(chars: &[char], mut at: usize) -> usize {
    while at > 0 && chars[at - 1].is_whitespace() {
        at -= 1;
    }
    while at > 0 && !chars[at - 1].is_whitespace() {
        at -= 1;
    }
    at
}

/// Take the word at the end of `text`, with the spaces after it.
pub fn pop_word(text: &mut String) {
    let chars: Vec<char> = text.chars().collect();
    let at = word_start(&chars, chars.len());
    text.truncate(text.char_indices().nth(at).map_or(text.len(), |(i, _)| i));
}

/// What backspace does at the end of `text`: the word when `ev` is a word's erase, else a
/// character.
pub fn back(text: &mut String, ev: &KeyEvent) {
    if is_word(ev) {
        pop_word(text);
    } else {
        text.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    const ALT: KeyModifiers = KeyModifiers::ALT;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    /// What each encoding a terminal may send for option-backspace and ctrl-backspace arrives as
    /// in crossterm 0.29 (read in its `parse.rs`: `esc DEL` and `CSI 127;3u` are backspace+alt,
    /// `esc BS` is `h`+ctrl+alt, `CSI 8;…u` is the character `\x08`), and that all are a word.
    #[test]
    fn every_encoding_of_option_and_ctrl_backspace_is_an_erase_of_a_word() {
        let word = [
            ("esc DEL (option-backspace)", KeyCode::Backspace, ALT),
            ("CSI 127;3u", KeyCode::Backspace, ALT),
            ("CSI 127;5u (ctrl-backspace)", KeyCode::Backspace, CTRL),
            ("CSI 127;7u", KeyCode::Backspace, ALT | CTRL),
            ("esc BS", KeyCode::Char('h'), ALT | CTRL),
            ("CSI 8;3u", KeyCode::Char('\x08'), ALT),
            ("CSI 8;5u", KeyCode::Char('\x08'), CTRL),
            ("CSI 8;7u", KeyCode::Char('\x08'), ALT | CTRL),
        ];
        for (name, code, mods) in word {
            let ev = canonical(key(code, mods));
            assert!(is_word(&ev), "{name}: {ev:?}");
            assert_eq!(ev.modifiers, ALT, "{name} is backspace with alt only");
        }
    }

    #[test]
    fn a_plain_backspace_and_ctrl_h_are_not_a_word() {
        for ev in [
            key(KeyCode::Backspace, KeyModifiers::NONE),
            key(KeyCode::Backspace, KeyModifiers::SHIFT),
            key(KeyCode::Char('h'), CTRL),
            key(KeyCode::Char('h'), ALT),
            key(KeyCode::Char('w'), CTRL),
        ] {
            assert!(!is_word(&canonical(ev)), "{ev:?}");
        }
        let plain = canonical(key(KeyCode::Char('\x08'), KeyModifiers::NONE));
        assert_eq!(plain.code, KeyCode::Backspace, "BS alone is backspace");
        assert!(!is_word(&plain));
        let other = key(KeyCode::Char('x'), ALT);
        assert_eq!(canonical(other), other, "any other key is left as it came");
    }

    #[test]
    fn a_word_is_what_spaces_bound_and_the_spaces_after_it_go_too() {
        let word = |s: &str| {
            let mut s = s.to_owned();
            pop_word(&mut s);
            s
        };
        assert_eq!(word("uno dos tres"), "uno dos ");
        assert_eq!(word("uno dos "), "uno ");
        assert_eq!(word("uno"), "");
        assert_eq!(word(""), "");
        assert_eq!(word("feat/login-form"), "", "a slash does not bound a word");
        assert_eq!(word("año ñandú"), "año ", "by characters, not bytes");
    }

    #[test]
    fn back_takes_a_character_or_a_word_by_the_key() {
        let mut s = "uno dos".to_owned();
        back(&mut s, &key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(s, "uno do");
        back(&mut s, &canonical(key(KeyCode::Backspace, CTRL)));
        assert_eq!(s, "uno ");
    }
}
