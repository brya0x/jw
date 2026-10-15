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
    config.env = vec![
        ("HOME".into(), home.to_string_lossy().into()),
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

fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + WAIT;
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

#[test]
fn a_program_started_by_the_wrapper_leaves_a_shell_with_blocks() {
    let Some(shell) = find("bash").or_else(|| find("zsh")) else {
        return;
    };
    let sb = Sandbox::new();
    let home = sb.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut config = sb.config();
    config.shell = Some(shell);
    config.env = vec![("HOME".into(), home.to_string_lossy().into())];
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
            program: Some(vec!["echo".into(), "from the wrapper".into()]),
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
        "#!/bin/sh\nprintf '\\033[?1049h'\nsleep 1\nprintf '\\033[?1049l'\nsleep 1.5\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (pane, blocks) = (r.pane.clone(), r.blocks.clone());
    let alt = move |c: &Client| c.nodes[&pane]["screen"]["alt"] == true;
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
    assert!(
        r.c.wait_until(Duration::from_secs(10), left).unwrap(),
        "it left the alternate screen and is still the running block, full: {}",
        r.c.nodes[&r.blocks]
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
