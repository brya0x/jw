//! Syntax highlighting for the viewers (diff, Markdown code blocks), with
//! syntect's bundled grammars and one dark theme. Foreground colours only:
//! the caller paints backgrounds (added/removed lines, code blocks).

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Sits well on the petrol background (#10232A).
const THEME: &str = "base16-ocean.dark";

/// Loading the bundled grammars takes tens of milliseconds: do it once.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME_: OnceLock<Theme> = OnceLock::new();
    THEME_.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        set.themes.remove(THEME).expect("bundled theme")
    })
}

/// The grammar for a language token ("rs", "ts", "go") or a file path
/// ("src/a.ts"), plain text when none fits. The bundled set has no
/// TypeScript, so TS files borrow JavaScript's grammar.
fn syntax_for(lang_or_path: &str) -> &'static SyntaxReference {
    let set = syntaxes();
    let token = lang_or_path
        .rsplit('/')
        .next()
        .unwrap_or(lang_or_path)
        .rsplit('.')
        .next()
        .unwrap_or(lang_or_path)
        .to_ascii_lowercase();
    let token = match token.as_str() {
        "ts" | "tsx" | "mts" | "cts" | "typescript" | "jsx" | "mjs" | "cjs" => "js",
        "jsonc" => "json",
        "sh" | "bash" | "zsh" | "shell" => "sh",
        "yml" => "yaml",
        "golang" => "go",
        "rust" => "rs",
        "markdown" => "md",
        other => other,
    };
    set.find_syntax_by_token(token)
        .or_else(|| set.find_syntax_by_extension(token))
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// Highlights consecutive lines of one file or block, keeping the parser's
/// state between them so multi-line constructs (block comments, strings)
/// colour correctly.
pub struct Highlighter {
    inner: HighlightLines<'static>,
}

impl Highlighter {
    pub fn new(lang_or_path: &str) -> Self {
        Self {
            inner: HighlightLines::new(syntax_for(lang_or_path), theme()),
        }
    }

    /// One line, without its newline.
    pub fn line(&mut self, line: &str) -> Vec<Span<'static>> {
        // The grammars expect newline-terminated lines (line comments end
        // at the newline); it is dropped from the output.
        let with_nl = format!("{line}\n");
        match self.inner.highlight_line(&with_nl, syntaxes()) {
            Ok(parts) => parts
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.strip_suffix('\n').unwrap_or(text);
                    (!text.is_empty()).then(|| Span::styled(text.to_string(), convert(style)))
                })
                .collect(),
            Err(_) => vec![Span::raw(line.to_string())],
        }
    }
}

/// One line on its own, for when there is no surrounding context.
pub fn highlight_line(line: &str, lang_or_path: &str) -> Vec<Span<'static>> {
    Highlighter::new(lang_or_path).line(line)
}

fn convert(s: syntect::highlighting::Style) -> Style {
    let fg = s.foreground;
    let mut style = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colours(spans: &[Span]) -> usize {
        let mut fgs: Vec<_> = spans.iter().filter_map(|s| s.style.fg).collect();
        fgs.dedup();
        fgs.len()
    }

    #[test]
    fn known_languages_get_colours() {
        for (line, lang) in [
            ("fn main() { let x = 1; }", "rs"),
            ("const x: number = 1; // n", "src/a.ts"),
            ("func main() { return }", "go"),
            ("echo \"hi\" | grep h", "sh"),
        ] {
            let spans = highlight_line(line, lang);
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text, line, "{lang}: text is kept");
            assert!(colours(&spans) > 1, "{lang}: {spans:?}");
            assert!(spans.iter().all(|s| s.style.bg.is_none()), "fg only");
        }
    }

    #[test]
    fn unknown_language_is_plain_text() {
        let spans = highlight_line("whatever = 1", "nope");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "whatever = 1");
    }

    #[test]
    fn state_carries_across_lines() {
        let mut h = Highlighter::new("rs");
        let open = h.line("/* a block");
        let inside = h.line("still comment */ let x = 1;");
        let fresh = highlight_line("still comment */ let x = 1;", "rs");
        // Inside the comment the first word is coloured like the comment
        // opener, unlike the same line highlighted on its own.
        assert_eq!(inside[0].style.fg, open[0].style.fg);
        assert_ne!(inside[0].style.fg, fresh[0].style.fg);
    }
}
