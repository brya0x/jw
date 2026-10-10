//! `jw server status|stop`: the daemon seen from outside, like herdr's
//! `herdr status` and `herdr server stop` (REQ-80). The client starts the
//! daemon; these say whether it runs and stop it without hunting its pid.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::client::Client;
use crate::proto::{ClientMsg, DaemonMsg, PROTOCOL, pidfile, socket_path};

/// How long `stop` waits for the daemon to save and go.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [a] if a == "status" => status(&socket_path()),
        [a] if a == "stop" => stop(&socket_path()),
        _ => bail!("usage: jw server status|stop"),
    }
}

fn status(socket: &Path) -> Result<()> {
    let Ok(mut c) = Client::connect(socket) else {
        println!("not running (socket {})", socket.display());
        return Ok(());
    };
    c.set_read_timeout(Some(Duration::from_secs(3)))?;
    let pid = pid(socket).map_or("?".into(), |p| p.to_string());
    c.send(&ClientMsg::Hello { protocol: PROTOCOL })?;
    let protocol = loop {
        match c.recv()? {
            Some(DaemonMsg::Hello { protocol }) => break Some(protocol),
            Some(DaemonMsg::Error { .. }) | None => break None,
            Some(_) => {}
        }
    };
    if protocol != Some(PROTOCOL) {
        println!(
            "running, pid {pid}, from another build of jw: jw server stop, then open jw again\n\
             socket {}",
            socket.display()
        );
        return Ok(());
    }
    c.send(&ClientMsg::List)?;
    let panes = loop {
        match c.recv()? {
            Some(DaemonMsg::Panes { panes }) => break panes,
            Some(DaemonMsg::Error { msg }) => bail!("{msg}"),
            None => bail!("the daemon hung up"),
            Some(_) => {}
        }
    };
    let alive = panes.iter().filter(|p| p.exited.is_none()).count();
    println!(
        "running, pid {pid}, {alive} {} running\nsocket {}",
        if alive == 1 { "pane" } else { "panes" },
        socket.display()
    );
    Ok(())
}

/// Sends SIGTERM, on which the daemon saves `session.json` and the
/// scrollback before it exits (REQ-79), and waits for it to go.
fn stop(socket: &Path) -> Result<()> {
    if Client::connect(socket).is_err() {
        println!("not running");
        return Ok(());
    }
    // Only a daemon that answers on the socket: its pidfile is its own.
    let pid = pid(socket).with_context(|| format!("reading {}", pidfile(socket).display()))?;
    // SAFETY: kill(2) with a pid read from the daemon's own pidfile.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error()).context(format!("signalling pid {pid}"));
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Client::connect(socket).is_ok() {
        if Instant::now() > deadline {
            bail!("pid {pid} is still running after {STOP_TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    println!(
        "stopped (pid {pid}). Its panes closed; jw brings the workspaces and their scrollback back when it opens."
    );
    Ok(())
}

fn pid(socket: &Path) -> Option<libc::pid_t> {
    std::fs::read_to_string(pidfile(socket))
        .ok()?
        .trim()
        .parse()
        .ok()
}
