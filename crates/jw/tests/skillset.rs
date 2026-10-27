//! `skillset ls`, `on` and `off` (N184b), run as the built binary with private configuration, Claude
//! configuration, state and temporary directories, a `claude` that is a script in the `PATH`, and
//! GitHub that is a folder of git repositories named by `<PREFIX>_SKILLSET_GITHUB`. No network, no
//! `~/.config`, no `~/.claude`, no `~/.agents`, no daemon, and no plugin is ever installed.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::Env;
use serde_json::Value;

const CAVEMAN: &str = "juliusbrussee/caveman";
const SKILL: &str = "skills/caveman";

fn name() -> &'static str {
    &identity::get().name
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
    text(&out.stdout).trim().to_owned()
}

/// A `claude` that answers the two lists from files, records every call and installs nothing.
const CLAUDE: &str = "#!/bin/sh
echo \"$@\" >> \"$FAKE_LOG\"
case \"$*\" in
  \"plugin list --json\") cat \"$FAKE_LIST\";;
  \"plugin marketplace list --json\") echo '[]';;
esac
exit 0
";

struct World {
    env: Env,
}

impl World {
    /// `label` keeps the tests, which run in one process, out of each other's directory. The repository
    /// of `caveman` has a skill with a Markdown file in a folder and a script.
    fn new(label: &str) -> World {
        let w = World {
            env: Env::new(&format!("skillset-{label}")),
        };
        let bin = w.dir("bin/claude");
        write(&bin, CLAUDE);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        write(&w.dir("list.json"), "[]");
        std::fs::create_dir_all(w.dir("tmp")).unwrap();
        let repo = w.repo(CAVEMAN);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        w.commit(
            CAVEMAN,
            &[
                (
                    &format!("{SKILL}/SKILL.md"),
                    "---\nname: caveman\n---\nfew words\n",
                ),
                (&format!("{SKILL}/refs/a.md"), "a\n"),
                (&format!("{SKILL}/run.sh"), "echo hi\n"),
                ("README.md", "not part of the skill\n"),
            ],
            &[],
        );
        w
    }

    fn dir(&self, rel: &str) -> PathBuf {
        self.env.dir.join(rel)
    }

    fn repo(&self, repo: &str) -> PathBuf {
        self.dir("gh").join(repo)
    }

