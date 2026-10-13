//! `jwn`: the daemon and a few JSON-speaking commands to drive and watch it.

mod bench;
mod tui;

use std::process::ExitCode;
use std::time::Duration;

use contract::{ClientMsg, DaemonMsg, Peer};
use daemon::{Config, Daemon};
use wire::Conn;

const USAGE: &str = "usage:
  jwn daemon              run the daemon (foreground)
  jwn stop                stop the daemon recorded in the pid file
  jwn watch [NODE]        print every JSON line for NODE (default: the session)
  jwn cmd '<json cmd>'    send one command, print its reply (exit 1 if it failed)
  jwn tui                 a small terminal client of the daemon
  jwn bench               measure sizes and latency on a private daemon";

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
        ["watch"] => watch(None),
        ["watch", node] => watch(Some(node)),
        ["cmd", json] => cmd(json),
        ["bench"] => bench::run().map(|()| ExitCode::SUCCESS),
        ["tui"] => tui::run().map(|()| ExitCode::SUCCESS),
        _ => Err(USAGE.into()),
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

fn run_daemon() -> Res {
    let daemon =
        Daemon::start(Config::from_identity()).map_err(|e| format!("cannot start: {e}"))?;
    daemon.wait();
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

/// Connect and say hello, returning the connection and the session node id.
fn connect() -> Result<(Conn, String), String> {
    let socket = identity::socket_path();
    let mut conn =
        Conn::connect(&socket).map_err(|e| format!("cannot reach {}: {e}", socket.display()))?;
    let client = Peer {
        name: "jwn-cli".into(),
        version: env!("CARGO_PKG_VERSION").into(),
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
        DaemonMsg::Hello { root, .. } => Ok((conn, root)),
        other => Err(format!("expected hello, got {other:?}")),
    }
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
