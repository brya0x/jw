//! N17: the shell integration and the blocks, against real zsh, bash and fish (a shell that
//! is not installed is skipped).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Sandbox, WAIT};
use contract::Cmd;
use daemon::Daemon;
use serde_json::Value;
use wire::Client;

fn find(name: &str) -> Option<String> {
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

struct Rig {
    _sb: Sandbox,
    config: daemon::Config,
    d: Option<Daemon>,
    c: Client,
    pane: String,
    blocks: String,
    dir: PathBuf,
}

/// A pane running `shell` with an empty home (so the user's own rc files stay out of it),
/// with its blocks node open.
fn rig(shell: &str) -> Rig {
    let sb = Sandbox::new();
    let home = sb.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut config = sb.config();
    config.shell = Some(shell.to_owned());
    // ZDOTDIR too: a pane's zsh reads the dotfiles of ZDOTDIR, and the user's own would
    // otherwise be inherited from the test runner's environment. `skip_global_compinit`: Ubuntu's
    // /etc/zsh/zshrc runs compinit, which on the GitHub runner finds insecure directories and asks
    // «continue [y] or abort [n]?» at the prompt, so the shell never reaches it.
    config.env = vec![
        ("HOME".into(), home.to_string_lossy().into()),
        ("ZDOTDIR".into(), home.to_string_lossy().into()),
        ("skip_global_compinit".into(), "1".into()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").to_string_lossy().into(),
        ),
    ];
    let d = Daemon::start(config.clone()).unwrap();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "{shell}: the pane has no blocks node: {:?}",
        c.nodes.get(&pane)
    );
    let blocks = c.nodes[&pane]["blocks"].as_str().unwrap().to_owned();
    c.open(&blocks).unwrap();
    let mut seen_msgs = Vec::new();
    let end = std::time::Instant::now() + WAIT;
    while !c.nodes.contains_key(&blocks) && std::time::Instant::now() < end {
        if let Some(m) = c.poll(Duration::from_millis(100)).unwrap() {
            seen_msgs.push(format!("{m:?}").chars().take(160).collect::<String>());
        }
    }
    assert!(
        c.nodes.contains_key(&blocks),
        "{shell}: no blocks node {blocks}; got {seen_msgs:#?}"
    );
    // Typed before the shell has drawn its first prompt, a line loses its first characters
    // (`echo` arrives as `cho`, and a mangled command ends with 127): a slow runner showed it.
    // The pane says the shell is at its prompt when it carries what the line holds (`input`).
    assert!(
        c.wait_until(Duration::from_secs(20), |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("input").is_some()))
            .unwrap(),
        "{shell}: the shell did not reach its first prompt: {:?}",
        c.nodes.get(&pane)
    );
    Rig {
        _sb: sb,
        config,
        d: Some(d),
        c,
        pane,
        blocks,
        dir,
    }
}

impl Rig {
    fn run(&mut self, line: &str) {
        let pane = self.pane.clone();
        self.c
            .cmd(Cmd::Write {
                pane,
                data: format!("{line}\r"),
            })
            .unwrap()
            .unwrap();
    }

