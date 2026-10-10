//! Placeholders in config strings: {name}, {base}, {slot}, {port.<service>}.
//! Port of internal/core/config/expand.go.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

/// The values placeholders expand to.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub name: String,
    /// The default branch.
    pub base: String,
    pub slot: u32,
    pub ports: BTreeMap<String, u32>,
}

/// Replaces every placeholder in `s`. An unknown placeholder is an error
/// rather than being left in place.
pub fn expand(s: &str, v: &Vars) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut bad = Vec::new();
    let mut rest = s;
    while let Some((before, key, after)) = next_placeholder(rest) {
        out.push_str(before);
        let value = match key {
            "name" => Some(v.name.clone()),
            "base" => Some(v.base.clone()),
            "slot" => Some(v.slot.to_string()),
            _ => key
                .strip_prefix("port.")
                .and_then(|svc| v.ports.get(svc))
                .map(u32::to_string),
        };
        match value {
            Some(val) => out.push_str(&val),
            None => {
                bad.push(format!("{{{key}}}"));
                out.push_str(&format!("{{{key}}}"));
            }
        }
        rest = after;
    }
    out.push_str(rest);
    if !bad.is_empty() {
        bail!("unknown placeholder {}", bad.join(", "));
    }
    Ok(out)
}

/// The services whose port `s` refers to, in order of first use.
pub fn ports_in(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = s;
    while let Some((_, key, after)) = next_placeholder(rest) {
        if let Some(svc) = key.strip_prefix("port.")
            && !out.iter().any(|o| o == svc)
        {
            out.push(svc.to_string());
        }
        rest = after;
    }
    out
}

/// Turns a service name into its variable: console-web → JW_PORT_CONSOLE_WEB.
pub fn env_name(service: &str) -> String {
    format!(
        "JW_PORT_{}",
        service.to_uppercase().replace(['-', '.'], "_")
    )
}

/// Finds the next `{key}` whose key matches Go's `[a-z0-9_.-]+`, returning
/// the text before it, the key, and the text after it.
fn next_placeholder(s: &str) -> Option<(&str, &str, &str)> {
    let is_key = |c: char| matches!(c, 'a'..='z' | '0'..='9' | '_' | '.' | '-');
    let mut from = 0;
    while let Some(open) = s[from..].find('{').map(|i| from + i) {
        let body = &s[open + 1..];
        let len = body.find(|c: char| !is_key(c)).unwrap_or(body.len());
        if len > 0 && body[len..].starts_with('}') {
            return Some((&s[..open], &body[..len], &body[len + 1..]));
        }
        from = open + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_every_placeholder() {
        let v = Vars {
            name: "web".into(),
            base: "development".into(),
            slot: 3,
            ports: [("api".to_string(), 20302)].into(),
        };
        let got = expand("{name} {base} {slot} http://localhost:{port.api}", &v).unwrap();
        assert_eq!(got, "web development 3 http://localhost:20302");
    }

    #[test]
    fn unknown_placeholder_fails() {
        let err = expand("vite --port {port.wbe} {nope}", &Vars::default()).unwrap_err();
        assert_eq!(err.to_string(), "unknown placeholder {port.wbe}, {nope}");
    }

    #[test]
    fn braces_that_are_not_placeholders_stay() {
        let v = Vars {
            name: "x".into(),
            ..Vars::default()
        };
        assert_eq!(expand("{{name}} {A} {} {", &v).unwrap(), "{x} {A} {} {");
    }

    #[test]
    fn ports_in_first_use_order() {
        let got = ports_in("PORT={port.api} vite --port {port.web} --api {port.api} {name}");
        assert_eq!(got, ["api", "web"]);
    }

    #[test]
    fn env_names() {
        assert_eq!(env_name("console-web"), "JW_PORT_CONSOLE_WEB");
        assert_eq!(env_name("a.b"), "JW_PORT_A_B");
    }
}
