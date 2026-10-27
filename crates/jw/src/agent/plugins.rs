//! A `plugin` entry of the skillset: third-party code with hooks, offered with what it is and where it
//! comes from and installed only after a yes (N184, REQ-1012 to REQ-1014; N184b, REQ-1021). It runs
//! `claude plugin` and never edits claude's settings itself (REQ-1015).

use std::process::Command;

use serde_json::Value;

use super::skillset::confirm;
use super::{Fail, Out, fail};

/// The program of the plugins; found in the `PATH`.
const CLAUDE: &str = "claude";

/// The arguments of `claude` that install a plugin of `repo`'s marketplace, in order.
fn commands(plugin: &str, repo: &str, marketplace_there: bool) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    if !marketplace_there {
        commands.push(
            ["plugin", "marketplace", "add", repo]
                .map(String::from)
                .to_vec(),
        );
    }
    commands.push(vec!["plugin".into(), "install".into(), plugin.to_owned()]);
    commands
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

/// The ids (`<plugin>@<marketplace>`) of the installed plugins.
pub(super) fn installed_ids() -> Out<Vec<String>> {
    look(&["plugin", "list", "--json"], "id")
}

/// Say what would be installed and from where, ask, and run `claude plugin` for it (REQ-1012 to
/// REQ-1014). A plugin that is installed is skipped.
pub(super) fn on(plugin: &str, repo: &str, yes: bool) -> Out {
    let installed = installed_ids()?;
    if installed.iter().any(|i| i == plugin) {
        println!("plugins: {plugin} is installed");
        return Ok(());
    }
    let marketplace = plugin.split_once('@').map_or("", |(_, m)| m);
    let marketplaces = look(&["plugin", "marketplace", "list", "--json"], "name")?;
    let todo = commands(plugin, repo, marketplaces.iter().any(|m| m == marketplace));
    println!("plugin {plugin} from github {repo} (marketplace {marketplace})");
    for c in &todo {
        println!("  would run: {CLAUDE} {}", c.join(" "));
    }
    println!(
        "this is code of a third party; it can run programs on your machine through its hooks"
    );
    if !confirm(yes)? {
        println!("not installed: you did not say yes");
        return Ok(());
    }
    for c in &todo {
        println!("$ {CLAUDE} {}", c.join(" "));
        let status = Command::new(CLAUDE)
            .args(c)
            .status()
            .map_err(|e| Fail::Error(format!("{CLAUDE} {}: {e}", c.join(" "))))?;
        if !status.success() {
            return fail(format!("{CLAUDE} {} failed ({status})", c.join(" ")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_comes_with_its_marketplace_when_that_is_missing() {
        let both: Vec<_> = commands("ponytail@ponytail", "DietrichGebert/ponytail", false)
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert_eq!(
            both,
            [
                "plugin marketplace add DietrichGebert/ponytail",
                "plugin install ponytail@ponytail"
            ]
        );
        let one = commands("ponytail@ponytail", "DietrichGebert/ponytail", true);
        assert_eq!(one, [["plugin", "install", "ponytail@ponytail"]]);
    }

    /// The shape `claude plugin list --json` and `claude plugin marketplace list --json` print.
    #[test]
    fn the_ids_and_the_marketplaces_are_read_from_what_claude_prints() {
        let list = r#"[
          {"id": "rust-analyzer-lsp@claude-plugins-official", "version": "1.0.0", "scope": "user", "enabled": true},
          {"id": "vercel@claude-plugins-official", "version": "0.50.0",
           "mcpServers": {"vercel": {"type": "http", "url": "https://mcp.vercel.com"}}}
        ]"#;
        assert_eq!(
            keys(list, "id").unwrap(),
            [
                "rust-analyzer-lsp@claude-plugins-official",
                "vercel@claude-plugins-official"
            ]
        );
        let markets =
            r#"[{"name": "claude-plugins-official", "source": "github", "repo": "anthropics/x"}]"#;
        assert_eq!(keys(markets, "name").unwrap(), ["claude-plugins-official"]);
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
}
