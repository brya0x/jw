//! Syntax colour for the file viewer (spec REQ-621 to REQ-624, RAT-67 to RAT-69): the lines of a
//! file as runs of text with the class of the token they are, from a real grammar (`syntect`
//! with the grammars of `two-face`, pure Rust). The clients only map a class to an accent.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use contract::text::Run;
use two_face::re_exports::syntect::parsing::{
    ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet,
};

/// How much of a file is coloured: the parse runs at about 0.5 MB/s.
const COLOUR_BYTES: usize = 64 * 1024;
/// How long the colouring may take after its first line; it is looked at between lines.
const COLOUR_TIME: Duration = Duration::from_millis(300);
/// The most characters a line keeps; the rest is `…`. A longer line is never coloured.
const MAX_LINE: usize = 2000;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// The class a scope of the grammar becomes, most specific first: a token takes the class of the
/// innermost of its scopes that has one.
static CLASSES: LazyLock<Vec<(Scope, &'static str)>> = LazyLock::new(|| {
    [
        ("comment", "cm"),
        ("string", "str"),
        ("constant.numeric", "num"),
        ("constant.language", "num"),
        ("constant.character", "num"),
        ("entity.name.function", "fn"),
        ("support.function", "fn"),
        ("variable.function", "fn"),
        ("entity.name.type", "ty"),
        ("entity.name.class", "ty"),
        ("entity.name.struct", "ty"),
        ("entity.name.enum", "ty"),
        ("entity.name.trait", "ty"),
        ("entity.name.interface", "ty"),
        ("entity.other.inherited-class", "ty"),
        ("support.type", "ty"),
        ("support.class", "ty"),
        ("entity.name.tag", "kw"),
        ("entity.other.attribute-name", "attr"),
        ("meta.annotation", "attr"),
        ("variable.annotation", "attr"),
        ("markup.heading", "fn"),
        ("markup.raw", "str"),
        ("markup.underline.link", "attr"),
        ("keyword", "kw"),
        ("storage", "kw"),
    ]
    .into_iter()
    .map(|(scope, class)| (Scope::new(scope).expect("a scope"), class))
    .collect()
});

/// The language of a file named `name` whose text begins with `first`: by its extension, by its
/// whole name (`Makefile`) or by its first line (`#!`), none for plain text.
fn syntax_of(name: &str, first: &str) -> Option<&'static SyntaxReference> {
    let set: &'static SyntaxSet = &SYNTAXES;
    let ext = name.rsplit_once('.').map(|(_, e)| e);
    let found = ext
        .and_then(|e| set.find_syntax_by_extension(&e.to_lowercase()))
        .or_else(|| set.find_syntax_by_extension(name))
        .or_else(|| set.find_syntax_by_first_line(first));
    found.filter(|s| s.name != "Plain Text")
}

/// The class of the innermost scope of `stack` that has one.
fn class_of(stack: &ScopeStack) -> Option<&'static str> {
    stack.as_slice().iter().rev().find_map(|scope| {
        CLASSES
            .iter()
            .find(|(prefix, _)| prefix.is_prefix_of(*scope))
            .map(|(_, class)| *class)
    })
}

/// Colours one line after another, keeping the grammar's state between them.
struct Painter {
    parse: ParseState,
    stack: ScopeStack,
}

impl Painter {
    fn new(syntax: &SyntaxReference) -> Self {
        Painter {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
        }
    }

    /// The runs of `line`, which ends with its `\n`; none when the grammar cannot read it.
    fn line(&mut self, line: &str) -> Option<Vec<Run>> {
        let ops = self.parse.parse_line(line, &SYNTAXES).ok()?;
        let end = line.trim_end_matches(['\n', '\r']).len();
        let mut runs: Vec<Run> = Vec::new();
        let mut at = 0;
        for (pos, op) in ops {
            let pos = pos.min(end);
            if pos > at {
                push(&mut runs, &line[at..pos], class_of(&self.stack));
                at = pos;
            }
            self.stack.apply(&op).ok()?;
        }
        if end > at {
            push(&mut runs, &line[at..end], class_of(&self.stack));
        }
        Some(runs)
    }
}

/// Add `text` of `class` to `runs`, joined to the last run when it has the same class.
fn push(runs: &mut Vec<Run>, text: &str, class: Option<&'static str>) {
    match runs.last_mut() {
        Some(last) if last.k.as_deref() == class => last.t.push_str(text),
        _ => runs.push(Run {
            t: text.to_owned(),
            k: class.map(str::to_owned),
        }),
    }
}

