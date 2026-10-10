//! [`PullRequests`] with the gh CLI. Port of internal/connectors/github.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use super::{Pr, PullRequests};

/// Finds gh lazily, on the first call: jw works without gh until something
/// actually needs PR state.
#[derive(Debug, Clone, Default)]
pub struct Client {
    /// The gh binary; `None` means `$JW_GH`, then `gh` on PATH.
    pub bin: Option<PathBuf>,
}

/// gh's shape; callers get [`Pr`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrJson {
    number: u64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    head_ref_oid: String,
    #[serde(default)]
    merged_at: Option<String>,
}

const FIELDS: &str = "number,state,isDraft,url,headRefName,headRefOid,mergedAt";

impl Client {
    fn bin(&self) -> Result<PathBuf> {
        if let Some(b) = &self.bin {
            return Ok(b.clone());
        }
        if let Some(b) = std::env::var_os("JW_GH").filter(|b| !b.is_empty()) {
            return Ok(b.into());
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|d| d.join("gh"))
            .find(|p| p.is_file())
            .ok_or_else(|| anyhow!("gh not found on PATH (set JW_GH to its path)"))
    }

    /// Runs `gh pr list` in `dir`.
    fn list(&self, dir: &Path, args: &[&str]) -> Result<Vec<Pr>> {
        let bin = self.bin()?;
        let out = Command::new(&bin)
            .args(["pr", "list", "--state", "all", "--json", FIELDS])
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| anyhow!("{}: {e}", bin.display()))?;
        if !out.status.success() {
            bail!(
                "gh pr list: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let raw: Vec<PrJson> =
            serde_json::from_slice(&out.stdout).map_err(|e| anyhow!("gh pr list: {e}"))?;
        Ok(raw
            .into_iter()
            .map(|p| Pr {
                number: p.number,
                state: p.state,
                is_draft: p.is_draft,
                url: p.url,
                branch: p.head_ref_name,
                head_sha: p.head_ref_oid,
                // gh prints the zero time for PRs that aren't merged.
                merged_at: p.merged_at.filter(|m| !m.starts_with("0001-")),
            })
            .collect())
    }
}

impl PullRequests for Client {
    fn for_branch(&self, dir: &Path, branch: &str) -> Result<Option<Pr>> {
        Ok(self
            .list(dir, &["--head", branch, "--limit", "1"])?
            .into_iter()
            .next())
    }

    /// Looks at the latest 200 PRs. gh lists newest first, so a reused branch
    /// name keeps its latest PR.
    fn by_branch(&self, dir: &Path) -> Result<BTreeMap<String, Pr>> {
        let mut m = BTreeMap::new();
        for p in self.list(dir, &["--limit", "200"])? {
            m.entry(p.branch.clone()).or_insert(p);
        }
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_gh(dir: &Path, reply: &str) -> Client {
        let bin = dir.join("gh");
        std::fs::write(&bin, format!("#!/bin/sh\ncat <<'EOF'\n{reply}\nEOF\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Client { bin: Some(bin) }
    }

    #[test]
    fn for_branch() {
        let d = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            d.path(),
            r#"[{"number":4,"state":"MERGED","headRefName":"feat/web","headRefOid":"abc","mergedAt":"2026-10-01T10:00:00Z"}]"#,
        );
        let pr = gh.for_branch(d.path(), "feat/web").unwrap().unwrap();
        assert_eq!((pr.number, pr.head_sha.as_str()), (4, "abc"));
        assert_eq!(pr.label(), "#4 merged");
        assert!(pr.merged_at.is_some());
    }

    #[test]
    fn for_branch_none() {
        let d = tempfile::tempdir().unwrap();
        let gh = fake_gh(d.path(), "[]");
        assert_eq!(gh.for_branch(d.path(), "feat/web").unwrap(), None);
    }

    #[test]
    fn by_branch_keeps_newest() {
        let d = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            d.path(),
            r#"[{"number":9,"state":"OPEN","isDraft":true,"headRefName":"feat/web","mergedAt":"0001-01-01T00:00:00Z"},{"number":3,"state":"CLOSED","headRefName":"feat/web"}]"#,
        );
        let prs = gh.by_branch(d.path()).unwrap();
        let pr = &prs["feat/web"];
        assert_eq!(pr.label(), "#9 draft");
        assert_eq!(pr.merged_at, None);
    }
}
