//! Who else has a conversation's session open (spec REQ-575). Claude Code writes a file for each
//! live process, `<claude folder>/sessions/<pid>.json`, with the session it has open, the folder
//! it runs in and whether it works or waits: the daemon reads them, so a `claude` that is only
//! waiting for a message is seen as well as one that is writing.

use std::collections::BTreeMap;
use std::process::Command;

use contract::conversation::Held;
use serde_json::Value;

/// Whether the process `pid` is alive, is a `claude` (a number may be given to another program
/// after one ends) and is not one this daemon started: its own engines, one that is ending
/// while another begins, are not another `claude`.
fn is_another_claude(pid: u32, ours: u32) -> bool {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid=,command="])
        .output();
    let Ok(out) = out else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    let (parent, command) = text.split_once(' ').unwrap_or((text, ""));
    parent.trim() != ours.to_string() && command.contains("claude")
}

/// Another process of Claude that has `session` open, when there is one: the one that works if
/// more than one does.
pub(crate) fn holder(env: Option<&BTreeMap<String, String>>, session: &str) -> Option<Held> {
    holder_except(env, session, std::process::id())
}

/// `holder`, not counting the processes that `ours` started.
fn holder_except(env: Option<&BTreeMap<String, String>>, session: &str, ours: u32) -> Option<Held> {
    let dir = crate::settings::claude_dir(env)?.join("sessions");
    let mut found: Vec<Held> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            (v["sessionId"] == session).then_some(v)
        })
        .filter_map(|v| {
            let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
            is_another_claude(pid, ours).then(|| Held {
                pid,
                cwd: v["cwd"].as_str().unwrap_or_default().to_owned(),
                busy: v["status"] == "busy",
            })
        })
        .collect();
    found.sort_by_key(|h| !h.busy);
    found.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-575: a live process with the session is the holder, one with another session or that
    /// is not alive is not, and the one that works is named when there are two.
    #[test]
    fn a_live_claude_with_the_session_is_the_holder_and_nothing_else_is() {
        let dir = std::env::temp_dir().join(format!("jwn-sessions-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let env: BTreeMap<String, String> = [(
            "CLAUDE_CONFIG_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        )]
        .into();
        let write = |pid: u32, session: &str, status: &str| {
            std::fs::write(
                sessions.join(format!("{pid}.json")),
                format!(
                    r#"{{"pid":{pid},"sessionId":"{session}","cwd":"/work","status":"{status}"}}"#
                ),
            )
            .unwrap();
        };
        assert_eq!(holder_except(Some(&env), "s1", 1), None, "nothing is open");
        // a live process that is not a `claude`
        let mut other = Command::new("sh")
            .args(["-c", "sleep 30; :", "otro"])
            .spawn()
            .unwrap();
        write(other.id(), "s1", "idle");
        assert_eq!(holder_except(Some(&env), "s1", 1), None, "not a claude");
        let _ = other.kill();
        let _ = other.wait();
        std::fs::remove_file(sessions.join(format!("{}.json", other.id()))).unwrap();
        // a process that is not there
        write(4_000_000, "s1", "busy");
        assert_eq!(holder_except(Some(&env), "s1", 1), None, "not alive");
        // a live process whose command says `claude`, with the session, and one with another
        let mut live = Command::new("sh")
            .args(["-c", "sleep 30; :", "claude"])
            .spawn()
            .unwrap();
        write(live.id(), "s1", "idle");
        let h = holder_except(Some(&env), "s1", 1).expect("the holder");
        assert_eq!((h.pid, h.cwd.as_str(), h.busy), (live.id(), "/work", false));
        assert_eq!(
            holder_except(Some(&env), "otra", 1),
            None,
            "another session"
        );
        write(live.id(), "s1", "busy");
        assert!(holder_except(Some(&env), "s1", 1).unwrap().busy);
        assert_eq!(
            holder_except(Some(&env), "s1", std::process::id()),
            None,
            "one that the daemon started is not another claude"
        );
        let _ = live.kill();
        let _ = live.wait();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
