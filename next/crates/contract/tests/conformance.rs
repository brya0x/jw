//! Keeps the Rust types, the JSON Schema and the shared fixtures from drifting apart.
//! The fixtures are read by the Kotlin side too (REQ-12), so they are the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use contract::{BridgeMsg, ClientMsg, DaemonMsg, PhoneMsg};
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

#[test]
fn phone_and_bridge_fixtures_round_trip_and_validate() {
    let phone = validator("PhoneMsg");
    for (name, doc) in fixtures("phone") {
        let errs: Vec<_> = phone.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "phone/{name} breaks the schema: {errs:?}");
        round_trip::<PhoneMsg>(&format!("phone/{name}"), &doc);
    }
    let bridge = validator("BridgeMsg");
    for (name, doc) in fixtures("bridge") {
        let errs: Vec<_> = bridge.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errs.is_empty(), "bridge/{name} breaks the schema: {errs:?}");
        round_trip::<BridgeMsg>(&format!("bridge/{name}"), &doc);
    }
}

/// Every invalid fixture must be rejected by the schema (and name its side by prefix).
#[test]
fn invalid_fixtures_are_rejected() {
    let client = validator("ClientMsg");
    let daemon = validator("DaemonMsg");
    let phone = validator("PhoneMsg");
    let bridge = validator("BridgeMsg");
    for (name, doc) in fixtures("invalid") {
        let v = match name.split('-').next() {
            Some("client") => &client,
            Some("phone") => &phone,
            Some("bridge") => &bridge,
            _ => &daemon,
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
    let cases: [(&str, &str, &str, &str); 10] = [
        ("ClientMsg", "t", "client", "t"),
        ("DaemonMsg", "t", "daemon", "t"),
        ("Cmd", "cmd", "client", "cmd"),
        ("PatchOp", "op", "daemon", "op"),
        ("AttentionKind", "", "daemon", "kind"),
        ("NodeType", "", "daemon", "type"),
        ("ErrorCode", "", "daemon", "code"),
        ("PhoneMsg", "t", "phone", "t"),
        ("BridgeMsg", "t", "bridge", "t"),
        ("Denied", "", "bridge", "reason"),
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

/// Every patch case applies the same way in every language: `after` for a good case, an
/// error for a bad one. The Kotlin suite reads the same files.
#[test]
fn patch_cases_apply_as_written() {
    for (name, case) in fixtures("patch-cases") {
        let ops: Vec<contract::PatchOp> = serde_json::from_value(case["ops"].clone()).unwrap();
        let to = case["to"].as_u64().unwrap();
        let mut doc = case["before"].clone();
        let result = contract::patch::apply_patch(&mut doc, &ops, to);
        if case["error"] == true {
            assert!(result.is_err(), "patch-cases/{name} should fail");
        } else {
            result.unwrap_or_else(|e| panic!("patch-cases/{name}: {e}"));
            assert_eq!(doc, case["after"], "patch-cases/{name}");
        }
    }
}

/// What `diff` makes applies to the same result, for every good case.
#[test]
fn diff_of_a_patch_case_reproduces_its_after() {
    for (name, case) in fixtures("patch-cases") {
        if case["error"] == true {
            continue;
        }
        let ops = contract::patch::diff(&case["before"], &case["after"]);
        let mut doc = case["before"].clone();
        contract::patch::apply_patch(&mut doc, &ops, case["to"].as_u64().unwrap()).unwrap();
        assert_eq!(doc, case["after"], "patch-cases/{name} via diff {ops:?}");
    }
}

/// The result of `list_files` in the fixture is what `FileListing` writes and reads: the phone reads
/// the same document (`BlocksTest` of the Kotlin side does for its own type).
#[test]
fn the_reply_of_list_files_is_a_file_listing() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-list-files.json")
        .expect("the fixture");
    let listing: contract::FileListing = serde_json::from_value(doc["result"].clone()).unwrap();
    assert_eq!((listing.path.as_str(), listing.total), ("src", 4));
    assert_eq!(listing.entries[0].git, Some(contract::GitMark::Modified));
    assert!(listing.entries[1].ignored);
    assert_eq!(listing.entries[2].size, Some(1536));
    assert_eq!(listing.entries[3].kind, contract::FileKind::Link);
    assert_eq!(serde_json::to_value(&listing).unwrap(), doc["result"]);
}

/// The result of `open_conversation` in the fixture is what `Opened` writes and reads.
#[test]
fn the_reply_of_open_conversation_is_a_node_and_a_session() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-open-conversation.json")
        .expect("the fixture");
    let opened: contract::conversation::Opened =
        serde_json::from_value(doc["result"].clone()).unwrap();
    assert_eq!(opened.session.len(), 36);
    assert_eq!(serde_json::to_value(&opened).unwrap(), doc["result"]);
}

/// The result of `list_conversations` in the fixture is what `ConversationList` writes and reads.
#[test]
fn the_reply_of_list_conversations_is_a_list_of_saved_conversations() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-list-conversations.json")
        .expect("the fixture");
    let list: contract::conversation::ConversationList =
        serde_json::from_value(doc["result"].clone()).unwrap();
    assert_eq!(list.conversations.len(), 2);
    assert!(
        list.conversations[0].at > list.conversations[1].at,
        "newest first"
    );
    assert_eq!(serde_json::to_value(&list).unwrap(), doc["result"]);
}

/// The result of `read_markdown` in the fixture is what `MarkdownDoc` writes and reads, and holds
/// every kind of block and of text.
#[test]
fn the_reply_of_read_markdown_is_a_document_of_blocks() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-read-markdown.json")
        .expect("the fixture");
    let md: contract::markdown::MarkdownDoc =
        serde_json::from_value(doc["result"].clone()).unwrap();
    assert_eq!(md.title, "Plan: pruebas del módulo auth");
    let kinds: Vec<&str> = md
        .blocks
        .iter()
        .map(|b| match b {
            contract::markdown::MdBlock::Heading { .. } => "heading",
            contract::markdown::MdBlock::Para { .. } => "para",
            contract::markdown::MdBlock::Quote { .. } => "quote",
            contract::markdown::MdBlock::List { .. } => "list",
            contract::markdown::MdBlock::Code { .. } => "code",
            contract::markdown::MdBlock::Table { .. } => "table",
            contract::markdown::MdBlock::Rule => "rule",
        })
        .collect();
    assert_eq!(
        kinds,
        ["heading", "para", "quote", "list", "code", "table", "rule"]
    );
    assert_eq!(serde_json::to_value(&md).unwrap(), doc["result"]);
}

