//! The tools jw drives: git, the forge (gh) and the shell. Port of
//! internal/connectors, without herdr: the daemon replaces the multiplexer.
//!
//! The forge and the shell sit behind traits so actions can be tested with
//! fakes, as internal/commands/fakes_test.go does. git stays concrete and is
//! tested against real repositories, like the Go code.

pub mod git;
pub mod github;
pub mod system;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pr {
    pub number: u64,
    /// OPEN, CLOSED or MERGED.
    pub state: String,
    pub is_draft: bool,
    pub url: String,
    pub branch: String,
    /// The commit the forge has for the branch.
    pub head_sha: String,
    /// RFC 3339, `None` unless merged.
    pub merged_at: Option<String>,
}

impl Pr {
    /// The state in lower case, with "draft" for open drafts.
    pub fn status(&self) -> String {
        if self.is_draft && self.state == "OPEN" {
            return "draft".into();
        }
        self.state.to_lowercase()
    }

    /// The short form the sidebar shows: "#12 merged", "#9 draft".
    pub fn label(&self) -> String {
        format!("#{} {}", self.number, self.status())
    }
}

/// Reads PR state. `dir` is any directory of the repository.
pub trait PullRequests {
    /// The newest PR whose head is `branch`.
    fn for_branch(&self, dir: &Path, branch: &str) -> Result<Option<Pr>>;
    /// The newest PR of each head branch.
    fn by_branch(&self, dir: &Path) -> Result<BTreeMap<String, Pr>>;
}

/// The process listening on a port. Every field is unknown (zero/empty) when
/// the OS can't tell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortOwner {
    pub pid: u32,
    pub cmdline: String,
    /// Where it runs: tells which worktree started it.
    pub cwd: String,
}

/// Runs command lines outside the panes: setup, sync hooks, port checks.
/// Commands that need a terminal run in a daemon pane instead.
pub trait Shell {
    /// Runs `cmdline` with `sh -c` in `dir` and returns its combined output.
    /// A non-zero exit is an error carrying that output.
    fn run(&self, dir: &Path, env: &[(String, String)], cmdline: &str) -> Result<String>;
    /// Whether something listens on a local port and, when the OS can tell,
    /// which process.
    fn port_owner(&self, port: u16) -> Option<PortOwner>;
}
