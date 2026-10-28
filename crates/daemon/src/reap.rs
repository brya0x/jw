//! Ending what runs in a workspace before its worktree is removed (REQ-1066, REQ-1067): the tree
//! of every shell and engine the daemon started for it, read from `ps` and signalled by number.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::engine::may_end;

/// How long what was sent `SIGTERM` has to end before it is sent `SIGKILL`.
const GRACE: Duration = Duration::from_secs(2);
const LOOK: Duration = Duration::from_millis(100);

/// A row of `ps`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub tty: String,
    pub zombie: bool,
}

/// `pid ppid pgid tty stat` per line; a line that is not that is skipped.
pub fn parse(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            Some(Proc {
                pid: f.next()?.parse().ok()?,
                ppid: f.next()?.parse().ok()?,
                pgid: f.next()?.parse().ok()?,
                tty: f.next()?.to_owned(),
                zombie: f.next()?.starts_with('Z'),
            })
        })
        .collect()
}

fn snapshot() -> Result<Vec<Proc>, String> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "pgid="])
        .args(["-o", "tty=", "-o", "stat="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("no se pudo leer los procesos: {e}"))?;
    let procs = parse(&String::from_utf8_lossy(&out.stdout));
    if !out.status.success() || procs.is_empty() {
        return Err("no se pudo leer los procesos: `ps` no contestó".into());
    }
    Ok(procs)
}

/// Whether a `ps` terminal column names a terminal (macOS prints `??` and Linux `?` for none).
fn has_tty(tty: &str) -> bool {
    !tty.starts_with('?') && tty != "-"
}

/// The `(pid, pgid)` of what belongs to the `roots`: their descendants, the members of every group
/// of those and, for a root that has a terminal, the processes that have it. Never `own` (the
/// daemon's pid and group), what hangs from `own`, or a number `may_end` refuses.
pub fn targets(procs: &[Proc], roots: &[u32], own: (u32, u32)) -> BTreeSet<(u32, u32)> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut chosen: HashSet<u32> = HashSet::new();
    let mut ttys: HashSet<&str> = HashSet::new();
    let mut stack: Vec<u32> = roots
        .iter()
        .copied()
        .filter(|r| may_end(*r, own.0))
        .collect();
    while let Some(pid) = stack.pop() {
        let Some(p) = by_pid.get(&pid) else { continue };
        if pid == own.0 || !chosen.insert(pid) {
            continue;
        }
        if roots.contains(&pid) && has_tty(&p.tty) {
            ttys.insert(&p.tty);
        }
        stack.extend(children.get(&pid).into_iter().flatten());
    }
    chosen.extend(
        procs
            .iter()
            .filter(|p| ttys.contains(p.tty.as_str()) && p.pid != own.0)
            .map(|p| p.pid),
    );
    let groups: HashSet<u32> = chosen
        .iter()
        .filter_map(|pid| by_pid.get(pid))
        .map(|p| p.pgid)
        .filter(|g| *g > 1 && *g != own.1)
        .collect();
    procs
        .iter()
        .filter(|p| chosen.contains(&p.pid) || groups.contains(&p.pgid))
        .filter(|p| p.pid != own.0 && p.pgid != own.1 && !p.zombie)
        .filter(|p| may_end(p.pid, own.0))
        .map(|p| (p.pid, p.pgid))
        .collect()
}

