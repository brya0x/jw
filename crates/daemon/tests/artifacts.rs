//! N171a: artifacts are kept as immutable versions per workspace, listed by a node, held as
//! attention while one waits for a verdict, and answered by commands (REQ-759 to REQ-768, 790, 791).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{Sandbox, WAIT};
use contract::artifacts::{ArtifactFormat, Verdict};
use contract::{Cmd, ErrorCode};
use serde_json::Value;
use wire::Client;

struct World {
    sb: Sandbox,
    ws: String,
}

fn open_folder(c: &mut Client, dir: &Path) -> String {
    c.cmd(Cmd::OpenFolder {
        path: dir.to_string_lossy().into(),
    })
    .unwrap()
    .unwrap()
    .unwrap()["workspace"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn world() -> (World, daemon::Daemon, Client) {
    let sb = Sandbox::new();
    let d = sb.start();
    let dir = std::fs::canonicalize(&sb.dir).unwrap().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = sb.client();
    let ws = open_folder(&mut c, &dir);
    (World { sb, ws }, d, c)
}

fn publish(c: &mut Client, ws: &str, name: &str, text: &str) -> Result<Value, contract::ErrorCode> {
    publish_as(c, ws, name, ArtifactFormat::Html, text)
}

fn publish_as(
    c: &mut Client,
    ws: &str,
    name: &str,
    format: ArtifactFormat,
    text: &str,
) -> Result<Value, contract::ErrorCode> {
    c.cmd(Cmd::ArtifactPublish {
        workspace: ws.into(),
        name: name.into(),
        title: None,
        kind: None,
        format,
        text: text.into(),
        pane: None,
    })
    .unwrap()
    .map(Option::unwrap)
    .map_err(|e| e.code)
}

fn decide(
    c: &mut Client,
    id: &str,
    n: u32,
    hash: &str,
    verdict: Verdict,
    text: Option<&str>,
) -> Result<(), ErrorCode> {
    c.cmd(Cmd::ArtifactDecide {
        id: id.into(),
        n,
        hash: hash.into(),
        verdict,
        text: text.map(Into::into),
    })
    .unwrap()
    .map(|_| ())
    .map_err(|e| e.code)
}

fn open_node(c: &mut Client, ws: &str) -> String {
    let node = c
        .cmd(Cmd::OpenArtifacts {
            workspace: ws.into(),
        })
        .unwrap()
        .unwrap()
        .unwrap()["node"]
        .as_str()
        .unwrap()
        .to_owned();
    c.open(&node).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(&node)).unwrap());
    node
}

fn watch(c: &mut Client, node: &str) {
    c.open(node).unwrap();
    assert!(c.wait_until(WAIT, |c| c.nodes.contains_key(node)).unwrap());
}

fn items(c: &Client, node: &str) -> Vec<Value> {
    c.nodes[node]["items"].as_array().unwrap().clone()
}

