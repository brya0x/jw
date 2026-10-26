//! `role`, the skill of the roles and `skill install --plugins` (N184), run as the built binary with
//! private folders for the person's roles and the repository's, a private `CLAUDE_CONFIG_DIR` and, for
//! the plugins, a fake `claude` first in the `PATH` that records what it is called with. No daemon, no
//! `~/.config`, no `~/.claude`, and no plugin is ever installed.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::Env;
use serde_json::{Value, json};

fn name() -> &'static str {
    &identity::get().name
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `jw` with the person's roles in `<dir>/config/<name>/roles` and nothing of the real home.
fn jw(env: &Env, from: &Path) -> Command {
    let mut c = env.jw();
    c.env("XDG_CONFIG_HOME", env.dir.join("config"))
        .env("HOME", env.dir.join("home"))
        .current_dir(from);
    c
}

fn user_roles(env: &Env) -> PathBuf {
    env.dir.join("config").join(name()).join("roles")
}

/// A repository (a `.git` file, as in a worktree) with one role of its own and a folder below it.
fn repo(env: &Env) -> (PathBuf, PathBuf) {
    let repo = env.dir.join("repo");
    write(&repo.join(".git"), "gitdir: elsewhere");
    write(
        &identity::roles_dir(&repo).join("brain.md"),
        "---\ndescription: the repository's brain\n---\nplan with {bin} here\n",
    );
    let deep = repo.join("a/b");
    std::fs::create_dir_all(&deep).unwrap();
    (repo, deep)
}

fn run(env: &Env, from: &Path, args: &[&str]) -> Output {
    jw(env, from).args(args).output().unwrap()
}

#[test]
fn ls_and_show_resolve_the_layers_with_no_daemon_and_say_what_hides_what() {
    let env = Env::new("roles-layers");
    let (_, deep) = repo(&env);
    write(
        &user_roles(&env).join("brain.md"),
        "---\ndescription: my brain\n---\nmy plan\n",
    );
    write(
        &user_roles(&env).join("scribe.md"),
        "---\ndescription: writes with {bin}\n---\nWrite.\n",
    );

    let out = run(&env, &deep, &["role", "ls"]);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", text(&out.stderr));
    let rows: Vec<Vec<&str>> = stdout
        .lines()
        .take(4)
        .map(|l| l.split_whitespace().take(2).collect())
        .collect();
    assert_eq!(
        rows,
        [
            vec!["ROLE", "LAYER"],
            vec!["brain", "project"],
            vec!["researcher", "built-in"],
            vec!["scribe", "user"]
        ]
    );
    assert!(
        stdout.contains(&format!("writes with {}\n", name())),
        "{stdout}"
    );
    assert!(
        stdout.contains("note: brain of project hides the one of user, built-in\n"),
        "{stdout}"
    );
    assert!(
        stdout
            .ends_with("project roles are text of this repository: read one before you take it\n"),
        "{stdout}"
    );

    let out = run(&env, &deep, &["role", "ls", "--json"]);
    let roles: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        roles[0],
        json!({"name": "brain", "description": "the repository's brain", "layer": "project",
               "hides": ["user", "built-in"]})
    );
    assert_eq!(roles[1]["name"], "researcher");
    assert_eq!(roles[1]["hides"], json!([]));
    assert_eq!(roles.as_array().unwrap().len(), 3);

    // Outside the repository only the person's and the built-in ones are there.
    let out = run(&env, &env.dir, &["role", "ls", "--json"]);
    let roles: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(roles[0]["layer"], "user");
    assert!(!text(&out.stdout).contains("project"));

    // No daemon was needed and none was started.
    assert!(!env.dir.join("s.sock").exists());
}

