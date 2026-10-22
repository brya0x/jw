//! The project screen (`p`) of the terminal client (prototype: `docs/prototypes/project.html`,
//! REQ-520 to REQ-526): where a repository's settings come from and the simple ones, edited in
//! place. It only holds what the person is doing; what to save comes out as a value for `View`
//! to send, and the daemon puts it in the file without touching what else the file holds.

use std::path::{Path, PathBuf};

use contract::{ConfigFrom, OpenKind, ProjectEdit, ProjectInfo};
use crossterm::event::KeyCode;

use super::{Grid, Rect, Style, clip, erase, look};

/// What a key in the screen asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Stay,
    Close,
    /// Put `set` in the file `to` of the repository `repo`.
    Save {
        repo: PathBuf,
        to: ConfigFrom,
        set: ProjectEdit,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Root,
    Setup,
    Copy,
    Ports,
    Open,
}

const FIELDS: [(Field, &str); 5] = [
    (Field::Root, "carpeta de worktrees"),
    (Field::Setup, "setup"),
    (Field::Copy, "copiar"),
    (Field::Ports, "puertos"),
    (Field::Open, "al abrir"),
];

/// What the screen shows and edits.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Values {
    root: String,
    /// The root is written in a file, or was typed: when not, `root` is the default, as the
    /// daemon says it.
    root_set: bool,
    setup: Vec<String>,
    copy: Vec<String>,
    ports: Vec<(String, u32)>,
    open: OpenKind,
}

impl Values {
    fn of(info: &ProjectInfo) -> Values {
        Values {
            root: info.root_text.clone().unwrap_or_else(|| info.root.clone()),
            root_set: info.root_text.is_some(),
            setup: info.setup.clone(),
            copy: info.copy.clone(),
            ports: info.ports.iter().map(|(n, o)| (n.clone(), *o)).collect(),
            open: info.open,
        }
    }
}

/// How a note is told.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Good,
    Warn,
}

#[derive(Debug)]
struct Form {
    info: ProjectInfo,
    orig: Values,
    cur: Values,
    sel: usize,
    /// The item of a list that is chosen.
    sub: Option<usize>,
    /// The text being typed, for the root or for the item `sub` of a list.
    edit: Option<String>,
    /// The item being typed was just added: leaving it empty takes it away.
    adding: bool,
    to: ConfigFrom,
    /// `esc` was pressed with changes not saved.
    armed: bool,
}

#[derive(Debug)]
enum Load {
    Reading,
    Ready(Box<Form>),
    Refused(String),
}

/// The screen of one repository's settings.
#[derive(Debug)]
pub struct Project {
    pub repo: PathBuf,
    load: Load,
    note: Option<(String, Tone)>,
}

impl Project {
    pub fn new(repo: PathBuf) -> Project {
        Project {
            repo,
            load: Load::Reading,
            note: None,
        }
    }

    /// The daemon's answer to `project_config`: the screen shows it, whatever was there, with the
    /// person's note kept.
    pub fn set_info(&mut self, info: Result<ProjectInfo, String>) {
        // Reading again after a save keeps the field and the file the person was on.
        let before = match &self.load {
            Load::Ready(f) => Some((f.sel, f.to)),
            _ => None,
        };
        self.load = match info {
            Ok(info) => {
                let values = Values::of(&info);
                let to = before.map_or_else(
                    || {
                        if info.from == Some(ConfigFrom::Repo) {
                            ConfigFrom::Repo
                        } else {
                            ConfigFrom::Personal
                        }
                    },
                    |(_, to)| to,
                );
                Load::Ready(Box::new(Form {
                    to,
                    info,
                    orig: values.clone(),
                    cur: values,
                    sel: before.map_or(0, |(sel, _)| sel),
                    sub: None,
                    edit: None,
                    adding: false,
                    armed: false,
                }))
            }
            Err(why) => Load::Refused(why),
        };
    }

    /// What saving said: where it went, or why it did not.
    pub fn saved(&mut self, result: Result<(String, bool), String>) {
        self.note = Some(match result {
            Ok((path, true)) => (
                format!("guardado en {path} · queda tapado por el .jw.toml del repositorio"),
                Tone::Warn,
            ),
            Ok((path, false)) => (format!("guardado en {path}"), Tone::Good),
            Err(why) => (format!("no se pudo guardar: {why}"), Tone::Warn),
        });
    }

