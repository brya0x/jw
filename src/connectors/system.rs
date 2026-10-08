//! [`Shell`] for unix. Port of internal/connectors/system (shell_unix.go);
//! Windows is not ported (docs/specs/rust-tui.md, OPEN-4).

use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Result, bail};

use super::{PortOwner, Shell};

#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Shell for System {
    fn run(&self, dir: &Path, env: &[(String, String)], cmdline: &str) -> Result<String> {
        // One pipe for both streams keeps stdout and stderr in the order the
        // command wrote them.
        let (mut reader, writer) = std::io::pipe()?;
        let mut cmd = Command::new("sh");
        cmd.args(["-c", cmdline])
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer);
        let mut child = cmd.spawn()?;
        drop(cmd); // closes our copies of the write end, so the read sees EOF
        let mut out = String::new();
        reader.read_to_string(&mut out)?;
        let status = child.wait()?;
        if !status.success() {
            bail!("{cmdline}: {status}\n{}", out.trim_end());
        }
        Ok(out)
    }

    /// Asks lsof who listens on the port. Without lsof, or when the process
    /// belongs to another user, the port is still reported busy, with an
    /// unknown owner.
    fn port_owner(&self, port: u16) -> Option<PortOwner> {
        if !listening(port) {
            return None;
        }
        let mut owner = PortOwner::default();
        let Some(pid) = output(
            "lsof",
            &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"],
        )
        .and_then(|o| o.split_whitespace().next()?.parse::<u32>().ok()) else {
            return Some(owner);
        };
        owner.pid = pid;
        let pid = pid.to_string();
        if let Some(cmd) = output("ps", &["-o", "command=", "-p", &pid]) {
            owner.cmdline = cmd.trim().to_string();
        }
        // -Fn prints fields one per line; the cwd is the line starting with n.
        if let Some(cwd) = output("lsof", &["-a", "-p", &pid, "-d", "cwd", "-Fn"])
            && let Some(n) = cwd.lines().filter_map(|l| l.strip_prefix('n')).next_back()
        {
            owner.cwd = n.to_string();
        }
        Some(owner)
    }
}

/// Whether anything accepts connections on the port, over IPv4 or IPv6 —
/// dev servers often bind only one of them (Vite: localhost, which can
/// resolve to ::1). A port we can't bind counts as taken too.
fn listening(port: u16) -> bool {
    let hosts = [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ];
    if hosts
        .iter()
        .any(|a| TcpStream::connect_timeout(a, Duration::from_millis(200)).is_ok())
    {
        return true;
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
}

fn output(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_both_streams_in_order() {
        let d = tempfile::tempdir().unwrap();
        let env = [("JW_NAME".to_string(), "web".to_string())];
        let out = System
            .run(d.path(), &env, "echo out; echo err >&2; echo $JW_NAME; pwd")
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(&lines[..3], ["out", "err", "web"]);
        assert_eq!(
            Path::new(lines[3]).canonicalize().unwrap(),
            d.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn run_fails_with_the_output() {
        let d = tempfile::tempdir().unwrap();
        let err = System.run(d.path(), &[], "echo boom; exit 3").unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[test]
    fn port_owner_sees_a_real_listener() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let owner = System
            .port_owner(port)
            .expect("a port we listen on is busy");
        // lsof may be missing on a minimal machine: then the owner is
        // unknown, but if it's there it must name this very process.
        assert!(
            owner.pid == 0 || owner.pid == std::process::id(),
            "{owner:?}"
        );
        drop(l);
        assert_eq!(System.port_owner(port), None, "a closed port is free");
    }
}
