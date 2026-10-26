//! `jw`: the daemon and a few JSON-speaking commands to drive and watch it.

mod agent;
mod bench;
mod claude;
mod hook;
mod pair;
mod tui;
mod update;
mod version;

use std::process::ExitCode;
use std::time::Duration;

use contract::{ClientMsg, DaemonMsg, Peer};
use daemon::{Config, Daemon};
use wire::Conn;

const USAGE: &str = "usage:
  {n}                     the terminal client; starts the daemon in the background when none runs
  {n} daemon              run the daemon (foreground)
  {n} stop                stop the daemon recorded in the pid file
  {n} watch [NODE]        print every JSON line for NODE (default: the session)
  {n} cmd '<json cmd>'    send one command, print its reply (exit 1 if it failed)
  {n} tui                 the same as {n} with no arguments
  {n} pair                turn the phone link on, show the QR and answer the pairing question
  {n} pair stop           turn the phone link off (paired phones are kept)
  {n} devices             list the paired phones
  {n} revoke <id|name>    unpair a phone and drop its connection
  {n} bench               measure sizes and latency on a private daemon
  {n} update [--ref R] [--cache] [--keep] [--restart]
                          install the latest version: fetch the repository into a temporary
                          directory, build it there, install it, delete it
  {n} android install --debug [--serial ID] [--ref R] [--keep]
                          the same for the phone's debug build, with adb
  {n} --version | version the binary's version and the daemon's
  {n} hook <event>        an agent's hook (claude runs it): tell the daemon what the agent does
  {n} claude [args]       what `claude` typed in a pane runs (the shell integration's function): a conversation
  {n} ls | read | prompt | worktree | artifact | pane | group | chat | tasks | skill | role
                          what an agent in a pane uses: {n} <command> shows each one's usage";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["daemon"] => run_daemon(),
        ["stop"] => stop(),
        ["--version" | "version"] => Ok(version::show()),
        ["update", rest @ ..] => update::update(rest),
        ["android", rest @ ..] => update::android(rest),
        ["watch"] => watch(None),
        ["watch", node] => watch(Some(node)),
        ["cmd", json] => cmd(json),
        ["pair"] => pair::pair(),
        ["pair", "stop"] => pair::stop(),
        ["devices"] => pair::devices(),
        ["revoke", which] => pair::revoke(which),
        ["bench"] => bench::run().map(|()| ExitCode::SUCCESS),
        ["hook", event] => Ok(hook::run(event)),
        ["claude", rest @ ..] => Ok(claude::run(rest)),
        [
            c @ ("ls" | "read" | "prompt" | "worktree" | "artifact" | "pane" | "group" | "chat"
            | "tasks" | "skill" | "role"),
            rest @ ..,
        ] => Ok(agent::run(c, rest)),
        [] | ["tui"] => enter().map(|()| ExitCode::SUCCESS),
        _ => Err(USAGE.replace("{n}", &identity::get().name)),
    };
    match result {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<ExitCode, String>;

/// How long a daemon started for the client has to answer on its socket.
const DAEMON_WAIT: Duration = Duration::from_secs(5);
/// How long `jw stop` waits for a daemon that was asked to stop: the engines' grace (3 s) and the save, with margin.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// The terminal client, with a daemon to talk to: the one that answers, else one started
/// here in the background (REQ-186). It keeps running when the client or its terminal ends.
fn enter() -> Result<(), String> {
    ensure_daemon()?;
    tui::run()
}

fn ensure_daemon() -> Result<(), String> {
    let config = Config::from_identity();
    if Conn::connect(&config.socket).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(&config.state_dir)
        .map_err(|e| format!("{}: {e}", config.state_dir.display()))?;
    let log_path = config.state_dir.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Its own process group, so that the terminal's hang-up does not reach it, and no
    // terminal to read or write: what it says goes to its log.
    let mut daemon = std::process::Command::new(exe);
    daemon
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    std::os::unix::process::CommandExt::process_group(&mut daemon, 0);
    let mut child = daemon
        .spawn()
        .map_err(|e| format!("cannot start the daemon: {e}"))?;
    eprintln!(
        "{}: starting the daemon (log: {})",
        identity::get().name,
        log_path.display()
    );
    let end = std::time::Instant::now() + DAEMON_WAIT;
    while std::time::Instant::now() < end {
        if Conn::connect(&config.socket).is_ok() {
            return Ok(());
        }
        // It ended already (another one holds the socket, or it cannot start): say why.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the daemon ended ({status}); see {}",
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "the daemon did not answer on {} within {} s; see {}",
        config.socket.display(),
        DAEMON_WAIT.as_secs(),
        log_path.display()
    ))
}

