//! What the lists of jwn hold (spec REQ-424, REQ-425): `/help`, `/status`, `/resume`, `/skills`,
//! `/tasks`, `/artifacts`, `/permissions` and `/memory`. Each is built from the conversation
//! node and the machine the client runs on, and is drawn by the conversation's list box; the
//! engine is sent nothing.

use std::path::{Path, PathBuf};

use contract::conversation::{CommandSource, Conversation, SavedConversation, TaskState};

use super::agents::Section;
use super::sections::{self, Head};
use super::{Rgb, look};

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAct {
    Nothing,
    /// Put this in the line (a skill's name).
    Insert(String),
    /// Retake this conversation in the pane.
    Resume(String),
    /// Copy this (a page's address).
    Copy(String),
    /// Open this file in the viewer (REQ-641).
    Open(PathBuf),
    /// Stop this task of the engine's (REQ-433).
    Stop(String),
    /// Put the message of this place of the history in the line, as ↑ would (REQ-717).
    Recall(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub detail: String,
    /// A word at the right with the colour it is written in.
    pub tag: Option<(String, Rgb)>,
    /// When it happened, in milliseconds since the epoch: drawn as how long ago.
    pub at: Option<u64>,
    /// The section it is listed in, when the list has sections (REQ-803).
    pub group: Option<Head>,
    pub act: RowAct,
}

impl Row {
    fn new(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            label: label.into(),
            detail: detail.into(),
            tag: None,
            at: None,
            group: None,
            act: RowAct::Nothing,
        }
    }

    fn tagged(mut self, tag: &str, colour: Rgb) -> Row {
        self.tag = Some((tag.to_owned(), colour));
        self
    }

    fn doing(mut self, act: RowAct) -> Row {
        self.act = act;
        self
    }
}

/// A list: its title, its rows and what it says when it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    pub rows: Vec<Row>,
    pub empty: String,
}

/// The names of the lists, `resume` and `continue` being one.
pub fn is_panel(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "status"
            | "resume"
            | "continue"
            | "skills"
            | "tasks"
            | "artifacts"
            | "permissions"
            | "memory"
            | "keys"
    )
}

/// The keys of the conversation, by what they are for (REQ-599): `/help` lists them before the
/// commands and `^/` lists only them.
/// The group of the keys that are the program's own: its tag is the product name.
const OWN: &str = "\0";

const KEYS: [(&str, &str, &str); 22] = [
    ("↵", "enviar; en cola mientras Claude trabaja", "escribir"),
    ("⇧↵ · ^J", "un salto de línea", "escribir"),
    ("⇥", "completar lo que se propone o la lista", "escribir"),
    (
        "/",
        "los comandos, en cualquier parte del mensaje",
        "escribir",
    ),
    ("@", "los archivos del workspace", "escribir"),
    (
        "RePág · AvPág",
        "en la lista de comandos, de una sección a otra",
        "escribir",
    ),
    ("!", "una línea de shell", "escribir"),
    ("^V", "pegar una imagen del portapapeles", "escribir"),
    ("esc · ^C", "interrumpir lo que Claude hace", "Claude"),
    (
        "⇧tab",
        "cambiar el modo, también mientras trabaja",
        "Claude",
    ),
    ("^B", "los agentes y las tareas", "Claude"),
    ("^T", "la lista de pendientes", "Claude"),
    (
        "^O",
        "abrir o cerrar la salida de las herramientas",
        "Claude",
    ),
    (
        "↑ ↓",
        "en la primera o la última fila del campo: los mensajes enviados",
        "escribir",
    ),
    (
        "^R",
        "la lista de los mensajes enviados, con filtro",
        "escribir",
    ),
    ("↑ ↓", "con un mensaje elegido: pasar al siguiente", "ver"),
    ("c · t", "copiar el mensaje elegido o solo su texto", "ver"),
    ("PgUp · PgDn", "recorrer la conversación", "ver"),
    ("v", "abrir el plan completo", "ver"),
    ("^/ · ?", "esta lista", OWN),
    (
        "^␣",
        "el líder: salir al terminal, espacios, paneles, diff",
        OWN,
    ),
    ("^␣ ?", "las teclas del líder", OWN),
];

/// The keys as a list (REQ-599).
pub fn keys() -> Panel {
    Panel {
        title: "teclas".into(),
        rows: KEYS
            .iter()
            .map(|(k, d, group)| {
                let tag = if *group == OWN {
                    identity::get().name.as_str()
                } else {
                    group
                };
                Row::new(*k, *d).tagged(tag, look::dim())
            })
            .collect(),
        empty: String::new(),
    }
}

