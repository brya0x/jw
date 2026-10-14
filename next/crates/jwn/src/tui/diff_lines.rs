//! The pure part of the diff viewer: a `diff` document as the lines its body draws, the
//! tree of its files, the words that changed inside a changed line, and syntax tones.
//! Nothing here draws or talks to the daemon, so it is tested on plain documents.

use std::cmp::Ordering;

use contract::{Diff, DiffFile, FileStatus, Hunk, Seen};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Add,
    Del,
    Ctx,
}

/// One line of a hunk, numbered from the hunk's starts.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub kind: Kind,
    pub text: String,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// The chars of `text` that differ from the line it replaced.
    pub words: Option<(usize, usize)>,
}

/// What the body of the viewer is made of. The index is the file's place in the tree order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Line {
    File(usize),
    Note(usize, String),
    Hunk(usize, String),
    One(usize, Row),
    Two(usize, Option<Row>, Option<Row>),
    Gap(usize),
}

impl Line {
    pub fn file(&self) -> usize {
        match self {
            Line::File(f)
            | Line::Note(f, _)
            | Line::Hunk(f, _)
            | Line::One(f, _)
            | Line::Two(f, _, _)
            | Line::Gap(f) => *f,
        }
    }
}

/// Files before the folders' files, folders and files by name, as a tree lists them.
pub fn tree_cmp(a: &str, b: &str) -> Ordering {
    let (x, y): (Vec<_>, Vec<_>) = (a.split('/').collect(), b.split('/').collect());
    for i in 0.. {
        if i >= x.len() || i >= y.len() {
            return x.len().cmp(&y.len());
        }
        let (dx, dy) = (i + 1 < x.len(), i + 1 < y.len());
        if dx != dy {
            return if dx {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if x[i] != y[i] {
            return x[i].cmp(y[i]);
        }
    }
    Ordering::Equal
}

/// The indices of `diff.files` in tree order.
pub fn order(diff: &Diff) -> Vec<usize> {
    let mut order: Vec<usize> = (0..diff.files.len()).collect();
    order.sort_by(|a, b| tree_cmp(&diff.files[*a].path, &diff.files[*b].path));
    order
}

/// The chars that differ between an old and a new line, as the part between their common
/// start and common end. `None` when they have too little in common for that to say anything.
pub fn word_ranges(old: &str, new: &str) -> Option<((usize, usize), (usize, usize))> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let shared = start + end;
    (shared * 10 >= a.len().min(b.len()) * 3)
        .then_some(((start, a.len() - end), (start, b.len() - end)))
}

/// Text that is safe to put on a terminal screen: a tab becomes spaces, and any other
/// control character (an escape in a changed file, say) becomes a dot, so a diff can never
/// send the terminal a command.
pub fn clean(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\t' => "    ".chars().collect::<Vec<_>>(),
            c if c.is_control() => vec!['·'],
            c => vec![c],
        })
        .collect()
}

fn rows_of(hunk: &Hunk) -> Vec<Row> {
    let (mut old, mut new) = (hunk.old_start, hunk.new_start);
    let mut rows: Vec<Row> = hunk
        .lines
        .iter()
        .map(|line| {
            let mut chars = line.chars();
            let (kind, text) = match chars.next() {
                Some('+') => (Kind::Add, chars.as_str()),
                Some('-') => (Kind::Del, chars.as_str()),
                _ => (Kind::Ctx, chars.as_str()),
            };
            let row = Row {
                kind,
                text: clean(text),
                old: (kind != Kind::Add).then_some(old),
                new: (kind != Kind::Del).then_some(new),
                words: None,
            };
            if kind != Kind::Add {
                old += 1;
            }
            if kind != Kind::Del {
                new += 1;
            }
            row
        })
        .collect();
    // A run of deleted lines followed by added ones: the i-th pairs with the i-th.
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != Kind::Del {
            i += 1;
            continue;
        }
        let dels = rows[i..].iter().take_while(|r| r.kind == Kind::Del).count();
        let adds = rows[i + dels..]
            .iter()
            .take_while(|r| r.kind == Kind::Add)
            .count();
        for k in 0..dels.min(adds) {
            if let Some((o, n)) = word_ranges(&rows[i + k].text, &rows[i + dels + k].text) {
                rows[i + k].words = Some(o);
                rows[i + dels + k].words = Some(n);
            }
        }
        i += dels + adds;
    }
    rows
}

