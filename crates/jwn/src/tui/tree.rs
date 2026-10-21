//! The file tree of a workspace (spec REQ-315 to REQ-322): the folders that are open, what each
//! was last said to hold, and the rows they make. The daemon lists one folder at a time; this is
//! what the client keeps of those answers and shows.

use std::collections::{BTreeSet, HashMap};

use contract::{FileEntry, FileKind, GitMark};

/// What is known of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dir {
    /// It has been asked for.
    Loading,
    /// Its entries, the first ones when there are more (`total`).
    Ready {
        entries: Vec<FileEntry>,
        total: usize,
    },
    /// The daemon could not list it, and why.
    Failed(String),
}

/// What a search found for the text typed so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub query: String,
    pub paths: Vec<String>,
    pub cut: bool,
}

/// One row of the tree as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// From the workspace's folder, with `/` between names.
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub kind: FileKind,
    pub git: Option<GitMark>,
    /// Git ignores it, or a folder above it.
    pub ignored: bool,
    pub open: bool,
    /// The weight of a file, in bytes.
    pub size: Option<u64>,
    /// Not an entry but a note under a folder: `leyendo…`, why it could not be read, how many are
    /// not shown.
    pub note: bool,
}

#[derive(Debug, Default)]
pub struct Tree {
    /// The row the keys are on, and the first row shown.
    pub sel: usize,
    pub scroll: usize,
    /// The folders that are open, by path; the workspace's own folder (`""`) always is.
    pub open: BTreeSet<String>,
    pub hidden: bool,
    /// The text of the search being typed, when there is one, and what it found.
    pub filter: Option<String>,
    pub found: Option<Found>,
    dirs: HashMap<String, Dir>,
}

impl Tree {
    /// The open folders that nobody has asked for yet, marked as asked: the root first.
    pub fn wanted(&mut self) -> Vec<String> {
        let mut want = vec![String::new()];
        want.extend(self.open.iter().cloned());
        want.retain(|p| !self.dirs.contains_key(p));
        for p in &want {
            self.dirs.insert(p.clone(), Dir::Loading);
        }
        want
    }

    /// The root and the open folders already listed, to be asked for again without forgetting what
    /// they hold: the rows stay as they are until the new answer replaces them.
    pub fn stale(&mut self) -> Vec<String> {
        let mut want = vec![String::new()];
        want.extend(self.open.iter().cloned());
        want.retain(|p| matches!(self.dirs.get(p), Some(Dir::Ready { .. } | Dir::Failed(_))));
        want
    }

    pub fn listed(&mut self, path: &str, dir: Dir) {
        self.dirs.insert(path.to_owned(), dir);
    }

    /// Forget everything listed, to ask again for what is open.
    pub fn forget(&mut self) {
        self.dirs.clear();
    }

    /// Open a folder, or close it.
    pub fn toggle(&mut self, path: &str) {
        if !self.open.remove(path) {
            self.open.insert(path.to_owned());
        }
    }

    /// The rows now: the search's paths while one is being typed, else the open folders from the top.
    pub fn rows(&self) -> Vec<Row> {
        if self.filter.is_some() {
            return self
                .found
                .iter()
                .flat_map(|f| f.paths.iter())
                .map(|p| Row {
                    path: p.clone(),
                    name: p.clone(),
                    depth: 0,
                    kind: FileKind::File,
                    git: None,
                    ignored: false,
                    open: false,
                    size: None,
                    note: false,
                })
                .collect();
        }
        let mut out = Vec::new();
        self.walk("", 0, false, &mut out);
        out
    }

