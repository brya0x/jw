//! The engine of a conversation: Claude Code run headless, its threads and the commands that
//! reach it (spec: *The conversation*, REQ-371 and following; the messages are in
//! `docs/claude-engine.md`).
//!
//! The engine is the daemon's: it goes on when no client is connected. Nothing here writes to it
//! or reads from it with the state lock held (`server.rs`: never across I/O): each engine has a
//! thread that writes what a channel hands it, a thread that reads its output and takes the lock
//! only to apply one message, and a thread that asks for a refresh at most every 50 ms while text
//! streams.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use contract::conversation::{Accent, Image, Mode};
use contract::{ErrorCode, Id};
use serde_json::{Value, json};

use crate::blocks::now_ms;
use crate::commands::{Done, err};
use crate::conversation::{Effect, OpenArgs, Saved, State, parse_args};
use crate::model::new_id;
use crate::server::Shared;

/// How often a conversation is written to disk at most (REQ-392).
const SAVE_EVERY: Duration = Duration::from_secs(2);
/// How often the transcripts of the conversations with no engine are looked at (REQ-545).
const FOLLOW_EVERY: Duration = Duration::from_secs(1);
/// How often the text that streams is shown at most (REQ-378).
const SHOW_EVERY: Duration = Duration::from_millis(50);
/// A patch of a text that grows carries the whole text (the patches only replace), so the longer
/// it is the less often it is shown: one more `SHOW_EVERY` for each `SHOW_BYTES`, up to a second.
/// A 48 KB answer sent 2.7 MB to a client at 20 patches a second; now it sends a twelfth of that.
const SHOW_BYTES: usize = 4096;
const SHOW_SLOWEST: u32 = 20;

fn show_pause(live_len: usize) -> Duration {
    SHOW_EVERY * (1 + (live_len / SHOW_BYTES) as u32).min(SHOW_SLOWEST)
}
/// How long a `!command` may run (REQ-394).
const SHELL_SECONDS: u64 = 60;
const SHELL_BYTES: usize = 64 * 1024;
/// A picture no larger than 5 MiB, as base64.
const IMAGE_CHARS: usize = 7 * 1024 * 1024;
const MAX_IMAGES: usize = 4;
const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];

/// One conversation of a pane: its state and, while it runs, its engine.
pub struct Conv {
    pub id: Id,
    pub st: State,
    pub engine: Option<Engine>,
    /// Its mode is the person's choice, so a new conversation starts in it (REQ-578): not so for
    /// the agent of a review, which is opened in the mode that its workspace needs (REQ-585).
    remember: bool,
    /// The process of the engine started last, kept when it ends: it is not another `claude` on
    /// the session while it is still ending (REQ-575).
    engine_pid: Option<u32>,
    /// Counts up at each start, so a thread of an engine that was replaced is ignored.
    pub generation: u64,
    pub cwd: PathBuf,
    /// The environment `claude` was typed in; the engine runs with it.
    pub env: Option<BTreeMap<String, String>>,
    /// `--add-dir`, `--settings` and `--agent`, handed to every engine of this conversation.
    pub extra: Vec<String>,
    /// What the person asked for with `--model` and `--effort` or `conversation_set`.
    pub model_arg: Option<String>,
    pub effort_arg: Option<String>,
    pub dirty: bool,
    pub last_saved: Option<Instant>,
    /// The transcript this conversation is read from while it has no engine (REQ-545): what is added
    /// to it, by another `claude` that has the session open, shows here.
    pub source: Option<crate::transcript::Source>,
    /// The binary's `claude` was seen in the pane's foreground: when it leaves, so does this.
    pub stub_seen: bool,
}

enum Out {
    Line(String),
    Close,
}

/// A running engine: where to write to it, and which process to end.
pub struct Engine {
    tx: Sender<Out>,
    pid: u32,
    /// Its process ended and was reaped: its group is not signalled any more (the number may be
    /// somebody else's by now).
    exited: Arc<AtomicBool>,
}

impl Engine {
    fn write(&self, v: &Value) {
        let _ = self.tx.send(Out::Line(v.to_string()));
    }

    /// Close its input (it ends by itself) and, if it is still there a moment later, end its
    /// process group: the tools it started die with it.
    pub fn stop(&self) {
        let _ = self.tx.send(Out::Close);
        let (pid, exited) = (self.pid, self.exited.clone());
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(3));
            if !exited.load(Ordering::Relaxed) {
                kill_group(pid);
            }
        });
    }
}