fn kinds(c: &Client, ws: &str) -> Vec<String> {
    c.nodes[ws]["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn store(sb: &Sandbox) -> PathBuf {
    sb.dir.join("state").join("artifacts")
}

#[test]
fn a_publish_makes_the_next_version_the_same_bytes_make_none_and_the_oldest_go_past_twenty() {
    let (w, d, mut c) = world();
    let first = publish(&mut c, &w.ws, "ajustes", "<h1>1</h1>").unwrap();
    assert_eq!(
        (first["n"].as_u64(), first["new"].as_bool()),
        (Some(1), Some(true))
    );
    assert_eq!(
        first["hash"].as_str().unwrap(),
        web::auth::sha256_hex("<h1>1</h1>")
    );
    let id = first["id"].as_str().unwrap().to_owned();
    let again = publish(&mut c, &w.ws, "ajustes", "<h1>1</h1>").unwrap();
    assert_eq!(
        (
            again["id"].as_str(),
            again["n"].as_u64(),
            again["new"].as_bool()
        ),
        (Some(id.as_str()), Some(1), Some(false))
    );
    for n in 2..=21 {
        let r = publish(&mut c, &w.ws, "ajustes", &format!("<h1>{n}</h1>")).unwrap();
        assert_eq!(r["n"].as_u64(), Some(n));
    }
    let node = open_node(&mut c, &w.ws);
    let versions = items(&c, &node)[0]["versions"].as_array().unwrap().clone();
    let ns: Vec<u64> = versions.iter().map(|v| v["n"].as_u64().unwrap()).collect();
    assert_eq!(ns, (2..=21).collect::<Vec<_>>());
    let dir = store(&w.sb).join(&w.ws).join(&id);
    assert!(!dir.join("v1.html").exists());
    assert!(dir.join("v2.html").exists() && dir.join("v21.html").exists());
    // A second name is a second artifact, and a name is the workspace's own.
    publish(&mut c, &w.ws, "otro", "x").unwrap();
    assert!(c.wait_until(WAIT, |c| items(c, &node).len() == 2).unwrap());
    d.stop();
}

#[test]
fn what_is_refused_is_not_stored() {
    let (w, d, mut c) = world();
    let at_cap = "x".repeat(512 * 1024);
    assert!(publish(&mut c, &w.ws, "cap", &at_cap).is_ok());
    // The cap counts bytes, not characters.
    let multi = "é".repeat(256 * 1024 + 1);
    assert_eq!(
        publish(&mut c, &w.ws, "multi", &multi),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        publish(&mut c, &w.ws, "big", &format!("{at_cap}x")),
        Err(ErrorCode::BadRequest)
    );
    for bad in ["", "   ", "a/b", &"n".repeat(65)] {
        assert_eq!(
            publish(&mut c, &w.ws, bad, "x"),
            Err(ErrorCode::BadRequest),
            "{bad:?}"
        );
    }
    assert!(publish(&mut c, &w.ws, &"n".repeat(64), "x").is_ok());
    assert_eq!(
        publish(&mut c, "00000000-0000-4000-8000-000000000999", "x", "x"),
        Err(ErrorCode::NotFound)
    );
    // An artifact keeps its format: the other one is another name.
    assert_eq!(
        publish_as(&mut c, &w.ws, "cap", ArtifactFormat::Md, "# x"),
        Err(ErrorCode::BadRequest)
    );
    let node = open_node(&mut c, &w.ws);
    let names: Vec<String> = items(&c, &node)
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["cap".to_owned(), "n".repeat(64)]);
    d.stop();
}

#[test]
fn the_files_are_private_and_a_meta_that_cannot_be_read_is_moved_aside() {
    let (w, d, mut c) = world();
    let r = publish(&mut c, &w.ws, "ajustes", "<p>a").unwrap();
    let id = r["id"].as_str().unwrap().to_owned();
    publish(&mut c, &w.ws, "bueno", "<p>b").unwrap();
    let dir = store(&w.sb).join(&w.ws).join(&id);
    for f in ["meta.json", "v1.html"] {
        let mode = std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{f}");
    }
    d.stop();
    std::fs::write(dir.join("meta.json"), "not json").unwrap();
    let d = w.sb.start();
    let mut c = w.sb.client();
    let node = open_node(&mut c, &w.ws);
    let names: Vec<String> = items(&c, &node)
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["bueno"]);
    assert!(dir.join("meta.json.bad").exists() && !dir.join("meta.json").exists());
    d.stop();
}

#[test]
fn the_attention_is_held_while_one_waits_it_is_not_ended_by_seeing_and_it_comes_back_after_a_restart()
 {
    let (w, d, mut c) = world();
    watch(&mut c, &w.ws);
    assert!(kinds(&c, &w.ws).is_empty());
    let a = publish(&mut c, &w.ws, "a", "<p>a").unwrap();
    assert!(
        c.wait_until(WAIT, |c| kinds(c, &w.ws) == ["artifact"])
            .unwrap()
    );
    // Looking at the workspace does not end it (REQ-791).
    let rev = c.nodes[&w.ws]["attention"][0]["rev"].as_u64().unwrap();
    c.cmd(Cmd::MarkSeen {
        node: w.ws.clone(),
        rev: rev + 100,
    })
    .unwrap()
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    c.poll(std::time::Duration::from_millis(200)).unwrap();
    assert_eq!(kinds(&c, &w.ws), ["artifact"]);
    // A second artifact, and a verdict on the first: one still waits.
    let b = publish(&mut c, &w.ws, "b", "<p>b").unwrap();
    decide(
        &mut c,
        a["id"].as_str().unwrap(),
        1,
        a["hash"].as_str().unwrap(),
        Verdict::Approved,
        None,
    )
    .unwrap();
    c.poll(std::time::Duration::from_millis(200)).unwrap();
    assert_eq!(kinds(&c, &w.ws), ["artifact"]);
    // It is raised again from disk when the daemon starts.
    d.stop();
    let d = w.sb.start();
    let mut c = w.sb.client();
    watch(&mut c, &w.ws);
    assert_eq!(kinds(&c, &w.ws), ["artifact"]);
    // The last verdict ends it.
    decide(
        &mut c,
        b["id"].as_str().unwrap(),
        1,
        b["hash"].as_str().unwrap(),
        Verdict::Changes,
        Some("más"),
    )
    .unwrap();
    assert!(c.wait_until(WAIT, |c| kinds(c, &w.ws).is_empty()).unwrap());
    d.stop();
}

