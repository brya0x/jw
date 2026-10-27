//! The skillset screen of the terminal client (N185; REQ-1031 to REQ-1045): the catalog of
//! `jw skillset ls` as a list, and the keys that activate and remove what is in it. `Screen` is a pure
//! state: a key gives back what must be done (`Act`) and what the thread answered (`Reply`) is applied
//! to it, so the keys, the sheet and the lines are tested with no terminal. `Worker` is the thread's
//! side: it calls the library the CLI calls (`agent::skillset`), in the client's process, with no
//! daemon; the prototype is `docs/prototypes/skillset-screen.html`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use contract::themes::Rgb;
use crossterm::event::KeyCode;

use super::{Grid, Rect, Style, clip, look, shell};
use crate::agent::skillset::{self, Ask, Event, Item, Place, Sheet, Step};

/// The groups of the list, in order: the title and the `kind` of the library.
const GROUPS: [(&str, &str); 3] = [
    ("Skills", "skill"),
    ("Roles", "role"),
    ("Plugins", "plugin"),
];

/// What the person is told when a write cannot be stopped (REQ-1042).
const NO_CANCEL: &str = "no se puede cancelar: ya se está escribiendo";

const KEYS_LIST: &str = "j k mover · espacio activa o quita · u actualiza · r revisa GitHub · ↵ ficha · c copia el comando · esc vuelve";
const KEYS_SHEET: &str = "y instala · n, ↵ o esc no instalan (el defecto es no)";
const KEYS_CARD: &str = "esc o ↵ cierran la ficha";

/// What the screen wants done, for the loop to give to the `Worker` (or to the client).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    /// Leave the screen.
    Close,
    /// Read the list again; `check` also asks GitHub (`ls --check`).
    Read {
        check: bool,
    },
    /// `skillset on <name>` up to the question.
    On(String),
    Off(String),
    /// The yes of the sheet.
    Apply,
    /// The no of the sheet: what was fetched is dropped.
    Decline,
    /// Stop the download.
    Cancel,
    /// Text for the clipboard.
    Copy(String),
}

