//! A conversation: what the engine says, turned into the state a `conversation` node shows
//! (spec: *The conversation*, REQ-371 and following; what the engine says, `docs/claude-engine.md`).
//!
//! Everything here is a function of the state and one message and returns what the daemon must
//! do about it (`Effect`), so it is tested with recorded messages and no process. The processes,
//! the threads and the lock are `engine.rs`'s.

use std::collections::{BTreeMap, VecDeque};

use contract::conversation::{
    Accent, Artifact, Ask, AskKind, AskOption, CommandKind, CommandSource, ConvCommand, ConvStatus,
    Conversation, DiffLine, DiffSign, DividerKind, Held, Image, Item, Limits, Mode, Running, Task,
    TaskKind, TaskState, Todo, Tone, ToolState,
};
use contract::{AttentionKind, Head, Id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How many items a node keeps (REQ-392).
pub const KEEP: usize = 300;

/// The longest suggestion that is kept (REQ-572).
const SUGGESTION_CHARS: usize = 400;

/// The row of a consultation of the advisor (REQ-570): the name Claude Code gives it, and what it
/// says when the advisor has answered.
pub(crate) const ADVISING: &str = "Advising";
pub(crate) const ADVISED: &str = "✔ El asesor revisó la conversación y aplicará sus comentarios";

/// The row of a consultation of the advisor, while it runs.
pub(crate) fn advisor_row(id: &str) -> Item {
    Item::Tool {
        id: id.to_owned(),
        name: ADVISING.into(),
        arg: String::new(),
        state: ToolState::Run,
        summary: None,
        output: Vec::new(),
        diff: Vec::new(),
        url: None,
        by: None,
        lost: false,
    }
}

/// What the `advisor_tool_result` block makes of its row: its state and what it says.
pub(crate) fn advisor_answer(block: &Value) -> (ToolState, String) {
    if block["content"]["type"]
        .as_str()
        .is_some_and(|t| t.contains("error"))
    {
        (ToolState::Failed, "el asesor no pudo responder".into())
    } else {
        (ToolState::Ok, ADVISED.into())
    }
}
/// The most a saved conversation takes on disk.
pub const MAX_SAVED: usize = 2 * 1024 * 1024;
pub(crate) const OUTPUT_LINES: usize = 40;
const ARG_CHARS: usize = 200;
pub(crate) const LINE_CHARS: usize = 300;
/// The lines of a plan the node keeps.
const PLAN_LINES: usize = 200;
/// The tasks of the engine the node keeps, and the steps of a subagent.
const TASKS_KEPT: usize = 50;
const STEPS_KEPT: usize = 60;
/// The most of a subagent's prompt the node keeps.
const PROMPT_CHARS: usize = 600;
/// The most of what a task said at its end the node keeps.
const SAID_CHARS: usize = 400;

/// What `claude` was typed with, as the engine serves it (REQ-372). Anything else is not served
/// and the original runs (REQ-373).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OpenArgs {
    pub resume: Option<String>,
    pub continue_latest: bool,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<Mode>,
    pub add_dirs: Vec<String>,
    pub settings: Option<String>,
    pub agent: Option<String>,
    /// `--chrome` or `--no-chrome`, when the person typed one (REQ-634).
    pub chrome: Option<bool>,
    /// `--prompt-suggestions <true|false>`, which `jwn claude` adds when `config.toml` turns the
    /// suggestions off (REQ-572).
    pub prompt_suggestions: Option<String>,
    pub message: Option<String>,
}

/// The flags the conversation takes a value for, with their long names.
const VALUED: [(&str, &str); 9] = [
    ("-r", "--resume"),
    ("--resume", "--resume"),
    ("--session-id", "--session-id"),
    ("-n", "--name"),
    ("--name", "--name"),
    ("--model", "--model"),
    ("--effort", "--effort"),
    ("--permission-mode", "--permission-mode"),
    ("--add-dir", "--add-dir"),
];

/// Commands of Claude's own that are not a conversation: the original runs them.
const SUBCOMMANDS: [&str; 9] = [
    "mcp",
    "config",
    "update",
    "doctor",
    "install",
    "auth",
    "plugin",
    "agents",
    "migrate-installer",
];

/// The id of the `initialize` request every engine is sent when it starts.
pub const INIT_REQUEST: &str = "jwn-init";

/// The request that asks an engine what it can do: when it starts and when a reload ends.
fn initialize() -> Effect {
    Effect::Write(json!({
        "type": "control_request",
        "request_id": INIT_REQUEST,
        "request": {"subtype": "initialize"},
    }))
}

/// Commands of Claude's that need its own screen: they are not sent, and the client points at
/// `command claude` (REQ-423).
pub const SCREEN: [&str; 12] = [
    "login",
    "logout",
    "vim",
    "theme",
    "terminal-setup",
    "ide",
    "chrome",
    "doctor",
    "insights",
    "upgrade",
    "keybindings",
    "release-notes",
];

/// Commands of Claude's that start a turn like any message.
const PROMPTS: [&str; 3] = ["init", "review", "security-review"];

/// The commands of Claude's own screen that the engine run headless does not serve, from its
/// commands page: they are listed so that the list has all of Claude's, and the client says
/// where they work (`command claude`) instead of sending them (REQ-423, REQ-460).
const INTERACTIVE: &[(&str, &str)] = &[
    ("add-dir", "añadir una carpeta de trabajo"),
    (
        "artifact-capabilities",
        "referencia de capacidades de los artifacts",
    ),
    ("artifact-diagramming", "guía para diagramas en artifacts"),
    (
        "autofix-pr",
        "un agente en la nube arregla el PR de la rama",
    ),
    ("background", "pasar la sesión a segundo plano"),
    ("branch", "bifurcar la conversación en este punto"),
    ("btw", "una pregunta lateral sin tocar la conversación"),
    ("bug", "reportar un error"),
    ("cd", "mover la sesión a otra carpeta"),
    ("chrome", "configurar Claude en Chrome"),
    ("claude-in-chrome", "hacer una tarea en tu navegador"),
    ("copy", "copiar la última respuesta"),
    ("design-login", "dar acceso a Claude Design"),
    ("desktop", "seguir la sesión en la app de escritorio"),
    ("diff", "revisar los cambios del árbol de trabajo"),
    ("doctor", "diagnosticar la instalación"),
    ("export", "exportar la conversación como texto"),
    ("feedback", "enviar comentarios sobre Claude Code"),
    (
        "fork",
        "copiar la conversación a una sesión en segundo plano",
    ),
    ("hooks", "ver los hooks configurados"),
    ("ide", "integraciones con editores"),
    ("insights", "informe de tus sesiones recientes"),
    ("install-github-app", "instalar la app de GitHub de Claude"),
    ("install-slack-app", "instalar la app de Slack de Claude"),
    ("keybindings", "abrir el archivo de atajos"),
    ("login", "iniciar sesión"),
    ("logout", "cerrar sesión"),
    ("mobile", "QR de la app móvil de Claude"),
    ("passes", "compartir una semana gratis de Claude Code"),
    ("plugin", "gestionar plugins"),
    ("powerup", "lecciones interactivas de funciones"),
    ("privacy-settings", "ajustes de privacidad"),
    ("radio", "radio lo-fi de Claude FM"),
    (
        "rate-limit-options",
        "opciones cuando un límite de uso te frena",
    ),
    ("release-notes", "notas de las versiones"),
    ("remote-control", "controlar la sesión desde claude.ai"),
    ("remote-env", "entorno en la nube por defecto"),
    ("rewind", "volver a un punto anterior"),
    ("sandbox", "activar o desactivar el sandbox"),
    ("scroll-speed", "velocidad de la rueda del ratón"),
    ("setup-bedrock", "configurar Amazon Bedrock"),
    ("setup-vertex", "configurar Google Vertex"),
    ("slides", "hacer una presentación"),
    ("statusline", "configurar la línea de estado"),
    ("stickers", "pedir stickers de Claude Code"),
    ("stop", "parar la sesión en segundo plano"),
    (
        "subtask",
        "un subagente en segundo plano con esta conversación",
    ),
    ("teleport", "traer una sesión de la nube a este terminal"),
    ("terminal-setup", "atajo Shift+Enter para saltos de línea"),
    ("theme", "cambiar el tema de color"),
    ("tui", "elegir el modo de pantalla de Claude"),
    ("ultrareview", "revisión profunda en la nube"),
    ("upgrade", "cambiar a un plan superior"),
    ("voice", "dictado por voz"),
    ("web-setup", "conectar GitHub para sesiones en la nube"),
    ("workflows", "progreso de los workflows"),
];

/// The lists jwn draws itself (REQ-424): a client opens them from its own state and sends
/// nothing to the engine. `continue` is `resume`'s other name.
const PANELS: [(&str, &str); 9] = [
    ("help", "las teclas y los comandos propios"),
    ("status", "modelo, esfuerzo, modo, sesión y límites"),
    ("resume", "retomar una conversación de esta carpeta"),
    ("continue", "retomar una conversación de esta carpeta"),
    ("tasks", "trabajo en segundo plano"),
    ("artifacts", "las páginas publicadas en esta conversación"),
    ("skills", "las skills, comandos y plugins disponibles"),
    ("permissions", "las reglas de permisos (solo lectura)"),
    ("memory", "CLAUDE.md y AGENTS.md: abrir en un editor"),
];

/// What jwn answers itself (REQ-418, REQ-419), whatever the engine lists under the same name.
const OWN: [(&str, &str); 8] = [
    ("exit", "salir de la conversación"),
    (
        "plan",
        "entrar en modo plan, y enviar lo que sigue si hay algo",
    ),
    ("rename", "cambiar el nombre de la conversación"),
    ("color", "cambiar el color de la conversación"),
    ("model", "cambiar el modelo"),
    ("effort", "cambiar el esfuerzo"),
    ("mode", "cambiar el modo de permisos"),
    ("cost", "lo que ha costado esta conversación"),
];

/// The commands every conversation has before the engine says anything.
pub fn own_commands() -> Vec<ConvCommand> {
    let own = OWN.iter().map(|(name, description)| ConvCommand {
        name: format!("/{name}"),
        source: CommandSource::Jwn,
        kind: CommandKind::Local,
        description: Some((*description).to_owned()),
        group: None,
    });
    let panels = PANELS.iter().map(|(name, description)| ConvCommand {
        name: format!("/{name}"),
        source: CommandSource::Jwn,
        kind: CommandKind::Panel,
        description: Some((*description).to_owned()),
        group: None,
    });
    let screens = INTERACTIVE.iter().map(|(name, description)| ConvCommand {
        name: format!("/{name}"),
        source: CommandSource::Claude,
        kind: CommandKind::Screen,
        description: Some((*description).to_owned()),
        group: None,
    });
    own.chain(panels).chain(screens).collect()
}

/// Where the engine says a command that is not Claude's is defined, which it says in the end of
/// the description (or the beginning, for a plugin's) and in no field (REQ-792): the source, the
/// plugin's name when it is one, and the description without that mark.
fn origin<'a>(
    name: &str,
    description: &'a str,
) -> Option<(CommandSource, Option<String>, &'a str)> {
    if let Some((plugin, _)) = name.split_once(':') {
        let rest = description
            .strip_prefix('(')
            .and_then(|d| d.strip_prefix(plugin))
            .and_then(|d| d.strip_prefix(") "))
            .unwrap_or(description);
        return Some((CommandSource::Plugin, Some(plugin.to_owned()), rest));
    }
    let marks = [
        ("(user)", CommandSource::Skill, None),
        ("(project)", CommandSource::Project, None),
        ("(claude.ai sync)", CommandSource::Plugin, Some("claude.ai")),
    ];
    let text = description.trim_end();
    marks.into_iter().find_map(|(mark, source, group)| {
        let rest = text.strip_suffix(mark)?;
        Some((source, group.map(str::to_owned), rest.trim_end()))
    })
}

/// The `/` list: jwn's own, then what the engine lists, each tagged by where it is defined
/// (REQ-417, REQ-792). `skills` are the names the engine's `init` calls skills, when it has said
/// them; they tell a skill from a project's command for the ones the engine marks nothing on
/// (REQ-793), whose names come back as the second part.
fn classify(listed: &Value, skills: Option<&[&str]>) -> (Vec<ConvCommand>, Vec<String>) {
    let mut out = own_commands();
    let mut unmarked = Vec::new();
    for c in listed.as_array().into_iter().flatten() {
        let Some(name) = c["name"].as_str().filter(|n| !n.is_empty()) else {
            continue;
        };
        if OWN.iter().any(|(own, _)| *own == name) || PANELS.iter().any(|(p, _)| *p == name) {
            continue;
        }
        let described = c["description"].as_str().unwrap_or("");
        let (mut group, mut description) = (None, described);
        let (source, kind) = if c["builtin"].as_bool().unwrap_or(false) {
            let kind = if SCREEN.contains(&name) {
                CommandKind::Screen
            } else if PROMPTS.contains(&name) {
                CommandKind::Prompt
            } else {
                CommandKind::Text
            };
            (CommandSource::Claude, kind)
        } else if let Some((source, g, rest)) = origin(name, described) {
            (group, description) = (g, rest);
            (source, CommandKind::Prompt)
        } else {
            unmarked.push(name.to_owned());
            let source = if skills.is_some_and(|s| !s.contains(&name)) {
                CommandSource::Project
            } else {
                CommandSource::Skill
            };
            (source, CommandKind::Prompt)
        };
        // What the engine serves replaces what was only listed.
        out.retain(|k| k.name.trim_start_matches('/') != name);
        out.push(ConvCommand {
            name: format!("/{name}"),
            source,
            kind,
            description: Some(description)
                .filter(|d| !d.is_empty())
                .map(|d| clip(d, 160)),
            group,
        });
    }
    (out, unmarked)
}

#[cfg(test)]
fn classify_commands(listed: &Value, skills: Option<&[&str]>) -> Vec<ConvCommand> {
    classify(listed, skills).0
}

pub fn parse_args(args: &[String]) -> Result<OpenArgs, String> {
    if let Some(first) = args.first().filter(|a| SUBCOMMANDS.contains(&a.as_str())) {
        return Err(format!("{first} is a command of claude's own"));
    }
    let mut out = OpenArgs::default();
    let mut words: Vec<String> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if !a.starts_with('-') || a == "-" {
            words.push(a.clone());
            continue;
        }
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        if flag == "-c" || flag == "--continue" {
            out.continue_latest = true;
            continue;
        }
        if flag == "--chrome" || flag == "--no-chrome" {
            out.chrome = Some(flag == "--chrome");
            continue;
        }
        let long = match flag {
            "--settings" | "--agent" | "--prompt-suggestions" => flag,
            other => match VALUED.iter().find(|(f, _)| *f == other) {
                Some((_, long)) => long,
                None => return Err(format!("{flag} is not served")),
            },
        };
        let value = match inline {
            Some(v) => v,
            None => match it.peek() {
                Some(v) if !v.starts_with('-') => it.next().expect("peeked").clone(),
                _ => return Err(format!("{flag} needs a value")),
            },
        };
        match long {
            "--resume" => out.resume = Some(value),
            "--session-id" => out.session_id = Some(value),
            "--name" => out.name = Some(value),
            "--model" => out.model = Some(value),
            "--effort" => out.effort = Some(value),
            "--permission-mode" => {
                out.mode = Some(
                    Mode::from_flag(&value)
                        .ok_or_else(|| format!("{value} is not a mode of a conversation"))?,
                )
            }
            "--add-dir" => out.add_dirs.push(value),
            "--settings" => out.settings = Some(value),
            "--agent" => out.agent = Some(value),
            "--prompt-suggestions" => out.prompt_suggestions = Some(value),
            _ => unreachable!("every long flag is handled"),
        }
    }
    if !words.is_empty() {
        out.message = Some(words.join(" "));
    }
    Ok(out)
}

/// What the daemon must do after a message: the state has changed already.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send this line to the engine.
    Write(Value),
    /// Raise an attention on the pane.
    Raise(AttentionKind),
    /// End the permission, question and plan attentions of the pane.
    ClearAsk,
    /// The conversation can be resumed from now on: the pane brings it back after a restart.
    Resumable(String),
    /// The answer to an ask that was waiting before a restart: no engine has the question any more, so
    /// the daemon starts one and this is the first message it is sent (REQ-879 to REQ-881).
    Resume(String),
}

/// What a local command of the engine (`/model`, `/effort`, `/rename`) changes once it answers.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Model(String),
    Effort(String),
    None,
}

/// A task whose output is read (REQ-607).
pub struct TailTarget {
    pub task: String,
    pub ended: bool,
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
struct TaskLink {
    task: String,
    tool_use_id: Option<String>,
    background: bool,
    /// Its end was already told in a note.
    noted: bool,
    /// The tail of its output was read once after it ended (REQ-607).
    final_read: bool,
}

/// What is in flight to the engine: one line at a time, so a `result` always answers something.
#[derive(Debug, Clone, PartialEq)]
enum Inflight {
    Turn,
    Local(Change),
}

#[derive(Debug, Clone)]
enum Queued {
    Text { text: String, images: Vec<Image> },
    Local { line: String, change: Change },
}

impl Queued {
    fn shown(&self) -> String {
        match self {
            Queued::Text { text, .. } => text.clone(),
            Queued::Local { line, .. } => line.clone(),
        }
    }
}

/// What a waiting question is: a permission, the questions Claude asks (one at a time) or a plan.
#[derive(Debug, Clone)]
enum Kind {
    Permission,
    Question(Box<Questions>),
    Plan,
}

/// The questions of one `AskUserQuestion`, and what was answered so far (REQ-435).
#[derive(Debug, Clone)]
struct Questions {
    all: Vec<Q>,
    /// Per question: its text, what the engine is told (the labels and the person's own words joined with
    /// `, `) and what the tool's line shows (the own words between « »).
    answered: Vec<(String, String, String)>,
}

#[derive(Debug, Clone)]
struct Q {
    question: String,
    /// (label, description) of each option.
    options: Vec<(String, String)>,
    multi: bool,
}

/// The question that waits, with what it needs to be answered.
#[derive(Debug, Clone)]
struct Pending {
    ask: Ask,
    /// The engine's own id of the request, which the reply names (the `ask`'s id is longer for the second question of a call).
    request: String,
    kind: Kind,
    input: Value,
    tool_use_id: Option<String>,
    session_rule: Option<String>,
    /// The request came from a subagent (`agent_id`): not kept for a restart (REQ-877).
    nested: bool,
}

/// What a conversation is. The first group is saved; the rest is about now.
#[derive(Debug, Clone)]
pub struct State {
    pub name: String,
    /// The person (or the first message) gave it a name.
    pub named: bool,
    pub accent: Accent,
    pub model: String,
    pub effort: String,
    pub mode: Mode,
    /// The mode the conversation was in just before it was in `plan`, so that approving the plan
    /// can go back to it (REQ-605). None when it began in plan or has left it. It is kept on
    /// disk: a conversation that was in plan when the daemon stopped still knows where it came from.
    before_plan: Option<Mode>,
    /// The engine's id for the conversation; always known, since the daemon chooses it.
    pub session: String,
    /// The engine has written its transcript, so `--resume` can read it.
    pub has_transcript: bool,
    pub first: u32,
    /// When each item was made, in milliseconds since 1970, and whether jwn made it (and did not read
    /// it from the transcript): what places the items that are jwn's own among the lines of the
    /// transcript (REQ-544). Always as long as `items`.
    stamps: Vec<(u64, bool)>,
    /// What was saved of the items that are jwn's own, until they are put among the transcript's.
    kept_saved: Vec<Kept>,
    /// It was saved with its own items, before the transcript was read (REQ-544): it keeps them.
    legacy: bool,
    /// The messages another `claude` has typed and not been taken in yet, read from the transcript (REQ-549).
    followed_queue: Vec<String>,
    pub items: Vec<Item>,
    /// Millionths of a dollar.
    pub cost: u64,

    pub status: ConvStatus,
    pub since: Option<u64>,
    pending: Option<Pending>,
    /// The permission the saved file kept, until the transcript says whether it still waits (REQ-876).
    saved_pending: Option<SavedPending>,
    /// What a restored permission was answered yes to: the call Claude makes again for it is allowed once (REQ-881).
    allow_once: Option<(String, Value)>,
    queue: VecDeque<Queued>,
    pub tasks: Vec<Task>,
    pub todos: Vec<Todo>,
    pub artifacts: Vec<Artifact>,
    pub commands: Vec<ConvCommand>,
    /// When the engine last said `commands`: the freshest list is the one `commands.json` keeps (REQ-758).
    pub commands_at: u64,
    /// The turn that runs is `/reload-plugins` or `/reload-skills`: when it ends the engine is asked again (REQ-756).
    reloading: bool,
    /// What the engine runs with when asked (`--model`): its name for the model, until its own answer says which one that is.
    wanted_model: Option<String>,
    /// The names the engine's `init` calls skills: they tell a skill from a project's command.
    skills: Option<Vec<String>>,
    /// The commands the engine marked no origin on (REQ-793): the only ones `init`'s skills may retag.
    unmarked: Vec<String>,
    pub limits: Option<Limits>,
    /// Allowed for this session: what the person said yes to for the rest of the conversation.
    allowed: Vec<String>,
    inflight: Option<Inflight>,
    /// A turn the engine started by itself (a monitor's line, a task that ended).
    auto: bool,
    interrupting: bool,
    tools_in_turn: u32,
    turn_started: u64,
    /// What the turn that runs has written, for each message of the engine: a message comes
    /// more than once with the same count, so each counts once, by its id (REQ-563).
    turn_tokens: BTreeMap<String, u64>,
    /// The id of the message the engine is writing.
    msg_id: String,
    /// The turn that another `claude` runs in the transcript this conversation follows, while the
    /// lines say one is open (REQ-565).
    followed: Option<Followed>,
    /// A conversation resumed by its id says so before the first message sent in it, and not
    /// while it is only followed (REQ-564).
    resume_divider: bool,
    /// What the engine proposed the person writes next, after the last turn (REQ-572).
    suggestion: Option<String>,
    /// The folder the engine runs in (REQ-630), known once the conversation is opened from a pane.
    pub cwd: Option<String>,
    /// The model the advisor runs on, as Claude Code names it, from Claude's settings (REQ-574).
    advisor: Option<String>,
    /// Another `claude` that has this session open, while the conversation has no engine (REQ-575).
    held: Option<Held>,
    /// What the engine that runs says its version is (`claude_code_version` of `system/init`),
    /// not saved (REQ-852).
    engine_version: Option<String>,
    /// The engine's total cost so far (its process), to know what one turn cost.
    last_total_micro: u64,
    live_texts: VecDeque<usize>,
    /// What `!command` lines printed, sent with the next message.
    shell_context: Vec<String>,
    next_request: u64,
    /// Each task the engine started: its id, the tool call that made it (a subagent's card) and whether it runs in the background.
    task_links: Vec<TaskLink>,
    /// The file of a task's output that the engine told of; the files of the others of the session
    /// are beside it (REQ-607).
    known_output: Option<std::path::PathBuf>,
    /// Tool ids the person refused, so their results say so.
    refused: Vec<String>,
}

/// What is kept on disk (REQ-392).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub v: u32,
    pub saved_at: u64,
    pub name: String,
    pub named: bool,
    pub accent: Accent,
    pub model: String,
    pub effort: String,
    pub mode: Mode,
    /// Where a conversation that is in `plan` came from (REQ-605).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_plan: Option<Mode>,
    pub session: String,
    pub has_transcript: bool,
    pub first: u32,
    /// The items of a conversation that has no transcript to read them from (REQ-544); none when it has one.
    pub items: Vec<Item>,
    /// What the transcript does not hold: the items that are jwn's own, each with its time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
    pub cost: u64,
    /// What the person asked for with `--model` and `--effort` (or `conversation_set`): the
    /// engine that comes back runs with it, and the short names above are not flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_arg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_arg: Option<String>,
    /// The `/` list the engine gave, so a conversation that comes back has it before its engine does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<ConvCommand>,
    /// The permission that waited when it was saved: the transcript cannot tell it from a tool that runs (REQ-876).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<SavedPending>,
}

