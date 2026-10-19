//! Who else has a conversation's session open (spec REQ-575). Claude Code writes a file for each
//! live process, `<claude folder>/sessions/<pid>.json`, with the session it has open, the folder
//! it runs in and whether it works or waits: the daemon reads them, so a `claude` that is only
//! waiting for a message is seen as well as one that is writing.

use std::collections::BTreeMap;
use std::process::Command;

use contract::conversation::Held;
use serde_json::Value;

/// Whether the process `pid` is alive and is a `claude` (a number may be given to another
/// program after one ends).
fn is_claude(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("claude"))
}

/// Another process of Claude that has `session` open, when there is one: the one that works if
/// more than one does. `own` is the engine this conversation started last, which may still be
/// ending when another begins: it is not another `claude`, and the engine of another
/// conversation of this daemon on the same session is.
pub(crate) fn holder(
    env: Option<&BTreeMap<String, String>>,
    session: &str,
    own: Option<u32>,
) -> Option<Held> {
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
            (Some(pid) != own && is_claude(pid)).then(|| Held {
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

    /// A process that is killed when the test ends, however it ends.
    struct Proc(std::process::Child);

    impl Proc {
        /// A live process whose command says `name`.
        fn named(name: &str) -> Proc {
            Proc(
                Command::new("bash")
                    .args(["-c", &format!("exec -a {name} sleep 30")])
                    .spawn()
                    .unwrap(),
            )
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// REQ-575: a live process with the session is the holder, one with another session, one
    /// that is not a claude, one that is not alive and the engine the conversation started are
    /// not, and the one that works is named when there are two.
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
        let held = |session: &str, own: Option<u32>| holder(Some(&env), session, own);
        assert_eq!(held("s1", None), None, "nothing is open");
        let other = Proc::named("otro");
        write(other.pid(), "s1", "idle");
        assert_eq!(held("s1", None), None, "not a claude");
        write(4_000_000, "s1", "busy");
        assert_eq!(held("s1", None), None, "not alive");
        let live = Proc::named("claude");
        write(live.pid(), "s1", "idle");
        let h = held("s1", None).expect("the holder");
        assert_eq!(
            (h.pid, h.cwd.as_str(), h.busy),
            (live.pid(), "/work", false)
        );
        assert_eq!(held("otra", None), None, "another session");
        write(live.pid(), "s1", "busy");
        assert!(held("s1", None).unwrap().busy);
        assert_eq!(
            held("s1", Some(live.pid())),
            None,
            "the engine the conversation started is not another claude"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
