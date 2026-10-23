//! `artifact`: publish a file for the person to look at on the phone, list what is published,
//! remove it, and wait for the verdict (N171b, REQ-771 to REQ-773).

use std::fs;
use std::time::Duration;

use contract::Cmd;
use contract::artifacts::{
    ArtifactFormat, ArtifactItem, ArtifactKind, Artifacts, MAX_ARTIFACT_BYTES, Verdict,
};
use serde_json::{Value, json};

use super::{Args, Fail, Link, Out, TURN_WAIT, fail, name_of, own_pane, timeout_flag};

pub(super) fn run(args: &[&str]) -> Out {
    match args {
        ["publish", rest @ ..] => publish(rest),
        ["ls", rest @ ..] => ls(rest),
        ["wait", rest @ ..] => wait(rest),
        ["rm", rest @ ..] => rm(rest),
        _ => Err(Fail::Usage(String::new())),
    }
}

/// `.html` and `.htm` are `html`, `.md` is `md`; and the file's name without it, which names the
/// artifact unless `--name` does.
fn format_of(file: &str) -> Out<(ArtifactFormat, String)> {
    let path = std::path::Path::new(file);
    let format = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        Some(e) if e == "html" || e == "htm" => ArtifactFormat::Html,
        Some(e) if e == "md" => ArtifactFormat::Md,
        _ => return Err(Fail::Usage("a file of .html, .htm or .md".into())),
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(file);
    Ok((format, stem.to_lowercase()))
}

/// The title the file says: the `<title>` of a page, the first heading of Markdown.
fn title_of(text: &str, format: ArtifactFormat) -> Option<String> {
    let found = match format {
        ArtifactFormat::Html => {
            // `to_ascii_lowercase` keeps every byte offset, so the same ones cut `text`.
            let lower = text.to_ascii_lowercase();
            let open = lower.find("<title")?;
            let from = open + lower[open..].find('>')? + 1;
            let to = from + lower[from..].find("</title>")?;
            text[from..to].to_owned()
        }
        ArtifactFormat::Md => text
            .lines()
            .find(|l| l.starts_with('#'))?
            .trim_start_matches('#')
            .to_owned(),
    };
    let title = found.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn status_of(item: &ArtifactItem) -> &'static str {
    match item
        .versions
        .last()
        .and_then(|v| v.verdict.as_ref())
        .map(|v| v.verdict)
    {
        None => "pending",
        Some(Verdict::Approved) => "approved",
        Some(Verdict::Changes) => "changes",
    }
}

fn publish(args: &[&str]) -> Out {
    let a = Args::parse(args, &["name", "title", "kind", "timeout"], &["wait"])?;
    let [file] = a.words::<1>()?;
    let timeout = timeout_flag(&a)?;
    let (format, stem) = format_of(file)?;
    let kind = a
        .value("kind")
        .map(|k| {
            serde_json::from_value::<ArtifactKind>(json!(k))
                .map_err(|_| Fail::Usage("--kind is prototype, document, report or spec".into()))
        })
        .transpose()?;
    let size = fs::metadata(file)
        .map_err(|e| Fail::Error(format!("cannot read {file}: {e}")))?
        .len();
    if size > MAX_ARTIFACT_BYTES as u64 {
        return fail(format!(
            "{file} is {size} bytes, the most is {MAX_ARTIFACT_BYTES}: put its images and styles inside it, or make it smaller"
        ));
    }
    let text = fs::read_to_string(file)
        .map_err(|e| Fail::Error(format!("cannot read {file} as text: {e}")))?;
    let title = a
        .value("title")
        .map(str::to_owned)
        .or_else(|| title_of(&text, format));
    let name = a.value("name").map_or(stem, str::to_owned);
    let pane = own_pane().ok_or_else(|| {
        Fail::Error("it publishes from a pane of the daemon: run it where an agent runs".into())
    })?;
    let mut s = Link::connect()?;
    let ws = s
        .workspace_of(&pane)?
        .ok_or_else(|| Fail::Error(format!("no workspace has the pane {pane}")))?;
    let reply = s.cmd(Cmd::ArtifactPublish {
        workspace: ws.id.clone(),
        name,
        title,
        kind,
        format,
        text,
        pane: Some(pane),
    })?;
    let n = version_of(&reply)?;
    let id = reply["id"]
        .as_str()
        .ok_or_else(|| Fail::Error("the daemon's reply has no artifact id".into()))?
        .to_owned();
    let hash = reply["hash"].as_str().unwrap_or_default().to_owned();
    println!("{id} v{n} {}", hash.get(..6).unwrap_or(&hash));
    match timeout {
        Some(t) => wait_verdict(&mut s, &ws.id, &id, n, t),
        None => Ok(()),
    }
}

/// The version number the daemon gave a publish: it must fit a `u32`, else the wait would watch
/// the wrong version.
fn version_of(reply: &Value) -> Out<u32> {
    reply["n"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Fail::Error("the daemon's reply has no usable version number".into()))
}

/// The `artifacts` node of a workspace, open and current.
fn artifacts_of(s: &mut Link, workspace: &str) -> Out<(String, Artifacts)> {
    let reply = s.cmd(Cmd::OpenArtifacts {
        workspace: workspace.to_owned(),
    })?;
    let node = reply["node"].as_str().unwrap_or_default().to_owned();
    let doc = s.node::<Artifacts>(&node)?;
    Ok((node, doc))
}

fn verdict_of(doc: &Value, id: &str, n: u32) -> Option<(Verdict, Option<String>)> {
    let doc: Artifacts = serde_json::from_value(doc.clone()).ok()?;
    let item = doc.items.into_iter().find(|i| i.id == id)?;
    let v = item.versions.into_iter().find(|v| v.n == n)?.verdict?;
    Some((v.verdict, v.text))
}