/// A permission that waits, as the saved file keeps it (REQ-876, REQ-877).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPending {
    pub tool_use_id: String,
    pub tool: String,
    pub input: Value,
    /// «Allow for this session» was offered.
    #[serde(default)]
    pub suggested: bool,
}

pub const SAVED_VERSION: u32 = 1;

/// How long a turn that another `claude` runs may write nothing before it is taken as left
/// behind (it crashed or was closed): a tool or a subagent can be silent for minutes, and the
/// lines of a subagent count.
const FOLLOW_SILENCE_MS: u64 = 15 * 60 * 1000;

/// A turn of another `claude` that is open: when it began, the time of its last line and what it
/// has written.
#[derive(Debug, Clone, Copy)]
struct Followed {
    since: u64,
    at: u64,
    tokens: u64,
}

/// An item that the transcript of the engine does not hold, and the time it was made at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub at: u64,
    pub item: Item,
}

/// Whether an item is jwn's own: what the engine's transcript never says. The person's messages,
/// Claude's text, its tools and the compactions are the transcript's.
fn is_ours(item: &Item) -> bool {
    match item {
        Item::User { .. } | Item::Text { .. } | Item::Tool { .. } => false,
        Item::Divider { kind, .. } => *kind != DividerKind::Compact,
        Item::Agent { .. }
        | Item::Shell { .. }
        | Item::Out { .. }
        | Item::Note { .. }
        | Item::Result { .. } => true,
    }
}

pub(crate) fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The lines of a transcript with the items that are jwn's own among them, each after the last
/// line that was made no later than it. A card of a subagent takes the place of the tool that
/// started it, which is also a line of the transcript.
pub(crate) fn merge(
    base: &[crate::transcript::Stamped],
    kept: &[Kept],
) -> (Vec<Item>, Vec<(u64, bool)>) {
    let mut kept: Vec<&Kept> = kept.iter().collect();
    kept.sort_by_key(|k| k.at);
    let cards: Vec<&Kept> = kept
        .iter()
        .copied()
        .filter(|k| matches!(k.item, Item::Agent { .. }))
        .collect();
    let (mut items, mut stamps) = (Vec::new(), Vec::new());
    let mut next = kept
        .iter()
        .copied()
        .filter(|k| !matches!(k.item, Item::Agent { .. }))
        .peekable();
    for line in base {
        while let Some(k) = next.next_if(|k| k.at < line.at) {
            items.push(k.item.clone());
            stamps.push((k.at, true));
        }
        let card = match &line.item {
            Item::Tool { id, .. } => cards
                .iter()
                .find(|k| matches!(&k.item, Item::Agent { id: a, .. } if a == id)),
            _ => None,
        };
        match card {
            Some(k) => {
                items.push(k.item.clone());
                stamps.push((k.at, true));
            }
            None => {
                items.push(line.item.clone());
                stamps.push((line.at, false));
            }
        }
    }
    for k in next {
        items.push(k.item.clone());
        stamps.push((k.at, true));
    }
    // A card whose tool is not among the lines read stays, where its time puts it.
    for k in cards {
        let placed = items.contains(&k.item);
        if !placed {
            let at = stamps
                .iter()
                .position(|(t, _)| *t > k.at)
                .unwrap_or(items.len());
            items.insert(at, k.item.clone());
            stamps.insert(at, (k.at, true));
        }
    }
    (items, stamps)
}

impl State {
    pub fn new(session: String) -> State {
        State {
            name: String::new(),
            named: false,
            accent: Accent::Blue,
            model: "claude".into(),
            effort: "high".into(),
            mode: Mode::Manual,
            session,
            has_transcript: false,
            first: 0,
            stamps: Vec::new(),
            kept_saved: Vec::new(),
            legacy: false,
            followed_queue: Vec::new(),
            items: Vec::new(),
            cost: 0,
            status: ConvStatus::Idle,
            since: None,
            pending: None,
            saved_pending: None,
            allow_once: None,
            queue: VecDeque::new(),
            tasks: Vec::new(),
            todos: Vec::new(),
            artifacts: Vec::new(),
            commands: own_commands(),
            commands_at: 0,
            reloading: false,
            wanted_model: None,
            skills: None,
            unmarked: Vec::new(),
            limits: None,
            allowed: Vec::new(),
            inflight: None,
            auto: false,
            interrupting: false,
            tools_in_turn: 0,
            turn_started: 0,
            turn_tokens: BTreeMap::new(),
            msg_id: String::new(),
            followed: None,
            resume_divider: false,
            before_plan: None,
            suggestion: None,
            cwd: None,
            advisor: None,
            held: None,
            engine_version: None,
            last_total_micro: 0,
            live_texts: VecDeque::new(),
            shell_context: Vec::new(),
            next_request: 1,
            task_links: Vec::new(),
            known_output: None,
            refused: Vec::new(),
        }
    }

    /// A conversation as it was saved: its engine is gone until the next message.
    pub fn restored(saved: Saved, at: &str) -> State {
        let mut st = State::new(saved.session);
        st.name = saved.name;
        st.named = saved.named;
        st.accent = saved.accent;
        st.model = saved.model;
        st.effort = saved.effort;
        st.mode = saved.mode;
        st.before_plan = saved.before_plan.filter(|_| saved.mode == Mode::Plan);
        st.has_transcript = saved.has_transcript;
        st.saved_pending = saved.pending;
        st.first = saved.first;
        st.legacy = !saved.items.is_empty();
        st.stamps = vec![(saved.saved_at, true); saved.items.len()];
        st.items = saved.items;
        // What ran when the daemon stopped does not any more.
        st.settle_running();
        for item in &mut st.items {
            if let Item::Text { text, blocks, .. } = item {
                *blocks = crate::markdown::conversation_blocks(text);
            }
        }
        st.kept_saved = saved.kept;
        st.cost = saved.cost;
        // The list the engine gave, with jwn's own in case the file is older than they are.
        if !saved.commands.is_empty() {
            let known: Vec<String> = saved.commands.iter().map(|c| c.name.clone()).collect();
            st.commands = saved.commands;
            st.commands.extend(
                own_commands()
                    .into_iter()
                    .filter(|c| !known.contains(&c.name)),
            );
        }
        st.status = ConvStatus::Dead;
        st.push(Item::Divider {
            text: format!("el daemon se reinició · {at}"),
            kind: DividerKind::Restart,
        });
        st
    }

    /// A conversation the person resumes by its id, with nothing of ours saved: what it said, and
    /// the mode, the model and the effort it ended with, come from the engine's own transcript
    /// (REQ-540).
    pub fn resumed(session: String, source: Option<&crate::transcript::Parser>) -> State {
        let mut st = State::new(session);
        st.has_transcript = true;
        st.status = ConvStatus::Dead;
        if let Some(t) = source {
            st.follow(t);
        }
        st.resume_divider = true;
        st
    }

    /// Make the items from what the transcript says, with the items that are jwn's own put among
    /// them, and take the mode, the model and the effort it ended with (REQ-542, REQ-544).
    pub fn follow(&mut self, t: &crate::transcript::Parser) {
        self.follow_at(t, stamp());
    }

    /// `follow` at the time `now`, which says whether the turn that is open was left behind.
    fn follow_at(&mut self, t: &crate::transcript::Parser, now: u64) {
        let mut kept = std::mem::take(&mut self.kept_saved);
        kept.extend(self.kept());
        // Only the end of the lines can be among the items a node keeps; the rest is counted.
        let skip = t.items.len().saturating_sub(KEEP + kept.len());
        let (mut items, mut stamps) = merge(&t.items[skip..], &kept);
        let from = items.len().saturating_sub(KEEP);
        items.drain(..from);
        stamps.drain(..from);
        self.items = items;
        self.stamps = stamps;
        self.first = u32::try_from(skip + from).unwrap_or(u32::MAX);
        self.live_texts.clear();
        self.followed_queue = t.waiting();
        // The items are made again: the tool that asks stays the one that asks.
        let asking = self
            .pending
            .as_ref()
            .filter(|p| p.ask.restored)
            .and_then(|p| p.tool_use_id.clone());
        if let Some(id) = asking {
            self.set_tool(&id, |t| t.0 = ToolState::Ask);
        }
        if let Some(mode) = t.mode {
            self.change_mode(mode);
        }
        if let Some(model) = &t.model {
            self.model = short_model(model);
        }
        if let Some(effort) = &t.effort {
            self.effort.clone_from(effort);
        }
        self.followed = t.turn_since().map(|since| Followed {
            since,
            at: t.last_at(),
            tokens: t.turn_written(),
        });
        self.expire_follow(now);
    }

    /// A turn that nothing has written to for `FOLLOW_SILENCE_MS` is left behind: the line of
    /// work goes, and the tools that waited for a result were interrupted. Whether it changed.
    pub fn expire_follow(&mut self, now: u64) -> bool {
        let Some(f) = self.followed else {
            return false;
        };
        if now.saturating_sub(f.at) <= FOLLOW_SILENCE_MS {
            return false;
        }
        self.followed = None;
        self.settle_running();
        true
    }

    /// The tools that the transcript said run, when what ran them is not there any more: they
    /// were interrupted.
    fn settle_running(&mut self) {
        for item in &mut self.items {
            if let Item::Tool {
                state,
                summary,
                lost,
                ..
            } = item
                && *state == ToolState::Run
            {
                *state = ToolState::Failed;
                *summary = Some(crate::transcript::LOST.into());
                *lost = true;
            }
        }
    }

    /// The items for the node: the rows of the advisor say which model it is (REQ-574), which
    /// the transcript does not.
    fn shown_items(&self) -> Vec<Item> {
        let mut items = self.items.clone();
        if let Some(model) = &self.advisor {
            for item in &mut items {
                if let Item::Tool { name, arg, .. } = item
                    && name == ADVISING
                    && arg.is_empty()
                {
                    *arg = format!("using {model}");
                }
            }
        }
        items
    }

    /// What is proposed, while it can be taken: the engine is stopped, nothing waits and no turn
    /// of another `claude` is being followed.
    fn shown_suggestion(&self) -> Option<String> {
        (self.status == ConvStatus::Idle
            && self.followed.is_none()
            && !self.busy()
            && self.pending.is_none()
            && self.queue.is_empty())
        .then(|| self.suggestion.clone())
        .flatten()
    }

    /// The engine thinks, writes, runs a tool or compacts: what a workspace's row says it does
    /// (REQ-579). Waiting for an answer is not working: that is a mark of its own.
    pub fn works(&self) -> bool {
        matches!(
            self.status,
            ConvStatus::Thinking
                | ConvStatus::Streaming
                | ConvStatus::Tool
                | ConvStatus::Compacting
        )
    }

    /// Another `claude` that has the session open, or none: whether the node changed. A turn
    /// that the transcript leaves open is followed only while a `claude` that is busy has the
    /// session: with none, or one that waits for a message, nothing runs it (a turn that was
    /// cut off, or one whose process was ended, never writes the line that closes it).
    pub fn set_held(&mut self, held: Option<Held>) -> bool {
        let over = self.followed.is_some() && !held.as_ref().is_some_and(|h| h.busy);
        if over {
            self.followed = None;
            self.settle_running();
        }
        let changed = self.held != held;
        self.held = held;
        changed || over
    }

    /// A first message that the field offers before Claude proposes any (REQ-585).
    pub fn suggest(&mut self, text: String) {
        self.suggestion = Some(text);
    }

    /// The engine has said its commands (those of Claude, skills, plugins and the project's),
    /// and not only jwn's own are known.
    pub fn has_engine_commands(&self) -> bool {
        self.commands
            .iter()
            .any(|c| matches!(c.kind, CommandKind::Text | CommandKind::Prompt))
    }

    /// The list the engine gave to another conversation, for one whose engine has not started:
    /// the person types `/` before the first message (REQ-593). jwn's own stay.
    pub fn use_cached_commands(&mut self, cached: Vec<ConvCommand>) {
        if self.has_engine_commands() || cached.is_empty() {
            return;
        }
        let known: Vec<String> = cached.iter().map(|c| c.name.clone()).collect();
        self.commands = cached;
        self.commands.extend(
            own_commands()
                .into_iter()
                .filter(|c| !known.contains(&c.name)),
        );
    }

    /// The model of the advisor, for its rows (REQ-574).
    pub fn set_advisor(&mut self, model: Option<String>) {
        self.advisor = model;
    }

    /// The time of the first item the node holds, when the transcript may hold older ones: the
    /// `before` that `read_older` asks with (REQ-567).
    fn older_cursor(&self) -> Option<u64> {
        // The first item of the transcript: jwn's own items that come before it do not say where
        // in the file the window starts.
        self.leaves_chat_to_transcript()
            .then(|| {
                self.stamps
                    .iter()
                    .find(|(_, own)| !*own)
                    .or(self.stamps.first())
                    .map(|(at, _)| *at)
            })
            .flatten()
    }

    /// The conversation was saved with its own items: its chat is not read from the transcript.
    pub fn legacy(&self) -> bool {
        self.legacy
    }

    /// Whether its chat can be left out of what is saved, to be read from the transcript instead
    /// (REQ-544): the engine has written one, and the conversation is not of before it was read.
    pub fn leaves_chat_to_transcript(&self) -> bool {
        self.has_transcript && !self.legacy
    }

    /// The items that are jwn's own, with their times.
    pub fn kept(&self) -> Vec<Kept> {
        self.items
            .iter()
            .zip(&self.stamps)
            .filter(|(item, (_, here))| *here && is_ours(item))
            .map(|(item, (at, _))| Kept {
                at: *at,
                item: item.clone(),
            })
            .collect()
    }

    pub fn saved(&self, now: u64) -> Saved {
        Saved {
            v: SAVED_VERSION,
            saved_at: now,
            name: self.name.clone(),
            named: self.named,
            accent: self.accent,
            model: self.model.clone(),
            effort: self.effort.clone(),
            mode: self.mode,
            before_plan: self.before_plan,
            session: self.session.clone(),
            has_transcript: self.has_transcript,
            first: self.first,
            // The blocks are made again from the text when the file is read, so they are not kept
            // twice (REQ-560).
            items: self
                .items
                .iter()
                .map(|item| match item {
                    Item::Text { text, live, .. } => Item::Text {
                        text: text.clone(),
                        live: *live,
                        blocks: None,
                    },
                    other => other.clone(),
                })
                .collect(),
            kept: if self.legacy { Vec::new() } else { self.kept() },
            cost: self.cost,
            model_arg: None,
            effort_arg: None,
            commands: self.commands.clone(),
            pending: self.pending.as_ref().and_then(|p| {
                if !matches!(p.kind, Kind::Permission) || p.nested {
                    return None;
                }
                Some(SavedPending {
                    tool_use_id: p.tool_use_id.clone()?,
                    tool: p.ask.tool.clone(),
                    input: p.input.clone(),
                    suggested: p.session_rule.is_some(),
                })
            }),
        }
    }

    #[cfg(test)]
    fn restore_pending_for_test(&mut self, tool: &str, input: Value, id: &str) {
        let mut text = json!({"type": "user", "message": {"role": "user", "content": "hola"}})
            .to_string()
            + "\n";
        text += &json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":id,"name":tool,"input":input}]}}).to_string();
        let parser = crate::transcript::parse(&text);
        self.saved_pending = Some(SavedPending {
            tool_use_id: id.into(),
            tool: tool.into(),
            input,
            suggested: false,
        });
        self.restore_ask(Some(&parser));
    }

