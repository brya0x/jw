//! `skill install --plugins`: the few plugins jw recommends, which are code of third parties, offered
//! with what they are and where they come from and installed only after a yes (N184, REQ-1003 to
//! REQ-1006). It runs `claude plugin` and never edits claude's settings itself (REQ-1007).

use std::io::{BufRead, IsTerminal, Write};
use std::process::Command;

use serde_json::Value;

use super::{Fail, Out, fail};

/// What jw recommends, and the only thing this module installs.
struct Plugin {
    name: &'static str,
    marketplace: &'static str,
    /// `<owner>/<repo>` on GitHub.
    source: &'static str,
}

const PLUGINS: [Plugin; 1] = [Plugin {
    name: "ponytail",
    marketplace: "ponytail",
    source: "DietrichGebert/ponytail",
}];

/// The program of the plugins; found in the `PATH`.
const CLAUDE: &str = "claude";

impl Plugin {
    fn id(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }
}

/// A plugin that is not there yet, and whether its marketplace is not either.
struct Pending<'a> {
    plugin: &'a Plugin,
    add_marketplace: bool,
}

impl Pending<'_> {
    /// The arguments of `claude` that install it, in order.
    fn commands(&self) -> Vec<Vec<String>> {
        let mut commands = Vec::new();
        if self.add_marketplace {
            commands.push(
                ["plugin", "marketplace", "add", self.plugin.source]
                    .map(String::from)
                    .to_vec(),
            );
        }
        commands.push(vec!["plugin".into(), "install".into(), self.plugin.id()]);
        commands
    }
}

/// The `key` of each object of a JSON array (`id` of `plugin list`, `name` of `marketplace list`).
fn keys(json: &str, key: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let items = value.as_array().ok_or("not a JSON array")?;
    Ok(items
        .iter()
        .filter_map(|i| i.get(key)?.as_str().map(str::to_owned))
        .collect())
}

fn pending<'a>(
    plugins: &'a [Plugin],
    installed: &[String],
    marketplaces: &[String],
) -> Vec<Pending<'a>> {
    plugins
        .iter()
        .filter(|p| !installed.contains(&p.id()))
        .map(|plugin| Pending {
            plugin,
            add_marketplace: !marketplaces.iter().any(|m| m == plugin.marketplace),
        })
        .collect()
}

/// Only `y` or `yes` goes on.
fn is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// What `claude` prints for `args`, or why it could not.
fn capture(args: &[&str]) -> Result<String, String> {
    let shown = format!("{CLAUDE} {}", args.join(" "));
    let out = Command::new(CLAUDE).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{CLAUDE} is not in the PATH: install it first")
        } else {
            format!("{shown}: {e}")
        }
    })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{shown} failed ({})", out.status))
    }
}

fn look(args: &[&str], key: &str) -> Out<Vec<String>> {
    let text = capture(args).map_err(Fail::Error)?;
    keys(&text, key).map_err(|e| Fail::Error(format!("{CLAUDE} {}: {e}", args.join(" "))))
}