    /// Whether there is something typed or changed that a closing would lose.
    #[cfg(test)]
    fn dirty(&self) -> bool {
        matches!(&self.load, Load::Ready(f) if f.cur != f.orig)
    }

    /// Option-backspace: the word before the cursor of the line that is being typed.
    pub fn erase_word(&mut self) {
        if let Load::Ready(form) = &mut self.load
            && let Some(buf) = form.edit.as_mut()
        {
            erase::pop_word(buf);
        }
    }

    pub fn press(&mut self, code: KeyCode) -> Act {
        let Load::Ready(form) = &mut self.load else {
            return if code == KeyCode::Esc {
                Act::Close
            } else {
                Act::Stay
            };
        };
        self.note = None;
        match form.press(code, &mut self.note) {
            Step::Stay => Act::Stay,
            Step::Close => Act::Close,
            Step::Save => Act::Save {
                repo: self.repo.clone(),
                to: form.to,
                set: form.set(),
            },
        }
    }
}

enum Step {
    Stay,
    Close,
    Save,
}

fn warn(text: &str) -> Option<(String, Tone)> {
    Some((text.to_owned(), Tone::Warn))
}

impl Form {
    fn field(&self) -> Field {
        FIELDS[self.sel].0
    }

    fn list_len(&self) -> usize {
        match self.field() {
            Field::Setup => self.cur.setup.len(),
            Field::Copy => self.cur.copy.len(),
            Field::Ports => self.cur.ports.len(),
            Field::Root | Field::Open => 0,
        }
    }

    fn press(&mut self, code: KeyCode, note: &mut Option<(String, Tone)>) -> Step {
        if self.edit.is_some() {
            self.typing(code, note);
            return Step::Stay;
        }
        if self.sub.is_some() {
            self.in_list(code);
            return Step::Stay;
        }
        let armed = std::mem::take(&mut self.armed);
        match code {
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(FIELDS.len() - 1),
            KeyCode::Enter | KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if self.field() == Field::Open =>
            {
                self.cur.open = match self.cur.open {
                    OpenKind::Shell => OpenKind::Claude,
                    OpenKind::Claude => OpenKind::Shell,
                };
            }
            KeyCode::Enter => match self.field() {
                Field::Root => self.edit = Some(self.cur.root.clone()),
                _ if self.list_len() == 0 => self.add(),
                _ => self.sub = Some(0),
            },
            KeyCode::Char('g') => {
                self.to = match self.to {
                    ConfigFrom::Repo => ConfigFrom::Personal,
                    ConfigFrom::Personal => ConfigFrom::Repo,
                };
            }
            KeyCode::Char('s') => {
                if self.cur == self.orig && self.info.from == Some(self.to) {
                    *note = Some(("no hay nada que guardar".into(), Tone::Good));
                } else {
                    return Step::Save;
                }
            }
            KeyCode::Esc if self.cur != self.orig && !armed => {
                self.armed = true;
                *note = warn("hay cambios sin guardar: s guarda, esc otra vez descarta");
            }
            KeyCode::Esc => return Step::Close,
            _ => {}
        }
        Step::Stay
    }

    fn in_list(&mut self, code: KeyCode) {
        let Some(at) = self.sub else { return };
        let len = self.list_len();
        match code {
            KeyCode::Down => self.sub = Some((at + 1).min(len.saturating_sub(1))),
            KeyCode::Up => self.sub = Some(at.saturating_sub(1)),
            KeyCode::Enter if len > 0 => self.edit = Some(self.item(at)),
            KeyCode::Char('a') => self.add(),
            KeyCode::Char('d') if len > 0 => {
                self.remove(at);
                let len = self.list_len();
                self.sub = (len > 0).then(|| at.min(len - 1));
            }
            KeyCode::Esc => self.sub = None,
            _ => {}
        }
    }