    fn blocks(&self) -> Vec<Value> {
        self.c.nodes[&self.blocks]["blocks"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// Wait until the `n`th block (1-based) has ended.
    fn finished(&mut self, n: usize) -> Value {
        let id = self.blocks.clone();
        assert!(
            self.c
                .wait_until(Duration::from_secs(10), |c| c.nodes[&id]["blocks"]
                    .as_array()
                    .is_some_and(|b| b.len() >= n && b[n - 1]["running"] == false))
                .unwrap(),
            "block {n} did not end: {}\nscreen: {}",
            self.c.nodes[&id],
            self.c.nodes[&self.pane]["screen"]["lines"]
        );
        self.blocks()[n - 1].clone()
    }

    fn text(block: &Value) -> Vec<String> {
        block["out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["t"].as_str().unwrap())
                    .collect()
            })
            .collect()
    }
}

fn check_shell(shell: &str) {
    let mut r = rig(shell);

    // A command is a block: its words as typed, its folder, its status, its output, no prompt.
    // fish has no `$((…))`.
    let sum = if shell.ends_with("fish") {
        r#"echo "it's "(math 1+1)"#
    } else {
        r#"echo "it's $((1+1))""#
    };
    r.run(sum);
    let b = r.finished(1);
    assert_eq!(b["cmd"], sum, "{shell}");
    assert_eq!(b["cwd"], r.dir.to_string_lossy().as_ref(), "{shell}");
    assert_eq!(
        (b["code"].as_i64(), b["running"].as_bool()),
        (Some(0), Some(false)),
        "{shell}"
    );
    assert!(b["ms"].as_u64().is_some());
    assert_eq!(
        Rig::text(&b),
        ["it's 2"],
        "{shell}: the output is the command's, not the prompt's"
    );

    // The status of a failure.
    r.run("false");
    assert_eq!(r.finished(2)["code"], 1, "{shell}");

    // Colours become spans.
    r.run(r"printf '\033[1;31mred\033[0m plain\n'");
    let b = r.finished(3);
    let row = b["out"][0].as_array().unwrap();
    assert_eq!(
        (
            row[0]["t"].as_str(),
            row[0]["fg"].as_u64(),
            row[0]["b"].as_bool()
        ),
        (Some("red"), Some(1), Some(true)),
        "{shell}: {row:?}"
    );

    // A running command has no status yet, and ends.
    r.run("sleep 1.5");
    let id = r.blocks.clone();
    assert!(
        r.c.wait_until(WAIT, |c| c.nodes[&id]["blocks"]
            .as_array()
            .is_some_and(|b| b.len() == 4 && b[3]["running"] == true))
            .unwrap(),
        "{shell}"
    );
    let running = r.blocks()[3].clone();
    assert!(
        running.get("code").is_none() && running.get("ms").is_none(),
        "{shell}: {running}"
    );
    assert_eq!(r.finished(4)["code"], 0, "{shell}");

    // The folder a command starts in.
    r.run("cd / && pwd");
    let b = r.finished(5);
    assert_eq!(
        b["cwd"],
        r.dir.to_string_lossy().as_ref(),
        "{shell}: it started where it was"
    );
    r.run("true");
    assert_eq!(
        r.finished(6)["cwd"],
        "/",
        "{shell}: the next one starts where the cd left it"
    );
    // REQ-625, REQ-626: the folder the shell says at its prompt is the pane's `cwd` as well.
    let pane = r.pane.clone();
    assert!(
        r.c.wait_until(Duration::from_secs(10), |c| c.nodes[&pane]["cwd"] == "/")
            .unwrap(),
        "{shell}: the pane stayed at {}",
        r.c.nodes[&pane]["cwd"]
    );

    // A program that takes the whole screen is a block with no output, and the node says so.
    r.run(r"printf '\033[?1049hhello\033[?1049l'");
    let b = r.finished(7);
    assert_eq!(
        (b["full"].as_bool(), b["out"].as_array().map(Vec::len)),
        (Some(true), Some(0)),
        "{shell}"
    );
    assert_eq!(r.c.nodes[&r.blocks]["mode"], "blocks");

    // The pane's own screen is still there, and carries the shell.
    assert_eq!(r.c.nodes[&r.pane]["blocks"], r.blocks.as_str());
    r.d.take().unwrap().stop();
}

#[test]
fn zsh_reports_its_commands_as_blocks() {
    match find("zsh") {
        Some(p) => check_shell(&p),
        None => eprintln!("zsh is not installed; skipped"),
    }
}

#[test]
fn bash_reports_its_commands_as_blocks() {
    match find("bash") {
        Some(p) => check_shell(&p),
        None => eprintln!("bash is not installed; skipped"),
    }
}

#[test]
fn fish_reports_its_commands_as_blocks() {
    match find("fish") {
        Some(p) => check_shell(&p),
        None => eprintln!("fish is not installed; skipped"),
    }
}

/// REQ-645: `cd @` takes the shell to the folder of the workspace, and the other `cd`s are as they were.
fn check_cd_at(shell: &str) {
    let mut r = rig(shell);
    let sub = r.dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let (sub, root) = (
        sub.to_string_lossy().into_owned(),
        r.dir.to_string_lossy().into_owned(),
    );
    let pane = r.pane.clone();
    let at = |r: &mut Rig, want: &str| {
        r.c.wait_until(Duration::from_secs(10), |c| c.nodes[&pane]["cwd"] == want)
            .unwrap()
    };
    r.run("cd sub");
    r.finished(1);
    assert!(at(&mut r, &sub), "a plain cd is a cd");
    r.run("cd @");
    r.finished(2);
    assert!(
        at(&mut r, &root),
        "cd @ went to {root}: {}",
        r.c.nodes[&pane]["cwd"]
    );
    r.run("cd -");
    r.finished(3);
    assert!(at(&mut r, &sub), "cd - is still the folder before");
    r.run("cd ..");
    r.finished(4);
    assert!(at(&mut r, &root), "cd .. is still the folder above");
    r.d.take().unwrap().stop();
}

#[test]
fn zsh_goes_to_the_workspace_folder_with_cd_at() {
    match find("zsh") {
        Some(p) => check_cd_at(&p),
        None => eprintln!("zsh is not installed; skipped"),
    }
}

#[test]
fn bash_goes_to_the_workspace_folder_with_cd_at() {
    match find("bash") {
        Some(p) => check_cd_at(&p),
        None => eprintln!("bash is not installed; skipped"),
    }
}

#[test]
fn fish_goes_to_the_workspace_folder_with_cd_at() {
    match find("fish") {
        Some(p) => check_cd_at(&p),
        None => eprintln!("fish is not installed; skipped"),
    }
}

/// REQ-628, REQ-629: a pane made beside one starts where that shell is, and the daemon brings a
/// shell back where it was.
#[test]
fn a_pane_beside_and_a_restart_start_where_the_shell_is() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let mut r = rig(&shell);
    let sub = r.dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let sub = sub.to_string_lossy().into_owned();
    r.run("cd sub");
    r.finished(1);
    let pane = r.pane.clone();
    assert!(
        r.c.wait_until(Duration::from_secs(10), |c| c.nodes[&pane]["cwd"]
            == sub.as_str())
            .unwrap(),
        "the pane stayed at {}",
        r.c.nodes[&pane]["cwd"]
    );
    let ws = r.c.nodes[&pane]["workspace"].as_str().unwrap().to_owned();
    let beside =
        r.c.cmd(Cmd::NewPane {
            workspace: ws,
            program: None,
            beside: Some(pane.clone()),
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
            .as_str()
            .unwrap()
            .to_owned();
    r.c.open(&beside).unwrap();
    assert!(
        r.c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(&beside)
            .is_some_and(|n| n["cwd"] == sub.as_str()))
            .unwrap(),
        "the pane beside did not start in {sub}: {:?}",
        r.c.nodes.get(&beside)
    );
    // A restart: the shell comes back in the folder it was in.
    let config = r.config.clone();
    r.d.take().unwrap().stop();
    let d = Daemon::start(config.clone()).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c.nodes.contains_key(&pane))
            .unwrap()
    );
    assert_eq!(c.nodes[&pane]["cwd"], sub.as_str());
    d.stop();
}

