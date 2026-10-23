//! N117: the review of a pull request (REQ-581 to REQ-586), with a stand-in for `gh` and a local
//! bare repository for the remote, so no test goes near the network: GitHub's `refs/pull/<n>/head`
//! is a ref of the bare repository.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Sandbox, WAIT, git, text};
use contract::{Cmd, DiffScope, ErrorCode};
use serde_json::{Value, json};
use wire::Client;

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A `gh` that answers `pr list`, `pr view` and `api` from files in its folder and fails otherwise.
fn fake_gh(dir: &Path) -> PathBuf {
    let script = dir.join("gh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> {d}/gh.args\ncase \"$1 $2\" in\n  'pr list') cat {d}/list ;;\n  'pr view') cat {d}/view ;;\n  api*) cat {d}/inline 2>/dev/null || echo '[]' ;;\n  *) echo 'gh: unknown' >&2; exit 1 ;;\nesac\n",
            d = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The engine that every conversation test runs: it answers from a script, as `claude` does.
const ENGINE: &str = include_str!("fake-claude.py");

struct Rig {
    sb: Sandbox,
    gh_dir: PathBuf,
    work: PathBuf,
    head: String,
    _daemon: daemon::Daemon,
    client: Client,
    ws: String,
}

fn rig() -> Rig {
    let sb = Sandbox::new();
    let gh_dir = sb.dir.join("ghbin");
    std::fs::create_dir_all(&gh_dir).unwrap();
    // the remote, and a clone that is the project
    let remote = sb.dir.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    let work = sb.dir.join("work/app");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("a.txt"), "uno\ndos\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "init"]);
    git(&work, &["remote", "add", "origin", &text(&remote)]);
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["fetch", "-q", "origin"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    // the pull request: a branch with one commit, which the remote offers as pull/7/head
    git(&work, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(work.join("b.txt"), "nuevo\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "feature"]);
    let head = git_out(&work, &["rev-parse", "HEAD"]);
    git(
        &remote,
        &["fetch", "-q", &text(&work), "feat/x:refs/heads/feat/x"],
    );
    git(&remote, &["update-ref", "refs/pull/7/head", &head]);
    git(&work, &["checkout", "-q", "main"]);
    let work = std::fs::canonicalize(work).unwrap();

    std::fs::write(
        gh_dir.join("list"),
        r#"[{"number":7,"title":"Add b","author":{"login":"maria"},"headRefName":"feat/x","baseRefName":"main","isDraft":false,"updatedAt":"2026-10-19T10:00:00Z","reviewDecision":"REVIEW_REQUIRED","additions":1,"deletions":0,"url":"https://x/pull/7","body":"Adds b.","files":[{"path":"b.txt","additions":1,"deletions":0}]}]"#,
    )
    .unwrap();
    std::fs::write(
        gh_dir.join("view"),
        r#"{"title":"Add b","body":"Adds b.","author":{"login":"maria"},"baseRefName":"main","headRefName":"feat/x","url":"https://x/pull/7","comments":[{"author":{"login":"luis"},"createdAt":"t","body":"why?"}],"reviews":[],"files":[{"path":"b.txt","additions":1,"deletions":0}]}"#,
    )
    .unwrap();
    let mut config = sb.config();
    config.gh = Some(fake_gh(&gh_dir));
    let bin = sb.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("claude"), ENGINE).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    config.env = vec![
        (
            "PATH".into(),
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        ),
        ("FAKE_LOG".into(), text(&sb.dir.join("engine.log"))),
    ];
    let daemon = daemon::Daemon::start(config.clone()).unwrap();
    let mut client = Client::connect(&config.socket, "test").unwrap();
    let ws = client
        .cmd(Cmd::OpenFolder { path: text(&work) })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = client.root.clone();
    client.open(&root).unwrap();
    assert!(
        client
            .wait_until(WAIT, |c| c.nodes.contains_key(&root))
            .unwrap()
    );
    Rig {
        sb,
        gh_dir,
        work,
        head,
        _daemon: daemon,
        client,
        ws,
    }
}

impl Rig {
    fn cmd(&mut self, cmd: Cmd) -> Result<Option<Value>, contract::ErrorBody> {
        self.client.cmd(cmd).unwrap()
    }

    /// The summary of a workspace in the session node, once `f` holds of it.
    fn summary(&mut self, id: &str, f: impl Fn(&Value) -> bool) -> Value {
        let root = self.client.root.clone();
        let find = |c: &Client| {
            c.nodes[&root]["workspaces"]
                .as_array()
                .and_then(|a| a.iter().find(|w| w["id"] == id).cloned())
        };
        assert!(
            self.client
                .wait_until(WAIT, |c| find(c).is_some_and(|w| f(&w)))
                .unwrap(),
            "no such summary: {:?}",
            find(&self.client)
        );
        find(&self.client).unwrap()
    }
}

#[test]
fn the_open_pull_requests_are_listed_from_gh_and_a_failure_says_why() {
    let mut r = rig();
    let list = r
        .cmd(Cmd::ListPrs {
            workspace: r.ws.clone(),
            requested: false,
        })
        .unwrap()
        .unwrap();
    assert_eq!(list["prs"][0]["number"], 7);
    assert_eq!(list["prs"][0]["author"], "maria");
    assert_eq!(list["prs"][0]["decision"], "REVIEW_REQUIRED");
    assert_eq!(list["prs"][0]["files"][0]["path"], "b.txt");
    let args = std::fs::read_to_string(r.gh_dir.join("gh.args")).unwrap();
    assert!(args.contains("pr list --state open"), "{args}");
    assert!(!args.contains("review-requested"), "{args}");
    r.cmd(Cmd::ListPrs {
        workspace: r.ws.clone(),
        requested: true,
    })
    .unwrap();
    let args = std::fs::read_to_string(r.gh_dir.join("gh.args")).unwrap();
    assert!(args.contains("--search review-requested:@me"), "{args}");
    // gh answers nothing useful
    std::fs::remove_file(r.gh_dir.join("list")).unwrap();
    let e = r
        .cmd(Cmd::ListPrs {
            workspace: r.ws.clone(),
            requested: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
}

#[test]
fn a_pull_request_is_a_worktree_at_its_head_that_compares_against_its_base() {
    let mut r = rig();
    let reply = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        (reply["existed"].as_bool(), reply["dirty"].as_bool()),
        (Some(false), Some(false))
    );
    let id = reply["workspace"].as_str().unwrap().to_owned();
    // the worktree is beside the project, on its own branch, at the head of the pull request
    let dest = r.sb.dir.join("work/app-wt/review-7");
    let dest = std::fs::canonicalize(&dest).unwrap_or(dest);
    assert_eq!(git_out(&dest, &["rev-parse", "HEAD"]), r.head);
    assert_eq!(
        git_out(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "review/pr-7"
    );
    // it is a workspace that says it is a review, with the one panel
    let w = r.summary(&id, |w| w["pane_count"] == 1);
    assert_eq!(w["review"], json!({"pr": 7, "base": "main"}));
    // its diff is a view at the right of the agent's panel (REQ-716)
    r.client.open(&id).unwrap();
    assert!(
        r.client
            .wait_until(WAIT, |c| c
                .nodes
                .get(&id)
                .is_some_and(|n| n["views"][0]["kind"] == "diff"))
            .unwrap()
    );
    let agent = r.client.nodes[&id]["panes"][0]["id"].clone();
    let diff = r.client.nodes[&id]["views"][0]["id"].clone();
    let layout = r.client.nodes[&id]["layout"].clone();
    assert_eq!(layout["t"], "split");
    assert_eq!(layout["dir"], "row");
    assert_eq!(layout["a"]["pane"], agent);
    assert_eq!(layout["b"]["pane"], diff);
    // the file that briefs it says what the pull request says
    let file = r.sb.dir.join("state/reviews/app-pr-7.md");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("# Pull request #7: Add b") && text.contains("**luis**"),
        "{text}"
    );
    // its changes are the pull request's, against origin/main
    let node = r
        .cmd(Cmd::OpenDiff {
            workspace: id.clone(),
            scope: DiffScope::General,
            commit: None,
        })
        .unwrap()
        .unwrap()["node"]
        .as_str()
        .unwrap()
        .to_owned();
    r.client.open(&node).unwrap();
    assert!(
        r.client
            .wait_until(WAIT, |c| c.nodes.contains_key(&node))
            .unwrap()
    );
    let d = &r.client.nodes[&node];
    assert_eq!(d["base"], "origin/main");
    assert_eq!(d["files"].as_array().unwrap().len(), 1, "{d}");
    assert_eq!(d["files"][0]["path"], "b.txt");
    // a review that was made is no unpushed work
    let n = r
        .cmd(Cmd::CheckWorkspace {
            workspace: id.clone(),
        })
        .unwrap()
        .unwrap();
    assert_eq!(n["ahead"], 0);
}

#[test]
fn a_review_that_is_there_is_brought_to_the_new_head_unless_it_has_changes() {
    let mut r = rig();
    let id = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let dest = r.sb.dir.join("work/app-wt/review-7");
    let dest = std::fs::canonicalize(&dest).unwrap_or(dest);
    // the pull request gets a new commit
    git(&r.work, &["checkout", "-q", "feat/x"]);
    std::fs::write(r.work.join("c.txt"), "otro\n").unwrap();
    git(&r.work, &["add", "."]);
    git(&r.work, &["commit", "-q", "-m", "more"]);
    let newer = git_out(&r.work, &["rev-parse", "HEAD"]);
    git(&r.work, &["checkout", "-q", "main"]);
    let remote = r.sb.dir.join("remote.git");
    git(
        &remote,
        &[
            "fetch",
            "-q",
            &text(&r.work),
            "feat/x:refs/heads/feat/x",
            "--force",
        ],
    );
    git(&remote, &["update-ref", "refs/pull/7/head", &newer]);
    // a change of the person's is left as it is
    std::fs::write(dest.join("notas.txt"), "mias\n").unwrap();
    std::fs::write(dest.join("b.txt"), "editado\n").unwrap();
    let again = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap();
    assert_eq!(again["workspace"], id.as_str());
    assert_eq!(
        (again["existed"].as_bool(), again["dirty"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("b.txt")).unwrap(),
        "editado\n"
    );
    // with nothing changed it moves to the new head
    git(&dest, &["checkout", "--", "b.txt"]);
    std::fs::remove_file(dest.join("notas.txt")).unwrap();
    let again = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap();
    assert_eq!(again["dirty"], false);
    assert_eq!(git_out(&dest, &["rev-parse", "HEAD"]), newer);
    // a commit of the person's is work too: the review is not moved over it
    std::fs::write(dest.join("mio.txt"), "mi nota\n").unwrap();
    git(&dest, &["add", "."]);
    git(&dest, &["commit", "-q", "-m", "mine"]);
    let mine = git_out(&dest, &["rev-parse", "HEAD"]);
    let again = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap();
    assert_eq!(again["dirty"], true);
    assert_eq!(git_out(&dest, &["rev-parse", "HEAD"]), mine);
}

/// REQ-604: a review that waits for GitHub (a `gh` that sleeps) is stopped by `cancel` from another
/// connection: it answers `cancelado`, makes nothing, and the review can be made after.
#[test]
fn a_review_that_waits_for_github_is_stopped_by_cancel_and_makes_nothing() {
    let mut r = rig();
    let slow = r.gh_dir.join("gh");
    let fast = std::fs::read_to_string(&slow).unwrap();
    std::fs::write(
        &slow,
        fast.replace("'pr view') cat", "'pr view') sleep 30; cat"),
    )
    .unwrap();
    let socket = r.sb.config().socket;
    let (ws, began) = (r.ws.clone(), std::time::Instant::now());
    let asking = std::thread::spawn({
        let ws = ws.clone();
        move || {
            let mut c = Client::connect(&socket, "test-review").unwrap();
            c.cmd(Cmd::ReviewPr {
                workspace: ws,
                pr: 7,
            })
            .unwrap()
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(1200));
    r.cmd(Cmd::Cancel {
        workspace: ws.clone(),
    })
    .unwrap();
    let e = asking.join().unwrap().unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed);
    assert_eq!(e.message, "cancelado");
    assert!(
        began.elapsed() < std::time::Duration::from_secs(20),
        "gh was not waited for"
    );
    assert!(
        !r.work.parent().unwrap().join("app-wt/review-7").exists(),
        "nothing was made"
    );
    // GitHub answers now: the review is made, as if nothing had happened
    std::fs::write(&slow, fast).unwrap();
    let made = r
        .cmd(Cmd::ReviewPr {
            workspace: ws,
            pr: 7,
        })
        .unwrap()
        .unwrap();
    assert_eq!(made["existed"], false, "{made}");
}

#[test]
fn a_fetch_that_fails_makes_nothing_and_gh_that_is_missing_says_so() {
    let mut r = rig();
    // pull request 8 has no head in the remote
    let e = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 8,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::GitFailed, "{e:?}");
    assert!(!r.sb.dir.join("work/app-wt/review-8").exists());
    let root = r.client.root.clone();
    let count = r.client.nodes[&root]["workspaces"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(count, 1, "no workspace was made");
    let e = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);

    // a daemon with no gh
    let sb = Sandbox::new();
    let d = sb.start();
    let _keep = d;
    let mut c = sb.client();
    let dir = sb.dir.join("p");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    let ws = c
        .cmd(Cmd::OpenFolder {
            path: text(&std::fs::canonicalize(&dir).unwrap()),
        })
        .unwrap()
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = c
        .cmd(Cmd::ListPrs {
            workspace: ws,
            requested: false,
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
}

#[test]
fn the_agent_of_a_review_is_in_manual_waits_with_a_suggestion_and_leaves_the_last_mode_as_it_was() {
    let mut r = rig();
    let id = r
        .cmd(Cmd::ReviewPr {
            workspace: r.ws.clone(),
            pr: 7,
        })
        .unwrap()
        .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    // the panel the daemon started runs `claude` in a shell; the binary of the shell opens the conversation
    let pane = {
        r.summary(&id, |w| w["pane_count"] == 1);
        r.client.open(&id).unwrap();
        assert!(
            r.client
                .wait_until(WAIT, |c| c.nodes.contains_key(&id))
                .unwrap()
        );
        r.client.nodes[&id]["panes"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let reply = r
        .cmd(Cmd::OpenConversation {
            pane,
            resume: None,
            args: vec!["--permission-mode".into(), "manual".into()],
            env: None,
        })
        .unwrap()
        .unwrap();
    let node = reply["node"].as_str().unwrap().to_owned();
    r.client.open(&node).unwrap();
    assert!(
        r.client
            .wait_until(WAIT, |c| c
                .nodes
                .get(&node)
                .is_some_and(|n| n["suggestion"].is_string()))
            .unwrap(),
        "{:?}",
        r.client.nodes.get(&node)
    );
    let n = &r.client.nodes[&node];
    assert_eq!(n["mode"], "manual");
    let suggestion = n["suggestion"].as_str().unwrap();
    assert!(
        suggestion.contains("reviews/app-pr-7.md") && suggestion.contains("PR #7"),
        "{suggestion}"
    );
    // nothing was sent to the engine for it
    let log = std::fs::read_to_string(r.sb.dir.join("engine.log")).unwrap_or_default();
    assert!(!log.lines().any(|l| l.starts_with("user")), "{log}");
    // the mode of this conversation is not what the next one starts in
    r.cmd(Cmd::ConversationSet {
        node: node.clone(),
        name: None,
        accent: None,
        model: None,
        effort: None,
        mode: Some(contract::conversation::Mode::AcceptEdits),
    })
    .unwrap();
    assert!(
        r.client
            .wait_until(WAIT, |c| c.nodes[&node]["mode"] == "acceptEdits")
            .unwrap()
    );
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(
        !r.sb.dir.join("state/last-mode").exists(),
        "a review does not set the mode"
    );
}