#[test]
fn a_verdict_is_bound_to_the_latest_version_and_its_hash() {
    let (w, d, mut c) = world();
    let node = open_node(&mut c, &w.ws);
    let one = publish(&mut c, &w.ws, "a", "<p>1").unwrap();
    let id = one["id"].as_str().unwrap().to_owned();
    let h1 = one["hash"].as_str().unwrap().to_owned();
    let two = publish(&mut c, &w.ws, "a", "<p>2").unwrap();
    let h2 = two["hash"].as_str().unwrap().to_owned();
    // Version 1 is not the latest; a wrong hash is not the latest's.
    assert_eq!(
        decide(&mut c, &id, 1, &h1, Verdict::Approved, None),
        Err(ErrorCode::Conflict)
    );
    assert_eq!(
        decide(&mut c, &id, 2, &h1, Verdict::Approved, None),
        Err(ErrorCode::Conflict)
    );
    assert_eq!(
        decide(&mut c, "nope", 2, &h2, Verdict::Approved, None),
        Err(ErrorCode::NotFound)
    );
    // A «changes» needs a text, of up to 4 KiB; an approval takes none.
    assert_eq!(
        decide(&mut c, &id, 2, &h2, Verdict::Changes, None),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        decide(&mut c, &id, 2, &h2, Verdict::Changes, Some("  \n")),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        decide(
            &mut c,
            &id,
            2,
            &h2,
            Verdict::Changes,
            Some(&"x".repeat(4097))
        ),
        Err(ErrorCode::BadRequest)
    );
    assert_eq!(
        decide(&mut c, &id, 2, &h2, Verdict::Approved, Some("hi")),
        Err(ErrorCode::BadRequest)
    );
    decide(
        &mut c,
        &id,
        2,
        &h2,
        Verdict::Changes,
        Some(&"x".repeat(4096)),
    )
    .unwrap();
    // Decided once.
    assert_eq!(
        decide(&mut c, &id, 2, &h2, Verdict::Approved, None),
        Err(ErrorCode::Conflict)
    );
    // The node says it, with the text trimmed, and a new version is pending again.
    assert!(
        c.wait_until(
            WAIT,
            |c| c.nodes[&node]["items"][0]["versions"][1]["verdict"]["type"] == "changes"
        )
        .unwrap()
    );
    publish(&mut c, &w.ws, "a", "<p>3").unwrap();
    assert!(
        c.wait_until(WAIT, |c| c.nodes[&node]["items"][0]["versions"]
            .as_array()
            .unwrap()
            .len()
            == 3)
            .unwrap()
    );
    assert!(
        c.nodes[&node]["items"][0]["versions"][2]
            .get("verdict")
            .is_none()
    );
    d.stop();
}

#[test]
fn a_verdict_text_is_trimmed() {
    let (w, d, mut c) = world();
    let node = open_node(&mut c, &w.ws);
    let r = publish(&mut c, &w.ws, "a", "<p>1").unwrap();
    decide(
        &mut c,
        r["id"].as_str().unwrap(),
        1,
        r["hash"].as_str().unwrap(),
        Verdict::Changes,
        Some("  más grande \n"),
    )
    .unwrap();
    assert!(
        c.wait_until(
            WAIT,
            |c| c.nodes[&node]["items"][0]["versions"][0]["verdict"]["text"] == "más grande"
        )
        .unwrap()
    );
    d.stop();
}