    /// Whether the ask `ask` waits from before a restart: no engine has it, so its answer starts one (REQ-879).
    pub fn is_restored_ask(&self, ask: &str) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.ask.id == ask && p.ask.restored)
    }

    /// Whether a permission waits that the saved file keeps: answering it takes it out of the file (REQ-877).
    pub fn records_permission(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| matches!(p.kind, Kind::Permission) && !p.nested)
    }

    /// Ask again what the conversation was waiting for when the daemon stopped (REQ-875, REQ-876): a
    /// question or a plan the transcript ends with, or the permission the saved file kept if the
    /// transcript still has its call and no result. A session another `claude` holds is not asked in.
    pub fn restore_ask(&mut self, parser: Option<&crate::transcript::Parser>) -> Vec<Effect> {
        let record = self.saved_pending.take();
        let Some(parser) = parser.filter(|_| self.held.is_none()) else {
            return Vec::new();
        };
        let (tool, input, id, suggested) = if let Some(a) = parser
            .open_ask
            .as_ref()
            .filter(|a| a.tool != "AskUserQuestion" || !parse_questions(&a.input).is_empty())
        {
            (a.tool.clone(), a.input.clone(), a.id.clone(), false)
        } else if let Some(r) = record.filter(|r| parser.has_no_result(&r.tool_use_id)) {
            (r.tool, r.input, r.tool_use_id, r.suggested)
        } else {
            return Vec::new();
        };
        // The same road a live request takes, so the ask, the tool's state and the attention are the same.
        let mut request = json!({
            "subtype": "can_use_tool",
            "tool_name": tool,
            "input": input,
            "tool_use_id": id,
        });
        if suggested {
            request["permission_suggestions"] = json!([{}]);
        }
        let fx = self.control_request(&json!({"request_id": id, "request": request}));
        if let Some(p) = &mut self.pending {
            p.ask.restored = true;
        }
        fx
    }

    /// The node's document.
    pub fn node(&self, head: Head, pane: &Id) -> Conversation {
        Conversation {
            head,
            pane: pane.clone(),
            session: Some(self.session.clone()),
            name: if self.name.is_empty() {
                self.session.chars().take(7).collect()
            } else {
                self.name.clone()
            },
            accent: self.accent,
            model: self.model.clone(),
            effort: self.effort.clone(),
            mode: self.mode,
            status: self.status,
            since: self.since,
            first: self.first,
            items: self.shown_items(),
            ask: self.pending.as_ref().map(|p| p.ask.clone()),
            queue: self
                .queue
                .iter()
                .map(Queued::shown)
                .chain(self.followed_queue.iter().cloned())
                .collect(),
            tasks: self.tasks.clone(),
            todos: self.todos.clone(),
            artifacts: self.artifacts.clone(),
            commands: self.commands.clone(),
            cost: self.cost,
            limits: self.limits.clone(),
            // Only while a turn runs, its own or the one of another `claude` that is followed.
            running: self
                .items
                .iter()
                .zip(&self.stamps)
                .filter(|_| self.since.is_some() || self.followed.is_some())
                .filter_map(|(item, (at, _))| match item {
                    Item::Tool {
                        id,
                        state: ToolState::Run,
                        ..
                    } => Some(Running {
                        id: id.clone(),
                        since: *at,
                    }),
                    _ => None,
                })
                .collect(),
            tokens: match (self.since, self.followed) {
                (Some(_), _) => self.turn_tokens.values().sum(),
                (None, Some(f)) => f.tokens,
                (None, None) => 0,
            },
            following: self.followed.map(|f| f.since),
            held: self.held.clone(),
            cwd: self.cwd.clone(),
            engine_version: self.engine_version.clone(),
            engine_newer: None,
            suggestion: self.shown_suggestion(),
            older: self.older_cursor(),
        }
    }

    /// How long the text that is still arriving is, in bytes: what a patch of it carries whole.
    pub fn live_len(&self) -> usize {
        self.live_texts
            .back()
            .and_then(|at| self.items.get(*at))
            .map_or(0, |i| match i {
                Item::Text { text, .. } => text.len(),
                _ => 0,
            })
    }

    pub fn busy(&self) -> bool {
        self.inflight.is_some() || self.auto
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        self.stamps.push((stamp(), true));
        self.trim();
    }

    /// Keep the last `KEEP` items; `first` counts the ones dropped.
    fn trim(&mut self) {
        if self.items.len() <= KEEP {
            return;
        }
        let drop = self.items.len() - KEEP;
        self.items.drain(..drop);
        self.stamps.drain(..drop);
        self.first += drop as u32;
        self.live_texts = self
            .live_texts
            .iter()
            .filter_map(|i| i.checked_sub(drop))
            .collect();
    }

    fn note(&mut self, text: impl Into<String>, tone: Option<Tone>) {
        self.push(Item::Note {
            text: text.into(),
            tone,
        });
    }

    // ---- what the person does ----

    /// A message from the person: sent now, or queued when the engine is busy.
    pub fn send(&mut self, text: &str, images: Vec<Image>, now: u64) -> Vec<Effect> {
        if self.busy() || self.pending.is_some() {
            self.queue.push_back(Queued::Text {
                text: text.to_owned(),
                images,
            });
            return Vec::new();
        }
        self.start_text(text, images, now)
    }

    /// `send` that goes first (REQ-966): ahead of what waited, a turn that runs is interrupted.
    pub fn send_first(&mut self, text: &str, images: Vec<Image>, now: u64) -> Vec<Effect> {
        let restored = self.pending.as_ref().is_some_and(|p| p.ask.restored);
        if !self.busy() && !restored {
            return self.send(text, images, now);
        }
        self.queue.push_front(Queued::Text {
            text: text.to_owned(),
            images,
        });
        // A local command is not a turn (REQ-967).
        let mut out = if matches!(self.inflight, Some(Inflight::Local(_))) {
            Vec::new()
        } else {
            self.interrupt()
        };
        // Nothing ends a turn that had no engine (REQ-968); while one runs, this is a no-op.
        out.extend(self.drain(now));
        out
    }

    fn start_text(&mut self, text: &str, images: Vec<Image>, now: u64) -> Vec<Effect> {
        if !self.named {
            self.name = clip(&text.split_whitespace().collect::<Vec<_>>().join(" "), 28);
            self.named = true;
        }
        let count = images.len() as u32;
        if std::mem::take(&mut self.resume_divider) {
            self.push(Item::Divider {
                text: "conversación retomada".into(),
                kind: DividerKind::Resume,
            });
        }
        self.push(Item::User {
            text: text.to_owned(),
            images: count,
        });
        let body = if self.shell_context.is_empty() {
            text.to_owned()
        } else {
            let ctx = std::mem::take(&mut self.shell_context).join("\n\n");
            format!("{ctx}\n\n{text}")
        };
        self.begin_turn(now);
        self.inflight = Some(Inflight::Turn);
        // Typed, queued or sent by an agent, it comes through here: the reload runs while it does.
        self.reloading = matches!(
            text.split_whitespace().next(),
            Some("/reload-plugins" | "/reload-skills")
        );
        vec![Effect::Write(user_line(&body, &images))]
    }

    fn begin_turn(&mut self, now: u64) {
        self.suggestion = None;
        self.status = ConvStatus::Thinking;
        // The turn of another `claude` that was followed is not this one's: what it ran is over.
        if self.followed.take().is_some() {
            self.settle_running();
        }
        self.since = Some(now);
        self.turn_started = now;
        self.turn_tokens.clear();
        self.tools_in_turn = 0;
        self.interrupting = false;
    }

    /// A line of the engine's own that changes what it runs with: sent when nothing is in flight.
    fn local(&mut self, line: String, change: Change) -> Vec<Effect> {
        if self.busy() || self.pending.is_some() {
            self.queue.push_back(Queued::Local { line, change });
            return Vec::new();
        }
        self.suggestion = None;
        self.inflight = Some(Inflight::Local(change));
        vec![Effect::Write(user_line(&line, &[]))]
    }

    /// The answer to the question that waits. `by` says where it came from.
    pub fn answer(
        &mut self,
        ask: &str,
        key: &str,
        text: Option<&str>,
        by: Option<&str>,
    ) -> Result<Vec<Effect>, String> {
        let Some(p) = self.pending.as_ref().filter(|p| p.ask.id == ask) else {
            return Err("that question is not waiting".into());
        };
        // The keys of a question are the numbers of its options, many at once for a choice of several.
        if !matches!(p.kind, Kind::Question(_)) && !p.ask.options.iter().any(|o| o.key == key) {
            return Err(format!("{key} is not one of its answers"));
        }
        let p = self.pending.take().expect("checked above");
        match p.kind {
            Kind::Question(_) => return self.answer_question(p, key, text, by),
            Kind::Plan => return Ok(self.answer_plan(&p, key, text, by)),
            Kind::Permission => {}
        }
        if p.ask.restored {
            return Ok(self.answer_restored_permission(p, key, text, by));
        }
        let mut out = vec![Effect::ClearAsk];
        let tool_id = p.tool_use_id.clone();
        let by = by.map(str::to_owned);
        match key {
            "no" => {
                let said = text.map(str::trim).filter(|t| !t.is_empty());
                let message = match said {
                    Some(t) => format!("El usuario rechazó esta acción y dice: {t}"),
                    None => "El usuario rechazó esta acción.".to_owned(),
                };
                out.push(Effect::Write(permission_reply(
                    &p.request, false, &p.input, &message,
                )));
                if let Some(id) = &tool_id {
                    self.refused.push(id.clone());
                    self.set_tool(id, |t| {
                        t.0 = ToolState::Denied;
                        t.1 = Some("Rechazado".into());
                        t.2 = by.clone();
                    });
                }
            }
            _ => {
                if key == "session"
                    && let Some(rule) = p.session_rule
                {
                    self.allowed.push(rule);
                }
                out.push(Effect::Write(permission_reply(
                    &p.request, true, &p.input, "",
                )));
                if let Some(id) = &tool_id {
                    self.set_tool(id, |t| {
                        t.0 = ToolState::Run;
                        t.2 = by.clone();
                    });
                }
            }
        }
        self.status = ConvStatus::Tool;
        Ok(out)
    }

    /// The answer to a permission that waited before a restart: what Claude is told, as a message (REQ-881).
    fn answer_restored_permission(
        &mut self,
        p: Pending,
        key: &str,
        text: Option<&str>,
        by: Option<&str>,
    ) -> Vec<Effect> {
        let what = format!("{} {}", p.ask.tool, tool_arg(&p.ask.tool, &p.input));
        let refused = key == "no";
        let line = if refused {
            match text.map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => format!("No, no ejecutes {what}: {t}"),
                None => format!("No, no ejecutes {what}"),
            }
        } else {
            if key == "session"
                && let Some(rule) = p.session_rule
            {
                self.allowed.push(rule);
            }
            self.allow_once = Some((p.ask.tool.clone(), p.input.clone()));
            format!("Sí, adelante con {what}")
        };
        if let Some(id) = &p.tool_use_id {
            let by = by.map(str::to_owned);
            if refused {
                self.refused.push(id.clone());
            }
            if refused {
                self.set_tool(id, |t| {
                    t.0 = ToolState::Denied;
                    t.1 = Some("Rechazado".into());
                    t.2 = by;
                });
            } else {
                // It did not run; Claude proposes it again as a new call, so this one is left without a result.
                self.mark_lost(id, "aprobada · Claude la vuelve a proponer");
            }
        }
        self.resuming();
        vec![Effect::ClearAsk, Effect::Resume(line)]
    }

    /// The answer to one of the questions Claude asks: the number of the option (several, with commas, for a
    /// choice of several), the person's own words in `text`, or both in a choice of several (REQ-750). The
    /// engine is answered when the last one is, with `answers` (REQ-435).
    fn answer_question(
        &mut self,
        mut p: Pending,
        key: &str,
        text: Option<&str>,
        by: Option<&str>,
    ) -> Result<Vec<Effect>, String> {
        let Kind::Question(mut qs) = p.kind.clone() else {
            return Err("that is not a question".into());
        };
        let q = &qs.all[qs.answered.len()];
        let own = text.map(str::trim).filter(|t| !t.is_empty());
        let picks: Vec<usize> = if key.trim().is_empty() && own.is_some() {
            Vec::new()
        } else {
            key.split(',')
                .map(|k| k.trim().parse().unwrap_or(usize::MAX))
                .collect()
        };
        // An option, or the own words; in a choice of one not both, in a choice of several the options with them.
        let single_with_both = !q.multi && own.is_some() && !picks.is_empty();
        if picks.iter().any(|i| *i >= q.options.len())
            || (!q.multi && own.is_none() && picks.len() != 1)
            || single_with_both
        {
            self.pending = Some(p);
            return Err(format!("{key} is not one of its answers"));
        }
        let labels: Vec<String> = picks.iter().map(|i| q.options[*i].0.clone()).collect();
        let joined = |own: Option<String>| {
            labels
                .iter()
                .cloned()
                .chain(own)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let told = joined(own.map(str::to_owned));
        let shown = joined(own.map(|t| format!("«{t}»")));
        qs.answered.push((q.question.clone(), told, shown));
        if qs.answered.len() < qs.all.len() {
            let restored = p.ask.restored;
            p.ask = question_ask(&p.request, qs.answered.len(), &qs.all[qs.answered.len()]);
            p.ask.restored = restored;
            p.kind = Kind::Question(qs);
            self.pending = Some(p);
            return Ok(Vec::new());
        }
        let mut input = p.input.clone();
        input["answers"] = Value::Object(
            qs.answered
                .iter()
                .map(|(q, told, _)| (q.clone(), json!(told)))
                .collect(),
        );
        let said = qs
            .answered
            .iter()
            .map(|(_, _, shown)| shown.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        if let Some(id) = &p.tool_use_id {
            let by = by.map(str::to_owned);
            // A question of before a restart gets no result on the stream (Claude settles it in its own file): it ends here.
            let state = if p.ask.restored {
                ToolState::Ok
            } else {
                ToolState::Run
            };
            self.set_tool(id, |t| {
                t.0 = state;
                t.1 = Some(clip(&format!("Respondida: {said}"), LINE_CHARS));
                t.2 = by;
            });
        }
        if p.ask.restored {
            let line = qs
                .answered
                .iter()
                .map(|(q, told, _)| format!("Elegí «{told}» para «{q}»"))
                .collect::<Vec<_>>()
                .join("\n");
            self.resuming();
            return Ok(vec![Effect::ClearAsk, Effect::Resume(line)]);
        }
        self.status = ConvStatus::Tool;
        Ok(vec![
            Effect::ClearAsk,
            Effect::Write(permission_reply(&p.request, true, &input, "")),
        ])
    }

    /// The approval of a plan: with edits allowed or back in `auto` (the engine is asked to change its mode at once, so the
    /// next edit is not asked), asking for each edit, or the plan refused with what to do instead (REQ-436).
    fn answer_plan(
        &mut self,
        p: &Pending,
        key: &str,
        text: Option<&str>,
        by: Option<&str>,
    ) -> Vec<Effect> {
        let mut out = vec![Effect::ClearAsk];
        let by = by.map(str::to_owned);
        if p.ask.restored {
            // No engine has the plan: the mode the answer means is the one the engine starts in.
            let said = text.map(str::trim).filter(|t| !t.is_empty());
            let (mode, line) = match key {
                "no" => (
                    Mode::Plan,
                    said.map_or("No apruebo el plan; seguí planeando.".to_owned(), |t| {
                        format!("No apruebo el plan; seguí planeando: {t}")
                    }),
                ),
                "accept" => (
                    Mode::AcceptEdits,
                    "Aprobado: seguí con el plan y editá sin preguntar.".to_owned(),
                ),
                "auto" => (Mode::Auto, "Aprobado: seguí con el plan.".to_owned()),
                _ => (Mode::Manual, "Aprobado: seguí con el plan.".to_owned()),
            };
            self.change_mode(mode);
            if let Some(id) = &p.tool_use_id {
                let refused = key == "no";
                if refused {
                    self.refused.push(id.clone());
                }
                self.set_tool(id, |t| {
                    t.0 = if refused {
                        ToolState::Denied
                    } else {
                        ToolState::Ok
                    };
                    t.1 = Some(
                        if refused {
                            "Plan rechazado"
                        } else {
                            "Plan aprobado"
                        }
                        .into(),
                    );
                    t.2 = by;
                });
            }
            // The engine may be running already (a message sent meanwhile started it): it is told the mode too.
            if key != "no" {
                out.push(self.mode_request(mode));
            }
            self.resuming();
            out.push(Effect::Resume(line));
            return out;
        }
        if key == "no" {
            let message = match text.map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => format!("El usuario no aprobó el plan y dice: {t}"),
                None => "El usuario no aprobó el plan: sigue planeando.".to_owned(),
            };
            out.push(Effect::Write(permission_reply(
                &p.request, false, &p.input, &message,
            )));
            if let Some(id) = &p.tool_use_id {
                self.refused.push(id.clone());
                self.set_tool(id, |t| {
                    t.0 = ToolState::Denied;
                    t.1 = Some("Plan rechazado".into());
                    t.2 = by;
                });
            }
        } else {
            out.push(Effect::Write(permission_reply(
                &p.request, true, &p.input, "",
            )));
            match key {
                "accept" => out.push(self.mode_request(Mode::AcceptEdits)),
                // Back to the mode the plan was entered from (REQ-605).
                "auto" => out.push(self.mode_request(Mode::Auto)),
                _ => {}
            }
            if let Some(id) = &p.tool_use_id {
                self.set_tool(id, |t| {
                    t.0 = ToolState::Run;
                    t.1 = Some("Plan aprobado".into());
                    t.2 = by;
                });
            }
        }
        self.status = ConvStatus::Tool;
        out
    }

    /// Stop a task the engine runs in the background (REQ-432); the engine says when it ended.
    pub fn stop_task(&mut self, task: &str) -> Result<Vec<Effect>, String> {
        if !self
            .tasks
            .iter()
            .any(|t| t.id == task && t.state == TaskState::Run)
        {
            return Err("that task is not running".into());
        }
        let id = self.next_request;
        self.next_request += 1;
        Ok(vec![Effect::Write(json!({
            "type": "control_request",
            "request_id": format!("jwn-{id}"),
            "request": {"subtype": "stop_task", "task_id": task},
        }))])
    }

    /// Stop what runs: a waiting question is refused first, so the engine is not left asking.
    pub fn interrupt(&mut self) -> Vec<Effect> {
        // No engine has an ask of before a restart: dropping it is all there is to interrupt.
        if self.pending.as_ref().is_some_and(|p| p.ask.restored) {
            let p = self.pending.take().expect("checked above");
            if let Some(id) = p.tool_use_id {
                self.set_tool(&id, |t| {
                    t.0 = ToolState::Denied;
                    t.1 = Some("Interrumpido".into());
                });
            }
            self.status = ConvStatus::Dead;
            return vec![Effect::ClearAsk];
        }
        if !self.busy() || self.interrupting {
            return Vec::new();
        }
        self.interrupting = true;
        let mut out = Vec::new();
        if let Some(p) = self.pending.take() {
            out.push(Effect::ClearAsk);
            out.push(Effect::Write(permission_reply(
                &p.request,
                false,
                &p.input,
                "El usuario interrumpió.",
            )));
            if let Some(id) = p.tool_use_id.clone() {
                self.set_tool(&id, |t| {
                    t.0 = ToolState::Denied;
                    t.1 = Some("Interrumpido".into());
                });
            }
        }
        let id = self.next_request;
        self.next_request += 1;
        out.push(Effect::Write(json!({
            "type": "control_request",
            "request_id": format!("jwn-{id}"),
            "request": {"subtype": "interrupt"},
        })));
        out
    }

    /// Change what the conversation is called, its colour, the model, the effort or the mode.
    pub fn set(
        &mut self,
        name: Option<String>,
        accent: Option<Accent>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<Mode>,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        if let Some(n) = name {
            // One line: a name with a line break would be two lines to the engine.
            let n = clip(&n.split_whitespace().collect::<Vec<_>>().join(" "), 80);
            if !n.is_empty() {
                self.name = n.clone();
                self.named = true;
                out.extend(self.local(format!("/rename {n}"), Change::None));
            }
        }
        if let Some(a) = accent {
            self.accent = a;
        }
        if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
            out.extend(self.local(
                format!("/model {}", m.trim()),
                Change::Model(m.trim().to_owned()),
            ));
        }
        if let Some(e) = effort.filter(|e| !e.trim().is_empty()) {
            out.extend(self.local(
                format!("/effort {}", e.trim()),
                Change::Effort(e.trim().to_owned()),
            ));
        }
        // The engine takes the change at once, also in the middle of a turn (REQ-389). It is asked
        // every time: the mode shown lags until it answers, so two changes before its reply would
        // otherwise look like one and the second (back to the shown mode) would never reach it.
        if let Some(m) = mode {
            out.push(self.mode_request(m));
        }
        out
    }

    /// Ask the engine to change its mode while it runs; the mode is shown once it says it did.
    fn mode_request(&mut self, m: Mode) -> Effect {
        let id = self.next_request;
        self.next_request += 1;
        Effect::Write(json!({
            "type": "control_request",
            "request_id": format!("jwn-{id}"),
            "request": {"subtype": "set_permission_mode", "mode": m.flag()},
        }))
    }

    /// A `!command` ran: what it printed goes to the engine with the next message.
    pub fn shell_done(&mut self, command: &str, ok: bool, output: Vec<String>) {
        let at = self.items.iter().rposition(
            |i| matches!(i, Item::Shell { command: c, state: ToolState::Run, .. } if c == command),
        );
        if let Some(Item::Shell {
            state, output: out, ..
        }) = at.and_then(|i| self.items.get_mut(i))
        {
            *state = if ok { ToolState::Ok } else { ToolState::Failed };
            out.clone_from(&output);
        }
        let body = if output.is_empty() {
            format!("! {command}")
        } else {
            format!("! {command}\n{}", output.join("\n"))
        };
        self.shell_context.push(body);
    }

    pub fn shell_started(&mut self, command: &str) {
        self.push(Item::Shell {
            command: command.to_owned(),
            state: ToolState::Run,
            output: Vec::new(),
        });
    }

    /// The plan that waits as `ask`: its text, whole, and the file the engine says it is in.
    pub fn waiting_plan(&self, ask: &str) -> Option<(String, Option<String>)> {
        let p = self
            .pending
            .as_ref()
            .filter(|p| p.ask.id == ask && p.ask.kind == AskKind::Plan)?;
        Some((
            p.input["plan"].as_str()?.to_owned(),
            p.input["planFilePath"].as_str().map(str::to_owned),
        ))
    }

    /// `/cost`: the conversation's own cost, not the subscription's use (REQ-419).
    pub fn answer_cost(&mut self) {
        let text = format!(
            "{} en esta conversación · {}",
            dollars(self.cost),
            self.model
        );
        self.push(Item::Out { lines: vec![text] });
    }

    // ---- what the engine says ----

    /// One message of the engine.
    pub fn apply(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        let nested = !msg["parent_tool_use_id"].is_null();
        match msg["type"].as_str().unwrap_or("") {
            "system" => self.system(msg, now),
            "stream_event" if !nested => self.stream(&msg["event"], now),
            "assistant" if !nested => self.assistant(msg, now),
            "assistant" => {
                self.nested(msg);
                Vec::new()
            }
            "user" if !nested => self.user(msg),
            "control_request" => self.control_request(msg),
            "control_response" => {
                let r = &msg["response"];
                if r["request_id"] == INIT_REQUEST {
                    if r["subtype"] == "success" {
                        self.initialized(&r["response"]);
                    }
                    return Vec::new();
                }
                if r["subtype"] == "error" {
                    let why = r["error"].as_str().unwrap_or("el motor rechazó el cambio");
                    self.note(clip(why, LINE_CHARS), Some(Tone::Error));
                }
                Vec::new()
            }
            "result" => self.result(msg, now),
            // After a turn, the engine proposes what the person writes next (REQ-572).
            "prompt_suggestion" => {
                // One that comes while another turn runs or a message waits is for a turn that is
                // over: it would show, stale, after this one if it brings none.
                if self.busy() || self.status != ConvStatus::Idle || !self.queue.is_empty() {
                    return Vec::new();
                }
                self.suggestion = msg["suggestion"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| clip(t, SUGGESTION_CHARS));
                Vec::new()
            }
            "conversation_reset" => self.reset(msg),
            _ => Vec::new(),
        }
    }

    fn begin_auto(&mut self, now: u64) {
        if self.inflight.is_none() && !self.auto {
            self.auto = true;
            self.begin_turn(now);
        }
    }

    /// The conversation is in `mode` now, whoever said it first: the engine's own message or the
    /// line it wrote in its transcript (which a follow can read a moment before the message
    /// arrives). Into plan, the mode left is the one a plan goes back to (REQ-605); any other change
    /// forgets it. The footer shows the mode; a line for each change was noise.
    fn change_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.before_plan = (mode == Mode::Plan).then_some(self.mode);
        self.mode = mode;
    }

    fn system(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        match msg["subtype"].as_str().unwrap_or("") {
            "init" => {
                if let Some(m) = msg["model"].as_str() {
                    self.model = short_model(m);
                }
                if let Some(m) = msg["permissionMode"].as_str().and_then(Mode::from_flag) {
                    // A new engine that starts in plan is the one the conversation was in when it
                    // stopped: where it came from is still what a plan goes back to.
                    if m != Mode::Plan {
                        self.before_plan = None;
                    }
                    self.mode = m;
                }
                if msg["skills"].is_array() {
                    self.learn_skills(&msg["skills"]);
                }
                if let Some(v) = msg["claude_code_version"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                {
                    self.engine_version = Some(v.to_owned());
                }
                self.begin_auto(now);
            }
            "compact_boundary" => {
                let meta = &msg["compact_metadata"];
                let (pre, post) = (meta["pre_tokens"].as_u64(), meta["post_tokens"].as_u64());
                let text = match (pre, post) {
                    (Some(a), Some(b)) => {
                        format!("conversación compactada · {} → {} tokens", kilo(a), kilo(b))
                    }
                    _ => "conversación compactada".to_owned(),
                };
                self.push(Item::Divider {
                    text,
                    kind: DividerKind::Compact,
                });
            }
            "status" => {
                if let Some(m) = msg["permissionMode"].as_str().and_then(Mode::from_flag) {
                    self.change_mode(m);
                }
            }
            kind @ ("task_started" | "task_progress" | "task_updated" | "task_notification") => {
                self.task_event(kind, msg, now);
            }
            // What the engine says runs in the background, whole: nothing in the list is something that ended.
            "background_tasks_changed" => {
                let running: Vec<&str> = msg["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["task_id"].as_str())
                    .collect();
                let gone: Vec<String> = self
                    .tasks
                    .iter()
                    .filter(|t| t.state == TaskState::Run && !running.contains(&t.id.as_str()))
                    .filter(|t| {
                        self.task_links
                            .iter()
                            .any(|l| l.task == t.id && l.background)
                    })
                    .map(|t| t.id.clone())
                    .collect();
                for id in gone {
                    self.end_task(&id, "completed", None, true);
                }
            }
            "permission_denied" => {
                if let Some(id) = msg["tool_use_id"].as_str() {
                    let why = msg["message"].as_str().unwrap_or("Sin permiso").to_owned();
                    self.set_tool(id, |t| {
                        t.0 = ToolState::Denied;
                        t.1 = Some(clip(&why, LINE_CHARS));
                    });
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn stream(&mut self, ev: &Value, now: u64) -> Vec<Effect> {
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.begin_auto(now);
                // A message with no id is counted by nothing, and not by the one before it.
                self.msg_id = ev["message"]["id"].as_str().unwrap_or_default().to_owned();
                self.count_tokens(self.msg_id.clone(), &ev["message"]["usage"]);
            }
            "message_delta" => {
                let id = self.msg_id.clone();
                self.count_tokens(id, &ev["usage"]);
            }
            "content_block_start" => {
                if ev["content_block"]["type"] == "text"
                    && !matches!(self.inflight, Some(Inflight::Local(_)))
                {
                    self.status = ConvStatus::Streaming;
                    self.push(Item::Text {
                        text: String::new(),
                        live: true,
                        blocks: None,
                    });
                    self.live_texts.push_back(self.items.len() - 1);
                }
            }
            "content_block_delta" => {
                if let Some(t) = ev["delta"]["text"]
                    .as_str()
                    .filter(|_| ev["delta"]["type"] == "text_delta")
                {
                    if self.live_texts.is_empty() {
                        self.push(Item::Text {
                            text: String::new(),
                            live: true,
                            blocks: None,
                        });
                        self.live_texts.push_back(self.items.len() - 1);
                    }
                    self.status = ConvStatus::Streaming;
                    let at = *self.live_texts.back().expect("one was just made");
                    if let Some(Item::Text { text, .. }) = self.items.get_mut(at) {
                        text.push_str(t);
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Take the tokens a message says it has written: the most it ever says.
    fn count_tokens(&mut self, id: String, usage: &Value) {
        if id.is_empty() {
            return;
        }
        if let Some(n) = usage["output_tokens"].as_u64() {
            let at = self.turn_tokens.entry(id).or_insert(0);
            *at = (*at).max(n);
        }
    }

    fn assistant(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.begin_auto(now);
        if let Some(id) = msg["message"]["id"].as_str() {
            self.count_tokens(id.to_owned(), &msg["message"]["usage"]);
        }
        let blocks = msg["message"]["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for b in &blocks {
            match b["type"].as_str().unwrap_or("") {
                "text" => {
                    let full = b["text"].as_str().unwrap_or("").to_owned();
                    if full.trim().is_empty() {
                        continue;
                    }
                    // What a local command (`/effort`, `/model`) answers is its output, not Claude's.
                    if matches!(self.inflight, Some(Inflight::Local(_))) {
                        self.push(Item::Out {
                            lines: full.lines().map(|l| clip(l, LINE_CHARS)).collect(),
                        });
                        continue;
                    }
                    match self.live_texts.pop_front() {
                        Some(at) => {
                            if let Some(Item::Text { text, live, blocks }) = self.items.get_mut(at)
                            {
                                *blocks = crate::markdown::conversation_blocks(&full);
                                *text = full;
                                *live = false;
                            }
                        }
                        None => self.push(Item::Text {
                            blocks: crate::markdown::conversation_blocks(&full),
                            text: full,
                            live: false,
                        }),
                    }
                }
                "tool_use" => {
                    let name = b["name"].as_str().unwrap_or("").to_owned();
                    let id = b["id"].as_str().unwrap_or("").to_owned();
                    match name.as_str() {
                        "ToolSearch" => continue,
                        // The checklist is the node's, not a line of the conversation (REQ-434).
                        "TodoWrite" => {
                            self.todos = parse_todos(&b["input"]);
                            continue;
                        }
                        // Claude's own notice to the person: a note, and the pane asks for attention (REQ-438).
                        "PushNotification" => {
                            if let Some(m) = b["input"]["message"].as_str() {
                                self.note(clip(m, LINE_CHARS), None);
                            }
                            fx.push(Effect::Raise(AttentionKind::AgentDone));
                            continue;
                        }
                        "Agent" | "Task" => {
                            self.tools_in_turn += 1;
                            self.status = ConvStatus::Tool;
                            self.push(Item::Agent {
                                id,
                                name: b["input"]["subagent_type"]
                                    .as_str()
                                    .unwrap_or("agent")
                                    .to_owned(),
                                description: clip(
                                    b["input"]["description"].as_str().unwrap_or(""),
                                    LINE_CHARS,
                                ),
                                state: ToolState::Run,
                                tools: 0,
                                tokens: 0,
                                ms: 0,
                                last: None,
                                steps: Vec::new(),
                                prompt: b["input"]["prompt"]
                                    .as_str()
                                    .filter(|p| !p.trim().is_empty())
                                    .map(|p| clip(p, PROMPT_CHARS)),
                                model: b["input"]["model"]
                                    .as_str()
                                    .filter(|m| !m.trim().is_empty())
                                    .map(short_model),
                            });
                            continue;
                        }
                        _ => {}
                    }
                    self.tools_in_turn += 1;
                    self.status = ConvStatus::Tool;
                    self.push(Item::Tool {
                        id,
                        arg: tool_arg(&name, &b["input"]),
                        diff: edit_diff(&name, &b["input"]),
                        name,
                        state: ToolState::Run,
                        summary: None,
                        output: Vec::new(),
                        url: None,
                        by: None,
                        lost: false,
                    });
                }
                // The advisor is consulted by the API itself: the call and its answer are blocks
                // of Claude's own message (REQ-570).
                "server_tool_use" if b["name"] == "advisor" => {
                    self.status = ConvStatus::Tool;
                    self.push(advisor_row(b["id"].as_str().unwrap_or("")));
                }
                "advisor_tool_result" => {
                    let (state, says) = advisor_answer(b);
                    self.set_tool(b["tool_use_id"].as_str().unwrap_or(""), |t| {
                        t.0 = state;
                        t.1 = Some(says);
                    });
                }
                _ => {}
            }
        }
        // Whatever stream block was left open is finished by now.
        for at in self.live_texts.drain(..) {
            if let Some(Item::Text { live, .. }) = self.items.get_mut(at) {
                *live = false;
            }
        }
        fx
    }

    /// What a subagent does with its own tools: the steps of its card.
    fn nested(&mut self, msg: &Value) {
        let Some(parent) = msg["parent_tool_use_id"].as_str() else {
            return;
        };
        let steps: Vec<String> = msg["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| {
                let name = b["name"].as_str().unwrap_or("");
                clip(
                    format!("{name} {}", tool_arg(name, &b["input"])).trim(),
                    120,
                )
            })
            .collect();
        // What Claude Code writes itself (an API error, «no response requested») is not what ran.
        let ran_with = msg["message"]["model"]
            .as_str()
            .filter(|m| *m != "<synthetic>")
            .map(short_model);
        if let Some(Item::Agent {
            steps: have, model, ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Agent { id, .. } if id == parent))
        {
            have.extend(steps);
            have.truncate(STEPS_KEPT);
            // The model its own messages say it runs with is the one it does run with.
            if ran_with.is_some() {
                *model = ran_with;
            }
        }
    }

    /// The agent's tool call came back: it ended, unless it runs in the background and the engine
    /// has still to say so. True when the call was an agent's.
    fn finish_agent(&mut self, id: &str, failed: bool, result: &Value) -> bool {
        let background = self
            .task_links
            .iter()
            .any(|l| l.background && l.tool_use_id.as_deref() == Some(id));
        let Some(Item::Agent {
            state,
            tools,
            tokens,
            ms,
            ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Agent { id: a, .. } if a == id))
        else {
            return false;
        };
        if background {
            return true;
        }
        *state = if failed {
            ToolState::Failed
        } else {
            ToolState::Ok
        };
        if let Some(n) = result["totalToolUseCount"].as_u64() {
            *tools = n as u32;
        }
        if let Some(n) = result["totalTokens"].as_u64() {
            *tokens = n;
        }
        if let Some(n) = result["totalDurationMs"].as_u64() {
            *ms = n;
        }
        true
    }

    /// A task of the engine started, moved or ended (REQ-428, REQ-429, REQ-431).
    fn task_event(&mut self, kind: &str, msg: &Value, now: u64) {
        let Some(task) = msg["task_id"].as_str() else {
            return;
        };
        let tool_use_id = msg["tool_use_id"].as_str().map(str::to_owned);
        match kind {
            "task_started" => {
                let label = clip(msg["description"].as_str().unwrap_or("tarea"), LINE_CHARS);
                let mut kind = match msg["task_type"].as_str().unwrap_or("") {
                    "local_agent" => TaskKind::Agent,
                    "local_bash" => TaskKind::Bash,
                    t if t.contains("monitor") => TaskKind::Monitor,
                    _ => TaskKind::Other,
                };
                // The engine calls a monitor `local_bash` as well: what tells them apart is the tool.
                if kind == TaskKind::Bash
                    && self.tool_named(tool_use_id.as_deref()) == Some("Monitor")
                {
                    kind = TaskKind::Monitor;
                }
                self.tasks.retain(|t| t.id != task);
                self.task_links.retain(|l| l.task != task);
                self.tasks.push(Task {
                    id: task.to_owned(),
                    kind,
                    label,
                    state: TaskState::Run,
                    started: now,
                    info: None,
                    item: tool_use_id.clone(),
                    tail: Vec::new(),
                });
                if self.tasks.len() > TASKS_KEPT {
                    let gone = self.tasks.remove(0);
                    self.task_links.retain(|l| l.task != gone.id);
                }
                self.task_links.push(TaskLink {
                    task: task.to_owned(),
                    tool_use_id,
                    background: msg["is_backgrounded"].as_bool().unwrap_or(false),
                    noted: false,
                    final_read: false,
                });
            }
            "task_progress" => {
                if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task) {
                    t.info = msg["description"].as_str().map(|d| clip(d, LINE_CHARS));
                }
                let tool = tool_use_id.or_else(|| self.link_of(task));
                self.card_usage(tool, &msg["usage"], msg["last_tool_name"].as_str());
            }
            "task_updated" => {
                let status = msg["patch"]["status"].as_str().unwrap_or("");
                if !status.is_empty() {
                    self.end_task(task, status, None, false);
                }
            }
            "task_notification" => {
                if let Some(file) = msg["output_file"].as_str().filter(|f| !f.is_empty()) {
                    self.known_output = Some(file.into());
                }
                // The totals at the end: the last progress was before its last tool finished.
                let tool = tool_use_id.or_else(|| self.link_of(task));
                self.card_usage(tool, &msg["usage"], None);
                let status = msg["status"].as_str().unwrap_or("");
                self.end_task(task, status, msg["summary"].as_str(), true);
            }
            _ => {}
        }
    }

    /// The numbers of a subagent's card, as the engine counts them (its tools, its tokens and its time).
    fn card_usage(&mut self, tool: Option<String>, usage: &Value, last: Option<&str>) {
        if usage.is_null() {
            return;
        }
        if let Some(Item::Agent {
            tools,
            tokens,
            ms,
            last: last_tool,
            ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Agent { id, .. } if Some(id) == tool.as_ref()))
        {
            // A figure the engine leaves out keeps the one it said before, not a zero.
            if let Some(n) = usage["tool_uses"].as_u64() {
                *tools = n as u32;
            }
            if let Some(n) = usage["total_tokens"].as_u64() {
                *tokens = n;
            }
            if let Some(n) = usage["duration_ms"].as_u64() {
                *ms = n;
            }
            if let Some(l) = last {
                *last_tool = Some(l.to_owned());
            }
        }
    }

    /// The name of the tool whose call has the id `id`.
    fn tool_named(&self, id: Option<&str>) -> Option<&str> {
        let id = id?;
        self.items.iter().rev().find_map(|i| match i {
            Item::Tool { id: t, name, .. } if t == id => Some(name.as_str()),
            _ => None,
        })
    }

    /// The tasks whose output is to be looked at: the shells and monitors that run and the ones
    /// that ended and were not read since (REQ-607). Each with where its file is, when it is known.
    pub fn tail_targets(&self) -> Vec<TailTarget> {
        self.tasks
            .iter()
            .filter(|t| matches!(t.kind, TaskKind::Bash | TaskKind::Monitor))
            .filter_map(|t| {
                let link = self.task_links.iter().find(|l| l.task == t.id)?;
                (t.state == TaskState::Run || !link.final_read).then(|| TailTarget {
                    task: t.id.clone(),
                    ended: t.state != TaskState::Run,
                    path: self
                        .known_output
                        .as_deref()
                        .and_then(|k| crate::tail::sibling(k, &self.session, &t.id)),
                })
            })
            .collect()
    }

    /// What was read from the file of `task`: `None` is a file that is not there. `ended` is whether
    /// the task was over when it was read; a read of one that ends meanwhile is made again.
    /// Whether the lines changed.
    pub fn set_tail(&mut self, task: &str, lines: Option<Vec<String>>, ended: bool) -> bool {
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == task) else {
            return false;
        };
        let over = t.state != TaskState::Run;
        let changed = lines.as_ref().is_some_and(|l| t.tail != *l);
        if let Some(l) = lines {
            t.tail = l;
        }
        if over
            && ended
            && let Some(link) = self.task_links.iter_mut().find(|l| l.task == task)
        {
            link.final_read = true;
        }
        changed
    }

    fn link_of(&self, task: &str) -> Option<String> {
        self.task_links
            .iter()
            .find(|l| l.task == task)?
            .tool_use_id
            .clone()
    }

    /// The engine says a task ended: `completed`, `stopped` (or `killed`) or a failure. What ran in the
    /// background leaves a note, told once, when the engine's notification comes (it carries the summary);
    /// a subagent that ran in front already has its card.
    fn end_task(&mut self, task: &str, status: &str, summary: Option<&str>, notifies: bool) {
        let state = match status {
            "completed" => TaskState::Done,
            "stopped" | "killed" => TaskState::Stopped,
            "failed" | "error" => TaskState::Failed,
            _ => return,
        };
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == task) else {
            return;
        };
        if t.state == TaskState::Run {
            t.state = state;
        }
        // What it said at the end replaces what it was doing: the clients show it as what it said (REQ-600).
        if let Some(said) = summary
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
        {
            t.info = Some(clip(&said, SAID_CHARS));
        }
        let (state, label) = (t.state, t.label.clone());
        if let Some(tool) = self.link_of(task)
            && let Some(Item::Agent { state: card, .. }) = self
                .items
                .iter_mut()
                .rev()
                .find(|i| matches!(i, Item::Agent { id, .. } if *id == tool))
            && *card == ToolState::Run
        {
            *card = if state == TaskState::Done {
                ToolState::Ok
            } else {
                ToolState::Failed
            };
        }
        let Some(link) = self.task_links.iter_mut().find(|l| l.task == task) else {
            return;
        };
        if !notifies || !link.background || link.noted {
            return;
        }
        link.noted = true;
        let word = match state {
            TaskState::Done => "terminó",
            TaskState::Stopped => "se detuvo",
            _ => "falló",
        };
        let what = summary
            .and_then(|s| s.lines().next())
            .filter(|s| !s.is_empty() && *s != label)
            .map_or(String::new(), |s| format!(" · {s}"));
        self.note(
            clip(&format!("Tarea {word}: {label}{what}"), LINE_CHARS),
            (state == TaskState::Failed).then_some(Tone::Error),
        );
    }

    fn user(&mut self, msg: &Value) -> Vec<Effect> {
        let Some(blocks) = msg["message"]["content"].as_array() else {
            return Vec::new();
        };
        for b in blocks.clone() {
            if b["type"] != "tool_result" {
                continue;
            }
            let Some(id) = b["tool_use_id"].as_str() else {
                continue;
            };
            let text = content_text(&b["content"]);
            if let Some(file) = crate::tail::named_in(&text) {
                self.known_output = Some(file);
            }
            let failed = b["is_error"].as_bool().unwrap_or(false);
            if self.finish_agent(id, failed, &msg["tool_use_result"]) {
                continue;
            }
            let refused = self.refused.iter().any(|r| r == id);
            let lines: Vec<String> = text
                .lines()
                .take(OUTPUT_LINES)
                .map(|l| clip(l, LINE_CHARS))
                .collect();
            let result = msg["tool_use_result"].clone();
            self.set_tool(id, |t| {
                t.0 = if refused {
                    ToolState::Denied
                } else if failed {
                    ToolState::Failed
                } else {
                    ToolState::Ok
                };
                if t.1.is_none() {
                    t.1 = Some(summary_of(&t.4, &lines, &result, failed));
                }
                t.3 = lines.clone();
            });
        }
        Vec::new()
    }

    fn control_request(&mut self, msg: &Value) -> Vec<Effect> {
        let req = &msg["request"];
        if req["subtype"] != "can_use_tool" {
            return Vec::new();
        }
        let id = msg["request_id"].as_str().unwrap_or("").to_owned();
        let tool = req["tool_name"].as_str().unwrap_or("").to_owned();
        let input = req["input"].clone();
        let tool_use_id = req["tool_use_id"].as_str().map(str::to_owned);
        let nested = !req["agent_id"].is_null();
        // What Claude asks the person (a choice, a plan) is not a permission: it has its own box (REQ-435, REQ-436).
        let questions = parse_questions(&input);
        if tool == "AskUserQuestion" && !questions.is_empty() {
            let ask = question_ask(&id, 0, &questions[0]);
            return self.wait(
                Pending {
                    ask,
                    request: id,
                    kind: Kind::Question(Box::new(Questions {
                        all: questions,
                        answered: Vec::new(),
                    })),
                    input,
                    tool_use_id,
                    session_rule: None,
                    nested,
                },
                AttentionKind::Question,
            );
        }
        if tool == "ExitPlanMode" {
            let ask = plan_ask(&id, &input, self.before_plan);
            return self.wait(
                Pending {
                    ask,
                    request: id,
                    kind: Kind::Plan,
                    input,
                    tool_use_id,
                    session_rule: None,
                    nested,
                },
                AttentionKind::Plan,
            );
        }
        // The person said yes to this before a restart: Claude proposes the same call again (REQ-882).
        if self
            .allow_once
            .as_ref()
            .is_some_and(|(t, i)| *t == tool && *i == input)
        {
            self.allow_once = None;
            if let Some(tid) = req["tool_use_id"].as_str() {
                self.set_tool(tid, |t| t.2 = Some("aprobada antes de reiniciar".into()));
            }
            return vec![Effect::Write(permission_reply(&id, true, &input, ""))];
        }
        let rule = rule_key(&tool, &input);
        // «Allow for this session» said yes to this once already.
        if self.allowed.contains(&rule) {
            if let Some(tid) = req["tool_use_id"].as_str() {
                self.set_tool(tid, |t| t.2 = Some("regla de esta sesión".into()));
            }
            return vec![Effect::Write(permission_reply(&id, true, &input, ""))];
        }
        let suggested = req["permission_suggestions"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let (title, command) = ask_title(&tool, &input);
        let mut options = vec![AskOption {
            key: "yes".into(),
            label: "Sí".into(),
        }];
        if suggested {
            options.push(AskOption {
                key: "session".into(),
                label: session_label(&tool, &input),
            });
        }
        options.push(AskOption {
            key: "no".into(),
            label: "No, y decirle a Claude qué hacer distinto".into(),
        });
        let ask = Ask {
            id: id.clone(),
            kind: AskKind::Permission,
            tool: tool.clone(),
            title,
            command,
            diff: edit_diff(&tool, &input),
            plan: Vec::new(),
            options,
            multi: false,
            restored: false,
        };
        self.wait(
            Pending {
                ask,
                request: id,
                kind: Kind::Permission,
                input,
                tool_use_id,
                session_rule: suggested.then_some(rule),
                nested,
            },
            AttentionKind::Permission,
        )
    }

    /// Something waits for the person: the tool says it is asking, the node shows the box and the pane asks for attention.
    fn wait(&mut self, p: Pending, attention: AttentionKind) -> Vec<Effect> {
        if let Some(tid) = &p.tool_use_id {
            self.set_tool(tid, |t| t.0 = ToolState::Ask);
        }
        self.pending = Some(p);
        self.status = ConvStatus::Asking;
        vec![Effect::Raise(attention)]
    }

    fn result(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        let mut out = Vec::new();
        self.allow_once = None;
        if let Some(s) = msg["session_id"].as_str().filter(|s| !s.is_empty())
            && s != self.session
        {
            self.session = s.to_owned();
        }
        let total = (msg["total_cost_usd"].as_f64().unwrap_or(0.0) * 1_000_000.0).round() as u64;
        let turn_cost = total.saturating_sub(self.last_total_micro);
        self.last_total_micro = total.max(self.last_total_micro);
        let inflight = self.inflight.take();
        if let Some(Inflight::Local(change)) = inflight {
            if !msg["is_error"].as_bool().unwrap_or(false) {
                match change {
                    Change::Model(m) => self.model = short_model(&m),
                    Change::Effort(e) => self.effort = e,
                    Change::None => {}
                }
            }
            self.after_end(&mut out, now);
            return out;
        }
        // The reload is done, so the list the engine gives now is the new one; the request goes
        // before the message that waits (REQ-756).
        if inflight.is_some() && std::mem::take(&mut self.reloading) {
            out.push(initialize());
        }
        self.auto = false;
        self.cost += turn_cost;
        self.finish_live();
        let aborted = msg["terminal_reason"] == "aborted_streaming";
        let ms = msg["duration_ms"]
            .as_u64()
            .unwrap_or_else(|| now.saturating_sub(self.turn_started));
        if aborted || self.interrupting {
            self.note("Interrumpido por ti", Some(Tone::Warn));
        } else if msg["is_error"].as_bool().unwrap_or(false) {
            let why = msg["result"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("el motor terminó con un error");
            self.note(clip(why, LINE_CHARS), Some(Tone::Error));
        }
        // A tool that never answered was cut with the turn, and so was a subagent that ran in front.
        for id in self.running_tools() {
            self.set_tool(&id, |t| {
                t.0 = ToolState::Denied;
                t.1 = Some("Interrumpido".into());
            });
        }
        self.cut_agents(false);
        self.push(Item::Result {
            ms,
            tools: self.tools_in_turn,
            cost: turn_cost,
        });
        self.interrupting = false;
        if !self.has_transcript {
            self.has_transcript = true;
            out.push(Effect::Resumable(self.session.clone()));
        }
        out.push(Effect::Raise(AttentionKind::AgentDone));
        self.after_end(&mut out, now);
        out
    }

    /// A turn or a local command ended: settle, then send what waited.
    fn after_end(&mut self, out: &mut Vec<Effect>, now: u64) {
        self.status = ConvStatus::Idle;
        self.since = None;
        out.extend(self.drain(now));
    }

    /// Send the next message that waited, if the engine is free for it.
    pub fn drain(&mut self, now: u64) -> Vec<Effect> {
        if self.busy() || self.pending.is_some() {
            return Vec::new();
        }
        match self.queue.pop_front() {
            Some(Queued::Text { text, images }) => self.start_text(&text, images, now),
            Some(Queued::Local { line, change }) => self.local(line, change),
            None => Vec::new(),
        }
    }

    fn reset(&mut self, msg: &Value) -> Vec<Effect> {
        if let Some(s) = msg["new_conversation_id"].as_str() {
            self.session = s.to_owned();
        }
        self.items.clear();
        self.stamps.clear();
        self.first = 0;
        self.live_texts.clear();
        self.has_transcript = false;
        self.resume_divider = false;
        self.suggestion = None;
        self.push(Item::Divider {
            text: "conversación vacía".into(),
            kind: DividerKind::Clear,
        });
        Vec::new()
    }

    fn finish_live(&mut self) {
        for at in self.live_texts.drain(..) {
            if let Some(Item::Text { live, .. }) = self.items.get_mut(at) {
                *live = false;
            }
        }
    }

    /// What the look for the session's transcript found as the engine is about to start (REQ-755):
    /// the flag becomes what the disk says; a transcript that came to be is the pane's resume, and
    /// one the flag said was there and is not is said in a note (RAT-108).
    pub fn transcript_found(&mut self, found: bool) -> Vec<Effect> {
        let had = std::mem::replace(&mut self.has_transcript, found);
        match (had, found) {
            (false, true) => vec![Effect::Resumable(self.session.clone())],
            (true, false) => {
                self.note(
                    "no se encontró el transcript de esta conversación: empieza de nuevo con el mismo id, sin lo anterior",
                    Some(Tone::Warn),
                );
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The engine's process ended while the conversation was open.
    pub fn engine_gone(&mut self, reason: &str) -> Vec<Effect> {
        self.engine_version = None;
        // What it asked is not lost with it: it waits again as after a restart, and its answer starts another engine (REQ-886).
        let had_ask = self.pending.as_mut().is_some_and(|p| {
            p.ask.restored = true;
            true
        });
        self.suggestion = None;
        self.inflight = None;
        self.auto = false;
        self.interrupting = false;
        self.finish_live();
        // The tool of the ask that stays is still the one that asks.
        let asking = self.pending.as_ref().and_then(|p| p.tool_use_id.clone());
        for id in self.running_tools() {
            if asking.as_deref() == Some(id.as_str()) {
                continue;
            }
            self.set_tool(&id, |t| {
                t.0 = ToolState::Failed;
                t.1 = Some("El motor se detuvo".into());
            });
        }
        self.cut_agents(true);
        self.task_links.clear();
        self.status = if had_ask {
            ConvStatus::Asking
        } else {
            ConvStatus::Dead
        };
        self.since = None;
        self.note(reason.to_owned(), Some(Tone::Error));
        Vec::new()
    }

    /// Whether an ask waits for the person: the engine that has it is ended at once when the daemon stops (REQ-885).
    pub fn has_ask(&self) -> bool {
        self.pending.is_some()
    }

    /// The engine is being started for a message: show it.
    pub fn resuming(&mut self) {
        self.status = ConvStatus::Resuming;
    }

    /// A new engine process starts counting its cost from nothing. It is asked what it can do
    /// and which models it has, which is what the `/` list and the model shown come from (REQ-417,
    /// REQ-458); `wanted` is the model it was told to run with.
    pub fn engine_started(&mut self, wanted: Option<&str>) -> Vec<Effect> {
        self.last_total_micro = 0;
        // What waits is the engine's own from now on, not what another `claude` queued (REQ-549).
        self.followed_queue.clear();
        self.wanted_model = wanted.map(str::to_owned);
        self.reloading = false;
        if self.status == ConvStatus::Resuming || self.status == ConvStatus::Dead {
            self.status = ConvStatus::Idle;
        }
        vec![initialize()]
    }

    /// The engine's answer to `initialize`: the commands, and the model it runs with when no turn
    /// has said it yet.
    fn initialized(&mut self, resp: &Value) {
        // An answer with no list would leave only jwn's own: the list it had is better (REQ-757).
        if resp["commands"].is_array() {
            let skills: Option<Vec<&str>> = self
                .skills
                .as_ref()
                .map(|s| s.iter().map(String::as_str).collect());
            (self.commands, self.unmarked) = classify(&resp["commands"], skills.as_deref());
            self.commands_at = stamp();
        }
        // Still the placeholder, or the name it was asked with (`opus`): the engine says which model that is.
        if self.model == "claude" || self.wanted_model.as_deref() == Some(self.model.as_str()) {
            let models = resp["models"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let want = self.wanted_model.as_deref().unwrap_or("default");
            let resolved = models
                .iter()
                .find(|m| m["value"].as_str() == Some(want))
                .and_then(|m| m["resolvedModel"].as_str());
            if let Some(full) = resolved.or(want.starts_with("claude-").then_some(want)) {
                self.model = short_model(full);
            }
        }
    }

    /// Why `/word` is not sent to the engine, when it is not: it only works in Claude's own
    /// screen, or it is one of the lists a client draws itself.
    pub fn refusal(&self, word: &str) -> Option<String> {
        let kind = self
            .commands
            .iter()
            .find(|c| c.name.strip_prefix('/') == Some(word))
            .map(|c| c.kind);
        match kind {
            Some(CommandKind::Panel) => Some(format!(
                "/{word} es una lista que dibuja el cliente: ábrela desde el terminal o el móvil"
            )),
            Some(CommandKind::Screen) => Some(format!(
                "/{word} necesita la pantalla de Claude: usa `command claude`"
            )),
            _ if SCREEN.contains(&word) => Some(format!(
                "/{word} necesita la pantalla de Claude: usa `command claude`"
            )),
            _ => None,
        }
    }

    /// The skills `init` lists tell the commands that are skills from those of the project, for the
    /// ones the engine marked no origin on (REQ-793): a mark is not undone.
    fn learn_skills(&mut self, listed: &Value) {
        let names: Vec<String> = listed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(|n| n.trim_start_matches('/').to_owned()))
            .collect();
        for c in &mut self.commands {
            let name = c.name.trim_start_matches('/');
            if self.unmarked.iter().any(|u| u == name) {
                c.source = if names.iter().any(|n| n == name) {
                    CommandSource::Skill
                } else {
                    CommandSource::Project
                };
            }
        }
        self.skills = Some(names);
    }

    /// The tool item with this id: its state, summary, who answered, its output and its name.
    /// A tool with no result that may have finished unseen: `failed` and `lost`, with what is known of it (REQ-887).
    fn mark_lost(&mut self, id: &str, summary: &str) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                id: tid,
                state,
                summary: said,
                lost,
                ..
            } = item
                && tid == id
            {
                *state = ToolState::Failed;
                *said = Some(summary.to_owned());
                *lost = true;
                return;
            }
        }
    }

    fn set_tool(
        &mut self,
        id: &str,
        f: impl FnOnce(
            &mut (
                ToolState,
                Option<String>,
                Option<String>,
                Vec<String>,
                String,
            ),
        ),
    ) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                id: tid,
                state,
                summary,
                by,
                output,
                name,
                ..
            } = item
                && tid == id
            {
                let mut t = (
                    *state,
                    summary.clone(),
                    by.clone(),
                    output.clone(),
                    name.clone(),
                );
                f(&mut t);
                *state = t.0;
                *summary = t.1;
                *by = t.2;
                *output = t.3;
                return;
            }
        }
    }

    /// Subagents and tasks that ran in front die with the turn they were in; with `all`, those in the
    /// background too, because the engine that ran them is gone. None is left running for ever.
    fn cut_agents(&mut self, all: bool) {
        let links = &self.task_links;
        let behind = |tool: &str| {
            links
                .iter()
                .any(|l| l.background && l.tool_use_id.as_deref() == Some(tool))
        };
        for item in &mut self.items {
            if let Item::Agent { id, state, .. } = item
                && matches!(*state, ToolState::Run | ToolState::Ask)
                && (all || !behind(id))
            {
                *state = ToolState::Failed;
            }
        }
        for t in &mut self.tasks {
            if t.state == TaskState::Run
                && (all || !links.iter().any(|l| l.task == t.id && l.background))
            {
                t.state = TaskState::Stopped;
            }
        }
    }

    /// The ids of the tools that have not answered.
    fn running_tools(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Tool {
                    id,
                    state: ToolState::Run | ToolState::Ask,
                    ..
                } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
}

// ---- small things the reducer uses ----

/// A line for the engine's input.
pub fn user_line(text: &str, images: &[Image]) -> Value {
    let content = if images.is_empty() {
        json!(text)
    } else {
        let mut blocks: Vec<Value> = images
            .iter()
            .map(|i| {
                json!({"type": "image", "source": {"type": "base64", "media_type": i.media_type, "data": i.data}})
            })
            .collect();
        blocks.push(json!({"type": "text", "text": text}));
        json!(blocks)
    };
    json!({"type": "user", "message": {"role": "user", "content": content}})
}

/// The reply to a `can_use_tool` request.
pub fn permission_reply(request: &str, allow: bool, input: &Value, message: &str) -> Value {
    let inner = if allow {
        json!({"behavior": "allow", "updatedInput": input})
    } else {
        json!({"behavior": "deny", "message": message})
    };
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request, "response": inner}})
}

/// `claude-sonnet-5-5` as `sonnet-5.5`.
pub fn short_model(full: &str) -> String {
    let rest = full.strip_prefix("claude-").unwrap_or(full);
    let mut parts = rest.split('-');
    let Some(name) = parts.next() else {
        return full.to_owned();
    };
    let nums: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if nums.is_empty() {
        rest.to_owned()
    } else {
        format!("{name}-{}", nums.join("."))
    }
}

pub(crate) fn clip(s: &str, n: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(n).collect();
    if chars.next().is_some() {
        let mut cut: String = head.chars().take(n.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        head
    }
}

fn kilo(n: u64) -> String {
    if n >= 1000 {
        format!("{}k", (n + 500) / 1000)
    } else {
        n.to_string()
    }
}

fn dollars(micro: u64) -> String {
    format!("${},{:02}", micro / 1_000_000, (micro % 1_000_000) / 10_000)
}

/// What a tool result says as text: a string, or the text blocks of an array.
pub(crate) fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The path or the command a tool is about, cut short.
/// The checklist `TodoWrite` holds: what each line says and whether it is done.
fn parse_todos(input: &Value) -> Vec<Todo> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Todo {
                text: clip(t["content"].as_str()?, LINE_CHARS),
                done: t["status"] == "completed",
            })
        })
        .take(30)
        .collect()
}