    fn walk(&self, path: &str, depth: usize, ignored: bool, out: &mut Vec<Row>) {
        let note = |text: String, out: &mut Vec<Row>| {
            out.push(Row {
                path: String::new(),
                name: text,
                depth,
                kind: FileKind::File,
                git: None,
                ignored: true,
                open: false,
                size: None,
                note: true,
            });
        };
        match self.dirs.get(path) {
            None | Some(Dir::Loading) => {
                if depth > 0 || out.is_empty() {
                    note("leyendo…".into(), out);
                }
            }
            Some(Dir::Failed(why)) => note(why.clone(), out),
            Some(Dir::Ready { entries, total }) => {
                for e in entries
                    .iter()
                    .filter(|e| self.hidden || !e.name.starts_with('.'))
                {
                    let child = if path.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{path}/{}", e.name)
                    };
                    let open = e.kind == FileKind::Dir && self.open.contains(&child);
                    let ignored = ignored || e.ignored;
                    out.push(Row {
                        path: child.clone(),
                        name: e.name.clone(),
                        depth,
                        kind: e.kind,
                        git: e.git,
                        ignored,
                        open,
                        size: e.size,
                        note: false,
                    });
                    if open {
                        self.walk(&child, depth + 1, ignored, out);
                    }
                }
                if total > &entries.len() {
                    note(format!("…y {} más", total - entries.len()), out);
                }
            }
        }
    }

    /// Keep the row the keys are on, and the first one shown, inside `rows` rows that show `room`.
    pub fn clamp(&mut self, rows: usize, room: usize) {
        self.sel = self.sel.min(rows.saturating_sub(1));
        if self.sel < self.scroll {
            self.scroll = self.sel;
        }
        if room > 0 && self.sel >= self.scroll + room {
            self.scroll = self.sel + 1 - room;
        }
        self.scroll = self.scroll.min(rows.saturating_sub(room));
    }

    /// The parent folder of a path ("" for the workspace's own).
    pub fn parent_of(path: &str) -> &str {
        path.rsplit_once('/').map_or("", |(parent, _)| parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: FileKind) -> FileEntry {
        FileEntry {
            name: name.into(),
            kind,
            size: None,
            git: None,
            ignored: false,
        }
    }

    fn ready(entries: Vec<FileEntry>) -> Dir {
        let total = entries.len();
        Dir::Ready { entries, total }
    }

    fn names(t: &Tree) -> Vec<String> {
        t.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn loaded() -> Tree {
        let mut t = Tree::default();
        t.listed(
            "",
            ready(vec![
                e("src", FileKind::Dir),
                e("target", FileKind::Dir),
                FileEntry {
                    git: Some(GitMark::Modified),
                    ..e("Cargo.toml", FileKind::File)
                },
                e(".gitignore", FileKind::File),
            ]),
        );
        t.listed(
            "src",
            ready(vec![
                e("main.rs", FileKind::File),
                e(".hid", FileKind::File),
            ]),
        );
        t
    }

    #[test]
    fn the_root_is_asked_for_first_and_each_open_folder_once() {
        let mut t = Tree::default();
        assert_eq!(t.wanted(), [""]);
        assert!(t.wanted().is_empty(), "it is not asked for twice");
        t.open.insert("src".into());
        assert_eq!(t.wanted(), ["src"]);
        t.forget();
        assert_eq!(
            t.wanted(),
            ["", "src"],
            "after forgetting, again, the root first"
        );
    }

    #[test]
    fn rows_are_the_open_folders_from_the_top_without_the_hidden_unless_asked() {
        let mut t = loaded();
        assert_eq!(names(&t), ["src", "target", "Cargo.toml"]);
        t.toggle("src");
        assert_eq!(names(&t), ["src", "  main.rs", "target", "Cargo.toml"]);
        t.hidden = true;
        assert_eq!(
            names(&t),
            [
                "src",
                "  main.rs",
                "  .hid",
                "target",
                "Cargo.toml",
                ".gitignore"
            ]
        );
        t.toggle("src");
        assert!(!t.open.contains("src"));
        assert_eq!(t.rows()[0].path, "src");
        assert!(!t.rows()[0].open);
    }

    #[test]
    fn what_git_says_comes_with_the_row_and_what_is_under_an_ignored_folder_is_ignored() {
        let mut t = loaded();
        t.listed(
            "",
            ready(vec![
                FileEntry {
                    ignored: true,
                    ..e("target", FileKind::Dir)
                },
                FileEntry {
                    git: Some(GitMark::Modified),
                    ..e("Cargo.toml", FileKind::File)
                },
            ]),
        );
        t.open.insert("target".into());
        t.listed("target", ready(vec![e("debug", FileKind::Dir)]));
        let rows = t.rows();
        assert!(rows[0].ignored);
        assert!(rows[1].ignored, "inside an ignored folder");
        assert_eq!(rows[2].git, Some(GitMark::Modified));
        assert!(!rows[2].ignored);
    }

    #[test]
    fn a_folder_being_read_a_folder_that_failed_and_a_cut_listing_say_so_in_a_note_row() {
        let mut t = loaded();
        t.open.insert("src".into());
        t.forget();
        t.listed("", ready(vec![e("src", FileKind::Dir)]));
        assert_eq!(names(&t), ["src", "  leyendo…"]);
        t.listed("src", Dir::Failed("cannot read src: denied".into()));
        assert_eq!(names(&t), ["src", "  cannot read src: denied"]);
        assert!(t.rows()[1].note);
        t.listed(
            "src",
            Dir::Ready {
                entries: vec![e("a.rs", FileKind::File)],
                total: 2005,
            },
        );
        assert_eq!(names(&t), ["src", "  a.rs", "  …y 2004 más"]);
        assert_eq!(
            Tree::default().rows().len(),
            1,
            "before the root arrives it says it is being read"
        );
    }

    #[test]
    fn while_a_search_is_typed_the_rows_are_what_it_found_and_nothing_else() {
        let mut t = loaded();
        t.filter = Some("ma".into());
        assert!(t.rows().is_empty(), "nothing found yet");
        t.found = Some(Found {
            query: "ma".into(),
            paths: vec!["src/main.rs".into(), "docs/map.md".into()],
            cut: false,
        });
        assert_eq!(names(&t), ["src/main.rs", "docs/map.md"]);
        t.filter = None;
        assert_eq!(names(&t), ["src", "target", "Cargo.toml"]);
    }

    #[test]
    fn the_row_the_keys_are_on_stays_inside_the_rows_and_the_view() {
        let mut t = Tree {
            sel: 99,
            ..Tree::default()
        };
        t.clamp(10, 4);
        assert_eq!((t.sel, t.scroll), (9, 6));
        t.sel = 2;
        t.clamp(10, 4);
        assert_eq!((t.sel, t.scroll), (2, 2));
        t.clamp(0, 4);
        assert_eq!((t.sel, t.scroll), (0, 0));
    }

    #[test]
    fn the_parent_of_a_path_is_its_folder() {
        assert_eq!(Tree::parent_of("src/auth/login.rs"), "src/auth");
        assert_eq!(Tree::parent_of("src"), "");
    }
}