    /// The item of the list as it is typed: a port is `name=offset`.
    fn item(&self, at: usize) -> String {
        match self.field() {
            Field::Setup => self.cur.setup.get(at).cloned().unwrap_or_default(),
            Field::Copy => self.cur.copy.get(at).cloned().unwrap_or_default(),
            Field::Ports => self
                .cur
                .ports
                .get(at)
                .map(|(n, o)| format!("{n}={o}"))
                .unwrap_or_default(),
            Field::Root | Field::Open => String::new(),
        }
    }

    fn remove(&mut self, at: usize) {
        match self.field() {
            Field::Setup if at < self.cur.setup.len() => drop(self.cur.setup.remove(at)),
            Field::Copy if at < self.cur.copy.len() => drop(self.cur.copy.remove(at)),
            Field::Ports if at < self.cur.ports.len() => drop(self.cur.ports.remove(at)),
            _ => {}
        }
    }

    /// A new item at the end, typed at once.
    fn add(&mut self) {
        let next = (0..100)
            .find(|o| self.cur.ports.iter().all(|(_, p)| p != o))
            .unwrap_or(0);
        let text = match self.field() {
            Field::Setup => {
                self.cur.setup.push(String::new());
                String::new()
            }
            Field::Copy => {
                self.cur.copy.push(String::new());
                String::new()
            }
            Field::Ports => {
                self.cur.ports.push((String::new(), next));
                String::new()
            }
            Field::Root | Field::Open => return,
        };
        self.sub = Some(self.list_len() - 1);
        self.adding = true;
        self.edit = Some(text);
    }