/// The questions of an `AskUserQuestion`'s input.
fn parse_questions(input: &Value) -> Vec<Q> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let question = q["question"].as_str()?.to_owned();
            let options: Vec<(String, String)> = q["options"]
                .as_array()?
                .iter()
                .filter_map(|o| {
                    Some((
                        o["label"].as_str()?.to_owned(),
                        o["description"].as_str().unwrap_or("").to_owned(),
                    ))
                })
                .collect();
            (!options.is_empty()).then(|| Q {
                question,
                options,
                multi: q["multiSelect"].as_bool().unwrap_or(false),
            })
        })
        .take(4)
        .collect()
}

/// The ask for the `at`-th question of a call: its id says which, so an answer for another is refused.
fn question_ask(request: &str, at: usize, q: &Q) -> Ask {
    Ask {
        id: format!("{request}:{at}"),
        kind: AskKind::Question,
        tool: "AskUserQuestion".into(),
        title: clip(&q.question, LINE_CHARS),
        command: None,
        diff: Vec::new(),
        // What each option means, for those that say it.
        plan: q
            .options
            .iter()
            .filter(|(_, d)| !d.is_empty())
            .map(|(l, d)| clip(&format!("{l}: {d}"), LINE_CHARS))
            .collect(),
        options: q
            .options
            .iter()
            .enumerate()
            .map(|(i, (l, _))| AskOption {
                key: i.to_string(),
                label: clip(l, LINE_CHARS),
            })
            .collect(),
        multi: q.multi,
        restored: false,
    }
}

