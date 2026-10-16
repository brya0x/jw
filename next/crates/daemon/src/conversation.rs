//! A conversation: what the engine says, turned into the state a `conversation` node shows
//! (spec: *The conversation*, REQ-371 and following; what the engine says, `docs/claude-engine.md`).
//!
//! Everything here is a function of the state and one message and returns what the daemon must
//! do about it (`Effect`), so it is tested with recorded messages and no process. The processes,
//! the threads and the lock are `engine.rs`'s.

use std::collections::VecDeque;

use contract::conversation::{
    Accent, Artifact, Ask, AskKind, AskOption, CommandKind, CommandSource, ConvCommand, ConvStatus,
    Conversation, DiffLine, DiffSign, DividerKind, Image, Item, Limits, Mode, Task, Todo, Tone,
    ToolState,
};
use contract::{AttentionKind, Head, Id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How many items a node keeps (REQ-392).
pub const KEEP: usize = 300;
/// The most a saved conversation takes on disk.
pub const MAX_SAVED: usize = 2 * 1024 * 1024;
const OUTPUT_LINES: usize = 40;
const ARG_CHARS: usize = 200;
const LINE_CHARS: usize = 300;

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

/// What the engine answers «isn't available in this environment»: jwn's own lists stand in for
/// them (N83b), so until then they are not offered.
const UNAVAILABLE: [&str; 10] = [
    "help",
    "status",
    "resume",
    "tasks",
    "artifacts",
    "skills",
    "permissions",
    "memory",
    "hooks",
    "rewind",
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
    OWN.iter()
        .map(|(name, description)| ConvCommand {
            name: format!("/{name}"),
            source: CommandSource::Jwn,
            kind: CommandKind::Local,
            description: Some((*description).to_owned()),
        })
        .collect()
}

/// The `/` list: jwn's own, then what the engine lists, each tagged by where it is defined
/// (REQ-417). `skills` are the names the engine's `init` calls skills, when it has said them.
pub fn classify_commands(listed: &Value, skills: Option<&[&str]>) -> Vec<ConvCommand> {
    let mut out = own_commands();
    for c in listed.as_array().into_iter().flatten() {
        let Some(name) = c["name"].as_str().filter(|n| !n.is_empty()) else {
            continue;
        };
        if OWN.iter().any(|(own, _)| *own == name) || UNAVAILABLE.contains(&name) {
            continue;
        }
        let (source, kind) = if c["builtin"].as_bool().unwrap_or(false) {
            let kind = if SCREEN.contains(&name) {
                CommandKind::Screen
            } else if PROMPTS.contains(&name) {
                CommandKind::Prompt
            } else {
                CommandKind::Text
            };
            (CommandSource::Claude, kind)
        } else if name.contains(':') {
            (CommandSource::Plugin, CommandKind::Prompt)
        } else if skills.is_some_and(|s| !s.contains(&name)) {
            (CommandSource::Project, CommandKind::Prompt)
        } else {
            (CommandSource::Skill, CommandKind::Prompt)
        };
        out.push(ConvCommand {
            name: format!("/{name}"),
            source,
            kind,
            description: c["description"]
                .as_str()
                .filter(|d| !d.is_empty())
                .map(|d| clip(d, 160)),
        });
    }
    out
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
        let long = match flag {
            "--settings" | "--agent" => flag,
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
}

/// What a local command of the engine (`/model`, `/effort`, `/rename`) changes once it answers.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Model(String),
    Effort(String),
    None,
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

/// The question that waits, with what it needs to be answered.
#[derive(Debug, Clone)]
struct Pending {
    ask: Ask,
    input: Value,
    tool_use_id: Option<String>,
    session_rule: Option<String>,
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
    /// The engine's id for the conversation; always known, since the daemon chooses it.
    pub session: String,
    /// The engine has written its transcript, so `--resume` can read it.
    pub has_transcript: bool,
    pub first: u32,
    pub items: Vec<Item>,
    /// Millionths of a dollar.
    pub cost: u64,

    pub status: ConvStatus,
    pub since: Option<u64>,
    pending: Option<Pending>,
    queue: VecDeque<Queued>,
    pub tasks: Vec<Task>,
    pub todos: Vec<Todo>,
    pub artifacts: Vec<Artifact>,
    pub commands: Vec<ConvCommand>,
    /// What the engine runs with when asked (`--model`): its name for the model, until its own answer says which one that is.
    wanted_model: Option<String>,
    /// The names the engine's `init` calls skills: they tell a skill from a project's command.
    skills: Option<Vec<String>>,
    pub limits: Option<Limits>,
    /// Allowed for this session: what the person said yes to for the rest of the conversation.
    allowed: Vec<String>,
    inflight: Option<Inflight>,
    /// A turn the engine started by itself (a monitor's line, a task that ended).
    auto: bool,
    interrupting: bool,
    tools_in_turn: u32,
    turn_started: u64,
    /// The engine's total cost so far (its process), to know what one turn cost.
    last_total_micro: u64,
    live_texts: VecDeque<usize>,
    /// What `!command` lines printed, sent with the next message.
    shell_context: Vec<String>,
    pending_mode: Option<Mode>,
    next_request: u64,
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
    pub session: String,
    pub has_transcript: bool,
    pub first: u32,
    pub items: Vec<Item>,
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
}

pub const SAVED_VERSION: u32 = 1;

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
            items: Vec::new(),
            cost: 0,
            status: ConvStatus::Idle,
            since: None,
            pending: None,
            queue: VecDeque::new(),
            tasks: Vec::new(),
            todos: Vec::new(),
            artifacts: Vec::new(),
            commands: own_commands(),
            wanted_model: None,
            skills: None,
            limits: None,
            allowed: Vec::new(),
            inflight: None,
            auto: false,
            interrupting: false,
            tools_in_turn: 0,
            turn_started: 0,
            last_total_micro: 0,
            live_texts: VecDeque::new(),
            shell_context: Vec::new(),
            pending_mode: None,
            next_request: 1,
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
        st.has_transcript = saved.has_transcript;
        st.first = saved.first;
        st.items = saved.items;
        st.cost = saved.cost;
        if !saved.commands.is_empty() {
            st.commands = saved.commands;
        }
        st.status = ConvStatus::Dead;
        st.push(Item::Divider {
            text: format!("el daemon se reinició · {at}"),
            kind: DividerKind::Restart,
        });
        st
    }

    /// A conversation the person resumes by its id, with nothing of ours saved: the engine's own
    /// transcript is what it comes back to.
    pub fn resumed(session: String) -> State {
        let mut st = State::new(session);
        st.has_transcript = true;
        st.status = ConvStatus::Dead;
        st.push(Item::Divider {
            text: "conversación retomada".into(),
            kind: DividerKind::Resume,
        });
        st
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
            session: self.session.clone(),
            has_transcript: self.has_transcript,
            first: self.first,
            items: self.items.clone(),
            cost: self.cost,
            model_arg: None,
            effort_arg: None,
            commands: self.commands.clone(),
        }
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
            items: self.items.clone(),
            ask: self.pending.as_ref().map(|p| p.ask.clone()),
            queue: self.queue.iter().map(Queued::shown).collect(),
            tasks: self.tasks.clone(),
            todos: self.todos.clone(),
            artifacts: self.artifacts.clone(),
            commands: self.commands.clone(),
            cost: self.cost,
            limits: self.limits.clone(),
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
        self.trim();
    }

    /// Keep the last `KEEP` items; `first` counts the ones dropped.
    fn trim(&mut self) {
        if self.items.len() <= KEEP {
            return;
        }
        let drop = self.items.len() - KEEP;
        self.items.drain(..drop);
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
        if self.busy() || self.pending.is_some() || self.pending_mode.is_some() {
            self.queue.push_back(Queued::Text {
                text: text.to_owned(),
                images,
            });
            return Vec::new();
        }
        self.start_text(text, images, now)
    }

    fn start_text(&mut self, text: &str, images: Vec<Image>, now: u64) -> Vec<Effect> {
        if !self.named {
            self.name = clip(&text.split_whitespace().collect::<Vec<_>>().join(" "), 28);
            self.named = true;
        }
        let count = images.len() as u32;
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
        vec![Effect::Write(user_line(&body, &images))]
    }

    fn begin_turn(&mut self, now: u64) {
        self.status = ConvStatus::Thinking;
        self.since = Some(now);
        self.turn_started = now;
        self.tools_in_turn = 0;
        self.interrupting = false;
    }

    /// A line of the engine's own that changes what it runs with: sent when nothing is in flight.
    fn local(&mut self, line: String, change: Change) -> Vec<Effect> {
        if self.busy() || self.pending.is_some() {
            self.queue.push_back(Queued::Local { line, change });
            return Vec::new();
        }
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
        if !p.ask.options.iter().any(|o| o.key == key) {
            return Err(format!("{key} is not one of its answers"));
        }
        let p = self.pending.take().expect("checked above");
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
                    &p.ask.id, false, &p.input, &message,
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
                    &p.ask.id, true, &p.input, "",
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

    /// Stop what runs: a waiting question is refused first, so the engine is not left asking.
    pub fn interrupt(&mut self) -> Vec<Effect> {
        if !self.busy() || self.interrupting {
            return Vec::new();
        }
        self.interrupting = true;
        let mut out = Vec::new();
        if let Some(p) = self.pending.take() {
            out.push(Effect::ClearAsk);
            out.push(Effect::Write(permission_reply(
                &p.ask.id,
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
        if let Some(m) = mode.filter(|m| *m != self.mode) {
            if self.busy() || self.pending.is_some() {
                self.pending_mode = Some(m);
            } else {
                out.push(self.mode_request(m));
            }
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

    fn system(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        match msg["subtype"].as_str().unwrap_or("") {
            "init" => {
                if let Some(m) = msg["model"].as_str() {
                    self.model = short_model(m);
                }
                if let Some(m) = msg["permissionMode"].as_str().and_then(Mode::from_flag) {
                    self.mode = m;
                }
                if msg["skills"].is_array() {
                    self.learn_skills(&msg["skills"]);
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
                if let Some(m) = msg["permissionMode"].as_str().and_then(Mode::from_flag)
                    && m != self.mode
                {
                    self.mode = m;
                    self.note(format!("Modo: {}", mode_name(m)), None);
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
            }
            "content_block_start" => {
                if ev["content_block"]["type"] == "text"
                    && !matches!(self.inflight, Some(Inflight::Local(_)))
                {
                    self.status = ConvStatus::Streaming;
                    self.push(Item::Text {
                        text: String::new(),
                        live: true,
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

    fn assistant(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        self.begin_auto(now);
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
                            if let Some(Item::Text { text, live }) = self.items.get_mut(at) {
                                *text = full;
                                *live = false;
                            }
                        }
                        None => self.push(Item::Text {
                            text: full,
                            live: false,
                        }),
                    }
                }
                "tool_use" => {
                    let name = b["name"].as_str().unwrap_or("").to_owned();
                    if name == "ToolSearch" {
                        continue;
                    }
                    let id = b["id"].as_str().unwrap_or("").to_owned();
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
        Vec::new()
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
            let failed = b["is_error"].as_bool().unwrap_or(false);
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
        };
        if let Some(tid) = req["tool_use_id"].as_str() {
            self.set_tool(tid, |t| t.0 = ToolState::Ask);
        }
        self.pending = Some(Pending {
            ask,
            input,
            tool_use_id: req["tool_use_id"].as_str().map(str::to_owned),
            session_rule: suggested.then_some(rule),
        });
        self.status = ConvStatus::Asking;
        vec![Effect::Raise(AttentionKind::Permission)]
    }

    fn result(&mut self, msg: &Value, now: u64) -> Vec<Effect> {
        let mut out = Vec::new();
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
        // A tool that never answered was cut with the turn.
        for id in self.running_tools() {
            self.set_tool(&id, |t| {
                t.0 = ToolState::Denied;
                t.1 = Some("Interrumpido".into());
            });
        }
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
        if let Some(mode) = self.pending_mode.take() {
            let request = self.mode_request(mode);
            out.push(request);
        }
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
        self.first = 0;
        self.live_texts.clear();
        self.has_transcript = false;
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

    /// The engine's process ended while the conversation was open.
    pub fn engine_gone(&mut self, reason: &str) -> Vec<Effect> {
        let had_ask = self.pending.take().is_some();
        self.inflight = None;
        self.auto = false;
        self.interrupting = false;
        self.finish_live();
        for id in self.running_tools() {
            self.set_tool(&id, |t| {
                t.0 = ToolState::Failed;
                t.1 = Some("El motor se detuvo".into());
            });
        }
        self.status = ConvStatus::Dead;
        self.since = None;
        self.note(reason.to_owned(), Some(Tone::Error));
        if had_ask {
            vec![Effect::ClearAsk]
        } else {
            Vec::new()
        }
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
        self.wanted_model = wanted.map(str::to_owned);
        if self.status == ConvStatus::Resuming || self.status == ConvStatus::Dead {
            self.status = ConvStatus::Idle;
        }
        vec![Effect::Write(json!({
            "type": "control_request",
            "request_id": INIT_REQUEST,
            "request": {"subtype": "initialize"},
        }))]
    }

    /// The engine's answer to `initialize`: the commands, and the model it runs with when no turn
    /// has said it yet.
    fn initialized(&mut self, resp: &Value) {
        let skills: Option<Vec<&str>> = self
            .skills
            .as_ref()
            .map(|s| s.iter().map(String::as_str).collect());
        self.commands = classify_commands(&resp["commands"], skills.as_deref());
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

    /// The skills `init` lists tell the commands that are skills from those of the project.
    fn learn_skills(&mut self, listed: &Value) {
        let names: Vec<String> = listed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(|n| n.trim_start_matches('/').to_owned()))
            .collect();
        for c in &mut self.commands {
            if matches!(c.source, CommandSource::Skill | CommandSource::Project) {
                let name = c.name.trim_start_matches('/');
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

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::AcceptEdits => "accept edits",
        other => other.flag(),
    }
}

fn clip(s: &str, n: usize) -> String {
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
fn content_text(c: &Value) -> String {
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
fn edit_diff(name: &str, input: &Value) -> Vec<DiffLine> {
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

fn summary_of(tool: &str, lines: &[String], result: &Value, failed: bool) -> String {
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
            matches!(s.items.last(), Some(Item::Text { text, live: true }) if text == "Voy a mirar el test.")
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
            s.items
                .iter()
                .any(|i| matches!(i, Item::Note { text, .. } if text == "Modo: plan"))
        );
        // while a turn runs the change waits for it to end
        s.send("go", vec![], 2);
        assert!(s.set(None, None, None, None, Some(Mode::Auto)).is_empty());
        let e = s.apply(&result(0.0), 3);
        assert_eq!(writes(&e)[0]["request"]["mode"], "auto");
        // a refusal of the engine is told
        s.apply(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": "jwn-1", "error": "no se puede"}}), 4);
        assert!(s.items.iter().any(
            |i| matches!(i, Item::Note { text, tone: Some(Tone::Error) } if text == "no se puede")
        ));
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
            {"name": "help", "builtin": true},
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
        assert!(
            find("/help").is_none(),
            "the lists of jwn stand in for it later"
        );
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

    #[test]
    fn initialize_gives_the_commands_and_the_model_before_any_turn() {
        let mut s = st();
        assert_eq!(s.model, "claude", "nothing is known yet");
        assert_eq!(
            s.commands.len(),
            own_commands().len(),
            "jwn's own are always there"
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

    #[test]
    fn the_commands_are_saved_so_a_conversation_that_comes_back_has_them() {
        let mut s = st();
        s.commands = classify_commands(&json!([{"name": "deploy"}]), None);
        let saved = s.saved(5);
        let back = State::restored(saved, "20:00");
        assert!(back.commands.iter().any(|c| c.name == "/deploy"));
        let mut none = st().saved(5);
        none.commands.clear();
        assert_eq!(
            State::restored(none, "20:00").commands.len(),
            own_commands().len(),
            "an older file has none: jwn's own"
        );
    }
}
