//! The commands an agent in a pane uses: what there is (`ls`, `pane ls`), what a pane shows
//! (`read`), what to do (`prompt`, `worktree`, `pane open`, `pane close`) and the skill that
//! teaches an agent all this (`skill`). They are clients of the daemon like the terminal
//! client, over nodes and commands that exist (spec N80, REQ-353 to REQ-370).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use contract::{
    Attention, AttentionKind, Block, Blocks, Cmd, Pane, ProjectInfo, Session, SplitDir, Workspace,
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

const SKILL: &str = include_str!("../skill/SKILL.md");

/// Why a command ended.
#[derive(Debug, PartialEq, Eq)]
enum Fail {
    /// A use the command does not accept: exit 2 with the usage.
    Usage(String),
    /// Anything else: exit 1.
    Error(String),
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
        "prompt" => {
            format!("usage: {b} prompt <workspace> [--pane <pane>] [--tag] [--now] [--] <text>")
        }
        "worktree" => format!(
            "usage: {b} worktree <name> [--in <repo>] [--branch <branch>] [--from <ref>] [--task <text>]"
        ),
        "pane" => format!(
            "usage: {b} pane ls <workspace> [--json]\n       {b} pane open <workspace> [--split right|down] [--of <pane>] [--] [<program words>...]\n       {b} pane close <workspace> <pane> [--force]"
        ),
        _ => format!("usage: {b} skill [install]"),
    }
}

/// Run the subcommand `cmd` with `args`, printing what it has to say; the exit code is 0, 1 for
/// an error and 2 for a use it does not accept (REQ-354).
pub fn run(cmd: &str, args: &[&str]) -> ExitCode {
    let done = match (cmd, args) {
        ("ls", rest) => ls(rest),
        ("read", rest) => read(rest),
        ("prompt", rest) => prompt(rest),
        ("worktree", rest) => worktree(rest),
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

/// What is known of a pane of a workspace.
#[derive(Debug, Clone, PartialEq)]
struct PaneInfo {
    id: String,
    title: String,
    running: Option<Vec<String>>,
    agent: Option<&'static str>,
    blocks: Option<String>,
    lines: Vec<String>,
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
        let c = Client::connect(&socket, "jwn-agent")
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

    fn workspaces(&mut self) -> Out<Vec<WorkspaceSummary>> {
        let root = self.c.root.clone();
        Ok(self.node::<Session>(&root)?.workspaces)
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
    let all = s.workspaces()?;
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
                "{}\t{}\t{}\t{} panes\t{}",
                w.name,
                name_of(w.kind),
                w.branch.as_deref().unwrap_or("-"),
                w.pane_count,
                most_urgent(&w.attention).map_or_else(|| "-".to_owned(), name_of),
            );
        }
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
    let panes = s.panes(&ws.id)?;
    let Some(pane) = pick_agent_pane(&a, &panes)?.or(panes.first()) else {
        return fail(format!("{} has no panes", ws.name));
    };
    if a.has("blocks") {
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

fn prompt(args: &[&str]) -> Out {
    let a = Args::parse(args, &["pane"], &["tag", "now"])?;
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
    send_prompt(&mut s, &target.id, &text, a.has("now"))?;
    println!("sent to pane {} of {}", target.id, ws.name);
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
    let a = Args::parse(args, &["in", "branch", "from", "task"], &[])?;
    let [name] = a.words()?;
    let mut s = Link::connect()?;
    let repo = repo_of(&mut s, a.value("in"))?;
    let info: ProjectInfo =
        serde_json::from_value(s.cmd(Cmd::ProjectConfig { repo: repo.clone() })?)
            .map_err(|e| Fail::Error(e.to_string()))?;
    let branch = a
        .value("branch")
        .map_or_else(|| info.branch.replace("{name}", name), str::to_owned);
    let made = s.cmd(Cmd::CreateWorktree {
        repo,
        branch,
        base: a.value("from").map(str::to_owned),
        folder: Some(name.to_owned()),
    })?;
    let (Some(id), Some(path)) = (made["workspace"].as_str(), made["path"].as_str()) else {
        return fail("the daemon did not say what it made");
    };
    let (id, path) = (id.to_owned(), path.to_owned());
    let started = s.cmd(Cmd::StartWorkspace {
        workspace: id.clone(),
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
                break Some(p.id);
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
        send_prompt(&mut s, &pane, task, false)?;
    }
    Ok(())
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

/// Where the skill goes: `skills/<name>/SKILL.md` under the Claude configuration (REQ-370).
fn skill_path(config_dir: Option<String>, home: Option<String>) -> Option<PathBuf> {
    let base = match config_dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(home.filter(|h| !h.is_empty())?).join(".claude"),
    };
    Some(base.join("skills").join(bin()).join("SKILL.md"))
}

fn skill_install() -> Out {
    let Some(path) = skill_path(
        std::env::var("CLAUDE_CONFIG_DIR").ok(),
        std::env::var("HOME").ok(),
    ) else {
        return fail("no HOME and no CLAUDE_CONFIG_DIR to install into");
    };
    let dir = path
        .parent()
        .ok_or_else(|| Fail::Error("no folder".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| Fail::Error(format!("{}: {e}", dir.display())))?;
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, skill_text())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    println!("installed {}", path.display());
    Ok(())
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

    fn pane(id: &str, title: &str, agent: Option<&'static str>) -> PaneInfo {
        PaneInfo {
            id: id.into(),
            title: title.into(),
            running: None,
            agent,
            blocks: None,
            lines: Vec::new(),
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

    #[test]
    fn the_folder_of_a_path_is_its_last_part() {
        assert_eq!(folder_of("/code/app-wt/login"), "login");
        assert_eq!(folder_of("/code/app/"), "app");
    }
}