fn run_daemon() -> Res {
    let daemon =
        Daemon::start(Config::from_identity()).map_err(|e| format!("cannot start: {e}"))?;
    daemon::warm_highlighting();
    // `jw stop` asks over the socket: the daemon stops as it stops by itself, ending its engines and
    // saving what waits, which a signal would not let it do (REQ-891).
    while daemon.accepting() && !daemon.stop_requested() {
        std::thread::sleep(Duration::from_millis(100));
    }
    if daemon.stop_requested() {
        daemon.stop();
    } else {
        daemon.wait();
    }
    Ok(ExitCode::SUCCESS)
}

fn stop() -> Res {
    let config = Config::from_identity();
    // A pid file outlives a killed daemon and its pid may belong to someone else by now:
    // only signal it while the daemon answers on its socket.
    if wire::Conn::connect(&config.socket).is_err() {
        return Err(format!(
            "no daemon is listening on {}",
            config.socket.display()
        ));
    }
    // Turn the phone link off first: its tunnel is a process of its own, and a signal does
    // not let the daemon stop it.
    if let Ok(mut client) = wire::Client::connect(&config.socket, &identity::peer("stop")) {
        let _ = client.cmd(contract::Cmd::MobileStop {});
    }
    // The daemon is asked to stop itself, so its engines end as they should (REQ-891); a daemon
    // that does not know the command, or does not go, is signalled as before.
    if let Ok(mut client) = wire::Client::connect(&config.socket, &identity::peer("stop"))
        && matches!(client.cmd(contract::Cmd::Stop {}), Ok(Ok(_)))
    {
        let end = std::time::Instant::now() + STOP_WAIT;
        while std::time::Instant::now() < end {
            if !config.socket.exists() {
                return Ok(ExitCode::SUCCESS);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let pid_file = config.pid_file();
    let pid = std::fs::read_to_string(&pid_file)
        .map_err(|e| format!("no daemon recorded in {}: {e}", pid_file.display()))?;
    let pid: u32 = pid
        .trim()
        .parse()
        .map_err(|_| format!("{} does not hold a pid", pid_file.display()))?;
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map_err(|e| e.to_string())?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Say hello as `name`: the daemon that answered and the session node id.
fn hello(conn: &mut Conn, name: String) -> Result<(Peer, String), String> {
    let client = Peer {
        name,
        version: version::VERSION.into(),
    };
    conn.send(&ClientMsg::Hello {
        client,
        accepts: wire::default_accepts(),
    })
    .map_err(|e| e.to_string())?;
    let line = conn
        .recv_line()
        .map_err(|e| e.to_string())?
        .ok_or("the daemon closed the connection")?;
    match serde_json::from_str(&line).map_err(|e| e.to_string())? {
        DaemonMsg::Hello { root, daemon, .. } => Ok((daemon, root)),
        other => Err(format!("expected hello, got {other:?}")),
    }
}

/// Connect and say hello, returning the connection and the session node id.
fn connect() -> Result<(Conn, String), String> {
    let socket = identity::socket_path();
    let mut conn =
        Conn::connect(&socket).map_err(|e| format!("cannot reach {}: {e}", socket.display()))?;
    let (daemon, root) = hello(&mut conn, identity::peer("cli"))?;
    version::warn(&daemon);
    Ok((conn, root))
}

fn watch(node: Option<&str>) -> Res {
    let (mut conn, root) = connect()?;
    let node = node.map(str::to_owned).unwrap_or(root);
    conn.send(&ClientMsg::Open {
        node,
        epoch: None,
        since: None,
    })
    .map_err(|e| e.to_string())?;
    while let Some(line) = conn.recv_line().map_err(|e| e.to_string())? {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd(json: &str) -> Res {
    let cmd: contract::Cmd =
        serde_json::from_str(json).map_err(|e| format!("not a command: {e}"))?;
    let (mut conn, _) = connect()?;
    conn.set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    conn.send(&ClientMsg::Cmd {
        id: "1".into(),
        cmd,
    })
    .map_err(|e| e.to_string())?;
    while let Some(line) = conn.recv_line().map_err(|e| e.to_string())? {
        if let Ok(DaemonMsg::Reply { id, outcome }) = serde_json::from_str(&line)
            && id == "1"
        {
            println!("{line}");
            return Ok(match outcome {
                contract::Outcome::Ok { .. } => ExitCode::SUCCESS,
                contract::Outcome::Err { .. } => ExitCode::FAILURE,
            });
        }
    }
    Err("the daemon closed the connection before replying".into())
}

#[cfg(test)]
mod tests {
    /// The binary's name is the one name `identity.toml` cannot derive: Cargo spells it.
    #[test]
    fn the_binary_is_named_as_the_identity_says() {
        assert_eq!(env!("CARGO_BIN_NAME"), identity::get().name);
    }
}
