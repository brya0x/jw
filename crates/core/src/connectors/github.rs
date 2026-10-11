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
        let mut all = vec!["pr", "list", "--state", "all", "--json", FIELDS];
        all.extend(args);
        let out = self.gh(dir, &all)?;
        let raw: Vec<PrJson> =
            serde_json::from_slice(&out).map_err(|e| anyhow!("gh pr list: {e}"))?;
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

/// An open PR as the review picker lists it (REQ-163, REQ-164).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    /// The head branch.
    pub branch: String,
    pub base: String,
    pub is_draft: bool,
    /// RFC 3339.
    pub updated_at: String,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED or empty.
    pub review: String,
    pub files: u64,
    /// The paths of the changed files, for the preview.
    pub paths: Vec<String>,
    pub additions: u64,
    pub deletions: u64,
    pub url: String,
}

/// A comment, a review or an inline comment: who said what.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Remark {
    pub author: String,
    pub body: String,
    /// A review's verdict (APPROVED, CHANGES_REQUESTED, COMMENTED).
    pub state: String,
    /// An inline comment's file and line.
    pub path: String,
    pub line: Option<u64>,
}

/// One changed file of a PR.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changed {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Everything the agent needs to know about a PR besides its diff.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrDetail {
    pub title: String,
    pub body: String,
    pub author: String,
    pub branch: String,
    pub base: String,
    pub url: String,
    pub comments: Vec<Remark>,
    pub reviews: Vec<Remark>,
    pub inline: Vec<Remark>,
    pub files: Vec<Changed>,
    /// Why the inline comments are missing, when they are.
    pub inline_error: Option<String>,
}

/// gh prints `null` for a deleted account or no decision.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize, Default)]
struct Login {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryJson {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default, deserialize_with = "null_default")]
    body: String,
    #[serde(default, deserialize_with = "null_default")]
    author: Login,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    updated_at: String,
    #[serde(default, deserialize_with = "null_default")]
    review_decision: String,
    #[serde(default)]
    changed_files: u64,
    #[serde(default)]
    files: Vec<ChangedJson>,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemarkJson {
    #[serde(default, deserialize_with = "null_default")]
    author: Login,
    #[serde(default, deserialize_with = "null_default")]
    body: String,
    #[serde(default)]
    state: String,
}