/// What the thread answers.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Items {
        result: Result<(Vec<Item>, Vec<String>), String>,
        checked: bool,
    },
    /// What the CLI would have printed, or why it did not.
    Done(Result<Vec<String>, String>),
    /// Something third-party is fetched and waits for the yes.
    Asked(Sheet),
    /// A step of the work that has not answered yet (N186; REQ-1047).
    Step(Event),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Running {
    Reading,
    Checking,
    Fetching,
    Writing,
    Removing,
    /// The commands of a plugin.
    Installing,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Busy {
    running: Running,
    what: String,
    /// The entry the work is on, whose row shows it (REQ-1046).
    name: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tone {
    Ok,
    Err,
    Info,
}

#[derive(Clone, PartialEq, Debug)]
enum Mode {
    List,
    Sheet(Sheet),
    Card,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    Embedded,
    Github,
    Plugin,
}

fn src(item: &Item) -> Src {
    if item.source.starts_with("github") {
        Src::Github
    } else if item.source.starts_with("plugin") {
        Src::Plugin
    } else {
        Src::Embedded
    }
}

/// The glyph, the word and the colour of a state (REQ-1033): the CLI's words are English, the
/// screen's are Spanish.
fn chip(state: &str) -> (&'static str, &'static str, Rgb) {
    match state {
        "available" => ("○", "disponible", look::dim()),
        "installed" => ("✓", "instalado", look::green()),
        "outdated" => ("↑", "desactualizado", look::yellow()),
        "modified" => ("✎", "modificado", look::magenta()),
        "other tool" => ("⇄", "otra herramienta", look::cyan()),
        "built-in" => ("●", "incluido", look::blue()),
        _ => ("?", "desconocido", look::red()),
    }
}

/// What a state means for the row under the cursor, in a line or two.
fn why(item: &Item) -> String {
    let mine = if item.yours {
        " Es una entrada tuya (skillset.toml): su fuente es texto de un tercero."
    } else {
        ""
    };
    let bin = &identity::get().name;
    let text = match (item.state, src(item)) {
        ("available", Src::Plugin) => "No está en claude plugin list. Activar corre claude plugin install tras un sí.".to_owned(),
        ("available", Src::Github) => "No está. Activar baja el repo a un temporal, enseña de dónde viene y qué escribirá, y pide un sí.".to_owned(),
        ("available", _) => format!("No está escrito. Activar lo escribe: es texto de {bin}, sin pregunta."),
        ("installed", Src::Github) => format!(
            "Lo escribió {bin} (commit {}). El estado no mira la red hasta que pulses r.",
            item.commit.as_deref().map_or("?", |c| &c[..c.len().min(7)])
        ),
        ("installed", Src::Plugin) => format!("claude plugin list lo trae. {bin} no lo desinstala: dice el comando."),
        ("installed", _) => format!("Lo escribió {bin} y sigue igual."),
        ("outdated", Src::Github) => "El repo tiene un commit nuevo. Actualizar baja, enseña y pregunta otra vez.".to_owned(),
        ("outdated", _) => format!("{bin} trae un texto más nuevo que el escrito. Actualizar escribe el nuevo."),
        ("modified", Src::Github) => format!("Lo que {bin} escribió tiene cambios tuyos. No lo pisa: actualizar se niega y quitar solo borra lo que no editaste."),
        ("modified", _) => "El archivo tiene cambios tuyos: se deja el tuyo y se escribe SKILL.md.new al lado.".to_owned(),
        ("other tool", _) => format!("Hay un enlace (p. ej. de la herramienta «skills») o una carpeta que {bin} no escribió. No la toca: para usar la de {bin} quita esa antes, con la herramienta que la puso."),
        ("built-in", _) => format!("Viene dentro de {bin}: no hay nada que activar. Se usa con {bin} role show {}.", item.name),
        _ => "claude no contestó (¿no está en el PATH?): no se sabe si está instalado.".to_owned(),
    };
    format!("{text}{mine}")
}

/// The commands of the CLI that apply to a row, the first being its main action (REQ-1032).
fn commands(item: &Item) -> Vec<(String, &'static str)> {
    let bin = &identity::get().name;
    let (n, id) = (
        &item.name,
        item.source.strip_prefix("plugin ").unwrap_or(""),
    );
    let on = |why: &'static str| (format!("{bin} skillset on {n}"), why);
    let off = |why: &'static str| (format!("{bin} skillset off {n}"), why);
    match (item.state, src(item)) {
        ("built-in", _) => vec![(format!("{bin} role show {n}"), "ver el texto del rol")],
        ("available", Src::Plugin) => vec![
            on("muestra los comandos de claude y pregunta [y/N]"),
            (format!("{bin} skillset on {n} --yes"), "sin preguntar"),
        ],
        ("installed", Src::Plugin) => vec![
            off("NO desinstala: imprime el comando"),
            (format!("claude plugin uninstall {id}"), "lo que corres tú"),
        ],
        ("available", Src::Github) => vec![
            on("baja, enseña de dónde y qué escribe, pregunta [y/N]"),
            (format!("{bin} skillset on {n} --yes"), "sin preguntar"),
        ],
        ("installed", Src::Github) => vec![
            off("quita lo que escribió, si no lo editaste"),
            (
                format!("{bin} skillset ls --check"),
                "pregunta a GitHub si hay commit nuevo",
            ),
        ],
        ("outdated", Src::Github) => vec![
            on("baja el commit nuevo, enseña, pregunta [y/N]"),
            off("quita"),
        ],
        ("modified", Src::Github) => vec![
            on("se niega: lo editaste"),
            off("quita solo lo que no editaste"),
        ],
        ("available", _) => vec![on("escribe el texto; sin pregunta")],
        ("installed", _) => vec![off("quita el archivo si no lo editaste")],
        ("outdated", _) => vec![on("actualiza"), off("quita")],
        ("modified", _) => vec![
            on("deja el tuyo y escribe SKILL.md.new al lado"),
            off("no borra lo editado"),
        ],
        ("unknown", _) => vec![(
            "claude plugin list --json".into(),
            "lo que se le pregunta a claude",
        )],
        _ => Vec::new(),
    }
}

/// A line of the sheet and how it is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Style2 {
    Plain,
    Dim,
    Bold,
    File,
    Command,
    Warn,
}

/// The lines of the sheet: the CLI's confirmation (REQ-1022, REQ-1012) drawn, with the question.
fn sheet_lines(sheet: &Sheet) -> Vec<(String, Style2)> {
    let mut l = Vec::new();
    match sheet {
        Sheet::Github {
            url,
            branch,
            commit,
            from,
            files,
            scripts,
            ..
        } => {
            l.push((
                format!(
                    "Código de un tercero, no de {}. Se baja de:",
                    identity::get().name
                ),
                Style2::Plain,
            ));
            l.push((format!("  {url}"), Style2::Bold));
            l.push((format!("  rama {branch} · commit {commit}"), Style2::Bold));
            if let Some(from) = from {
                l.push((
                    format!("  ahora tienes el commit {}", &from[..from.len().min(7)]),
                    Style2::Dim,
                ));
            }
            l.push((String::new(), Style2::Plain));
            l.push((
                format!(
                    "Escribirá {} archivo{}:",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
                ),
                Style2::Plain,
            ));
            for (path, bytes) in files {
                l.push((format!("  {path} ({bytes} bytes)"), Style2::File));
            }
            if !scripts.is_empty() {
                l.push((String::new(), Style2::Plain));
                l.push((
                    format!(
                        "! La carpeta trae archivos que no son Markdown ({}):",
                        scripts.join(", ")
                    ),
                    Style2::Warn,
                ));
                l.push(("  un skill puede traer scripts.".to_owned(), Style2::Warn));
            }
        }
        Sheet::Plugin {
            plugin,
            repo,
            marketplace,
            commands,
        } => {
            l.push((format!("Plugin de un tercero: {plugin}"), Style2::Bold));
            l.push((
                format!("del repo de GitHub {repo} (marketplace {marketplace})"),
                Style2::Plain,
            ));
            l.push((String::new(), Style2::Plain));
            l.push((
                "Correrá en tu máquina, en este orden:".to_owned(),
                Style2::Plain,
            ));
            for c in commands {
                l.push((format!("  $ claude {c}"), Style2::Command));
            }
            l.push((String::new(), Style2::Plain));
            l.push((
                "! Es código de un tercero: puede correr programas en tu máquina por sus hooks."
                    .to_owned(),
                Style2::Warn,
            ));
        }
    }
    l.push((String::new(), Style2::Plain));
    l.push((
        "  y  instalar     n ↵ esc  no instalar (el defecto es no)".to_owned(),
        Style2::Bold,
    ));
    l
}

/// What the CLI says when nobody said yes.
const NOT_SAID_YES: &str = "not installed: you did not say yes";

fn archivos(n: usize) -> String {
    format!("{n} archivo{}", if n == 1 { "" } else { "s" })
}

/// A line of the library in Spanish, with the tone it has (REQ-1049, REQ-1050); `ran`: the work ran
/// the commands of a plugin. `None` for a line that has no Spanish form.
fn spanish(line: &str, ran: bool) -> Option<(Tone, String)> {
    let bin = &identity::get().name;
    let ok = |s: String| Some((Tone::Ok, s));
    let info = |s: String| Some((Tone::Info, s));
    if let Some(rest) = line.strip_prefix("installed ") {
        let github = rest
            .split_once(" file(s) in ")
            .and_then(|(left, dir)| left.rsplit_once("): ").map(|(head, n)| (head, n, dir)));
        return match github {
            Some((head, n, dir)) => {
                let n = n.parse().unwrap_or(0);
                ok(format!("instalado {head}): {} en {dir}", archivos(n)))
            }
            None => ok(format!("instalado {rest}")),
        };
    }
    if let Some(rest) = line.strip_prefix("unchanged ") {
        return match rest.split_once(" (commit ") {
            Some((name, commit)) => info(format!(
                "{name} ya está en el commit {}: sin cambios",
                commit.trim_end_matches(')')
            )),
            None => info(format!("sin cambios: {rest}")),
        };
    }
    if let Some(rest) = line.strip_prefix("removed ") {
        return match rest.split_once(" file(s) of ") {
            Some((n, name)) => ok(format!(
                "quitado {name} ({})",
                archivos(n.parse().unwrap_or(0))
            )),
            None => ok(format!("quitado {rest}")),
        };
    }
    if let Some(rest) = line.strip_prefix("kept ") {
        if let Some(name) = rest.strip_suffix(": it was edited") {
            return info(format!(
                "{name}: tiene cambios tuyos; no se borra lo que editaste"
            ));
        }
        let mark = format!(": not written by {bin} or edited");
        if let Some((path, new)) = rest.split_once(&format!("{mark}; the new one is ")) {
            return info(format!(
                "{path}: no lo escribió {bin} o lo editaste; se deja el tuyo y el nuevo está en {new}"
            ));
        }
        if let Some(path) = rest.strip_suffix(&mark) {
            return info(format!(
                "{path}: no lo escribió {bin} o lo editaste; no se borra"
            ));
        }
    }
    if let Some(name) = line.strip_suffix(" is not installed") {
        return info(format!("{name} no está instalado"));
    }
    if let Some((name, rest)) = line.split_once(" is built in: ") {
        let said = match rest.strip_prefix("always available with ") {
            Some(command) => format!("siempre disponible con {command}"),
            None => "no hay nada que quitar".to_owned(),
        };
        return info(format!("{name} viene dentro de {bin}: {said}"));
    }
    if let Some(command) = line.strip_prefix(&format!(
        "{bin} does not uninstall plugins; to remove it run: "
    )) {
        return info(format!(
            "{bin} no desinstala plugins; para quitarlo corre: {command}"
        ));
    }
    if let Some(plugin) = line
        .strip_prefix("plugins: ")
        .and_then(|r| r.strip_suffix(" is installed"))
    {
        return if ran {
            ok(format!("{plugin} instalado"))
        } else {
            info(format!("{plugin} ya está instalado"))
        };
    }
    if let Some(name) = line.strip_suffix(": installed by another tool, left alone") {
        return info(format!(
            "{name}: instalado por otra herramienta; {bin} no lo toca. Para usar el de {bin}, quita esa antes con la herramienta que la puso"
        ));
    }
    if let Some(name) = line.strip_suffix(&format!(": not installed by {bin}, left alone")) {
        return info(format!("{name}: no lo instaló {bin}; no lo toca"));
    }
    if let Some(name) = line.strip_suffix(&format!(
        ": what {bin} wrote there was edited, left alone: move your changes away and run it again"
    )) {
        return Some((
            Tone::Err,
            format!(
                "{name}: lo que {bin} escribió se editó; no lo pisa. Mueve tus cambios y vuelve a activar"
            ),
        ));
    }
    match line {
        "interrupted: nothing installed" => info("interrumpido: no se instaló nada".to_owned()),
        NOT_SAID_YES => info("no se instaló: no dijiste que sí".to_owned()),
        CLOSED => info("no se instaló nada: cerraste la pantalla antes del sí".to_owned()),
        _ => None,
    }
}

/// What a work that ended well says: the lines of the library in Spanish, `✓` when something was
/// written or taken away, and not the `$ claude …` lines of a plugin, which were its steps.
fn result(lines: &[String]) -> (Tone, String) {
    let ran = lines.iter().any(|l| l.starts_with("$ "));
    let said: Vec<(Tone, String)> = lines
        .iter()
        .filter(|l| !l.starts_with("$ "))
        .map(|l| spanish(l, ran).unwrap_or((Tone::Info, l.clone())))
        .collect();
    let tone = if said.iter().any(|(t, _)| *t == Tone::Ok) {
        Tone::Ok
    } else {
        Tone::Info
    };
    let text: Vec<&str> = said.iter().map(|(_, t)| t.as_str()).collect();
    (tone, text.join("  ·  "))
}

/// What a work that failed says: a refusal the library words is information or a failure by what it
/// is (REQ-1050), anything else is `✗` with the reason, and says nothing was installed when the work
/// was an install that writes nothing until it ends (REQ-1024).
fn failure(why: &str, installing: bool) -> (Tone, String) {
    if why.starts_with("interrupted") {
        return (Tone::Info, "interrumpido: no se instaló nada".to_owned());
    }
    spanish(why, false).unwrap_or_else(|| {
        let more = if installing {
            " · no se instaló nada"
        } else {
            ""
        };
        (Tone::Err, format!("{why}{more}"))
    })
}

/// What is said when the screen was closed before the yes.
const CLOSED: &str = "not installed: the screen was closed";

fn rank(kind: &str) -> usize {
    GROUPS
        .iter()
        .position(|(_, k)| *k == kind)
        .unwrap_or(GROUPS.len())
}

/// The screen: the rows, the cursor, what is on top of the list and what runs.
pub struct Screen {
    items: Vec<Item>,
    cur: usize,
    mode: Mode,
    busy: Option<Busy>,
    msg: Option<(Tone, String)>,
    checked: bool,
}

impl Screen {
    /// A screen that is reading the catalog (REQ-1031): the loop asks for `Act::Read` when it opens it.
    pub fn new() -> Screen {
        Screen {
            items: Vec::new(),
            cur: 0,
            mode: Mode::List,
            busy: Some(Busy {
                running: Running::Reading,
                what: "leyendo el catálogo…".into(),
                name: None,
            }),
            msg: None,
            checked: false,
        }
    }

    fn item(&self) -> Option<&Item> {
        self.items.get(self.cur)
    }

    pub fn hint(&self) -> String {
        match self.mode {
            Mode::List => KEYS_LIST,
            Mode::Sheet(_) => KEYS_SHEET,
            Mode::Card => KEYS_CARD,
        }
        .to_owned()
    }

    fn start(&mut self, running: Running, name: Option<String>, what: impl Into<String>) {
        self.busy = Some(Busy {
            running,
            what: what.into(),
            name,
        });
        self.msg = None;
    }

    fn say(&mut self, tone: Tone, text: impl Into<String>) {
        self.msg = Some((tone, text.into()));
    }

    /// A line of the library, in Spanish when it has a form (REQ-1049).
    fn say_line(&mut self, line: &str) {
        let (tone, text) = spanish(line, false).unwrap_or((Tone::Info, line.to_owned()));
        self.say(tone, text);
    }

    /// The step the work is in, on the line above the keys (REQ-1047).
    fn step(&mut self, event: Event) {
        let Some(busy) = self.busy.as_mut() else {
            return;
        };
        busy.what = match event {
            Event::Fetching { url, branch } => {
                format!("bajando {url} (rama {branch}) a un temporal…")
            }
            Event::Fetched { commit, files } => format!(
                "bajado el commit {} ({}): preparando la hoja…",
                &commit[..commit.len().min(7)],
                archivos(files)
            ),
            Event::Writing { files } => format!("escribiendo {}…", archivos(files)),
            Event::Locking => "guardando el lock de lo instalado…".to_owned(),
            Event::Running(command) => format!("{command}…"),
        };
    }

    /// What the row of `item` says while the work is on it, in place of its chip (REQ-1046).
    fn row_word(&self, item: &Item) -> Option<&'static str> {
        let busy = self.busy.as_ref()?;
        let mine = busy.name.as_deref() == Some(item.name.as_str());
        match busy.running {
            Running::Checking => {
                (src(item) == Src::Github && item.state == "installed").then_some("revisando…")
            }
            Running::Fetching if mine => Some("bajando…"),
            Running::Writing if mine => Some("escribiendo…"),
            Running::Removing if mine => Some("quitando…"),
            Running::Installing if mine => Some("instalando…"),
            Running::Reading if mine => Some("leyendo…"),
            _ => None,
        }
    }

    pub fn press(&mut self, code: KeyCode) -> Act {
        match &self.mode {
            Mode::Sheet(sheet) => {
                let plugin = matches!(sheet, Sheet::Plugin { .. });
                return match code {
                    KeyCode::Char('y') => {
                        self.mode = Mode::List;
                        let name = self.item().map(|i| i.name.clone());
                        if plugin {
                            self.start(Running::Installing, name, "claude plugin install…");
                        } else {
                            self.start(Running::Writing, name, "escribiendo…");
                        }
                        Act::Apply
                    }
                    KeyCode::Char('n') | KeyCode::Enter | KeyCode::Esc => {
                        self.mode = Mode::List;
                        self.say_line(NOT_SAID_YES);
                        Act::Decline
                    }
                    _ => Act::None,
                };
            }
            Mode::Card => {
                if matches!(code, KeyCode::Esc | KeyCode::Enter) {
                    self.mode = Mode::List;
                }
                return Act::None;
            }
            Mode::List => {}
        }
        if let Some(busy) = &self.busy {
            return match (code, busy.running) {
                (KeyCode::Esc, Running::Fetching) => {
                    self.say(Tone::Info, "cancelando…");
                    Act::Cancel
                }
                (KeyCode::Esc, Running::Writing | Running::Removing | Running::Installing) => {
                    self.say(Tone::Info, NO_CANCEL);
                    Act::None
                }
                (KeyCode::Esc, _) => Act::Close,
                _ => Act::None,
            };
        }
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.cur = (self.cur + 1).min(self.items.len().saturating_sub(1));
                Act::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cur = self.cur.saturating_sub(1);
                Act::None
            }
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('u') => self.update(),
            KeyCode::Char('r') => {
                self.start(
                    Running::Checking,
                    None,
                    "preguntando a GitHub por el último commit…",
                );
                Act::Read { check: true }
            }
            KeyCode::Enter => {
                if self.item().is_some() {
                    self.mode = Mode::Card;
                }
                Act::None
            }
            KeyCode::Char('c') => match self.item().and_then(|i| commands(i).into_iter().next()) {
                Some((command, _)) => {
                    self.say(Tone::Info, format!("copiado: {command}"));
                    Act::Copy(command)
                }
                None => {
                    self.say(
                        Tone::Info,
                        format!(
                            "esta fila no tiene comando: {} no la toca",
                            identity::get().name
                        ),
                    );
                    Act::None
                }
            },
            KeyCode::Esc => Act::Close,
            _ => Act::None,
        }
    }

    /// `espacio` (REQ-1035, REQ-1036, REQ-1039): what is there goes away, what is not comes in, and
    /// what cannot be changed is asked to say why (the library's line).
    fn toggle(&mut self) -> Act {
        let Some(item) = self.item() else {
            return Act::None;
        };
        let name = item.name.clone();
        if matches!(item.state, "installed" | "outdated" | "modified") {
            self.start(
                Running::Removing,
                Some(name.clone()),
                format!("quitando {name}…"),
            );
            return Act::Off(name);
        }
        self.on()
    }

    /// `skillset on` for the row: a download that can be cancelled when it fetches.
    fn on(&mut self) -> Act {
        let Some(item) = self.item() else {
            return Act::None;
        };
        let (name, state, source) = (item.name.clone(), item.state, src(item));
        let fetches = source == Src::Github && matches!(state, "available" | "outdated");
        if fetches {
            let from = item.source.trim_start_matches("github ").to_owned();
            self.start(
                Running::Fetching,
                Some(name.clone()),
                format!("bajando github.com/{from} a un temporal…"),
            );
        } else {
            self.start(Running::Reading, Some(name.clone()), format!("{name}…"));
        }
        Act::On(name)
    }

    /// `u` (REQ-1040).
    fn update(&mut self) -> Act {
        match self.item().map(|i| i.state) {
            Some("outdated" | "modified") => self.on(),
            Some(_) => {
                self.say(
                    Tone::Info,
                    "nada que actualizar (r revisa si hay commit nuevo en GitHub)",
                );
                Act::None
            }
            None => Act::None,
        }
    }

    /// What the thread answered.
    pub fn reply(&mut self, reply: Reply) -> Act {
        match reply {
            Reply::Asked(sheet) => {
                self.busy = None;
                self.msg = None;
                self.mode = Mode::Sheet(sheet);
                Act::None
            }
            Reply::Step(event) => {
                self.step(event);
                Act::None
            }
            Reply::Done(Ok(lines)) => {
                self.busy = None;
                let (tone, text) = result(&lines);
                self.say(tone, text);
                Act::Read { check: false }
            }
            Reply::Done(Err(why)) => {
                let was = self.busy.take().map(|b| b.running);
                let installing = matches!(was, Some(Running::Fetching | Running::Writing));
                let (tone, text) = failure(&why, installing);
                self.say(tone, text);
                Act::None
            }
            Reply::Items {
                result: Ok((items, warnings)),
                checked,
            } => {
                let first = self.items.is_empty();
                self.busy = None;
                self.set_items(items);
                if checked {
                    self.checked = true;
                    let outdated = self.items.iter().filter(|i| i.state == "outdated").count();
                    match warnings.first() {
                        Some(w) => self.say(
                            Tone::Err,
                            format!(
                                "no pude preguntar a GitHub ({w}): el estado queda como estaba"
                            ),
                        ),
                        None if outdated > 0 => self.say(
                            Tone::Ok,
                            format!("revisado: {outdated} con commit nuevo (↑)"),
                        ),
                        None => self.say(Tone::Ok, "revisado: todo al día"),
                    }
                } else if let Some(w) = warnings.first() {
                    self.say(Tone::Info, format!("warning: {w}"));
                } else if first {
                    self.say(
                        Tone::Info,
                        "estado leído del disco, sin red (r revisa GitHub)",
                    );
                }
                Act::None
            }
            Reply::Items {
                result: Err(why), ..
            } => {
                self.busy = None;
                self.say(Tone::Err, why);
                Act::None
            }
        }
    }

    /// The new list, grouped, with the cursor on the same entry; an entry that a check found
    /// outdated stays so while its commit is the one it was (the list is read again after every action).
    fn set_items(&mut self, mut items: Vec<Item>) {
        let name = self.item().map(|i| i.name.clone());
        for item in &mut items {
            let old = self.items.iter().find(|o| o.name == item.name);
            if item.state == "installed"
                && old.is_some_and(|o| o.state == "outdated" && o.commit == item.commit)
            {
                item.state = "outdated";
            }
        }
        items.sort_by_key(|i| rank(i.kind));
        self.cur = name
            .and_then(|n| items.iter().position(|i| i.name == n))
            .unwrap_or(0)
            .min(items.len().saturating_sub(1));
        self.items = items;
    }

    /// The screen over `area`, with the spinner of `now_ms`.
    pub fn draw(&self, g: &mut Grid, area: Rect, now_ms: u64) {
        let (x, y, w, h) = area;
        g.clear_box(area);
        g.boxed(
            area,
            "skillset",
            look::blue(),
            Style::fg(look::fg()).bold(),
            None,
        );
        let ground = look::panel();
        let (dim, plain) = (
            Style::fg(look::dim()).on(ground),
            Style::fg(look::fg()).on(ground),
        );
        let (x0, iw) = (x + 2, w.saturating_sub(4));
        let note = if self.checked {
            "GitHub revisado"
        } else {
            "sin red · r revisa GitHub"
        };
        g.put(
            (x + w).saturating_sub(note.chars().count() + 3),
            y + 1,
            note,
            if self.checked {
                Style::fg(look::green()).on(ground)
            } else {
                dim
            },
        );

        // The foot: the line of what runs or what was said, then the keys.
        let (msg_y, keys_y) = (y + h.saturating_sub(3), y + h.saturating_sub(2));
        match (&self.busy, &self.msg) {
            (Some(b), _) => {
                let cancel = match b.running {
                    Running::Fetching => "   esc cancela",
                    Running::Writing => "   (no se puede cancelar · ya casi)",
                    Running::Removing | Running::Installing => "   (no se puede cancelar)",
                    _ => "",
                };
                g.put(
                    x0,
                    msg_y,
                    &clip(&format!("{} {}{cancel}", shell::spin(now_ms), b.what), iw),
                    Style::fg(look::blue()).on(ground),
                );
            }
            (None, Some((tone, text))) => {
                let (mark, colour) = match tone {
                    Tone::Ok => ("✓", look::green()),
                    Tone::Err => ("✗", look::red()),
                    Tone::Info => ("·", look::yellow()),
                };
                g.put(
                    x0,
                    msg_y,
                    &clip(&format!("{mark} {text}"), iw),
                    Style::fg(colour).on(ground),
                );
            }
            _ => {}
        }
        g.put(x0, keys_y, &clip(&self.hint(), iw), dim);

        // The detail of the row under the cursor, above the foot.
        const DETAIL: usize = 5;
        let detail_y = msg_y.saturating_sub(DETAIL);
        if let Some(item) = self.item() {
            g.put(
                x + 1,
                detail_y,
                &"─".repeat(w.saturating_sub(2)),
                Style::fg(look::line()).on(ground),
            );
            let head = format!(" {} · {} · {} ", item.name, item.kind, item.source);
            g.put(
                x0,
                detail_y,
                &clip(&head, iw),
                Style::fg(look::fg()).bold().on(ground),
            );
            let text = why(item);
            for (at, line) in (detail_y + 1..).zip(wrap(&text, iw).into_iter().take(3)) {
                g.put(x0, at, &line, plain);
            }
            if let Some((command, _)) = commands(item).into_iter().next() {
                g.put(
                    x0,
                    detail_y + DETAIL - 1,
                    &clip(&format!("$ {command}"), iw),
                    Style::fg(look::blue()).on(ground),
                );
            }
        }

        // The list, in a window that keeps the cursor in sight.
        let top = y + 2;
        let cap = detail_y.saturating_sub(top);
        let mut rows: Vec<Option<Result<usize, &str>>> = Vec::new();
        for (title, kind) in GROUPS {
            let members: Vec<usize> = (0..self.items.len())
                .filter(|i| self.items[*i].kind == kind)
                .collect();
            if members.is_empty() {
                continue;
            }
            if !rows.is_empty() {
                rows.push(None);
            }
            rows.push(Some(Err(title)));
            rows.extend(members.into_iter().map(|i| Some(Ok(i))));
        }
        let at_cursor = rows
            .iter()
            .position(|r| *r == Some(Ok(self.cur)))
            .unwrap_or(0);
        let start = (at_cursor + 1).saturating_sub(cap);
        for (i, row) in rows.iter().skip(start).take(cap).enumerate() {
            let ry = top + i;
            match row {
                Some(Err(title)) => g.put(x0, ry, title, Style::fg(look::fg()).bold().on(ground)),
                Some(Ok(idx)) => self.draw_row(g, (x0, ry, iw), *idx, (ground, now_ms)),
                None => {}
            }
        }

        match &self.mode {
            Mode::Sheet(sheet) => draw_box(g, area, "activar", &sheet_lines(sheet)),
            Mode::Card => {
                if let Some(item) = self.item() {
                    draw_box(
                        g,
                        area,
                        &format!("ficha de {}", item.name),
                        &card_lines(item),
                    );
                }
            }
            Mode::List => {}
        }
    }

    fn draw_row(
        &self,
        g: &mut Grid,
        (x0, y, iw): (usize, usize, usize),
        idx: usize,
        (ground, now_ms): (Rgb, u64),
    ) {
        let item = &self.items[idx];
        let on = idx == self.cur && self.mode == Mode::List;
        let ground = if on { look::sel() } else { ground };
        g.put(x0 - 1, y, &" ".repeat(iw + 2), Style::default().on(ground));
        if on {
            g.put(x0, y, "▸", Style::fg(look::blue()).bold().on(ground));
        }
        let name = Style::fg(look::fg()).on(ground);
        g.put(
            x0 + 2,
            y,
            &clip(&item.name, 12),
            if on { name.bold() } else { name },
        );
        let (glyph, word, colour) = chip(item.state);
        let (text, colour) = match self.row_word(item) {
            Some(busy) => (format!("{} {busy}", shell::spin(now_ms)), look::blue()),
            None => (format!("{glyph} {word}"), colour),
        };
        g.put(x0 + 16, y, &clip(&text, 18), Style::fg(colour).on(ground));
        let dim = Style::fg(look::dim()).on(ground);
        g.put(x0 + 36, y, &clip(&item.source, 32), dim);
        g.put(
            x0 + 70,
            y,
            &clip(&item.description, iw.saturating_sub(70)),
            dim,
        );
    }
}