/// Why a file shows no lines, in the words the viewer says it with.
fn why_unread(reason: &str) -> &str {
    match reason {
        "symlink" => "es un enlace simbólico",
        "too_big" => "es demasiado grande",
        "special" => "no es un archivo normal",
        "repository" => "es otro repositorio",
        "gone" => "ya no existe",
        other => other,
    }
}

/// A file with a single side to show (new or deleted) is drawn in one column even when the
/// body is wide.
fn one_sided(file: &DiffFile) -> bool {
    matches!(
        file.status,
        FileStatus::Added | FileStatus::Untracked | FileStatus::Deleted
    )
}

/// The body: every file of the diff in tree order, a seen file folded to its header.
pub fn build(diff: &Diff, order: &[usize], split: bool) -> Vec<Line> {
    let mut out = Vec::new();
    for (pos, &at) in order.iter().enumerate() {
        let file = &diff.files[at];
        out.push(Line::File(pos));
        if file.seen == Some(Seen::Yes) {
            out.push(Line::Gap(pos));
            continue;
        }
        if let Some(why) = &file.unreadable {
            out.push(Line::Note(
                pos,
                clean(&format!("{}: el contenido no se muestra", why_unread(why))),
            ));
        } else if file.binary {
            out.push(Line::Note(
                pos,
                "archivo binario: no hay líneas que mostrar".into(),
            ));
        } else if file.hunks.is_empty() && !diff.too_large {
            let note = if file.status == FileStatus::Renamed {
                "renombrado, sin cambios en el contenido"
            } else {
                "sin cambios en el contenido"
            };
            out.push(Line::Note(pos, note.into()));
        }
        let two = split && !one_sided(file);
        for hunk in &file.hunks {
            out.push(Line::Hunk(
                pos,
                clean(&format!(
                    "@@ -{},{} +{},{} @@ {}",
                    hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.context
                ))
                .trim_end()
                .to_owned(),
            ));
            let rows = rows_of(hunk);
            let mut i = 0;
            while i < rows.len() {
                if rows[i].kind == Kind::Ctx {
                    let row = rows[i].clone();
                    out.push(if two {
                        Line::Two(pos, Some(row.clone()), Some(row))
                    } else {
                        Line::One(pos, row)
                    });
                    i += 1;
                    continue;
                }
                let dels: Vec<&Row> = rows[i..]
                    .iter()
                    .take_while(|r| r.kind == Kind::Del)
                    .collect();
                let adds: Vec<&Row> = rows[i + dels.len()..]
                    .iter()
                    .take_while(|r| r.kind == Kind::Add)
                    .collect();
                if two {
                    for k in 0..dels.len().max(adds.len()) {
                        out.push(Line::Two(
                            pos,
                            dels.get(k).map(|r| (*r).clone()),
                            adds.get(k).map(|r| (*r).clone()),
                        ));
                    }
                } else {
                    out.extend(
                        dels.iter()
                            .chain(&adds)
                            .map(|r| Line::One(pos, (*r).clone())),
                    );
                }
                i += dels.len() + adds.len();
            }
        }
        out.push(Line::Gap(pos));
    }
    out
}

/// A row of the tree on the left.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeRow {
    Dir {
        /// `crates/web/` : the folder chain that this row stands for.
        key: String,
        label: String,
        depth: usize,
        added: u32,
        deleted: u32,
        shut: bool,
    },
    File {
        pos: usize,
        depth: usize,
    },
}

/// The tree of the files in `order`: a folder with a single folder in it is one row, and a
/// shut folder hides what is under it. `shut` says which folder keys are folded.
pub fn tree_rows(diff: &Diff, order: &[usize], shut: &dyn Fn(&str) -> bool) -> Vec<TreeRow> {
    let parts: Vec<Vec<&str>> = order
        .iter()
        .map(|&i| diff.files[i].path.split('/').collect())
        .collect();
    let mut out = Vec::new();
    walk(
        diff,
        order,
        &parts,
        shut,
        (0, order.len()),
        (0, 0, false),
        &mut out,
    );
    out
}