#[test]
fn read_gives_html_or_blocks_and_a_reply_the_link_cannot_carry_is_refused() {
    let (w, d, mut c) = world();
    let html = publish(&mut c, &w.ws, "p", "<h1>Hola</h1>").unwrap();
    let md = publish_as(
        &mut c,
        &w.ws,
        "spec",
        ArtifactFormat::Md,
        "# N171\n\nUna spec.",
    )
    .unwrap();
    let read = |c: &mut Client, id: &str, n: Option<u32>| {
        c.cmd(Cmd::ArtifactRead { id: id.into(), n })
            .unwrap()
            .map(Option::unwrap)
            .map_err(|e| e.code)
    };
    let doc = read(&mut c, html["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(
        (doc["html"].as_str(), doc.get("doc")),
        (Some("<h1>Hola</h1>"), None)
    );
    assert_eq!(doc["hash"], html["hash"]);
    let doc = read(&mut c, md["id"].as_str().unwrap(), Some(1)).unwrap();
    assert_eq!(
        (doc["title"].as_str(), doc["doc"]["title"].as_str()),
        (Some("spec"), Some("N171"))
    );
    assert!(doc.get("html").is_none());
    assert_eq!(read(&mut c, "nope", None), Err(ErrorCode::NotFound));
    assert_eq!(
        read(&mut c, html["id"].as_str().unwrap(), Some(9)),
        Err(ErrorCode::NotFound)
    );
    // 512 KiB of quotes is accepted and escapes to about 1 MiB: too much for one message.
    let quotes = "\"".repeat(512 * 1024);
    let q = publish(&mut c, &w.ws, "q", &quotes).unwrap();
    assert_eq!(
        read(&mut c, q["id"].as_str().unwrap(), None),
        Err(ErrorCode::BadRequest)
    );
    d.stop();
}

#[test]
fn the_node_is_empty_before_the_first_publish_and_patched_after() {
    let (w, d, mut c) = world();
    let node = open_node(&mut c, &w.ws);
    assert_eq!(c.nodes[&node]["type"], "artifacts");
    assert!(items(&c, &node).is_empty());
    // A second client, a second request: the same node.
    let mut other = w.sb.client();
    assert_eq!(open_node(&mut other, &w.ws), node);
    publish(&mut c, &w.ws, "a", "<p>a").unwrap();
    assert!(c.wait_until(WAIT, |c| items(c, &node).len() == 1).unwrap());
    assert!(
        other
            .wait_until(WAIT, |c| items(c, &node).len() == 1)
            .unwrap()
    );
    assert_eq!(
        c.cmd(Cmd::OpenArtifacts {
            workspace: "00000000-0000-4000-8000-000000000999".into()
        })
        .unwrap()
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    d.stop();
}

#[test]
fn removing_an_artifact_or_its_workspace_deletes_the_files_and_the_attention() {
    let (w, d, mut c) = world();
    watch(&mut c, &w.ws);
    let a = publish(&mut c, &w.ws, "a", "<p>a").unwrap();
    let id = a["id"].as_str().unwrap().to_owned();
    assert!(
        c.wait_until(WAIT, |c| kinds(c, &w.ws) == ["artifact"])
            .unwrap()
    );
    c.cmd(Cmd::ArtifactRm { id: id.clone() }).unwrap().unwrap();
    assert!(!store(&w.sb).join(&w.ws).join(&id).exists());
    assert!(c.wait_until(WAIT, |c| kinds(c, &w.ws).is_empty()).unwrap());
    assert_eq!(
        c.cmd(Cmd::ArtifactRm { id }).unwrap().unwrap_err().code,
        ErrorCode::NotFound
    );
    publish(&mut c, &w.ws, "b", "<p>b").unwrap();
    assert!(store(&w.sb).join(&w.ws).exists());
    c.cmd(Cmd::RemoveWorkspace {
        workspace: w.ws.clone(),
        remove_worktree: false,
        force: false,
    })
    .unwrap()
    .unwrap();
    assert!(!store(&w.sb).join(&w.ws).exists());
    d.stop();
}

#[test]
fn the_text_of_an_artifact_is_in_no_node_but_its_own() {
    let (w, d, mut c) = world();
    let node = open_node(&mut c, &w.ws);
    let root = c.root.clone();
    watch(&mut c, &root);
    watch(&mut c, &w.ws);
    let marker = "MARKER-7f3a-never-elsewhere";
    let r = publish(&mut c, &w.ws, "a", &format!("<p>{marker}")).unwrap();
    decide(
        &mut c,
        r["id"].as_str().unwrap(),
        1,
        r["hash"].as_str().unwrap(),
        Verdict::Changes,
        Some("más"),
    )
    .unwrap();
    c.poll(std::time::Duration::from_millis(300)).unwrap();
    for (id, doc) in &c.nodes {
        assert!(!doc.to_string().contains(marker), "node {id}");
    }
    assert!(items(&c, &node).len() == 1);
    d.stop();
}

#[test]
fn a_title_is_kept_on_one_line() {
    let (w, d, mut c) = world();
    let node = open_node(&mut c, &w.ws);
    c.cmd(Cmd::ArtifactPublish {
        workspace: w.ws.clone(),
        name: "a".into(),
        title: Some("  Ajustes\n\tde  prueba ".into()),
        kind: None,
        format: ArtifactFormat::Html,
        text: "<p>a".into(),
        pane: None,
    })
    .unwrap()
    .unwrap();
    assert!(c.wait_until(WAIT, |c| items(c, &node).len() == 1).unwrap());
    assert_eq!(items(&c, &node)[0]["title"], "Ajustes de prueba");
    d.stop();
}