/// Say what would be installed, ask, and run `claude plugin` for each (REQ-1004 to REQ-1006).
pub(super) fn offer(yes: bool) -> Out {
    let installed = look(&["plugin", "list", "--json"], "id")?;
    let marketplaces = look(&["plugin", "marketplace", "list", "--json"], "name")?;
    for p in PLUGINS.iter().filter(|p| installed.contains(&p.id())) {
        println!("plugins: {} is installed", p.id());
    }
    let todo = pending(&PLUGINS, &installed, &marketplaces);
    if todo.is_empty() {
        return Ok(());
    }
    for p in &todo {
        println!(
            "plugin {} from github {} (marketplace {})",
            p.plugin.id(),
            p.plugin.source,
            p.plugin.marketplace
        );
        for c in p.commands() {
            println!("  would run: {CLAUDE} {}", c.join(" "));
        }
    }
    println!(
        "this is code of a third party; it can run programs on your machine through its hooks"
    );
    if !yes {
        if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
            return fail(
                "not installed: no terminal to ask on; run this in a terminal or pass --yes",
            );
        }
        eprint!("install? [y/N] ");
        let mut line = String::new();
        let _ = std::io::stderr().flush();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Fail::Error(e.to_string()))?;
        if !is_yes(&line) {
            println!("not installed: you did not say yes");
            return Ok(());
        }
    }
    for p in &todo {
        for c in p.commands() {
            println!("$ {CLAUDE} {}", c.join(" "));
            let status = Command::new(CLAUDE)
                .args(&c)
                .status()
                .map_err(|e| Fail::Error(format!("{CLAUDE} {}: {e}", c.join(" "))))?;
            if !status.success() {
                return fail(format!("{CLAUDE} {} failed ({status})", c.join(" ")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `claude plugin list --json` and `claude plugin marketplace list --json` print.
    const LIST: &str = r#"[
      {"id": "rust-analyzer-lsp@claude-plugins-official", "version": "1.0.0", "scope": "user", "enabled": true},
      {"id": "vercel@claude-plugins-official", "version": "0.50.0", "scope": "user", "enabled": true,
       "mcpServers": {"vercel": {"type": "http", "url": "https://mcp.vercel.com"}}}
    ]"#;
    const MARKETS: &str = r#"[
      {"name": "claude-plugins-official", "source": "github", "repo": "anthropics/claude-plugins-official"}
    ]"#;

    fn todo(list: &str, markets: &str) -> Vec<(String, Vec<String>)> {
        let (i, m) = (keys(list, "id").unwrap(), keys(markets, "name").unwrap());
        pending(&PLUGINS, &i, &m)
            .iter()
            .map(|p| {
                let cmds = p.commands().iter().map(|c| c.join(" ")).collect();
                (p.plugin.id(), cmds)
            })
            .collect()
    }

    #[test]
    fn the_list_is_short_fixed_and_every_source_is_an_owner_and_a_repo() {
        assert!(!PLUGINS.is_empty() && PLUGINS.len() <= 3);
        for p in &PLUGINS {
            let (owner, repo) = p.source.split_once('/').expect("owner/repo");
            for part in [owner, repo, p.name, p.marketplace] {
                assert!(
                    !part.is_empty()
                        && part
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
                    "{part}"
                );
            }
            assert_eq!(p.id(), format!("{}@{}", p.name, p.marketplace));
        }
        assert_eq!(PLUGINS[0].id(), "ponytail@ponytail");
    }

    #[test]
    fn a_plugin_that_is_not_installed_comes_with_its_marketplace_when_that_is_missing_too() {
        assert_eq!(
            todo(LIST, MARKETS),
            [(
                "ponytail@ponytail".into(),
                vec![
                    "plugin marketplace add DietrichGebert/ponytail".into(),
                    "plugin install ponytail@ponytail".into()
                ]
            )]
        );
        let markets = r#"[{"name": "ponytail"}]"#;
        assert_eq!(
            todo(LIST, markets)[0].1,
            ["plugin install ponytail@ponytail"]
        );
    }

    #[test]
    fn what_is_installed_is_skipped() {
        let list = r#"[{"id": "ponytail@ponytail", "version": "5.3.2"}]"#;
        assert!(todo(list, MARKETS).is_empty());
        // Another marketplace's `ponytail` is not this one.
        let other = r#"[{"id": "ponytail@somebody-else"}]"#;
        assert_eq!(todo(other, MARKETS).len(), 1);
        assert!(todo("[]", "[]").len() == 1);
    }

    #[test]
    fn what_claude_prints_that_is_not_a_list_is_an_error_and_not_an_empty_list() {
        assert!(keys("", "id").is_err());
        assert!(keys("{}", "id").is_err());
        assert!(keys("warning: x\n[]", "id").is_err());
        assert_eq!(
            keys("[{\"id\": 3}, {}]", "id").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn only_yes_goes_on() {
        for yes in ["y", "Y\n", " yes ", "YES"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "n", "\n", "ye", "yep", "sure"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
