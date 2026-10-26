//! The commands an agent in a pane uses: what there is (`ls`, `pane ls`), what a pane shows
//! (`read`), what to do (`prompt`, `worktree`, `pane open`, `pane close`, `group`), the task list of the
//! workspace (`tasks`, N180) and the skill that
//! teaches an agent all this (`skill`). They are clients of the daemon like the terminal
//! client, over nodes and commands that exist (spec N80, REQ-353 to REQ-370).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use contract::conversation::{ConvStatus, Conversation, Item};
use contract::{
    Attention, AttentionKind, Block, Blocks, Cmd, Group, Pane, Session, SplitDir, Workspace,
    WorkspaceSummary,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wire::Client;

/// How long a node is waited for, an agent to settle or to appear, and the daemon's replies.
const NODE_WAIT: Duration = Duration::from_secs(10);
const AGENT_WAIT: Duration = Duration::from_secs(40);
const SETTLED: Duration = Duration::from_secs(1);
const REPLY_WAIT: Duration = Duration::from_secs(60);
/// Between the paste and the Enter: an agent's input box takes the paste as text first.
const ENTER_AFTER: Duration = Duration::from_millis(200);
const DEFAULT_LINES: usize = 40;
/// How long `prompt --wait` waits for a turn to end (REQ-454).
const TURN_WAIT: Duration = Duration::from_secs(600);
/// How long `worktree --task` waits for the new agent's conversation before it types the task.
const CONVERSATION_WAIT: Duration = Duration::from_secs(10);

mod artifact;
mod tasks;

const SKILL: &str = include_str!("../skill/SKILL.md");
const TASKS_SKILL: &str = include_str!("../skill/TASKS.md");

/// Why a command ended.
#[derive(Debug, PartialEq, Eq)]
enum Fail {
    /// A use the command does not accept: exit 2 with the usage.
    Usage(String),
    /// Anything else: exit 1.
    Error(String),
    /// Nothing to do: the message goes to stderr and the exit is 3 (REQ-940).
    Idle(String),
    /// The agent waits for an answer: what it asks goes to stdout and the exit is 3 (REQ-454).
    Waiting(String),
    /// The person asked for changes to an artifact: what to change goes to stdout and the exit is 4 (REQ-772).
    Changes(String),
}

type Out<T = ()> = Result<T, Fail>;

fn fail<T>(message: impl Into<String>) -> Out<T> {
    Err(Fail::Error(message.into()))
}

/// How long an agent is waited for; `<PREFIX>_AGENT_WAIT_MS` shortens it for tests.
fn agent_wait() -> Duration {
    std::env::var(identity::env_var("AGENT_WAIT_MS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(AGENT_WAIT, Duration::from_millis)
}

fn bin() -> &'static str {
    &identity::get().name
}

fn usage_of(cmd: &str) -> String {
    let b = bin();
    match cmd {
        "ls" => format!("usage: {b} ls [--json]"),
        "read" => format!("usage: {b} read <workspace> [--pane <pane>] [--lines N] [--blocks]"),
        "prompt" => format!(
            "usage: {b} prompt <workspace> [--pane <pane>] [--tag] [--now] [--wait [--timeout SECONDS]] [--] <text>"
        ),
        "worktree" => format!(
            "usage: {b} worktree <name> [--in <repo>] [--branch <branch>] [--from <ref>] [--open shell|claude] [--task <text>]"
        ),
        "group" => format!(
            "usage: {b} group ls [--json]\n       {b} group new <name>\n       {b} group rename <group> <name>\n       {b} group rm <group>\n       {b} group move <workspace> [<group>]"
        ),
        "chat" => format!(
            "usage: {b} chat new [--json]\n       {b} chat archive <chat>\n       {b} chat resume <chat>\n       {b} chat rm <chat>\n       {b} chat promote <chat> <workspace> [--no-summary]"
        ),
        "artifact" => format!(
            "usage: {b} artifact publish <file> [--name N] [--title T] [--kind prototype|document|report|spec] [--wait [--timeout SECONDS]]\n       {b} artifact ls [<workspace>] [--json]\n       {b} artifact wait <id> [--timeout SECONDS]\n       {b} artifact rm <id>"
        ),
        "pane" => format!(
            "usage: {b} pane ls <workspace> [--json]\n       {b} pane open <workspace> [--split right|down] [--of <pane>] [--] [<program words>...]\n       {b} pane close <workspace> <pane> [--force]"
        ),
        "tasks" => tasks::usage(),
        _ => format!("usage: {b} skill [install]"),
    }
}

/// Run the subcommand `cmd` with `args`, printing what it has to say; the exit code is 0, 1 for
/// an error, 2 for a use it does not accept (REQ-354) and 3 for an agent that waits for an answer
/// (REQ-454).
pub fn run(cmd: &str, args: &[&str]) -> ExitCode {
    let done = match (cmd, args) {
        ("ls", rest) => ls(rest),
        ("read", rest) => read(rest),
        ("prompt", rest) => prompt(rest),
        ("worktree", rest) => worktree(rest),
        ("artifact", rest) => artifact::run(rest),
        ("tasks", rest) => tasks::run(rest),
        ("group", ["ls", rest @ ..]) => group_ls(rest),
        ("group", ["new", rest @ ..]) => group_new(rest),
        ("group", ["rename", rest @ ..]) => group_rename(rest),
        ("group", ["rm", rest @ ..]) => group_rm(rest),
        ("group", ["move", rest @ ..]) => group_move(rest),
        ("chat", ["new", rest @ ..]) => chat_new(rest),
        ("chat", ["archive", rest @ ..]) => chat_archive(rest),
        ("chat", ["resume", rest @ ..]) => chat_resume(rest),
        ("chat", ["rm", rest @ ..]) => chat_rm(rest),
        ("chat", ["promote", rest @ ..]) => chat_promote(rest),
        ("pane", ["ls", rest @ ..]) => pane_ls(rest),
        ("pane", ["open", rest @ ..]) => pane_open(rest),
        ("pane", ["close", rest @ ..]) => pane_close(rest),
        ("skill", []) => {
            print!("{}", skill_text());
            Ok(())
        }
        ("skill", ["install"]) => skill_install(),
        _ => Err(Fail::Usage(String::new())),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Usage(why)) => {
            if !why.is_empty() {
                eprintln!("{} {cmd}: {why}", bin());
            }
            eprintln!("{}", usage_of(cmd));
            ExitCode::from(2)
        }
        Err(Fail::Error(why)) => {
            eprintln!("{} {cmd}: {why}", bin());
            ExitCode::FAILURE
        }
        Err(Fail::Idle(why)) => {
            eprintln!("{} {cmd}: {why}", bin());
            ExitCode::from(3)
        }
        Err(Fail::Waiting(asks)) => {
            println!("{asks}");
            ExitCode::from(3)
        }
        Err(Fail::Changes(what)) => {
            println!("changes\n{what}");
            ExitCode::from(4)
        }
    }
}

/// The arguments of a command: the flags it was given, what is not a flag, and what follows `--`.
#[derive(Default, Debug)]
struct Args {
    flags: Vec<(String, Option<String>)>,
    pos: Vec<String>,
    rest: Vec<String>,
}

impl Args {
    /// `valued` flags take the next argument; `plain` ones do not. Anything else that starts with
    /// `--` is not a flag of this command.
    fn parse(args: &[&str], valued: &[&str], plain: &[&str]) -> Out<Args> {
        let mut out = Args::default();
        let mut it = args.iter();
        while let Some(&arg) = it.next() {
            if arg == "--" {
                out.rest = it.map(|s| (*s).to_owned()).collect();
                break;
            }
            match arg.strip_prefix("--") {
                Some(name) if valued.contains(&name) => {
                    let value = it
                        .next()
                        .ok_or_else(|| Fail::Usage(format!("--{name} needs a value")))?;
                    out.flags.push((name.to_owned(), Some((*value).to_owned())));
                }
                Some(name) if plain.contains(&name) => out.flags.push((name.to_owned(), None)),
                Some(name) => return Err(Fail::Usage(format!("unknown flag --{name}"))),
                None => out.pos.push(arg.to_owned()),
            }
        }
        Ok(out)
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    /// Exactly `n` words that are not flags, or a usage error.
    fn words<const N: usize>(&self) -> Out<[&str; N]> {
        let got: Vec<&str> = self.pos.iter().map(String::as_str).collect();
        got.try_into()
            .map_err(|_| Fail::Usage(format!("expected {N} argument(s)")))
    }
}

/// The last folder of a path.
fn folder_of(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// The one candidate that `word` names, trying `stages` in turn (the first that fits any is
/// the one that counts): none or several is said, the several listed by `label`.
fn the_one<'a, T>(
    word: &str,
    what: &str,
    stages: [Vec<&'a T>; 3],
    label: impl Fn(&T) -> &str,
) -> Result<&'a T, String> {
    let found = stages
        .into_iter()
        .find(|f| !f.is_empty())
        .unwrap_or_default();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no {what} is named «{word}»")),
        several => Err(format!(
            "«{word}» fits several {what}s: {}",
            several
                .iter()
                .map(|c| label(c))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The workspace `word` names: its id, else exactly its name, branch or folder, else the one
/// whose name or branch starts with it (REQ-355).
fn resolve_workspace<'a>(
    word: &str,
    all: &'a [WorkspaceSummary],
) -> Result<&'a WorkspaceSummary, String> {
    let lower = word.to_lowercase();
    let exact = |w: &WorkspaceSummary| {
        w.name.to_lowercase() == lower
            || w.branch
                .as_deref()
                .is_some_and(|b| b.to_lowercase() == lower)
            || folder_of(&w.path).to_lowercase() == lower
    };
    let starts = |w: &WorkspaceSummary| {
        w.name.to_lowercase().starts_with(&lower)
            || w.branch
                .as_deref()
                .is_some_and(|b| b.to_lowercase().starts_with(&lower))
    };
    let stage = |fits: &dyn Fn(&WorkspaceSummary) -> bool| all.iter().filter(|w| fits(w)).collect();
    the_one(
        word,
        "workspace",
        [stage(&|w| w.id == word), stage(&exact), stage(&starts)],
        |w| &w.name,
    )
}

/// The group `word` names: its id, else exactly its name without regard to case (names are
/// unique so, REQ-686).
fn resolve_group<'a>(word: &str, all: &'a [Group]) -> Result<&'a Group, String> {
    let lower = word.to_lowercase();
    let stage = |fits: &dyn Fn(&Group) -> bool| all.iter().filter(|g| fits(g)).collect();
    the_one(
        word,
        "group",
        [
            stage(&|g| g.id == word),
            stage(&|g| g.name.to_lowercase() == lower),
            Vec::new(),
        ],
        |g| &g.name,
    )
}

/// The name of a workspace's group, when it has one.
fn group_name<'a>(w: &WorkspaceSummary, groups: &'a [Group]) -> Option<&'a str> {
    let id = w.group.as_deref()?;
    groups.iter().find(|g| g.id == id).map(|g| g.name.as_str())
}