/// REQ-273: `clear` empties the pane's blocks, its own one included, and the next command is a block.
#[test]
fn clear_empties_the_blocks_of_the_pane() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let mut r = rig(&shell);
    r.run("echo one");
    r.finished(1);
    r.run("echo two");
    r.finished(2);
    r.run("clear");
    let id = r.blocks.clone();
    assert!(
        r.c.wait_until(Duration::from_secs(10), |c| c.nodes[&id]["blocks"]
            .as_array()
            .is_some_and(Vec::is_empty))
            .unwrap(),
        "the blocks were not dropped: {}",
        r.c.nodes[&id]
    );
    r.run("echo three");
    let b = r.finished(1);
    assert_eq!(Rig::text(&b), ["three"]);
    assert_eq!(b["id"], 4, "ids go on after a clear");
    r.d.take().unwrap().stop();
}

/// REQ-297 to REQ-303: the blocks of a pane are saved, and come back with the daemon, a command
/// that was running cut and a mark where the daemon restarted.
#[test]
fn the_blocks_of_a_pane_come_back_after_a_restart_with_the_cut_command_and_a_mark() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let mut r = rig(&shell);
    r.run("echo one");
    r.finished(1);
    r.run("echo two");
    r.finished(2);
    r.run("sleep 30");
    assert!(
        r.c.wait_until(WAIT, |c| c.nodes[&r.blocks]["blocks"][2]["running"] == true)
            .unwrap()
    );
    let (pane, config) = (r.pane.clone(), r.config.clone());
    let file = config
        .state_dir
        .join("history")
        .join(format!("{pane}.json"));
    // A command that starts is on disk at once, not after the pause between writes: a daemon that
    // is stopped with a signal saves nothing more.
    assert!(
        wait_within(Duration::from_millis(800), || std::fs::read_to_string(
            &file
        )
        .is_ok_and(|t| t.contains("sleep 30"))),
        "the running command was not saved when it started"
    );
    // The daemon stops: what it has is written.
    let Rig { _sb, d, c, .. } = r;
    drop(c);
    d.unwrap().stop();
    assert!(file.is_file(), "the history of the pane is on disk");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "for the person alone");
    }

    let d = Daemon::start(config.clone()).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    c.open(&pane).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "the pane has its blocks node from the start"
    );
    let blocks = c.nodes[&pane]["blocks"].as_str().unwrap().to_owned();
    c.open(&blocks).unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes.contains_key(&blocks))
            .unwrap()
    );
    let list = c.nodes[&blocks]["blocks"].as_array().unwrap().clone();
    let cmds: Vec<&str> = list.iter().map(|b| b["cmd"].as_str().unwrap()).collect();
    assert_eq!(cmds, ["echo one", "echo two", "sleep 30", ""]);
    assert_eq!(list[0]["code"], 0);
    assert_eq!(
        (
            list[2]["running"].as_bool(),
            list[2]["interrupted"].as_bool()
        ),
        (Some(false), Some(true)),
        "the command that was running was cut"
    );
    assert!(list[2]["ms"].as_u64().is_some());
    assert_eq!(list[3]["restart"], true);
    let last_id = list[3]["id"].as_u64().unwrap();
    assert!(last_id > list[2]["id"].as_u64().unwrap());

    // The new shell goes on after the mark, and the ids go on.
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo three\r".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&blocks]["blocks"]
            .as_array()
            .is_some_and(|b| b.len() == 5 && b[4]["running"] == false))
            .unwrap(),
        "{}",
        c.nodes[&blocks]
    );
    assert_eq!(c.nodes[&blocks]["blocks"][4]["cmd"], "echo three");
    assert!(c.nodes[&blocks]["blocks"][4]["id"].as_u64().unwrap() > last_id);

    // Closing the pane takes its history away.
    c.cmd(Cmd::ClosePane { pane }).unwrap().unwrap();
    assert!(
        wait_for(|| !file.exists()),
        "the history of a closed pane is removed"
    );
    d.stop();
}

