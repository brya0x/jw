//! `jwn claude`: what typing `claude` in a pane of the daemon runs when the shell integration
//! defines the function (spec REQ-371 to REQ-373, REQ-395).
//!
//! It asks the daemon for the pane's conversation, stays in the foreground while it lasts and
//! prints a plain version of it into the pane's own terminal, so any way of looking at the pane
//! sees something; the clients that understand the `conversation` node draw it themselves. When
//! it cannot serve what was typed (another command of Claude's, a pipe, a terminal that is not
//! one, a daemon that will not) it runs the original `claude` with the same arguments.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write, stdout};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use contract::conversation::{Ask, AskKind, ConvStatus, Conversation, DiffSign, Item, ToolState};
use contract::{Cmd, DaemonMsg};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use wire::Client;

mod pick;

pub fn run(args: &[&str]) -> ExitCode {
    let typed: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let mut argv = typed.clone();
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    // `claude --resume` with no session, or with a search term, is a choice among the conversations
    // of the folder; it is made here and the conversation is opened with the one chosen (REQ-547).
    if tty
        && std::env::var_os(identity::env_var("PANE_ID")).is_some()
        && let Some(term) = pick::take_search(&mut argv)
    {
        match pick::choose(term.as_deref()) {
            Ok(Some(id)) => argv.extend(["--resume".to_owned(), id]),
            Ok(None) => return ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("{} claude: {why}; running claude", identity::get().name);
                return original(&typed);
            }
        }
    }
    if let Some(pane) = std::env::var(identity::env_var("PANE_ID"))
        .ok()
        .filter(|_| tty && daemon::conversation_served(&argv))
    {
        // The engine proposes what to write next unless `config.toml` says not to (REQ-572); an
        // argument of the person's own about it is the only one that goes.
        let mut with = argv.clone();
        let own = argv.iter().any(|a| a.starts_with("--prompt-suggestions"));
        if !own && !suggestions_on(&config_text()) {
            with.extend(["--prompt-suggestions".to_owned(), "false".to_owned()]);
        }
        match converse(&pane, &with) {
            Ok(code) => return code,
            Err(why) => eprintln!("{} claude: {why}; running claude", identity::get().name),
        }
    }
    original(&argv)
}

/// What `config.toml` says, or nothing when it cannot be read.
fn config_text() -> String {
    std::fs::read_to_string(identity::config_dir().join("config.toml")).unwrap_or_default()
}

/// Whether the engine is asked to propose what to write next: `suggestions = false` in `config.toml`
/// turns it off, and anything else, a file with no such key or one that is not TOML, leaves it on.
fn suggestions_on(config: &str) -> bool {
    config
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("suggestions").and_then(toml::Value::as_bool))
        .unwrap_or(true)
}

/// The original `claude`, in place of this process.
fn original(argv: &[String]) -> ExitCode {
    let found: Option<PathBuf> = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("claude"))
            .find(|c| c.is_file())
    });
    let Some(exe) = found else {
        eprintln!("claude: command not found");
        return ExitCode::from(127);
    };
    let e = Command::new(exe).args(argv).exec();
    eprintln!("claude: {e}");
    ExitCode::from(126)
}

/// Switches the terminal back when the view ends, however it ends.
struct Raw;

