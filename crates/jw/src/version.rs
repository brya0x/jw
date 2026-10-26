//! `jw --version` and the warning of a daemon of another version (N182, REQ-987 to REQ-990).

use std::process::ExitCode;
use std::sync::Once;
use std::time::Duration;

use contract::{ClientMsg, DaemonMsg, Peer};
use wire::Conn;

/// How long a daemon has to answer a `hello` to be said to run.
const ANSWER: Duration = Duration::from_secs(2);

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The daemon on the socket, if one answers: it is never started and never waited for.
pub fn daemon() -> Option<Peer> {
    let mut conn = Conn::connect(&identity::socket_path()).ok()?;
    conn.set_read_timeout(Some(ANSWER)).ok()?;
    conn.send(&ClientMsg::Hello {
        client: Peer {
            name: identity::peer("version"),
            version: VERSION.into(),
        },
        accepts: wire::default_accepts(),
    })
    .ok()?;
    match serde_json::from_str(&conn.recv_line().ok()??).ok()? {
        DaemonMsg::Hello { daemon, .. } => Some(daemon),
        _ => None,
    }
}

pub fn show() -> ExitCode {
    let name = &identity::get().name;
    println!("{name} {VERSION}");
    match daemon() {
        Some(d) => println!("daemon {}", d.version),
        None => println!("daemon: not running"),
    }
    ExitCode::SUCCESS
}

/// The two versions are different strings as they travel in the `hello`; there is no order (RAT-163).
fn differs(daemon: &str, binary: &str) -> bool {
    daemon != binary
}

fn line(daemon: &str, binary: &str) -> String {
    let name = &identity::get().name;
    format!(
        "{name}: the daemon is {daemon} and this binary is {binary}: {name} stop and open it again (it closes every pane, so leave it to the person)"
    )
}

/// Once per process, on standard error, and never more than a line (REQ-989, REQ-990).
pub fn warn(daemon: &Peer) {
    static ONCE: Once = Once::new();
    if differs(&daemon.version, VERSION) {
        ONCE.call_once(|| eprintln!("{}", line(&daemon.version, VERSION)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_compared_as_strings_in_either_order() {
        assert!(differs("0.0.46", "0.0.50"));
        assert!(differs("0.0.50", "0.0.46"));
        assert!(differs("", "0.0.50"));
        assert!(!differs("0.0.50", "0.0.50"));
    }

    #[test]
    fn the_line_names_both_versions_and_the_command() {
        let l = line("0.0.46", "0.0.50");
        let name = &identity::get().name;
        assert!(l.contains("daemon is 0.0.46") && l.contains("binary is 0.0.50"));
        assert!(l.contains(&format!("{name} stop")) && !l.contains('\n'));
    }
}