/// The card: what the row is and every command of the CLI that applies to it.
fn card_lines(item: &Item) -> Vec<(String, Style2)> {
    let (_, word, _) = chip(item.state);
    let mut l = vec![
        (format!("tipo     {}", item.kind), Style2::Plain),
        (format!("fuente   {}", item.source), Style2::Plain),
        (
            format!("estado   {word}  (en el CLI: «{}»)", item.state),
            Style2::Plain,
        ),
    ];
    if let Some(c) = &item.commit {
        l.push((format!("commit   {c}"), Style2::Plain));
    }
    l.push((String::new(), Style2::Plain));
    l.extend(
        wrap(&item.description, 84)
            .into_iter()
            .map(|t| (t, Style2::Dim)),
    );
    l.push((String::new(), Style2::Plain));
    l.extend(wrap(&why(item), 84).into_iter().map(|t| (t, Style2::Plain)));
    l.push((String::new(), Style2::Plain));
    l.push(("Desde el CLI:".to_owned(), Style2::Dim));
    for (command, what) in commands(item) {
        l.push((format!("  $ {command}   {what}"), Style2::Command));
    }
    l
}

/// A box over the screen, with its lines.
fn draw_box(g: &mut Grid, (x, y, w, h): Rect, title: &str, lines: &[(String, Style2)]) {
    let bw = w.saturating_sub(4).min(100);
    let bh = (lines.len() + 2).min(h.saturating_sub(2));
    let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 1);
    g.clear_box((bx, by, bw, bh));
    g.boxed(
        (bx, by, bw, bh),
        title,
        look::yellow(),
        Style::fg(look::fg()).bold(),
        None,
    );
    let ground = look::panel();
    for (i, (text, style)) in lines.iter().take(bh.saturating_sub(2)).enumerate() {
        let st = match style {
            Style2::Plain => Style::fg(look::fg()),
            Style2::Dim => Style::fg(look::dim()),
            Style2::Bold => Style::fg(look::fg()).bold(),
            Style2::File => Style::fg(look::cyan()),
            Style2::Command => Style::fg(look::blue()),
            Style2::Warn => Style::fg(look::yellow()),
        };
        g.put(
            bx + 2,
            by + 1 + i,
            &clip(text, bw.saturating_sub(4)),
            st.on(ground),
        );
    }
}

