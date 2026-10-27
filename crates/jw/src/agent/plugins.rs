//! A `plugin` entry of the skillset: third-party code with hooks, offered with what it is and where it
//! comes from and installed only after a yes (N184, REQ-1004 to REQ-1006; N184b, REQ-1013). It runs
//! `claude plugin` and never edits claude's settings itself (REQ-1007).

use std::process::Command;

use serde_json::Value;

use super::skillset::confirm;
use super::{Fail, Out, fail};

/// The program of the plugins; found in the `PATH`.
pub(super) const CLAUDE: &str = "claude";

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

/// What `claude` (the program `claude`) prints for `args`, or why it could not.
fn capture(claude: &str, args: &[&str]) -> Result<String, String> {
    let shown = format!("{claude} {}", args.join(" "));
    let out = Command::new(claude).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{claude} is not in the PATH: install it first")
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

fn look(claude: &str, args: &[&str], key: &str) -> Out<Vec<String>> {
    let text = capture(claude, args).map_err(Fail::Error)?;
    keys(&text, key).map_err(|e| Fail::Error(format!("{claude} {}: {e}", args.join(" "))))
}

/// The ids (`<plugin>@<marketplace>`) of the installed plugins.
pub(super) fn installed_ids(claude: &str) -> Out<Vec<String>> {
    look(claude, &["plugin", "list", "--json"], "id")
}

/// What installing a plugin would do: its marketplace and the `claude` commands, in order.
pub(crate) struct Offer {
    pub plugin: String,
    pub marketplace: String,
    pub repo: String,
    pub todo: Vec<Vec<String>>,
}

impl Offer {
    /// The lines that say it (REQ-1004): the plugin, where from, what would run and the warning.
    pub(super) fn lines(&self, claude: &str) -> Vec<String> {
        let mut lines = vec![format!(
            "plugin {} from github {} (marketplace {})",
            self.plugin, self.repo, self.marketplace
        )];
        for c in &self.todo {
            lines.push(format!("  would run: {claude} {}", c.join(" ")));
        }
        lines.push(
            "this is code of a third party; it can run programs on your machine through its hooks"
                .into(),
        );
        lines
    }
}

/// What there is to ask: `None` when the plugin is installed already (REQ-1004).
pub(super) fn prepare(claude: &str, plugin: &str, repo: &str) -> Out<Option<Offer>> {
    if installed_ids(claude)?.iter().any(|i| i == plugin) {
        return Ok(None);
    }
    let marketplace = plugin.split_once('@').map_or("", |(_, m)| m);
    let marketplaces = look(claude, &["plugin", "marketplace", "list", "--json"], "name")?;
    Ok(Some(Offer {
        plugin: plugin.to_owned(),
        marketplace: marketplace.to_owned(),
        repo: repo.to_owned(),
        todo: commands(plugin, repo, marketplaces.iter().any(|m| m == marketplace)),
    }))
}

/// Run the commands of an offer after a yes (REQ-1005, REQ-1006). `quiet`: the screen's, with
/// stdin closed and the output of `claude` captured, the first line of its stderr in the error.
pub(super) fn run(claude: &str, offer: &Offer, quiet: bool, say: &mut dyn FnMut(String)) -> Out {
    for c in &offer.todo {
        say(format!("$ {claude} {}", c.join(" ")));
        let mut cmd = Command::new(claude);
        cmd.args(c);
        let (ok, shown, tail) = if quiet {
            let out = cmd
                .output()
                .map_err(|e| Fail::Error(format!("{claude} {}: {e}", c.join(" "))))?;
            let tail = String::from_utf8_lossy(&out.stderr)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| format!(": {}", l.trim()))
                .unwrap_or_default();
            (out.status.success(), out.status.to_string(), tail)
        } else {
            let status = cmd
                .status()
                .map_err(|e| Fail::Error(format!("{claude} {}: {e}", c.join(" "))))?;
            (status.success(), status.to_string(), String::new())
        };
        if !ok {
            return fail(format!("{claude} {} failed ({shown}){tail}", c.join(" ")));
        }
    }
    Ok(())
}

/// Say what would be installed and from where, ask, and run `claude plugin` for it (REQ-1004 to
/// REQ-1006). A plugin that is installed is skipped.
pub(super) fn on(plugin: &str, repo: &str, yes: bool) -> Out {
    let Some(offer) = prepare(CLAUDE, plugin, repo)? else {
        println!("plugins: {plugin} is installed");
        return Ok(());
    };
    for l in offer.lines(CLAUDE) {
        println!("{l}");
    }
    if !confirm(yes)? {
        println!("not installed: you did not say yes");
        return Ok(());
    }
    run(CLAUDE, &offer, false, &mut |l| println!("{l}"))
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
