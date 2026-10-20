//! The Cloudflare quick tunnel (REQ-80): `cloudflared` is run against the bridge's loopback
//! port, its host name is read from what it prints, and the QR is offered only once that
//! name resolves.

use std::io::{self, BufRead, BufReader};
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// A running tunnel; it ends with the value.
pub struct Tunnel {
    pub host: String,
    child: Child,
    pidfile: Option<PathBuf>,
}

impl Tunnel {
    /// Whether the process is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(p) = &self.pidfile {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// End a tunnel left running by a daemon that did not get to stop it (it was killed, or
/// asked to end by a signal). Only a process that still looks like a tunnel to `program` is
/// signalled: the pid may belong to something else by now. (RISK-21.)
pub fn reap(pidfile: &Path, program: &str) {
    let Some(pid) = std::fs::read_to_string(pidfile)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
    else {
        return;
    };
    let _ = std::fs::remove_file(pidfile);
    let Ok(out) = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
    else {
        return;
    };
    let command = String::from_utf8_lossy(&out.stdout);
    if command.contains(program) && command.contains("tunnel --url") {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
}

/// The `….trycloudflare.com` name in a line `cloudflared` printed, if there is one.
pub fn find_host(line: &str) -> Option<String> {
    let rest = &line[line.find("https://")? + "https://".len()..];
    let host: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        .collect();
    host.ends_with(".trycloudflare.com").then_some(host)
}

#[derive(Debug)]
pub enum TunnelError {
    /// `cloudflared` could not be started (not installed, usually).
    Start(io::Error),
    /// It ended, or said nothing useful, before a host name came.
    NoHost(String),
    /// The name never resolved.
    NotResolving(String),
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelError::Start(e) => write!(f, "cannot start cloudflared: {e}"),
            TunnelError::NoHost(last) => {
                write!(f, "cloudflared gave no tunnel address (last said: {last})")
            }
            TunnelError::NotResolving(host) => write!(f, "{host} does not resolve yet"),
        }
    }
}

impl std::error::Error for TunnelError {}

