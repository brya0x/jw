//! Opening a folder (`o`) and making a worktree (`w`) in the terminal client (prototype:
//! `docs/prototypes/tui-open.html`). The folder browser reads this machine's disk, as the
//! client runs beside the daemon; the worktree box checks what it can before asking git.
//! Both only hold what the person is doing: what they ask of the daemon comes out as a
//! value for `View` to send.

use std::path::{Path, PathBuf};
use std::process::Command;

use contract::{OpenKind, ProjectInfo, WorktreeInfo};
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

/// The text typed for a new worktree is its branch, as typed (REQ-512): lowercase words, digits and
/// hyphens, with `/` between the parts of a prefix (`fix/login`).
fn valid_branch(name: &str) -> bool {
    name.len() <= 60
        && name.split('/').all(|part| {
            !part.is_empty()
                && part.len() <= 40
                && !part.starts_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// The worktree's folder is the last part of its branch: `login` for `fix/login`.
fn folder_of(branch: &str) -> &str {
    branch.rsplit('/').next().unwrap_or(branch)
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

/// Draw the browser over `area`: the list at the left, a look at the highlighted folder at
/// the right.
pub fn draw_picker(g: &mut Grid, (x, y, w, h): Rect, p: &Picker, is_open: &dyn Fn(&Path) -> bool) {
    let bw = 82.min(w.saturating_sub(2)).max(40);
    let bh = 22.min(h.saturating_sub(1)).max(10);
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 1);
    let list = bw * 4 / 9;
    g.clear_box((bx, by, bw, bh));
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
    Ok {
        branch: String,
        folder: PathBuf,
    },
    /// The daemon has not yet said what the repository's `.jw.toml` holds.
    Reading,
    Wrong(String),
}

/// What the box knows of the repository's settings (`.jw.toml`): the daemon's answer to
/// `project_config`, which says where the worktree goes and what opens with it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Config {
    Reading,
    Ready(ProjectInfo),
    Refused(String),
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
        /// What the worktree opens with, as the person left it (REQ-510).
        open: OpenKind,
    },
    /// Open these worktrees that exist (REQ-534); the box stays.
    OpenExisting {
        repo: PathBuf,
        paths: Vec<String>,
    },
}

/// The two sides of the worktree box: a new worktree, or the ones git already has (REQ-533).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    New,
    Existing,
}

/// What the box knows of the worktrees that exist: the daemon's answer to `list_worktrees`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Existing {
    Reading,
    Ready(Vec<WorktreeInfo>),
    Refused(String),
}

/// A worktree that can be opened: its folder is there and no workspace has it.
fn openable(w: &WorktreeInfo) -> bool {
    !w.missing && w.workspace.is_none()
}