/// The ask for a plan: its lines and the three ways to answer it. The first goes back to `auto`
/// when that is the mode the plan was entered from, else it allows edits (REQ-605).
fn plan_ask(request: &str, input: &Value, back: Option<Mode>) -> Ask {
    let first = if back == Some(Mode::Auto) {
        AskOption {
            key: "auto".into(),
            label: "Aprobar y volver a auto".into(),
        }
    } else {
        AskOption {
            key: "accept".into(),
            label: "Aprobar y editar sin preguntar".into(),
        }
    };
    Ask {
        id: request.to_owned(),
        kind: AskKind::Plan,
        tool: "ExitPlanMode".into(),
        title: "Plan listo".into(),
        command: None,
        diff: Vec::new(),
        plan: input["plan"]
            .as_str()
            .unwrap_or("")
            .lines()
            .take(PLAN_LINES)
            .map(|l| clip(l, LINE_CHARS))
            .collect(),
        options: vec![
            first,
            AskOption {
                key: "manual".into(),
                label: "Aprobar, preguntando cada edición".into(),
            },
            AskOption {
                key: "no".into(),
                label: "Seguir planeando".into(),
            },
        ],
        multi: false,
        restored: false,
    }
}

pub fn tool_arg(name: &str, input: &Value) -> String {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input[*k].as_str())
            .map(|s| s.lines().next().unwrap_or("").to_owned())
    };
    let arg = match name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            pick(&["file_path", "path", "notebook_path"])
        }
        "Bash" => pick(&["command"]),
        "Grep" | "Glob" => pick(&["pattern"]),
        "WebFetch" => pick(&["url"]),
        "WebSearch" => pick(&["query"]),
        "Agent" | "Task" => pick(&["description"]),
        "Skill" => pick(&["skill"]),
        "ExitPlanMode" => Some("plan".to_owned()),
        "AskUserQuestion" => input["questions"][0]["question"]
            .as_str()
            .map(str::to_owned),
        _ => None,
    }
    .or_else(|| {
        input.as_object().and_then(|o| {
            o.values().find_map(|v| {
                v.as_str()
                    .map(|s| s.lines().next().unwrap_or("").to_owned())
            })
        })
    })
    .unwrap_or_default();
    clip(&arg, ARG_CHARS)
}

/// The lines an edit changes, from what it is asked to do (the engine gives the strings, not
/// the numbers of the lines).
pub(crate) fn edit_diff(name: &str, input: &Value) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let add = |sign: DiffSign, text: &str, out: &mut Vec<DiffLine>| {
        for l in text.lines().take(OUTPUT_LINES) {
            out.push(DiffLine {
                n: None,
                sign,
                text: clip(l, LINE_CHARS),
            });
        }
    };
    match name {
        "Edit" => {
            add(
                DiffSign::Del,
                input["old_string"].as_str().unwrap_or(""),
                &mut out,
            );
            add(
                DiffSign::Add,
                input["new_string"].as_str().unwrap_or(""),
                &mut out,
            );
        }
        "MultiEdit" => {
            for e in input["edits"].as_array().into_iter().flatten() {
                add(
                    DiffSign::Del,
                    e["old_string"].as_str().unwrap_or(""),
                    &mut out,
                );
                add(
                    DiffSign::Add,
                    e["new_string"].as_str().unwrap_or(""),
                    &mut out,
                );
            }
        }
        "Write" => add(
            DiffSign::Add,
            input["content"].as_str().unwrap_or(""),
            &mut out,
        ),
        _ => {}
    }
    out.truncate(OUTPUT_LINES);
    out
}

fn ask_title(tool: &str, input: &Value) -> (String, Option<String>) {
    let arg = tool_arg(tool, input);
    match tool {
        "Edit" | "MultiEdit" | "NotebookEdit" => (format!("Editar {arg}"), None),
        "Write" => (format!("Escribir {arg}"), None),
        "Bash" => (
            "Ejecutar comando".into(),
            Some(input["command"].as_str().unwrap_or(&arg).to_owned()),
        ),
        other => (
            format!(
                "Usar {other}{}",
                if arg.is_empty() {
                    String::new()
                } else {
                    format!(" · {arg}")
                }
            ),
            None,
        ),
    }
}

/// What «yes for this session» covers: edits, a command's first two words, or the tool.
fn rule_key(tool: &str, input: &Value) -> String {
    match tool {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "edit".into(),
        "Bash" => {
            let words: Vec<&str> = input["command"]
                .as_str()
                .unwrap_or("")
                .split_whitespace()
                .take(2)
                .collect();
            format!("bash:{}", words.join(" "))
        }
        other => other.to_owned(),
    }
}

fn session_label(tool: &str, input: &Value) -> String {
    match tool {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            "Sí, y no volver a preguntar por ediciones en esta sesión".into()
        }
        "Bash" => {
            let key = rule_key(tool, input);
            let prefix = key.strip_prefix("bash:").unwrap_or("");
            format!("Sí, y no volver a preguntar por «{prefix}» en esta sesión")
        }
        other => format!("Sí, y no volver a preguntar por {other} en esta sesión"),
    }
}