pub fn help(conv: &Conversation) -> Panel {
    let keys = KEYS
        .iter()
        .map(|(k, d, _)| Row::new(*k, *d).tagged("tecla", look::dim()));
    let commands = conv
        .commands
        .iter()
        .filter(|c| c.source == CommandSource::Jwn)
        .map(|c| {
            Row::new(c.name.clone(), c.description.clone().unwrap_or_default())
                .tagged(&identity::get().name, look::blue())
                .doing(RowAct::Insert(format!("{} ", c.name)))
        });
    Panel {
        title: "ayuda".into(),
        rows: keys.chain(commands).collect(),
        empty: String::new(),
    }
}

pub fn status(conv: &Conversation, cwd: &str) -> Panel {
    let mut rows = vec![
        Row::new("modelo", conv.model.clone()),
        Row::new("esfuerzo", conv.effort.clone()),
        Row::new("modo", conv.mode.flag().to_owned()),
        Row::new("sesión", conv.session.clone().unwrap_or_default()),
        Row::new("carpeta", cwd),
        Row::new(
            "costo",
            format!(
                "${}.{:02}",
                conv.cost / 1_000_000,
                (conv.cost % 1_000_000) / 10_000
            ),
        ),
    ];
    if let Some(l) = &conv.limits {
        let pct = |p: Option<u32>| p.map_or("—".to_owned(), |p| format!("{p} %"));
        rows.push(Row::new("límite de 5 horas", pct(l.five_hour)));
        rows.push(Row::new("límite de 7 días", pct(l.seven_day)));
    }
    Panel {
        title: "estado".into(),
        rows,
        empty: String::new(),
    }
}

pub fn skills(conv: &Conversation) -> Panel {
    let mut skills: Vec<_> = conv
        .commands
        .iter()
        .filter(|c| {
            matches!(
                c.source,
                CommandSource::Skill | CommandSource::Project | CommandSource::Plugin
            )
        })
        .collect();
    skills.sort_by_key(|c| sections::order(c, ""));
    let rows = skills
        .into_iter()
        .map(|c| {
            let mut row = Row::new(c.name.clone(), c.description.clone().unwrap_or_default())
                .doing(RowAct::Insert(format!("{} ", c.name)));
            row.group = Some(sections::slot(c).1);
            row
        })
        .collect();
    Panel {
        title: "skills y comandos".into(),
        rows,
        empty: "Nada de tu proyecto, tus skills o tus plugins.".into(),
    }
}

pub fn tasks(conv: &Conversation) -> Panel {
    // By section (subagents, shells, monitors), the newest first in each, each with how long ago it
    // began and the kind it is; `x` stops the one that runs (REQ-433, REQ-601).
    let mut newest: Vec<_> = conv.tasks.iter().collect();
    newest.sort_by_key(|t| (Section::of(t.kind) as usize, std::cmp::Reverse(t.started)));
    let rows = newest
        .into_iter()
        .map(|t| {
            let (state, colour) = match t.state {
                TaskState::Run => ("en curso", look::yellow()),
                TaskState::Done => ("terminó", look::green()),
                TaskState::Stopped => ("detenido", look::dim()),
                TaskState::Failed => ("falló", look::red()),
            };
            let tag = format!("{} · {state}", Section::of(t.kind).word());
            let row =
                Row::new(t.label.clone(), t.info.clone().unwrap_or_default()).tagged(&tag, colour);
            Row {
                at: Some(t.started),
                act: if t.state == TaskState::Run {
                    RowAct::Stop(t.id.clone())
                } else {
                    RowAct::Nothing
                },
                ..row
            }
        })
        .collect();
    Panel {
        title: "segundo plano · x detiene".into(),
        rows,
        empty: "Nada corre en segundo plano.".into(),
    }
}

pub fn artifacts(conv: &Conversation) -> Panel {
    let rows = conv
        .artifacts
        .iter()
        .map(|a| Row::new(a.title.clone(), a.url.clone()).doing(RowAct::Copy(a.url.clone())))
        .collect();
    Panel {
        title: "páginas publicadas".into(),
        rows,
        empty: "Claude no ha publicado ninguna página en esta conversación.".into(),
    }
}

/// The conversations Claude saved for the folder, newest first.
pub fn saved(list: &[SavedConversation]) -> Panel {
    let rows = list
        .iter()
        .map(|c| Row {
            label: c.name.clone(),
            detail: String::new(),
            tag: Some((c.id.chars().take(8).collect(), look::dim())),
            at: Some(c.at),
            group: None,
            act: RowAct::Resume(c.id.clone()),
        })
        .collect();
    Panel {
        title: "retomar una conversación".into(),
        rows,
        empty: "No hay conversaciones guardadas de esta carpeta.".into(),
    }
}

