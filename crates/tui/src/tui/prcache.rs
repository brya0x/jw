//! gh's answer for a branch, kept for a short while (REQ-265): `^␣ X` twice in
//! a row, or `X` and then the done checks, ask gh once. Only answers are kept,
//! never failures, so a gh that was offline is asked again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::connectors::github::Client;
use crate::connectors::{Pr, PullRequests};

/// How long an answer stands (OPEN-38).
const TTL: Duration = Duration::from_secs(30);
/// How long gh gets to answer before the PR line reads "unknown" (REQ-264).
const GH_TIMEOUT: Duration = Duration::from_secs(3);

type Key = (PathBuf, String);
type Memory = Mutex<HashMap<Key, (Instant, Option<Pr>)>>;

static MEMORY: LazyLock<Memory> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// gh with a timeout and the memory above. Hand it to `done_plan` too, so a
/// merged PR costs one gh call, not two.
pub struct Cached {
    gh: Client,
}

pub fn client() -> Cached {
    Cached {
        gh: Client {
            timeout: Some(GH_TIMEOUT),
            ..Client::default()
        },
    }
}

impl PullRequests for Cached {
    fn for_branch(&self, dir: &Path, branch: &str) -> Result<Option<Pr>> {
        lookup(&MEMORY, dir, branch, Instant::now(), TTL, || {
            self.gh.for_branch(dir, branch)
        })
    }

    fn by_branch(&self, dir: &Path) -> Result<std::collections::BTreeMap<String, Pr>> {
        self.gh.by_branch(dir)
    }
}

fn lookup(
    memory: &Memory,
    dir: &Path,
    branch: &str,
    now: Instant,
    ttl: Duration,
    ask: impl FnOnce() -> Result<Option<Pr>>,
) -> Result<Option<Pr>> {
    let key = (dir.to_path_buf(), branch.to_string());
    if let Ok(m) = memory.lock()
        && let Some((at, pr)) = m.get(&key)
        && now.saturating_duration_since(*at) < ttl
    {
        return Ok(pr.clone());
    }
    let pr = ask()?;
    if let Ok(mut m) = memory.lock() {
        m.insert(key, (now, pr.clone()));
    }
    Ok(pr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::cell::Cell;

    fn pr(n: u64) -> Pr {
        Pr {
            number: n,
            ..Pr::default()
        }
    }

    /// REQ-265: inside the TTL gh is not asked again; after it, it is.
    #[test]
    fn an_answer_stands_for_the_ttl() {
        let mem: Memory = Mutex::new(HashMap::new());
        let calls = Cell::new(0);
        let ask = |n| {
            calls.set(calls.get() + 1);
            Ok(Some(pr(n)))
        };
        let t0 = Instant::now();
        let d = Path::new("/r");
        let ttl = Duration::from_secs(30);
        assert_eq!(
            lookup(&mem, d, "b", t0, ttl, || ask(1)).unwrap(),
            Some(pr(1))
        );
        let soon = t0 + Duration::from_secs(29);
        assert_eq!(
            lookup(&mem, d, "b", soon, ttl, || ask(2)).unwrap(),
            Some(pr(1))
        );
        assert_eq!(calls.get(), 1);
        let late = t0 + Duration::from_secs(31);
        assert_eq!(
            lookup(&mem, d, "b", late, ttl, || ask(3)).unwrap(),
            Some(pr(3))
        );
        assert_eq!(calls.get(), 2);
        // Another branch is another question.
        assert_eq!(
            lookup(&mem, d, "c", soon, ttl, || ask(4)).unwrap(),
            Some(pr(4))
        );
    }

    /// A failure is not remembered: the next ask goes to gh again.
    #[test]
    fn a_failure_is_not_kept() {
        let mem: Memory = Mutex::new(HashMap::new());
        let t0 = Instant::now();
        let d = Path::new("/r");
        let ttl = Duration::from_secs(30);
        assert!(lookup(&mem, d, "b", t0, ttl, || bail!("offline")).is_err());
        assert_eq!(
            lookup(&mem, d, "b", t0, ttl, || Ok(Some(pr(1)))).unwrap(),
            Some(pr(1))
        );
    }
}