/// A worktree's name in a list: its branch, or its commit when the head is detached.
fn shown(w: &WorktreeInfo) -> String {
    w.branch.clone().unwrap_or_else(|| format!("({})", w.head))
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
    pub config: Config,
    plan: Plan,
    /// What this worktree opens with: the settings' choice until `tab` changes it.
    pub opens: OpenKind,
    pub side: Side,
    pub existing: Existing,
    /// The list of the worktrees that exist has been asked for.
    listed_once: bool,
    /// The row of the list of existing worktrees the keys are on.
    sel: usize,
    /// The paths marked with `espacio` to open together.
    marked: Vec<String>,
    /// What the last opening did, or why it could not.
    pub note: Option<(String, bool)>,
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

pub(super) fn repo_name(repo: &Path) -> String {
    repo.file_name()
        .map_or_else(|| "repo".into(), |n| n.to_string_lossy().into_owned())
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
    /// A box for a worktree of `repo` starting from `from`. It waits for the daemon to say what
    /// the repository's settings are (`set_config`) before it can name a place or a branch.
    pub fn new(repo: PathBuf, from: String, open: Vec<PathBuf>) -> Worktree {
        Worktree {
            repo,
            from,
            name: "ws-1".into(),
            picked: true,
            open,
            config: Config::Reading,
            plan: Plan::Reading,
            opens: OpenKind::Shell,
            side: Side::New,
            existing: Existing::Reading,
            listed_once: false,
            sel: 0,
            marked: Vec::new(),
            note: None,
        }
    }

    /// The daemon's answer to `list_worktrees`: the row and the marks stay where they can.
    pub fn set_existing(&mut self, reply: Result<Vec<WorktreeInfo>, String>) {
        self.existing = match reply {
            Ok(list) => {
                self.sel = self.sel.min(list.len().saturating_sub(1));
                self.marked
                    .retain(|p| list.iter().any(|w| &w.path == p && openable(w)));
                Existing::Ready(list)
            }
            Err(why) => Existing::Refused(why),
        };
    }

    /// Whether the list of the worktrees that exist must be asked for now: the first time the
    /// side is shown, and not when the box opens, because git reads the status of each one.
    pub fn wants_list(&mut self) -> bool {
        let wants = self.side == Side::Existing && !self.listed_once;
        self.listed_once |= wants;
        wants
    }

    /// What opening did: said in the box.
    pub fn say(&mut self, text: String, good: bool) {
        self.note = Some((text, good));
    }

    fn listed(&self) -> &[WorktreeInfo] {
        match &self.existing {
            Existing::Ready(list) => list,
            _ => &[],
        }
    }

    /// A key on the side of the worktrees that exist.
    fn press_existing(&mut self, code: KeyCode, ctrl: bool) -> Make {
        let n = self.listed().len();
        match code {
            KeyCode::Esc => return Make::Close,
            KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if !ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(w) = self.listed().get(self.sel).filter(|w| openable(w)) {
                    let path = w.path.clone();
                    match self.marked.iter().position(|p| *p == path) {
                        Some(at) => {
                            self.marked.remove(at);
                        }
                        None => self.marked.push(path),
                    }
                }
            }
            KeyCode::Char('a') if !ctrl => {
                let all: Vec<String> = self
                    .listed()
                    .iter()
                    .filter(|w| openable(w))
                    .map(|w| w.path.clone())
                    .collect();
                return self.open_these(all);
            }
            KeyCode::Enter => {
                if !self.marked.is_empty() {
                    let marked = std::mem::take(&mut self.marked);
                    return self.open_these(marked);
                }
                let here = self.listed().get(self.sel).cloned();
                match here {
                    Some(w) if openable(&w) => return self.open_these(vec![w.path]),
                    Some(w) if w.missing => {
                        self.say(
                            "esa carpeta ya no existe: no hay nada que abrir".into(),
                            false,
                        );
                    }
                    Some(_) => self.say("ya está abierto".into(), false),
                    None => {}
                }
            }
            _ => {}
        }
        Make::Stay
    }

    fn open_these(&mut self, paths: Vec<String>) -> Make {
        if paths.is_empty() {
            self.say("no hay más cerrados".into(), false);
            return Make::Stay;
        }
        self.note = None;
        Make::OpenExisting {
            repo: self.repo.clone(),
            paths,
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The daemon's answer to `project_config`. While the name is still the suggested one it
    /// moves to the first `ws-N` that is free.
    pub fn set_config(&mut self, reply: Result<ProjectInfo, String>) {
        self.config = match reply {
            Ok(info) => {
                self.opens = info.open;
                Config::Ready(info)
            }
            Err(why) => Config::Refused(why),
        };
        if self.picked {
            // A thousand taken is a sign nothing will be.
            for n in 1..=1000 {
                self.name = format!("ws-{n}");
                if matches!(self.check(), Plan::Ok { .. }) {
                    break;
                }
            }
        }
        self.plan = self.check();
    }

    fn check(&self) -> Plan {
        let info = match &self.config {
            Config::Reading => return Plan::Reading,
            Config::Refused(why) => return Plan::Wrong(why.clone()),
            Config::Ready(info) => info,
        };
        let branch = &self.name;
        if !valid_branch(branch) {
            return Plan::Wrong(
                "nombre no válido: minúsculas, números y guiones, con / para un prefijo".into(),
            );
        }
        let name = folder_of(branch);
        let folder = Path::new(&info.root).join(name);
        if self.open.contains(&folder) {
            return Plan::Wrong(format!("{}/{name} ya existe", repo_name(&self.repo)));
        }
        if folder.exists() {
            return Plan::Wrong(format!("{} ya existe", short(&folder)));
        }
        if branch_exists(&self.repo, branch) {
            return Plan::Wrong(format!("la rama {branch} ya existe"));
        }
        Plan::Ok {
            branch: branch.clone(),
            folder,
        }
    }

    pub fn press(&mut self, code: KeyCode, ctrl: bool) -> Make {
        if matches!(code, KeyCode::Left | KeyCode::Right) {
            self.side = match self.side {
                Side::New => Side::Existing,
                Side::Existing => Side::New,
            };
            self.note = None;
            return Make::Stay;
        }
        if self.side == Side::Existing {
            return self.press_existing(code, ctrl);
        }
        match code {
            KeyCode::Esc => return Make::Close,
            KeyCode::Enter => {
                if let Plan::Ok { branch, .. } = &self.plan {
                    return Make::Create {
                        repo: self.repo.clone(),
                        branch: branch.clone(),
                        from: self.from.clone(),
                        folder: folder_of(&self.name).to_owned(),
                        open: self.opens,
                    };
                }
            }
            KeyCode::Tab => {
                self.opens = match self.opens {
                    OpenKind::Shell => OpenKind::Claude,
                    OpenKind::Claude => OpenKind::Shell,
                };
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

/// The tabs of the box: the side in force is in brackets.
fn tabs(t: &Worktree) -> Vec<(String, Style)> {
    let mark = |side: Side, name: String| {
        if t.side == side {
            format!("[{name}]")
        } else {
            format!(" {name} ")
        }
    };
    let count = match &t.existing {
        Existing::Ready(list) => format!("existentes {}", list.len()),
        _ => "existentes".to_owned(),
    };
    vec![
        (
            format!(
                "{}  {}   ← → cambia",
                mark(Side::New, "nuevo".into()),
                mark(Side::Existing, count)
            ),
            Style::fg(look::fg()),
        ),
        (String::new(), Style::fg(look::fg())),
    ]
}

/// The lines of the side of the worktrees that exist.
fn existing_lines(t: &Worktree) -> Vec<(String, Style)> {
    let (plain, dim) = (Style::fg(look::fg()), Style::fg(look::dim()));
    let list = match &t.existing {
        Existing::Reading => return vec![("leyendo los worktrees…".to_owned(), dim)],
        Existing::Refused(why) => return vec![(why.clone(), Style::fg(look::red()))],
        Existing::Ready(list) => list,
    };
    if list.is_empty() {
        return vec![
            ("Este repositorio no tiene otros worktrees.".to_owned(), dim),
            (
                format!(
                    "El principal ya es el workspace de {}. Haz uno en «nuevo».",
                    repo_name(&t.repo)
                ),
                dim,
            ),
        ];
    }
    let mut v: Vec<(String, Style)> = list
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let glyph = if t.marked.contains(&w.path) {
                '■'
            } else if w.missing {
                '✕'
            } else if w.workspace.is_some() {
                '●'
            } else {
                '○'
            };
            let note = if w.workspace.is_some() {
                "abierto".to_owned()
            } else if w.missing {
                "carpeta no existe".to_owned()
            } else if w.branch.is_none() {
                "sin rama".to_owned()
            } else {
                match w.changes {
                    Some(n) if n > 0 => format!("{n} cambios"),
                    _ => String::new(),
                }
            };
            let style = if w.missing {
                Style::fg(look::red())
            } else if w.workspace.is_some() {
                dim
            } else if w.changes.unwrap_or(0) > 0 {
                Style::fg(look::yellow())
            } else {
                plain
            };
            // The note comes before the folder, which is the long one and the one that can be cut.
            let text = format!(
                "{} {glyph} {:<24} {note:<18} {}",
                if i == t.sel { '▸' } else { ' ' },
                clip(&shown(w), 24),
                short(Path::new(&w.path)),
            );
            (text, if i == t.sel { style.bold() } else { style })
        })
        .collect();
    let closed = list.iter().filter(|w| openable(w)).count();
    v.push((String::new(), plain));
    let marked = if t.marked.is_empty() {
        String::new()
    } else {
        format!("{} marcados · ", t.marked.len())
    };
    v.push((format!("{marked}{closed} cerrados"), dim));
    v
}

/// Draw the box of a worktree over `area`.
pub fn draw_worktree(g: &mut Grid, (x, y, w, _): Rect, t: &Worktree) {
    let mut lines = tabs(t);
    if t.side == Side::Existing {
        lines.extend(existing_lines(t));
        lines.push((String::new(), Style::fg(look::fg())));
        if let Some((text, good)) = &t.note {
            lines.push((
                text.clone(),
                Style::fg(if *good { look::green() } else { look::red() }),
            ));
        }
        lines.push((
            "↵ abrir · espacio marca · a abre todos · ← → nuevo · esc cierra".to_owned(),
            Style::fg(look::dim()),
        ));
    } else {
        lines.extend(new_lines(t));
    }
    draw_box(g, (x, y, w), t, &lines);
}

/// The lines of the side of a new worktree.
fn new_lines(t: &Worktree) -> Vec<(String, Style)> {
    {
        let plain = Style::fg(look::fg());
        let mut v = vec![
            (format!("nombre    {}▏", t.name), plain.bold()),
            (String::new(), plain),
        ];
        match (t.plan(), &t.config) {
            (Plan::Reading, _) => v.extend([
                ("leyendo .jw.toml…".to_owned(), Style::fg(look::dim())),
                (String::new(), plain),
                (String::new(), plain),
            ]),
            (Plan::Wrong(why), _) => v.extend([
                (why.clone(), Style::fg(look::red())),
                (String::new(), plain),
                (String::new(), plain),
            ]),
            (Plan::Ok { branch, folder }, config) => {
                v.extend([
                    (format!("rama      {branch}"), Style::fg(look::green())),
                    (format!("desde     {}", t.from), plain),
                    (format!("carpeta   {}", short(folder)), plain),
                ]);
                if let Config::Ready(info) = config {
                    for (i, line) in info.setup.iter().enumerate() {
                        let label = if i == 0 { "setup     " } else { "          " };
                        v.push((format!("{label}{line}"), Style::fg(look::yellow())));
                    }
                    let mark = |kind: OpenKind, name: &str| {
                        if t.opens == kind {
                            format!("[{name}]")
                        } else {
                            format!(" {name} ")
                        }
                    };
                    let after = if info.setup.is_empty() {
                        ""
                    } else {
                        "  tras el setup"
                    };
                    v.push((
                        format!(
                            "abre      {} {}{after}",
                            mark(OpenKind::Shell, "shell"),
                            mark(OpenKind::Claude, "claude")
                        ),
                        plain,
                    ));
                    if !info.copy.is_empty() {
                        v.push((format!("copia     {}", info.copy.join(" · ")), plain));
                    }
                    if !info.ports.is_empty() {
                        let names: Vec<&str> = info.ports.keys().map(String::as_str).collect();
                        v.push((format!("puertos   {}", names.join(" · ")), plain));
                    }
                    let source = match info.from {
                        Some(contract::ConfigFrom::Repo) => {
                            format!("según el .jw.toml de {}", repo_name(&t.repo))
                        }
                        Some(contract::ConfigFrom::Personal) => {
                            format!("según tu configuración de {}", repo_name(&t.repo))
                        }
                        None => "sin configuración: como siempre".to_owned(),
                    };
                    v.push((source, Style::fg(look::dim())));
                }
            }
        }
        v.push((String::new(), plain));
        let ok = matches!(t.plan(), Plan::Ok { .. });
        v.push((
            format!(
                "↵ crear{} · tab shell o claude · esc cancelar",
                if ok { " y abrir" } else { "" }
            ),
            Style::fg(look::dim()),
        ));
        v
    }
}

fn draw_box(
    g: &mut Grid,
    (x, y, w): (usize, usize, usize),
    t: &Worktree,
    lines: &[(String, Style)],
) {
    let widest = lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let title = match t.side {
        Side::New => format!("worktree nuevo de {}", repo_name(&t.repo)),
        Side::Existing => format!("worktrees de {}", repo_name(&t.repo)),
    };
    let bw = (widest + 8)
        .max(title.chars().count() + 8)
        .min(w.saturating_sub(2))
        .max(30);
    let bh = lines.len() + 4;
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 3);
    g.clear_box((bx, by, bw, bh));
    let border = if matches!(t.plan(), Plan::Wrong(_)) {
        look::red()
    } else {
        look::blue()
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

    /// What the daemon says of a repository with no `.jw.toml`: its worktrees beside it.
    fn defaults(root: &Path) -> ProjectInfo {
        ProjectInfo {
            source: None,
            from: None,
            personal: String::new(),
            copy: Vec::new(),
            ports: Default::default(),
            root: root.join("app-wt").to_string_lossy().into_owned(),
            root_text: None,
            setup: Vec::new(),
            open: OpenKind::Shell,
        }
    }

    /// A box whose repository's settings the daemon has told.
    fn ready(app: &Path, open: Vec<PathBuf>, info: ProjectInfo) -> Worktree {
        let mut t = Worktree::new(app.to_path_buf(), "main".into(), open);
        t.set_config(Ok(info));
        t
    }

    #[test]
    fn the_worktree_box_suggests_the_next_free_name_and_shows_what_it_will_make() {
        let root = tree("wt");
        let app = repo(&root);
        let t = ready(&app, vec![], defaults(&root));
        assert_eq!(t.name, "ws-1");
        let Plan::Ok { branch, folder } = t.plan() else {
            panic!("{:?}", t.plan())
        };
        assert_eq!(branch, "ws-1", "REQ-512: the branch is what is typed");
        assert_eq!(*folder, root.join("app-wt").join("ws-1"));
        // The first one is taken (open, on disk, or as a branch): the suggestion moves on.
        let t = ready(&app, vec![root.join("app-wt/ws-1")], defaults(&root));
        assert_eq!(t.name, "ws-2");
        std::fs::create_dir_all(root.join("app-wt/ws-2")).unwrap();
        assert!(git_out(&app, &["branch", "ws-3"]).is_some());
        let t = ready(&app, vec![root.join("app-wt/ws-1")], defaults(&root));
        assert_eq!(t.name, "ws-4");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typing_replaces_the_suggestion_and_says_why_a_name_cannot_be_used() {
        let root = tree("wt2");
        let app = repo(&root);
        std::fs::create_dir_all(root.join("app-wt/taken")).unwrap();
        assert!(git_out(&app, &["branch", "feat/old"]).is_some());
        let mut t = ready(&app, vec![root.join("app-wt/there")], defaults(&root));
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
            Plan::Ok { .. } | Plan::Reading => String::new(),
        };
        type_name(&mut t, "Bad Name");
        assert_eq!(
            why(&t),
            "nombre no válido: minúsculas, números y guiones, con / para un prefijo"
        );
        for bad in ["fix/", "/fix", "fix//x", "fix/-x", "FIX/x"] {
            type_name(&mut t, bad);
            assert!(why(&t).starts_with("nombre no válido"), "{bad}");
        }
        type_name(&mut t, "");
        assert!(
            why(&t).starts_with("nombre no válido"),
            "empty is not a name"
        );
        type_name(&mut t, "there");
        assert_eq!(why(&t), "app/there ya existe");
        type_name(&mut t, "taken");
        assert!(why(&t).ends_with("app-wt/taken ya existe"), "{}", why(&t));
        type_name(&mut t, "feat/old");
        assert_eq!(why(&t), "la rama feat/old ya existe");
        type_name(&mut t, "old");
        assert!(
            matches!(t.plan(), Plan::Ok { .. }),
            "`old` is not `feat/old`"
        );
        type_name(&mut t, "fix/there");
        assert_eq!(
            why(&t),
            "app/there ya existe",
            "the folder is the last part"
        );
        // Enter does nothing while it cannot be made, and asks for what the plan says when it can.
        assert_eq!(t.press(KeyCode::Enter, false), Make::Stay);
        type_name(&mut t, "fresh");
        assert_eq!(
            t.press(KeyCode::Enter, false),
            Make::Create {
                repo: app.clone(),
                branch: "fresh".into(),
                from: "main".into(),
                folder: "fresh".into(),
                open: OpenKind::Shell,
            }
        );
        type_name(&mut t, "fix/fresh");
        assert_eq!(
            t.press(KeyCode::Enter, false),
            Make::Create {
                repo: app,
                branch: "fix/fresh".into(),
                from: "main".into(),
                folder: "fresh".into(),
                open: OpenKind::Shell,
            }
        );
        assert_eq!(t.press(KeyCode::Esc, false), Make::Close);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn the_box_waits_for_the_daemon_and_enter_does_nothing_until_it_has_answered() {
        let root = tree("wt3");
        let app = repo(&root);
        let mut t = Worktree::new(app.clone(), "main".into(), vec![]);
        assert_eq!((t.plan(), &t.config), (&Plan::Reading, &Config::Reading));
        assert_eq!(t.press(KeyCode::Enter, false), Make::Stay);
        // Typing meanwhile is kept: the answer does not replace the name the person chose.
        t.press(KeyCode::Char('q'), false);
        assert_eq!(t.plan(), &Plan::Reading);
        t.set_config(Ok(defaults(&root)));
        assert_eq!(t.name, "q");
        assert!(matches!(t.plan(), Plan::Ok { .. }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_refusal_of_the_daemon_is_shown_in_place_of_the_plan_and_nothing_can_be_made() {
        let root = tree("wt4");
        let app = repo(&root);
        let mut t = Worktree::new(app, "main".into(), vec![]);
        t.set_config(Err(
            ".jw.toml, line 2: «{nme}» is no marker (use {name})".into()
        ));
        assert_eq!(
            t.plan(),
            &Plan::Wrong(".jw.toml, line 2: «{nme}» is no marker (use {name})".into())
        );
        assert_eq!(t.press(KeyCode::Enter, false), Make::Stay);
        t.press(KeyCode::Char('x'), false);
        assert!(matches!(t.plan(), Plan::Wrong(w) if w.contains(".jw.toml")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_folder_comes_from_the_settings_and_the_branch_is_what_is_typed_and_both_are_checked() {
        let root = tree("wt5");
        let app = repo(&root);
        let info = ProjectInfo {
            source: Some(app.join(".jw.toml").to_string_lossy().into_owned()),
            from: Some(contract::ConfigFrom::Repo),
            personal: String::new(),
            copy: vec![".env".into()],
            ports: [("web".to_owned(), 0), ("api".to_owned(), 1)].into(),
            root: root.join("trees").to_string_lossy().into_owned(),
            root_text: Some("../trees".into()),
            setup: vec!["bun install".into()],
            open: OpenKind::Claude,
        };
        let t = ready(&app, vec![], info.clone());
        let Plan::Ok { branch, folder } = t.plan() else {
            panic!("{:?}", t.plan())
        };
        assert_eq!(branch, "ws-1");
        assert_eq!(*folder, root.join("trees/ws-1"));
        // A branch of that name stops it, and one with a prefix does not.
        assert!(git_out(&app, &["branch", "feat/ws-1"]).is_some());
        assert!(git_out(&app, &["branch", "ws-1"]).is_some());
        std::fs::create_dir_all(root.join("trees/ws-2")).unwrap();
        let mut t = ready(&app, vec![], info);
        assert_eq!(
            t.name, "ws-3",
            "the branch of ws-1 and the folder of ws-2 are taken"
        );
        t.press(KeyCode::Char('x'), false);
        assert_eq!(
            t.press(KeyCode::Enter, false),
            Make::Create {
                repo: app,
                branch: "x".into(),
                from: "main".into(),
                folder: "x".into(),
                open: OpenKind::Claude,
            }
        );
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