/// The messages sent, newest first, for `^R` (REQ-717): each is one line, with its rows said at
/// the right when it has several, and choosing one recalls it.
pub fn sent(messages: &[super::paste::Sent]) -> Panel {
    let rows = messages
        .iter()
        .enumerate()
        .rev()
        .map(|(i, m)| {
            let rows = m.text.lines().count();
            Row {
                label: m.text.replace('\n', " ⏎ "),
                detail: String::new(),
                tag: (rows > 1).then(|| (format!("{rows} líneas"), look::dim())),
                at: None,
                group: None,
                act: RowAct::Recall(i),
            }
        })
        .collect();
    Panel {
        title: "mensajes enviados".into(),
        rows,
        empty: "No se ha enviado nada todavía.".into(),
    }
}

/// The permission rules the person's settings hold, read only: the user's, the project's and
/// the project's local ones.
pub fn permissions(cwd: &Path, home: Option<&Path>) -> Panel {
    let files = [
        ("usuario", home.map(|h| h.join(".claude/settings.json"))),
        ("proyecto", Some(cwd.join(".claude/settings.json"))),
        ("local", Some(cwd.join(".claude/settings.local.json"))),
    ];
    let mut rows = Vec::new();
    for (whose, path) in files {
        let Some(doc) = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            continue;
        };
        let perms = &doc["permissions"];
        if let Some(mode) = perms["defaultMode"].as_str() {
            rows.push(Row::new("modo por defecto", mode).tagged(whose, look::dim()));
        }
        for (kind, word, colour) in [
            ("allow", "permitir", look::green()),
            ("ask", "preguntar", look::yellow()),
            ("deny", "denegar", look::red()),
        ] {
            for rule in perms[kind]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str())
            {
                rows.push(Row::new(rule, whose).tagged(word, colour));
            }
        }
    }
    Panel {
        title: "permisos (solo lectura)".into(),
        rows,
        empty: "Tus ajustes no tienen reglas de permisos.".into(),
    }
}

/// The memory files that exist: the person's `CLAUDE.md` and the project's.
pub fn memory(cwd: &Path, home: Option<&Path>) -> Panel {
    let candidates = [
        ("usuario", home.map(|h| h.join(".claude/CLAUDE.md"))),
        ("proyecto", Some(cwd.join("CLAUDE.md"))),
        ("proyecto", Some(cwd.join("AGENTS.md"))),
        ("local", Some(cwd.join("CLAUDE.local.md"))),
    ];
    let rows = candidates
        .into_iter()
        .filter_map(|(whose, path)| {
            let path = path?;
            let size = std::fs::metadata(&path).ok()?.len();
            let shown = path.to_string_lossy().into_owned();
            Some(
                Row::new(shown, format!("{size} bytes"))
                    .tagged(whose, look::dim())
                    .doing(RowAct::Open(path)),
            )
        })
        .collect();
    Panel {
        title: "memoria".into(),
        rows,
        empty: "No hay CLAUDE.md ni AGENTS.md.".into(),
    }
}