/// The blocks of `read_markdown` and of a finished text of a conversation are checked against the
/// schema's `MdBlock`, and the text of the conversation fixture keeps its blocks as written.
#[test]
fn markdown_blocks_validate_and_a_finished_text_keeps_them() {
    let block = validator("MdBlock");
    let docs = fixtures("daemon");
    let (_, reply) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-read-markdown.json")
        .expect("the fixture");
    for b in reply["result"]["blocks"].as_array().unwrap() {
        assert!(block.is_valid(b), "the schema accepts {b}");
    }
    assert!(!block.is_valid(&serde_json::json!({ "t": "table", "head": [] })));
    let (_, snap) = docs
        .iter()
        .find(|(n, _)| n == "snapshot-conversation-markdown.json")
        .expect("the fixture");
    let items: Vec<contract::conversation::Item> =
        serde_json::from_value(snap["node"]["items"].clone()).unwrap();
    let with = items
        .iter()
        .find_map(|i| match i {
            contract::conversation::Item::Text {
                blocks: Some(b), ..
            } => Some(b),
            _ => None,
        })
        .expect("a text with blocks");
    assert!(
        with.iter()
            .any(|b| matches!(b, contract::markdown::MdBlock::Table { .. }))
    );
    assert_eq!(serde_json::to_value(&items).unwrap(), snap["node"]["items"]);
}