fn signal(name: &str, pids: impl IntoIterator<Item = u32>) {
    let _ = Command::new("/bin/kill")
        .args(["-s", name])
        .args(pids.into_iter().map(|p| p.to_string()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `SIGHUP` too, because an interactive shell ignores `SIGTERM` and would always cost the wait.
fn ask_to_end(pids: Vec<u32>) {
    signal("TERM", pids.iter().copied());
    signal("HUP", pids);
}

/// What was ended: how many processes were reached and how many of them needed `SIGKILL`.
#[derive(Debug, PartialEq, Eq)]
pub struct Ended {
    pub reached: usize,
    pub killed: usize,
}

/// End everything that belongs to the `roots` (REQ-1066): `SIGTERM` and `SIGHUP`, a wait that ends
/// as soon as nothing lives, `SIGKILL` for the rest. What lives after that is an error that says
/// which processes.
pub fn end(roots: &[u32]) -> Result<Ended, String> {
    let me = std::process::id();
    let procs = snapshot()?;
    let own = procs
        .iter()
        .find(|p| p.pid == me)
        .map(|p| (me, p.pgid))
        .ok_or("no se pudo leer los procesos: falta el del daemon")?;
    let mut known = targets(&procs, roots, own);
    if known.is_empty() {
        return Ok(Ended {
            reached: 0,
            killed: 0,
        });
    }
    ask_to_end(known.iter().map(|(p, _)| *p).collect());
    // A number is alive only with the group it had: a reused one is not the process it was.
    let alive = |known: &BTreeSet<(u32, u32)>, procs: &[Proc]| -> Vec<u32> {
        let now: HashSet<(u32, u32)> = procs
            .iter()
            .filter(|p| !p.zombie)
            .map(|p| (p.pid, p.pgid))
            .collect();
        known
            .iter()
            .filter(|k| now.contains(k))
            .map(|k| k.0)
            .collect()
    };
    let deadline = Instant::now() + GRACE;
    let mut killed = 0;
    let mut hard_deadline = None;
    loop {
        sleep(LOOK);
        let procs = snapshot()?;
        // What a process started meanwhile (a watcher that restarts its child) is ended too.
        let fresh: Vec<u32> = targets(&procs, roots, own)
            .into_iter()
            .filter(|t| known.insert(*t))
            .map(|t| t.0)
            .collect();
        if !fresh.is_empty() {
            ask_to_end(fresh);
        }
        let left = alive(&known, &procs);
        if left.is_empty() {
            return Ok(Ended {
                reached: known.len(),
                killed,
            });
        }
        let now = Instant::now();
        match hard_deadline {
            None if now >= deadline => {
                killed = left.len();
                signal("KILL", left);
                hard_deadline = Some(now + GRACE);
            }
            Some(d) if now >= d => {
                let which: Vec<String> = left.iter().map(u32::to_string).collect();
                return Err(format!(
                    "siguen vivos después de SIGKILL los procesos {}",
                    which.join(", ")
                ));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // pid ppid pgid tty stat
    const TREE: &str = "\
1 0 1 ?? Ss
100 1 100 ttys001 Ss
101 100 101 ttys001 S+
102 101 101 ttys001 S+
103 102 103 ?? S
104 1 101 ?? S
200 1 200 ttys002 Ss
201 200 201 ttys002 S+
300 1 300 ?? S
301 300 300 ?? S
302 301 302 ?? Z
";

    fn pids(t: &BTreeSet<(u32, u32)>) -> Vec<u32> {
        t.iter().map(|p| p.0).collect()
    }

    #[test]
    fn a_shell_takes_its_tree_its_groups_and_its_terminal() {
        let procs = parse(TREE);
        // 103 is a child with a group of its own, 104 was reparented to init but is in 101's group.
        assert_eq!(
            pids(&targets(&procs, &[100], (9999, 9999))),
            [100, 101, 102, 103, 104]
        );
    }

    #[test]
    fn an_engine_takes_its_group_and_no_terminal() {
        let procs = parse(TREE);
        assert_eq!(pids(&targets(&procs, &[300], (9999, 9999))), [300, 301]);
    }

    #[test]
    fn the_daemon_its_group_and_what_hangs_from_it_are_never_chosen() {
        let procs = parse(TREE);
        // The daemon is 102, in the group of 101: it and the group stay, and 103 hangs from it.
        let t = targets(&procs, &[100], (102, 101));
        assert_eq!(pids(&t), [100]);
        // Started from the shell's terminal: the terminal's members do not make it a target.
        let t = targets(&procs, &[200], (201, 201));
        assert_eq!(pids(&t), [200]);
    }

    #[test]
    fn init_and_a_missing_root_are_never_chosen() {
        let procs = parse(TREE);
        assert!(targets(&procs, &[1], (9999, 9999)).is_empty());
        assert!(targets(&procs, &[777], (9999, 9999)).is_empty());
    }

    #[test]
    fn a_process_in_group_one_does_not_pull_the_group_in() {
        let procs = parse("1 0 1 ?? Ss\n100 1 100 ttys001 Ss\n400 100 1 ttys001 S\n401 1 1 ?? S\n");
        assert_eq!(pids(&targets(&procs, &[100], (9999, 9999))), [100, 400]);
    }

    #[test]
    fn a_line_that_is_not_a_process_is_skipped() {
        assert_eq!(parse("pid ppid\n12 1 12 ttys000 S\n").len(), 1);
    }
}