/// What is known of a pane of a workspace.
#[derive(Debug, Clone, PartialEq)]
struct PaneInfo {
    id: String,
    title: String,
    running: Option<Vec<String>>,
    agent: Option<&'static str>,
    blocks: Option<String>,
    lines: Vec<String>,
    /// The node of its conversation, while it has one (REQ-453).
    conversation: Option<String>,
}

/// The pane `word` names: its id, the start of one id, or exactly a title.
fn resolve_pane<'a>(word: &str, panes: &'a [PaneInfo]) -> Result<&'a PaneInfo, String> {
    let lower = word.to_lowercase();
    let stage = |fits: &dyn Fn(&PaneInfo) -> bool| panes.iter().filter(|p| fits(p)).collect();
    the_one(
        word,
        "pane",
        [
            stage(&|p| p.id == word),
            stage(&|p| p.id.starts_with(word)),
            stage(&|p| p.title.to_lowercase() == lower),
        ],
        |p| &p.id,
    )
}

/// The last `n` lines of a screen, without the blank ones at the end.
fn screen_text(lines: &[String], n: usize) -> String {
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}

/// The last `n` commands of a pane: `❯ command`, what it printed and how it ended.
fn blocks_text(blocks: &[Block], n: usize) -> String {
    let commands: Vec<&Block> = blocks.iter().filter(|b| !b.restart).collect();
    let mut out = Vec::new();
    for b in &commands[commands.len().saturating_sub(n)..] {
        out.push(format!("❯ {}", b.cmd));
        if b.full {
            out.push("[the command used the whole screen: its output is not kept]".to_owned());
        } else {
            let rows: Vec<String> = b
                .out
                .iter()
                .map(|row| row.iter().map(|s| s.t.as_str()).collect::<String>())
                .collect();
            let text = screen_text(&rows, usize::MAX);
            if !text.is_empty() {
                out.push(text);
            }
        }
        out.push(match (b.running, b.code, b.ms) {
            _ if b.interrupted => "[interrupted: the daemon restarted]".to_owned(),
            (true, ..) => "[running]".to_owned(),
            (false, Some(code), Some(ms)) => format!("[exit {code} · {ms} ms]"),
            (false, Some(code), None) => format!("[exit {code}]"),
            (false, None, _) => "[ended]".to_owned(),
        });
    }
    out.join("\n")
}

/// `text` as a bracketed paste (REQ-363).
fn paste(text: &str) -> String {
    format!("\x1b[200~{text}\x1b[201~")
}

/// The name a kind or an enum has in the JSON documents (`ci_failed`, `folder`).
fn name_of(kind: impl serde::Serialize) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The kind of the most urgent of `attention`, in the order of the terminal client's legend.
fn most_urgent(attention: &[Attention]) -> Option<AttentionKind> {
    crate::tui::URGENCY
        .into_iter()
        .find(|k| attention.iter().any(|a| a.kind == *k))
}

/// Tells when a screen has held some text and not changed for [`SETTLED`] (REQ-363).
struct Settle {
    last: Vec<String>,
    since: Instant,
}

impl Settle {
    fn new(now: Instant) -> Settle {
        Settle {
            last: Vec::new(),
            since: now,
        }
    }

    /// Look at the screen at `now`: whether it has been still, with text, long enough.
    fn settled(&mut self, lines: &[String], now: Instant) -> bool {
        if lines != self.last {
            self.last = lines.to_vec();
            self.since = now;
        }
        lines.iter().any(|l| !l.trim().is_empty()) && now.duration_since(self.since) >= SETTLED
    }
}

/// A connection with the nodes it has opened.
struct Link {
    c: Client,
    opened: HashSet<String>,
}

impl Link {
    fn connect() -> Out<Link> {
        let socket = identity::socket_path();
        let c = Client::connect(&socket, &identity::peer("agent"))
            .map_err(|e| Fail::Error(format!("no daemon answers on {} ({e})", socket.display())))?;
        Ok(Link {
            c,
            opened: HashSet::new(),
        })
    }

    /// The document of `node`, opened if it is not and waited for.
    fn node<T: DeserializeOwned>(&mut self, node: &str) -> Out<T> {
        if self.opened.insert(node.to_owned()) {
            self.c.open(node).map_err(|e| Fail::Error(e.to_string()))?;
        }
        let there = self
            .c
            .wait_until(NODE_WAIT, |c| c.nodes.contains_key(node))
            .map_err(|e| Fail::Error(e.to_string()))?;
        if !there {
            return fail(format!("the daemon did not send {node}"));
        }
        serde_json::from_value(self.c.nodes[node].clone())
            .map_err(|e| Fail::Error(format!("{node}: {e}")))
    }