impl Raw {
    fn on() -> Option<Raw> {
        terminal::enable_raw_mode().ok().map(|()| Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

fn converse(pane: &str, argv: &[String]) -> Result<ExitCode, String> {
    let mut client =
        Client::connect(&identity::socket_path(), "jwn-claude").map_err(|e| e.to_string())?;
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let reply = client
        .cmd(Cmd::OpenConversation {
            pane: pane.to_owned(),
            resume: None,
            args: argv.to_vec(),
            env: Some(env),
        })
        .map_err(|e| e.to_string())?;
    let opened = match reply {
        Ok(Some(v)) => v,
        Ok(None) => return Err("the daemon opened nothing".into()),
        Err(e) => return Err(e.message),
    };
    let node = opened["node"]
        .as_str()
        .ok_or("no node in the reply")?
        .to_owned();
    client.open(&node).map_err(|e| e.to_string())?;
    let _raw = Raw::on().ok_or("the terminal cannot be used raw")?;
    let mut out = stdout();
    let _ = write!(
        out,
        "{} · conversación de este panel (esc interrumpe · ^D sale · /exit)\r\n",
        identity::get().name
    );
    Ok(view(&mut client, &node))
}

/// What has been printed, and what is being typed.
struct Screen {
    /// Counts items from the first of the conversation, so it survives the node dropping old ones.
    printed: u64,
    shown_ask: Option<String>,
    buffer: String,
    rev: u64,
}

fn view(client: &mut Client, node: &str) -> ExitCode {
    let mut out = stdout();
    let mut screen = Screen {
        printed: 0,
        shown_ask: None,
        buffer: String::new(),
        rev: 0,
    };
    let started = Instant::now();
    loop {
        // What the person does.
        if event::poll(Duration::from_millis(40)).unwrap_or(false)
            && let Ok(Event::Key(k)) = event::read()
            && k.kind != KeyEventKind::Release
            && let Step::Leave = handle_key(client, node, &mut screen, k.code, k.modifiers)
        {
            let _ = client.cmd(Cmd::ConversationClose {
                node: node.to_owned(),
            });
            let _ = write!(out, "\r\n");
            return ExitCode::SUCCESS;
        }
        // What the daemon says.
        loop {
            match client.poll(Duration::from_millis(1)) {
                Ok(Some(DaemonMsg::Gone { node: n })) if n == node => {
                    let _ = write!(out, "\r\x1b[2K");
                    let _ = writeln!(out, "conversación terminada\r");
                    return ExitCode::SUCCESS;
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    let _ = writeln!(out, "\r\nel daemon se cerró\r");
                    return ExitCode::FAILURE;
                }
            }
        }
        let Some(doc) = client.nodes.get(node) else {
            // The node has not arrived yet; give it a moment before giving up.
            if started.elapsed() > Duration::from_secs(5) {
                let _ = writeln!(out, "\r\nla conversación no llegó\r");
                return ExitCode::FAILURE;
            }
            continue;
        };
        let rev = doc["rev"].as_u64().unwrap_or(0);
        if rev == screen.rev && screen.printed > 0 {
            continue;
        }
        screen.rev = rev;
        let Ok(conv) = serde_json::from_value::<Conversation>(doc.clone()) else {
            continue;
        };
        draw(&mut out, &conv, &mut screen);
    }
}

enum Step {
    Go,
    Leave,
}

fn handle_key(
    client: &mut Client,
    node: &str,
    screen: &mut Screen,
    code: KeyCode,
    mods: KeyModifiers,
) -> Step {
    let busy = client
        .nodes
        .get(node)
        .is_some_and(|d| d["status"] != "idle" && d["status"] != "dead");
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Char('c') if ctrl => {
            if busy {
                let _ = client.send(Cmd::ConversationInterrupt {
                    node: node.to_owned(),
                });
            } else {
                return Step::Leave;
            }
        }
        KeyCode::Char('d') if ctrl && screen.buffer.is_empty() => return Step::Leave,
        KeyCode::Esc => {
            let _ = client.send(Cmd::ConversationInterrupt {
                node: node.to_owned(),
            });
        }
        KeyCode::Enter => {
            let line = std::mem::take(&mut screen.buffer);
            let line = line.trim().to_owned();
            if line.is_empty() {
                return Step::Go;
            }
            let ask = client
                .nodes
                .get(node)
                .and_then(|d| d["ask"].as_object().cloned());
            match ask {
                Some(a) => {
                    let options: Vec<String> = a["options"]
                        .as_array()
                        .map(|o| {
                            o.iter()
                                .filter_map(|x| x["key"].as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let key = match line.as_str() {
                        "y" | "s" => Some("yes".to_owned()),
                        "n" => Some("no".to_owned()),
                        "a" => Some("session".to_owned()),
                        d => d
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| options.get(i.wrapping_sub(1)).cloned()),
                    };
                    if let Some(key) = key {
                        let _ = client.send(Cmd::ConversationAnswer {
                            node: node.to_owned(),
                            ask: a["id"].as_str().unwrap_or("").to_owned(),
                            key,
                            text: None,
                            by: Some("este terminal".into()),
                        });
                    }
                }
                None => {
                    let _ = client.send(Cmd::ConversationSend {
                        node: node.to_owned(),
                        text: line,
                        images: Vec::new(),
                    });
                }
            }
        }
        KeyCode::Backspace => {
            screen.buffer.pop();
        }
        KeyCode::Char(c) if !ctrl => screen.buffer.push(c),
        _ => {}
    }
    let mut out = stdout();
    let _ = write!(out, "\r\x1b[2K❯ {}", screen.buffer);
    let _ = out.flush();
    Step::Go
}

/// Print what is new and redraw the line being typed.
fn draw(out: &mut impl Write, conv: &Conversation, screen: &mut Screen) {
    let mut lines: Vec<String> = Vec::new();
    // Items the node dropped are gone; start from the first it still has.
    let first = u64::from(conv.first);
    if screen.printed < first {
        screen.printed = first;
    }
    while let Some(item) = conv.items.get((screen.printed - first) as usize) {
        if !settled(item) {
            break;
        }
        lines.extend(item_lines(item));
        screen.printed += 1;
    }
    if let Some(ask) = &conv.ask {
        if screen.shown_ask.as_deref() != Some(ask.id.as_str()) {
            screen.shown_ask = Some(ask.id.clone());
            lines.extend(ask_lines(ask));
            lines.push("  elige un número (o y / n):".into());
        }
    } else {
        screen.shown_ask = None;
    }
    for l in &lines {
        let _ = write!(out, "\r\x1b[2K{l}\r\n");
    }
    let prompt = if conv.status == ConvStatus::Idle {
        "❯"
    } else {
        "…"
    };
    let _ = write!(out, "\r\x1b[2K{prompt} {}", screen.buffer);
    let _ = out.flush();
}

/// Whether an item is finished enough to print once and for all.
fn settled(item: &Item) -> bool {
    match item {
        Item::Text { live, .. } => !live,
        Item::Tool { state, .. } | Item::Agent { state, .. } | Item::Shell { state, .. } => {
            !matches!(state, ToolState::Run | ToolState::Ask)
        }
        _ => true,
    }
}

fn diff_line(d: &contract::conversation::DiffLine) -> String {
    let sign = match d.sign {
        DiffSign::Add => '+',
        DiffSign::Del => '-',
        DiffSign::Same => ' ',
    };
    format!("    {sign} {}", d.text)
}

/// What an `ask` says, as plain lines: what it is, what it is about and its numbered options.
pub fn ask_lines(ask: &Ask) -> Vec<String> {
    let what = match ask.kind {
        AskKind::Permission => "permiso",
        AskKind::Question => "pregunta",
        AskKind::Plan => "plan",
    };
    let mut lines = vec![format!("? {what}: {}", ask.title)];
    if let Some(c) = &ask.command {
        lines.push(format!("    {c}"));
    }
    lines.extend(ask.diff.iter().map(diff_line));
    lines.extend(ask.plan.iter().map(|p| format!("    {p}")));
    for (i, o) in ask.options.iter().enumerate() {
        lines.push(format!("  {}) {}", i + 1, o.label));
    }
    lines
}

/// «1 herramienta», «3 herramientas».
pub fn tools_word(n: u32) -> String {
    format!(
        "{n} {}",
        if n == 1 {
            "herramienta"
        } else {
            "herramientas"
        }
    )
}

/// An item as plain lines: what the pane's own terminal and `read` show (REQ-395, REQ-455).
pub fn item_lines(item: &Item) -> Vec<String> {
    match item {
        Item::User { text, images } => {
            let more = if *images > 0 {
                format!(" [{images} imagen(es)]")
            } else {
                String::new()
            };
            vec![format!("❯ {text}{more}")]
        }
        Item::Text { text, .. } => text
            .lines()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    format!("● {l}")
                } else {
                    format!("  {l}")
                }
            })
            .collect(),
        Item::Tool {
            name,
            arg,
            summary,
            diff,
            by,
            state,
            ..
        } => {
            let mut v = vec![format!("● {name}({arg})")];
            let by = by.as_deref().map(|b| format!(" · {b}")).unwrap_or_default();
            if let Some(s) = summary {
                v.push(format!("  ⎿  {s}{by}"));
            } else if *state == ToolState::Ok {
                v.push(format!("  ⎿  listo{by}"));
            }
            v.extend(diff.iter().map(diff_line));
            v
        }
        Item::Agent {
            name,
            description,
            tools,
            tokens,
            ms,
            ..
        } => vec![
            format!("● Agent({name}) {description}"),
            format!(
                "  ⎿  {} · {}k tokens · {:.1} s",
                tools_word(*tools),
                tokens / 1000,
                *ms as f64 / 1000.0
            ),
        ],
        Item::Shell {
            command, output, ..
        } => std::iter::once(format!("! {command}"))
            .chain(output.iter().cloned())
            .collect(),
        Item::Out { lines } => lines.iter().map(|l| format!("  ⎿  {l}")).collect(),
        Item::Note { text, .. } => vec![format!("  ⎿  {text}")],
        Item::Result { ms, tools, cost } => vec![format!(
            "✻ Listo · {} s · {} · ${},{:02}",
            ms / 1000,
            tools_word(*tools),
            cost / 1_000_000,
            (cost % 1_000_000) / 10_000
        )],
        Item::Divider { text, .. } => vec![format!("── {text} ──")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::conversation::DividerKind;

    #[test]
    fn an_item_is_printed_as_plain_lines() {
        let user = Item::User {
            text: "hola".into(),
            images: 1,
        };
        assert_eq!(item_lines(&user), ["❯ hola [1 imagen(es)]"]);
        let text = Item::Text {
            text: "uno\ndos".into(),
            live: false,
            blocks: None,
        };
        assert_eq!(item_lines(&text), ["● uno", "  dos"]);
        let tool = Item::Tool {
            id: "t".into(),
            name: "Read".into(),
            arg: "src/a.rs".into(),
            state: ToolState::Ok,
            summary: Some("Leídas 3 líneas".into()),
            output: vec![],
            diff: vec![],
            url: None,
            by: Some("el móvil".into()),
        };
        assert_eq!(
            item_lines(&tool),
            ["● Read(src/a.rs)", "  ⎿  Leídas 3 líneas · el móvil"]
        );
        let result = Item::Result {
            ms: 11_000,
            tools: 3,
            cost: 60_000,
        };
        assert_eq!(
            item_lines(&result),
            ["✻ Listo · 11 s · 3 herramientas · $0,06"]
        );
        let d = Item::Divider {
            text: "el daemon se reinició · 20:10".into(),
            kind: DividerKind::Restart,
        };
        assert_eq!(item_lines(&d), ["── el daemon se reinició · 20:10 ──"]);
    }

    #[test]
    fn a_live_text_and_a_running_tool_wait_to_be_printed() {
        assert!(!settled(&Item::Text {
            text: "x".into(),
            live: true,
            blocks: None,
        }));
        assert!(settled(&Item::Text {
            text: "x".into(),
            live: false,
            blocks: None,
        }));
        let running = Item::Tool {
            id: "t".into(),
            name: "Bash".into(),
            arg: "ls".into(),
            state: ToolState::Run,
            summary: None,
            output: vec![],
            diff: vec![],
            url: None,
            by: None,
        };
        assert!(!settled(&running));
    }

    /// REQ-572: the suggestion is on unless `config.toml` says `suggestions = false`.
    #[test]
    fn the_suggestion_is_on_unless_the_config_turns_it_off() {
        assert!(suggestions_on(""));
        assert!(suggestions_on(
            "[[program]]\nname = \"x\"\ncommand = [\"claude\"]\n"
        ));
        assert!(suggestions_on("suggestions = true\n"));
        assert!(!suggestions_on("suggestions = false\n"));
        assert!(
            suggestions_on("suggestions = \"no\"\n"),
            "not a boolean: it stays on"
        );
        assert!(
            suggestions_on("esto no es toml ["),
            "a file that cannot be read leaves it on"
        );
    }
}