/// End a process group by the number the daemon recorded when it started it, and nothing else.
fn kill_group(pgid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-s", "KILL", "--", &format!("-{pgid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The time of day, for the line that says the daemon restarted.
fn clock() -> String {
    Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Whether `claude` typed with these arguments is one the engine serves (REQ-372, REQ-373).
pub fn served(args: &[String]) -> bool {
    parse_args(args).is_ok()
}

// ---- finding and starting the engine ----

fn find_in_path(name: &str, path: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn resolve_claude(
    env: Option<&BTreeMap<String, String>>,
    cfg_env: &[(String, String)],
) -> Option<PathBuf> {
    let path = env
        .and_then(|e| e.get("PATH").cloned())
        .or_else(|| {
            cfg_env
                .iter()
                .rev()
                .find(|(k, _)| k == "PATH")
                .map(|(_, v)| v.clone())
        })
        .or_else(|| std::env::var("PATH").ok())?;
    find_in_path("claude", &path)
}

struct Plan {
    exe: PathBuf,
    args: Vec<String>,
    env: Option<BTreeMap<String, String>>,
    cfg_env: Vec<(String, String)>,
    cwd: PathBuf,
}

/// What a conversation's engine is started with, taken under the lock; the program itself is
/// looked for on disk afterwards, without it.
struct Launch {
    args: Vec<String>,
    env: Option<BTreeMap<String, String>>,
    cwd: PathBuf,
}

/// The engine proposes what the person writes next, after a turn (REQ-572), unless the arguments
/// it was opened with say otherwise (`jwn claude` adds `--prompt-suggestions false` when
/// `config.toml` says so): then only that one is passed, whatever order `claude` reads them in.
fn suggestions_args(extra: &[String]) -> Vec<String> {
    if extra
        .iter()
        .any(|a| a == "--prompt-suggestions" || a.starts_with("--prompt-suggestions="))
    {
        Vec::new()
    } else {
        vec!["--prompt-suggestions".to_owned(), "true".to_owned()]
    }
}

fn launch_of(c: &Conv) -> Launch {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(str::to_owned)
    .into();
    // The transcript exists once the first turn ended: `--resume` needs it, `--session-id` makes it.
    args.extend([
        if c.st.has_transcript {
            "--resume"
        } else {
            "--session-id"
        }
        .to_owned(),
        c.st.session.clone(),
        "--permission-mode".into(),
        c.st.mode.flag().to_owned(),
    ]);
    if let Some(m) = &c.model_arg {
        args.extend(["--model".into(), m.clone()]);
    }
    if let Some(e) = &c.effort_arg {
        args.extend(["--effort".into(), e.clone()]);
    }
    if c.st.named {
        args.extend(["--name".into(), c.st.name.clone()]);
    }
    args.extend(suggestions_args(&c.extra));
    args.extend(c.extra.iter().cloned());
    Launch {
        args,
        env: c.env.clone(),
        cwd: c.cwd.clone(),
    }
}

fn is_delta(msg: &Value) -> bool {
    msg["type"] == "stream_event" && msg["event"]["type"] == "content_block_delta"
}

/// What the thread that reads an engine's output needs.
struct Feed {
    shared: Shared,
    pane: Id,
    generation: u64,
    pid: u32,
    tail: Arc<Mutex<VecDeque<String>>>,
    dirty: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    /// The length of the text that is arriving, for the pace of showing it.
    live_len: Arc<AtomicUsize>,
}

fn start(shared: &Shared, pane: &Id, generation: u64, plan: Plan) -> Result<Engine, String> {
    let mut cmd = Command::new(&plan.exe);
    cmd.args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    match &plan.env {
        Some(env) => {
            cmd.env_clear().envs(env);
        }
        None => {
            cmd.envs(plan.cfg_env.iter().cloned());
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", plan.exe.display()))?;
    let pid = child.id();
    let stdin = child.stdin.take().ok_or("no input")?;
    let stdout = child.stdout.take().ok_or("no output")?;
    let stderr = child.stderr.take().ok_or("no error output")?;

    let (tx, rx) = mpsc::channel::<Out>();
    thread::Builder::new()
        .name("engine-in".into())
        .spawn(move || {
            let mut stdin = stdin;
            for out in rx {
                match out {
                    Out::Line(l) => {
                        if writeln!(stdin, "{l}").and_then(|()| stdin.flush()).is_err() {
                            break;
                        }
                    }
                    Out::Close => break,
                }
            }
        })
        .map_err(|e| e.to_string())?;

    let feed = Feed {
        shared: shared.clone(),
        pane: pane.clone(),
        generation,
        pid,
        tail: Arc::default(),
        dirty: Arc::new(AtomicBool::new(false)),
        alive: Arc::new(AtomicBool::new(true)),
        exited: Arc::new(AtomicBool::new(false)),
        live_len: Arc::new(AtomicUsize::new(0)),
    };
    let exited = feed.exited.clone();
    let tail = feed.tail.clone();
    thread::Builder::new()
        .name("engine-err".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut q = tail.lock().unwrap_or_else(|p| p.into_inner());
                q.push_back(line);
                if q.len() > 20 {
                    q.pop_front();
                }
            }
        })
        .map_err(|e| e.to_string())?;
    show_loop(&feed)?;
    thread::Builder::new()
        .name("engine-out".into())
        .spawn(move || read_loop(&feed, child, stdout))
        .map_err(|e| e.to_string())?;
    Ok(Engine { tx, pid, exited })
}

/// Show the text that streams at most every `SHOW_EVERY`: the reader marks it and this refreshes.
fn show_loop(feed: &Feed) -> Result<(), String> {
    let (shared, pane, dirty, alive, live_len, generation) = (
        feed.shared.clone(),
        feed.pane.clone(),
        feed.dirty.clone(),
        feed.alive.clone(),
        feed.live_len.clone(),
        feed.generation,
    );
    thread::Builder::new()
        .name("engine-show".into())
        .spawn(move || {
            while alive.load(Ordering::Relaxed) {
                thread::sleep(show_pause(live_len.load(Ordering::Relaxed)));
                if dirty.swap(false, Ordering::Relaxed) {
                    let mut core = shared.lock();
                    if core
                        .convs
                        .get(&pane)
                        .is_some_and(|c| c.generation == generation)
                    {
                        core.refresh();
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn read_loop(feed: &Feed, mut child: Child, stdout: ChildStdout) {
    let (shared, pane, generation) = (&feed.shared, &feed.pane, feed.generation);
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let delta = is_delta(&msg);
        let mut core = shared.lock();
        let fx = match core
            .convs
            .get_mut(pane)
            .filter(|c| c.generation == generation)
        {
            Some(c) => {
                c.dirty = true;
                let fx = c.st.apply(&msg, now_ms());
                feed.live_len.store(c.st.live_len(), Ordering::Relaxed);
                fx
            }
            None => break,
        };
        run_effects(&mut core, pane, fx);
        if delta {
            feed.dirty.store(true, Ordering::Relaxed);
        } else {
            core.refresh();
        }
        drop(core);
        // A turn that ended is on disk at once: a daemon stopped by a signal has no last save.
        if msg["type"] == "result" {
            save_due(shared, true);
        }
    }
    feed.alive.store(false, Ordering::Relaxed);
    let status = child.wait().ok();
    // What its tools left behind goes with it; the number is not signalled again afterwards.
    kill_group(feed.pid);
    feed.exited.store(true, Ordering::Relaxed);
    let how = match status.and_then(|s| s.code()) {
        Some(0) => "el motor terminó".to_owned(),
        Some(c) => format!("el motor terminó con código {c}"),
        None => "el motor se detuvo".to_owned(),
    };
    let stderr_tail = feed
        .tail
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .rev()
        .take(3)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    let reason = if stderr_tail.is_empty() {
        how
    } else {
        format!("{how}: {stderr_tail}")
    };
    let mut core = shared.lock();
    let Some(c) = core
        .convs
        .get_mut(pane)
        .filter(|c| c.generation == generation)
    else {
        return;
    };
    c.engine = None;
    c.dirty = true;
    let fx = c.st.engine_gone(&reason);
    run_effects(&mut core, pane, fx);
    core.refresh();
    drop(core);
    save_due(shared, true);
}

/// Do what a message asked, under the lock and without waiting.
fn run_effects(core: &mut crate::core::Core, pane: &Id, fx: Vec<Effect>) {
    for e in fx {
        match e {
            Effect::Write(v) => {
                if let Some(en) = core.convs.get(pane).and_then(|c| c.engine.as_ref()) {
                    en.write(&v);
                }
            }
            Effect::Raise(kind) => core.raise_agent(pane, kind),
            Effect::ClearAsk => core.clear_ask(pane),
            Effect::Resumable(session) => core.set_pane_resume(
                pane,
                Some(vec!["claude".into(), "--resume".into(), session]),
            ),
        }
    }
}

/// Start the engine of a conversation that has none (it is new, or its daemon restarted).
fn start_engine(shared: &Shared, pane: &Id) -> Result<(), crate::commands::ErrorBody> {
    // A session that another `claude` has open is not written to by a second one (REQ-575).
    let (env, session, own) = {
        let core = shared.lock();
        let c = core
            .convs
            .get(pane)
            .ok_or_else(|| err(ErrorCode::NotFound, "no conversation in this pane"))?;
        (c.env.clone(), c.st.session.clone(), c.engine_pid)
    };
    if let Some(h) = crate::sessions::holder(env.as_ref(), &session, own) {
        {
            let mut core = shared.lock();
            if let Some(c) = core.convs.get_mut(pane) {
                c.st.set_held(Some(h.clone()));
            }
            core.refresh();
        }
        return Err(err(
            ErrorCode::Conflict,
            format!(
                "otro claude (pid {}, en {}) tiene esta sesión abierta: ciérralo para escribir aquí",
                h.pid, h.cwd
            ),
        ));
    }
    let (launch, cfg_env, generation) = {
        let mut core = shared.lock();
        let cfg_env = core.env.clone();
        let c = core
            .convs
            .get_mut(pane)
            .ok_or_else(|| err(ErrorCode::NotFound, "no conversation in this pane"))?;
        c.generation += 1;
        c.st.set_held(None);
        (launch_of(c), cfg_env, c.generation)
    };
    let exe = resolve_claude(launch.env.as_ref(), &cfg_env)
        .ok_or_else(|| err(ErrorCode::Io, "claude is not on the PATH it was typed in"))?;
    let plan = Plan {
        exe,
        args: launch.args,
        env: launch.env,
        cfg_env,
        cwd: launch.cwd,
    };
    let engine = start(shared, pane, generation, plan).map_err(|m| err(ErrorCode::Io, m))?;
    let mut core = shared.lock();
    match core
        .convs
        .get_mut(pane)
        .filter(|c| c.generation == generation)
    {
        Some(c) => {
            c.engine_pid = Some(engine.pid);
            c.engine = Some(engine);
            // Its own engine writes the transcript now: what it says is live.
            c.source = None;
            let wanted = c.model_arg.clone();
            let fx = c.st.engine_started(wanted.as_deref());
            run_effects(&mut core, pane, fx);
            core.refresh();
        }
        None => engine.stop(),
    }
    Ok(())
}

// ---- the commands ----

fn no_conversation(node: &str) -> crate::commands::ErrorBody {
    err(ErrorCode::NotFound, format!("no conversation {node}"))
}

/// The pane whose conversation is the node `node`.
fn pane_of(core: &crate::core::Core, node: &str) -> Result<Id, crate::commands::ErrorBody> {
    core.convs
        .iter()
        .find(|(_, c)| c.id == node)
        .map(|(p, _)| p.clone())
        .ok_or_else(|| no_conversation(node))
}

fn pwd_of(env: Option<&BTreeMap<String, String>>) -> Option<PathBuf> {
    env.and_then(|e| e.get("PWD"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Claude's own folder of transcripts for a working folder: where `--continue` looks.
pub(crate) fn projects_dir(cwd: &Path, env: Option<&BTreeMap<String, String>>) -> Option<PathBuf> {
    let base = crate::settings::claude_dir(env)?;
    // The folder is Claude's own naming of the path: `/` and `.` become `-`, and a `_` or a space may
    // too (it is not documented which), so the one that exists is used.
    let named = |keep: fn(char) -> bool| -> PathBuf {
        let encoded: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if keep(c) { c } else { '-' })
            .collect();
        base.join("projects").join(encoded)
    };
    let (dots, letters) = (
        named(|c| c != '/' && c != '.'),
        named(|c| c.is_ascii_alphanumeric()),
    );
    Some(if letters.is_dir() && !dots.is_dir() {
        letters
    } else {
        dots
    })
}

fn latest_session(cwd: &Path, env: Option<&BTreeMap<String, String>>) -> Option<String> {
    let dir = projects_dir(cwd, env)?;
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .and_then(|(_, p)| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
}

pub fn open_conversation(
    shared: &Shared,
    pane: &Id,
    resume: Option<String>,
    args: Vec<String>,
    env: Option<BTreeMap<String, String>>,
) -> Done {
    let parsed: OpenArgs = parse_args(&args).map_err(|m| err(ErrorCode::Unsupported, m))?;
    let (pane_cwd, dir, old, review) = {
        let mut core = shared.lock();
        let cwd = core
            .pane_cwd(pane)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no pane {pane}")))?;
        let review = core.review_of_pane(pane);
        (cwd, core.conv_dir.clone(), core.convs.remove(pane), review)
    };
    if let Some(en) = old.and_then(|o| o.engine) {
        en.stop();
    }
    let cwd = pwd_of(env.as_ref()).unwrap_or(pane_cwd);
    let resume = resume.or(parsed.resume.clone()).or_else(|| {
        parsed
            .continue_latest
            .then(|| latest_session(&cwd, env.as_ref()))
            .flatten()
    });
    let mut model_arg = parsed.model.clone();
    let mut effort_arg = parsed.effort.clone();
    // What the engine said before is read from its own transcript, not copied (REQ-544).
    let mut source = resume
        .as_deref()
        .and_then(|id| crate::transcript::Source::open(&cwd, env.as_ref(), id));
    let (mut st, lazy) = match &resume {
        Some(id) => {
            let saved = dir
                .as_deref()
                .and_then(|d| load(d, pane))
                .filter(|s| s.session == *id);
            let st = match saved {
                Some(s) => {
                    // What it was asked to run with comes back with it, unless typed again.
                    model_arg = model_arg.or_else(|| s.model_arg.clone());
                    effort_arg = effort_arg.or_else(|| s.effort_arg.clone());
                    let mut st = State::restored(s, &clock());
                    // A file of before the transcript was read keeps its own items.
                    if st.legacy() {
                        source = None;
                    } else {
                        // Without a transcript what jwn kept is still shown.
                        let parser = source.as_ref().map(|s| &s.parser);
                        st.follow(parser.unwrap_or(&crate::transcript::Parser::default()));
                    }
                    st
                }
                None => {
                    if let Some(src) = &source {
                        model_arg = model_arg.or_else(|| src.parser.model.clone());
                        effort_arg = effort_arg.or_else(|| src.parser.effort.clone());
                    }
                    State::resumed(id.clone(), source.as_ref().map(|s| &s.parser))
                }
            };
            (st, true)
        }
        None => {
            let mut st = State::new(parsed.session_id.clone().unwrap_or_else(new_id));
            // A new conversation starts in the mode the person last worked in (REQ-578).
            if let Some(mode) = dir.as_deref().and_then(last_mode) {
                st.mode = mode;
            }
            (st, false)
        }
    };
    if let Some(n) = parsed.name.clone().filter(|n| !n.trim().is_empty()) {
        st.name = n;
        st.named = true;
    }
    if let Some(m) = parsed.mode {
        st.mode = m;
    }
    if let Some(m) = &parsed.model {
        st.model = m.clone();
    }
    if let Some(e) = &parsed.effort {
        st.effort = e.clone();
    }
    st.set_advisor(crate::settings::advisor_label(env.as_ref(), &cwd));
    // The agent of a review is not briefed: it is offered a first question and waits (REQ-585).
    if let (Some((pr, file)), None) = (&review, &resume) {
        st.suggest(format!(
            "Lee {} y dime los riesgos del PR #{pr}",
            file.display()
        ));
    }
    if lazy {
        st.set_held(crate::sessions::holder(env.as_ref(), &st.session, None));
    }
    let mut extra: Vec<String> = Vec::new();
    for d in &parsed.add_dirs {
        extra.extend(["--add-dir".into(), d.clone()]);
    }
    if let Some(s) = &parsed.settings {
        extra.extend(["--settings".into(), s.clone()]);
    }
    if let Some(a) = &parsed.agent {
        extra.extend(["--agent".into(), a.clone()]);
    }
    let node = new_id();
    let session = st.session.clone();
    {
        let mut core = shared.lock();
        if st.has_transcript {
            core.set_pane_resume(
                pane,
                Some(vec!["claude".into(), "--resume".into(), session.clone()]),
            );
        }
        core.convs.insert(
            pane.clone(),
            Conv {
                id: node.clone(),
                st,
                engine: None,
                remember: review.is_none(),
                engine_pid: None,
                generation: 0,
                cwd,
                env,
                extra,
                model_arg,
                effort_arg,
                dirty: true,
                last_saved: None,
                source,
                stub_seen: false,
            },
        );
        core.refresh();
    }
    if !lazy && let Err(e) = start_engine(shared, pane) {
        shared.lock().convs.remove(pane);
        return Err(e);
    }
    if let Some(message) = parsed.message {
        send(shared, &node, &message, Vec::new())?;
    }
    Ok(Some(json!({ "node": node, "session": session })))
}

fn check_images(images: &[Image]) -> Result<(), crate::commands::ErrorBody> {
    if images.len() > MAX_IMAGES {
        return Err(err(
            ErrorCode::BadRequest,
            format!("at most {MAX_IMAGES} images in a message"),
        ));
    }
    for i in images {
        if !IMAGE_TYPES.contains(&i.media_type.as_str()) {
            return Err(err(
                ErrorCode::BadRequest,
                "images are JPEG, PNG, WebP or GIF",
            ));
        }
        if i.data.len() > IMAGE_CHARS {
            return Err(err(ErrorCode::BadRequest, "an image is larger than 5 MiB"));
        }
    }
    Ok(())
}

pub fn send(shared: &Shared, node: &str, text: &str, images: Vec<Image>) -> Done {
    check_images(&images)?;
    let line = text.trim();
    if line.is_empty() && images.is_empty() {
        return Err(err(ErrorCode::BadRequest, "nothing to send"));
    }
    // What jwn answers itself, so every client and `prompt` behave the same (REQ-418, REQ-419).
    if let Some(rest) = line.strip_prefix('/') {
        let (cmd, arg) = rest
            .split_once(' ')
            .map_or((rest, ""), |(c, a)| (c, a.trim()));
        match cmd {
            "exit" => return close(shared, node),
            "rename" if !arg.is_empty() => {
                return set(shared, node, Some(arg.to_owned()), None, None, None, None);
            }
            "color" => {
                let accent = parse_accent(arg).ok_or_else(|| {
                    err(
                        ErrorCode::BadRequest,
                        "colours: blue, green, yellow, red, magenta, cyan",
                    )
                })?;
                return set(shared, node, None, Some(accent), None, None, None);
            }
            "model" if !arg.is_empty() => {
                return set(shared, node, None, None, Some(arg.to_owned()), None, None);
            }
            "effort" if !arg.is_empty() => {
                return set(shared, node, None, None, None, Some(arg.to_owned()), None);
            }
            "mode" => {
                let mode = Mode::from_flag(&arg.replace("accept edits", "acceptEdits"))
                    .ok_or_else(|| {
                        err(
                            ErrorCode::BadRequest,
                            "modes: manual, accept edits, plan, auto",
                        )
                    })?;
                return set(shared, node, None, None, None, None, Some(mode));
            }
            "plan" => {
                set(shared, node, None, None, None, None, Some(Mode::Plan))?;
                // What follows is the first message of the plan; it waits for the mode.
                return if arg.is_empty() {
                    Ok(None)
                } else {
                    send(shared, node, arg, images)
                };
            }
            "cost" => {
                let mut core = shared.lock();
                let pane = pane_of(&core, node)?;
                if let Some(c) = core.convs.get_mut(&pane) {
                    c.st.answer_cost();
                    c.dirty = true;
                }
                core.refresh();
                return Ok(None);
            }
            _ => {}
        }
    }
    let pane = {
        let core = shared.lock();
        pane_of(&core, node)?
    };
    if let Some(word) = line
        .strip_prefix('/')
        .and_then(|rest| rest.split_whitespace().next())
    {
        let why = shared
            .lock()
            .convs
            .get(&pane)
            .and_then(|c| c.st.refusal(word));
        if let Some(why) = why {
            return Err(err(ErrorCode::BadRequest, why));
        }
    }
    let dead = shared
        .lock()
        .convs
        .get(&pane)
        .is_some_and(|c| c.engine.is_none());
    if dead {
        {
            let mut core = shared.lock();
            if let Some(c) = core.convs.get_mut(&pane) {
                c.st.resuming();
            }
            core.refresh();
        }
        start_engine(shared, &pane)?;
    }
    let mut core = shared.lock();
    let fx = {
        let c = core
            .convs
            .get_mut(&pane)
            .ok_or_else(|| no_conversation(node))?;
        c.dirty = true;
        c.st.send(line, images, now_ms())
    };
    // Sending is moving on: what the last turn asked of the person is over.
    core.clear_agent(&pane);
    run_effects(&mut core, &pane, fx);
    core.refresh();
    drop(core);
    save_due(shared, true);
    Ok(None)
}

fn parse_accent(s: &str) -> Option<Accent> {
    Some(match s.trim() {
        "blue" | "azul" => Accent::Blue,
        "green" | "verde" => Accent::Green,
        "yellow" | "amarillo" => Accent::Yellow,
        "red" | "rojo" => Accent::Red,
        "magenta" => Accent::Magenta,
        "cyan" | "cian" => Accent::Cyan,
        _ => return None,
    })
}

pub fn answer(
    shared: &Shared,
    node: &str,
    ask: &str,
    key: &str,
    text: Option<&str>,
    by: Option<&str>,
) -> Done {
    let mut core = shared.lock();
    let pane = pane_of(&core, node)?;
    let fx = {
        let c = core
            .convs
            .get_mut(&pane)
            .ok_or_else(|| no_conversation(node))?;
        c.dirty = true;
        c.st.answer(ask, key, text, by)
            .map_err(|m| err(ErrorCode::Conflict, m))?
    };
    run_effects(&mut core, &pane, fx);
    core.refresh();
    Ok(None)
}

pub fn stop_task(shared: &Shared, node: &str, task: &str) -> Done {
    let mut core = shared.lock();
    let pane = pane_of(&core, node)?;
    let fx = core
        .convs
        .get_mut(&pane)
        .ok_or_else(|| no_conversation(node))?
        .st
        .stop_task(task)
        .map_err(|m| err(ErrorCode::BadRequest, m))?;
    run_effects(&mut core, &pane, fx);
    core.refresh();
    Ok(None)
}

pub fn interrupt(shared: &Shared, node: &str) -> Done {
    let mut core = shared.lock();
    let pane = pane_of(&core, node)?;
    let fx = core
        .convs
        .get_mut(&pane)
        .map(|c| c.st.interrupt())
        .unwrap_or_default();
    run_effects(&mut core, &pane, fx);
    core.refresh();
    Ok(None)
}

pub fn set(
    shared: &Shared,
    node: &str,
    name: Option<String>,
    accent: Option<Accent>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<Mode>,
) -> Done {
    let dead = {
        let core = shared.lock();
        let pane = pane_of(&core, node)?;
        core.convs.get(&pane).is_some_and(|c| c.engine.is_none())
    };
    // A command of the engine needs an engine: start it for a model, an effort or a name.
    let needs_engine = name.is_some() || model.is_some() || effort.is_some() || mode.is_some();
    if dead && needs_engine {
        let pane = {
            let core = shared.lock();
            pane_of(&core, node)?
        };
        start_engine(shared, &pane)?;
    }
    let mut core = shared.lock();
    let pane = pane_of(&core, node)?;
    let fx = {
        let c = core
            .convs
            .get_mut(&pane)
            .ok_or_else(|| no_conversation(node))?;
        if let Some(m) = &model {
            c.model_arg = Some(m.trim().to_owned());
        }
        if let Some(e) = &effort {
            c.effort_arg = Some(e.trim().to_owned());
        }
        c.dirty = true;
        c.st.set(name, accent, model, effort, mode)
    };
    run_effects(&mut core, &pane, fx);
    core.refresh();
    Ok(None)
}

pub fn close(shared: &Shared, node: &str) -> Done {
    let mut core = shared.lock();
    let pane = pane_of(&core, node)?;
    let conv = core.convs.remove(&pane);
    core.clear_agent(&pane);
    core.set_pane_resume(&pane, None);
    core.refresh();
    drop(core);
    if let Some(en) = conv.and_then(|c| c.engine) {
        en.stop();
    }
    Ok(None)
}

pub fn shell(shared: &Shared, node: &str, command: &str) -> Done {
    let command = command.trim();
    if command.is_empty() || command.len() > 4000 {
        return Err(err(
            ErrorCode::BadRequest,
            "a shell line of 1 to 4000 characters",
        ));
    }
    let (cwd, shell, dir) = {
        let mut core = shared.lock();
        let pane = pane_of(&core, node)?;
        let shell = core.shell.clone();
        let c = core
            .convs
            .get_mut(&pane)
            .ok_or_else(|| no_conversation(node))?;
        c.st.shell_started(command);
        c.dirty = true;
        let cwd = c.cwd.clone();
        core.refresh();
        (cwd, shell, core.conv_dir.clone())
    };
    let (shared, node, command) = (shared.clone(), node.to_owned(), command.to_owned());
    thread::Builder::new()
        .name("conv-shell".into())
        .spawn(move || {
            let (ok, output) = run_shell(&shell, &cwd, &command, dir.as_deref());
            let mut core = shared.lock();
            if let Ok(pane) = pane_of(&core, &node) {
                if let Some(c) = core.convs.get_mut(&pane) {
                    c.st.shell_done(&command, ok, output);
                    c.dirty = true;
                }
                core.refresh();
            }
        })
        .map_err(|e| err(ErrorCode::Io, e.to_string()))?;
    Ok(None)
}

/// Run a line with the person's shell, in its own process group and with no terminal, for at
/// most a minute; what it printed (both streams, in order) is what is returned, the last 64 KiB.
fn run_shell(
    shell: &str,
    cwd: &Path,
    command: &str,
    scratch: Option<&Path>,
) -> (bool, Vec<String>) {
    let base = scratch.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let _ = std::fs::create_dir_all(&base);
    let file = base.join(format!("shell-{}.out", new_id()));
    let Ok(out) = std::fs::File::create(&file) else {
        return (false, vec!["no se pudo preparar la salida".into()]);
    };
    let Ok(err_out) = out.try_clone() else {
        return (false, vec!["no se pudo preparar la salida".into()]);
    };
    let spawned = Command::new(shell)
        .args(["-c", command])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err_out))
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            return (false, vec![format!("no se pudo ejecutar: {e}")]);
        }
    };
    let pid = child.id();
    let started = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) if started.elapsed() > Duration::from_secs(SHELL_SECONDS) => {
                kill_group(pid);
                let _ = child.wait();
                break false;
            }
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(_) => break false,
        }
    };
    let mut bytes = Vec::new();
    if let Ok(f) = std::fs::File::open(&file) {
        let _ = f.take(SHELL_BYTES as u64).read_to_end(&mut bytes);
    }
    let _ = std::fs::remove_file(&file);
    let mut lines: Vec<String> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::to_owned)
        .collect();
    if started.elapsed() > Duration::from_secs(SHELL_SECONDS) {
        lines.push(format!("(cortado a los {SHELL_SECONDS} s)"));
    }
    (ok, lines)
}

/// Read what was added to the transcripts of the conversations that have no engine, and show it
/// (REQ-545). What is read is outside the lock; what changes the node is inside.
pub fn follow_due(shared: &Shared) {
    type Taken = (
        Id,
        Id,
        crate::transcript::Source,
        Option<BTreeMap<String, String>>,
        String,
        Option<u32>,
    );
    let taken: Vec<Taken> = shared
        .lock()
        .convs
        .iter_mut()
        .filter(|(_, c)| c.engine.is_none())
        .filter_map(|(pane, c)| {
            let (env, session, own) = (c.env.clone(), c.st.session.clone(), c.engine_pid);
            c.source
                .take()
                .map(|s| (pane.clone(), c.id.clone(), s, env, session, own))
        })
        .collect();
    if taken.is_empty() {
        return;
    }
    let polled: Vec<_> = taken
        .into_iter()
        .map(|(pane, node, mut src, env, session, own)| {
            let changed = src.poll();
            // Whether another `claude` has the session open is looked at with the transcript.
            let held = crate::sessions::holder(env.as_ref(), &session, own);
            (pane, node, src, changed, held)
        })
        .collect();
    let mut core = shared.lock();
    let mut shown = false;
    for (pane, node, src, changed, held) in polled {
        // A conversation that started its engine meanwhile, was opened again (another session in
        // the pane has another node) or is gone is not followed with this.
        let Some(c) = core
            .convs
            .get_mut(&pane)
            .filter(|c| c.engine.is_none() && c.id == node)
        else {
            continue;
        };
        if changed {
            c.st.follow(&src.parser);
            shown = true;
        } else if c.st.expire_follow(crate::conversation::stamp()) {
            shown = true;
        }
        shown |= c.st.set_held(held);
        c.source = Some(src);
    }
    if shown {
        core.refresh();
    }
}

/// Follow the transcripts until the daemon stops.
pub fn follow_run(shared: Shared, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        follow_due(&shared);
        let end = Instant::now() + FOLLOW_EVERY;
        while Instant::now() < end && !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `read_older`: a page of the items of the conversation `node` that came before the time
/// `before`, from its transcript, with the items of jwn's own among them (REQ-567).
pub fn read_older(shared: &Shared, node: &str, before: u64, count: u32) -> Done {
    let (cwd, env, session, kept) = {
        let core = shared.lock();
        let pane = pane_of(&core, node)?;
        let c = core.convs.get(&pane).ok_or_else(|| no_conversation(node))?;
        if !c.st.leaves_chat_to_transcript() {
            return Err(err(
                ErrorCode::NotFound,
                "that conversation has no transcript to read",
            ));
        }
        (
            c.cwd.clone(),
            c.env.clone(),
            c.st.session.clone(),
            c.st.kept(),
        )
    };
    let path = crate::transcript::Source::path_of(&cwd, env.as_ref(), &session)
        .ok_or_else(|| err(ErrorCode::NotFound, "no transcript"))?;
    let count = count.clamp(1, 500) as usize;
    let (page, older) = crate::transcript::older_page(&path, before, count)
        .ok_or_else(|| err(ErrorCode::Io, "the transcript could not be read"))?;
    // jwn's own items that were made inside the time the page covers.
    let from = page.first().map_or(before, |s| s.at);
    let kept: Vec<_> = kept
        .into_iter()
        .filter(|k| k.at >= from && k.at < before)
        .collect();
    let (items, _) = crate::conversation::merge(&page, &kept);
    Ok(Some(
        serde_json::to_value(contract::conversation::OlderItems { items, older })
            .expect("items serialise"),
    ))
}

// ---- saved conversations ----

fn file_of(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{pane}.json"))
}

fn load(dir: &Path, pane: &str) -> Option<Saved> {
    let text = std::fs::read_to_string(file_of(dir, pane)).ok()?;
    let saved: Saved = serde_json::from_str(&text).ok()?;
    (saved.v == crate::conversation::SAVED_VERSION).then_some(saved)
}

/// Where to look for a conversation's transcript: its folder, its environment and its session.
type Probe = (PathBuf, Option<BTreeMap<String, String>>, String);

/// The file that says the mode a new conversation starts in, beside the folder of the saved
/// ones (a file inside it would be taken for a conversation).
fn last_mode_file(dir: &Path) -> Option<PathBuf> {
    Some(dir.parent()?.join("last-mode"))
}

fn last_mode(dir: &Path) -> Option<Mode> {
    let text = std::fs::read_to_string(last_mode_file(dir)?).ok()?;
    Mode::from_flag(text.trim())
}

fn write_last_mode(dir: &Path, mode: Mode) {
    let Some(file) = last_mode_file(dir) else {
        return;
    };
    if last_mode(dir) != Some(mode) {
        let _ = std::fs::write(file, mode.flag());
    }
}

/// What is written of a conversation: its file, what it saves and where its transcript is.
type Due = (PathBuf, Saved, Option<Probe>);

/// The conversations that changed and are due are written without the lock: only what is
/// to be written is copied under it.
pub fn save_due(shared: &Shared, force: bool) {
    let (due, chosen): (Vec<Due>, Option<(PathBuf, Mode)>) = {
        let mut core = shared.lock();
        let Some(dir) = core.conv_dir.clone() else {
            return;
        };
        let now = now_ms();
        let is_due =
            |c: &Conv| c.dirty && (force || c.last_saved.is_none_or(|t| t.elapsed() >= SAVE_EVERY));
        // The mode of a conversation that has an engine is the person's choice (or Claude's,
        // when a plan is approved); one that only follows a transcript says nothing of it.
        let chosen = core
            .convs
            .values()
            .filter(|c| c.engine.is_some() && c.remember && is_due(c))
            .map(|c| (dir.clone(), c.st.mode))
            .last();
        let due = core
            .convs
            .iter_mut()
            .filter(|(_, c)| is_due(c))
            .map(|(pane, c)| {
                c.dirty = false;
                c.last_saved = Some(Instant::now());
                let mut saved = c.st.saved(now);
                saved.model_arg.clone_from(&c.model_arg);
                saved.effort_arg.clone_from(&c.effort_arg);
                let probe =
                    c.st.leaves_chat_to_transcript()
                        .then(|| (c.cwd.clone(), c.env.clone(), c.st.session.clone()));
                (file_of(&dir, pane), saved, probe)
            })
            .collect();
        (due, chosen)
    };
    if let Some((dir, mode)) = chosen {
        write_last_mode(&dir, mode);
    }
    for (path, mut saved, probe) in due {
        // The chat is not copied when the engine's transcript is there to read it from (REQ-544).
        match probe {
            Some((cwd, env, session))
                if crate::transcript::Source::exists(&cwd, env.as_ref(), &session) =>
            {
                saved.items.clear();
                saved.first = 0;
            }
            _ => saved.kept.clear(),
        }
        let Some(text) = encode(saved) else {
            continue;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_err()
        {
            eprintln!("cannot save the conversation {}", path.display());
        }
    }
}

/// The saved document, with the oldest items dropped until it takes at most 2 MiB (REQ-392): the
/// chat's while it has one, else the oldest of what is jwn's own.
fn encode(mut saved: Saved) -> Option<String> {
    loop {
        let text = serde_json::to_string(&saved).ok()?;
        if text.len() <= crate::conversation::MAX_SAVED
            || (saved.items.len() <= 1 && saved.kept.len() <= 1)
        {
            return Some(text);
        }
        if saved.items.len() > 1 {
            let drop = (saved.items.len() / 4).max(1);
            saved.items.drain(..drop);
            saved.first += drop as u32;
        } else {
            let drop = (saved.kept.len() / 4).max(1);
            saved.kept.drain(..drop);
        }
    }
}

/// Forget the saved conversation of a pane that is gone.
pub fn forget(dir: &Path, pane: &str) {
    let _ = std::fs::remove_file(file_of(dir, pane));
}

/// End every engine (the daemon is stopping).
pub fn stop_all(convs: &mut std::collections::HashMap<Id, Conv>) {
    for c in convs.values_mut() {
        if let Some(en) = c.engine.take() {
            // Its threads ignore what follows: a daemon that stops is not an engine that died.
            c.generation += 1;
            en.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longer_the_text_that_arrives_the_less_often_it_is_shown() {
        assert_eq!(show_pause(0), SHOW_EVERY);
        assert_eq!(show_pause(SHOW_BYTES - 1), SHOW_EVERY);
        assert_eq!(show_pause(SHOW_BYTES), SHOW_EVERY * 2);
        assert_eq!(show_pause(48 * 1024), SHOW_EVERY * 13);
        // never slower than a second, however long it is
        assert_eq!(show_pause(10 * 1024 * 1024), SHOW_EVERY * SHOW_SLOWEST);
    }

    /// REQ-572: the engine is asked for suggestions once, and `false` from the arguments it was
    /// opened with is the only one passed when there is one.
    #[test]
    fn the_engine_is_asked_for_suggestions_once_and_the_opening_arguments_can_turn_them_off() {
        assert_eq!(suggestions_args(&[]), ["--prompt-suggestions", "true"]);
        assert_eq!(
            suggestions_args(&["--model".into(), "opus".into()]),
            ["--prompt-suggestions", "true"]
        );
        let off = ["--prompt-suggestions".to_owned(), "false".to_owned()];
        assert!(suggestions_args(&off).is_empty());
        assert!(suggestions_args(&["--prompt-suggestions=false".to_owned()]).is_empty());
    }
}