    fn session(&mut self) -> Out<Session> {
        let root = self.c.root.clone();
        self.node::<Session>(&root)
    }

    fn workspaces(&mut self) -> Out<Vec<WorkspaceSummary>> {
        Ok(self.session()?.workspaces)
    }

    fn workspace(&mut self, word: &str) -> Out<WorkspaceSummary> {
        let all = self.workspaces()?;
        resolve_workspace(word, &all).cloned().map_err(Fail::Error)
    }

    /// The panes of a workspace, each with what it is running.
    fn panes(&mut self, workspace: &str) -> Out<Vec<PaneInfo>> {
        let ws: Workspace = self.node(workspace)?;
        let mut out = Vec::new();
        for summary in ws.panes {
            let pane: Pane = self.node(&summary.id)?;
            let running = pane.running.map(|p| p.argv);
            out.push(PaneInfo {
                conversation: pane.conversation,
                agent: running.as_deref().and_then(daemon::agent_name),
                id: summary.id,
                title: summary.title,
                running,
                blocks: pane.blocks,
                lines: pane.screen.lines,
            });
        }
        Ok(out)
    }

    fn cmd(&mut self, cmd: Cmd) -> Out<Value> {
        match self.c.cmd_within(cmd, REPLY_WAIT) {
            Ok(Ok(result)) => Ok(result.unwrap_or(Value::Null)),
            Ok(Err(e)) => fail(e.message),
            Err(e) => fail(e.to_string()),
        }
    }

    /// The workspace that has the pane `id`.
    fn workspace_of(&mut self, id: &str) -> Out<Option<WorkspaceSummary>> {
        for ws in self.workspaces()? {
            if self
                .node::<Workspace>(&ws.id)?
                .panes
                .iter()
                .any(|p| p.id == id)
            {
                return Ok(Some(ws));
            }
        }
        Ok(None)
    }
}

/// The pane this command runs in, when it runs in one of the daemon's.
fn own_pane() -> Option<String> {
    std::env::var(identity::env_var("PANE_ID"))
        .ok()
        .filter(|p| !p.is_empty())
}