pub(crate) fn summary_of(tool: &str, lines: &[String], result: &Value, failed: bool) -> String {
    if failed {
        return lines.first().cloned().unwrap_or_else(|| "falló".into());
    }
    match tool {
        "Read" => result["file"]["numLines"]
            .as_u64()
            .map(|n| format!("Leídas {n} líneas"))
            .unwrap_or_else(|| "Leído".into()),
        "Edit" | "MultiEdit" | "NotebookEdit" => "Editado".into(),
        "Write" => "Escrito".into(),
        _ => lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "listo".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real messages of Claude 2.1.304 for a background shell and for a monitor, replayed
    /// (REQ-607, REQ-609): the monitor is `local_bash` for the engine too, and a Bash result says
    /// where the output is.
    fn replay(capture: &str, session: &str) -> State {
        let mut s = st();
        s.session = session.to_owned();
        for line in capture.lines() {
            s.apply(&serde_json::from_str::<Value>(line).unwrap(), 1);
        }
        s
    }

    /// REQ-605: what the real engine (2.1.304, started with `--permission-mode auto`) sent when
    /// Claude entered plan mode with its own tool and the plan was approved: `status plan` with no
    /// permission asked, then the plan's ask, and, once allowed, `status auto` of its own.
    #[test]
    fn the_capture_of_the_real_engine_entering_plan_from_auto_offers_to_go_back_to_auto() {
        let capture = include_str!("../tests/captures/plan-from-auto-2.1.304.jsonl");
        let mut s = st();
        s.send("entra a plan", vec![], 0);
        let mut offered = None;
        let mut after = None;
        for line in capture.lines() {
            s.apply(&serde_json::from_str::<Value>(line).unwrap(), 1);
            if offered.is_none()
                && let Some(p) = &s.pending
            {
                offered = Some(p.ask.options[0].key.clone());
            }
            if offered.is_some() && after.is_none() && s.mode != Mode::Plan {
                after = Some(s.mode);
            }
        }
        assert_eq!(offered.as_deref(), Some("auto"));
        assert_eq!(
            after,
            Some(Mode::Auto),
            "the engine goes back to the mode it came from"
        );
    }

    #[test]
    fn the_captures_of_the_real_engine_say_a_monitor_is_a_monitor_and_where_the_output_is() {
        let bash = replay(
            include_str!("../tests/captures/local-bash-2.1.304.jsonl"),
            "dda07519-fc54-41e6-a13f-f1ec558a3ee4",
        );
        assert_eq!(bash.tasks[0].kind, TaskKind::Bash);
        let want = std::path::PathBuf::from(
            "/private/tmp/claude-501/-private-tmp-cap-n135/dda07519-fc54-41e6-a13f-f1ec558a3ee4/tasks/b3n42ka0g.output",
        );
        assert_eq!(bash.known_output.as_deref(), Some(want.as_path()));
        let targets = bash.tail_targets();
        assert_eq!(targets.len(), 1, "it ended and was not read yet");
        assert_eq!(targets[0].path.as_deref(), Some(want.as_path()));
        // the files of another session are not beside this one
        let mut other = replay(
            include_str!("../tests/captures/local-bash-2.1.304.jsonl"),
            "dda07519-fc54-41e6-a13f-f1ec558a3ee4",
        );
        other.session = "another".into();
        assert!(other.tail_targets()[0].path.is_none());

        let monitor = replay(
            include_str!("../tests/captures/monitor-2.1.304.jsonl"),
            "082f2384-a6e1-4533-b324-c60b5b2e2025",
        );
        assert_eq!(monitor.tasks[0].kind, TaskKind::Monitor);
    }

    fn st() -> State {
        State::new("2cab1f5a-3d20-4cd2-9d13-485b39b26147".into())
    }

    fn writes(e: &[Effect]) -> Vec<Value> {
        e.iter()
            .filter_map(|e| {
                if let Effect::Write(v) = e {
                    Some(v.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn result(cost: f64) -> Value {
        json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": cost, "duration_ms": 4200, "session_id": "2cab1f5a-3d20-4cd2-9d13-485b39b26147", "terminal_reason": "completed"})
    }

    #[test]
    fn a_message_is_sent_named_and_a_turn_ends_with_its_result() {
        let mut s = st();
        let e = s.send(
            "arregla el test que falla en auth::expired con un token largo",
            vec![],
            1000,
        );
        assert_eq!(
            writes(&e)[0]["message"]["content"],
            "arregla el test que falla en auth::expired con un token largo"
        );
        assert_eq!(s.status, ConvStatus::Thinking);
        assert_eq!(s.name, "arregla el test que falla e…");
        // a second message waits
        assert!(s.send("y luego los tests", vec![], 1100).is_empty());
        assert_eq!(s.queue.len(), 1);
        let e = s.apply(&result(0.06), 5200);
        // the turn ended: the next message went out, the cost was kept, and the person is told
        assert!(e.contains(&Effect::Raise(AttentionKind::AgentDone)));
        assert!(e.contains(&Effect::Resumable(
            "2cab1f5a-3d20-4cd2-9d13-485b39b26147".into()
        )));
        assert_eq!(writes(&e).len(), 1);
        assert_eq!(s.cost, 60_000);
        assert!(s.items.iter().any(|i| matches!(
            i,
            Item::Result {
                cost: 60_000,
                tools: 0,
                ..
            }
        )));
        assert_eq!(s.status, ConvStatus::Thinking);
    }

    #[test]
    fn text_streams_into_one_item_and_the_final_message_settles_it() {
        let mut s = st();
        s.send("hola", vec![], 0);
        s.apply(
            &json!({"type": "stream_event", "event": {"type": "message_start"}}),
            1,
        );
        s.apply(&json!({"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "text"}}}), 2);
        for part in ["Voy a ", "mirar el ", "test."] {
            s.apply(&json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": part}}}), 3);
        }
        assert_eq!(s.status, ConvStatus::Streaming);
        assert!(
            matches!(s.items.last(), Some(Item::Text { text, live: true, .. }) if text == "Voy a mirar el test.")
        );
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Voy a mirar el test."}]}}), 4);
        let texts: Vec<&Item> = s
            .items
            .iter()
            .filter(|i| matches!(i, Item::Text { .. }))
            .collect();
        assert_eq!(texts.len(), 1);
        assert!(matches!(texts[0], Item::Text { live: false, .. }));
    }

    #[test]
    fn a_text_that_settles_with_a_table_carries_its_blocks_and_one_that_is_plain_does_not() {
        let mut s = st();
        s.send("hola", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Listo."}]}}), 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "a\n\n| c | d |\n|---|---|\n| 1 | 2 |"}]}}), 2);
        let got: Vec<bool> = s
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { blocks, .. } => Some(blocks.is_some()),
                _ => None,
            })
            .collect();
        assert_eq!(got, [false, true]);
    }

    #[test]
    fn the_blocks_of_a_text_are_not_saved_and_come_back_from_its_text() {
        let mut s = st();
        s.send("hola", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "a **b**"}]}}), 1);
        let saved = s.saved(2);
        assert!(saved.items.iter().all(|i| !matches!(
            i,
            Item::Text {
                blocks: Some(_),
                ..
            }
        )));
        let back = State::restored(saved, "20:00");
        assert!(back.items.iter().any(|i| matches!(
            i,
            Item::Text {
                blocks: Some(_),
                ..
            }
        )));
    }

    #[test]
    fn the_length_of_the_text_that_arrives_is_known_until_the_message_settles() {
        let mut s = st();
        s.send("hola", vec![], 0);
        assert_eq!(s.live_len(), 0);
        s.apply(&json!({"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "text"}}}), 1);
        s.apply(&json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Voy a mirar"}}}), 2);
        assert_eq!(s.live_len(), 11);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Voy a mirar."}]}}), 3);
        assert_eq!(
            s.live_len(),
            0,
            "nothing arrives any more, so nothing is held back"
        );
    }

    fn node_of(s: &State) -> Conversation {
        let head = Head {
            id: Id::new(),
            v: 1,
            epoch: "e".into(),
            rev: 1,
            attention: Vec::new(),
        };
        s.node(head, &Id::new())
    }

    fn followed_turn() -> crate::transcript::Parser {
        let mut t = crate::transcript::Parser::default();
        t.feed(&format!(
            "{}\n{}\n",
            json!({"type":"user","timestamp":"2026-10-17T11:55:00.000Z","message":{"role":"user","content":"corre los tests"}}),
            json!({"type":"assistant","timestamp":"2026-10-17T11:55:03.000Z","message":{"id":"m1","usage":{"output_tokens":30},"model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}),
        ));
        t
    }

    #[test]
    fn a_turn_of_another_claude_that_is_open_is_followed_with_its_tool_and_its_tokens() {
        let t = followed_turn();
        let mut s = st();
        s.follow_at(&t, t.last_at() + 1_000);
        let node = node_of(&s);
        assert_eq!(
            (node.following, node.tokens),
            (t.turn_since(), 30),
            "since the message of the person, and what it has written"
        );
        assert_eq!(node.running.len(), 1);
        assert_eq!(node.running[0].since, t.last_at());
        assert_eq!(
            node.since, None,
            "a turn that is not jwn's gives no `since`, and so nothing to interrupt"
        );
        // read long after its last line, it is already left behind
        let mut late = st();
        late.follow_at(&t, t.last_at() + FOLLOW_SILENCE_MS + 1);
        assert_eq!(node_of(&late).following, None);
        assert!(node_of(&late).running.is_empty());
    }

    #[test]
    fn a_followed_turn_that_nothing_writes_to_is_left_behind_and_its_tool_was_interrupted() {
        let mut s = st();
        s.follow(&followed_turn());
        s.followed = Some(Followed {
            since: 1,
            at: 1_000,
            tokens: 0,
        });
        assert!(!s.expire_follow(1_000 + FOLLOW_SILENCE_MS));
        assert!(s.expire_follow(1_001 + FOLLOW_SILENCE_MS));
        assert!(!s.expire_follow(2_000 + FOLLOW_SILENCE_MS), "once");
        assert_eq!(node_of(&s).following, None);
        assert!(s.items.iter().any(|i| matches!(i, Item::Tool { state: ToolState::Failed, summary: Some(x), lost: true, .. } if x == crate::transcript::LOST)));
    }

    #[test]
    fn the_tools_of_a_followed_turn_are_interrupted_when_a_message_is_sent_and_when_it_is_restored()
    {
        let t = followed_turn();
        let mut s = st();
        s.follow_at(&t, t.last_at() + 1_000);
        assert!(s.items.iter().any(|i| matches!(
            i,
            Item::Tool {
                state: ToolState::Run,
                ..
            }
        )));
        s.send("sigue", vec![], stamp());
        assert!(!s.items.iter().any(|i| matches!(
            i,
            Item::Tool {
                state: ToolState::Run,
                ..
            }
        )));
        let mut saved = st().saved(1);
        saved.items = vec![Item::Tool {
            id: "t".into(),
            name: "Bash".into(),
            arg: "sleep".into(),
            state: ToolState::Run,
            summary: None,
            output: Vec::new(),
            diff: Vec::new(),
            url: None,
            by: None,
            lost: false,
        }];
        let back = State::restored(saved, "20:00");
        assert!(matches!(
            back.items[0],
            Item::Tool {
                state: ToolState::Failed,
                ..
            }
        ));
    }

    /// REQ-565: the registry of Claude's processes says whether a turn that the file leaves open
    /// is run: «Pensando…» is not drawn for a turn that no `claude` that is busy has.
    #[test]
    fn a_followed_turn_is_left_when_no_claude_that_is_busy_has_the_session() {
        let t = followed_turn();
        let held = |busy| {
            Some(Held {
                pid: 7,
                cwd: "/x".into(),
                busy,
            })
        };
        for (held, kept) in [(held(true), true), (held(false), false), (None, false)] {
            let mut s = st();
            s.follow_at(&t, t.last_at() + 1_000);
            assert!(node_of(&s).following.is_some());
            let changed = s.set_held(held);
            assert_eq!(node_of(&s).following.is_some(), kept);
            assert_eq!(node_of(&s).running.is_empty(), !kept);
            assert!(changed);
        }
    }

    #[test]
    fn a_message_sent_ends_the_following_of_a_turn() {
        let mut s = st();
        s.followed = Some(Followed {
            since: 1,
            at: stamp(),
            tokens: 5,
        });
        s.send("sigue", vec![], stamp());
        assert_eq!(node_of(&s).following, None);
    }

    /// REQ-570: the advisor of jwn's own engine is a row too, with the same words.
    #[test]
    fn a_consultation_of_the_advisor_in_the_own_engine_is_a_row_with_its_answer() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "server_tool_use", "id": "srv1", "name": "advisor", "input": {}}]}}), 1);
        assert!(s.items.iter().any(
            |i| matches!(i, Item::Tool { name, state: ToolState::Run, .. } if name == ADVISING)
        ));
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "advisor_tool_result", "tool_use_id": "srv1", "content": {"type": "advisor_result", "text": "x"}}]}}), 2);
        assert!(s.items.iter().any(|i| matches!(i, Item::Tool { state: ToolState::Ok, summary: Some(x), .. } if x == ADVISED)));
    }

    /// REQ-572: what the engine proposes after a turn is in the node while nothing runs or waits,
    /// and goes when a turn begins, a question waits or a message is queued.
    #[test]
    fn what_the_engine_proposes_is_in_the_node_only_while_nothing_runs_or_waits() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 10, "total_cost_usd": 0.0}), 1);
        assert_eq!(node_of(&s).suggestion, None, "none yet");
        s.apply(&json!({"type": "prompt_suggestion", "suggestion": "  corre los tests  ", "uuid": "u", "session_id": "s"}), 2);
        assert_eq!(node_of(&s).suggestion.as_deref(), Some("corre los tests"));
        s.apply(
            &json!({"type": "prompt_suggestion", "suggestion": "   "}),
            3,
        );
        assert_eq!(node_of(&s).suggestion, None, "an empty one is none");
        s.apply(
            &json!({"type": "prompt_suggestion", "suggestion": "otra"}),
            4,
        );
        s.send("sigue", vec![], 5);
        assert_eq!(node_of(&s).suggestion, None, "a turn began");
        s.apply(
            &json!({"type": "prompt_suggestion", "suggestion": "tarde"}),
            5,
        );
        assert_eq!(
            s.suggestion, None,
            "one that comes during a turn is not kept"
        );
        s.apply(&json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 10, "total_cost_usd": 0.0}), 6);
        assert_eq!(
            node_of(&s).suggestion,
            None,
            "the old one went with the turn"
        );
        s.apply(
            &json!({"type": "prompt_suggestion", "suggestion": "y esto"}),
            7,
        );
        s.queue.push_back(Queued::Text {
            text: "x".into(),
            images: Vec::new(),
        });
        assert_eq!(node_of(&s).suggestion, None, "a message waits");
        s.queue.clear();
        assert!(node_of(&s).suggestion.is_some());
        s.followed = Some(Followed {
            since: 1,
            at: stamp(),
            tokens: 0,
        });
        assert_eq!(
            node_of(&s).suggestion,
            None,
            "another claude runs a turn in the transcript that is followed"
        );
    }

    /// REQ-574: the rows of the advisor in the node say which model it is, when the settings do.
    #[test]
    fn the_rows_of_the_advisor_say_its_model_when_the_settings_name_it() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "server_tool_use", "id": "srv1", "name": "advisor", "input": {}}]}}), 1);
        let arg = |s: &State| {
            node_of(s).items.iter().find_map(|i| match i {
                Item::Tool { name, arg, .. } if name == ADVISING => Some(arg.clone()),
                _ => None,
            })
        };
        assert_eq!(arg(&s).as_deref(), Some(""));
        s.set_advisor(Some("Opus 5.5".into()));
        assert_eq!(arg(&s).as_deref(), Some("using Opus 5.5"));
    }

    #[test]
    fn a_tool_that_runs_is_listed_with_when_it_began_and_is_not_once_it_ends() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}}]}}), 1);
        let run = node_of(&s).running;
        assert_eq!(run.len(), 1);
        assert_eq!(run[0].id, "t1");
        assert!(run[0].since > 0);
        s.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}}), 3);
        assert!(node_of(&s).running.is_empty());
    }

    #[test]
    fn a_conversation_that_runs_no_turn_lists_no_tool_as_running() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "sleep 9"}}]}}), 1);
        assert_eq!(node_of(&s).running.len(), 1);
        s.since = None;
        assert!(node_of(&s).running.is_empty());
    }

    #[test]
    fn the_tokens_of_a_turn_count_each_message_once_and_start_again_with_the_next_turn() {
        let mut s = st();
        s.send("go", vec![], 0);
        assert_eq!(node_of(&s).tokens, 0, "nothing said yet");
        s.apply(&json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "m1", "usage": {"output_tokens": 1}}}}), 1);
        s.apply(&json!({"type": "stream_event", "event": {"type": "message_delta", "usage": {"output_tokens": 40}}}), 2);
        // the same message comes again with the same count, and a second one adds its own
        s.apply(&json!({"type": "assistant", "message": {"id": "m1", "usage": {"output_tokens": 40}, "content": [{"type": "text", "text": "a"}]}}), 3);
        s.apply(&json!({"type": "assistant", "message": {"id": "m1", "usage": {"output_tokens": 40}, "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {}}]}}), 3);
        s.apply(&json!({"type": "assistant", "message": {"id": "m2", "usage": {"output_tokens": 25}, "content": [{"type": "text", "text": "b"}]}}), 4);
        assert_eq!(node_of(&s).tokens, 65);
        s.apply(&json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 10, "total_cost_usd": 0.0}), 5);
        assert_eq!(node_of(&s).tokens, 0, "nothing runs, so nothing is counted");
        s.send("otra", vec![], 6);
        assert_eq!(node_of(&s).tokens, 0, "a new turn starts at nothing");
    }

    #[test]
    fn a_resumed_conversation_says_so_before_the_first_message_sent_and_only_once() {
        let mut s = State::resumed("2cab1f5a-3d20-4cd2-9d13-485b39b26147".into(), None);
        assert!(
            !s.items.iter().any(|i| matches!(i, Item::Divider { .. })),
            "a session that is only followed has no divider"
        );
        s.send("sigue", vec![], 0);
        let kinds: Vec<&str> = s
            .items
            .iter()
            .map(|i| match i {
                Item::Divider {
                    kind: DividerKind::Resume,
                    ..
                } => "resume",
                Item::User { .. } => "user",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["resume", "user"]);
        s.after_end(&mut Vec::new(), 1);
        s.send("más", vec![], 2);
        assert_eq!(
            s.items
                .iter()
                .filter(|i| matches!(i, Item::Divider { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_tool_gets_its_line_its_diff_and_its_result() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "src/auth.rs"}}]}}), 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "src/auth.rs", "old_string": "a < b", "new_string": "a <= b"}}]}}), 2);
        s.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "pub fn verify()\n    …"}]}, "tool_use_result": {"file": {"numLines": 142}}}), 3);
        let tools: Vec<&Item> = s
            .items
            .iter()
            .filter(|i| matches!(i, Item::Tool { .. }))
            .collect();
        match tools[0] {
            Item::Tool {
                arg,
                state,
                summary,
                output,
                ..
            } => {
                assert_eq!(arg, "src/auth.rs");
                assert_eq!(*state, ToolState::Ok);
                assert_eq!(summary.as_deref(), Some("Leídas 142 líneas"));
                assert_eq!(output.len(), 2);
            }
            _ => unreachable!(),
        }
        match tools[1] {
            Item::Tool { diff, .. } => {
                assert_eq!(diff.len(), 2);
                assert_eq!(diff[0].sign, DiffSign::Del);
                assert_eq!(diff[1].text, "a <= b");
            }
            _ => unreachable!(),
        }
        assert_eq!(s.tools_in_turn, 2);
    }

    fn can_use(id: &str, tool: &str, input: Value, suggested: bool) -> Value {
        json!({"type": "control_request", "request_id": id, "request": {"subtype": "can_use_tool", "tool_name": tool, "input": input, "tool_use_id": "toolu_x",
            "permission_suggestions": if suggested { json!([{"type": "addRules"}]) } else { json!([]) }}})
    }

    /// REQ-877: a permission is kept for a restart unless a subagent asked it; a question or a plan is not kept (the transcript has them).
    #[test]
    fn only_a_permission_that_the_main_agent_asked_is_kept_for_a_restart() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&can_use("r1", "Bash", json!({"command": "ls"}), true), 1);
        let kept = s.saved(0).pending.expect("a permission waits");
        assert_eq!(
            (
                kept.tool.as_str(),
                kept.tool_use_id.as_str(),
                kept.suggested
            ),
            ("Bash", "toolu_x", true)
        );
        let mut nested = can_use("r2", "Bash", json!({"command": "ls"}), false);
        nested["request"]["agent_id"] = json!("a1");
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&nested, 1);
        assert!(s.pending.is_some() && s.saved(0).pending.is_none());
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(
            &can_use("r3", "ExitPlanMode", json!({"plan": "# Plan"}), false),
            1,
        );
        assert!(s.pending.is_some() && s.saved(0).pending.is_none());
    }

    /// REQ-875: the items are made again when the transcript grows, and the tool of a restored ask is still the one that asks.
    #[test]
    fn making_the_items_again_leaves_the_tool_of_a_restored_ask_asking() {
        let call = json!({"type":"assistant","message":{"model":"m","content":[{"type":"tool_use","id":"toolu_q","name":"AskUserQuestion","input":{"questions":[{"question":"¿Letra?","header":"L","options":[{"label":"A","description":"a"}]}]}}]}});
        let text = json!({"type":"user","message":{"role":"user","content":"hola"}}).to_string()
            + "\n"
            + &call.to_string()
            + "\n";
        let parser = crate::transcript::parse(&text);
        let mut s = State::resumed("old".into(), Some(&parser));
        let fx = s.restore_ask(Some(&parser));
        assert!(matches!(
            fx.as_slice(),
            [Effect::Raise(AttentionKind::Question)]
        ));
        assert!(s.pending.as_ref().is_some_and(|p| p.ask.restored));
        s.follow(&parser);
        assert!(
            matches!(
                s.items.last(),
                Some(Item::Tool {
                    state: ToolState::Ask,
                    ..
                })
            ),
            "{:?}",
            s.items.last()
        );
        // a session another `claude` holds is not asked in
        let mut held = State::resumed("old".into(), Some(&parser));
        held.set_held(Some(Held {
            pid: 1,
            cwd: "/x".into(),
            busy: false,
        }));
        assert!(held.restore_ask(Some(&parser)).is_empty() && held.pending.is_none());
    }

    fn restored(tool: &str, input: Value, id: &str) -> State {
        let mut s = st();
        s.restore_pending_for_test(tool, input, id);
        s
    }

    /// REQ-879 to REQ-881: what a restored ask says when it is answered, and what it leaves for the next call.
    #[test]
    fn a_restored_ask_answers_with_a_message_and_the_mode_that_the_answer_means() {
        let line = |fx: Vec<Effect>| {
            assert!(matches!(fx[0], Effect::ClearAsk), "{fx:?}");
            fx.iter()
                .find_map(|e| match e {
                    Effect::Resume(l) => Some(l.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{fx:?}"))
        };
        // two questions are answered one after the other and go as one message of two lines
        let two = json!({"questions": [
            {"question": "¿Uno?", "header": "U", "options": [{"label": "A", "description": ""}, {"label": "B", "description": ""}], "multiSelect": false},
            {"question": "¿Dos?", "header": "D", "options": [{"label": "X", "description": ""}], "multiSelect": true}]});
        let mut s = restored("AskUserQuestion", two, "q");
        assert_eq!(s.answer("q:0", "1", None, None).unwrap(), Vec::new());
        assert!(
            s.pending
                .as_ref()
                .is_some_and(|p| p.ask.restored && p.ask.id == "q:1")
        );
        let said = line(s.answer("q:1", "0", Some("Z"), None).unwrap());
        assert_eq!(said, "Elegí «B» para «¿Uno?»\nElegí «X, Z» para «¿Dos?»");
        // a plan: the mode is the answer's, and the refusal keeps `plan` with what the person wrote
        let plan = json!({"plan": "# Plan"});
        for (key, text, mode, said) in [
            (
                "accept",
                None,
                Mode::AcceptEdits,
                "Aprobado: seguí con el plan y editá sin preguntar.",
            ),
            ("manual", None, Mode::Manual, "Aprobado: seguí con el plan."),
            (
                "no",
                Some(" usa i32 "),
                Mode::Plan,
                "No apruebo el plan; seguí planeando: usa i32",
            ),
            (
                "no",
                None,
                Mode::Plan,
                "No apruebo el plan; seguí planeando.",
            ),
        ] {
            let mut s = restored("ExitPlanMode", plan.clone(), "p");
            s.mode = Mode::Plan;
            assert_eq!(line(s.answer("p", key, text, None).unwrap()), said);
            assert_eq!(s.mode, mode);
        }
        // a permission: yes allows the same call once and no does not
        let input = json!({"command": "touch probe-file"});
        let mut s = restored("Bash", input.clone(), "b");
        assert_eq!(
            line(s.answer("b", "yes", None, None).unwrap()),
            "Sí, adelante con Bash touch probe-file"
        );
        let again = |s: &mut State, input: Value| s.apply(&can_use("r2", "Bash", input, false), 1);
        let other = again(&mut s, json!({"command": "rm -rf x"}));
        assert!(
            s.pending.is_some() && matches!(other.as_slice(), [Effect::Raise(_)]),
            "another call is asked"
        );
        s.pending = None;
        let same = again(&mut s, input.clone());
        assert!(
            s.pending.is_none() && matches!(same.as_slice(), [Effect::Write(_)]),
            "the same call is allowed once"
        );
        assert!(s.allow_once.is_none());
        let mut s = restored("Bash", input, "b");
        assert_eq!(
            line(s.answer("b", "no", Some("no"), None).unwrap()),
            "No, no ejecutes Bash touch probe-file: no"
        );
        assert!(s.allow_once.is_none());
    }

    /// REQ-881: the tool of an ask of before a restart gets no result on the stream, so it ends when it is answered; `^C` on it drops it.
    #[test]
    fn the_tool_of_a_restored_ask_ends_when_it_is_answered_and_is_not_left_running() {
        let state = |s: &State| match s.items.last() {
            Some(Item::Tool { state, lost, .. }) => (*state, *lost),
            other => panic!("{other:?}"),
        };
        let with_tool = |tool: &str, input: Value, id: &str| {
            let mut s = st();
            s.push(Item::Tool {
                id: id.into(),
                name: tool.into(),
                arg: String::new(),
                state: ToolState::Failed,
                summary: None,
                output: Vec::new(),
                diff: Vec::new(),
                url: None,
                by: None,
                lost: false,
            });
            s.restore_pending_for_test(tool, input, id);
            s
        };
        let q = json!({"questions": [{"question": "¿Uno?", "header": "U", "options": [{"label": "A", "description": ""}], "multiSelect": false}]});
        let mut s = with_tool("AskUserQuestion", q.clone(), "q");
        assert_eq!(state(&s), (ToolState::Ask, false));
        s.answer("q:0", "0", None, None).unwrap();
        assert_eq!(state(&s), (ToolState::Ok, false));
        let mut s = with_tool("ExitPlanMode", json!({"plan": "# P"}), "p");
        s.answer("p", "accept", None, None).unwrap();
        assert_eq!(state(&s), (ToolState::Ok, false));
        let mut s = with_tool("Bash", json!({"command": "ls"}), "b");
        s.answer("b", "yes", None, None).unwrap();
        assert_eq!(
            state(&s),
            (ToolState::Failed, true),
            "it did not run and has no result"
        );
        let mut s = with_tool("Bash", json!({"command": "ls"}), "b");
        s.status = ConvStatus::Asking;
        let fx = s.interrupt();
        assert!(matches!(fx.as_slice(), [Effect::ClearAsk]), "{fx:?}");
        assert!(s.pending.is_none() && !s.interrupting && s.status == ConvStatus::Dead);
        assert_eq!(state(&s).0, ToolState::Denied);
    }

    /// REQ-605: a plan entered from `auto` offers to go back to it; from any other mode, or when the
    /// conversation began in plan or left it another way, it allows edits as before.
    #[test]
    fn a_plan_entered_from_auto_goes_back_to_auto_and_from_any_other_mode_allows_edits() {
        let status = |m: &str| json!({"type": "system", "subtype": "status", "status": null, "permissionMode": m});
        let plan_asks = |s: &mut State| {
            s.send("go", vec![], 0);
            s.apply(
                &can_use(
                    "rp",
                    "ExitPlanMode",
                    json!({"plan": "# Plan\n1. algo"}),
                    false,
                ),
                1,
            );
            s.pending.as_ref().unwrap().ask.options.clone()
        };
        let first = |o: &[AskOption]| (o[0].key.clone(), o[0].label.clone());
        // from auto
        let mut s = st();
        s.apply(&status("auto"), 0);
        s.apply(&status("plan"), 0);
        assert_eq!(s.mode, Mode::Plan);
        let options = plan_asks(&mut s);
        assert_eq!(
            options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            ["auto", "manual", "no"],
            "still three, in the same places"
        );
        assert_eq!(
            first(&options),
            ("auto".into(), "Aprobar y volver a auto".into())
        );
        // an answer that is not one of them is refused; «auto» approves and asks the engine for auto at once
        assert!(s.answer("rp", "accept", None, None).is_err());
        let e = s.answer("rp", "auto", None, None).unwrap();
        let w = writes(&e);
        assert_eq!(w[0]["response"]["response"]["behavior"], "allow");
        assert_eq!(w[1]["request"]["subtype"], "set_permission_mode");
        assert_eq!(w[1]["request"]["mode"], "auto");
        // the engine says it left plan: what was remembered is forgotten
        s.apply(&status("auto"), 2);
        assert_eq!(s.before_plan, None);
        // from manual, and from accept edits: the first allows edits
        for from in ["default", "acceptEdits"] {
            let mut s = st();
            s.apply(&status(from), 0);
            s.apply(&status("plan"), 0);
            let options = plan_asks(&mut s);
            assert_eq!(
                first(&options),
                ("accept".into(), "Aprobar y editar sin preguntar".into()),
                "from {from}"
            );
        }
        // began in plan: nothing to go back to
        let mut s = st();
        s.apply(&json!({"type": "system", "subtype": "init", "permissionMode": "plan", "model": "claude-sonnet-5-5"}), 0);
        assert_eq!(first(&plan_asks(&mut s)).0, "accept");
        // left plan with ⇧tab and entered again from manual: the first plan's past is not kept
        let mut s = st();
        s.apply(&status("auto"), 0);
        s.apply(&status("plan"), 0);
        s.apply(&status("default"), 0);
        s.apply(&status("plan"), 0);
        assert_eq!(first(&plan_asks(&mut s)).0, "accept");
        // «manual» and «no» are as they were
        let e = s.answer("rp", "manual", None, None).unwrap();
        assert!(
            !writes(&e)
                .iter()
                .any(|w| w["request"]["subtype"] == "set_permission_mode")
        );
        // the transcript the engine writes can say `plan` a moment before its message does: the
        // mode it left is remembered all the same
        let mut s = st();
        s.apply(&status("auto"), 0);
        let mut t = crate::transcript::Parser::default();
        t.mode = Some(Mode::Plan);
        s.follow(&t);
        s.apply(&status("plan"), 0);
        assert_eq!(first(&plan_asks(&mut s)).0, "auto");
        // a conversation saved in plan comes back, and its engine starts in plan, knowing where it came from
        let saved = s.saved(9);
        assert_eq!(saved.before_plan, Some(Mode::Auto));
        let mut back = State::restored(saved, "");
        back.apply(&json!({"type": "system", "subtype": "init", "permissionMode": "plan", "model": "claude-sonnet-5-5"}), 0);
        assert_eq!(first(&plan_asks(&mut back)).0, "auto");
        // saved in any other mode, it keeps nothing
        let mut s = st();
        s.apply(&status("auto"), 0);
        assert_eq!(s.saved(9).before_plan, None);
    }

    #[test]
    fn a_permission_waits_and_the_first_answer_settles_it() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_x", "name": "Edit", "input": {"file_path": "a.rs", "old_string": "x", "new_string": "y"}}]}}), 1);
        let e = s.apply(
            &can_use(
                "req-1",
                "Edit",
                json!({"file_path": "a.rs", "old_string": "x", "new_string": "y"}),
                true,
            ),
            2,
        );
        assert_eq!(e, vec![Effect::Raise(AttentionKind::Permission)]);
        assert_eq!(s.status, ConvStatus::Asking);
        let ask = s.pending.as_ref().unwrap().ask.clone();
        assert_eq!(ask.title, "Editar a.rs");
        assert_eq!(
            ask.options
                .iter()
                .map(|o| o.key.as_str())
                .collect::<Vec<_>>(),
            ["yes", "session", "no"]
        );
        assert_eq!(ask.diff.len(), 2);
        // a wrong question or key is refused and nothing changes
        assert!(s.answer("req-9", "yes", None, None).is_err());
        assert!(s.answer("req-1", "maybe", None, None).is_err());
        assert!(s.pending.is_some());
        let e = s
            .answer("req-1", "session", None, Some("el móvil"))
            .unwrap();
        assert_eq!(e[0], Effect::ClearAsk);
        let w = writes(&e);
        assert_eq!(w[0]["response"]["request_id"], "req-1");
        assert_eq!(w[0]["response"]["response"]["behavior"], "allow");
        assert!(s.pending.is_none());
        match s
            .items
            .iter()
            .find(|i| matches!(i, Item::Tool { name, .. } if name == "Edit"))
            .unwrap()
        {
            Item::Tool { by, state, .. } => {
                assert_eq!(by.as_deref(), Some("el móvil"));
                assert_eq!(*state, ToolState::Run);
            }
            _ => unreachable!(),
        }
        // the next edit is allowed without asking
        let e = s.apply(
            &can_use("req-2", "Edit", json!({"file_path": "b.rs"}), true),
            3,
        );
        assert_eq!(writes(&e)[0]["response"]["response"]["behavior"], "allow");
        assert!(s.pending.is_none());
    }

    #[test]
    fn a_refusal_tells_the_engine_and_marks_the_tool() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_x", "name": "Bash", "input": {"command": "rm -rf build"}}]}}), 1);
        s.apply(
            &can_use("req-1", "Bash", json!({"command": "rm -rf build"}), false),
            2,
        );
        // no session answer is offered when the engine suggested none
        assert_eq!(s.pending.as_ref().unwrap().ask.options.len(), 2);
        let e = s.answer("req-1", "no", Some("déjalo"), None).unwrap();
        let w = writes(&e);
        assert_eq!(w[0]["response"]["response"]["behavior"], "deny");
        assert!(
            w[0]["response"]["response"]["message"]
                .as_str()
                .unwrap()
                .contains("déjalo")
        );
        s.apply(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_x", "is_error": true, "content": "denied"}]}}), 3);
        assert!(s.items.iter().any(|i| matches!(
            i,
            Item::Tool {
                state: ToolState::Denied,
                ..
            }
        )));
    }

    #[test]
    fn an_interrupt_refuses_what_waits_and_the_turn_says_so() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_x", "name": "Bash", "input": {"command": "sleep 9"}}]}}), 1);
        s.apply(
            &can_use("req-1", "Bash", json!({"command": "sleep 9"}), false),
            2,
        );
        let e = s.interrupt();
        let w = writes(&e);
        assert_eq!(w[0]["response"]["response"]["behavior"], "deny");
        assert_eq!(w[1]["request"]["subtype"], "interrupt");
        assert!(s.interrupt().is_empty(), "a second interrupt does nothing");
        s.apply(&json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_streaming", "total_cost_usd": 0.01}), 3);
        assert!(
            s.items
                .iter()
                .any(|i| matches!(i, Item::Note { text, .. } if text == "Interrumpido por ti"))
        );
        assert_eq!(s.status, ConvStatus::Idle);
    }

    fn queued(s: &State) -> Vec<String> {
        s.queue.iter().map(Queued::shown).collect()
    }

    /// REQ-966, REQ-969: a turn that runs is interrupted, the message goes first, and the last of two goes first.
    #[test]
    fn send_first_interrupts_a_turn_and_goes_before_what_waited() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.send("later", vec![], 1);
        let w = writes(&s.send_first("ahora", vec![], 2));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0]["request"]["subtype"], "interrupt");
        assert_eq!(queued(&s), ["ahora", "later"]);
        assert!(s.send_first("más nuevo", vec![], 3).is_empty());
        assert_eq!(queued(&s), ["más nuevo", "ahora", "later"]);
        let e = s.apply(&json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_streaming", "total_cost_usd": 0.01}), 4);
        let w = writes(&e);
        assert!(w[0].to_string().contains("más nuevo"), "{w:?}");
        assert_eq!(queued(&s), ["ahora", "later"]);
    }

    /// REQ-965: nothing runs, so it is a plain send. REQ-967: a local command is not interrupted.
    #[test]
    fn send_first_is_plain_when_nothing_runs_and_waits_for_a_local_command() {
        let mut s = st();
        let w = writes(&s.send_first("hola", vec![], 0));
        assert!(w[0].to_string().contains("hola") && s.queue.is_empty());
        let mut s = st();
        s.set(None, None, Some("opus".into()), None, None);
        assert!(matches!(s.inflight, Some(Inflight::Local(_))));
        assert!(s.send_first("después", vec![], 1).is_empty());
        assert_eq!(queued(&s), ["después"]);
    }

    /// REQ-968: an ask restored has no engine to interrupt: it is dropped and the message goes.
    #[test]
    fn send_first_drops_a_restored_ask_and_sends() {
        let mut s = restored("Bash", json!({"command": "ls"}), "t");
        assert!(s.pending.is_some());
        let e = s.send_first("otra cosa", vec![], 5);
        assert!(s.pending.is_none() && s.queue.is_empty());
        assert!(e.iter().any(|e| matches!(e, Effect::ClearAsk)), "{e:?}");
        assert!(writes(&e)[0].to_string().contains("otra cosa"), "{e:?}");
    }

    #[test]
    fn a_turn_the_engine_starts_by_itself_holds_the_queue_and_still_ends() {
        let mut s = st();
        // a monitor's line makes the engine start a turn nobody asked for
        s.apply(&json!({"type": "system", "subtype": "init", "model": "claude-sonnet-5-5", "permissionMode": "default"}), 10);
        assert!(s.busy());
        assert_eq!(s.model, "sonnet-5.5");
        assert!(s.send("hola", vec![], 11).is_empty());
        let e = s.apply(&result(0.02), 20);
        assert!(!s.auto);
        assert!(e.contains(&Effect::Raise(AttentionKind::AgentDone)));
        // what waited went out once it ended
        assert_eq!(writes(&e).len(), 1);
    }

    #[test]
    fn the_model_changes_only_when_the_engine_says_it_did() {
        let mut s = st();
        let e = s.set(None, None, Some("opus".into()), None, None);
        assert_eq!(writes(&e)[0]["message"]["content"], "/model opus");
        assert_eq!(s.model, "claude");
        s.apply(&json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.0}), 1);
        assert_eq!(s.model, "opus");
        // a local command's result is not a turn: no result item, no attention
        assert!(!s.items.iter().any(|i| matches!(i, Item::Result { .. })));
        // while a turn runs the change waits its turn
        s.send("go", vec![], 2);
        assert!(s.set(None, None, None, Some("max".into()), None).is_empty());
        let e = s.apply(&result(0.03), 3);
        assert_eq!(writes(&e)[0]["message"]["content"], "/effort max");
    }

    #[test]
    fn a_mode_is_asked_of_the_engine_and_shown_when_it_says_it_changed() {
        let mut s = st();
        let e = s.set(None, None, None, None, Some(Mode::Plan));
        let w = writes(&e);
        assert_eq!(w[0]["request"]["subtype"], "set_permission_mode");
        assert_eq!(w[0]["request"]["mode"], "plan");
        assert_eq!(
            s.mode,
            Mode::Manual,
            "shown only once the engine says it runs with it"
        );
        s.apply(&json!({"type": "system", "subtype": "status", "status": null, "permissionMode": "plan"}), 1);
        assert_eq!(s.mode, Mode::Plan);
        assert!(
            !s.items.iter().any(|i| matches!(i, Item::Note { .. })),
            "a change of mode is in the footer and not a line of the conversation"
        );
        // while a turn runs it is asked at once, and the turn goes on
        s.send("go", vec![], 2);
        let e = s.set(None, None, None, None, Some(Mode::Auto));
        assert_eq!(writes(&e)[0]["request"]["mode"], "auto");
        assert_eq!(
            s.mode,
            Mode::Plan,
            "shown when the engine says so, as always"
        );
        s.apply(&json!({"type": "system", "subtype": "status", "status": null, "permissionMode": "auto"}), 3);
        assert_eq!(s.mode, Mode::Auto);
        s.apply(&result(0.0), 3);
        // a refusal of the engine is told
        s.apply(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": "jwn-1", "error": "no se puede"}}), 4);
        assert!(s.items.iter().any(
            |i| matches!(i, Item::Note { text, tone: Some(Tone::Error) } if text == "no se puede")
        ));
    }

    #[test]
    fn a_second_mode_change_before_the_first_answer_still_reaches_the_engine() {
        let mut s = st();
        let first = writes(&s.set(None, None, None, None, Some(Mode::Plan)));
        let second = writes(&s.set(None, None, None, None, Some(Mode::Manual)));
        assert_eq!(first[0]["request"]["mode"], "plan");
        assert_eq!(
            second[0]["request"]["mode"], "manual",
            "the engine ends where the last change said, though the shown mode never left manual"
        );
        assert_eq!(s.mode, Mode::Manual);
    }

    #[test]
    fn what_a_local_command_answers_is_its_output_and_not_a_message_of_claudes() {
        let mut s = st();
        s.set(None, None, None, Some("low".into()), None);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Set effort level to low"}]}}), 1);
        assert!(
            matches!(s.items.last(), Some(Item::Out { lines }) if lines == &["Set effort level to low"])
        );
        assert!(!s.items.iter().any(|i| matches!(i, Item::Text { .. })));
    }

    #[test]
    fn the_shell_lines_go_to_the_engine_with_the_next_message() {
        let mut s = st();
        s.shell_started("git status");
        s.shell_done("git status", true, vec!["On branch main".into()]);
        let e = s.send("qué ves", vec![], 0);
        let sent = writes(&e)[0]["message"]["content"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(sent.starts_with("! git status\nOn branch main"));
        assert!(sent.ends_with("qué ves"));
        assert!(matches!(
            &s.items[0],
            Item::Shell {
                state: ToolState::Ok,
                ..
            }
        ));
    }

    #[test]
    fn images_go_as_blocks_before_the_text() {
        let line = user_line(
            "mira",
            &[Image {
                media_type: "image/jpeg".into(),
                data: "AA==".into(),
            }],
        );
        let c = line["message"]["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(c[1]["text"], "mira");
    }

    #[test]
    fn a_dead_engine_fails_what_ran_and_keeps_the_queue() {
        let mut s = st();
        s.send("go", vec![], 0);
        s.send("después", vec![], 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "sleep 9"}}]}}), 2);
        s.engine_gone("el motor terminó con código 1");
        assert_eq!(s.status, ConvStatus::Dead);
        assert!(s.items.iter().any(|i| matches!(
            i,
            Item::Tool {
                state: ToolState::Failed,
                ..
            }
        )));
        assert_eq!(s.queue.len(), 1);
        assert!(!s.busy());
    }

    #[test]
    fn only_the_last_items_are_kept_and_first_counts_the_rest() {
        let mut s = st();
        for i in 0..(KEEP + 10) {
            s.note(format!("n{i}"), None);
        }
        assert_eq!(s.items.len(), KEEP);
        assert_eq!(s.first, 10);
    }

    #[test]
    fn a_saved_conversation_comes_back_dead_with_a_divider() {
        let mut s = st();
        s.send("hola", vec![], 0);
        s.apply(&result(0.05), 1);
        let saved = s.saved(5);
        let text = serde_json::to_string(&saved).unwrap();
        let back = State::restored(serde_json::from_str(&text).unwrap(), "20:10");
        assert_eq!(back.status, ConvStatus::Dead);
        assert_eq!(back.name, "hola");
        assert_eq!(back.cost, 50_000);
        assert!(
            matches!(back.items.last(), Some(Item::Divider { text, kind: DividerKind::Restart }) if text == "el daemon se reinició · 20:10")
        );
    }

    fn args(a: &[&str]) -> Result<OpenArgs, String> {
        parse_args(&a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_flags_a_conversation_serves_are_parsed_and_the_rest_are_not() {
        let a = args(&[
            "--model",
            "opus",
            "--effort=max",
            "-n",
            "login",
            "--permission-mode",
            "plan",
            "--add-dir",
            "../a",
            "--add-dir",
            "../b",
            "arregla esto",
        ])
        .unwrap();
        assert_eq!(a.model.as_deref(), Some("opus"));
        assert_eq!(a.effort.as_deref(), Some("max"));
        assert_eq!(a.name.as_deref(), Some("login"));
        assert_eq!(a.mode, Some(Mode::Plan));
        assert_eq!(a.add_dirs, ["../a", "../b"]);
        assert_eq!(a.message.as_deref(), Some("arregla esto"));
        assert!(args(&["-c"]).unwrap().continue_latest);
        assert_eq!(args(&["-r", "abc"]).unwrap().resume.as_deref(), Some("abc"));
        // REQ-634, REQ-572: Claude in Chrome and the suggestions are served
        assert_eq!(args(&["--chrome"]).unwrap().chrome, Some(true));
        assert_eq!(args(&["--no-chrome", "hola"]).unwrap().chrome, Some(false));
        assert_eq!(args(&["hola"]).unwrap().chrome, None);
        let q = args(&["--prompt-suggestions", "false"]).unwrap();
        assert_eq!(q.prompt_suggestions.as_deref(), Some("false"));
        assert_eq!(
            args(&["--prompt-suggestions=true"])
                .unwrap()
                .prompt_suggestions
                .as_deref(),
            Some("true")
        );
        assert_eq!(args(&[]).unwrap(), OpenArgs::default());
        // what the engine cannot serve runs the original (REQ-373)
        for bad in [
            &["-p", "x"][..],
            &["--print"],
            &["--help"],
            &["-v"],
            &["--version"],
            &["--output-format", "json"],
            &["mcp", "list"],
            &["update"],
            &["--resume"],
            &["--model"],
            &["--permission-mode", "bypassPermissions"],
            &["--dangerously-skip-permissions"],
        ] {
            assert!(args(bad).is_err(), "{bad:?} must not be served");
        }
        // a message that merely starts like a subcommand is a message
        assert_eq!(
            args(&["update the docs please"])
                .unwrap()
                .message
                .as_deref(),
            Some("update the docs please")
        );
    }

    #[test]
    fn the_models_have_short_names() {
        assert_eq!(short_model("claude-sonnet-5-5"), "sonnet-5.5");
        assert_eq!(short_model("claude-opus-4-1-20250805"), "opus-4.1");
        assert_eq!(short_model("fable"), "fable");
    }

    #[test]
    fn the_engines_version_is_what_init_says_and_goes_with_the_engine() {
        let mut s = st();
        let head = || Head {
            id: "c".into(),
            v: 1,
            epoch: "e".into(),
            rev: 1,
            attention: Vec::new(),
        };
        assert_eq!(s.node(head(), &"p".into()).engine_version, None);
        s.apply(&json!({"type": "system", "subtype": "init", "model": "claude-sonnet-5-5", "claude_code_version": "2.1.309"}), 0);
        assert_eq!(
            s.node(head(), &"p".into()).engine_version.as_deref(),
            Some("2.1.309")
        );
        // An init that does not say it keeps what the engine said before.
        s.apply(
            &json!({"type": "system", "subtype": "init", "model": "claude-sonnet-5-5"}),
            1,
        );
        assert_eq!(
            s.node(head(), &"p".into()).engine_version.as_deref(),
            Some("2.1.309")
        );
        s.engine_gone("el motor terminó");
        assert_eq!(s.node(head(), &"p".into()).engine_version, None);
    }

    #[test]
    fn clear_empties_the_items_and_takes_the_new_session() {
        let mut s = st();
        s.send("hola", vec![], 0);
        s.apply(&json!({"type": "conversation_reset", "new_conversation_id": "n-1", "trigger": "clear"}), 1);
        assert_eq!(s.session, "n-1");
        assert_eq!(s.items.len(), 1);
        assert!(matches!(
            s.items[0],
            Item::Divider {
                kind: DividerKind::Clear,
                ..
            }
        ));
    }

    #[test]
    fn the_engines_commands_are_tagged_by_where_they_are_defined() {
        let listed = json!([
            {"name": "context", "description": "Show context usage", "builtin": true},
            {"name": "doctor", "builtin": true},
            {"name": "init", "builtin": true},
            {"name": "model", "description": "the engine's own", "builtin": true},
            {"name": "deploy", "description": "publicar"},
            {"name": "jw", "description": "(user) skill"},
            {"name": "vercel:deploy", "description": "(vercel) deploy"},
            {"name": ""},
        ]);
        let by = |skills: Option<&[&str]>| -> Vec<(String, CommandSource, CommandKind)> {
            classify_commands(&listed, skills)
                .into_iter()
                .map(|c| (c.name, c.source, c.kind))
                .collect()
        };
        let all = by(None);
        let find = |n: &str| all.iter().find(|c| c.0 == n).cloned();
        assert_eq!(all.iter().filter(|c| c.0 == "/model").count(), 1);
        assert_eq!(find("/model").unwrap().1, CommandSource::Jwn);
        let help = find("/help").unwrap();
        assert_eq!(
            (help.1, help.2),
            (CommandSource::Jwn, CommandKind::Panel),
            "the lists jwn draws itself are its own"
        );
        let rewind = find("/rewind").unwrap();
        assert_eq!(
            (rewind.1, rewind.2),
            (CommandSource::Claude, CommandKind::Screen),
            "all of Claude's are listed; the ones only its screen has say so"
        );
        assert_eq!(find("/continue").unwrap().2, CommandKind::Panel);
        assert_eq!(find("/plan").unwrap().1, CommandSource::Jwn);
        assert_eq!(find("/context").unwrap().2, CommandKind::Text);
        assert_eq!(find("/doctor").unwrap().2, CommandKind::Screen);
        assert_eq!(find("/init").unwrap().2, CommandKind::Prompt);
        assert_eq!(find("/vercel:deploy").unwrap().1, CommandSource::Plugin);
        assert_eq!(
            find("/deploy").unwrap().1,
            CommandSource::Skill,
            "until init says which are skills"
        );
        let told = by(Some(&["jw"]));
        assert_eq!(
            told.iter().find(|c| c.0 == "/deploy").unwrap().1,
            CommandSource::Project
        );
        assert_eq!(
            told.iter().find(|c| c.0 == "/jw").unwrap().1,
            CommandSource::Skill
        );
    }

    /// What claude 2.1.309 answered to `initialize` for a few of 124 commands, in a private
    /// folder with a project skill and command (`docs/claude-engine.md`).
    const LISTED: &str = include_str!("../tests/captures/initialize-commands-2.1.309.json");

    #[test]
    fn the_marks_of_the_description_say_where_a_command_is_defined() {
        let listed: Value = serde_json::from_str(LISTED).unwrap();
        let all = classify_commands(&listed, None);
        let find = |n: &str| all.iter().find(|c| c.name == format!("/{n}")).unwrap();
        let (skill, command) = (find("android-review"), find("commit"));
        assert_eq!(
            (skill.source, command.source),
            (CommandSource::Skill, CommandSource::Skill)
        );
        let described = skill.description.as_deref().unwrap();
        assert!(
            described.starts_with("Reviews Kotlin") && !described.contains("(user)"),
            "the mark is read on the whole description, which is longer than the clip: {described}"
        );
        assert_eq!(
            command
                .description
                .as_deref()
                .map(|d| d.ends_with("(user)")),
            Some(false)
        );
        for (n, text) in [
            ("proj-skill", "a project skill"),
            ("proj-cmd", "a project command"),
        ] {
            let c = find(n);
            assert_eq!(
                (c.source, c.description.as_deref()),
                (CommandSource::Project, Some(text))
            );
        }
        let (deploy, sdk, ponytail) = (
            find("vercel:deploy"),
            find("vercel:ai-sdk"),
            find("ponytail:ponytail"),
        );
        for (c, group) in [(deploy, "vercel"), (sdk, "vercel"), (ponytail, "ponytail")] {
            assert_eq!(
                (c.source, c.group.as_deref()),
                (CommandSource::Plugin, Some(group))
            );
            assert!(
                !c.description.as_deref().unwrap().starts_with('('),
                "{:?}",
                c.description
            );
        }
        let docs = find("docs");
        assert_eq!(
            (docs.source, docs.group.as_deref()),
            (CommandSource::Plugin, Some("claude.ai"))
        );
        assert!(
            !docs
                .description
                .as_deref()
                .unwrap()
                .contains("(claude.ai sync)")
        );
        for n in ["dataviz", "deep-research", "code-review"] {
            assert_eq!(
                (find(n).source, find(n).group.as_deref()),
                (CommandSource::Claude, None),
                "{n}"
            );
        }
        assert!(
            find("deep-research")
                .description
                .as_deref()
                .unwrap()
                .ends_with("(dynamic workflow)")
        );
        assert!(
            all.iter()
                .all(|c| c.group.is_none() || c.source == CommandSource::Plugin),
            "only a plugin has a group (REQ-794)"
        );
    }

    #[test]
    fn a_command_with_no_mark_is_tagged_as_before_and_init_skills_retag_only_it() {
        let mut listed: Vec<Value> = serde_json::from_str(LISTED).unwrap();
        listed.push(json!({"name": "deploy", "description": "publicar"}));
        let mut s = st();
        s.initialized(&json!({ "commands": listed }));
        let source = |s: &State, n: &str| {
            s.commands
                .iter()
                .find(|c| c.name == format!("/{n}"))
                .unwrap()
                .source
        };
        assert_eq!(
            source(&s, "deploy"),
            CommandSource::Skill,
            "until init says which are skills"
        );
        // `init` lists the project's skill and not the person's command
        s.learn_skills(&json!(["proj-skill", "android-review"]));
        assert_eq!(
            source(&s, "deploy"),
            CommandSource::Project,
            "no mark, not a skill"
        );
        assert_eq!(
            source(&s, "proj-skill"),
            CommandSource::Project,
            "a mark is not undone"
        );
        assert_eq!(
            source(&s, "commit"),
            CommandSource::Skill,
            "a mark is not undone"
        );
        assert_eq!(source(&s, "android-review"), CommandSource::Skill);
    }

    #[test]
    fn initialize_gives_the_commands_and_the_model_before_any_turn() {
        let mut s = st();
        assert_eq!(s.model, "claude", "nothing is known yet");
        assert_eq!(
            s.commands.len(),
            own_commands().len(),
            "jwn's own are always there"
        );
        assert!(
            s.commands
                .iter()
                .any(|c| c.name == "/rewind" && c.kind == CommandKind::Screen),
            "and all of Claude's, before the engine says anything"
        );
        let e = s.engine_started(None);
        assert_eq!(writes(&e)[0]["request"]["subtype"], "initialize");
        assert_eq!(writes(&e)[0]["request_id"], INIT_REQUEST);
        let answer = |model: &str| {
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": INIT_REQUEST,
                "response": {"commands": [{"name": "context", "builtin": true}],
                    "models": [{"value": "default", "resolvedModel": "claude-sonnet-5-5"},
                               {"value": "opus", "resolvedModel": model}]}}})
        };
        assert!(s.apply(&answer("claude-opus-5-5"), 1).is_empty());
        assert_eq!(s.model, "sonnet-5.5");
        assert!(s.commands.iter().any(|c| c.name == "/context"));
        assert!(
            s.items.is_empty(),
            "an answer to initialize is not a thing to show"
        );
        // a model asked for by its name is shown as what the engine says it is
        let mut s = st();
        s.model = "opus".into();
        s.engine_started(Some("opus"));
        s.apply(&answer("claude-opus-5-5"), 1);
        assert_eq!(s.model, "opus-5.5");
        // one a turn already told stays
        let mut s = st();
        s.model = "haiku-5.5".into();
        s.engine_started(None);
        s.apply(&answer("claude-opus-5-5"), 1);
        assert_eq!(s.model, "haiku-5.5");
    }

    fn answer_of(names: &[&str]) -> Value {
        let commands: Vec<Value> = names.iter().map(|n| json!({"name": n})).collect();
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": INIT_REQUEST,
            "response": {"commands": commands}}})
    }

    fn asks(e: &[Effect]) -> usize {
        writes(e)
            .iter()
            .filter(|w| w["request"]["subtype"] == "initialize")
            .count()
    }

    /// REQ-756: the engine is asked again when the turn of a reload ends, once, before what waits.
    #[test]
    fn the_engine_is_asked_again_when_a_reload_ends_and_before_the_message_that_waits() {
        for line in ["/reload-plugins", "/reload-skills", "  /reload-plugins now"] {
            let mut s = st();
            s.engine_started(None);
            s.apply(&answer_of(&["deploy"]), 1);
            let sent = s.send(line, vec![], 10);
            assert_eq!(
                asks(&sent),
                0,
                "{line}: with the line the answer is the old list"
            );
            assert!(s.send("y ahora esto", vec![], 11).is_empty());
            let e = s.apply(&result(0.0), 20);
            let w = writes(&e);
            assert_eq!(asks(&e), 1, "{line}");
            assert_eq!(
                w[0]["request"]["subtype"], "initialize",
                "before the message that waits"
            );
            assert_eq!(w[0]["request_id"], INIT_REQUEST);
            assert_eq!(w[1]["message"]["content"], "y ahora esto");
            // and the list is the new one when it comes, with the model and the turn untouched
            s.apply(&answer_of(&["deploy", "ponytail:ponytail"]), 21);
            assert!(s.commands.iter().any(|c| c.name == "/ponytail:ponytail"));
            assert_eq!(s.status, ConvStatus::Thinking);
            // the next turn that ends asks nothing
            assert_eq!(asks(&s.apply(&result(0.0), 30)), 0, "{line}");
        }
    }

    #[test]
    fn no_other_line_and_no_new_engine_asks_again() {
        let mut s = st();
        s.engine_started(None);
        for line in ["hola", "/reload", "/context", "no es /reload-plugins"] {
            s.send(line, vec![], 10);
            assert_eq!(asks(&s.apply(&result(0.0), 20)), 0, "{line}");
        }
        // a reload that was running when the engine went away is not asked of the next one twice
        s.send("/reload-plugins", vec![], 30);
        assert_eq!(asks(&s.engine_started(None)), 1);
        assert_eq!(asks(&s.apply(&result(0.0), 40)), 0);
    }

    /// REQ-757: an error, or an answer with no list, leaves the list the conversation had.
    #[test]
    fn an_answer_that_fails_or_has_no_list_leaves_the_commands_and_says_nothing() {
        let mut s = st();
        s.engine_started(None);
        s.apply(&answer_of(&["deploy"]), 1);
        let (before, at) = (s.commands.clone(), s.commands_at);
        let no_list = json!({"type": "control_response", "response": {"subtype": "success",
            "request_id": INIT_REQUEST, "response": {}}});
        let failed = json!({"type": "control_response", "response": {"subtype": "error",
            "request_id": INIT_REQUEST, "error": "boom"}});
        for msg in [no_list, failed] {
            assert!(s.apply(&msg, 2).is_empty());
            assert_eq!(s.commands, before);
            assert_eq!(s.commands_at, at);
            assert!(s.items.is_empty(), "no note, no notice");
        }
    }

    #[test]
    fn the_commands_are_saved_so_a_conversation_that_comes_back_has_them() {
        let mut s = st();
        s.commands = classify_commands(&json!([{"name": "deploy"}]), None);
        let saved = s.saved(5);
        let back = State::restored(saved, "20:00");
        assert!(back.commands.iter().any(|c| c.name == "/deploy"));
        // a file from before a command of jwn's existed gets it
        let mut older = st().saved(5);
        older.commands = vec![ConvCommand {
            name: "/deploy".into(),
            source: CommandSource::Skill,
            kind: CommandKind::Prompt,
            description: None,
            group: None,
        }];
        let names: Vec<String> = State::restored(older, "20:00")
            .commands
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(names.contains(&"/deploy".to_owned()) && names.contains(&"/plan".to_owned()));
        let mut none = st().saved(5);
        none.commands.clear();
        assert_eq!(
            State::restored(none, "20:00").commands.len(),
            own_commands().len(),
            "an older file has none: jwn's own"
        );
    }

    #[test]
    fn an_interrupt_refuses_a_question_waiting_with_the_engines_own_request_id() {
        let mut s = st();
        s.send("hola", vec![], 1);
        s.apply(&json!({"type": "control_request", "request_id": "r9", "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion",
            "input": {"questions": [{"question": "¿A?", "options": [{"label": "x"}, {"label": "y"}], "multiSelect": false},
                                     {"question": "¿B?", "options": [{"label": "z"}], "multiSelect": false}]}, "tool_use_id": "t"}}), 2);
        assert_eq!(s.pending.as_ref().unwrap().ask.id, "r9:0");
        // the second question's id is longer, the engine's request is the same
        let fx = s.answer("r9:0", "1", None, None).unwrap();
        assert!(
            fx.is_empty(),
            "nothing goes to the engine until the last answer"
        );
        assert_eq!(s.pending.as_ref().unwrap().ask.id, "r9:1");
        let e = s.interrupt();
        assert_eq!(
            writes(&e)[0]["response"]["request_id"],
            "r9",
            "{:?}",
            writes(&e)
        );
        assert_eq!(writes(&e)[0]["response"]["response"]["behavior"], "deny");
        assert!(s.pending.is_none());
    }

    #[test]
    fn tasks_the_engine_lists_no_more_have_ended_and_the_list_is_kept_short() {
        let mut s = st();
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "a", "description": "uno", "is_backgrounded": true, "task_type": "local_bash"}), 1);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "b", "description": "dos", "is_backgrounded": false, "task_type": "local_agent"}), 2);
        s.apply(
            &json!({"type": "system", "subtype": "background_tasks_changed", "tasks": []}),
            3,
        );
        let state = |id: &str| s.tasks.iter().find(|t| t.id == id).unwrap().state;
        assert_eq!(
            state("a"),
            TaskState::Done,
            "it ran in the background and the engine lists it no more"
        );
        assert_eq!(
            state("b"),
            TaskState::Run,
            "a subagent in front is not in that list"
        );
        assert!(
            s.items
                .iter()
                .any(|i| matches!(i, Item::Note { text, .. } if text == "Tarea terminó: uno"))
        );
        for n in 0..60 {
            s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": format!("t{n}"), "description": "x", "task_type": "other"}), 4);
        }
        assert_eq!(s.tasks.len(), TASKS_KEPT);
    }

    #[test]
    fn the_checklist_holds_what_todowrite_says_and_a_subagents_steps_are_kept_short() {
        let mut s = st();
        s.send("go", vec![], 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "w", "name": "TodoWrite",
            "input": {"todos": [{"content": "a", "status": "pending"}, {"content": "b", "status": "completed"}, {"nope": 1}]}}]}}), 2);
        assert_eq!(
            s.todos,
            vec![
                Todo {
                    text: "a".into(),
                    done: false
                },
                Todo {
                    text: "b".into(),
                    done: true
                }
            ]
        );
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "ag", "name": "Agent", "input": {"description": "d", "subagent_type": "Explore"}}]}}), 3);
        for n in 0..80 {
            s.apply(&json!({"type": "assistant", "parent_tool_use_id": "ag", "message": {"content": [{"type": "tool_use", "id": format!("n{n}"), "name": "Read", "input": {"file_path": format!("f{n}")}}]}}), 4);
        }
        let Some(Item::Agent { steps, name, .. }) =
            s.items.iter().find(|i| matches!(i, Item::Agent { .. }))
        else {
            panic!("no card");
        };
        assert_eq!(
            (steps.len(), name.as_str(), steps[0].as_str()),
            (STEPS_KEPT, "Explore", "Read f0")
        );
    }

    /// REQ-600: a subagent's card keeps what it was asked (clipped), and its task says which card
    /// shows it, so a client can put the two together.
    #[test]
    fn a_subagents_card_keeps_its_prompt_and_its_task_names_the_card() {
        let mut s = st();
        s.send("go", vec![], 1);
        let long = "x".repeat(PROMPT_CHARS * 2);
        s.apply(&json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "ag", "name": "Agent", "input": {"description": "d", "subagent_type": "Explore", "prompt": "busca verify"}},
            {"type": "tool_use", "id": "ag2", "name": "Agent", "input": {"description": "d2", "subagent_type": "Explore", "prompt": long}},
            {"type": "tool_use", "id": "ag3", "name": "Agent", "input": {"description": "d3", "subagent_type": "Explore", "prompt": "  "}}]}}), 2);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t1", "tool_use_id": "ag", "description": "d", "is_backgrounded": false, "task_type": "local_agent"}), 3);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t2", "description": "sleep", "is_backgrounded": true, "task_type": "local_bash"}), 3);
        let prompt = |id: &str| {
            s.items.iter().find_map(|i| match i {
                Item::Agent { id: a, prompt, .. } if a == id => Some(prompt.clone()),
                _ => None,
            })
        };
        assert_eq!(prompt("ag"), Some(Some("busca verify".to_owned())));
        assert!(prompt("ag2").unwrap().unwrap().chars().count() <= PROMPT_CHARS);
        assert_eq!(prompt("ag3"), Some(None), "a blank prompt is none");
        let item = |id: &str| s.tasks.iter().find(|t| t.id == id).unwrap().item.clone();
        assert_eq!(item("t1").as_deref(), Some("ag"));
        assert_eq!(
            item("t2"),
            None,
            "a task with no tool that started it has no card"
        );
    }

    #[test]
    fn a_subagents_model_is_not_replaced_by_the_synthetic_one_claude_code_writes_itself() {
        let mut s = st();
        s.send("go", vec![], 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "ag", "name": "Agent", "input": {"description": "d", "subagent_type": "Explore", "prompt": "p"}}]}}), 2);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t1", "tool_use_id": "ag", "description": "d", "is_backgrounded": false, "task_type": "local_agent"}), 3);
        let model = |s: &State| {
            s.items
                .iter()
                .find_map(|i| match i {
                    Item::Agent { id, model, .. } if id == "ag" => Some(model.clone()),
                    _ => None,
                })
                .unwrap()
        };
        s.apply(&json!({"type": "assistant", "parent_tool_use_id": "ag", "message": {"model": "claude-haiku-5-5", "content": [{"type": "text", "text": "mirando"}]}}), 4);
        assert_eq!(model(&s), Some("haiku-5.5".to_owned()));
        s.apply(&json!({"type": "assistant", "parent_tool_use_id": "ag", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "No response requested."}]}}), 5);
        assert_eq!(model(&s), Some("haiku-5.5".to_owned()));
    }

    #[test]
    fn a_usage_the_engine_leaves_out_keeps_the_figure_it_said_before() {
        let mut s = st();
        s.send("go", vec![], 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "ag", "name": "Agent", "input": {"description": "d", "subagent_type": "Explore", "prompt": "p"}}]}}), 2);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t1", "tool_use_id": "ag", "description": "d", "is_backgrounded": false, "task_type": "local_agent"}), 3);
        s.apply(&json!({"type": "system", "subtype": "task_progress", "task_id": "t1", "tool_use_id": "ag", "description": "d", "usage": {"tool_uses": 3, "total_tokens": 100, "duration_ms": 500}, "last_tool_name": "Read"}), 4);
        s.apply(&json!({"type": "system", "subtype": "task_notification", "task_id": "t1", "tool_use_id": "ag", "status": "completed", "summary": "ok", "usage": {"total_tokens": 150}}), 5);
        let card = s
            .items
            .iter()
            .find_map(|i| match i {
                Item::Agent {
                    id,
                    tools,
                    tokens,
                    ms,
                    ..
                } if id == "ag" => Some((*tools, *tokens, *ms)),
                _ => None,
            })
            .unwrap();
        assert_eq!(card, (3, 150, 500));
    }

    #[test]
    fn a_turn_that_ends_cuts_the_subagent_that_ran_in_front_but_not_the_task_in_the_background_and_a_dead_engine_cuts_both()
     {
        let mut s = st();
        s.send("go", vec![], 1);
        s.apply(&json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "ag", "name": "Task", "input": {"description": "d", "subagent_type": "Explore"}}]}}), 2);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t1", "tool_use_id": "ag", "description": "d", "is_backgrounded": false, "task_type": "local_agent"}), 3);
        s.apply(&json!({"type": "system", "subtype": "task_started", "task_id": "t2", "description": "sleep", "is_backgrounded": true, "task_type": "local_bash"}), 3);
        s.apply(&result(0.0), 4);
        let card = |s: &State| {
            s.items
                .iter()
                .find_map(|i| {
                    if let Item::Agent { state, .. } = i {
                        Some(*state)
                    } else {
                        None
                    }
                })
                .unwrap()
        };
        assert_eq!(
            card(&s),
            ToolState::Failed,
            "it ran in front and the turn ended without it"
        );
        let task = |s: &State, id: &str| s.tasks.iter().find(|t| t.id == id).unwrap().state;
        assert_eq!(
            (task(&s, "t1"), task(&s, "t2")),
            (TaskState::Stopped, TaskState::Run)
        );
        s.engine_gone("el motor se cerró");
        assert_eq!(
            task(&s, "t2"),
            TaskState::Stopped,
            "the engine that ran it is gone"
        );
        assert!(s.task_links.is_empty());
    }

    fn line_at(at: u64, item: Item) -> crate::transcript::Stamped {
        crate::transcript::Stamped { at, item }
    }

    fn said(text: &str) -> Item {
        Item::Text {
            text: text.into(),
            live: false,
            blocks: None,
        }
    }

    fn note(text: &str) -> Item {
        Item::Note {
            text: text.into(),
            tone: None,
        }
    }

    fn texts(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Item::Text { text, .. } | Item::Note { text, .. } => text.clone(),
                Item::User { text, .. } => format!("user:{text}"),
                Item::Out { .. } => "out".into(),
                Item::Result { .. } => "result".into(),
                Item::Agent { id, .. } => format!("card:{id}"),
                Item::Tool { id, .. } => format!("tool:{id}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn tool(id: &str) -> Item {
        Item::Tool {
            id: id.into(),
            name: "Task".into(),
            arg: String::new(),
            state: ToolState::Ok,
            summary: None,
            output: Vec::new(),
            diff: Vec::new(),
            url: None,
            by: None,
            lost: false,
        }
    }

    fn card(id: &str) -> Item {
        Item::Agent {
            id: id.into(),
            name: "Explore".into(),
            description: String::new(),
            state: ToolState::Ok,
            tools: 0,
            tokens: 0,
            ms: 0,
            last: None,
            steps: Vec::new(),
            prompt: None,
            model: None,
        }
    }

    /// REQ-544: the items that are jwn's own go among the transcript's lines by their time, and a
    /// subagent's card takes the place of the tool that started it.
    #[test]
    fn the_items_of_jwn_go_among_the_lines_of_the_transcript_by_their_time() {
        let base = [
            line_at(
                100,
                Item::User {
                    text: "hola".into(),
                    images: 0,
                },
            ),
            line_at(200, said("uno")),
            line_at(300, tool("a")),
            line_at(400, said("dos")),
        ];
        let kept = [
            Kept {
                at: 450,
                item: Item::Result {
                    ms: 1,
                    tools: 1,
                    cost: 2,
                },
            },
            Kept {
                at: 150,
                item: note("nota"),
            },
            Kept {
                at: 310,
                item: card("a"),
            },
            Kept {
                at: 50,
                item: Item::Out {
                    lines: vec!["x".into()],
                },
            },
        ];
        let (items, stamps) = merge(&base, &kept);
        assert_eq!(
            texts(&items),
            ["out", "user:hola", "nota", "uno", "card:a", "dos", "result"]
        );
        assert_eq!(stamps.len(), items.len());
        // Nothing of the transcript's own is lost when there is nothing of jwn's.
        assert_eq!(merge(&base, &[]).0.len(), 4);
        // A card whose tool was not read stays where its time puts it.
        let (items, _) = merge(
            &base[..2],
            &[Kept {
                at: 150,
                item: card("zz"),
            }],
        );
        assert_eq!(texts(&items), ["user:hola", "card:zz", "uno"]);
    }

    #[test]
    fn a_conversation_that_follows_a_transcript_takes_its_items_state_and_keeps_what_is_its_own() {
        let mut t = crate::transcript::Parser::default();
        t.items = vec![line_at(100, said("uno")), line_at(300, said("dos"))];
        t.mode = Some(Mode::Plan);
        t.model = Some("claude-opus-5-5".into());
        t.effort = Some("xhigh".into());
        let mut s = st();
        s.has_transcript = true;
        s.push(note("mía"));
        s.follow(&t);
        // The note was made now, after the lines, so it is last; the mode, model and effort are the transcript's.
        assert_eq!(texts(&s.items), ["uno", "dos", "mía"]);
        assert_eq!(
            (s.mode, s.model.as_str(), s.effort.as_str()),
            (Mode::Plan, "opus-5.5", "xhigh")
        );
        // What is added later is placed again and the note stays.
        t.items.push(line_at(u64::MAX, said("tres")));
        s.follow(&t);
        assert_eq!(texts(&s.items), ["uno", "dos", "mía", "tres"]);
        // Only what the transcript does not hold is saved of it, and the chat when there is none.
        let saved = s.saved(1);
        assert_eq!(saved.kept.len(), 1);
        assert!(s.leaves_chat_to_transcript());
        assert_eq!(
            saved.items.len(),
            4,
            "the caller leaves the chat out when the transcript is there"
        );
        // Many lines: the node keeps the last ones and counts the rest in `first`.
        t.items = (0..KEEP + 5)
            .map(|i| line_at(i as u64, said(&i.to_string())))
            .collect();
        s.follow(&t);
        assert_eq!(
            (s.items.len(), s.first),
            (KEEP, 6),
            "5 lines and the note beyond what it keeps, plus none lost"
        );
    }

    #[test]
    fn a_conversation_saved_with_its_own_items_keeps_them_and_does_not_follow() {
        let mut old = st().saved(5);
        old.items = vec![said("vieja")];
        old.has_transcript = true;
        let s = State::restored(old, "20:00");
        assert!(s.legacy() && !s.leaves_chat_to_transcript());
        assert!(
            s.saved(6).kept.is_empty(),
            "nothing of its is split from its items"
        );
        let mut fresh = st().saved(5);
        fresh.has_transcript = true;
        fresh.kept = vec![Kept {
            at: 9,
            item: note("n"),
        }];
        let s = State::restored(fresh, "20:00");
        assert!(!s.legacy() && s.leaves_chat_to_transcript());
    }

    #[test]
    fn what_jwn_kept_is_shown_when_the_transcript_is_not_there() {
        let mut saved = st().saved(5);
        saved.has_transcript = true;
        saved.kept = vec![
            Kept {
                at: 20,
                item: note("segunda"),
            },
            Kept {
                at: 10,
                item: note("primera"),
            },
        ];
        let mut s = State::restored(saved, "20:00");
        assert!(
            texts(&s.items).iter().all(|t| t != "primera"),
            "kept items wait for the follow"
        );
        s.follow(&crate::transcript::Parser::default());
        let shown = texts(&s.items);
        assert_eq!(&shown[..2], ["primera", "segunda"], "{shown:?}");
        assert!(
            s.saved(6).kept.len() >= 2,
            "and they are saved again, not lost"
        );
    }

    /// Following a long transcript re-places only the end of it.
    #[test]
    fn following_a_long_transcript_takes_the_end_of_it_only() {
        let mut t = crate::transcript::Parser::default();
        t.items = (0..50_000).map(|i| line_at(i, said("x"))).collect();
        let mut s = st();
        let started = std::time::Instant::now();
        for _ in 0..20 {
            s.follow(&t);
        }
        assert_eq!(s.items.len(), KEEP);
        assert_eq!(s.first as usize, 50_000 - KEEP);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    /// REQ-567: when the transcript grows, `first` grows by the items that leave the head of the
    /// window, with the items of jwn's own and the card of a subagent among them, and the window
    /// then starts with the item that was the one after those: what a client that holds the older
    /// items relies on to keep them in a row.
    #[test]
    fn first_grows_by_what_leaves_the_head_of_the_window_with_own_items_and_cards_in_the_merge() {
        let mut t = crate::transcript::Parser::default();
        t.items = (0..350)
            .map(|i| line_at(i * 10, said(&format!("t{i}"))))
            .collect();
        let mut s = st();
        s.kept_saved = vec![
            Kept {
                at: 1_005,
                item: note("mía en medio"),
            },
            Kept {
                at: 2_005,
                item: Item::Agent {
                    id: "tool-id".into(),
                    name: "Explore".into(),
                    description: "x".into(),
                    state: ToolState::Ok,
                    tools: 1,
                    tokens: 2,
                    ms: 3,
                    last: None,
                    steps: Vec::new(),
                    prompt: None,
                    model: None,
                },
            },
        ];
        t.items[200] = line_at(
            2_000,
            Item::Tool {
                id: "tool-id".into(),
                name: "Task".into(),
                arg: "x".into(),
                state: ToolState::Ok,
                summary: None,
                output: Vec::new(),
                diff: Vec::new(),
                url: None,
                by: None,
                lost: false,
            },
        );
        s.follow(&t);
        assert!(
            s.items.iter().any(|i| matches!(i, Item::Agent { .. })),
            "the card took the place of its tool"
        );
        for step in [1, 7, 40, 3] {
            let (before_first, before_items) = (s.first, s.items.clone());
            let len = t.items.len();
            t.items.extend(
                (len..len + step).map(|i| line_at(i as u64 * 10 + 5_000, said(&format!("t{i}")))),
            );
            s.follow(&t);
            let left = (s.first - before_first) as usize;
            assert_eq!(s.items.len(), KEEP);
            assert!(left <= before_items.len());
            assert_eq!(left, step, "an item left for each one that came");
            assert_eq!(
                s.items[0], before_items[left],
                "the window starts with the item that came after the ones that left"
            );
        }
    }

    /// REQ-567: the node says where its first line of the transcript is, and not the time of an
    /// item of jwn's own that is older than that.
    #[test]
    fn the_time_older_asks_from_is_the_first_item_of_the_transcript_and_not_an_own_one() {
        let mut t = crate::transcript::Parser::default();
        t.items = vec![line_at(5_000, said("a")), line_at(6_000, said("b"))];
        let mut s = st();
        s.has_transcript = true;
        s.kept_saved = vec![Kept {
            at: 1_000,
            item: note("mía, de antes"),
        }];
        s.follow(&t);
        assert!(
            matches!(s.items[0], Item::Note { .. }),
            "the own item heads the window"
        );
        assert_eq!(s.older_cursor(), Some(5_000));
        s.has_transcript = false;
        assert_eq!(
            s.older_cursor(),
            None,
            "no transcript, nothing older to read"
        );
    }

    /// REQ-549: what another `claude` queued is shown while its session is followed, and goes when
    /// this conversation's own engine starts.
    #[test]
    fn what_another_claude_queued_is_shown_until_the_own_engine_starts() {
        let t = crate::transcript::parse(
            r#"{"type":"queue-operation","operation":"enqueue","content":"y esto"}"#,
        );
        let mut s = st();
        s.follow(&t);
        assert_eq!(s.followed_queue, ["y esto"]);
        let _ = s.engine_started(None);
        assert!(s.followed_queue.is_empty());
    }
}
