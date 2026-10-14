//! Opening a folder (`o`) and making a worktree (`w`) in the terminal client (prototype:
//! `docs/prototypes/tui-open.html`). The folder browser reads this machine's disk, as the
//! client runs beside the daemon; the worktree box checks what it can before asking git.
//! Both only hold what the person is doing: what they ask of the daemon comes out as a
//! value for `View` to send.

use std::path::{Path, PathBuf};
use std::process::Command;

use crossterm::event::KeyCode;
use daemon::browse::{home, is_git, subdirs};

use super::{Grid, Rect, Style, clip, look};

/// What a row of the browser is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The folder the browser is in.
    Here,
    Dir,
    /// A folder that does not exist yet, named by what was typed.
    New,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub kind: Kind,
    pub name: String,
    pub path: PathBuf,
    pub git: bool,
    pub open: bool,
}

/// What a key in the browser asks.
#[derive(PartialEq, Eq, Debug)]
pub enum Pick {
    Stay,
    Close,
    Open { path: PathBuf, create: bool },
}

pub struct Picker {
    pub dir: PathBuf,
    pub query: String,
    pub sel: usize,
    /// What `dir` holds, read once when the browser comes to it: each folder and whether it is
    /// a repository. The screen is drawn many times a second and a folder may hold thousands.
    entries: Vec<(String, bool)>,
    /// The folder the highlighted row stands for and the names of the folders in it, read when
    /// the highlight lands on it.
    look: (PathBuf, Vec<String>),
}

/// The letters of `query` appear in `name` in that order.
fn fuzzy(query: &str, name: &str) -> bool {
    let mut want = query.chars().flat_map(char::to_lowercase);
    let mut next = want.next();
    for c in name.chars().flat_map(char::to_lowercase) {
        if next == Some(c) {
            next = want.next();
        }
    }
    next.is_none()
}

/// A name a new folder may have.
fn valid_new(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && name.len() <= 80
}

impl Picker {
    /// Starts in `dir`, the folder beside the project the person is in.
    pub fn new(dir: PathBuf) -> Picker {
        let mut p = Picker {
            dir,
            query: String::new(),
            sel: 0,
            entries: Vec::new(),
            look: (PathBuf::new(), Vec::new()),
        };
        p.reload();
        p.refresh();
        p
    }

    /// Read the folder the browser is in.
    fn reload(&mut self) {
        self.entries = subdirs(&self.dir)
            .into_iter()
            .map(|n| {
                let git = is_git(&self.dir.join(&n));
                (n, git)
            })
            .collect();
        self.look.0 = PathBuf::new();
    }

    /// The highlighted row's folder was not looked into yet: do it.
    fn refresh(&mut self) {
        let rows = self.rows(&|_| false);
        let row = &rows[self.sel.min(rows.len() - 1)];
        if self.look.0 != row.path {
            self.look = (
                row.path.clone(),
                if row.kind == Kind::New {
                    Vec::new()
                } else {
                    subdirs(&row.path).into_iter().take(12).collect()
                },
            );
        }
    }

    /// The names of the folders in the highlighted row's folder.
    pub fn look(&self) -> &[String] {
        &self.look.1
    }

    /// The rows: this folder, then its folders that match what was typed, then the row that
    /// makes a folder of that name when there is none.
    pub fn rows(&self, is_open: &dyn Fn(&Path) -> bool) -> Vec<Row> {
        let mut rows = vec![Row {
            kind: Kind::Here,
            name: "abrir esta carpeta".into(),
            path: self.dir.clone(),
            git: is_git(&self.dir),
            open: is_open(&self.dir),
        }];
        rows.extend(
            self.entries
                .iter()
                .filter(|(n, _)| self.query.is_empty() || fuzzy(&self.query, n))
                .map(|(n, git)| {
                    let path = self.dir.join(n);
                    Row {
                        kind: Kind::Dir,
                        name: format!("{n}/"),
                        git: *git,
                        open: is_open(&path),
                        path,
                    }
                }),
        );
        if valid_new(&self.query) && !self.entries.iter().any(|(n, _)| *n == self.query) {
            rows.push(Row {
                kind: Kind::New,
                name: format!("crear «{}»/", self.query),
                path: self.dir.join(&self.query),
                git: false,
                open: false,
            });
        }
        rows
    }

