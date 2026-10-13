//! Keeps the Rust types, the JSON Schema and the shared fixtures from drifting apart.
//! The fixtures are read by the Kotlin side too (REQ-12), so they are the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use contract::{ClientMsg, DaemonMsg};
use jsonschema::Validator;
use serde_json::Value;

fn contract_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract")
}

fn schema() -> Value {
    let text = fs::read_to_string(contract_dir().join("schema/contract.schema.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn validator(def: &str) -> Validator {
    let mut root = schema();
    root["$ref"] = format!("#/$defs/{def}").into();
    jsonschema::validator_for(&root).expect("schema compiles")
}

fn fixtures(dir: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(contract_dir().join("fixtures").join(dir)).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        out.push((name, serde_json::from_str(&text).unwrap()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no fixtures in {dir}");
    out
}

/// A fixture parses into the Rust type and writes back the identical document.
fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(name: &str, doc: &Value) {
    let typed: T = serde_json::from_value(doc.clone())
        .unwrap_or_else(|e| panic!("{name}: Rust cannot parse it: {e}"));
    let back = serde_json::to_value(&typed).unwrap();
    assert_eq!(&back, doc, "{name}: Rust writes a different document");
}

#[test]
fn client_fixtures_round_trip_and_validate() {
    let v = validator("ClientMsg");
    for (name, doc) in fixtures("client") {
        let errs: Vec<_> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "client/{name} breaks the schema: {errs:?}");
        round_trip::<ClientMsg>(&format!("client/{name}"), &doc);
    }
}

#[test]
fn daemon_fixtures_round_trip_and_validate() {
    let v = validator("DaemonMsg");
    for (name, doc) in fixtures("daemon") {
        let errs: Vec<_> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "daemon/{name} breaks the schema: {errs:?}");
        round_trip::<DaemonMsg>(&format!("daemon/{name}"), &doc);
    }
}

/// Every invalid fixture must be rejected by the schema (and name its side by prefix).
#[test]
fn invalid_fixtures_are_rejected() {
    let client = validator("ClientMsg");
    let daemon = validator("DaemonMsg");
    for (name, doc) in fixtures("invalid") {
        let v = if name.starts_with("client-") {
            &client
        } else {
            &daemon
        };
        assert!(!v.is_valid(&doc), "invalid/{name} passed the schema");
    }
}

/// An unknown field is ignored by Rust and allowed by the schema (additive changes never
/// break an old peer).
#[test]
fn unknown_fields_are_ignored() {
    let mut doc = fixtures("daemon")
        .into_iter()
        .find(|f| f.0 == "snapshot-pane.json")
        .unwrap()
        .1;
    doc["node"]["future_field"] = Value::from(1);
    doc["future_top"] = Value::from(true);
    assert!(serde_json::from_value::<DaemonMsg>(doc.clone()).is_ok());
    assert!(validator("DaemonMsg").is_valid(&doc));
}

/// Collect every `const` / `enum` string under `def`'s `oneOf` branches at `prop`.
fn schema_values(def: &str, prop: &str) -> BTreeSet<String> {
    let s = schema();
    let d = &s["$defs"][def];
    let mut out = BTreeSet::new();
    if let Some(branches) = d["oneOf"].as_array() {
        for b in branches {
            if let Some(c) = b["properties"][prop]["const"].as_str() {
                out.insert(c.to_owned());
            }
        }
    }
    if let Some(items) = d["enum"].as_array() {
        out.extend(items.iter().filter_map(|x| x.as_str().map(str::to_owned)));
    }
    out
}

fn collect(doc: &Value, key: &str, out: &mut BTreeSet<String>) {
    match doc {
        Value::Object(m) => {
            for (k, v) in m {
                if k == key
                    && let Some(s) = v.as_str()
                {
                    out.insert(s.to_owned());
                }
                collect(v, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| collect(v, key, out)),
        _ => {}
    }
}

fn found(dir: &str, key: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, doc) in fixtures(dir) {
        collect(&doc, key, &mut out);
    }
    out
}

/// A message, command, node type, attention kind or error code in the schema with no
/// valid fixture is an untested corner: fail until one is added.
#[test]
fn every_schema_variant_has_a_fixture() {
    let cases: [(&str, &str, &str, &str); 7] = [
        ("ClientMsg", "t", "client", "t"),
        ("DaemonMsg", "t", "daemon", "t"),
        ("Cmd", "cmd", "client", "cmd"),
        ("PatchOp", "op", "daemon", "op"),
        ("AttentionKind", "", "daemon", "kind"),
        ("NodeType", "", "daemon", "type"),
        ("ErrorCode", "", "daemon", "code"),
    ];
    for (def, prop, dir, key) in cases {
        let want = schema_values(def, prop);
        assert!(!want.is_empty(), "{def}: nothing found in the schema");
        let have = found(dir, key);
        let missing: Vec<_> = want.difference(&have).collect();
        assert!(
            missing.is_empty(),
            "{def}: no fixture in {dir}/ for {missing:?}"
        );
    }
}

/// Rust and the schema must list the same attention kinds.
#[test]
fn rust_enums_match_the_schema() {
    let kinds = schema_values("AttentionKind", "");
    for k in &kinds {
        let parsed: Result<contract::AttentionKind, _> =
            serde_json::from_value(Value::from(k.as_str()));
        assert!(parsed.is_ok(), "Rust does not know attention kind {k}");
    }
    let codes = schema_values("ErrorCode", "");
    for c in &codes {
        let parsed: Result<contract::ErrorCode, _> =
            serde_json::from_value(Value::from(c.as_str()));
        assert!(parsed.is_ok(), "Rust does not know error code {c}");
    }
}