/// How long ago `at` was, as the lists say it.
pub fn ago(now_ms: u64, at: u64) -> String {
    let s = now_ms.saturating_sub(at) / 1000;
    match s {
        0..=59 => "ahora".into(),
        60..=3599 => format!("hace {} min", s / 60),
        3600..=86_399 => format!("hace {} h", s / 3600),
        _ => format!("hace {} d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fixture() -> Conversation {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/fixtures/daemon");
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("snapshot-conversation.json")).unwrap(),
        )
        .unwrap();
        serde_json::from_value(doc["node"].clone()).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "{}-panels-{}-{name}",
            identity::get().name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        d
    }

    #[test]
    fn the_names_of_the_lists_are_known_and_resume_and_continue_are_one() {
        for n in [
            "help",
            "status",
            "resume",
            "continue",
            "skills",
            "tasks",
            "artifacts",
            "permissions",
            "memory",
        ] {
            assert!(is_panel(n), "{n}");
        }
        assert!(!is_panel("model") && !is_panel("rewind"));
    }

    #[test]
    fn help_holds_the_keys_and_jwns_commands() {
        let p = help(&fixture());
        let labels: Vec<&str> = p.rows.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"⇧tab") && labels.contains(&"^V"));
        assert!(
            labels.contains(&"/exit") && labels.contains(&"/tasks"),
            "{labels:?}"
        );
        assert!(!labels.contains(&"/context"), "Claude's are not jwn's");
        assert_eq!(
            p.rows.iter().find(|r| r.label == "/exit").unwrap().act,
            RowAct::Insert("/exit ".into())
        );
    }

    #[test]
    fn status_says_what_the_conversation_runs_with_and_what_it_has_used() {
        let p = status(&fixture(), "/work/app");
        let at = |k: &str| p.rows.iter().find(|r| r.label == k).unwrap().detail.clone();
        assert_eq!(at("modelo"), "sonnet-5.5");
        assert_eq!(at("modo"), "manual");
        assert_eq!(at("carpeta"), "/work/app");
        assert_eq!(at("costo"), "$0.06");
        assert_eq!(at("límite de 5 horas"), "62 %");
        assert_eq!(at("límite de 7 días"), "31 %");
    }

    #[test]
    fn skills_are_the_commands_that_are_not_claudes_or_jwns_and_choosing_one_puts_it_in_the_line() {
        let p = skills(&fixture());
        let labels: Vec<&str> = p.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["/jwn", "/deploy", "/vercel:deploy"],
            "by section: Personales, Proyecto, plugins (REQ-803)"
        );
        assert_eq!(p.rows[0].act, RowAct::Insert("/jwn ".into()));
        let titles: Vec<_> = p
            .rows
            .iter()
            .map(|r| r.group.as_ref().unwrap().title.as_str())
            .collect();
        assert_eq!(titles, ["Personales", "Proyecto", "vercel"]);
        assert!(p.rows.iter().all(|r| r.tag.is_none()), "no word of origin");
    }

    #[test]
    fn tasks_and_artifacts_come_from_the_node_and_say_so_when_there_are_none() {
        let c = fixture();
        let t = tasks(&c);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].tag.as_ref().unwrap().0, "shell · en curso");
        assert_eq!(t.rows[1].tag.as_ref().unwrap().0, "monitor · detenido");
        let a = artifacts(&c);
        assert_eq!(
            a.rows[0].act,
            RowAct::Copy("https://claude.ai/artifact/6HbvGgivzfNgDnkzpDjf4Y".into())
        );
        let mut none = c;
        none.tasks.clear();
        none.artifacts.clear();
        assert!(tasks(&none).rows.is_empty() && !tasks(&none).empty.is_empty());
        assert!(artifacts(&none).rows.is_empty());
    }

    #[test]
    fn the_saved_conversations_resume_by_their_id() {
        let p = saved(&[SavedConversation {
            id: "2cab1f5a-3d20-4cd2-9d13-485b39b26147".into(),
            name: "arregla el login".into(),
            at: 5,
        }]);
        assert_eq!(
            p.rows[0].act,
            RowAct::Resume("2cab1f5a-3d20-4cd2-9d13-485b39b26147".into())
        );
        assert_eq!(p.rows[0].tag.as_ref().unwrap().0, "2cab1f5a");
        assert_eq!(p.rows[0].at, Some(5));
    }

    #[test]
    fn permissions_are_the_rules_of_the_settings_files_that_exist() {
        let cwd = scratch("perm");
        std::fs::write(
            cwd.join(".claude/settings.json"),
            r#"{"permissions": {"defaultMode": "acceptEdits", "allow": ["Bash(cargo test:*)"], "deny": ["Read(./.env)"]}}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"permissions": {"ask": ["Bash(git push:*)"]}}"#,
        )
        .unwrap();
        let rows = permissions(&cwd, None).rows;
        let find = |l: &str| rows.iter().find(|r| r.label == l).unwrap().clone();
        assert_eq!(find("Bash(cargo test:*)").tag.unwrap().0, "permitir");
        assert_eq!(find("Read(./.env)").tag.unwrap().0, "denegar");
        assert_eq!(find("Bash(git push:*)").tag.unwrap().0, "preguntar");
        assert_eq!(find("Bash(git push:*)").detail, "local");
        assert_eq!(find("modo por defecto").detail, "acceptEdits");
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    fn memory_lists_the_files_that_exist_and_opens_them() {
        let cwd = scratch("memory");
        std::fs::write(cwd.join("AGENTS.md"), "reglas").unwrap();
        let p = memory(&cwd, None);
        assert_eq!(p.rows.len(), 1, "{:?}", p.rows);
        assert_eq!(p.rows[0].detail, "6 bytes");
        assert_eq!(p.rows[0].act, RowAct::Open(cwd.join("AGENTS.md")));
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    fn how_long_ago_is_said_in_the_biggest_unit() {
        let now = 10 * 86_400_000;
        assert_eq!(ago(now, now - 30_000), "ahora");
        assert_eq!(ago(now, now - 5 * 60_000), "hace 5 min");
        assert_eq!(ago(now, now - 3 * 3_600_000), "hace 3 h");
        assert_eq!(ago(now, now - 2 * 86_400_000), "hace 2 d");
    }
}