    pub fn press(&mut self, code: KeyCode, ctrl: bool, is_open: &dyn Fn(&Path) -> bool) -> Pick {
        let pick = self.act(code, ctrl, is_open);
        self.refresh();
        pick
    }

    fn act(&mut self, code: KeyCode, ctrl: bool, is_open: &dyn Fn(&Path) -> bool) -> Pick {
        let rows = self.rows(is_open);
        let at = self.sel.min(rows.len() - 1);
        match code {
            KeyCode::Esc => return Pick::Close,
            KeyCode::Down => self.sel = (at + 1).min(rows.len() - 1),
            KeyCode::Up => self.sel = at.saturating_sub(1),
            KeyCode::Char('n') if ctrl => self.sel = (at + 1).min(rows.len() - 1),
            KeyCode::Char('p') if ctrl => self.sel = at.saturating_sub(1),
            KeyCode::Char('u') if ctrl => self.reset(),
            KeyCode::Right | KeyCode::Tab => {
                if rows[at].kind == Kind::Dir {
                    self.dir = rows[at].path.clone();
                    self.reset();
                    self.reload();
                }
            }
            KeyCode::Left => self.up(),
            KeyCode::Backspace if self.query.is_empty() => self.up(),
            KeyCode::Backspace => {
                self.query.pop();
                self.sel = 0;
            }
            KeyCode::Char('~') if self.query.is_empty() && !ctrl => {
                self.dir = home();
                self.reset();
                self.reload();
            }
            KeyCode::Enter => {
                let row = &rows[at];
                return Pick::Open {
                    path: row.path.clone(),
                    create: row.kind == Kind::New,
                };
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Pick::Stay
    }

    fn reset(&mut self) {
        self.query.clear();
        self.sel = 0;
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.dir = parent.to_path_buf();
        }
        self.reset();
        self.reload();
    }
}

/// `path` with the home folder written `~`.
fn short(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn panel() -> Style {
    Style::default().on(look::panel())
}

/// Draw the browser over `area`: the list at the left, a look at the highlighted folder at
/// the right.
pub fn draw_picker(g: &mut Grid, (x, y, w, h): Rect, p: &Picker, is_open: &dyn Fn(&Path) -> bool) {
    let bw = 82.min(w.saturating_sub(2)).max(40);
    let bh = 22.min(h.saturating_sub(1)).max(10);
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 1);
    let list = bw * 4 / 9;
    for r in 0..bh {
        for c in 0..bw {
            if by + r < g.rows && bx + c < g.cols {
                g.cells[(by + r) * g.cols + bx + c] = (' ', panel());
            }
        }
    }
    g.boxed(
        (bx, by, bw, bh),
        "abrir una carpeta",
        look::blue(),
        Style::fg(look::fg()).bold(),
        None,
    );
    let on_panel = |s: Style| s.on(look::panel());
    g.put(
        bx + 3,
        by + 1,
        &clip(&short(&p.dir), list - 2),
        on_panel(Style::fg(look::dim())),
    );
    let typed = if p.query.is_empty() {
        ("escribe para filtrar esta carpeta…".to_owned(), look::dim())
    } else {
        (format!("{}▏", p.query), look::fg())
    };
    g.put(bx + 3, by + 2, "❯", on_panel(Style::fg(look::blue())));
    g.put(
        bx + 5,
        by + 2,
        &clip(&typed.0, list - 4),
        on_panel(Style::fg(typed.1)),
    );
    let rows = p.rows(is_open);
    let sel = p.sel.min(rows.len() - 1);
    let room = bh.saturating_sub(7);
    let first = sel.saturating_sub(room.saturating_sub(1));
    for (i, row) in rows.iter().enumerate().skip(first).take(room) {
        let at = by + 4 + i - first;
        let on = i == sel;
        let ground = if on { look::sel() } else { look::panel() };
        g.put(bx + 2, at, &" ".repeat(list), Style::default().on(ground));
        let (mark, colour) = match row.kind {
            Kind::Here => (".", look::dim()),
            Kind::New => ("+", look::yellow()),
            Kind::Dir if row.open => ("●", look::green()),
            Kind::Dir => ("▸", look::dim()),
        };
        g.put(bx + 3, at, mark, Style::fg(colour).on(ground));
        let name = if on {
            Style::fg(look::blue()).bold()
        } else {
            Style::fg(look::fg())
        };
        g.put(
            bx + 5,
            at,
            &clip(&row.name, list.saturating_sub(14)),
            name.on(ground),
        );
        let tag = match row.kind {
            Kind::New => "nueva",
            _ if row.git => "git",
            _ if row.open => "abierta",
            _ => "",
        };
        g.put(
            bx + 2 + list - tag.chars().count() - 1,
            at,
            tag,
            Style::fg(look::dim()).on(ground),
        );
    }
    if rows.len() == 1 && !p.query.is_empty() {
        g.put(
            bx + 5,
            by + 5,
            "Nada coincide.",
            on_panel(Style::fg(look::dim())),
        );
    }
    for r in 1..bh - 1 {
        g.put(
            bx + list + 3,
            by + r,
            "│",
            on_panel(Style::fg(look::line())),
        );
    }
    // What is highlighted.
    let row = &rows[sel];
    let px = bx + list + 6;
    let right = bw.saturating_sub(list + 9);
    g.put(
        px,
        by + 2,
        &clip(&short(&row.path), right),
        on_panel(Style::fg(look::fg()).bold()),
    );
    let (note, colour) = match (row.kind, row.git) {
        (Kind::New, _) => ("carpeta nueva: se crea al abrirla", look::dim()),
        (_, true) => ("repositorio git", look::green()),
        _ => ("no es un repositorio git", look::dim()),
    };
    g.put(px, by + 3, note, on_panel(Style::fg(colour)));
    if row.open {
        g.put(
            px,
            by + 4,
            "● abierta: ↵ va ahí",
            on_panel(Style::fg(look::green())),
        );
    }
    if row.kind != Kind::New {
        let kids = p.look();
        if kids.is_empty() {
            g.put(px, by + 6, "vacía", on_panel(Style::fg(look::dim())));
        }
        for (i, n) in kids.iter().take(bh.saturating_sub(9)).enumerate() {
            g.put(
                px,
                by + 6 + i,
                &clip(&format!("▸ {n}/"), right),
                on_panel(Style::fg(look::fg())),
            );
        }
    }
    g.put(
        bx + 3,
        by + bh - 2,
        &clip(
            "↑↓ mover · → entrar · ← subir · ~ casa · ↵ abrir · esc cerrar",
            bw - 6,
        ),
        on_panel(Style::fg(look::dim())),
    );
}

/// What making a worktree will make, or why it cannot be made.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Plan {
    Ok { branch: String, folder: PathBuf },
    Wrong(String),
}