/// `text` in lines of at most `width` characters, broken at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// What the thread sends: the answer, and the fetched thing that waits for the yes (it stays with
/// the worker: a `Sheet` is only what is drawn of it).
struct Done {
    reply: Reply,
    ask: Option<Ask>,
}

/// The thread's side: does what the screen asks with the library of the CLI, in the client's
/// process, and keeps what waits for the yes (REQ-1043). Nothing here reads the terminal.
pub struct Worker {
    place: Place,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    pending: Option<Ask>,
    cancel: Arc<AtomicBool>,
    open: bool,
    /// What a work answered after the screen was closed: shown when it is opened again (REQ-1052).
    late: Option<Reply>,
}

impl Worker {
    /// A worker of the process's own place: the Claude configuration, the lock and GitHub.
    pub fn here() -> Worker {
        Worker::new(Place::here())
    }

    pub fn new(place: Place) -> Worker {
        let (tx, rx) = mpsc::channel();
        Worker {
            place,
            tx,
            rx,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            open: false,
            late: None,
        }
    }

    /// The work gets the place and a way to tell its steps, which go in the channel the answers use.
    fn spawn(&self, work: impl FnOnce(&Place, &mut dyn FnMut(Event)) -> Done + Send + 'static) {
        let (place, tx) = (self.place.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let mut step = |event| {
                let _ = tx.send(Done {
                    reply: Reply::Step(event),
                    ask: None,
                });
            };
            let done = work(&place, &mut step);
            let _ = tx.send(done);
        });
    }

    pub fn run(&mut self, act: Act) {
        match act {
            Act::Read { check } => {
                self.open = true;
                self.spawn(move |p, _| Done {
                    reply: Reply::Items {
                        result: skillset::items(p, check),
                        checked: check,
                    },
                    ask: None,
                });
            }
            Act::On(name) => {
                // A flag for each download: one that was stopped does not stop the next.
                self.cancel = Arc::new(AtomicBool::new(false));
                let cancel = self.cancel.clone();
                self.spawn(move |p, step| match skillset::on(p, &name, cancel, step) {
                    Ok(Step::Done(lines)) => Done {
                        reply: Reply::Done(Ok(lines)),
                        ask: None,
                    },
                    Ok(Step::Ask(ask)) => Done {
                        reply: Reply::Asked(ask.sheet()),
                        ask: Some(ask),
                    },
                    Err(why) => Done {
                        reply: Reply::Done(Err(why)),
                        ask: None,
                    },
                });
            }
            Act::Off(name) => self.spawn(move |p, _| Done {
                reply: Reply::Done(skillset::off(p, &name)),
                ask: None,
            }),
            Act::Apply => {
                match self.pending.take() {
                    Some(ask) => self.spawn(move |p, step| Done {
                        reply: Reply::Done(skillset::apply(p, ask, step)),
                        ask: None,
                    }),
                    // Never leave the screen working on what nobody holds (REQ-1054).
                    None => {
                        let _ = self.tx.send(Done {
                            reply: Reply::Done(Err("la hoja ya no está".into())),
                            ask: None,
                        });
                    }
                }
            }
            Act::Decline => self.pending = None,
            Act::Cancel => self.cancel.store(true, Ordering::Relaxed),
            Act::Close => {
                self.open = false;
                self.cancel.store(true, Ordering::Relaxed);
                self.pending = None;
            }
            Act::None | Act::Copy(_) => {}
        }
    }

    /// What the threads have answered since the last time. After the screen was closed a fetched
    /// thing is dropped (and its directory deleted) and what a work answered is kept for the next
    /// time it is opened, after its list (REQ-1052).
    pub fn poll(&mut self) -> Vec<Reply> {
        let mut replies = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            if self.open {
                self.pending = done.ask.or(self.pending.take());
                let list = matches!(done.reply, Reply::Items { .. });
                replies.push(done.reply);
                if list {
                    replies.extend(self.late.take());
                }
            } else {
                match done.reply {
                    Reply::Done(answer) => self.late = Some(Reply::Done(answer)),
                    Reply::Asked(_) => {
                        self.late = Some(Reply::Done(Ok(vec![CLOSED.to_owned()])));
                    }
                    Reply::Items { .. } | Reply::Step(_) => {}
                }
            }
        }
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skillset::tests::World;

    fn item(name: &str, kind: &'static str, state: &'static str, source: &str) -> Item {
        Item {
            name: name.into(),
            kind,
            state,
            source: source.into(),
            description: format!("what {name} does"),
            yours: false,
            commit: None,
        }
    }

    /// The seven entries of the catalog, out of order, in the states `states` gives them by name.
    fn catalog(states: &[(&str, &'static str)]) -> Vec<Item> {
        let state =
            |n: &str, d: &'static str| states.iter().find(|(k, _)| *k == n).map_or(d, |(_, s)| *s);
        vec![
            item("jw", "skill", state("jw", "installed"), "embedded"),
            item("brain", "role", "built-in", "embedded"),
            item(
                "caveman",
                "skill",
                state("caveman", "available"),
                "github juliusbrussee/caveman",
            ),
            item(
                "ponytail",
                "plugin",
                state("ponytail", "available"),
                "plugin ponytail@ponytail",
            ),
            item("agent", "skill", state("agent", "installed"), "embedded"),
        ]
    }

    fn screen(states: &[(&str, &'static str)]) -> Screen {
        let mut s = Screen::new();
        s.reply(Reply::Items {
            result: Ok((catalog(states), vec![])),
            checked: false,
        });
        s
    }

    fn goto(s: &mut Screen, name: &str) {
        s.cur = s.items.iter().position(|i| i.name == name).unwrap();
    }

    fn sheet() -> Sheet {
        Sheet::Github {
            name: "caveman".into(),
            url: "https://github.com/juliusbrussee/caveman".into(),
            branch: "HEAD".into(),
            commit: "a".repeat(40),
            from: None,
            files: vec![("/home/ana/.claude/skills/caveman/SKILL.md".into(), 2214)],
            scripts: vec![],
        }
    }

    fn msg(s: &Screen) -> String {
        s.msg.clone().map(|(_, t)| t).unwrap_or_default()
    }

    #[test]
    fn the_list_is_grouped_skills_roles_plugins_and_the_cursor_stays_on_its_entry() {
        let mut s = screen(&[]);
        let names: Vec<_> = s.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["jw", "caveman", "agent", "brain", "ponytail"]);
        goto(&mut s, "caveman");
        s.reply(Reply::Items {
            result: Ok((catalog(&[("caveman", "installed")]), vec![])),
            checked: false,
        });
        assert_eq!(s.item().unwrap().name, "caveman");
        assert_eq!(s.item().unwrap().state, "installed");
    }

    #[test]
    fn the_states_are_said_in_spanish_with_a_glyph_and_the_cli_keeps_its_words() {
        let words: Vec<_> = [
            "available",
            "installed",
            "outdated",
            "modified",
            "other tool",
            "built-in",
            "unknown",
        ]
        .iter()
        .map(|s| {
            let (g, w, _) = chip(s);
            format!("{g} {w}")
        })
        .collect();
        assert_eq!(
            words,
            [
                "○ disponible",
                "✓ instalado",
                "↑ desactualizado",
                "✎ modificado",
                "⇄ otra herramienta",
                "● incluido",
                "? desconocido"
            ]
        );
    }

    #[test]
    fn j_k_and_the_arrows_move_inside_the_list_and_no_other_key_does_anything() {
        let mut s = screen(&[]);
        assert_eq!(s.cur, 0);
        for code in [KeyCode::Char('j'), KeyCode::Down, KeyCode::Down] {
            assert_eq!(s.press(code), Act::None);
        }
        assert_eq!(s.cur, 3);
        for code in [KeyCode::Char('k'), KeyCode::Up] {
            s.press(code);
        }
        assert_eq!(s.cur, 1);
        for _ in 0..10 {
            s.press(KeyCode::Down);
        }
        assert_eq!(s.cur, 4, "it stops at the last row");
        for code in [
            KeyCode::Char('x'),
            KeyCode::Char('q'),
            KeyCode::Tab,
            KeyCode::Char('a'),
            KeyCode::Char('d'),
        ] {
            assert_eq!(s.press(code), Act::None, "{code:?}");
        }
    }

    #[test]
    fn space_installs_what_is_not_there_and_takes_away_what_is_and_asks_the_rest_to_say_why() {
        for (name, state, act, busy) in [
            (
                "agent",
                "available",
                Act::On("agent".into()),
                Running::Reading,
            ),
            (
                "caveman",
                "available",
                Act::On("caveman".into()),
                Running::Fetching,
            ),
            (
                "ponytail",
                "available",
                Act::On("ponytail".into()),
                Running::Reading,
            ),
            (
                "agent",
                "installed",
                Act::Off("agent".into()),
                Running::Removing,
            ),
            (
                "caveman",
                "outdated",
                Act::Off("caveman".into()),
                Running::Removing,
            ),
            (
                "caveman",
                "modified",
                Act::Off("caveman".into()),
                Running::Removing,
            ),
            (
                "caveman",
                "other tool",
                Act::On("caveman".into()),
                Running::Reading,
            ),
            (
                "ponytail",
                "unknown",
                Act::On("ponytail".into()),
                Running::Reading,
            ),
        ] {
            let mut s = screen(&[(name, state)]);
            goto(&mut s, name);
            assert_eq!(s.press(KeyCode::Char(' ')), act, "{name} {state}");
            assert_eq!(
                s.busy.as_ref().map(|b| b.running),
                Some(busy),
                "{name} {state}"
            );
        }
        let mut s = screen(&[]);
        goto(&mut s, "brain");
        assert_eq!(
            s.press(KeyCode::Char(' ')),
            Act::On("brain".into()),
            "the library says it is built in"
        );
    }

    #[test]
    fn while_something_runs_the_list_keys_do_nothing_and_esc_depends_on_what_runs() {
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        s.press(KeyCode::Char(' '));
        assert_eq!(s.press(KeyCode::Char('j')), Act::None);
        assert_eq!(s.press(KeyCode::Char(' ')), Act::None);
        assert_eq!(s.press(KeyCode::Esc), Act::Cancel, "a download is stopped");
        s.busy = Some(Busy {
            running: Running::Writing,
            what: "escribiendo…".into(),
            name: None,
        });
        assert_eq!(s.press(KeyCode::Esc), Act::None);
        assert_eq!(msg(&s), NO_CANCEL);
        s.busy = Some(Busy {
            running: Running::Reading,
            what: "leyendo".into(),
            name: None,
        });
        assert_eq!(s.press(KeyCode::Esc), Act::Close);
        let mut idle = screen(&[]);
        assert_eq!(idle.press(KeyCode::Esc), Act::Close);
    }

    #[test]
    fn u_updates_what_is_outdated_or_modified_and_says_there_is_nothing_for_the_rest() {
        for (state, wants) in [
            ("outdated", true),
            ("modified", true),
            ("installed", false),
            ("available", false),
        ] {
            let mut s = screen(&[("caveman", state)]);
            goto(&mut s, "caveman");
            let act = s.press(KeyCode::Char('u'));
            assert_eq!(act == Act::On("caveman".into()), wants, "{state}");
            if !wants {
                assert!(msg(&s).starts_with("nada que actualizar"), "{}", msg(&s));
            }
        }
    }

    #[test]
    fn the_sheet_installs_only_on_y_and_n_enter_and_esc_are_no() {
        let mut s = screen(&[]);
        s.reply(Reply::Asked(sheet()));
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char('j'),
            KeyCode::Char('Y'),
            KeyCode::Char('u'),
        ] {
            assert_eq!(s.press(code), Act::None, "{code:?}");
            assert!(matches!(s.mode, Mode::Sheet(_)), "the sheet stays");
        }
        assert_eq!(s.press(KeyCode::Char('y')), Act::Apply);
        assert_eq!(s.mode, Mode::List);
        assert_eq!(s.busy.as_ref().map(|b| b.running), Some(Running::Writing));
        for no in [KeyCode::Char('n'), KeyCode::Enter, KeyCode::Esc] {
            let mut s = screen(&[]);
            s.reply(Reply::Asked(sheet()));
            assert_eq!(s.press(no), Act::Decline, "{no:?}");
            assert_eq!(s.mode, Mode::List);
            assert!(s.busy.is_none());
            assert_eq!(msg(&s), "no se instaló: no dijiste que sí");
        }
    }

    #[test]
    fn the_sheet_says_where_from_the_commit_the_files_and_the_scripts() {
        let mut with_scripts = sheet();
        if let Sheet::Github {
            files,
            scripts,
            from,
            ..
        } = &mut with_scripts
        {
            files.push((
                "/home/ana/.claude/skills/caveman/scripts/stats.py".into(),
                860,
            ));
            scripts.push("scripts/stats.py".into());
            *from = Some("b".repeat(40));
        }
        let text: Vec<String> = sheet_lines(&with_scripts)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        for want in [
            "  https://github.com/juliusbrussee/caveman",
            &format!("  rama HEAD · commit {}", "a".repeat(40)),
            "  ahora tienes el commit bbbbbbb",
            "Escribirá 2 archivos:",
            "  /home/ana/.claude/skills/caveman/SKILL.md (2214 bytes)",
            "  /home/ana/.claude/skills/caveman/scripts/stats.py (860 bytes)",
            "! La carpeta trae archivos que no son Markdown (scripts/stats.py):",
        ] {
            assert!(text.iter().any(|l| l == want), "{want}\n{text:#?}");
        }
        let plain: Vec<String> = sheet_lines(&sheet()).into_iter().map(|(t, _)| t).collect();
        assert!(
            !plain.iter().any(|l| l.contains("Markdown")),
            "no warning when there is no script"
        );
        assert!(
            !plain.iter().any(|l| l.contains("ahora tienes")),
            "no old commit for a first install"
        );
        let plugin = Sheet::Plugin {
            plugin: "ponytail@ponytail".into(),
            repo: "DietrichGebert/ponytail".into(),
            marketplace: "ponytail".into(),
            commands: vec![
                "plugin marketplace add DietrichGebert/ponytail".into(),
                "plugin install ponytail@ponytail".into(),
            ],
        };
        let text: Vec<String> = sheet_lines(&plugin).into_iter().map(|(t, _)| t).collect();
        assert!(
            text.contains(&"  $ claude plugin install ponytail@ponytail".to_owned()),
            "{text:#?}"
        );
        assert!(
            text.iter().any(|l| l.contains("por sus hooks")),
            "{text:#?}"
        );
    }

    #[test]
    fn what_the_thread_answers_is_shown_and_the_list_is_read_again_after_a_change() {
        let mut s = screen(&[]);
        let act = s.reply(Reply::Done(Ok(vec![
            "installed caveman (commit abc1234): 1 file(s) in /x".into(),
        ])));
        assert_eq!(act, Act::Read { check: false });
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Ok));
        assert_eq!(
            msg(&s),
            "instalado caveman (commit abc1234): 1 archivo en /x"
        );
        s.reply(Reply::Done(Ok(vec![
            "kept /x/SKILL.md: not written by jw or edited".into(),
        ])));
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Info));
        s.reply(Reply::Done(Err("interrupted".into())));
        assert_eq!(
            s.msg,
            Some((Tone::Info, "interrumpido: no se instaló nada".into()))
        );
    }

    #[test]
    fn r_asks_for_the_check_and_marks_what_has_a_newer_commit_and_a_failed_query_changes_nothing() {
        let mut s = screen(&[]);
        assert_eq!(s.press(KeyCode::Char('r')), Act::Read { check: true });
        assert_eq!(s.busy.as_ref().map(|b| b.running), Some(Running::Checking));
        let mut found = catalog(&[("caveman", "outdated")]);
        found[2].commit = Some("c1".into());
        s.reply(Reply::Items {
            result: Ok((found, vec![])),
            checked: true,
        });
        assert_eq!(msg(&s), "revisado: 1 con commit nuevo (↑)");
        assert!(s.checked);
        // Read again after an unrelated change, with no check: the mark stays while the commit is the same.
        let mut again = catalog(&[("caveman", "installed")]);
        again[2].commit = Some("c1".into());
        s.reply(Reply::Items {
            result: Ok((again, vec![])),
            checked: false,
        });
        assert_eq!(
            s.items.iter().find(|i| i.name == "caveman").unwrap().state,
            "outdated"
        );
        // A failed query: the state is as it was and the screen says it.
        s.press(KeyCode::Char('r'));
        s.reply(Reply::Items {
            result: Ok((catalog(&[]), vec!["caveman: git ls-remote failed".into()])),
            checked: true,
        });
        assert!(
            msg(&s).starts_with("no pude preguntar a GitHub (caveman: git ls-remote failed)"),
            "{}",
            msg(&s)
        );
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Err));
    }

    #[test]
    fn enter_opens_the_card_that_only_reads_and_c_copies_the_main_command() {
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        assert_eq!(s.press(KeyCode::Enter), Act::None);
        assert_eq!(s.mode, Mode::Card);
        assert_eq!(
            s.press(KeyCode::Char(' ')),
            Act::None,
            "the card reads and does nothing"
        );
        assert_eq!(s.press(KeyCode::Esc), Act::None);
        assert_eq!(s.mode, Mode::List);
        let bin = &identity::get().name;
        assert_eq!(
            s.press(KeyCode::Char('c')),
            Act::Copy(format!("{bin} skillset on caveman"))
        );
        goto(&mut s, "brain");
        assert_eq!(
            s.press(KeyCode::Char('c')),
            Act::Copy(format!("{bin} role show brain"))
        );
        let mut other = screen(&[("caveman", "other tool")]);
        goto(&mut other, "caveman");
        assert_eq!(
            other.press(KeyCode::Char('c')),
            Act::None,
            "no command for what another tool owns"
        );
    }

    #[test]
    fn the_screen_draws_the_groups_the_chips_the_detail_and_the_sheet() {
        look::set(
            contract::themes::get(contract::themes::DEFAULT_DARK, true).palette,
            true,
        );
        let mut s = screen(&[("caveman", "other tool")]);
        let mut g = Grid::new(110, 30);
        s.draw(&mut g, (0, 0, 110, 30), 0);
        let text = g.rows_plain().join("\n");
        for want in [
            "skillset",
            "Skills",
            "Roles",
            "Plugins",
            "▸ jw",
            "✓ instalado",
            "● incluido",
            "○ disponible",
            "sin red · r revisa GitHub",
            "j k mover · espacio activa o quita",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        goto(&mut s, "caveman");
        let mut g = Grid::new(110, 30);
        s.draw(&mut g, (0, 0, 110, 30), 0);
        let text = g.rows_plain().join("\n");
        assert!(text.contains("⇄ otra herramienta"), "{text}");
        assert!(text.contains("no escribió"), "{text}");
        s.reply(Reply::Asked(sheet()));
        let mut g = Grid::new(110, 30);
        s.draw(&mut g, (0, 0, 110, 30), 0);
        let text = g.rows_plain().join("\n");
        assert!(
            text.contains(&format!(
                "Código de un tercero, no de {}. Se baja de:",
                identity::get().name
            )),
            "{text}"
        );
        assert!(
            text.contains("y instala · n, ↵ o esc no instalan"),
            "{text}"
        );
    }

    #[test]
    fn a_small_screen_keeps_the_row_under_the_cursor_in_sight() {
        let mut s = screen(&[]);
        goto(&mut s, "ponytail");
        let mut g = Grid::new(80, 16);
        s.draw(&mut g, (0, 0, 80, 16), 0);
        let text = g.rows_plain().join("\n");
        assert!(text.contains("▸ ponytail"), "{text}");
        let mut tiny = Grid::new(30, 6);
        s.draw(&mut tiny, (0, 0, 30, 6), 0);
    }

    /// Run what the screen asks through a worker, and what that asks in turn, until every answer is in.
    fn pump(s: &mut Screen, w: &mut Worker, first: Act) {
        pump_seen(s, w, first);
    }

    /// `pump`, and the steps the work told on the way.
    fn pump_seen(s: &mut Screen, w: &mut Worker, first: Act) -> Vec<Event> {
        let mut seen = Vec::new();
        let mut acts = vec![first];
        let mut in_flight = 0;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            for act in acts.drain(..) {
                in_flight += usize::from(matches!(
                    act,
                    Act::Read { .. } | Act::On(_) | Act::Off(_) | Act::Apply
                ));
                w.run(act);
            }
            if in_flight == 0 {
                return seen;
            }
            let replies = w.poll();
            // A kept answer of a closed screen comes with the list and was not asked for now.
            in_flight = in_flight.saturating_sub(
                replies
                    .iter()
                    .filter(|r| !matches!(r, Reply::Step(_)))
                    .count(),
            );
            for r in replies {
                if let Reply::Step(e) = &r {
                    seen.push(e.clone());
                }
                let act = s.reply(r);
                if act != Act::None {
                    acts.push(act);
                }
            }
            assert!(
                std::time::Instant::now() < until,
                "the worker did not answer"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn the_screen_installs_from_a_local_repository_through_the_worker_and_a_no_installs_nothing() {
        let world = World::new();
        let mut w = Worker::new(world.place.clone());
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        assert_eq!(
            s.items.iter().find(|i| i.name == "caveman").unwrap().state,
            "available"
        );
        // A no: fetched, asked about, dropped; nothing is written.
        goto(&mut s, "caveman");
        let act = s.press(KeyCode::Char(' '));
        pump(&mut s, &mut w, act);
        assert!(
            matches!(s.mode, Mode::Sheet(Sheet::Github { .. })),
            "the sheet is open"
        );
        assert!(
            !world.skill_file().exists(),
            "nothing is written before the yes"
        );
        let act = s.press(KeyCode::Char('n'));
        w.run(act);
        assert!(!world.skill_file().exists());
        // A yes: written, the list read again, the row is installed.
        let act = s.press(KeyCode::Char(' '));
        pump(&mut s, &mut w, act);
        let act = s.press(KeyCode::Char('y'));
        pump(&mut s, &mut w, act);
        assert!(world.skill_file().exists());
        assert!(
            msg(&s).starts_with("instalado caveman (commit "),
            "{}",
            msg(&s)
        );
        assert_eq!(s.item().unwrap().name, "caveman");
        assert_eq!(s.item().unwrap().state, "installed");
        // And away again with the same key.
        let act = s.press(KeyCode::Char(' '));
        pump(&mut s, &mut w, act);
        assert!(!world.skill_file().exists());
        assert_eq!(s.item().unwrap().state, "available");
    }

    #[test]
    fn esc_while_the_download_runs_stops_it_and_nothing_is_installed() {
        let world = World::new();
        let mut w = Worker::new(world.place.clone());
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        goto(&mut s, "caveman");
        let act = s.press(KeyCode::Char(' '));
        w.run(act);
        let cancel = s.press(KeyCode::Esc);
        assert_eq!(cancel, Act::Cancel);
        w.run(cancel);
        // Whether the fetch was over before the flag or not, it ends as a no, never as an install.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while s.busy.is_some() {
            for r in w.poll() {
                s.reply(r);
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!world.skill_file().exists());
        assert!(
            matches!(s.mode, Mode::Sheet(_)) || msg(&s) == "interrumpido: no se instaló nada",
            "{:?} {}",
            s.mode,
            msg(&s)
        );
    }

    /// The screen drawn at `now_ms` in plain text.
    fn drawn(s: &Screen, now_ms: u64) -> String {
        look::set(
            contract::themes::get(contract::themes::DEFAULT_DARK, true).palette,
            true,
        );
        let mut g = Grid::new(110, 30);
        s.draw(&mut g, (0, 0, 110, 30), now_ms);
        g.rows_plain().join("\n")
    }

    fn row_of<'a>(text: &'a str, name: &str) -> &'a str {
        text.lines()
            .find(|l| l.contains(&format!(" {name} ")) || l.contains(&format!("▸ {name}")))
            .unwrap_or_else(|| panic!("no row of {name}\n{text}"))
    }

    #[test]
    fn the_row_at_work_has_the_spinner_and_its_word_in_place_of_the_chip_and_the_chip_comes_back() {
        let mut s = screen(&[("caveman", "installed")]);
        goto(&mut s, "ponytail");
        s.press(KeyCode::Char(' '));
        s.reply(Reply::Asked(Sheet::Plugin {
            plugin: "ponytail@ponytail".into(),
            repo: "o/r".into(),
            marketplace: "ponytail".into(),
            commands: vec![],
        }));
        s.press(KeyCode::Char('y'));
        let (a, b) = (drawn(&s, 0), drawn(&s, 110));
        let (ra, rb) = (row_of(&a, "ponytail"), row_of(&b, "ponytail"));
        assert!(
            ra.contains("⠋ instalando…") && rb.contains("⠙ instalando…"),
            "{ra}\n{rb}"
        );
        assert!(!ra.contains("disponible"), "the chip gives way: {ra}");
        assert!(
            row_of(&a, "caveman").contains("instalado"),
            "another row keeps its chip"
        );
        for (name, state, act, word) in [
            (
                "caveman",
                "available",
                Act::On("caveman".into()),
                "bajando…",
            ),
            ("agent", "installed", Act::Off("agent".into()), "quitando…"),
            ("jw", "installed", Act::Off("jw".into()), "quitando…"),
        ] {
            let mut s = screen(&[(name, state)]);
            goto(&mut s, name);
            assert_eq!(s.press(KeyCode::Char(' ')), act);
            let text = drawn(&s, 0);
            assert!(row_of(&text, name).contains(word), "{name}\n{text}");
        }
        // The write of a sheet says it too, and the foot ends with the same words (REQ-1047).
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        s.reply(Reply::Asked(sheet()));
        s.press(KeyCode::Char('y'));
        let text = drawn(&s, 0);
        assert!(
            row_of(&text, "caveman").contains("⠋ escribiendo…"),
            "{text}"
        );
        assert!(
            text.contains("⠋ escribiendo…   (no se puede cancelar · ya casi)"),
            "{text}"
        );
        let later = drawn(&s, 110);
        assert!(
            later.contains("⠙ escribiendo…   (no se puede cancelar · ya casi)"),
            "the foot's spinner turns: {later}"
        );
        // The answer ends it: the chip is back.
        s.reply(Reply::Done(Ok(vec![
            "installed caveman (commit abc1234): 1 file(s) in /x".into(),
        ])));
        s.reply(Reply::Items {
            result: Ok((catalog(&[("caveman", "installed")]), vec![])),
            checked: false,
        });
        assert!(row_of(&drawn(&s, 0), "caveman").contains("✓ instalado"));
    }

    #[test]
    fn r_shows_revisando_on_every_installed_github_row_and_only_there() {
        let mut s = screen(&[("caveman", "installed")]);
        s.press(KeyCode::Char('r'));
        let text = drawn(&s, 0);
        assert!(row_of(&text, "caveman").contains("⠋ revisando…"), "{text}");
        assert!(!row_of(&text, "agent").contains("revisando"), "{text}");
        assert!(
            text.contains("⠋ preguntando a GitHub por el último commit…"),
            "{text}"
        );
    }

    #[test]
    fn the_foot_follows_the_steps_the_work_tells_and_a_step_with_nothing_running_is_ignored() {
        let mut s = screen(&[]);
        s.reply(Reply::Step(Event::Locking));
        assert!(s.busy.is_none(), "no work, no step");
        goto(&mut s, "caveman");
        s.press(KeyCode::Char(' '));
        for (event, want) in [
            (
                Event::Fetching {
                    url: "https://github.com/o/r".into(),
                    branch: "HEAD".into(),
                },
                "⠋ bajando https://github.com/o/r (rama HEAD) a un temporal…   esc cancela",
            ),
            (
                Event::Fetched {
                    commit: "a".repeat(40),
                    files: 1,
                },
                "⠋ bajado el commit aaaaaaa (1 archivo): preparando la hoja…   esc cancela",
            ),
        ] {
            assert_eq!(s.reply(Reply::Step(event)), Act::None);
            assert!(drawn(&s, 0).contains(want), "{want}\n{}", drawn(&s, 0));
        }
        s.reply(Reply::Asked(sheet()));
        s.press(KeyCode::Char('y'));
        for (event, want) in [
            (
                Event::Writing { files: 2 },
                "⠋ escribiendo 2 archivos…   (no se puede cancelar · ya casi)",
            ),
            (
                Event::Locking,
                "⠋ guardando el lock de lo instalado…   (no se puede cancelar · ya casi)",
            ),
        ] {
            s.reply(Reply::Step(event));
            assert!(drawn(&s, 0).contains(want), "{want}\n{}", drawn(&s, 0));
        }
        let mut p = screen(&[]);
        goto(&mut p, "ponytail");
        p.press(KeyCode::Char(' '));
        p.reply(Reply::Asked(Sheet::Plugin {
            plugin: "ponytail@ponytail".into(),
            repo: "o/r".into(),
            marketplace: "m".into(),
            commands: vec![],
        }));
        p.press(KeyCode::Char('y'));
        p.reply(Reply::Step(Event::Running(
            "$ claude plugin install ponytail@ponytail".into(),
        )));
        assert!(
            drawn(&p, 0)
                .contains("⠋ $ claude plugin install ponytail@ponytail…   (no se puede cancelar)"),
            "{}",
            drawn(&p, 0)
        );
    }

    #[test]
    fn a_failure_is_drawn_with_a_cross_and_its_reason_and_the_spinner_is_gone() {
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        s.press(KeyCode::Char(' '));
        s.reply(Reply::Done(Err(
            "fatal: unable to access the repository".into()
        )));
        let text = drawn(&s, 0);
        assert!(
            text.contains("✗ fatal: unable to access the repository · no se instaló nada"),
            "{text}"
        );
        assert!(!text.contains("bajando"), "{text}");
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Err));
        // A refusal that is not an install says no more than it said.
        let mut o = screen(&[]);
        o.reply(Reply::Done(Err("the lock: it is read only".into())));
        assert_eq!(msg(&o), "the lock: it is read only");
    }

    #[test]
    fn what_another_tool_owns_is_information_with_the_way_out_and_a_modified_one_is_a_failure() {
        let mut s = screen(&[("caveman", "other tool")]);
        goto(&mut s, "caveman");
        s.press(KeyCode::Char(' '));
        s.reply(Reply::Done(Err(
            "caveman: installed by another tool, left alone".into(),
        )));
        let bin = &identity::get().name;
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Info));
        let text = msg(&s);
        assert!(
            text.starts_with(&format!(
                "caveman: instalado por otra herramienta; {bin} no lo toca."
            )),
            "{text}"
        );
        assert!(text.contains("quita esa antes"), "{text}");
        assert!(drawn(&s, 0).contains("· caveman: instalado por otra herramienta"));
        s.reply(Reply::Done(Err(format!(
            "caveman: not installed by {bin}, left alone"
        ))));
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Info));
        s.reply(Reply::Done(Err(format!(
            "caveman: what {bin} wrote there was edited, left alone: move your changes away and run it again"
        ))));
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Err));
        assert!(msg(&s).starts_with("caveman: lo que"), "{}", msg(&s));
    }

    #[test]
    fn every_line_the_library_says_has_a_spanish_form_with_its_tone() {
        let bin = &identity::get().name;
        for (line, tone, text) in [
            (
                "installed /h/skills/jw/SKILL.md",
                Tone::Ok,
                "instalado /h/skills/jw/SKILL.md".to_owned(),
            ),
            (
                "unchanged /h/skills/jw/SKILL.md",
                Tone::Info,
                "sin cambios: /h/skills/jw/SKILL.md".into(),
            ),
            (
                "unchanged caveman (commit abc1234)",
                Tone::Info,
                "caveman ya está en el commit abc1234: sin cambios".into(),
            ),
            (
                "removed 2 file(s) of caveman",
                Tone::Ok,
                "quitado caveman (2 archivos)".into(),
            ),
            (
                "removed /h/skills/jw/SKILL.md",
                Tone::Ok,
                "quitado /h/skills/jw/SKILL.md".into(),
            ),
            (
                "kept caveman: it was edited",
                Tone::Info,
                "caveman: tiene cambios tuyos; no se borra lo que editaste".into(),
            ),
            (
                "caveman is not installed",
                Tone::Info,
                "caveman no está instalado".into(),
            ),
            (
                &format!("brain is built in: always available with {bin} role show brain"),
                Tone::Info,
                format!(
                    "brain viene dentro de {bin}: siempre disponible con {bin} role show brain"
                ),
            ),
            (
                "brain is built in: it is always available, there is nothing to turn off",
                Tone::Info,
                format!("brain viene dentro de {bin}: no hay nada que quitar"),
            ),
            (
                &format!(
                    "{bin} does not uninstall plugins; to remove it run: claude plugin uninstall p@m"
                ),
                Tone::Info,
                format!(
                    "{bin} no desinstala plugins; para quitarlo corre: claude plugin uninstall p@m"
                ),
            ),
            (
                "interrupted: nothing installed",
                Tone::Info,
                "interrumpido: no se instaló nada".into(),
            ),
        ] {
            assert_eq!(spanish(line, false), Some((tone, text)), "{line}");
        }
        let new = "/h/skills/jw/SKILL.md.new";
        let kept = spanish(
            &format!(
                "kept /h/skills/jw/SKILL.md: not written by {bin} or edited; the new one is {new}"
            ),
            false,
        )
        .unwrap();
        assert_eq!(kept.0, Tone::Info);
        assert!(
            kept.1.contains("se deja el tuyo") && kept.1.ends_with(new),
            "{kept:?}"
        );
        // A plugin that ran says it was installed; one that was there already says so.
        let ran = vec![
            "$ claude plugin install p@m".to_owned(),
            "plugins: p@m is installed".to_owned(),
        ];
        assert_eq!(result(&ran), (Tone::Ok, "p@m instalado".to_owned()));
        assert_eq!(
            result(&["plugins: p@m is installed".to_owned()]),
            (Tone::Info, "p@m ya está instalado".to_owned())
        );
        // A line with no Spanish form is shown as the library says it.
        assert_eq!(
            result(&["something new".to_owned()]),
            (Tone::Info, "something new".to_owned())
        );
    }

    #[test]
    fn a_real_install_tells_where_it_fetches_the_commit_the_files_and_the_lock() {
        let world = World::new();
        let mut w = Worker::new(world.place.clone());
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        goto(&mut s, "caveman");
        let act = s.press(KeyCode::Char(' '));
        let seen = pump_seen(&mut s, &mut w, act);
        assert!(
            matches!(&seen[..], [Event::Fetching { url, .. }, Event::Fetched { files: 1, .. }]
                if url.ends_with("/gh/juliusbrussee/caveman")),
            "{seen:?}"
        );
        assert!(matches!(s.mode, Mode::Sheet(_)));
        let act = s.press(KeyCode::Char('y'));
        let seen = pump_seen(&mut s, &mut w, act);
        assert_eq!(seen, [Event::Writing { files: 1 }, Event::Locking]);
        assert!(s.busy.is_none());
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Ok));
    }

    #[test]
    fn a_plugin_that_fails_keeps_its_command_whole_in_the_line_and_is_not_said_to_install_nothing()
    {
        let world = World::new();
        std::fs::write(world.dir.path().join("install.fails"), "").unwrap();
        let mut w = Worker::new(world.place.clone());
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        goto(&mut s, "ponytail");
        let act = s.press(KeyCode::Char(' '));
        pump(&mut s, &mut w, act);
        let act = s.press(KeyCode::Char('y'));
        let seen = pump_seen(&mut s, &mut w, act);
        assert!(
            seen.iter().any(|e| matches!(e, Event::Running(c) if c.ends_with("plugin install ponytail@ponytail"))),
            "{seen:?}"
        );
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Err));
        let text = msg(&s);
        assert!(
            text.contains("plugin install ponytail@ponytail failed") && text.ends_with(": nope"),
            "{text}"
        );
        assert!(s.busy.is_none());
    }

    #[test]
    fn esc_during_the_download_says_interrupted_in_spanish() {
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        s.press(KeyCode::Char(' '));
        assert_eq!(s.press(KeyCode::Esc), Act::Cancel);
        s.reply(Reply::Done(Err("interrupted: nothing installed".into())));
        assert_eq!(msg(&s), "interrumpido: no se instaló nada");
        assert!(drawn(&s, 0).contains("· interrumpido: no se instaló nada"));
    }

    #[test]
    fn an_answer_that_comes_after_the_screen_was_closed_is_shown_when_it_is_opened_again() {
        let world = World::new();
        let mut w = Worker::new(world.place.clone());
        // Close while an embedded skill is being written: the answer has no screen to go to.
        w.run(Act::Read { check: false });
        w.run(Act::On(identity::get().name.clone()));
        w.run(Act::Close);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while w.late.is_none() {
            assert!(w.poll().is_empty(), "nothing reaches a closed screen");
            assert!(
                std::time::Instant::now() < until,
                "the worker did not answer"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Opened again: the list first, then what happened while it was closed.
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        assert!(w.late.is_none(), "it is said once");
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Ok));
        assert!(msg(&s).starts_with("instalado "), "{}", msg(&s));
        // A fetched thing that arrives after the close is dropped, and the line says so.
        let mut w = Worker::new(world.place.clone());
        w.tx.send(Done {
            reply: Reply::Asked(sheet()),
            ask: None,
        })
        .unwrap();
        w.tx.send(Done {
            reply: Reply::Step(Event::Locking),
            ask: None,
        })
        .unwrap();
        assert!(w.poll().is_empty());
        let mut s = Screen::new();
        pump(&mut s, &mut w, Act::Read { check: false });
        assert_eq!(
            msg(&s),
            "no se instaló nada: cerraste la pantalla antes del sí"
        );
        assert!(w.pending.is_none());
    }

    #[test]
    fn a_yes_with_nothing_held_answers_with_a_cross_and_does_not_stay_busy() {
        let mut w = Worker::new(Place::here());
        let mut s = screen(&[]);
        goto(&mut s, "caveman");
        s.reply(Reply::Asked(sheet()));
        assert_eq!(s.press(KeyCode::Char('y')), Act::Apply);
        assert!(s.busy.is_some());
        w.open = true;
        w.run(Act::Apply);
        let replies = w.poll();
        assert_eq!(replies.len(), 1);
        for r in replies {
            s.reply(r);
        }
        assert!(s.busy.is_none());
        assert_eq!(s.msg.as_ref().map(|m| m.0), Some(Tone::Err));
        assert!(msg(&s).starts_with("la hoja ya no está"), "{}", msg(&s));
    }
}
