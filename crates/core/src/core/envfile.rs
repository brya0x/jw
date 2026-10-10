//! Edits dotenv files line by line, keeping comments, order and every key it
//! was not asked to touch. Port of internal/core/envfile.

use std::collections::BTreeMap;

/// Overwrites the value of each key in `set`, keeping any `export` prefix,
/// and appends the keys that were missing (sorted, so output is stable).
pub fn set(data: &str, set: &BTreeMap<String, String>) -> String {
    if set.is_empty() {
        return data.to_string();
    }
    let mut lines: Vec<String> = if data.is_empty() {
        Vec::new()
    } else {
        data.strip_suffix('\n')
            .unwrap_or(data)
            .split('\n')
            .map(str::to_string)
            .collect()
    };

    let mut done = Vec::new();
    for line in &mut lines {
        let Some((key, value_at)) = assignment(line) else {
            continue;
        };
        if let Some(v) = set.get(key) {
            done.push(key.to_string());
            *line = format!("{}{v}", &line[..value_at]);
        }
    }
    for (k, v) in set {
        if !done.contains(k) {
            lines.push(format!("{k}={v}"));
        }
    }
    lines.join("\n") + "\n"
}

/// `^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=`: the key and the index
/// right after `=`.
pub(crate) fn assignment(line: &str) -> Option<(&str, usize)> {
    let start = line.len() - line.trim_start().len();
    let mut rest = &line[start..];
    let mut at = start;
    if let Some(r) = rest.strip_prefix("export")
        && r.starts_with(char::is_whitespace)
    {
        let r2 = r.trim_start();
        at += rest.len() - r2.len();
        rest = r2;
    }
    let first = rest.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let key_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let after = rest[key_len..].trim_start();
    if !after.starts_with('=') {
        return None;
    }
    let eq = at + key_len + (rest[key_len..].len() - after.len());
    Some((&line[at..at + key_len], eq + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn overwrites_keeps_and_appends() {
        let src = "# api\nexport API_URL = http://localhost:8787\nNAME=x\n\nOTHER=1\n";
        // Like Go: everything up to and including `=` stays, the value is replaced.
        let got = set(
            src,
            &m(&[
                ("API_URL", "http://localhost:20101"),
                ("ZED", "z"),
                ("NEW", "n"),
            ]),
        );
        assert_eq!(
            got,
            "# api\nexport API_URL =http://localhost:20101\nNAME=x\n\nOTHER=1\nNEW=n\nZED=z\n"
        );
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(set("A=1", &BTreeMap::new()), "A=1");
        assert_eq!(set("", &m(&[("A", "1")])), "A=1\n");
    }

    #[test]
    fn not_an_assignment() {
        let got = set(
            "exportA=1\n1A=2\nA_B=3\n",
            &m(&[("exportA", "x"), ("A_B", "y")]),
        );
        assert_eq!(got, "exportA=x\n1A=2\nA_B=y\n");
    }
}