/// The lines of `text`, a file called `name`, as runs: the head coloured when the language is
/// known (REQ-621, REQ-623), the rest plain; no `\n` or `\r\n`, a line cut after `MAX_LINE`
/// characters (REQ-614).
pub fn runs(text: &str, name: &str) -> Vec<Vec<Run>> {
    let first = text.lines().next().unwrap_or("");
    let mut painter = syntax_of(name, first).map(Painter::new);
    // The first line of a language compiles its grammar's regexes (up to 480 ms, once), so the
    // clock of `COLOUR_TIME` starts after it.
    let mut started: Option<Instant> = None;
    let mut seen = 0;
    text.split_inclusive('\n')
        .map(|line| {
            seen += line.len();
            let plain = line.trim_end_matches(['\n', '\r']);
            let long = plain.chars().nth(MAX_LINE).is_some();
            if long || seen > COLOUR_BYTES || started.is_some_and(|t| t.elapsed() > COLOUR_TIME) {
                painter = None;
            }
            let painted = painter.as_mut().and_then(|p| p.line(line));
            if painted.is_none() {
                painter = None;
            }
            started.get_or_insert_with(Instant::now);
            painted.unwrap_or_else(|| plain_runs(plain))
        })
        .collect()
}

fn plain_runs(line: &str) -> Vec<Run> {
    if line.is_empty() {
        return Vec::new();
    }
    let t = match line.char_indices().nth(MAX_LINE) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    };
    vec![Run { t, k: None }]
}