fn ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    a.words::<0>()?;
    let mut s = Link::connect()?;
    let session = s.session()?;
    let (all, groups) = (session.workspaces, session.groups);
    if a.has("json") {
        let rows: Vec<Value> = all
            .iter()
            .map(|w| {
                let mut row = json!({
                    "id": w.id, "name": w.name, "kind": w.kind, "path": w.path,
                    "panes": w.pane_count,
                    "attention": w.attention.iter().map(|a| name_of(a.kind)).collect::<Vec<_>>(),
                });
                if let Some(p) = &w.project {
                    row["project"] = json!(p);
                }
                if let Some(b) = &w.branch {
                    row["branch"] = json!(b);
                }
                if let Some(g) = group_name(w, &groups) {
                    row["group"] = json!(g);
                }
                if let Some(c) = &w.chat {
                    row["chat"] = json!(c);
                }
                row
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else {
        for w in &all {
            println!(
                "{}\t{}\t{}\t{} panes\t{}\t{}",
                w.name,
                name_of(w.kind),
                chat_word(w).or(w.branch.as_deref()).unwrap_or("-"),
                w.pane_count,
                most_urgent(&w.attention).map_or_else(|| "-".to_owned(), name_of),
                group_name(w, &groups).unwrap_or("-"),
            );
        }
    }
    Ok(())
}

fn group_ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    a.words::<0>()?;
    let session = Link::connect()?.session()?;
    let count = |g: &Group| {
        session
            .workspaces
            .iter()
            .filter(|w| w.group.as_deref() == Some(g.id.as_str()))
            .count()
    };
    if a.has("json") {
        let rows: Vec<Value> = session
            .groups
            .iter()
            .map(|g| json!({"id": g.id, "name": g.name, "workspaces": count(g)}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else {
        for g in &session.groups {
            println!("{}\t{} workspaces", g.name, count(g));
        }
    }
    Ok(())
}

fn group_new(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [name] = a.words()?;
    let reply = Link::connect()?.cmd(Cmd::CreateGroup { name: name.into() })?;
    println!("{}", reply["group"].as_str().unwrap_or_default());
    Ok(())
}

fn group_rename(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [word, name] = a.words()?;
    let mut s = Link::connect()?;
    let session = s.session()?;
    let group = resolve_group(word, &session.groups).map_err(Fail::Error)?;
    s.cmd(Cmd::RenameGroup {
        group: group.id.clone(),
        name: name.into(),
    })?;
    Ok(())
}

fn group_rm(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [word] = a.words()?;
    let mut s = Link::connect()?;
    let session = s.session()?;
    let group = resolve_group(word, &session.groups).map_err(Fail::Error)?;
    s.cmd(Cmd::DeleteGroup {
        group: group.id.clone(),
    })?;
    Ok(())
}

/// `group move <workspace> [<group>]`: the workspace's repository goes to the group, or out of
/// every group when none is named.
fn group_move(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let (word, to) = match a.pos.as_slice() {
        [ws] => (ws.as_str(), None),
        [ws, group] => (ws.as_str(), Some(group.as_str())),
        _ => return Err(Fail::Usage("expected 1 or 2 arguments".into())),
    };
    let mut s = Link::connect()?;
    let session = s.session()?;
    let ws = resolve_workspace(word, &session.workspaces).map_err(Fail::Error)?;
    let group = match to {
        Some(g) => Some(
            resolve_group(g, &session.groups)
                .map_err(Fail::Error)?
                .id
                .clone(),
        ),
        None => None,
    };
    s.cmd(Cmd::MoveToGroup {
        workspace: ws.id.clone(),
        group,
    })?;
    Ok(())
}

/// What `ls` says of a chat where it says the branch (REQ-833).
fn chat_word(w: &WorkspaceSummary) -> Option<&'static str> {
    w.chat
        .as_ref()
        .map(|c| if c.archived { "chat archivada" } else { "chat" })
}

/// The chat `word` names: a workspace that is one.
fn chat_named(s: &mut Link, word: &str) -> Out<WorkspaceSummary> {
    let ws = s.workspace(word)?;
    if ws.chat.is_none() {
        return fail(format!("{} is not a conversation of its own", ws.name));
    }
    Ok(ws)
}

/// The archived chat `ws` cannot be read from or prompted: it has no panel (REQ-834).
fn not_archived(ws: &WorkspaceSummary, word: &str) -> Out<()> {
    if ws.chat.as_ref().is_some_and(|c| c.archived) {
        return fail(format!("archivada: {} chat resume {word}", bin()));
    }
    Ok(())
}

/// `chat new [--json]`: a conversation of its own; prints its id and name (REQ-833).
fn chat_new(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    a.words::<0>()?;
    let mut s = Link::connect()?;
    let id = s.cmd(Cmd::ChatNew {})?["workspace"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    // The session says it a moment after the reply: its name is waited for.
    let deadline = Instant::now() + NODE_WAIT;
    let name = loop {
        if let Some(w) = s.workspaces()?.into_iter().find(|w| w.id == id) {
            break w.name;
        }
        if Instant::now() >= deadline {
            return fail(format!("the conversation {id} was made but is not listed"));
        }
        let _ = s.c.poll(Duration::from_millis(100));
    };
    // `claude` starts a moment after the panel and opens its conversation a moment after that:
    // they are waited for, as `worktree --task` does (REQ-456), so that `prompt --wait` works at
    // once. An agent that does not come is not an error: the chat is made.
    let deadline = Instant::now() + agent_wait();
    loop {
        // Whatever fails here, the chat exists: its id is printed all the same.
        if let Ok(panes) = s.panes(&id)
            && let Some(p) = panes.into_iter().find(|p| p.agent.is_some())
        {
            let _ = with_conversation(&mut s, &id, p);
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        let _ = s.c.poll(Duration::from_millis(200));
    }
    if a.has("json") {
        println!("{}", json!({"id": id, "name": name}));
    } else {
        println!("{id}\t{name}");
    }
    Ok(())
}

fn chat_archive(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [word] = a.words()?;
    let mut s = Link::connect()?;
    let ws = chat_named(&mut s, word)?;
    let reply = s.cmd(Cmd::ArchiveChat {
        workspace: ws.id.clone(),
    })?;
    if reply["discarded"] == true {
        println!("discarded: {} had nothing said", ws.name);
    }
    Ok(())
}

fn chat_resume(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [word] = a.words()?;
    let mut s = Link::connect()?;
    let ws = chat_named(&mut s, word)?;
    let reply = s.cmd(Cmd::ResumeChat {
        workspace: ws.id.clone(),
    })?;
    println!("{}", reply["pane"].as_str().unwrap_or_default());
    Ok(())
}

fn chat_rm(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [word] = a.words()?;
    let mut s = Link::connect()?;
    let ws = chat_named(&mut s, word)?;
    s.cmd(Cmd::DeleteChat {
        workspace: ws.id.clone(),
    })?;
    Ok(())
}

/// `chat promote <chat> <workspace> [--no-summary]`: a new conversation in the workspace; prints
/// the pane and, on stdout after it, the summary of the chat for the agent to start from.
fn chat_promote(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["no-summary"])?;
    let [word, to] = a.words()?;
    let mut s = Link::connect()?;
    let ws = chat_named(&mut s, word)?;
    let to = s.workspace(to)?;
    let reply = s.cmd(Cmd::PromoteChat {
        workspace: ws.id.clone(),
        to: to.id.clone(),
        summary: a.has("no-summary").then_some(false),
    })?;
    println!("{}", reply["pane"].as_str().unwrap_or_default());
    let text = reply["text"].as_str().unwrap_or_default();
    if !text.is_empty() {
        print!("{text}");
    }
    Ok(())
}

fn pane_ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    let [word] = a.words()?;
    let mut s = Link::connect()?;
    let ws = s.workspace(word)?;
    let panes = s.panes(&ws.id)?;
    if a.has("json") {
        let rows: Vec<Value> = panes
            .iter()
            .map(|p| {
                let mut row = json!({"id": p.id, "title": p.title});
                if let Some(r) = &p.running {
                    row["running"] = json!(r);
                }
                if let Some(agent) = p.agent {
                    row["agent"] = json!(agent);
                }
                row
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else {
        for p in &panes {
            println!(
                "{}\t{}\t{}\t{}",
                p.id,
                p.title,
                p.running
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |r| r.join(" ")),
                p.agent.unwrap_or("")
            );
        }
    }
    Ok(())
}

fn lines_flag(a: &Args) -> Out<usize> {
    match a.value("lines") {
        None => Ok(DEFAULT_LINES),
        Some(n) => n
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Fail::Usage("--lines needs a number above 0".into())),
    }
}

/// The pane that `--pane` names, else the first with an agent in it.
fn pick_agent_pane<'a>(a: &Args, panes: &'a [PaneInfo]) -> Out<Option<&'a PaneInfo>> {
    match a.value("pane") {
        Some(word) => resolve_pane(word, panes).map(Some).map_err(Fail::Error),
        None => Ok(panes.iter().find(|p| p.agent.is_some())),
    }
}

fn read(args: &[&str]) -> Out {
    let a = Args::parse(args, &["pane", "lines"], &["blocks"])?;
    let [word] = a.words()?;
    let n = lines_flag(&a)?;
    let mut s = Link::connect()?;
    let ws = s.workspace(word)?;
    not_archived(&ws, word)?;
    let panes = s.panes(&ws.id)?;
    let Some(pane) = pick_agent_pane(&a, &panes)?.or(panes.first()) else {
        return fail(format!("{} has no panes", ws.name));
    };
    if let Some(node) = pane.conversation.as_ref().filter(|_| !a.has("blocks")) {
        let conv: Conversation = s.node(node)?;
        println!("{}", conversation_text(&conv, n));
    } else if a.has("blocks") {
        let Some(node) = &pane.blocks else {
            return fail(format!("pane {} has no commands to read", pane.id));
        };
        let blocks: Blocks = s.node(node)?;
        println!("{}", blocks_text(&blocks.blocks, n));
    } else {
        println!("{}", screen_text(&pane.lines, n));
    }
    Ok(())
}

/// Type `text` into `pane` the way a person pastes it and presses Enter. Unless `now`, wait
/// first for the screen to settle (REQ-363).
fn send_prompt(s: &mut Link, pane: &str, text: &str, now: bool) -> Out {
    if !now {
        let mut settle = Settle::new(Instant::now());
        let deadline = Instant::now() + agent_wait();
        loop {
            let lines = s.node::<Pane>(pane)?.screen.lines;
            if settle.settled(&lines, Instant::now()) {
                break;
            }
            if Instant::now() >= deadline {
                return fail(format!(
                    "the screen of pane {pane} never settled; nothing was written (--now writes at once)"
                ));
            }
            s.c.poll(Duration::from_millis(100))
                .map_err(|e| Fail::Error(e.to_string()))?;
        }
    }
    s.cmd(Cmd::Write {
        pane: pane.to_owned(),
        data: paste(text),
    })?;
    std::thread::sleep(ENTER_AFTER);
    s.cmd(Cmd::Write {
        pane: pane.to_owned(),
        data: "\r".into(),
    })?;
    Ok(())
}

/// The last `n` items of a conversation as text, and what waits for an answer (REQ-455).
fn conversation_text(conv: &Conversation, n: usize) -> String {
    let from = conv.items.len().saturating_sub(n);
    let mut lines: Vec<String> = conv.items[from..]
        .iter()
        .flat_map(crate::claude::item_lines)
        .collect();
    if let Some(ask) = &conv.ask {
        lines.extend(crate::claude::ask_lines(ask));
    }
    lines.join("\n")
}

/// Whether the turn that `sent` started, after the item numbered `baseline`, is over, and what
/// it came to: Claude's last words, or what the question that waits asks (REQ-454). `None` while
/// it goes on.
fn turn_end(conv: &Conversation, baseline: u64, sent: &str) -> Option<Out<String>> {
    if let Some(ask) = &conv.ask {
        return Some(Err(Fail::Waiting(crate::claude::ask_lines(ask).join("\n"))));
    }
    if conv.status == ConvStatus::Dead {
        return Some(fail("the conversation's engine is gone"));
    }
    if conv.status != ConvStatus::Idle || !conv.queue.is_empty() {
        return None;
    }
    let items = conv
        .items
        .get(baseline.saturating_sub(u64::from(conv.first)) as usize..)?;
    // The daemon trims what it is given before it writes it down. A command of the `/` list
    // leaves no message of the person's behind it.
    let sent = sent.trim();
    let start = if sent.starts_with('/') {
        0
    } else {
        items
            .iter()
            .rposition(|i| matches!(i, Item::User { text, .. } if text == sent))?
    };
    let turn = &items[start..];
    if !turn
        .iter()
        .any(|i| matches!(i, Item::Result { .. } | Item::Out { .. }))
    {
        return None;
    }
    let said = turn.iter().rev().find_map(|i| match i {
        Item::Text { text, .. } => Some(text.clone()),
        _ => None,
    });
    Some(Ok(said.unwrap_or_else(|| {
        turn.iter()
            .filter_map(|i| match i {
                Item::Out { lines } => Some(lines.join("\n")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    })))
}

/// Say `text` to `node`'s conversation as the person would; with `wait`, wait for the turn it
/// starts to end and give back its answer (REQ-453, REQ-454).
fn converse(
    s: &mut Link,
    node: &str,
    text: &str,
    wait: Option<Duration>,
    now: bool,
) -> Out<Option<String>> {
    let baseline = match wait {
        Some(_) => {
            let conv: Conversation = s.node(node)?;
            u64::from(conv.first) + conv.items.len() as u64
        }
        None => 0,
    };
    s.cmd(Cmd::ConversationSend {
        node: node.to_owned(),
        text: text.to_owned(),
        images: Vec::new(),
        folds: Vec::new(),
        // What an agent sends is not what the person typed: it stays out of their ↑ (REQ-716).
        private: true,
        now,
    })?;
    let Some(limit) = wait else {
        return Ok(None);
    };
    let deadline = Instant::now() + limit;
    loop {
        let conv: Conversation = s.node(node)?;
        if let Some(end) = turn_end(&conv, baseline, text) {
            return end.map(Some);
        }
        if Instant::now() >= deadline {
            return fail(format!("the turn did not end in {} s", limit.as_secs()));
        }
        s.c.poll(Duration::from_millis(100))
            .map_err(|e| Fail::Error(e.to_string()))?;
    }
}

fn timeout_flag(a: &Args) -> Out<Option<Duration>> {
    if !a.has("wait") {
        return match a.value("timeout") {
            None => Ok(None),
            Some(_) => Err(Fail::Usage("--timeout goes with --wait".into())),
        };
    }
    match a.value("timeout") {
        None => Ok(Some(TURN_WAIT)),
        Some(n) => n
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| Some(Duration::from_secs(n)))
            .ok_or_else(|| Fail::Usage("--timeout needs a number of seconds above 0".into())),
    }
}

fn prompt(args: &[&str]) -> Out {
    let a = Args::parse(args, &["pane", "timeout"], &["tag", "now", "wait"])?;
    let wait = timeout_flag(&a)?;
    let Some((word, words)) = a.pos.split_first() else {
        return Err(Fail::Usage("expected a workspace and a text".into()));
    };
    let text = words
        .iter()
        .chain(&a.rest)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        return Err(Fail::Usage("expected a text to send".into()));
    }
    let mut s = Link::connect()?;
    let ws = s.workspace(word)?;
    not_archived(&ws, word)?;
    let panes = s.panes(&ws.id)?;
    let Some(target) = pick_agent_pane(&a, &panes)? else {
        return fail(format!("no agent runs in {}", ws.name));
    };
    let me = own_pane();
    if me.as_deref() == Some(target.id.as_str()) {
        return fail("that is the pane this command runs in; nothing was written");
    }
    let text = if a.has("tag") {
        let Some(me) = &me else {
            return fail("--tag needs the command to run in a pane");
        };
        let from = s
            .workspace_of(me)?
            .map_or_else(|| "?".to_owned(), |w| w.name);
        format!("[{}: {from}, pane {me}] {text}", bin())
    } else {
        text
    };
    let sent = format!("sent to pane {} of {}", target.id, ws.name);
    match &target.conversation {
        Some(node) => match converse(&mut s, node, &text, wait, a.has("now"))? {
            Some(answer) => {
                eprintln!("{sent}");
                println!("{answer}");
            }
            None => println!("{sent}"),
        },
        None if wait.is_some() => {
            return fail(format!(
                "pane {} has no conversation to wait on (--wait needs `claude` run by {})",
                target.id,
                bin()
            ));
        }
        None => {
            send_prompt(&mut s, &target.id, &text, a.has("now"))?;
            println!("{sent}");
        }
    }
    Ok(())
}

/// The repository of `worktree`: `--in` (a folder, or a workspace), else the project of the
/// workspace this command runs in, else the folder it runs in.
fn repo_of(s: &mut Link, given: Option<&str>) -> Out<String> {
    if let Some(given) = given {
        if Path::new(given).is_dir() {
            return Ok(absolute(Path::new(given)));
        }
        let ws = s.workspace(given)?;
        return Ok(ws.project.unwrap_or(ws.path));
    }
    if let Some(me) = own_pane()
        && let Some(ws) = s.workspace_of(&me)?
        && let Some(project) = ws.project
    {
        return Ok(project);
    }
    std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .map_err(|e| Fail::Error(e.to_string()))
}

fn absolute(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .into_owned()
}

fn worktree(args: &[&str]) -> Out {
    let a = Args::parse(args, &["in", "branch", "from", "task", "open"], &[])?;
    let [name] = a.words()?;
    // A task is for an agent: `claude` opens it unless the person said otherwise (REQ-511).
    let open = match (a.value("open"), a.value("task")) {
        (Some("shell"), _) => Some(contract::OpenKind::Shell),
        (Some("claude"), _) | (None, Some(_)) => Some(contract::OpenKind::Claude),
        (None, None) => None,
        (Some(_), _) => return Err(Fail::Usage("--open is shell or claude".into())),
    };
    let mut s = Link::connect()?;
    let repo = repo_of(&mut s, a.value("in"))?;
    // The name is the branch, as typed; the folder is its last part (REQ-512).
    let branch = a.value("branch").unwrap_or(name).to_owned();
    let folder = name.rsplit('/').next().unwrap_or(name).to_owned();
    let made = s.cmd(Cmd::CreateWorktree {
        repo,
        branch,
        base: a.value("from").map(str::to_owned),
        folder: Some(folder),
    })?;
    let (Some(id), Some(path)) = (made["workspace"].as_str(), made["path"].as_str()) else {
        return fail("the daemon did not say what it made");
    };
    let (id, path) = (id.to_owned(), path.to_owned());
    say_copies(&made);
    let started = s.cmd(Cmd::StartWorkspace {
        workspace: id.clone(),
        open,
    });
    if started.is_err() {
        let _ = s.cmd(Cmd::NewPane {
            workspace: id.clone(),
            program: None,
            beside: None,
            dir: None,
        });
    }
    println!("{id} {path}");
    if let Err(Fail::Error(why)) = started {
        return fail(format!(
            "the worktree was made at {path} and opened with one shell, but its panes were refused: {why}"
        ));
    }
    if let Some(task) = a.value("task") {
        let deadline = Instant::now() + agent_wait();
        let agent = loop {
            if let Some(p) = s.panes(&id)?.into_iter().find(|p| p.agent.is_some()) {
                break Some(p);
            }
            if Instant::now() >= deadline {
                break None;
            }
            let _ = s.c.poll(Duration::from_millis(200));
        };
        let Some(pane) = agent else {
            return fail(format!(
                "the worktree was made at {path}, but no agent took the task"
            ));
        };
        let pane = with_conversation(&mut s, &id, pane)?;
        match &pane.conversation {
            Some(node) => {
                converse(&mut s, node, task, None, false)?;
            }
            None => send_prompt(&mut s, &pane.id, task, false)?,
        }
    }
    Ok(())
}

/// The new agent `pane` of workspace `id` once its conversation is there: `claude` opens it a
/// moment after it starts, so it is waited for a little (REQ-456).
fn with_conversation(s: &mut Link, id: &str, mut pane: PaneInfo) -> Out<PaneInfo> {
    if pane.agent != Some("claude") {
        return Ok(pane);
    }
    let deadline = Instant::now() + CONVERSATION_WAIT.min(agent_wait());
    while pane.conversation.is_none() && Instant::now() < deadline {
        let _ = s.c.poll(Duration::from_millis(200));
        if let Some(p) = s.panes(id)?.into_iter().find(|p| p.id == pane.id) {
            pane = p;
        }
    }
    Ok(pane)
}

/// What was copied into a new worktree and what was not, on the error output so that what
/// `worktree` prints stays as it was (REQ-493).
fn say_copies(made: &Value) {
    let list = |key: &str| -> Vec<String> {
        made[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let copied = list("copied");
    if !copied.is_empty() {
        eprintln!("{} worktree: copied {}", bin(), copied.join(", "));
    }
    if let Some(ports) = made["ports"].as_object().filter(|p| !p.is_empty()) {
        let list: Vec<String> = ports
            .iter()
            .filter_map(|(name, port)| port.as_u64().map(|p| format!("{name}={p}")))
            .collect();
        eprintln!("{} worktree: ports {}", bin(), list.join(" "));
    }
    for skipped in made["skipped"].as_array().into_iter().flatten() {
        if let (Some(path), Some(why)) = (skipped["path"].as_str(), skipped["why"].as_str()) {
            eprintln!("{} worktree: did not copy {path}: {why}", bin());
        }
    }
}

fn pane_open(args: &[&str]) -> Out {
    let a = Args::parse(args, &["split", "of"], &[])?;
    let [word] = a.words()?;
    let dir = match a.value("split") {
        None => None,
        Some("right") => Some(SplitDir::Row),
        Some("down") => Some(SplitDir::Col),
        Some(_) => return Err(Fail::Usage("--split is right or down".into())),
    };
    let mut s = Link::connect()?;
    let ws = s.workspace(word)?;
    let panes = s.panes(&ws.id)?;
    let beside = match a.value("of") {
        Some(w) => Some(resolve_pane(w, &panes).map_err(Fail::Error)?.id.clone()),
        None if dir.is_some() => panes.last().map(|p| p.id.clone()),
        None => None,
    };
    let program = (!a.rest.is_empty()).then(|| a.rest.clone());
    let made = s.cmd(Cmd::NewPane {
        workspace: ws.id,
        program,
        beside,
        dir,
    })?;
    match made["pane"].as_str() {
        Some(id) => {
            println!("{id}");
            Ok(())
        }
        None => fail("the daemon did not say which pane it made"),
    }
}

fn pane_close(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["force"])?;
    let [word, pane] = a.words()?;
    let mut s = Link::connect()?;
    let ws = s.workspace(word)?;
    let panes = s.panes(&ws.id)?;
    let target = resolve_pane(pane, &panes).map_err(Fail::Error)?.id.clone();
    if !a.has("force") {
        if own_pane().as_deref() == Some(target.as_str()) {
            return fail("that is the pane this command runs in; --force closes it anyway");
        }
        if panes.len() == 1 {
            return fail(format!(
                "that is the last pane of {}; --force closes it anyway",
                ws.name
            ));
        }
    }
    s.cmd(Cmd::ClosePane { pane: target })?;
    Ok(())
}

/// The skill with the binary's name and the pane variable of the product's identity.
fn skill_text() -> String {
    SKILL
        .replace("{bin}", bin())
        .replace("{pane_env}", &identity::env_var("PANE_ID"))
}

/// The skill of the task lists (N180e, REQ-933): the binary's name, the pane variable and the list's
/// file as the product's identity says them.
fn tasks_skill_text() -> String {
    TASKS_SKILL
        .replace("{bin}", bin())
        .replace("{pane_env}", &identity::env_var("PANE_ID"))
        .replace(
            "{tasks_file}",
            &identity::tasks_path(Path::new("")).to_string_lossy(),
        )
}

/// The Claude configuration the skills go under: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_base(config_dir: Option<String>, home: Option<String>) -> Option<PathBuf> {
    match config_dir.filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => Some(PathBuf::from(home.filter(|h| !h.is_empty())?).join(".claude")),
    }
}

/// Where the skill goes: `skills/<name>/SKILL.md` under the Claude configuration (REQ-370).
fn skill_path(config_dir: Option<String>, home: Option<String>) -> Option<PathBuf> {
    Some(
        claude_base(config_dir, home)?
            .join("skills")
            .join(bin())
            .join("SKILL.md"),
    )
}

/// Where the skill of the task lists goes: `skills/<name>-tasks/SKILL.md` (REQ-932). The name is never
/// `tasks`, which is a command of claude's own (REQ-955).
fn tasks_skill_path(config_dir: Option<String>, home: Option<String>) -> Option<PathBuf> {
    Some(
        claude_base(config_dir, home)?
            .join("skills")
            .join(format!("{}-tasks", bin()))
            .join("SKILL.md"),
    )
}

/// Whether the skill of the task lists is installed, which is what lets a hook name it (REQ-934).
pub(crate) fn tasks_skill_installed() -> bool {
    tasks_skill_path(
        std::env::var("CLAUDE_CONFIG_DIR").ok(),
        std::env::var("HOME").ok(),
    )
    .is_some_and(|p| p.is_file())
}

/// The one line the `UserPromptSubmit` hook adds to what the agent reads (REQ-934).
pub(crate) fn tasks_nudge() -> String {
    let b = bin();
    format!(
        "You run inside {b}. After you make a plan, use the {b}-tasks skill before doing any step of it."
    )
}

/// Write a skill whole, through a rename, and say where.
fn write_skill(path: &Path, text: &str) -> Out {
    let dir = path
        .parent()
        .ok_or_else(|| Fail::Error("no folder".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| Fail::Error(format!("{}: {e}", dir.display())))?;
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    println!("installed {}", path.display());
    Ok(())
}

/// `skill install` writes the skill of the session and the one of the task lists (REQ-932).
fn skill_install() -> Out {
    let (config, home) = (
        std::env::var("CLAUDE_CONFIG_DIR").ok(),
        std::env::var("HOME").ok(),
    );
    let (Some(skill), Some(tasks)) = (
        skill_path(config.clone(), home.clone()),
        tasks_skill_path(config, home),
    ) else {
        return fail("no HOME and no CLAUDE_CONFIG_DIR to install into");
    };
    write_skill(&skill, &skill_text())?;
    write_skill(&tasks, &tasks_skill_text())
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::{Span, WorkspaceKind};

    fn ws(id: &str, name: &str, branch: Option<&str>, path: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: name.into(),
            kind: if branch.is_some() {
                WorkspaceKind::Worktree
            } else {
                WorkspaceKind::Folder
            },
            path: path.into(),
            project: None,
            branch: branch.map(str::to_owned),
            pane_count: 1,
            attention: Vec::new(),
            review: None,
            working: false,
            removing: false,
            group: None,
            chat: None,
            tasks: None,
        }
    }

    fn all() -> Vec<WorkspaceSummary> {
        vec![
            ws("a1", "app", None, "/code/app"),
            ws(
                "b2",
                "app: feat/login",
                Some("feat/login"),
                "/code/app-wt/login",
            ),
            ws(
                "c3",
                "app: feat/logout",
                Some("feat/logout"),
                "/code/app-wt/exit",
            ),
            ws("d4", "notes", None, "/home/u/notes"),
        ]
    }

    #[test]
    fn a_workspace_is_named_by_id_name_branch_or_folder_and_else_by_a_unique_start() {
        let all = all();
        let id = |w: &str| resolve_workspace(w, &all).map(|w| w.id.as_str());
        assert_eq!(id("b2"), Ok("b2"), "an id");
        assert_eq!(
            id("app"),
            Ok("a1"),
            "exactly a name, though others start with it"
        );
        assert_eq!(id("feat/login"), Ok("b2"), "a branch");
        assert_eq!(id("exit"), Ok("c3"), "a folder");
        assert_eq!(
            id("NOT"),
            Ok("d4"),
            "the start of a name, whatever the case"
        );
        assert_eq!(id("app: feat/logi"), Ok("b2"));
        assert_eq!(id("feat/logo"), Ok("c3"), "the start of a branch");
        let several = id("app: ").unwrap_err();
        assert!(
            several.contains("fits several workspaces: app: feat/login, app: feat/logout"),
            "{several}"
        );
        assert_eq!(id("zzz"), Err("no workspace is named «zzz»".into()));
        assert!(
            id("").unwrap_err().contains("several"),
            "an empty word fits all"
        );
    }

    #[test]
    fn a_group_is_named_by_id_or_by_exactly_its_name_without_regard_to_case_and_a_workspace_says_its_group()
     {
        let groups = vec![
            Group {
                id: "g1".into(),
                name: "Product".into(),
            },
            Group {
                id: "g2".into(),
                name: "product ideas".into(),
            },
        ];
        let id = |w: &str| resolve_group(w, &groups).map(|g| g.id.as_str());
        assert_eq!(id("g2"), Ok("g2"), "an id");
        assert_eq!(id("PRODUCT"), Ok("g1"), "a name, whatever the case");
        assert_eq!(
            id("product i"),
            Err("no group is named «product i»".into()),
            "no starts"
        );
        assert_eq!(id("zzz"), Err("no group is named «zzz»".into()));
        let mut w = ws("a1", "app", None, "/x/app");
        assert_eq!(group_name(&w, &groups), None);
        w.group = Some("g2".into());
        assert_eq!(group_name(&w, &groups), Some("product ideas"));
        w.group = Some("gone".into());
        assert_eq!(group_name(&w, &groups), None, "a group that is not listed");
    }

    fn pane(id: &str, title: &str, agent: Option<&'static str>) -> PaneInfo {
        PaneInfo {
            id: id.into(),
            title: title.into(),
            running: None,
            agent,
            blocks: None,
            lines: Vec::new(),
            conversation: None,
        }
    }

    #[test]
    fn a_pane_is_named_by_id_the_start_of_an_id_or_a_title() {
        let panes = [
            pane("abc1", "zsh", None),
            pane("abd2", "claude", Some("claude")),
        ];
        let id = |w: &str| resolve_pane(w, &panes).map(|p| p.id.as_str());
        assert_eq!(id("abc1"), Ok("abc1"));
        assert_eq!(id("abd"), Ok("abd2"));
        assert_eq!(id("Claude"), Ok("abd2"));
        assert!(id("ab").unwrap_err().contains("several panes: abc1, abd2"));
        assert!(id("x").unwrap_err().contains("no pane"));
    }

    #[test]
    fn a_screen_is_read_from_its_last_lines_without_the_blank_ones_below() {
        let lines: Vec<String> = ["one", "two", "", "three", "  ", ""]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(screen_text(&lines, 40), "one\ntwo\n\nthree");
        assert_eq!(screen_text(&lines, 2), "\nthree");
        assert_eq!(screen_text(&lines, 1), "three");
        assert_eq!(screen_text(&["".to_owned()], 5), "");
        assert_eq!(screen_text(&[], 5), "");
    }

    fn block(cmd: &str, out: &[&str], code: Option<i32>, ms: Option<u64>) -> Block {
        Block {
            id: 1,
            cmd: cmd.into(),
            cwd: "/".into(),
            started: 0,
            running: code.is_none(),
            code,
            ms,
            truncated: false,
            full: false,
            interrupted: false,
            restart: false,
            out: out
                .iter()
                .map(|l| {
                    vec![Span {
                        t: (*l).into(),
                        fg: None,
                        bg: None,
                        b: false,
                        i: false,
                        u: false,
                    }]
                })
                .collect(),
        }
    }

    #[test]
    fn commands_are_read_with_their_output_and_how_they_ended() {
        let mut full = block("nvim x", &[], Some(0), Some(9));
        full.full = true;
        let mut cut = block("sleep 9", &[], None, None);
        cut.interrupted = true;
        let mut marker = block("", &[], None, None);
        marker.restart = true;
        let blocks = vec![
            block("ls", &["a", "b", ""], Some(0), Some(12)),
            marker,
            block("false", &[], Some(1), None),
            full,
            cut,
            block("sleep 1", &[], None, None),
        ];
        assert_eq!(
            blocks_text(&blocks, 40),
            "❯ ls\na\nb\n[exit 0 · 12 ms]\n❯ false\n[exit 1]\n❯ nvim x\n[the command used the whole screen: its output is not kept]\n[exit 0 · 9 ms]\n❯ sleep 9\n[interrupted: the daemon restarted]\n❯ sleep 1\n[running]"
        );
        assert_eq!(blocks_text(&blocks, 1), "❯ sleep 1\n[running]");
        assert_eq!(blocks_text(&[], 3), "");
    }

    #[test]
    fn a_text_is_pasted_between_the_two_marks() {
        assert_eq!(paste("hi\nthere"), "\x1b[200~hi\nthere\x1b[201~");
    }

    #[test]
    fn an_agent_is_ready_when_its_screen_has_text_and_has_not_moved_for_a_second() {
        let t0 = Instant::now();
        let mut s = Settle::new(t0);
        let at = |ms| t0 + Duration::from_millis(ms);
        let blank = vec![String::new()];
        let one = vec!["❯ ".to_owned()];
        let two = vec!["❯ ".to_owned(), "loading".to_owned()];
        assert!(!s.settled(&blank, at(0)), "nothing on it");
        assert!(!s.settled(&blank, at(5_000)), "still nothing, however long");
        assert!(!s.settled(&one, at(5_100)), "text, but it just appeared");
        assert!(!s.settled(&one, at(5_900)));
        assert!(s.settled(&one, at(6_100)), "a second without a change");
        assert!(
            !s.settled(&two, at(6_200)),
            "it moved: the clock starts again"
        );
        assert!(s.settled(&two, at(7_300)));
    }

    #[test]
    fn the_most_urgent_attention_follows_the_terminal_clients_legend() {
        let a = |kind| Attention {
            kind,
            node: "n".into(),
            rev: 1,
            exit_code: None,
        };
        assert_eq!(most_urgent(&[]), None);
        assert_eq!(
            most_urgent(&[
                a(AttentionKind::Merged),
                a(AttentionKind::Question),
                a(AttentionKind::AgentDone)
            ]),
            Some(AttentionKind::Question)
        );
        assert_eq!(name_of(AttentionKind::CiFailed), "ci_failed");
    }

    #[test]
    fn flags_come_before_or_after_words_and_two_dashes_end_them() {
        let a = Args::parse(
            &["ws", "--pane", "p1", "--tag", "--", "--now", "x"],
            &["pane"],
            &["tag", "now"],
        )
        .unwrap();
        assert_eq!(a.pos, ["ws"]);
        assert_eq!(a.value("pane"), Some("p1"));
        assert!(a.has("tag") && !a.has("now"));
        assert_eq!(a.rest, ["--now", "x"], "what follows -- is not a flag");
        assert!(
            matches!(Args::parse(&["--nope"], &[], &[]), Err(Fail::Usage(m)) if m.contains("--nope"))
        );
        assert!(
            matches!(Args::parse(&["--pane"], &["pane"], &[]), Err(Fail::Usage(m)) if m.contains("needs a value"))
        );
        assert!(
            Args::parse(&["a", "b"], &[], &[])
                .unwrap()
                .words::<1>()
                .is_err()
        );
        assert_eq!(
            Args::parse(&["a"], &[], &[]).unwrap().words::<1>(),
            Ok(["a"])
        );
        assert_eq!(
            lines_flag(&Args::parse(&["--lines", "7"], &["lines"], &[]).unwrap()),
            Ok(7)
        );
        assert!(lines_flag(&Args::parse(&["--lines", "0"], &["lines"], &[]).unwrap()).is_err());
        assert!(lines_flag(&Args::parse(&["--lines", "x"], &["lines"], &[]).unwrap()).is_err());
    }

    #[test]
    fn the_skill_says_how_to_publish_an_artifact_and_what_the_verdict_is_like() {
        let text = skill_text();
        for word in [
            "artifact publish",
            "artifact wait",
            "--wait",
            "APROBADO",
            "exits 4",
        ] {
            assert!(text.contains(word), "the skill does not say {word:?}");
        }
    }

    #[test]
    fn the_skill_names_the_binary_and_goes_under_the_claude_configuration() {
        let text = skill_text();
        assert!(text.contains(bin()) && !text.contains("{bin}") && !text.contains("{pane_env}"));
        assert!(text.contains(&identity::env_var("PANE_ID")));
        assert!(
            text.starts_with("---\nname: "),
            "a skill starts with its front matter"
        );
        let name = bin();
        assert_eq!(
            skill_path(Some("/cfg".into()), Some("/home/u".into())),
            Some(PathBuf::from(format!("/cfg/skills/{name}/SKILL.md")))
        );
        assert_eq!(
            skill_path(Some(String::new()), Some("/home/u".into())),
            Some(PathBuf::from(format!(
                "/home/u/.claude/skills/{name}/SKILL.md"
            )))
        );
        assert_eq!(skill_path(None, None), None);
    }

    /// REQ-933, REQ-935: what the skill of the task lists says, by the words the loop and the commands use.
    #[test]
    fn the_tasks_skill_says_the_steps_with_the_words_of_the_commands() {
        let text = tasks_skill_text();
        let b = bin();
        assert!(
            !text.contains('{')
                || !text.contains("{bin}")
                    && !text.contains("{pane_env}")
                    && !text.contains("{tasks_file}")
        );
        assert!(
            text.starts_with(&format!("---\nname: {b}-tasks\n")),
            "{text}"
        );
        assert!(text.contains(&identity::env_var("PANE_ID")));
        // the file is named by the identity and not spelled in the skill
        let file = identity::tasks_path(Path::new(""))
            .to_string_lossy()
            .into_owned();
        assert!(
            text.contains(&format!("`{file}`")) && !TASKS_SKILL.contains(&file),
            "{text}"
        );
        for word in [
            "right after you have made a plan",
            "- [ ] t1 title (after: t0)",
            &format!("{b} tasks ls"),
            &format!("{b} tasks loop on"),
            &format!("{b} tasks done <id>"),
            &format!("{b} tasks block <id> \"why\""),
            &format!("{b} tasks note fin"),
            "do not\n   ask",
            "run the project's tests and CI",
            "ask the advisor",
            "keep the\nperson's wording and order",
            "wait for the person's OK",
            "never replace it",
            "never rewrite the list while a loop is on",
        ] {
            assert!(text.contains(word), "the skill does not say {word:?}");
        }
        // never `tasks` alone, which is a command of claude's own (REQ-955)
        let path = tasks_skill_path(Some("/cfg".into()), None).unwrap();
        assert_eq!(
            path,
            PathBuf::from(format!("/cfg/skills/{b}-tasks/SKILL.md"))
        );
        assert!(!path.to_string_lossy().contains("/tasks/"));
        assert_eq!(tasks_skill_path(None, None), None);
        assert!(
            tasks_nudge().contains(&format!("{b}-tasks skill")) && !tasks_nudge().contains('\n')
        );
    }

    #[test]
    fn the_folder_of_a_path_is_its_last_part() {
        assert_eq!(folder_of("/code/app-wt/login"), "login");
        assert_eq!(folder_of("/code/app/"), "app");
    }

    fn conversation(
        status: &str,
        first: u32,
        items: Value,
        ask: Value,
        queue: &[&str],
    ) -> Conversation {
        let mut doc = json!({
            "id": "c1", "v": 1, "epoch": "e", "rev": 1, "attention": [],
            "pane": "p1", "name": "n", "accent": "blue", "model": "m", "effort": "high",
            "mode": "manual", "status": status, "first": first, "items": items,
            "queue": queue, "tasks": [], "todos": [], "artifacts": [], "commands": [], "cost": 0,
        });
        if !ask.is_null() {
            doc["ask"] = ask;
        }
        serde_json::from_value(doc).unwrap()
    }

    fn turn() -> Value {
        json!([
            {"t": "user", "text": "antes"},
            {"t": "text", "text": "vieja"},
            {"t": "result", "ms": 1, "tools": 0, "cost": 0},
            {"t": "user", "text": "hola"},
            {"t": "text", "text": "primera"},
            {"t": "text", "text": "la última"},
            {"t": "result", "ms": 1, "tools": 0, "cost": 0},
        ])
    }

    #[test]
    fn a_turn_is_over_when_it_is_idle_with_its_own_result_and_the_answer_is_its_last_text() {
        let end = turn_end;
        let done = conversation("idle", 10, turn(), Value::Null, &[]);
        // the items before the baseline (13) are not this turn's: the first `hola` is item 13
        assert_eq!(end(&done, 13, "hola"), Some(Ok("la última".into())));
        // the daemon writes down the text trimmed
        assert_eq!(end(&done, 13, " hola\n"), Some(Ok("la última".into())));
        // a message that is not there yet (the document is older than the send) is not over
        assert_eq!(end(&done, 17, "hola"), None);
        assert_eq!(end(&done, 13, "otra cosa"), None);
        // an old result does not end a turn that waits in the queue or is still going
        assert_eq!(
            end(
                &conversation("idle", 10, turn(), Value::Null, &["hola"]),
                13,
                "hola"
            ),
            None
        );
        assert_eq!(
            end(
                &conversation("thinking", 10, turn(), Value::Null, &[]),
                13,
                "hola"
            ),
            None
        );
        assert!(matches!(
            end(
                &conversation("dead", 10, turn(), Value::Null, &[]),
                13,
                "hola"
            ),
            Some(Err(Fail::Error(_)))
        ));
    }

    #[test]
    fn a_question_that_waits_ends_the_wait_with_what_it_asks_and_a_command_ends_with_its_lines() {
        let ask = json!({
            "id": "a1", "kind": "question", "tool": "AskUserQuestion", "title": "¿Cuál?",
            "options": [{"key": "1", "label": "uno"}, {"key": "2", "label": "dos"}],
        });
        let c = conversation("asking", 0, json!([{"t": "user", "text": "x"}]), ask, &[]);
        let Some(Err(Fail::Waiting(text))) = turn_end(&c, 0, "x") else {
            panic!("a question waits");
        };
        // the last row is the person's own words (REQ-753)
        assert_eq!(
            text,
            "? pregunta: ¿Cuál?\n  1) uno\n  2) dos\n  3) Otra… (escribe tu respuesta)"
        );
        let c = conversation(
            "idle",
            0,
            json!([{"t": "out", "lines": ["a", "b"]}]),
            Value::Null,
            &[],
        );
        assert_eq!(turn_end(&c, 0, "/cost"), Some(Ok("a\nb".into())));
    }

    #[test]
    fn read_of_a_conversation_prints_its_last_items_and_what_waits() {
        let c = conversation("idle", 0, turn(), Value::Null, &[]);
        let text = conversation_text(&c, 3);
        assert_eq!(
            text,
            "● primera\n● la última\n✻ Listo · 0 s · 0 herramientas · $0,00"
        );
    }

    #[test]
    fn timeout_goes_with_wait_and_is_seconds_above_zero() {
        let parse = |a: &[&str]| timeout_flag(&Args::parse(a, &["timeout"], &["wait"]).unwrap());
        assert_eq!(parse(&[]), Ok(None));
        assert_eq!(parse(&["--wait"]), Ok(Some(TURN_WAIT)));
        assert_eq!(
            parse(&["--wait", "--timeout", "30"]),
            Ok(Some(Duration::from_secs(30)))
        );
        assert!(matches!(parse(&["--timeout", "30"]), Err(Fail::Usage(_))));
        assert!(matches!(
            parse(&["--wait", "--timeout", "0"]),
            Err(Fail::Usage(_))
        ));
        assert!(matches!(
            parse(&["--wait", "--timeout", "x"]),
            Err(Fail::Usage(_))
        ));
    }
}