fn wait_for(done: impl FnMut() -> bool) -> bool {
    wait_within(WAIT, done)
}

fn wait_within(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + within;
    while std::time::Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// REQ-123: the node of a pane's blocks is the same for as long as the pane lives, and
/// closing the pane takes it away.
#[test]
fn the_blocks_node_keeps_its_id_while_the_pane_lives_and_goes_with_it() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let mut r = rig(&shell);
    r.run("echo one");
    r.finished(1);
    let (pane, blocks) = (r.pane.clone(), r.blocks.clone());
    assert_eq!(
        r.c.nodes[&pane]["blocks"].as_str(),
        Some(blocks.as_str()),
        "the pane still names the same node after a command"
    );
    r.c.cmd(Cmd::ClosePane { pane }).unwrap().unwrap();
    assert!(
        r.c.wait_until(WAIT, |c| !c.nodes.contains_key(&blocks))
            .unwrap(),
        "gone was sent for the blocks node"
    );
    r.d.take().unwrap().stop();
}

#[test]
fn a_shell_without_the_integration_has_no_blocks() {
    let sb = Sandbox::new();
    let d = sb.start(); // /bin/sh
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = sb.client();
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: None,
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&pane)).unwrap());
    c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo hi\r".into(),
    })
    .unwrap()
    .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    c.wait_until(Duration::from_millis(200), |_| false).unwrap();
    assert!(
        c.nodes[&pane].get("blocks").is_none(),
        "sh has no integration, so the pane is only a screen"
    );
    d.stop();
}

/// A pane started with `program` in a daemon of its own, with an empty home so the user's rc files
/// stay out of it; `None` where neither bash nor zsh is installed.
fn pane_with_program(program: &[&str]) -> Option<(Sandbox, Daemon, Client, String)> {
    let shell = find("bash").or_else(|| find("zsh"))?;
    Some(pane_in(&shell, program))
}

/// A pane that starts `program` in `shell`, in a daemon of its own, with its node open.
fn pane_in(shell: &str, program: &[&str]) -> (Sandbox, Daemon, Client, String) {
    let sb = Sandbox::new();
    let home = sb.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut config = sb.config();
    config.shell = Some(shell.to_owned());
    config.env = vec![
        ("HOME".into(), home.to_string_lossy().into()),
        ("ZDOTDIR".into(), home.to_string_lossy().into()),
        ("skip_global_compinit".into(), "1".into()),
    ];
    let d = Daemon::start(config.clone()).unwrap();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Client::connect(&config.socket, "test").unwrap();
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: dir.to_string_lossy().into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let pane = c
        .cmd(Cmd::NewPane {
            workspace: ws,
            program: Some(program.iter().map(|w| (*w).to_owned()).collect()),
            beside: None,
            dir: None,
        })
        .unwrap()
        .unwrap()
        .unwrap()["pane"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&pane).unwrap();
    (sb, d, c, pane)
}