    /// A commit with these files written and these removed; its hash.
    fn commit(&self, repo: &str, files: &[(&str, &str)], remove: &[&str]) -> String {
        let dir = self.repo(repo);
        for (path, body) in files {
            write(&dir.join(path), body);
        }
        for path in remove {
            std::fs::remove_file(dir.join(path)).unwrap();
        }
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "c"]);
        git(&dir, &["rev-parse", "HEAD"])
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = self.env.jw();
        c.args(args)
            .env("XDG_CONFIG_HOME", self.dir("config"))
            .env("HOME", self.dir("home"))
            .env("TMPDIR", self.dir("tmp"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir("bin").display()),
            )
            .env("FAKE_LOG", self.dir("log"))
            .env("FAKE_LIST", self.dir("list.json"))
            .env(
                identity::env_var("SKILLSET_GITHUB"),
                format!("file://{}", self.dir("gh").display()),
            );
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        text(&out.stdout)
    }

    fn claude(&self) -> PathBuf {
        self.dir("claude-config")
    }

    fn skill(&self, rel: &str) -> PathBuf {
        self.claude().join("skills").join(rel)
    }

    fn lock(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir("state/skillset.lock")).unwrap())
            .unwrap()
    }

    fn entries(&self, extra: &[&str]) -> Vec<Value> {
        let mut args = vec!["skillset", "ls", "--json"];
        args.extend(extra);
        serde_json::from_str::<Value>(&self.ok(&args))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn state(&self, entry: &str) -> String {
        let all = self.entries(&[]);
        let e = all.iter().find(|e| e["name"] == entry).unwrap();
        e["state"].as_str().unwrap().to_owned()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The temporary directories of the commands that are still there.
    fn left(&self) -> usize {
        std::fs::read_dir(self.dir("tmp")).unwrap().count()
    }

    fn persons_file(&self, body: &str) {
        write(&self.dir("config").join(name()).join("skillset.toml"), body);
    }
}

#[test]
fn ls_lists_the_catalog_with_states_by_name_as_text_and_json() {
    let w = World::new("ls");
    let out = w.ok(&["skillset", "ls"]);
    let rows: Vec<Vec<&str>> = out
        .lines()
        .map(|l| l.split_whitespace().take(3).collect())
        .collect();
    let tasks = format!("{}-tasks", name());
    let agent = identity::get().agent_skill.as_str();
    assert_eq!(
        rows,
        [
            vec!["NAME", "KIND", "STATE"],
            vec![agent, "skill", "available"],
            vec!["brain", "role", "built-in"],
            vec!["caveman", "skill", "available"],
            vec![name(), "skill", "available"],
            vec![tasks.as_str(), "skill", "available"],
            vec!["ponytail", "plugin", "available"],
            vec!["researcher", "role", "built-in"],
        ]
    );
    assert!(out.contains("github juliusbrussee/caveman"), "{out}");
    assert!(out.contains("plugin ponytail@ponytail"), "{out}");

    let all = w.entries(&[]);
    let caveman = all.iter().find(|e| e["name"] == "caveman").unwrap();
    assert_eq!(caveman["source"]["type"], "github");
    assert_eq!(caveman["source"]["repo"], CAVEMAN);
    assert_eq!(caveman["source"]["path"], format!("{SKILL}/SKILL.md"));
    assert_eq!(caveman["yours"], false);
    assert!(caveman["commit"].is_null() && caveman["latest"].is_null());
    // No network: the repositories could be gone and `ls` would still answer.
    std::fs::remove_dir_all(w.dir("gh")).unwrap();
    assert!(w.run(&["skillset", "ls"]).status.success());
    assert!(!w.dir("s.sock").exists(), "no daemon");
}

#[test]
fn ls_says_when_ponytail_is_installed_and_unknown_when_claude_does_not_answer() {
    let w = World::new("ls-plugin");
    write(
        &w.dir("list.json"),
        r#"[{"id": "ponytail@ponytail", "version": "5.3.2"}]"#,
    );
    assert_eq!(w.state("ponytail"), "installed");
    let mut c = w.command(&["skillset", "ls", "--json"]);
    c.env("PATH", w.dir("empty"));
    let all: Value = serde_json::from_slice(&c.output().unwrap().stdout).unwrap();
    let e = all
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ponytail")
        .unwrap()
        .clone();
    assert_eq!(e["state"], "unknown");
}

#[test]
fn the_persons_entries_are_marked_third_party_and_cannot_replace_the_catalogs() {
    let w = World::new("yours");
    w.persons_file(
        "[[entry]]\nname = \"mine\"\nkind = \"skill\"\ndescription = \"my skill\"\nsource = \"github\"\nrepo = \"me/skills\"\npath = \"mine/SKILL.md\"\n\n\
         [[entry]]\nname = \"caveman\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"evil/caveman\"\npath = \"x/SKILL.md\"\n\n\
         [[entry]]\nname = \"bad\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"--upload-pack=x/y\"\npath = \"x/SKILL.md\"\n"
    );
    let out = w.run(&["skillset", "ls"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("github me/skills (yours)"), "{stdout}");
    assert!(
        stdout.contains("their sources are text of third parties"),
        "{stdout}"
    );
    assert_eq!(stderr.matches("warning:").count(), 2, "{stderr}");
    assert!(
        stderr.contains("taken") && stderr.contains("not <owner>/<repo>"),
        "{stderr}"
    );
    let all = w.entries(&[]);
    let mine = all.iter().find(|e| e["name"] == "mine").unwrap();
    assert_eq!(mine["yours"], true);
    // `caveman` is still the reviewed one.
    let caveman = all.iter().find(|e| e["name"] == "caveman").unwrap();
    assert_eq!(caveman["source"]["repo"], CAVEMAN);
    assert!(all.iter().all(|e| e["name"] != "bad"));
}

#[test]
fn what_another_tool_installed_is_left_alone_a_link_and_a_folder_alike() {
    let w = World::new("other");
    // As the `skills` tool leaves caveman: a link, to a place that is not even there.
    std::fs::create_dir_all(w.skill("")).unwrap();
    let link = w.skill("caveman");
    std::os::unix::fs::symlink("../../.agents/skills/caveman", &link).unwrap();
    // And a skill of the person's own with the name of one of jw's.
    let agent = identity::get().agent_skill.as_str();
    write(&w.skill(&format!("{agent}/SKILL.md")), "my own skill\n");
    std::os::unix::fs::symlink(w.dir("elsewhere"), w.skill(&format!("{}-tasks", name()))).unwrap();

    assert_eq!(w.state("caveman"), "other tool");
    assert_eq!(w.state(&format!("{}-tasks", name())), "other tool");
    assert_eq!(w.state(agent), "modified");

    let out = w.run(&["skillset", "on", "caveman", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("caveman: installed by another tool, left alone"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("../../.agents/skills/caveman")
    );
    assert!(!w.dir("state/skillset.lock").exists());
    // Nor does `off` touch it.
    let out = w.run(&["skillset", "off", "caveman"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("not installed by"),
        "{}",
        text(&out.stderr)
    );
    assert!(std::fs::symlink_metadata(&link).is_ok());

    // `skill install` writes the skill that is free, keeps the edited one with a `.new`, and says why
    // it did not write into the link, without stopping.
    let out = w.run(&["skill", "install"]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.contains(&format!(
            "installed {}",
            w.skill(&format!("{}/SKILL.md", name())).display()
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "kept {}",
            w.skill(&format!("{agent}/SKILL.md")).display()
        )),
        "{stdout}"
    );
    assert!(w.skill(&format!("{agent}/SKILL.md.new")).exists());
    assert!(
        !w.dir("elsewhere").exists(),
        "nothing was written through the link"
    );
}

#[test]
fn on_a_github_entry_shows_what_it_writes_asks_and_installs_it_with_a_lock() {
    let w = World::new("on");
    let commit = git(&w.repo(CAVEMAN), &["rev-parse", "HEAD"]);

    // No terminal and no `--yes`: it says all of it and writes nothing.
    let out = w.run(&["skillset", "on", "caveman"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    let url = format!("file://{}/{CAVEMAN}", w.dir("gh").display());
    assert!(
        stdout.contains(&format!("from {url}, branch HEAD, commit {commit}\n")),
        "{stdout}"
    );
    assert!(stdout.contains("will write 3 file(s):"), "{stdout}");
    for f in ["SKILL.md", "refs/a.md", "run.sh"] {
        assert!(
            stdout.contains(&format!("{}/{f} (", w.skill("caveman").display())),
            "{f}: {stdout}"
        );
    }
    assert!(
        !stdout.contains("README.md"),
        "only the skill's folder: {stdout}"
    );
    assert!(
        stdout.contains("files that are not Markdown (run.sh): a skill can carry scripts"),
        "{stdout}"
    );
    assert!(
        stderr.contains("not installed: no terminal to ask on"),
        "{stderr}"
    );
    assert!(!w.skill("caveman").exists() && !w.dir("state/skillset.lock").exists());
    assert_eq!(w.left(), 0, "the clone is deleted");

    // With `--yes`.
    let out = w.ok(&["skillset", "on", "caveman", "--yes"]);
    assert!(
        out.contains(&format!(
            "installed caveman (commit {}): 3 file(s) in",
            &commit[..7]
        )),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(w.skill("caveman/SKILL.md")).unwrap(),
        "---\nname: caveman\n---\nfew words\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.skill("caveman/refs/a.md")).unwrap(),
        "a\n"
    );
    assert!(w.skill("caveman/run.sh").exists());
    assert!(!w.skill("caveman/README.md").exists());
    assert_eq!(w.left(), 0);

    let lock = w.lock();
    let entry = &lock["entries"]["caveman"];
    assert_eq!(lock["v"], 1);
    assert_eq!(entry["commit"], commit.as_str());
    assert_eq!(entry["kind"], "skill");
    assert_eq!(entry["dir"], w.skill("caveman").to_string_lossy().as_ref());
    assert_eq!(entry["files"].as_object().unwrap().len(), 3);
    let key = w.skill("caveman/refs/a.md").to_string_lossy().into_owned();
    // The SHA-256 of "a\n".
    assert_eq!(
        entry["files"][&key],
        "87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7"
    );
    assert!(entry["installed_at"].as_u64().unwrap() > 0);

    // Again: nothing changed, so it says so and asks nothing.
    assert!(
        w.ok(&["skillset", "on", "caveman"])
            .contains(&format!("unchanged caveman (commit {})", &commit[..7]))
    );
    assert_eq!(w.state("caveman"), "installed");
    let all = w.entries(&[]);
    assert_eq!(
        all.iter().find(|e| e["name"] == "caveman").unwrap()["commit"],
        commit.as_str()
    );
}

#[test]
fn check_asks_the_remote_and_a_new_commit_makes_it_outdated_until_on_takes_it() {
    let w = World::new("check");
    w.ok(&["skillset", "on", "caveman", "--yes"]);
    // A newer commit: the skill's file changes and the script is gone.
    let newer = w.commit(
        CAVEMAN,
        &[(
            &format!("{SKILL}/SKILL.md"),
            "---\nname: caveman\n---\nfewer\n",
        )],
        &[&format!("{SKILL}/run.sh")],
    );
    // Without `--check` nothing asks the network.
    assert_eq!(w.state("caveman"), "installed");
    let all = w.entries(&["--check"]);
    let caveman = all.iter().find(|e| e["name"] == "caveman").unwrap();
    assert_eq!(caveman["state"], "outdated");
    assert_eq!(caveman["latest"], newer.as_str());
    assert!(w.ok(&["skillset", "ls", "--check"]).contains("outdated"));

    let out = w.ok(&["skillset", "on", "caveman", "--yes"]);
    assert!(out.contains(&format!("commit {newer}")), "{out}");
    assert_eq!(
        std::fs::read_to_string(w.skill("caveman/SKILL.md")).unwrap(),
        "---\nname: caveman\n---\nfewer\n"
    );
    assert!(
        !w.skill("caveman/run.sh").exists(),
        "what the new commit no longer has is taken away"
    );
    assert_eq!(w.lock()["entries"]["caveman"]["commit"], newer.as_str());
    assert_eq!(
        w.lock()["entries"]["caveman"]["files"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        w.entries(&["--check"])
            .iter()
            .find(|e| e["name"] == "caveman")
            .unwrap()["state"],
        "installed"
    );

    // A remote that does not answer is a warning and not a change of state.
    std::fs::remove_dir_all(w.dir("gh")).unwrap();
    let out = w.run(&["skillset", "ls", "--check"]);
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains("warning: caveman:"),
        "{}",
        text(&out.stderr)
    );
    assert!(text(&out.stdout).contains("installed"));
}

#[test]
fn off_removes_what_was_written_and_keeps_a_file_that_was_edited() {
    let w = World::new("off");
    w.ok(&["skillset", "on", "caveman", "--yes"]);
    // The person's own file, never in the lock.
    write(&w.skill("caveman/mine.txt"), "mine");
    write(&w.skill("caveman/refs/a.md"), "edited by the person\n");
    assert_eq!(w.state("caveman"), "modified");
    let out = w.run(&["skillset", "on", "caveman", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("was edited, left alone"),
        "{}",
        text(&out.stderr)
    );

    let out = w.ok(&["skillset", "off", "caveman"]);
    assert!(
        out.contains(&format!(
            "kept {}: it was edited",
            w.skill("caveman/refs/a.md").display()
        )),
        "{out}"
    );
    assert!(out.contains("removed 2 file(s) of caveman"), "{out}");
    assert!(!w.skill("caveman/SKILL.md").exists() && !w.skill("caveman/run.sh").exists());
    assert_eq!(
        std::fs::read_to_string(w.skill("caveman/refs/a.md")).unwrap(),
        "edited by the person\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.skill("caveman/mine.txt")).unwrap(),
        "mine"
    );
    // The lock keeps what was left, so `off` can end the job.
    assert_eq!(
        w.lock()["entries"]["caveman"]["files"]
            .as_object()
            .unwrap()
            .len(),
        1
    );

    // The person puts the file back: `off` takes it and the folder that is empty, and the lock forgets it.
    write(&w.skill("caveman/refs/a.md"), "a\n");
    std::fs::remove_file(w.skill("caveman/mine.txt")).unwrap();
    let out = w.ok(&["skillset", "off", "caveman"]);
    assert!(out.contains("removed 1 file(s)"), "{out}");
    assert!(!w.skill("caveman").exists(), "the empty folders go");
    assert!(w.skill("").exists(), "but never the skills folder");
    assert!(w.lock()["entries"].as_object().unwrap().is_empty());
    assert!(
        w.ok(&["skillset", "off", "caveman"])
            .contains("caveman is not installed")
    );
}

#[test]
fn a_lock_that_names_files_outside_the_folder_makes_off_touch_nothing_outside() {
    let w = World::new("tamper");
    w.ok(&["skillset", "on", "caveman", "--yes"]);
    let precious = w.dir("precious.txt");
    write(&precious, "keep me");
    let mut lock = w.lock();
    let hash = "5b0afa3afea5fea3a02c0cb2cb3c2d95ae0d40ac1a1e16dcad8c5ba2af8c8e5f"; // not checked: the path decides
    let files = lock["entries"]["caveman"]["files"].as_object_mut().unwrap();
    files.insert(precious.to_string_lossy().into_owned(), hash.into());
    files.insert(
        format!("{}/../../precious.txt", w.skill("caveman").display()),
        hash.into(),
    );
    std::fs::write(w.dir("state/skillset.lock"), lock.to_string()).unwrap();
    let out = w.ok(&["skillset", "off", "caveman"]);
    assert_eq!(out.matches("left ").count(), 2, "{out}");
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
    assert!(!w.skill("caveman").exists());
}

#[test]
fn a_skill_that_holds_a_link_or_is_not_there_is_refused_and_nothing_is_written() {
    let w = World::new("refuse");
    let secret = w.dir("secret.txt");
    write(&secret, "top secret");
    std::os::unix::fs::symlink(&secret, w.repo(CAVEMAN).join(SKILL).join("leak.md")).unwrap();
    git(&w.repo(CAVEMAN), &["add", "-A"]);
    git(&w.repo(CAVEMAN), &["commit", "-q", "-m", "link"]);
    let out = w.run(&["skillset", "on", "caveman", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("leak.md: a link, refused"),
        "{}",
        text(&out.stderr)
    );
    assert!(!w.skill("caveman").exists() && !w.dir("state/skillset.lock").exists());
    assert_eq!(w.left(), 0);

    // An entry of the person's with a path the repository does not have.
    w.persons_file(&format!(
        "[[entry]]\nname = \"ghost\"\nkind = \"skill\"\nsource = \"github\"\nrepo = \"{CAVEMAN}\"\npath = \"nowhere/SKILL.md\"\n"
    ));
    let out = w.run(&["skillset", "on", "ghost", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("no nowhere/SKILL.md"),
        "{}",
        text(&out.stderr)
    );
    assert!(!w.skill("ghost").exists());
    // And a name that is not an entry lists the ones there are.
    let out = w.run(&["skillset", "on", "nothing"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("no entry \"nothing\": there are "),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_role_of_a_github_entry_goes_to_the_users_roles_and_never_under_claude() {
    let w = World::new("role");
    let repo = "me/roles";
    std::fs::create_dir_all(w.repo(repo)).unwrap();
    git(&w.repo(repo), &["init", "-q", "-b", "main"]);
    w.commit(
        repo,
        &[(
            "roles/voice.md",
            "---\ndescription: a voice\n---\nSpeak like {bin}.\n",
        )],
        &[],
    );
    w.persons_file(&format!(
        "[[entry]]\nname = \"voice\"\nkind = \"role\"\ndescription = \"a voice\"\nsource = \"github\"\nrepo = \"{repo}\"\npath = \"roles/voice.md\"\n"
    ));
    assert_eq!(w.state("voice"), "available");
    let out = w.ok(&["skillset", "on", "voice", "--yes"]);
    let file = w.dir("config").join(name()).join("roles/voice.md");
    assert!(out.contains(&format!("in {}", file.display())), "{out}");
    assert!(!out.contains("not Markdown"));
    assert!(
        file.exists() && !w.claude().exists(),
        "nothing under the Claude configuration"
    );
    assert_eq!(w.state("voice"), "installed");
    // `role show` finds it with no change to the commands of the roles.
    let shown = w.ok(&["role", "show", "voice"]);
    assert_eq!(shown, format!("Speak like {}.\n", name()));

    // A role the person edited is kept by `off`.
    write(&file, "my own\n");
    assert_eq!(w.state("voice"), "modified");
    assert!(w.ok(&["skillset", "off", "voice"]).contains("kept "));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "my own\n");
}

#[test]
fn the_embedded_ones_are_on_with_their_mark_and_off_only_when_nobody_edited_them() {
    let w = World::new("embedded");
    let me = name();
    let path = w.skill(&format!("{me}/SKILL.md"));
    let out = w.ok(&["skillset", "on", me]);
    assert_eq!(out, format!("installed {}\n", path.display()));
    assert_eq!(w.state(me), "installed");
    assert!(w.ok(&["skillset", "on", me]).starts_with("unchanged "));
    // A role that is built in: nothing to install, nothing to turn off.
    for verb in ["on", "off"] {
        let out = w.ok(&["skillset", verb, "brain"]);
        assert!(out.contains("brain is built in"), "{out}");
    }
    write(&path, "edited\n");
    assert_eq!(w.state(me), "modified");
    assert!(w.ok(&["skillset", "off", me]).contains("kept "));
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();
    w.ok(&["skillset", "on", me]);
    assert!(w.ok(&["skillset", "off", me]).contains("removed "));
    assert!(!path.exists() && !path.parent().unwrap().exists());
    assert!(w.ok(&["skillset", "off", me]).contains("is not installed"));
}

#[test]
fn a_plugin_is_installed_after_a_yes_with_the_commands_shown_and_off_only_says_how() {
    let w = World::new("plugin");
    let out = w.run(&["skillset", "on", "ponytail"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stdout).contains("would run: claude plugin install ponytail@ponytail"),
        "{}",
        text(&out.stdout)
    );
    assert!(
        text(&out.stderr).contains("no terminal"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        w.calls(),
        ["plugin list --json", "plugin marketplace list --json"]
    );

    let out = w.ok(&["skillset", "on", "ponytail", "--yes"]);
    assert!(
        out.contains("$ claude plugin marketplace add DietrichGebert/ponytail\n"),
        "{out}"
    );
    assert!(
        out.contains("$ claude plugin install ponytail@ponytail\n"),
        "{out}"
    );
    let calls = w.calls();
    assert_eq!(
        &calls[2..],
        [
            "plugin list --json",
            "plugin marketplace list --json",
            "plugin marketplace add DietrichGebert/ponytail",
            "plugin install ponytail@ponytail"
        ]
    );

    let before = w.calls().len();
    let out = w.ok(&["skillset", "off", "ponytail"]);
    assert!(
        out.contains("to remove it run: claude plugin uninstall ponytail@ponytail"),
        "{out}"
    );
    assert_eq!(w.calls().len(), before, "off runs nothing");
}

#[test]
fn bad_use_is_exit_2() {
    let w = World::new("usage");
    for args in [
        &["skillset"][..],
        &["skillset", "ls", "x"],
        &["skillset", "on"],
        &["skillset", "on", "a", "b"],
        &["skillset", "off", "--yes", "a"],
        &["skillset", "new"],
        &["skillset", "ls", "--nope"],
    ] {
        assert_eq!(w.run(args).status.code(), Some(2), "{args:?}");
    }
}