/// Build the grammars and compile the regexes of the common languages, so the first file of
/// each does not pay for them (RISK-67). Runs once, in a thread of its own.
pub fn warm() {
    std::thread::spawn(|| {
        for (name, code) in [
            ("a.rs", "fn main() { let x = \"a\"; }\n"),
            ("a.ts", "const x: number = 1;\n"),
            ("a.tsx", "const a = <b c=\"d\" />;\n"),
            ("a.js", "const x = () => 1;\n"),
            ("a.py", "def f(x):\n    return x\n"),
            ("a.go", "package main\nfunc main() {}\n"),
            ("a.java", "class A { int x; }\n"),
            ("a.kt", "fun main() {}\n"),
            ("a.swift", "let x = 1\n"),
            ("a.c", "int main() { return 0; }\n"),
            ("a.cpp", "int main() { return 0; }\n"),
            ("a.sh", "echo \"a\"\n"),
            ("a.json", "{\"a\": 1}\n"),
            ("a.toml", "a = 1\n"),
            ("a.yaml", "a: 1\n"),
            ("a.html", "<a href=\"b\">c</a>\n"),
            ("a.css", "a { color: red; }\n"),
            ("a.md", "# a\n"),
        ] {
            let _ = runs(code, name);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(runs: &[Run]) -> Vec<(&str, &str)> {
        runs.iter()
            .filter_map(|r| r.k.as_deref().map(|k| (r.t.as_str(), k)))
            .collect()
    }

    fn text_of(lines: &[Vec<Run>]) -> String {
        lines
            .iter()
            .map(|l| l.iter().map(|r| r.t.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// REQ-621: a Rust file comes back with its keywords, strings, numbers, comments and types.
    #[test]
    fn rust_is_coloured_by_its_grammar_and_keeps_its_text() {
        let src = "// a note\nfn main() {\r\n    let s: String = \"hola\"; let n = 42;\n}\n";
        let lines = runs(src, "main.rs");
        assert_eq!(
            text_of(&lines),
            "// a note\nfn main() {\n    let s: String = \"hola\"; let n = 42;\n}"
        );
        assert_eq!(classes(&lines[0]), [("// a note", "cm")]);
        let c = classes(&lines[1]);
        assert!(
            c.contains(&("fn", "kw")) && c.contains(&("main", "fn")),
            "{c:?}"
        );
        let c = classes(&lines[2]);
        assert!(c.contains(&("let", "kw")), "{c:?}");
        assert!(c.contains(&("String", "ty")), "{c:?}");
        assert!(
            c.iter().any(|(t, k)| t.contains("hola") && *k == "str"),
            "{c:?}"
        );
        assert!(c.contains(&("42", "num")), "{c:?}");
    }

    /// REQ-621: TypeScript and Markdown (REQ-624), and a file known by its name or its `#!` line.
    #[test]
    fn typescript_markdown_a_name_and_a_shebang_are_known() {
        let ts = runs("const x: number = 1; // uno\n", "a.ts");
        let c = classes(&ts[0]);
        assert!(
            c.contains(&("const", "kw")) && c.contains(&("number", "ty")),
            "{c:?}"
        );
        assert!(
            c.contains(&("1", "num")) && c.iter().any(|(_, k)| *k == "cm"),
            "{c:?}"
        );
        let md = runs("# Título\n\ntexto `código`\n", "NOTES.md");
        assert!(
            classes(&md[0]).iter().any(|(_, k)| *k == "fn"),
            "a heading is blue: {:?}",
            md[0]
        );
        assert!(
            classes(&md[2]).iter().any(|(_, k)| *k == "str"),
            "{:?}",
            md[2]
        );
        let make = runs("all:\n\techo \"a\"\n", "Makefile");
        assert!(
            make.iter().any(|l| !classes(l).is_empty()),
            "Makefile by its name"
        );
        let sh = runs("#!/bin/sh\nif true; then echo \"a\"; fi\n", "run");
        assert!(
            sh.iter().any(|l| !classes(l).is_empty()),
            "a script by its first line"
        );
    }

    /// REQ-621: a name with an unknown extension, and plain text, are sent plain.
    #[test]
    fn an_unknown_language_and_plain_text_are_plain() {
        for name in ["notes.zzz", "readme.txt", "noext"] {
            let lines = runs("let x = \"a\";\nfn b() {}\n", name);
            assert!(lines.iter().flatten().all(|r| r.k.is_none()), "{name}");
            assert_eq!(text_of(&lines), "let x = \"a\";\nfn b() {}");
        }
    }

    /// REQ-623: the head is coloured and the lines past 64 KiB are plain; a line too long to be
    /// kept stops the colour and is cut (REQ-614); an empty line has no runs.
    #[test]
    fn the_head_is_coloured_and_the_rest_is_plain() {
        let line = "let value = 12345; // x\n";
        let src = line.repeat(COLOUR_BYTES / line.len() + 2000);
        let lines = runs(&src, "a.rs");
        assert!(!classes(&lines[0]).is_empty(), "the head is coloured");
        let last = lines.last().unwrap();
        assert!(classes(last).is_empty(), "the tail is plain");
        assert_eq!(text_of(&lines[lines.len() - 1..]), line.trim_end());

        let long = format!(
            "let a = 1;\n{}\nlet b = 2;\n\nfn c() {{}}\n",
            "x".repeat(5000)
        );
        let lines = runs(&long, "a.rs");
        assert!(!classes(&lines[0]).is_empty());
        assert_eq!(lines[1].len(), 1);
        assert!(lines[1][0].t.ends_with('…') && lines[1][0].t.chars().count() == MAX_LINE + 1);
        assert!(
            classes(&lines[2]).is_empty(),
            "after a long line the colour stopped"
        );
        assert!(lines[3].is_empty());
    }

    /// RISK-70: a single 200 KiB line and a minified file come back at once, plain.
    #[test]
    fn a_huge_line_does_not_make_the_file_wait() {
        let one = "a".repeat(200 * 1024);
        let start = Instant::now();
        let lines = runs(&one, "bundle.min.js");
        assert!(
            start.elapsed() < Duration::from_millis(1500),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0][0].k.is_none());
    }

    /// RAT-68: the common languages of a person who reads code are in the pack.
    #[test]
    fn the_common_languages_have_a_grammar() {
        let cases = [
            ("a.rs", "fn a() { let x = 1; }"),
            ("a.ts", "const x = 1;"),
            ("a.tsx", "const x = 1;"),
            ("a.js", "const x = 1;"),
            ("a.kt", "fun a() { val x = 1 }"),
            ("a.py", "def a():\n    return 1"),
            ("a.go", "func a() { x := 1 }"),
            ("a.java", "class A { int x = 1; }"),
            ("a.swift", "let x = 1"),
            ("a.c", "int x = 1;"),
            ("a.cpp", "int x = 1;"),
            ("a.cs", "class A { int x = 1; }"),
            ("a.sh", "if true; then echo \"a\"; fi"),
            ("a.json", "{\"a\": 1}"),
            ("a.toml", "a = \"b\""),
            ("a.yaml", "a: \"b\""),
            ("a.html", "<a href=\"b\">c</a>"),
            ("a.css", "a { color: red; }"),
            ("a.sql", "select 1 from a"),
            ("a.md", "# a"),
            ("a.rb", "def a; 1; end"),
            ("a.php", "<?php echo \"a\"; ?>"),
            ("a.lua", "local x = 1"),
            ("Dockerfile", "FROM a\nRUN b"),
            ("Makefile", "all:\n\techo a"),
        ];
        let plain: Vec<&str> = cases
            .iter()
            .filter(|(name, code)| runs(code, name).iter().flatten().all(|r| r.k.is_none()))
            .map(|(name, _)| *name)
            .collect();
        assert!(plain.is_empty(), "no colour for: {plain:?}");
    }

    /// REQ-622: every class the daemon sends is one the clients map.
    #[test]
    fn the_classes_are_the_closed_set() {
        let known = ["kw", "str", "num", "ty", "fn", "cm", "attr"];
        assert!(CLASSES.iter().all(|(_, k)| known.contains(k)));
        for k in known {
            assert!(CLASSES.iter().any(|(_, c)| *c == k), "{k} has a scope");
        }
    }
}