impl Tunnel {
    /// Start `program` as a quick tunnel to `127.0.0.1:port` and wait up to `wait` for its host
    /// name and, with `resolve`, for the name to resolve in DNS.
    pub fn up(
        program: &str,
        port: u16,
        resolve: bool,
        wait: Duration,
        pidfile: Option<&Path>,
    ) -> Result<Tunnel, TunnelError> {
        let mut child = Command::new(program)
            .args([
                "tunnel",
                "--url",
                &format!("http://127.0.0.1:{port}"),
                "--no-autoupdate",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(TunnelError::Start)?;
        if let Some(p) = pidfile {
            let _ = std::fs::write(p, child.id().to_string());
        }
        let stderr = child.stderr.take().expect("stderr was piped");
        let (tx, rx) = mpsc::channel::<(Option<String>, String)>();
        // It reads for as long as the tunnel lives, also after the host is known and nobody listens:
        // a program that writes to a pipe nobody reads any more is killed by SIGPIPE (Go's does
        // this on stderr), and the tunnel would end at its next line of log.
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx.send((find_host(&line), line));
            }
        });
        let started = Instant::now();
        let mut last = String::new();
        let host = loop {
            let left = wait.saturating_sub(started.elapsed());
            match rx.recv_timeout(left) {
                Ok((Some(host), _)) => break host,
                Ok((None, line)) => last = line,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(p) = pidfile {
                        let _ = std::fs::remove_file(p);
                    }
                    return Err(TunnelError::NoHost(last));
                }
            }
        };
        let mut tunnel = Tunnel {
            host,
            child,
            pidfile: pidfile.map(Path::to_path_buf),
        };
        if resolve {
            let name = format!("{}:443", tunnel.host);
            while name.to_socket_addrs().is_err() {
                if started.elapsed() > wait {
                    return Err(TunnelError::NotResolving(std::mem::take(&mut tunnel.host)));
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        Ok(tunnel)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_host_is_the_trycloudflare_name_in_a_line() {
        let line = "2026-10-14T09:00:00Z INF |  https://quiet-river-1234.trycloudflare.com                |";
        assert_eq!(
            find_host(line).as_deref(),
            Some("quiet-river-1234.trycloudflare.com")
        );
        assert_eq!(
            find_host("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(
            find_host("https://api.trycloudflare.com/tunnel"),
            Some("api.trycloudflare.com".into())
        );
        assert_eq!(find_host("https://example.com/x"), None);
    }

    fn script(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("cloudflared");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_tunnel_reads_its_host_and_is_killed_with_its_value() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let program = script(
            dir.path(),
            &format!(
                "echo $$ > {}\necho 'INF starting' >&2\necho 'INF https://fake-one-2.trycloudflare.com' >&2\nexec sleep 30",
                pidfile.display()
            ),
        );
        let t = Tunnel::up(&program, 7000, false, Duration::from_secs(5), None).unwrap();
        assert_eq!(t.host, "fake-one-2.trycloudflare.com");
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(t);
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the tunnel process ended with its value");
    }

    /// What it logs once the host is known is read and dropped: a program that writes to a pipe nobody
    /// reads is killed (the real one died four minutes after it started because of this).
    #[test]
    fn a_tunnel_that_goes_on_logging_after_its_host_is_known_lives() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("lived");
        let program = script(
            dir.path(),
            &format!(
                "echo 'INF https://fake-log-3.trycloudflare.com' >&2\nsleep 1\nfor i in 1 2 3 4 5; do echo \"INF line $i\" >&2; done\ntouch {}\nexec sleep 30",
                mark.display()
            ),
        );
        let mut t = Tunnel::up(&program, 7000, false, Duration::from_secs(5), None).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !mark.exists() && Instant::now() < end {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(mark.exists(), "it died writing its log");
        assert!(t.alive());
    }

    #[test]
    fn a_tunnel_that_ended_says_it_is_not_alive() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "echo 'INF https://fake-end-4.trycloudflare.com' >&2\nsleep 1",
        );
        let mut t = Tunnel::up(&program, 7000, false, Duration::from_secs(5), None).unwrap();
        assert!(t.alive());
        thread::sleep(Duration::from_millis(1600));
        assert!(!t.alive());
    }

    #[test]
    fn a_tunnel_left_by_a_dead_daemon_is_ended_by_the_next_one_and_other_processes_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "sleep 60 &\nwait");
        let pidfile = dir.path().join("tunnel.pid");
        let mut stale = Command::new(&program)
            .args(["tunnel", "--url", "http://127.0.0.1:1", "--no-autoupdate"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(&pidfile, stale.id().to_string()).unwrap();
        reap(&pidfile, &program);
        assert!(
            stale.wait().is_ok() && !pidfile.exists(),
            "the stale tunnel ended"
        );
        // A pid that is now something else is left alone.
        let mut other = Command::new("sleep").arg("30").spawn().unwrap();
        std::fs::write(&pidfile, other.id().to_string()).unwrap();
        reap(&pidfile, &program);
        assert!(
            other.try_wait().unwrap().is_none(),
            "an unrelated process is not signalled"
        );
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn a_tunnel_that_says_nothing_useful_is_an_error_and_a_missing_program_too() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "echo 'ERR no network' >&2\nexit 1");
        let e = Tunnel::up(&program, 7000, false, Duration::from_secs(2), None)
            .err()
            .unwrap();
        assert!(
            matches!(&e, TunnelError::NoHost(last) if last.contains("no network")),
            "{e}"
        );
        let e = Tunnel::up(
            "/nonexistent/cloudflared",
            7000,
            false,
            Duration::from_secs(1),
            None,
        )
        .err()
        .unwrap();
        assert!(matches!(e, TunnelError::Start(_)));
    }
}