#[test]
fn show_prints_the_body_with_the_binary_s_name_and_its_layer_on_standard_error() {
    let env = Env::new("roles-show");
    let (_, deep) = repo(&env);

    let out = run(&env, &deep, &["role", "show", "brain"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), format!("plan with {} here\n", name()));
    assert_eq!(
        text(&out.stderr),
        "role brain (project: text of this repository)\n"
    );

    // The built-in role of the user's seed: step 0 stays, the binary is named by the identity.
    let out = run(&env, &env.dir, &["role", "show", "researcher"]);
    let body = text(&out.stdout);
    assert!(out.status.success());
    assert_eq!(text(&out.stderr), "role researcher (built-in)\n");
    assert!(body.starts_with("# Researcher\n"), "{body}");
    assert!(body.contains("`ponytail:ponytail`") && !body.contains("{bin}"));
    let brain = text(&run(&env, &env.dir, &["role", "show", "brain"]).stdout);
    assert!(brain.contains(&format!("`{} ls`", name())), "{brain}");

    // A name that is not a role: exit 1 and the ones there are. A path is never followed.
    for bad in ["nope", "../repo/.git", "brain.md", "Brain"] {
        let out = run(&env, &env.dir, &["role", "show", bad]);
        assert_eq!(out.status.code(), Some(1), "{bad}");
        assert!(
            text(&out.stderr).contains(&format!("no role \"{bad}\": there are brain, researcher")),
            "{}",
            text(&out.stderr)
        );
        assert_eq!(text(&out.stdout), "");
    }
    for args in [
        &["role"][..],
        &["role", "show"],
        &["role", "ls", "x"],
        &["role", "new"],
    ] {
        assert_eq!(run(&env, &env.dir, args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn a_file_that_cannot_be_read_is_a_warning_and_the_layer_below_answers() {
    let env = Env::new("roles-bad");
    std::fs::create_dir_all(user_roles(&env)).unwrap();
    std::fs::write(user_roles(&env).join("brain.md"), [0xff, 0xfe]).unwrap();
    let out = run(&env, &env.dir, &["role", "show", "brain"]);
    assert!(out.status.success());
    assert!(text(&out.stderr).contains("warning:") && text(&out.stderr).contains("brain.md"));
    assert!(text(&out.stderr).ends_with("role brain (built-in)\n"));
}

fn agent_skill(config: &Path) -> PathBuf {
    config.join(format!("skills/{}/SKILL.md", identity::get().agent_skill))
}

#[test]
fn the_skill_of_the_roles_sends_the_agent_to_the_commands_and_no_file() {
    let env = Env::new("roles-skill");
    let config = env.dir.join("claude-config");
    let out = env.jw().args(["skill", "install"]).output().unwrap();
    assert!(out.status.success());
    let skill = std::fs::read_to_string(agent_skill(&config)).unwrap();
    let a = &identity::get().agent_skill;
    assert!(skill.starts_with(&format!("---\nname: {a}\n")), "{skill}");
    assert!(skill.contains("disable-model-invocation: true"));
    assert!(skill.contains(&format!("`{} role ls`", name())));
    assert!(skill.contains(&format!("`{} role show <name>`", name())));
    assert!(!skill.contains("{bin}") && !skill.contains("{agent_skill}"));
    assert!(!skill.contains("~/.claude"), "it reads no file");
}

#[test]
fn skill_install_keeps_a_skill_the_person_edited_and_refreshes_the_one_it_wrote() {
    let env = Env::new("roles-keep");
    let config = env.dir.join("claude-config");
    let path = agent_skill(&config);
    // What the user has today at `~/.claude/skills/agent`: no mark, a text of their own.
    write(&path, "my own /agent skill\n");
    let install = || env.jw().args(["skill", "install"]).output().unwrap();

    let out = install();
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", text(&out.stderr));
    let new = format!("{}.new", path.display());
    assert!(
        stdout.contains(&format!(
            "kept {}: not written by {} or edited; the new one is {new}\n",
            path.display(),
            name()
        )),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "my own /agent skill\n"
    );
    let fresh = std::fs::read_to_string(&new).unwrap();
    assert!(fresh.contains("disable-model-invocation: true") && fresh.ends_with(" -->\n"));
    // The other two had nothing there: written.
    assert_eq!(stdout.matches("installed ").count(), 2, "{stdout}");

    // Nothing changes on a second run, and the person's file is still theirs.
    let stdout = text(&install().stdout);
    assert_eq!(stdout.matches("unchanged ").count(), 2, "{stdout}");
    assert_eq!(stdout.matches("kept ").count(), 1, "{stdout}");

    // A file of an older version that nobody edited is refreshed: the `.new` carries a valid mark.
    let older = fresh.replace(env!("CARGO_PKG_VERSION"), "0.0.1");
    std::fs::write(&path, older).unwrap();
    let stdout = text(&install().stdout);
    assert!(
        stdout.contains(&format!("installed {}\n", path.display())),
        "{stdout}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), fresh);

    // An edit under the mark is an edit: kept.
    let tasks = config.join(format!("skills/{}-tasks/SKILL.md", name()));
    let edited = std::fs::read_to_string(&tasks)
        .unwrap()
        .replace("# ", "## ");
    std::fs::write(&tasks, &edited).unwrap();
    let stdout = text(&install().stdout);
    assert!(
        stdout.contains(&format!("kept {}", tasks.display())),
        "{stdout}"
    );
    assert_eq!(std::fs::read_to_string(&tasks).unwrap(), edited);
}

/// A `claude` that answers `plugin list --json` and `plugin marketplace list --json` from the files
/// the test points at, records every call, and fails an `install` when `FAKE_FAIL` is set.
const CLAUDE: &str = "#!/bin/sh
echo \"$@\" >> \"$FAKE_LOG\"
case \"$*\" in
  \"plugin list --json\") cat \"$FAKE_LIST\";;
  \"plugin marketplace list --json\") cat \"$FAKE_MARKETS\";;
  \"plugin install\"*) [ -n \"$FAKE_FAIL\" ] && exit 3;;
esac
exit 0
";

struct Fake {
    env: Env,
}

impl Fake {
    /// `label` keeps the tests, which run in one process, out of each other's directory.
    fn new(label: &str, list: &str) -> Fake {
        let env = Env::new(&format!("roles-plugins-{label}"));
        let bin = env.dir.join("bin/claude");
        write(&bin, CLAUDE);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        write(&env.dir.join("list.json"), list);
        write(&env.dir.join("markets.json"), "[]");
        Fake { env }
    }

    /// `skill install --plugins ...` with the fake first in the `PATH`; `path` replaces the rest of it.
    fn run(&self, args: &[&str], path: &str, fail: bool) -> Output {
        let d = &self.env.dir;
        let mut c = self.env.jw();
        c.args(["skill", "install", "--plugins"])
            .args(args)
            .env(
                "PATH",
                path.replace("{fake}", &d.join("bin").to_string_lossy()),
            )
            .env("FAKE_LOG", d.join("log"))
            .env("FAKE_LIST", d.join("list.json"))
            .env("FAKE_MARKETS", d.join("markets.json"));
        if fail {
            c.env("FAKE_FAIL", "1");
        }
        c.output().unwrap()
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.env.dir.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

const LOOKS: [&str; 2] = ["plugin list --json", "plugin marketplace list --json"];
const ADD: &str = "plugin marketplace add DietrichGebert/ponytail";
const INSTALL: &str = "plugin install ponytail@ponytail";

#[test]
fn plugins_without_a_terminal_and_without_yes_show_what_they_would_do_and_install_nothing() {
    let fake = Fake::new("a", "[]");
    let out = fake.run(&[], "{fake}:/usr/bin:/bin", false);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "plugin ponytail@ponytail from github DietrichGebert/ponytail (marketplace ponytail)\n"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("would run: claude {ADD}\n")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("would run: claude {INSTALL}\n")),
        "{stdout}"
    );
    assert!(stdout.contains("code of a third party"), "{stdout}");
    assert!(
        stderr.contains("not installed: no terminal to ask on"),
        "{stderr}"
    );
    assert_eq!(fake.log(), LOOKS);
}

#[test]
fn plugins_with_yes_run_the_two_commands_they_print_and_nothing_else() {
    let fake = Fake::new("b", r#"[{"id": "vercel@claude-plugins-official"}]"#);
    let out = fake.run(&["--yes"], "{fake}:/usr/bin:/bin", false);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", text(&out.stderr));
    assert!(stdout.contains(&format!("$ claude {ADD}\n")), "{stdout}");
    assert!(
        stdout.contains(&format!("$ claude {INSTALL}\n")),
        "{stdout}"
    );
    // `--yes` is jw's answer: claude is never told `-y`.
    assert_eq!(fake.log(), [LOOKS[0], LOOKS[1], ADD, INSTALL]);

    // The marketplace is there already: only the install.
    write(
        &fake.env.dir.join("markets.json"),
        r#"[{"name": "ponytail"}]"#,
    );
    std::fs::remove_file(fake.env.dir.join("log")).unwrap();
    assert!(
        fake.run(&["--yes"], "{fake}:/usr/bin:/bin", false)
            .status
            .success()
    );
    assert_eq!(fake.log(), [LOOKS[0], LOOKS[1], INSTALL]);
}

#[test]
fn what_is_installed_is_skipped_with_no_question_and_a_failing_command_stops_with_exit_1() {
    let fake = Fake::new("c", r#"[{"id": "ponytail@ponytail", "version": "5.3.2"}]"#);
    let out = fake.run(&[], "{fake}:/usr/bin:/bin", false);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("plugins: ponytail@ponytail is installed\n"));
    assert_eq!(fake.log(), LOOKS);

    let fake = Fake::new("d", "[]");
    let out = fake.run(&["--yes"], "{fake}:/usr/bin:/bin", true);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains(&format!("claude {INSTALL} failed")),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn plugins_with_no_claude_say_so_and_yes_alone_is_a_bad_use() {
    let fake = Fake::new("e", "[]");
    let empty = fake.env.dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = fake.run(&["--yes"], &empty.to_string_lossy(), false);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("claude is not in the PATH"),
        "{}",
        text(&out.stderr)
    );
    let out = fake
        .env
        .jw()
        .args(["skill", "install", "--yes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