#[derive(Deserialize)]
struct ChangedJson {
    #[serde(default)]
    path: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailJson {
    #[serde(default)]
    title: String,
    #[serde(default, deserialize_with = "null_default")]
    body: String,
    #[serde(default, deserialize_with = "null_default")]
    author: Login,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    comments: Vec<RemarkJson>,
    #[serde(default)]
    reviews: Vec<RemarkJson>,
    #[serde(default)]
    files: Vec<ChangedJson>,
}

#[derive(Deserialize)]
struct InlineJson {
    #[serde(default, deserialize_with = "null_default")]
    user: Login,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    original_line: Option<u64>,
    #[serde(default, deserialize_with = "null_default")]
    body: String,
}

const SUMMARY_FIELDS: &str = "number,title,body,author,headRefName,baseRefName,isDraft,updatedAt,reviewDecision,changedFiles,files,additions,deletions,url";
const DETAIL_FIELDS: &str = "title,body,author,headRefName,baseRefName,url,comments,reviews,files";

impl Client {
    /// Runs gh in `dir` and returns its stdout; a failure carries gh's own
    /// message (REQ-162).
    fn gh(&self, dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let bin = self.bin()?;
        let out = Command::new(&bin)
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| anyhow!("{}: {e}", bin.display()))?;
        if !out.status.success() {
            bail!(
                "gh {}: {}",
                args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }

    /// The open PRs of the repository, the latest update first. With
    /// `requested`, only those whose review is asked of the user (REQ-179).
    pub fn open_prs(&self, dir: &Path, requested: bool) -> Result<Vec<PrSummary>> {
        let mut args = vec![
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            SUMMARY_FIELDS,
        ];
        if requested {
            args.extend(["--search", "review-requested:@me"]);
        }
        let out = self.gh(dir, &args)?;
        let raw: Vec<SummaryJson> =
            serde_json::from_slice(&out).map_err(|e| anyhow!("gh pr list: {e}"))?;
        let mut prs: Vec<PrSummary> = raw
            .into_iter()
            .map(|p| PrSummary {
                number: p.number,
                title: p.title,
                body: p.body,
                author: p.author.login,
                branch: p.head_ref_name,
                base: p.base_ref_name,
                is_draft: p.is_draft,
                updated_at: p.updated_at,
                review: p.review_decision,
                files: p.changed_files,
                paths: p.files.into_iter().map(|f| f.path).collect(),
                additions: p.additions,
                deletions: p.deletions,
                url: p.url,
            })
            .collect();
        prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(prs)
    }

    /// The conversation of PR `n`, its reviews, files and inline comments
    /// (REQ-170). Inline comments are best effort: a failure there is kept
    /// in `inline_error` and the rest still comes back.
    pub fn detail(&self, dir: &Path, n: u64) -> Result<PrDetail> {
        let num = n.to_string();
        let out = self.gh(dir, &["pr", "view", &num, "--json", DETAIL_FIELDS])?;
        let d: DetailJson = serde_json::from_slice(&out).map_err(|e| anyhow!("gh pr view: {e}"))?;
        let remark = |r: RemarkJson| Remark {
            author: r.author.login,
            body: r.body,
            state: r.state,
            ..Remark::default()
        };
        let (inline, inline_error) = match self
            .gh(
                dir,
                &[
                    "api",
                    &format!("repos/{{owner}}/{{repo}}/pulls/{n}/comments"),
                ],
            )
            .and_then(|o| {
                serde_json::from_slice::<Vec<InlineJson>>(&o).map_err(|e| anyhow!("gh api: {e}"))
            }) {
            Ok(list) => (
                list.into_iter()
                    .map(|c| Remark {
                        author: c.user.login,
                        body: c.body,
                        path: c.path,
                        line: c.line.or(c.original_line),
                        ..Remark::default()
                    })
                    .collect(),
                None,
            ),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        Ok(PrDetail {
            title: d.title,
            body: d.body,
            author: d.author.login,
            branch: d.head_ref_name,
            base: d.base_ref_name,
            url: d.url,
            comments: d.comments.into_iter().map(remark).collect(),
            reviews: d.reviews.into_iter().map(remark).collect(),
            inline,
            files: d
                .files
                .into_iter()
                .map(|f| Changed {
                    path: f.path,
                    additions: f.additions,
                    deletions: f.deletions,
                })
                .collect(),
            inline_error,
        })
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

    /// Runs `bin` until the OS lets it. Linux refuses to exec a file that a
    /// thread's `fork` still holds open for writing (ETXTBSY), and the tests
    /// fork in parallel, so a script written a moment ago can fail to start.
    fn settle(bin: &Path) {
        for _ in 0..100 {
            match std::process::Command::new(bin).output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                _ => return,
            }
        }
    }

    fn fake_gh(dir: &Path, reply: &str) -> Client {
        let bin = dir.join("gh");
        std::fs::write(&bin, format!("#!/bin/sh\ncat <<'EOF'\n{reply}\nEOF\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        settle(&bin);
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

    /// A gh that logs its arguments to `args` and answers `reply` (REQ-179).
    fn logging_gh(dir: &Path, reply: &str) -> (Client, PathBuf) {
        let bin = dir.join("gh");
        let log = dir.join("args");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\ncat <<'EOF'\n{reply}\nEOF\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        settle(&bin);
        // The run that settled it logged its own call.
        let _ = std::fs::remove_file(&log);
        (Client { bin: Some(bin) }, log)
    }

    #[test]
    fn open_prs_newest_update_first_and_null_fields_read() {
        let d = tempfile::tempdir().unwrap();
        let (gh, log) = logging_gh(
            d.path(),
            r#"[{"number":1,"title":"old","author":{"login":"ana"},"headRefName":"a","baseRefName":"main","updatedAt":"2026-10-01T00:00:00Z","reviewDecision":"","changedFiles":2,"additions":5,"deletions":1},
                {"number":2,"title":"new","author":null,"body":null,"headRefName":"b","baseRefName":"main","isDraft":true,"updatedAt":"2026-10-09T00:00:00Z","reviewDecision":null}]"#,
        );
        let prs = gh.open_prs(d.path(), false).unwrap();
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), [2, 1]);
        assert_eq!((prs[0].author.as_str(), prs[0].is_draft), ("", true));
        assert_eq!(
            (prs[1].author.as_str(), prs[1].files, prs[1].additions),
            ("ana", 2, 5)
        );
        let args = std::fs::read_to_string(log).unwrap();
        assert!(
            args.contains("pr list --state open") && !args.contains("--search"),
            "{args}"
        );
    }

    #[test]
    fn open_prs_requested_asks_for_the_users_reviews() {
        let d = tempfile::tempdir().unwrap();
        let (gh, log) = logging_gh(d.path(), "[]");
        assert!(gh.open_prs(d.path(), true).unwrap().is_empty());
        let args = std::fs::read_to_string(log).unwrap();
        assert!(args.contains("--search review-requested:@me"), "{args}");
    }

    #[test]
    fn open_prs_reports_gh_failing() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("gh");
        std::fs::write(&bin, "#!/bin/sh\necho 'not logged in' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let gh = Client { bin: Some(bin) };
        let err = gh.open_prs(d.path(), false).unwrap_err();
        assert_eq!(format!("{err}"), "gh pr list: not logged in");
        let missing = Client {
            bin: Some(d.path().join("nope")),
        };
        assert!(missing.open_prs(d.path(), false).is_err());
    }

    #[test]
    fn detail_keeps_the_conversation_and_survives_inline_failing() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("gh");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
if [ "$1" = api ]; then echo 'HTTP 404' >&2; exit 1; fi
echo '{"title":"T","author":{"login":"ana"},"comments":[{"author":{"login":"bo"},"body":"hi"}],"reviews":[{"author":{"login":"cy"},"state":"APPROVED","body":""}],"files":[{"path":"a.rs","additions":2,"deletions":1}]}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let det = Client { bin: Some(bin) }.detail(d.path(), 3).unwrap();
        assert_eq!((det.title.as_str(), det.author.as_str()), ("T", "ana"));
        assert_eq!(det.comments[0].body, "hi");
        assert_eq!(det.reviews[0].state, "APPROVED");
        assert_eq!(
            det.files[0],
            Changed {
                path: "a.rs".into(),
                additions: 2,
                deletions: 1
            }
        );
        assert!(det.inline.is_empty());
        assert_eq!(
            det.inline_error.as_deref(),
            Some("gh api repos/{owner}/{repo}/pulls/3/comments: HTTP 404")
        );
    }
}