fn walk(
    diff: &Diff,
    order: &[usize],
    parts: &[Vec<&str>],
    shut: &dyn Fn(&str) -> bool,
    (lo, hi): (usize, usize),
    (base, depth, hidden): (usize, usize, bool),
    out: &mut Vec<TreeRow>,
) {
    let mut i = lo;
    while i < hi {
        if parts[i].len() == base + 1 {
            if !hidden {
                out.push(TreeRow::File { pos: i, depth });
            }
            i += 1;
            continue;
        }
        let mut end = i;
        while end < hi && parts[end].len() > base + 1 && parts[end][base] == parts[i][base] {
            end += 1;
        }
        let mut chain = 1;
        while parts[i..end]
            .iter()
            .all(|q| q.len() > base + chain + 1 && q[base + chain] == parts[i][base + chain])
        {
            chain += 1;
        }
        let key = format!("{}/", parts[i][..base + chain].join("/"));
        let folded = shut(&key);
        if !hidden {
            let files = order[i..end].iter().map(|&o| &diff.files[o]);
            let (added, deleted) = files.fold((0, 0), |(a, d), f| (a + f.added, d + f.deleted));
            out.push(TreeRow::Dir {
                key,
                label: parts[i][base..base + chain].join("/"),
                depth,
                added,
                deleted,
                shut: folded,
            });
        }
        walk(
            diff,
            order,
            parts,
            shut,
            (i, end),
            (base + chain, depth + 1, hidden || folded),
            out,
        );
        i = end;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Plain,
    Keyword,
    Text,
    Number,
    Comment,
}

const KEYWORDS: [&str; 22] = [
    "fn", "let", "mut", "pub", "use", "struct", "impl", "if", "else", "match", "return", "async",
    "await", "const", "enum", "mod", "Self", "self", "true", "false", "None", "Some",
];

/// A tone for each char of a line of code: keywords, strings, numbers and `//` comments.
/// A rough guess that reads the same in any language, not a parser.
pub fn tones(text: &str) -> Vec<Tone> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![Tone::Plain; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            out[i..].fill(Tone::Comment);
            break;
        }
        if c == '"' {
            let end = chars[i + 1..]
                .iter()
                .position(|x| *x == '"')
                .map_or(chars.len(), |p| i + 2 + p);
            out[i..end].fill(Tone::Text);
            i = end;
        } else if c.is_alphanumeric() || c == '_' {
            let end = i + chars[i..]
                .iter()
                .take_while(|x| x.is_alphanumeric() || **x == '_')
                .count();
            let word: String = chars[i..end].iter().collect();
            if KEYWORDS.contains(&word.as_str()) {
                out[i..end].fill(Tone::Keyword);
            } else if c.is_ascii_digit() && word.chars().all(|x| x.is_ascii_digit()) {
                out[i..end].fill(Tone::Number);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// How long ago a Unix time was, for a commit's row.
pub fn ago(now_secs: u64, then: u64) -> String {
    let secs = now_secs.saturating_sub(then);
    match secs {
        0..=59 => "ahora".into(),
        60..=3599 => format!("hace {} min", secs / 60),
        3600..=86399 => format!("hace {} h", secs / 3600),
        _ => format!("hace {} d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Diff {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contract/fixtures/daemon")
            .join(name);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        serde_json::from_value(doc["node"].clone()).unwrap()
    }

    #[test]
    fn folders_come_before_files_and_a_chain_of_folders_is_one_row() {
        let mut names = ["z.rs", "a/b.rs", "a/a.rs", "m/n/o.rs", "b.rs"];
        names.sort_by(|a, b| tree_cmp(a, b));
        assert_eq!(names, ["a/a.rs", "a/b.rs", "m/n/o.rs", "b.rs", "z.rs"]);

        let d = fixture("snapshot-diff.json");
        let rows = tree_rows(&d, &order(&d), &|_| false);
        let dirs: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                TreeRow::Dir { label, depth, .. } => Some((label.as_str(), *depth)),
                TreeRow::File { .. } => None,
            })
            .collect();
        assert_eq!(dirs, [("assets", 0), ("crates/web/src", 0)]);
    }

    #[test]
    fn a_shut_folder_hides_what_is_under_it_and_keeps_its_totals() {
        let d = fixture("snapshot-diff.json");
        let o = order(&d);
        let rows = tree_rows(&d, &o, &|k| k == "crates/web/src/");
        assert!(rows.iter().all(|r| !matches!(r, TreeRow::File { pos, .. } if d.files[o[*pos]].path.starts_with("crates"))));
        let shut = rows
            .iter()
            .find_map(|r| match r {
                TreeRow::Dir {
                    key,
                    added,
                    deleted,
                    shut,
                    ..
                } if key == "crates/web/src/" => Some((*added, *deleted, *shut)),
                _ => None,
            })
            .unwrap();
        assert_eq!(shut, (15, 7, true));
    }

    #[test]
    fn lines_are_numbered_from_the_hunks_starts() {
        let d = fixture("snapshot-diff.json");
        let o = order(&d);
        let lines = build(&d, &o, false);
        let pos = o
            .iter()
            .position(|&i| d.files[i].path == "crates/web/src/auth.rs")
            .unwrap();
        let rows: Vec<&Row> = lines
            .iter()
            .filter_map(|l| match l {
                Line::One(p, r) if *p == pos => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!((rows[0].old, rows[0].new), (Some(41), Some(41)));
        assert_eq!(rows[1].kind, Kind::Del);
        assert_eq!((rows[1].old, rows[1].new), (Some(42), None));
        let first_add = rows.iter().find(|r| r.kind == Kind::Add).unwrap();
        assert_eq!((first_add.old, first_add.new), (None, Some(42)));
    }

    #[test]
    fn a_seen_file_folds_and_a_changed_one_does_not() {
        let d = fixture("snapshot-diff.json");
        let o = order(&d);
        let lines = build(&d, &o, false);
        let renamed = o
            .iter()
            .position(|&i| d.files[i].path == "crates/web/src/session.rs")
            .unwrap();
        let changed = o
            .iter()
            .position(|&i| d.files[i].path == "crates/web/src/auth.rs")
            .unwrap();
        assert!(
            !lines
                .iter()
                .any(|l| matches!(l, Line::Hunk(p, _) if *p == renamed)),
            "seen: folded"
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, Line::Hunk(p, _) if *p == changed)),
            "changed since: open"
        );
    }

    #[test]
    fn side_by_side_pairs_deleted_with_added_and_keeps_new_files_in_one_column() {
        let d = fixture("snapshot-diff.json");
        let o = order(&d);
        let lines = build(&d, &o, true);
        let pairs = lines.iter().filter(|l| matches!(l, Line::Two(_, Some(a), Some(b)) if a.kind == Kind::Del && b.kind == Kind::Add)).count();
        assert!(pairs >= 2, "{pairs}");
        let login = o
            .iter()
            .position(|&i| d.files[i].path == "crates/web/src/login.rs")
            .unwrap();
        assert!(
            lines
                .iter()
                .filter(|l| l.file() == login)
                .all(|l| !matches!(l, Line::Two(..)))
        );
    }

    #[test]
    fn the_words_that_changed_are_the_middle_of_a_pair() {
        let ((a0, a1), (b0, b1)) = word_ranges("let x = 1;", "let x = 22;").unwrap();
        assert_eq!(
            (
                "let x = 1;"
                    .chars()
                    .skip(a0)
                    .take(a1 - a0)
                    .collect::<String>(),
                "let x = 22;"
                    .chars()
                    .skip(b0)
                    .take(b1 - b0)
                    .collect::<String>()
            ),
            ("1".into(), "22".into())
        );
        assert!(word_ranges("completely", "different text here").is_none());
    }

    #[test]
    fn syntax_tones_find_keywords_strings_numbers_and_comments() {
        let t = tones(r#"let n = 42; // note "x""#);
        assert_eq!(t[0], Tone::Keyword);
        assert_eq!(t[4], Tone::Plain);
        assert_eq!(t[8], Tone::Number);
        assert_eq!(t[13], Tone::Comment);
        let s = tones(r#"say("hi")"#);
        assert_eq!(s[4], Tone::Text);
        assert_eq!(
            tones("identifier_with_let")
                .iter()
                .filter(|t| **t == Tone::Keyword)
                .count(),
            0
        );
    }

    #[test]
    fn nothing_in_a_diff_can_send_the_terminal_a_command() {
        assert_eq!(clean("a\tb\x1b[2Jc\r"), "a    b·[2Jc·");
        let mut d = fixture("snapshot-diff.json");
        d.files[0].hunks[0].lines[0] = "+\x1b]52;c;aGk=\x07".into();
        let o = order(&d);
        let first = o.iter().position(|&i| i == 0).unwrap();
        let text: String = build(&d, &o, false)
            .into_iter()
            .filter(|l| l.file() == first)
            .filter_map(|l| match l {
                Line::One(_, r) => Some(r.text),
                _ => None,
            })
            .collect();
        assert!(!text.contains('\x1b') && !text.contains('\x07'), "{text:?}");
    }

    #[test]
    fn a_commit_is_dated_in_words() {
        assert_eq!(ago(1000, 990), "ahora");
        assert_eq!(ago(10_000, 10_000 - 7200), "hace 2 h");
        assert_eq!(ago(1_000_000, 1_000_000 - 90_000), "hace 1 d");
    }
}