/// What a key in the worktree box asks.
#[derive(PartialEq, Eq, Debug)]
pub enum Make {
    Stay,
    Close,
    Create {
        repo: PathBuf,
        branch: String,
        from: String,
        folder: String,
    },
}

pub struct Worktree {
    /// The repository the worktree is of.
    pub repo: PathBuf,
    /// The branch it starts from: the one of the workspace the person is in.
    pub from: String,
    pub name: String,
    /// The name is the suggested one: the first key replaces it.
    picked: bool,
    /// The folders of the open workspaces, to tell a worktree that is already there.
    open: Vec<PathBuf>,
    plan: Plan,
}

/// What `git` prints for `args` run in `dir`, when it succeeds.
pub fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn repo_name(repo: &Path) -> String {
    repo.file_name()
        .map_or_else(|| "repo".into(), |n| n.to_string_lossy().into_owned())
}

/// The folder worktrees of `repo` go in: `<repo>-wt`, beside it.
fn worktrees_dir(repo: &Path) -> PathBuf {
    let name = format!("{}-wt", repo_name(repo));
    repo.parent()
        .map_or_else(|| PathBuf::from(&name), |p| p.join(&name))
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    git_out(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_some()
}

impl Worktree {
    /// A box for a worktree of `repo` starting from `from`, with the next `ws-N` free as the name.
    pub fn new(repo: PathBuf, from: String, open: Vec<PathBuf>) -> Worktree {
        let mut w = Worktree {
            repo,
            from,
            name: String::new(),
            picked: true,
            open,
            plan: Plan::Wrong(String::new()),
        };
        // The first `ws-N` that is free; a thousand taken is a sign nothing will be.
        for n in 1..=1000 {
            w.name = format!("ws-{n}");
            if matches!(w.check(), Plan::Ok { .. }) {
                break;
            }
        }
        w.plan = w.check();
        w
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    fn check(&self) -> Plan {
        let name = &self.name;
        let valid = !name.is_empty()
            && name.len() <= 40
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !name.starts_with('-');
        if !valid {
            return Plan::Wrong("nombre no válido: usa minúsculas, números y guiones".into());
        }
        let folder = worktrees_dir(&self.repo).join(name);
        let branch = format!("feat/{name}");
        if self.open.contains(&folder) {
            return Plan::Wrong(format!("{}/{name} ya existe", repo_name(&self.repo)));
        }
        if folder.exists() {
            return Plan::Wrong(format!("{} ya existe", short(&folder)));
        }
        if branch_exists(&self.repo, &branch) {
            return Plan::Wrong(format!("la rama {branch} ya existe"));
        }
        Plan::Ok { branch, folder }
    }

    pub fn press(&mut self, code: KeyCode, ctrl: bool) -> Make {
        match code {
            KeyCode::Esc => return Make::Close,
            KeyCode::Enter => {
                if let Plan::Ok { branch, .. } = &self.plan {
                    return Make::Create {
                        repo: self.repo.clone(),
                        branch: branch.clone(),
                        from: self.from.clone(),
                        folder: self.name.clone(),
                    };
                }
            }
            KeyCode::Backspace => {
                if self.picked {
                    self.name.clear();
                } else {
                    self.name.pop();
                }
                self.picked = false;
                self.plan = self.check();
            }
            KeyCode::Char(c) if !ctrl => {
                if self.picked {
                    self.name.clear();
                }
                self.name.push(c);
                self.picked = false;
                self.plan = self.check();
            }
            _ => {}
        }
        Make::Stay
    }
}

/// Draw the box of a worktree over `area`.
pub fn draw_worktree(g: &mut Grid, (x, y, w, _): Rect, t: &Worktree) {
    let lines: Vec<(String, Style)> = {
        let plain = Style::fg(look::fg());
        let mut v = vec![
            (format!("nombre    {}▏", t.name), plain.bold()),
            (String::new(), plain),
        ];
        match t.plan() {
            Plan::Wrong(why) => v.extend([
                (why.clone(), Style::fg(look::red())),
                (String::new(), plain),
                (String::new(), plain),
            ]),
            Plan::Ok { branch, folder } => v.extend([
                (format!("rama      {branch}"), Style::fg(look::green())),
                (format!("desde     {}", t.from), plain),
                (format!("carpeta   {}", short(folder)), plain),
            ]),
        }
        v.push((String::new(), plain));
        let ok = matches!(t.plan(), Plan::Ok { .. });
        v.push((
            format!("↵ crear{} · esc cancelar", if ok { " y abrir" } else { "" }),
            Style::fg(look::dim()),
        ));
        v
    };
    let widest = lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let title = format!("worktree nuevo de {}", repo_name(&t.repo));
    let bw = (widest + 8)
        .max(title.chars().count() + 8)
        .min(w.saturating_sub(2))
        .max(30);
    let bh = lines.len() + 4;
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 3);
    for r in 0..bh {
        for c in 0..bw {
            if by + r < g.rows && bx + c < g.cols {
                g.cells[(by + r) * g.cols + bx + c] = (' ', panel());
            }
        }
    }
    let border = if matches!(t.plan(), Plan::Ok { .. }) {
        look::blue()
    } else {
        look::red()
    };
    g.boxed(
        (bx, by, bw, bh),
        &title,
        border,
        Style::fg(look::fg()).bold(),
        None,
    );
    for (i, (text, style)) in lines.iter().enumerate() {
        g.put(
            bx + 3,
            by + 2 + i,
            &clip(text, bw - 6),
            style.on(look::panel()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder tree in a private place: `root/{api (git), notes, Zeta, .hidden}`.
    fn tree(name: &str) -> PathBuf {
        let root = PathBuf::from(format!("/tmp/jwn-open-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["api/.git", "api/src", "notes", "Zeta", ".hidden"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "x").unwrap();
        root
    }

    fn none(_: &Path) -> bool {
        false
    }

    #[test]
    fn the_browser_lists_this_folder_then_the_folders_without_hidden_ones_or_files() {
        let root = tree("list");
        let p = Picker::new(root.clone());
        let names: Vec<String> = p.rows(&none).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["abrir esta carpeta", "api/", "notes/", "Zeta/"]);
        let rows = p.rows(&|path| path == root.join("notes"));
        assert!(rows[1].git && !rows[1].open, "api is a repository");
        assert!(rows[2].open && !rows[2].git, "notes is open");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typing_filters_by_letters_in_order_and_offers_to_make_what_is_not_there() {
        let root = tree("filter");
        let mut p = Picker::new(root.clone());
        for c in "zt".chars() {
            p.press(KeyCode::Char(c), false, &none);
        }
        let names: Vec<String> = p.rows(&none).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["abrir esta carpeta", "Zeta/", "crear «zt»/"]);
        // A name that is a folder is not offered again; a bad name is not offered.
        p.query = "api".into();
        assert!(p.rows(&none).iter().all(|r| r.kind != Kind::New));
        p.query = "a b".into();
        assert!(p.rows(&none).iter().all(|r| r.kind != Kind::New));
        p.query = ".x".into();
        assert!(p.rows(&none).iter().all(|r| r.kind != Kind::New));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_arrows_move_in_and_out_and_enter_picks_what_is_highlighted() {
        let root = tree("keys");
        let mut p = Picker::new(root.clone());
        assert_eq!(p.press(KeyCode::Down, false, &none), Pick::Stay);
        assert_eq!(p.sel, 1);
        // `→` goes into api.
        p.press(KeyCode::Right, false, &none);
        assert_eq!(p.dir, root.join("api"));
        assert_eq!(p.sel, 0);
        // `←` and an empty backspace go up.
        p.press(KeyCode::Left, false, &none);
        assert_eq!(p.dir, root);
        p.press(KeyCode::Down, false, &none);
        p.press(KeyCode::Tab, false, &none);
        p.press(KeyCode::Backspace, false, &none);
        assert_eq!(p.dir, root);
        // Enter on the first row is this folder; on a row of a folder, that folder.
        assert_eq!(
            p.press(KeyCode::Enter, false, &none),
            Pick::Open {
                path: root.clone(),
                create: false
            }
        );
        p.press(KeyCode::Down, false, &none);
        p.press(KeyCode::Down, false, &none);
        assert_eq!(
            p.press(KeyCode::Enter, false, &none),
            Pick::Open {
                path: root.join("notes"),
                create: false
            }
        );
        // A new name is created and opened; `ctrl-u` clears what was typed; `esc` closes.
        for c in "new".chars() {
            p.press(KeyCode::Char(c), false, &none);
        }
        p.press(KeyCode::Down, false, &none);
        assert_eq!(
            p.press(KeyCode::Enter, false, &none),
            Pick::Open {
                path: root.join("new"),
                create: true
            }
        );
        p.press(KeyCode::Char('u'), true, &none);
        assert!(p.query.is_empty());
        assert_eq!(p.press(KeyCode::Esc, false, &none), Pick::Close);
        // `~` goes home when nothing is typed, and is a letter otherwise.
        p.press(KeyCode::Char('~'), false, &none);
        assert_eq!(p.dir, home());
        let _ = std::fs::remove_dir_all(root);
    }

    fn repo(root: &Path) -> PathBuf {
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&app)
                    .args(&args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        app
    }

    #[test]
    fn the_worktree_box_suggests_the_next_free_name_and_shows_what_it_will_make() {
        let root = tree("wt");
        let app = repo(&root);
        let t = Worktree::new(app.clone(), "main".into(), vec![]);
        assert_eq!(t.name, "ws-1");
        let Plan::Ok { branch, folder } = t.plan() else {
            panic!("{:?}", t.plan())
        };
        assert_eq!(branch, "feat/ws-1");
        assert_eq!(*folder, root.join("app-wt").join("ws-1"));
        // The first one is taken (open, on disk, or as a branch): the suggestion moves on.
        let t = Worktree::new(app.clone(), "main".into(), vec![root.join("app-wt/ws-1")]);
        assert_eq!(t.name, "ws-2");
        std::fs::create_dir_all(root.join("app-wt/ws-2")).unwrap();
        assert!(git_out(&app, &["branch", "feat/ws-3"]).is_some());
        let t = Worktree::new(app, "main".into(), vec![root.join("app-wt/ws-1")]);
        assert_eq!(t.name, "ws-4");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typing_replaces_the_suggestion_and_says_why_a_name_cannot_be_used() {
        let root = tree("wt2");
        let app = repo(&root);
        std::fs::create_dir_all(root.join("app-wt/taken")).unwrap();
        assert!(git_out(&app, &["branch", "feat/old"]).is_some());
        let mut t = Worktree::new(app.clone(), "main".into(), vec![root.join("app-wt/there")]);
        let type_name = |t: &mut Worktree, s: &str| {
            for _ in 0..12 {
                t.press(KeyCode::Backspace, false);
            }
            for c in s.chars() {
                t.press(KeyCode::Char(c), false);
            }
        };
        t.press(KeyCode::Char('x'), false);
        assert_eq!(t.name, "x", "the first key replaced ws-1");
        let why = |t: &Worktree| match t.plan() {
            Plan::Wrong(w) => w.clone(),
            Plan::Ok { .. } => String::new(),
        };
        type_name(&mut t, "Bad Name");
        assert_eq!(
            why(&t),
            "nombre no válido: usa minúsculas, números y guiones"
        );
        type_name(&mut t, "");
        assert!(
            why(&t).starts_with("nombre no válido"),
            "empty is not a name"
        );
        type_name(&mut t, "there");
        assert_eq!(why(&t), "app/there ya existe");
        type_name(&mut t, "taken");
        assert!(why(&t).ends_with("app-wt/taken ya existe"), "{}", why(&t));
        type_name(&mut t, "old");
        assert_eq!(why(&t), "la rama feat/old ya existe");
        // Enter does nothing while it cannot be made, and asks for what the plan says when it can.
        assert_eq!(t.press(KeyCode::Enter, false), Make::Stay);
        type_name(&mut t, "fresh");
        assert_eq!(
            t.press(KeyCode::Enter, false),
            Make::Create {
                repo: app,
                branch: "feat/fresh".into(),
                from: "main".into(),
                folder: "fresh".into()
            }
        );
        assert_eq!(t.press(KeyCode::Esc, false), Make::Close);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn a_folder_is_read_when_the_browser_comes_to_it_and_not_on_every_draw() {
        let root = tree("cache");
        let mut p = Picker::new(root.clone());
        assert_eq!(p.look().len(), 3, "what the first row, this folder, holds");
        // Something appears on disk: the list the screen draws from does not change by itself.
        std::fs::create_dir_all(root.join("later")).unwrap();
        let names = |p: &Picker| {
            p.rows(&none)
                .into_iter()
                .map(|r| r.name)
                .collect::<Vec<_>>()
        };
        assert!(!names(&p).contains(&"later/".to_owned()));
        // Going into a folder and back reads it again; the look follows the highlight.
        p.press(KeyCode::Down, false, &none);
        assert_eq!(p.look(), ["src".to_owned()], "what api holds");
        p.press(KeyCode::Right, false, &none);
        p.press(KeyCode::Left, false, &none);
        assert!(names(&p).contains(&"later/".to_owned()));
        let _ = std::fs::remove_dir_all(root);
    }
}