    fn typing(&mut self, code: KeyCode, note: &mut Option<(String, Tone)>) {
        match code {
            KeyCode::Char(c) => {
                if let Some(buf) = self.edit.as_mut() {
                    buf.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.edit.as_mut() {
                    buf.pop();
                }
            }
            KeyCode::Esc => {
                self.edit = None;
                if std::mem::take(&mut self.adding)
                    && let Some(at) = self.sub
                {
                    self.remove(at);
                    let len = self.list_len();
                    self.sub = (len > 0).then(|| at.min(len - 1));
                }
            }
            KeyCode::Enter => {
                let text = self.edit.clone().unwrap_or_default();
                match self.commit(&text) {
                    Ok(()) => {
                        self.edit = None;
                        self.adding = false;
                    }
                    Err(why) => *note = warn(&why),
                }
            }
            _ => {}
        }
    }

    /// What was typed is the value: an empty root is the default one, an empty line of a list is
    /// no line.
    fn commit(&mut self, text: &str) -> Result<(), String> {
        let text = text.trim();
        match (self.field(), self.sub) {
            (Field::Root, _) => {
                if text.is_empty() {
                    self.cur.root = self.info.root.clone();
                    self.cur.root_set = false;
                } else {
                    self.cur.root = text.to_owned();
                    self.cur.root_set = true;
                }
            }
            (Field::Setup | Field::Copy, Some(at)) => {
                let list = if self.field() == Field::Setup {
                    &mut self.cur.setup
                } else {
                    &mut self.cur.copy
                };
                match list.get_mut(at) {
                    Some(_) if text.is_empty() => drop(list.remove(at)),
                    Some(item) => *item = text.to_owned(),
                    None => {}
                }
                let len = list.len();
                self.sub = (len > 0).then(|| at.min(len - 1));
            }
            (Field::Ports, Some(at)) => {
                let (name, offset) = text.split_once('=').unwrap_or((text, ""));
                let name = name.trim();
                let offset = offset.trim().parse::<u32>().ok().filter(|o| *o <= 99);
                let name_ok = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                let Some(offset) = offset.filter(|_| name_ok && name != "base") else {
                    return Err(
                        "puerto: nombre en minúsculas, números y _ (no «base»), y un desplazamiento de 0 a 99"
                            .into(),
                    );
                };
                if self
                    .cur
                    .ports
                    .iter()
                    .enumerate()
                    .any(|(i, (n, _))| i != at && n == name)
                {
                    return Err(format!("el puerto {name} ya está"));
                }
                if let Some(port) = self.cur.ports.get_mut(at) {
                    *port = (name.to_owned(), offset);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// What to send: the fields that changed, and all of them when the file saved to is not the
    /// one the settings are read from, so that it says what the screen shows (REQ-524).
    fn set(&self) -> ProjectEdit {
        let all = self.info.from != Some(self.to);
        let (cur, orig) = (&self.cur, &self.orig);
        let root = if cur.root_set && (all || cur.root != orig.root) {
            Some(cur.root.clone())
        } else if !cur.root_set && orig.root_set {
            Some(String::new())
        } else {
            None
        };
        ProjectEdit {
            root,
            setup: (all || cur.setup != orig.setup).then(|| cur.setup.clone()),
            copy: (all || cur.copy != orig.copy).then(|| cur.copy.clone()),
            ports: (all || cur.ports != orig.ports).then(|| cur.ports.iter().cloned().collect()),
            open: (all || cur.open != orig.open).then_some(cur.open),
        }
    }

    fn target(&self, repo: &Path) -> String {
        match self.to {
            ConfigFrom::Repo => format!("{} (lo ve el equipo)", repo.join(".jw.toml").display()),
            ConfigFrom::Personal => self.info.personal.clone(),
        }
    }

    fn warning(&self) -> &'static str {
        match (self.to, self.info.from) {
            (ConfigFrom::Personal, Some(ConfigFrom::Repo)) => {
                "quedaría tapada por el .jw.toml del repositorio"
            }
            (ConfigFrom::Repo, Some(ConfigFrom::Repo)) => "cambia el .jw.toml que ve el equipo",
            (ConfigFrom::Repo, _) => "crea un .jw.toml en el repositorio",
            (ConfigFrom::Personal, _) => "",
        }
    }

    fn is_default(&self, field: Field) -> bool {
        match field {
            Field::Root => !self.cur.root_set,
            Field::Setup => self.cur.setup.is_empty(),
            Field::Copy => self.cur.copy.is_empty(),
            Field::Ports => self.cur.ports.is_empty(),
            Field::Open => self.cur.open == OpenKind::Shell,
        }
    }

    fn values(&self, field: Field) -> Vec<String> {
        match field {
            Field::Root => vec![self.cur.root.clone()],
            Field::Setup => self.cur.setup.clone(),
            Field::Copy => self.cur.copy.clone(),
            Field::Ports => self
                .cur
                .ports
                .iter()
                .map(|(n, o)| format!("{n} +{o}"))
                .collect(),
            Field::Open => {
                let mark = |kind: OpenKind, name: &str| {
                    if self.cur.open == kind {
                        format!("[{name}]")
                    } else {
                        format!(" {name} ")
                    }
                };
                vec![format!(
                    "{} {}",
                    mark(OpenKind::Shell, "shell"),
                    mark(OpenKind::Claude, "claude")
                )]
            }
        }
    }
}

/// Draw the screen over `area`.
pub fn draw(g: &mut Grid, (x, y, w, _): Rect, p: &Project) {
    let plain = Style::fg(look::fg());
    let dim = Style::fg(look::dim());
    let mut lines: Vec<(String, Style)> = Vec::new();
    match &p.load {
        Load::Reading => lines.push(("leyendo la configuración…".into(), dim)),
        Load::Refused(why) => lines.push((why.clone(), Style::fg(look::red()))),
        Load::Ready(f) => {
            let (from, style, why) = match f.info.from {
                Some(ConfigFrom::Repo) => (
                    format!(
                        "del repositorio · {}",
                        f.info.source.as_deref().unwrap_or("")
                    ),
                    Style::fg(look::yellow()),
                    "manda sobre tu archivo personal; el equipo lo ve",
                ),
                Some(ConfigFrom::Personal) => (
                    format!("personal · {}", f.info.source.as_deref().unwrap_or("")),
                    Style::fg(look::green()),
                    "está fuera del repositorio; todos tus worktrees lo comparten",
                ),
                None => (
                    "ninguna: los valores de siempre".into(),
                    dim,
                    "guardar crea tu archivo personal; el repositorio no se toca",
                ),
            };
            lines.push((format!("configuración  {from}"), style));
            lines.push((format!("               {why}"), dim));
            lines.push((String::new(), plain));
            for (i, (field, label)) in FIELDS.iter().enumerate() {
                let on = i == f.sel;
                let values = f.values(*field);
                let typing = on.then_some(f.edit.as_ref()).flatten();
                let listing = on && f.sub.is_some();
                let show = |k: usize, v: &str| match typing {
                    Some(buf) if *field == Field::Root || f.sub == Some(k) => format!("{buf}▏"),
                    _ if listing => {
                        format!("{}{v}", if f.sub == Some(k) { "› " } else { "  " })
                    }
                    _ => v.to_owned(),
                };
                let first = show(0, values.first().map_or("—", String::as_str));
                let mark = if on { "▸" } else { " " };
                let tail = if f.is_default(*field) && f.edit.is_none() {
                    "   (por defecto)"
                } else if *field == Field::Open && on && f.edit.is_none() {
                    "   ← → o ↵ cambia"
                } else {
                    ""
                };
                let row = Style::fg(look::fg()).on(if on { look::sel() } else { look::panel() });
                lines.push((format!("{mark} {label:<22}{first}{tail}"), row));
                for (k, v) in values.iter().enumerate().skip(1) {
                    lines.push((format!("  {:<22}{}", "", show(k, v)), row));
                }
            }
            lines.push((String::new(), plain));
            let (personal, repo) = match f.to {
                ConfigFrom::Personal => ("[personal]", " repositorio "),
                ConfigFrom::Repo => (" personal ", "[repositorio]"),
            };
            lines.push((format!("guardar en   {personal} {repo}"), plain));
            lines.push((format!("             {}", f.target(&p.repo)), dim));
            let warning = f.warning();
            lines.push((
                if warning.is_empty() {
                    String::new()
                } else {
                    format!("⚠ {warning}")
                },
                Style::fg(look::yellow()),
            ));
            let status = if f.cur != f.orig {
                (
                    "● hay cambios sin guardar".to_owned(),
                    Style::fg(look::yellow()),
                )
            } else {
                ("sin cambios".to_owned(), dim)
            };
            lines.push(status);
        }
    }
    if let Some((text, tone)) = &p.note {
        lines.push((
            text.clone(),
            Style::fg(if *tone == Tone::Good {
                look::green()
            } else {
                look::yellow()
            }),
        ));
    }
    lines.push((String::new(), plain));
    let hint = match &p.load {
        Load::Ready(f) if f.edit.is_some() && f.field() == Field::Ports => {
            "nombre=desplazamiento (0 a 99) · ↵ lo acepta · esc lo deja como estaba"
        }
        Load::Ready(f) if f.edit.is_some() => {
            "escribes el valor · ↵ lo acepta · esc lo deja como estaba"
        }
        Load::Ready(f) if f.sub.is_some() => {
            "↑↓ elegir · ↵ editar · a añadir · d quitar · esc volver"
        }
        Load::Ready(_) => "↑↓ campo · ↵ editar · g dónde guarda · s guardar · esc cerrar",
        _ => "esc cerrar",
    };
    lines.push((hint.to_owned(), dim));

    let widest = lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let title = format!("proyecto · {}", super::open::repo_name(&p.repo));
    let bw = (widest + 8)
        .max(title.chars().count() + 8)
        .max(64)
        .min(w.saturating_sub(2))
        .max(30);
    let bh = lines.len() + 4;
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 1);
    g.clear_box((bx, by, bw, bh));
    let border = if matches!(p.load, Load::Refused(_)) {
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
        let text = clip(text, bw - 6);
        let pad = " ".repeat((bw - 6).saturating_sub(text.chars().count()));
        g.put(
            bx + 3,
            by + 2 + i,
            &format!("{text}{pad}"),
            if style.bg.is_some() {
                *style
            } else {
                style.on(look::panel())
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(from: Option<ConfigFrom>) -> ProjectInfo {
        ProjectInfo {
            source: from.map(|_| "/p/.jw.toml".into()),
            from,
            personal: "/c/projects/app.toml".into(),
            root: "/code/app-wt".into(),
            root_text: None,
            setup: vec!["make deps".into()],
            open: OpenKind::Shell,
            copy: vec![],
            ports: [("web".to_owned(), 0)].into(),
        }
    }

    fn ready(from: Option<ConfigFrom>) -> Project {
        let mut p = Project::new("/code/app".into());
        p.set_info(Ok(info(from)));
        p
    }

    fn keys(p: &mut Project, text: &str) {
        for c in text.chars() {
            p.press(KeyCode::Char(c));
        }
    }

    #[test]
    fn it_waits_for_the_daemon_and_shows_a_refusal_in_place_of_the_fields() {
        let mut p = Project::new("/code/app".into());
        assert_eq!(
            p.press(KeyCode::Char('s')),
            Act::Stay,
            "nothing to save yet"
        );
        assert_eq!(p.press(KeyCode::Esc), Act::Close);
        p.set_info(Err(".jw.toml, line 2: bad".into()));
        assert_eq!(p.press(KeyCode::Down), Act::Stay);
        assert_eq!(p.press(KeyCode::Esc), Act::Close);
    }

    #[test]
    fn what_is_changed_is_all_that_is_sent_to_the_file_the_settings_came_from() {
        let mut p = ready(Some(ConfigFrom::Personal));
        // `al abrir` toggles with ↵ and the arrows
        for _ in 0..4 {
            p.press(KeyCode::Down);
        }
        p.press(KeyCode::Enter);
        // a line of the setup, added
        for _ in 0..3 {
            p.press(KeyCode::Up);
        }
        p.press(KeyCode::Enter);
        keys(&mut p, "a");
        keys(&mut p, "make seed");
        p.press(KeyCode::Enter);
        p.press(KeyCode::Esc);
        assert!(p.dirty());
        let Act::Save { repo, to, set } = p.press(KeyCode::Char('s')) else {
            panic!("s saves")
        };
        assert_eq!((repo, to), ("/code/app".into(), ConfigFrom::Personal));
        assert_eq!(
            set,
            ProjectEdit {
                setup: Some(vec!["make deps".into(), "make seed".into()]),
                open: Some(OpenKind::Claude),
                ..Default::default()
            },
            "the root, the copies and the ports did not change (REQ-524)"
        );
    }

    #[test]
    fn saving_in_another_file_sends_everything_the_screen_shows() {
        let mut p = ready(Some(ConfigFrom::Repo));
        p.press(KeyCode::Char('g'));
        let Act::Save { to, set, .. } = p.press(KeyCode::Char('s')) else {
            panic!("a new file is made even with nothing changed")
        };
        assert_eq!(to, ConfigFrom::Personal);
        assert_eq!(set.setup, Some(vec!["make deps".into()]));
        assert_eq!(set.ports, Some([("web".to_owned(), 0)].into()));
        assert_eq!(set.open, Some(OpenKind::Shell));
        assert_eq!(set.root, None, "a root nobody wrote is not written");
    }

    #[test]
    fn nothing_changed_in_the_file_it_came_from_says_so_and_sends_nothing() {
        let mut p = ready(Some(ConfigFrom::Personal));
        assert_eq!(p.press(KeyCode::Char('s')), Act::Stay);
        assert_eq!(p.note.as_ref().unwrap().0, "no hay nada que guardar");
        // with no file at all, saving makes the personal one
        let mut p = ready(None);
        assert!(matches!(p.press(KeyCode::Char('s')), Act::Save { .. }));
    }

    #[test]
    fn the_root_is_typed_and_an_empty_one_goes_back_to_the_default() {
        let mut p = ready(Some(ConfigFrom::Personal));
        p.press(KeyCode::Enter);
        for _ in 0..40 {
            p.press(KeyCode::Backspace);
        }
        keys(&mut p, "../x");
        p.press(KeyCode::Enter);
        let Act::Save { set, .. } = p.press(KeyCode::Char('s')) else {
            panic!()
        };
        assert_eq!(set.root.as_deref(), Some("../x"));
        // emptied, it is the default again and nothing is written for it
        p.press(KeyCode::Enter);
        for _ in 0..40 {
            p.press(KeyCode::Backspace);
        }
        p.press(KeyCode::Enter);
        let Load::Ready(f) = &p.load else { panic!() };
        assert_eq!(
            (f.cur.root.as_str(), f.cur.root_set),
            ("/code/app-wt", false)
        );
        assert!(!p.dirty());
    }

    #[test]
    fn ports_are_checked_when_typed_and_a_bad_one_stays_in_the_line() {
        let mut p = ready(Some(ConfigFrom::Personal));
        for _ in 0..3 {
            p.press(KeyCode::Down);
        }
        p.press(KeyCode::Enter);
        keys(&mut p, "a");
        for bad in ["=5", "base=1", "web=2", "Api=3", "api=100", "api"] {
            for _ in 0..12 {
                p.press(KeyCode::Backspace);
            }
            keys(&mut p, bad);
            p.press(KeyCode::Enter);
            let Load::Ready(f) = &p.load else { panic!() };
            assert!(f.edit.is_some(), "{bad} is not accepted");
            assert!(p.note.is_some(), "{bad} says why");
        }
        for _ in 0..12 {
            p.press(KeyCode::Backspace);
        }
        keys(&mut p, "api=1");
        p.press(KeyCode::Enter);
        let Load::Ready(f) = &p.load else { panic!() };
        assert_eq!(f.cur.ports, [("web".into(), 0), ("api".into(), 1)]);
    }

    #[test]
    fn esc_while_typing_leaves_what_was_and_a_new_empty_item_goes_away() {
        let mut p = ready(Some(ConfigFrom::Personal));
        p.press(KeyCode::Down);
        p.press(KeyCode::Enter);
        keys(&mut p, "a");
        keys(&mut p, "nada");
        p.press(KeyCode::Esc);
        let Load::Ready(f) = &p.load else { panic!() };
        assert_eq!(f.cur.setup, ["make deps"]);
        assert!(f.edit.is_none());
        // an item that is edited to nothing is taken away
        p.press(KeyCode::Enter);
        p.press(KeyCode::Backspace);
        let Load::Ready(f) = &mut p.load else {
            panic!()
        };
        f.edit = Some(String::new());
        p.press(KeyCode::Enter);
        let Load::Ready(f) = &p.load else { panic!() };
        assert!(f.cur.setup.is_empty());
    }

    #[test]
    fn esc_with_changes_asks_once_and_the_second_esc_discards() {
        let mut p = ready(Some(ConfigFrom::Personal));
        for _ in 0..4 {
            p.press(KeyCode::Down);
        }
        p.press(KeyCode::Right);
        assert_eq!(p.press(KeyCode::Esc), Act::Stay);
        assert!(p.note.as_ref().unwrap().0.contains("sin guardar"));
        assert_eq!(p.press(KeyCode::Esc), Act::Close);
        // any other key between them disarms it
        let mut p = ready(Some(ConfigFrom::Personal));
        for _ in 0..4 {
            p.press(KeyCode::Down);
        }
        p.press(KeyCode::Right);
        p.press(KeyCode::Esc);
        p.press(KeyCode::Up);
        assert_eq!(p.press(KeyCode::Esc), Act::Stay);
    }

    #[test]
    fn a_save_that_worked_or_failed_is_told_and_the_new_settings_replace_the_old() {
        let mut p = ready(Some(ConfigFrom::Repo));
        p.saved(Ok(("/p/.jw.toml".into(), false)));
        assert_eq!(p.note, Some(("guardado en /p/.jw.toml".into(), Tone::Good)));
        p.saved(Ok(("/c/a.toml".into(), true)));
        assert!(p.note.as_ref().unwrap().0.contains("tapado"));
        p.saved(Err("disk full".into()));
        assert!(p.note.as_ref().unwrap().0.contains("disk full"));
        let mut after = info(Some(ConfigFrom::Repo));
        after.open = OpenKind::Claude;
        p.set_info(Ok(after));
        assert!(!p.dirty(), "what was saved is what the file says now");
    }

    #[test]
    fn option_backspace_takes_a_word_of_the_line_being_typed() {
        let mut p = ready(Some(ConfigFrom::Personal));
        p.press(KeyCode::Enter);
        for _ in 0..40 {
            p.press(KeyCode::Backspace);
        }
        keys(&mut p, "../a b");
        p.erase_word();
        p.press(KeyCode::Enter);
        let Act::Save { set, .. } = p.press(KeyCode::Char('s')) else {
            panic!()
        };
        assert_eq!(
            set.root.as_deref(),
            Some("../a"),
            "the line is trimmed when it is kept"
        );
    }
}