/// The conversation fixture with a tool that runs says when it began and what the turn has
/// written, and writes both back as they were (REQ-562, REQ-563).
#[test]
fn a_tool_that_runs_says_when_it_began_and_the_turn_how_many_tokens() {
    let docs = fixtures("daemon");
    let (_, snap) = docs
        .iter()
        .find(|(n, _)| n == "snapshot-conversation-running.json")
        .expect("the fixture");
    let conv: contract::conversation::Conversation =
        serde_json::from_value(snap["node"].clone()).unwrap();
    assert_eq!(conv.running.len(), 1);
    assert_eq!(conv.running[0].since, 1_792_110_004_000);
    assert_eq!(conv.tokens, 1250);
    let back = serde_json::to_value(&conv).unwrap();
    assert_eq!(back["running"], snap["node"]["running"]);
    assert_eq!(back["tokens"], snap["node"]["tokens"]);
}

/// The conversation fixture of a followed turn says when it began, with its tool running and no
/// turn of the conversation's own (REQ-565).
#[test]
fn a_followed_turn_says_when_it_began() {
    let docs = fixtures("daemon");
    let (_, snap) = docs
        .iter()
        .find(|(n, _)| n == "snapshot-conversation-following.json")
        .expect("the fixture");
    let conv: contract::conversation::Conversation =
        serde_json::from_value(snap["node"].clone()).unwrap();
    assert_eq!(conv.following, Some(1_792_110_000_000));
    assert_eq!(conv.since, None);
    assert_eq!(conv.running.len(), 1);
    assert_eq!(
        serde_json::to_value(&conv).unwrap()["following"],
        snap["node"]["following"]
    );
}

/// The result of `read_older` in the fixture is what `OlderItems` writes and reads, and the node
/// of the running fixture says the `before` it asks with (REQ-567).
#[test]
fn the_reply_of_read_older_is_items_and_where_to_ask_for_the_ones_before() {
    let docs = fixtures("daemon");
    let (_, reply) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-read-older.json")
        .expect("the fixture");
    let older: contract::conversation::OlderItems =
        serde_json::from_value(reply["result"].clone()).unwrap();
    assert_eq!(older.items.len(), 3);
    assert_eq!(older.older, Some(1_792_109_900_000));
    assert_eq!(serde_json::to_value(&older).unwrap(), reply["result"]);
    let (_, snap) = docs
        .iter()
        .find(|(n, _)| n == "snapshot-conversation-running.json")
        .expect("the fixture");
    let conv: contract::conversation::Conversation =
        serde_json::from_value(snap["node"].clone()).unwrap();
    assert_eq!(conv.older, Some(1_792_109_990_000));
}

/// The result of `project_config` in the fixture is what `ProjectInfo` writes and reads.
#[test]
fn the_reply_of_project_config_is_a_project_info() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-project-config.json")
        .expect("the fixture");
    let info: contract::ProjectInfo = serde_json::from_value(doc["result"].clone()).unwrap();
    assert_eq!(info.open, contract::OpenKind::Claude);
    assert_eq!(info.root_text.as_deref(), Some("../app-trees"));
    assert_eq!(serde_json::to_value(&info).unwrap(), doc["result"]);
}

/// The result of `list_worktrees` in the fixture is what `WorktreeInfo` writes and reads.
#[test]
fn the_reply_of_list_worktrees_is_a_list_of_worktree_infos() {
    let docs = fixtures("daemon");
    let (_, doc) = docs
        .iter()
        .find(|(n, _)| n == "reply-ok-worktrees.json")
        .expect("the fixture");
    let list: Vec<contract::WorktreeInfo> =
        serde_json::from_value(doc["result"]["worktrees"].clone()).unwrap();
    assert_eq!(list.len(), 4);
    assert!(list[0].workspace.is_some() && list[1].changes == Some(3));
    assert!(list[2].branch.is_none(), "a detached head has no branch");
    assert!(list[3].missing && list[3].changes.is_none());
    assert_eq!(
        serde_json::to_value(&list).unwrap(),
        doc["result"]["worktrees"]
    );
}