#[test]
fn a_program_started_by_the_wrapper_leaves_a_shell_with_blocks() {
    let Some((_sb, d, mut c, pane)) = pane_with_program(&["echo", "from the wrapper"]) else {
        return;
    };
    // After the program, `exec $SHELL -i` brings the integration back: a prompt is reported.
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(&pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "{:?}",
        c.nodes.get(&pane)
    );
    d.stop();
}

/// A stand-in program in a folder of its own, found by its name: it prints a line and waits.
fn stand_in(name: &str, secs: u32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("jw-n187-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(
        &file,
        format!("#!/bin/sh\necho on-the-screen\nsleep {secs}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    file
}

/// REQ-844 and REQ-1058: a program that draws its own screen (`nvim`) and an agent (`claude`) are
/// reported by nobody while they run, so the pane is its screen (no blocks node, which would be an
/// empty list); the shell that takes over brings the blocks.
#[test]
fn a_pane_started_with_a_program_that_takes_the_screen_is_its_screen_until_the_prompt() {
    for name in ["nvim", "claude"] {
        let Some(shell) = find("bash").or_else(|| find("zsh")) else {
            return;
        };
        let file = stand_in(name, 4);
        let (_sb, d, mut c, pane) = pane_in(&shell, &[&file.to_string_lossy()]);
        let on_screen = |c: &Client| {
            c.nodes.get(&pane).is_some_and(|n| {
                n["screen"]["lines"].as_array().is_some_and(|l| {
                    l.iter()
                        .any(|l| l.as_str().unwrap().contains("on-the-screen"))
                })
            })
        };
        assert!(
            c.wait_until(Duration::from_secs(10), on_screen).unwrap(),
            "{name}: {:?}",
            c.nodes.get(&pane)
        );
        assert!(
            c.nodes[&pane].get("blocks").is_none(),
            "{name} runs: the pane is its screen, not a list of blocks"
        );
        assert!(
            c.wait_until(Duration::from_secs(15), |c| c
                .nodes
                .get(&pane)
                .is_some_and(|n| n.get("blocks").is_some()))
                .unwrap(),
            "{name} ended and the shell that took over did not bring the blocks: {:?}",
            c.nodes.get(&pane)
        );
        d.stop();
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}

/// The blocks node of a pane, opened, once it exists.
fn blocks_of(c: &mut Client, pane: &str) -> String {
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c
            .nodes
            .get(pane)
            .is_some_and(|n| n.get("blocks").is_some()))
            .unwrap(),
        "the pane has no blocks node: {:?}",
        c.nodes.get(pane)
    );
    let id = c.nodes[pane]["blocks"].as_str().unwrap().to_owned();
    c.open(&id).unwrap();
    assert!(
        c.wait_until(Duration::from_secs(10), |c| c.nodes.contains_key(&id))
            .unwrap()
    );
    id
}

/// REQ-1056, REQ-1057 and REQ-1060, in each shell installed: the program is a block of the pane
/// from the start, running, in mode `blocks`, with its output; it ends with its status and the shell
/// that takes over runs the next command as a second block (never a copy of the first).
#[test]
fn a_program_that_does_not_take_the_screen_is_a_running_block_from_the_start() {
    for shell in ["zsh", "bash", "fish"].into_iter().filter_map(find) {
        let (_sb, d, mut c, pane) =
            pane_in(&shell, &["sh", "-c", "echo on-the-block; sleep 3; exit 3"]);
        let id = blocks_of(&mut c, &pane);
        let running = |c: &Client| {
            c.nodes[&id]["blocks"].as_array().is_some_and(|b| {
                b.last().is_some_and(|l| {
                    l["running"] == true && Rig::text(l).iter().any(|t| t == "on-the-block")
                })
            })
        };
        assert!(
            c.wait_until(Duration::from_secs(10), running).unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        assert_eq!(c.nodes[&id]["mode"], "blocks", "{shell}");
        let first = c.nodes[&id]["blocks"][0].clone();
        assert!(
            first["cmd"].as_str().unwrap().contains("on-the-block"),
            "{shell}: {first}"
        );
        assert!(
            !first["cwd"].as_str().unwrap().is_empty(),
            "{shell}: its folder"
        );
        assert!(
            c.wait_until(
                Duration::from_secs(15),
                |c| c.nodes[&id]["blocks"][0]["running"] == false
            )
            .unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        assert_eq!(c.nodes[&id]["blocks"][0]["code"], 3, "{shell}");
        assert_eq!(
            c.nodes[&id]["blocks"].as_array().unwrap().len(),
            1,
            "{shell}: the prompt of the shell that took over adds no block"
        );
        assert!(
            c.wait_until(Duration::from_secs(20), |c| c
                .nodes
                .get(&pane)
                .is_some_and(|n| n.get("input").is_some()))
                .unwrap(),
            "{shell}: the shell did not take over"
        );
        c.cmd(Cmd::Write {
            pane: pane.clone(),
            data: "echo after\r".into(),
        })
        .unwrap()
        .unwrap();
        assert!(
            c.wait_until(Duration::from_secs(10), |c| c.nodes[&id]["blocks"]
                .as_array()
                .is_some_and(|b| b.len() == 2 && b[1]["running"] == false))
                .unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        d.stop();
    }
}

/// REQ-1057 (d): a program that ends at once, with an error, is one closed block with its status
/// (its start, its output and its end may come in one read) and the shell takes over with no second block.
#[test]
fn a_program_that_ends_at_once_leaves_one_closed_block_with_its_status() {
    for shell in ["zsh", "bash", "fish"].into_iter().filter_map(find) {
        let (_sb, d, mut c, pane) = pane_in(&shell, &["sh", "-c", "echo x; exit 3"]);
        let id = blocks_of(&mut c, &pane);
        assert!(
            c.wait_until(
                Duration::from_secs(10),
                |c| c.nodes[&id]["blocks"][0]["running"] == false
            )
            .unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        let block = c.nodes[&id]["blocks"][0].clone();
        assert_eq!(block["code"], 3, "{shell}: {block}");
        assert_eq!(Rig::text(&block), ["x"], "{shell}");
        assert!(
            c.wait_until(Duration::from_secs(20), |c| c
                .nodes
                .get(&pane)
                .is_some_and(|n| n.get("input").is_some()))
                .unwrap(),
            "{shell}: the shell did not take over"
        );
        assert_eq!(
            c.nodes[&id]["blocks"].as_array().unwrap().len(),
            1,
            "{shell}: one block, and none for the prompt"
        );
        d.stop();
    }
}

/// REQ-1057: a program that takes the screen, which no mark reports, does not take the pane with it
/// on `^C` either (the `trap` is for every program): the shell takes over and brings the blocks.
#[test]
fn control_c_ends_a_program_that_takes_the_screen_and_the_pane_stays() {
    for shell in ["zsh", "bash", "fish"].into_iter().filter_map(find) {
        let file = stand_in("nvim", 60);
        let (_sb, d, mut c, pane) = pane_in(&shell, &[&file.to_string_lossy()]);
        assert!(
            c.wait_until(Duration::from_secs(10), |c| c.nodes.get(&pane).is_some_and(
                |n| {
                    n["screen"]["lines"].as_array().is_some_and(|l| {
                        l.iter()
                            .any(|l| l.as_str().unwrap().contains("on-the-screen"))
                    })
                }
            ))
            .unwrap(),
            "{shell}: {:?}",
            c.nodes.get(&pane)
        );
        c.cmd(Cmd::Write {
            pane: pane.clone(),
            data: "\x03".into(),
        })
        .unwrap()
        .unwrap();
        assert!(
            c.wait_until(Duration::from_secs(20), |c| c
                .nodes
                .get(&pane)
                .is_some_and(|n| n.get("blocks").is_some()))
                .unwrap(),
            "{shell}: the pane did not survive ^C: {:?}",
            c.nodes.get(&pane)
        );
        d.stop();
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}

/// REQ-1057: `^C` reaches the program, which dies; `D` closes the block with a status that is not
/// success, and the shell goes on.
#[test]
fn control_c_ends_the_block_of_a_program_and_the_shell_takes_over() {
    for shell in ["zsh", "bash", "fish"].into_iter().filter_map(find) {
        let (_sb, d, mut c, pane) = pane_in(&shell, &["sh", "-c", "echo up; sleep 60"]);
        let id = blocks_of(&mut c, &pane);
        assert!(
            c.wait_until(Duration::from_secs(10), |c| c.nodes[&id]["blocks"]
                .as_array()
                .is_some_and(|b| b.last().is_some_and(
                    |l| l["running"] == true && Rig::text(l).iter().any(|t| t == "up")
                )))
            .unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        c.cmd(Cmd::Write {
            pane: pane.clone(),
            data: "\x03".into(),
        })
        .unwrap()
        .unwrap();
        assert!(
            c.wait_until(
                Duration::from_secs(10),
                |c| c.nodes[&id]["blocks"][0]["running"] == false
            )
            .unwrap(),
            "{shell}: {}",
            c.nodes[&id]
        );
        let code = c.nodes[&id]["blocks"][0]["code"].clone();
        assert_eq!(
            code, 130,
            "{shell}: ^C is the status of a program killed by SIGINT"
        );
        assert!(
            c.wait_until(Duration::from_secs(20), |c| c
                .nodes
                .get(&pane)
                .is_some_and(|n| n.get("input").is_some()))
                .unwrap(),
            "{shell}: the shell did not take over"
        );
        d.stop();
    }
}

/// REQ-250 and REQ-251: the mode follows the command the shell reports, not the 0.7 s look at
/// the foreground. A stand-in `claude` (it draws on the normal screen, as the real one does).
#[test]
fn a_program_that_takes_the_screen_is_full_from_its_block_to_its_end_and_not_a_poll_later() {
    use std::os::unix::fs::PermissionsExt;
    let Some(shell) = find("zsh").or_else(|| find("bash")) else {
        return;
    };
    let mut r = rig(&shell);
    let stub = r.dir.join("claude");
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf 'a screen of its own\\n'\nsleep 1.5\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id = r.blocks.clone();
    let mode = |r: &Rig| r.c.nodes[&r.blocks]["mode"].as_str().unwrap().to_owned();
    let running = move |c: &Client| {
        c.nodes[&id]["blocks"]
            .as_array()
            .is_some_and(|b| b.last().is_some_and(|l| l["running"] == true))
    };

    r.run("sleep 1");
    assert!(
        r.c.wait_until(Duration::from_secs(10), running.clone())
            .unwrap()
    );
    assert_eq!(
        mode(&r),
        "blocks",
        "a command that draws no screen of its own"
    );
    r.finished(1);

    r.run(&stub.to_string_lossy());
    assert!(r.c.wait_until(Duration::from_secs(10), running).unwrap());
    assert_eq!(
        mode(&r),
        "full",
        "full from the block the shell reports, not from the next look at the foreground"
    );
    r.finished(2);
    assert_eq!(
        mode(&r),
        "blocks",
        "blocks as soon as the shell says it ended, while the foreground may still say claude"
    );
}

/// REQ-252 and REQ-253: the pane says what its shell holds at the prompt, from where the shell
/// says the prompt ends, and nothing while a command runs.
#[test]
fn the_pane_carries_what_the_shell_holds_at_its_prompt_and_nothing_while_a_command_runs() {
    let Some(shell) = find("zsh").or_else(|| find("bash")) else {
        return;
    };
    let mut r = rig(&shell);
    let pane = r.pane.clone();
    let input = |c: &Client, pane: &str| c.nodes[pane].get("input").cloned();
    let held = |text: &'static str, cursor: u64| {
        let pane = pane.clone();
        move |c: &Client| {
            input(c, &pane).is_some_and(|i| i["text"] == text && i["cursor"] == cursor)
        }
    };

    assert!(
        r.c.wait_until(Duration::from_secs(10), held("", 0))
            .unwrap(),
        "{}",
        r.c.nodes[&pane]
    );
    r.c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "echo hi".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        r.c.wait_until(Duration::from_secs(10), held("echo hi", 7))
            .unwrap(),
        "{}",
        r.c.nodes[&pane]
    );
    // ^B moves the cursor back; the text stays whole.
    r.c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "\x02\x02".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        r.c.wait_until(Duration::from_secs(10), held("echo hi", 5))
            .unwrap()
    );

    // ^U empties the line, and a command that runs has no line to show.
    r.c.cmd(Cmd::Write {
        pane: pane.clone(),
        data: "\x05\x15".into(),
    })
    .unwrap()
    .unwrap();
    assert!(
        r.c.wait_until(Duration::from_secs(10), held("", 0))
            .unwrap()
    );
    r.run("sleep 1.5");
    let p2 = pane.clone();
    assert!(
        r.c.wait_until(Duration::from_secs(10), move |c| input(c, &p2).is_none())
            .unwrap(),
        "a running command has no input: {}",
        r.c.nodes[&pane]
    );
    r.finished(1);
    assert!(
        r.c.wait_until(Duration::from_secs(10), held("", 0))
            .unwrap(),
        "back at the prompt the line is empty again"
    );
}

/// REQ-263: the pane's screen says whether its program is on the alternate screen, and the program that
/// left it is still a running block in mode `full` until the shell says it ended.
#[test]
fn the_pane_says_when_its_program_is_on_the_alternate_screen_and_when_it_left_it_before_it_ended() {
    use std::os::unix::fs::PermissionsExt;
    let Some(shell) = find("zsh").or_else(|| find("bash")) else {
        return;
    };
    let mut r = rig(&shell);
    let stub = r.dir.join("altprog");
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf '\\033[?1049h\\033[31mred\\033[0m plain'\nsleep 1\nprintf '\\033[?1049l'\nsleep 1.5\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (pane, blocks) = (r.pane.clone(), r.blocks.clone());
    let alt = move |c: &Client| c.nodes[&pane]["screen"]["alt"] == true;
    let pane = r.pane.clone();
    let coloured = move |c: &Client| c.nodes[&pane]["screen"]["spans"][0][0]["t"] == "red";
    let pane = r.pane.clone();
    let left = move |c: &Client| {
        c.nodes[&pane]["screen"].get("alt").is_none()
            && c.nodes[&blocks]["blocks"].as_array().is_some_and(|b| {
                b.last()
                    .is_some_and(|k| k["running"] == true && k["full"] == true)
            })
    };

    assert!(
        r.c.nodes[&r.pane]["screen"].get("alt").is_none(),
        "at the shell's prompt the flag is absent"
    );
    r.run(&stub.to_string_lossy());
    assert!(
        r.c.wait_until(Duration::from_secs(10), alt).unwrap(),
        "{}",
        r.c.nodes[&r.pane]
    );
    // REQ-527: on the alternate screen the rows come with their colours, one for each row.
    assert!(
        r.c.wait_until(Duration::from_secs(10), coloured).unwrap(),
        "{}",
        r.c.nodes[&r.pane]
    );
    let screen = &r.c.nodes[&r.pane]["screen"];
    assert_eq!(
        screen["spans"].as_array().map(Vec::len),
        screen["rows"].as_u64().map(|n| n as usize)
    );
    assert_eq!(screen["spans"][0][0]["fg"], 1);
    assert!(screen["spans"][0][1].get("fg").is_none());
    assert!(
        r.c.wait_until(Duration::from_secs(10), left).unwrap(),
        "it left the alternate screen and is still the running block, full: {}",
        r.c.nodes[&r.blocks]
    );
    assert!(
        r.c.nodes[&r.pane]["screen"].get("spans").is_none(),
        "off the alternate screen the screen is text again"
    );
    assert_eq!(
        r.c.nodes[&r.blocks]["mode"], "full",
        "the node stays full until the shell says the command ended"
    );
    r.finished(1);
    assert_eq!(r.c.nodes[&r.blocks]["mode"], "blocks");
}

/// REQ-266 and REQ-268: a program that asks its terminal something is answered, and does not wait.
#[test]
fn a_program_that_asks_its_terminal_who_it_is_is_answered_at_once() {
    use std::os::unix::fs::PermissionsExt;
    let Some(bash) = find("bash") else {
        return;
    };
    let mut r = rig(&bash);
    // It asks for the device attributes and the cursor's place, and reads up to a second for each answer.
    let stub = r.dir.join("asker");
    std::fs::write(
        &stub,
        "#!/bin/bash\n\
         ask() { printf \"$1\"; IFS= read -rs -t 1 -d \"$2\" got; printf '%s=%q\\n' \"$3\" \"$got\"; }\n\
         ask '\\033[c' c ATTRS\n\
         ask '\\033[6n' R CURSOR\n\
         ask '\\033[>0q' '\\\\' VERSION\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = std::time::Instant::now();
    r.run(&stub.to_string_lossy());
    let block = r.finished(1);
    let said = Rig::text(&block).join("\n");
    assert!(said.contains("ATTRS=") && said.contains("62;22"), "{said}");
    assert!(said.contains("CURSOR=") && said.contains(';'), "{said}");
    assert!(
        said.contains("VERSION=") && said.contains(&identity::get().name),
        "{said}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "three questions answered at once, not after their timeouts: {:?}\n{said}",
        started.elapsed()
    );
}