/// Wait for the verdict of version `n`: 0 and `approved`, or exit 4 with `changes` and what to
/// change. The connection keeps the node open, so the daemon keeps it too.
fn wait_verdict(s: &mut Link, workspace: &str, id: &str, n: u32, timeout: Duration) -> Out {
    let (node, _) = artifacts_of(s, workspace)?;
    let got =
        s.c.wait_until(timeout, |c| {
            c.nodes
                .get(&node)
                .is_some_and(|d| verdict_of(d, id, n).is_some())
        })
        .map_err(|e| Fail::Error(e.to_string()))?;
    if !got {
        return fail(format!("no verdict in {} s", timeout.as_secs()));
    }
    match s.c.nodes.get(&node).and_then(|d| verdict_of(d, id, n)) {
        Some((Verdict::Approved, _)) => {
            println!("approved");
            Ok(())
        }
        Some((Verdict::Changes, text)) => Err(Fail::Changes(text.unwrap_or_default())),
        None => fail("the artifact is gone"),
    }
}

/// The workspace that has the artifact `id` and its item: that of the pane that asks first.
fn find(s: &mut Link, id: &str) -> Out<(String, ArtifactItem)> {
    let mut order: Vec<String> = Vec::new();
    if let Some(pane) = own_pane() {
        order.extend(s.workspace_of(&pane)?.map(|w| w.id));
    }
    for w in s.workspaces()? {
        if !order.contains(&w.id) {
            order.push(w.id);
        }
    }
    for ws in order {
        let (_, doc) = artifacts_of(s, &ws)?;
        if let Some(item) = doc.items.into_iter().find(|i| i.id == id) {
            return Ok((ws, item));
        }
    }
    fail(format!("no artifact {id}"))
}

fn wait(args: &[&str]) -> Out {
    let mut a = Args::parse(args, &["timeout"], &[])?;
    // `wait` always waits: the flag is there so `--timeout` is read as it is for `publish --wait`.
    a.flags.push(("wait".into(), None));
    let [id] = a.words::<1>()?;
    let timeout = timeout_flag(&a)?.unwrap_or(TURN_WAIT);
    let mut s = Link::connect()?;
    let (ws, item) = find(&mut s, id)?;
    let n = item.versions.last().map_or(0, |v| v.n);
    wait_verdict(&mut s, &ws, id, n, timeout)
}

fn rm(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &[])?;
    let [id] = a.words::<1>()?;
    Link::connect()?.cmd(Cmd::ArtifactRm { id: id.to_owned() })?;
    Ok(())
}

fn ls(args: &[&str]) -> Out {
    let a = Args::parse(args, &[], &["json"])?;
    if a.pos.len() > 1 {
        return Err(Fail::Usage("expected at most 1 argument".into()));
    }
    let mut s = Link::connect()?;
    let workspaces = match a.pos.first() {
        Some(word) => vec![s.workspace(word)?],
        None => s.workspaces()?,
    };
    let mut rows = Vec::new();
    for w in workspaces {
        let (_, doc) = artifacts_of(&mut s, &w.id)?;
        for i in doc.items {
            rows.push((w.name.clone(), i));
        }
    }
    if a.has("json") {
        let out: Vec<Value> = rows
            .iter()
            .map(|(ws, i)| {
                json!({
                    "workspace": ws, "id": i.id, "name": i.name, "title": i.title,
                    "kind": name_of(i.kind), "format": name_of(i.format),
                    "version": i.versions.last().map_or(0, |v| v.n), "status": status_of(i),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        for (ws, i) in &rows {
            println!(
                "{ws}\t{}\t{}\t{}\tv{}\t{}\t{}",
                i.id,
                i.name,
                name_of(i.kind),
                i.versions.last().map_or(0, |v| v.n),
                status_of(i),
                i.title
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_and_the_name_come_from_the_file() {
        assert_eq!(
            format_of("docs/Plan-B.HTML").unwrap(),
            (ArtifactFormat::Html, "plan-b".into())
        );
        assert_eq!(format_of("a.htm").unwrap().0, ArtifactFormat::Html);
        assert_eq!(
            format_of("n171.md").unwrap(),
            (ArtifactFormat::Md, "n171".into())
        );
        assert!(matches!(format_of("a.txt"), Err(Fail::Usage(_))));
        assert!(matches!(format_of("noext"), Err(Fail::Usage(_))));
    }

    #[test]
    fn the_version_of_a_publish_must_fit_a_u32() {
        assert_eq!(version_of(&json!({ "n": 3 })).ok(), Some(3));
        assert!(version_of(&json!({ "n": 1u64 << 32 })).is_err());
        assert!(version_of(&json!({})).is_err());
    }

    #[test]
    fn the_title_is_the_page_title_or_the_first_heading_on_one_line() {
        let html = "<!doctype html><HEAD><Title>\n  Ajustes ·\n propuesta B </TITLE></head>";
        assert_eq!(
            title_of(html, ArtifactFormat::Html).as_deref(),
            Some("Ajustes · propuesta B")
        );
        assert_eq!(title_of("<p>no title", ArtifactFormat::Html), None);
        assert_eq!(title_of("<title></title>", ArtifactFormat::Html), None);
        assert_eq!(
            title_of("intro\n## Una  spec\n# otra", ArtifactFormat::Md).as_deref(),
            Some("Una spec")
        );
        assert_eq!(title_of("sin título", ArtifactFormat::Md), None);
        // A multi-byte letter before the tag does not break the cut.
        assert_eq!(
            title_of("É<title>x</title>", ArtifactFormat::Html).as_deref(),
            Some("x")
        );
    }
}
