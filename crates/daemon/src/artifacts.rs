//! Artifacts (N171): the files an agent publishes for a person to look at on the phone, kept per
//! workspace as immutable versions on disk, listed by the `artifacts` node and held as attention
//! while one waits for a verdict (REQ-759 to REQ-768).

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use contract::artifacts::{
    ArtifactDoc, ArtifactFormat, ArtifactItem, ArtifactKind, ArtifactVerdict, ArtifactVersion,
    Delivered, MAX_ARTIFACT_BYTES, Verdict,
};
use contract::{AttentionKind, ErrorCode, Id};
use serde_json::json;

use crate::commands::{Done, err};
use crate::core::Core;
use crate::model::{Own, new_id};
use crate::server::Shared;

/// The versions of an artifact that are kept; the oldest go (REQ-762).
const MAX_VERSIONS: usize = 20;
/// The longest name, in characters, and the longest note of a «changes» verdict, in bytes.
const MAX_NAME: usize = 64;
const MAX_NOTE: usize = 4 * 1024;

/// The `artifacts` node of a workspace the daemon serves, and what it needs to be kept.
pub struct ArtifactsEntry {
    pub workspace: Id,
    pub dirty: bool,
    pub idle_since: Instant,
}

/// What `artifact_publish` brings.
pub struct Publish {
    pub workspace: Id,
    pub name: String,
    pub title: Option<String>,
    pub kind: Option<ArtifactKind>,
    pub format: ArtifactFormat,
    pub text: String,
    pub pane: Option<Id>,
}

// ---- the store: <state>/artifacts/<workspace id>/<artifact id>/{meta.json, v<n>.html|md}

pub fn dir(state: &Path) -> PathBuf {
    state.join("artifacts")
}

fn item_dir(root: &Path, workspace: &str, id: &str) -> PathBuf {
    root.join(workspace).join(id)
}

fn extension(format: ArtifactFormat) -> &'static str {
    match format {
        ArtifactFormat::Html => "html",
        ArtifactFormat::Md => "md",
    }
}

fn version_file(root: &Path, workspace: &str, item: &ArtifactItem, n: u32) -> PathBuf {
    item_dir(root, workspace, &item.id).join(format!("v{n}.{}", extension(item.format)))
}

/// Whole-file write: mode 0600, a temporary file and a rename, as `history` and `sent` do.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

fn save_meta(root: &Path, workspace: &str, item: &ArtifactItem) -> io::Result<()> {
    let bytes = serde_json::to_vec(item).map_err(io::Error::other)?;
    write_private(
        &item_dir(root, workspace, &item.id).join("meta.json"),
        &bytes,
    )
}

/// The artifacts a workspace has on disk. A `meta.json` that cannot be read is moved to
/// `meta.json.bad` and left out, as the other stores do; nothing of its text is logged (REQ-790).
fn load(root: &Path, workspace: &str) -> Vec<ArtifactItem> {
    let Ok(entries) = fs::read_dir(root.join(workspace)) else {
        return Vec::new();
    };
    let mut items: Vec<ArtifactItem> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.path().join("meta.json");
            let bytes = fs::read(&meta).ok()?;
            match serde_json::from_slice::<ArtifactItem>(&bytes) {
                Ok(item) if !item.versions.is_empty() => Some(item),
                _ => {
                    eprintln!(
                        "artifacts: {} is not readable, moved to .bad",
                        meta.display()
                    );
                    let _ = fs::rename(&meta, meta.with_extension("json.bad"));
                    None
                }
            }
        })
        .collect();
    sort_by_name(&mut items);
    items
}

/// The order a workspace's artifacts are listed in, on disk and in memory alike.
fn sort_by_name(items: &mut [ArtifactItem]) {
    items.sort_by(|a, b| a.name.cmp(&b.name));
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A title is one line: it is a column of `jw artifact ls`, a row of the phone's list and the head of a message.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pending(item: &ArtifactItem) -> bool {
    item.versions.last().is_some_and(|v| v.verdict.is_none())
}

// ---- what the core keeps

impl Core {
    /// Start serving what is on disk: the items of every workspace there is, and the attention of
    /// those that wait (REQ-764). Nothing on disk is deleted here: a model that came up empty must
    /// not take the artifacts with it.
    pub fn load_artifacts(&mut self, root: PathBuf) {
        self.artifacts_dir = Some(root.clone());
        let workspaces: Vec<Id> = self.model.workspaces.iter().map(|w| w.id.clone()).collect();
        for ws in workspaces {
            let items = load(&root, &ws);
            if !items.is_empty() {
                self.artifact_items.insert(ws.clone(), items);
            }
            self.sync_artifact_attention(&ws);
        }
    }

    fn find_artifact(&self, id: &str) -> Option<(&Id, &ArtifactItem)> {
        self.artifact_items
            .iter()
            .find_map(|(ws, items)| items.iter().find(|i| i.id == id).map(|i| (ws, i)))
    }

    /// The attention `artifact` is on the workspace while an artifact's latest version has no
    /// verdict, and only then (REQ-764). Nothing but a verdict, a removal or this ends it.
    pub fn sync_artifact_attention(&mut self, workspace: &str) {
        let waits = self
            .artifact_items
            .get(workspace)
            .is_some_and(|items| items.iter().any(pending));
        let mut changed = false;
        if let Some(ws) = self.model.workspaces.iter_mut().find(|w| w.id == workspace) {
            let has = ws
                .attention
                .iter()
                .any(|o| o.kind == AttentionKind::Artifact);
            if waits && !has {
                ws.attention.push(Own {
                    kind: AttentionKind::Artifact,
                    exit_code: None,
                    rev: None,
                });
                changed = true;
            } else if !waits && has {
                ws.attention.retain(|o| o.kind != AttentionKind::Artifact);
                changed = true;
            }
        }
        for e in self
            .artifact_nodes
            .values_mut()
            .filter(|e| e.workspace == workspace)
        {
            e.dirty = true;
            changed = true;
        }
        // Starting with many workspaces and no artifact in most would otherwise rebuild every document for each.
        if changed {
            self.refresh();
        }
    }

    /// A workspace was removed on purpose: its artifacts go with it (REQ-763).
    pub fn forget_artifacts(&mut self, workspace: &str) {
        self.artifact_items.remove(workspace);
        if let Some(root) = &self.artifacts_dir {
            let _ = fs::remove_dir_all(root.join(workspace));
        }
    }

    /// The `artifacts` node of a workspace, made when it is asked for (REQ-765).
    fn artifacts_node(&mut self, workspace: &Id) -> Id {
        if let Some((id, e)) = self
            .artifact_nodes
            .iter_mut()
            .find(|(_, e)| e.workspace == *workspace)
        {
            e.idle_since = Instant::now();
            return id.clone();
        }
        let id = new_id();
        self.artifact_nodes.insert(
            id.clone(),
            ArtifactsEntry {
                workspace: workspace.clone(),
                dirty: true,
                idle_since: Instant::now(),
            },
        );
        self.refresh();
        id
    }

    pub fn artifact_items_of(&self, workspace: &str) -> Vec<ArtifactItem> {
        self.artifact_items
            .get(workspace)
            .cloned()
            .unwrap_or_default()
    }
}

// ---- the commands

fn io_failed(what: &str, e: &io::Error) -> crate::commands::ErrorBody {
    err(ErrorCode::Io, format!("{what}: {e}"))
}

fn root_of(core: &Core) -> Result<PathBuf, crate::commands::ErrorBody> {
    core.artifacts_dir
        .clone()
        .ok_or_else(|| err(ErrorCode::Io, "the daemon has no folder for artifacts"))
}

/// `artifact_publish` (REQ-759 to REQ-762).
pub fn publish(shared: &Shared, p: Publish) -> Done {
    if p.text.len() > MAX_ARTIFACT_BYTES {
        return Err(err(
            ErrorCode::BadRequest,
            format!(
                "too big: {} bytes, the most is {MAX_ARTIFACT_BYTES}",
                p.text.len()
            ),
        ));
    }
    let name = p.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.contains('/') {
        return Err(err(
            ErrorCode::BadRequest,
            format!("bad name: 1 to {MAX_NAME} characters, no `/`"),
        ));
    }
    let hash = web::auth::sha256_hex(&p.text);
    let mut core = shared.lock();
    let root = root_of(&core)?;
    core.workspace(&p.workspace)?;
    let existing = core
        .artifact_items
        .get(&p.workspace)
        .and_then(|items| items.iter().find(|i| i.name == name))
        .cloned();
    let (mut item, is_new) = match existing {
        Some(item) => (item, false),
        None => (
            ArtifactItem {
                id: new_id(),
                name: name.to_owned(),
                title: name.to_owned(),
                kind: p.kind.unwrap_or(ArtifactKind::Document),
                format: p.format,
                versions: Vec::new(),
            },
            true,
        ),
    };
    if !is_new && item.format != p.format {
        return Err(err(
            ErrorCode::BadRequest,
            format!(
                "{name} was published as {}: use another name for the other format",
                extension(item.format)
            ),
        ));
    }
    if let Some(last) = item.versions.last().filter(|v| v.hash == hash) {
        return Ok(Some(
            json!({"id": item.id, "n": last.n, "hash": hash, "new": false}),
        ));
    }
    if let Some(title) = p.title.as_deref().map(one_line).filter(|t| !t.is_empty()) {
        item.title = title;
    }
    if let Some(kind) = p.kind {
        item.kind = kind;
    }
    let n = item.versions.last().map_or(1, |v| v.n + 1);
    item.versions.push(ArtifactVersion {
        n,
        hash: hash.clone(),
        size: p.text.len() as u64,
        at: now(),
        publisher: p.pane,
        verdict: None,
    });
    let extra = item.versions.len().saturating_sub(MAX_VERSIONS);
    let dropped: Vec<u32> = item.versions.drain(..extra).map(|v| v.n).collect();
    // shortcut: the files are written under the core's lock; at most 512 KiB, move it off the lock if it is felt.
    write_private(
        &version_file(&root, &p.workspace, &item, n),
        p.text.as_bytes(),
    )
    .and_then(|()| save_meta(&root, &p.workspace, &item))
    .map_err(|e| io_failed("cannot keep the artifact", &e))?;
    for old in dropped {
        let _ = fs::remove_file(version_file(&root, &p.workspace, &item, old));
    }
    let reply = json!({"id": item.id, "n": n, "hash": hash, "new": true});
    let items = core.artifact_items.entry(p.workspace.clone()).or_default();
    match items.iter_mut().find(|i| i.id == item.id) {
        Some(slot) => *slot = item,
        None => {
            items.push(item);
            sort_by_name(items);
        }
    }
    core.sync_artifact_attention(&p.workspace);
    Ok(Some(reply))
}

/// `open_artifacts` (REQ-765).
pub fn open(shared: &Shared, workspace: &Id) -> Done {
    let mut core = shared.lock();
    core.workspace(workspace)?;
    let node = core.artifacts_node(workspace);
    Ok(Some(json!({ "node": node })))
}

/// `artifact_read` (REQ-766).
pub fn read(shared: &Shared, id: &str, n: Option<u32>) -> Done {
    let (item, version, path) = {
        let core = shared.lock();
        let root = root_of(&core)?;
        let (ws, item) = core
            .find_artifact(id)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no artifact {id}")))?;
        let version = match n {
            Some(n) => item.versions.iter().find(|v| v.n == n),
            None => item.versions.last(),
        }
        .ok_or_else(|| {
            err(
                ErrorCode::NotFound,
                format!("{} has no such version", item.name),
            )
        })?
        .clone();
        let path = version_file(&root, ws, item, version.n);
        (item.clone(), version, path)
    };
    let text = fs::read_to_string(&path).map_err(|e| io_failed("cannot read the artifact", &e))?;
    let (html, doc) = match item.format {
        ArtifactFormat::Html => (Some(text), None),
        ArtifactFormat::Md => (
            None,
            Some(crate::markdown::document(&text, &item.name, None, false)),
        ),
    };
    let doc = ArtifactDoc {
        id: item.id,
        n: version.n,
        hash: version.hash,
        title: item.title,
        kind: item.kind,
        format: item.format,
        html,
        doc,
    };
    let value = serde_json::to_value(&doc).map_err(|e| err(ErrorCode::Io, e.to_string()))?;
    // The reply is one message of the phone's link (REQ-79); a little room is left for its envelope.
    if value.to_string().len() > web::bridge::MAX_MESSAGE - 1024 {
        return Err(err(
            ErrorCode::BadRequest,
            "too big for the link to carry in one message",
        ));
    }
    Ok(Some(value))
}

/// What the agent is told of a verdict: one line that names the version, and the person's words
/// after it (REQ-769). The title is one line whatever it was.
fn verdict_message(
    title: &str,
    n: u32,
    hash: &str,
    verdict: Verdict,
    note: Option<&str>,
) -> String {
    let title = one_line(title);
    let head = format!(
        "[{}] artefacto «{title}» v{n} ({})",
        identity::get().label,
        hash.get(..6).unwrap_or(hash)
    );
    match (verdict, note) {
        (Verdict::Changes, Some(note)) => {
            format!("{head}: CAMBIOS pedidos desde el teléfono:\n{note}")
        }
        (Verdict::Changes, None) => format!("{head}: CAMBIOS pedidos desde el teléfono."),
        (Verdict::Approved, _) => format!("{head}: APROBADO desde el teléfono."),
    }
}

/// Tell the conversation of the pane that published, as `jw prompt` does and privately (not kept
/// in the history of ↑, which `sent::record` is for): only a pane that runs a conversation is
/// told, since text pasted into a plain pane is typed into whatever runs there (RISK-112). The
/// lock is not held: `engine::send` takes it.
fn deliver(shared: &Shared, publisher: Option<&str>, text: &str) -> Delivered {
    let Some(node) = publisher.and_then(|pane| shared.lock().convs.get(pane).map(|c| c.id.clone()))
    else {
        return Delivered::Nobody;
    };
    match crate::engine::send(shared, &node, text, Vec::new()) {
        Ok(_) => Delivered::Agent,
        Err(_) => Delivered::Nobody,
    }
}

/// `artifact_decide` (REQ-767 to REQ-770): the verdict is kept, then the agent is told, then it
/// is kept whether it was.
pub fn decide(
    shared: &Shared,
    id: &str,
    n: u32,
    hash: &str,
    verdict: Verdict,
    text: Option<String>,
) -> Done {
    let (ws, item, note) = {
        let mut core = shared.lock();
        let root = root_of(&core)?;
        let (ws, item) = core
            .find_artifact(id)
            .map(|(ws, i)| (ws.clone(), i.clone()))
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no artifact {id}")))?;
        let last = item
            .versions
            .last()
            .ok_or_else(|| err(ErrorCode::NotFound, "no versions"))?;
        if last.n != n || last.hash != hash {
            return Err(err(
                ErrorCode::Conflict,
                format!("stale: {} is at version {} now", item.name, last.n),
            ));
        }
        if last.verdict.is_some() {
            return Err(err(
                ErrorCode::Conflict,
                "decided: that version already has a verdict",
            ));
        }
        let note = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        match (verdict, &note) {
            (Verdict::Changes, None) => {
                return Err(err(ErrorCode::BadRequest, "empty: say what to change"));
            }
            (Verdict::Changes, Some(t)) if t.len() > MAX_NOTE => {
                return Err(err(
                    ErrorCode::BadRequest,
                    format!("too long: the most is {MAX_NOTE} bytes"),
                ));
            }
            (Verdict::Approved, Some(_)) => {
                return Err(err(ErrorCode::BadRequest, "an approval takes no text"));
            }
            _ => {}
        }
        let mut item = item;
        if let Some(v) = item.versions.last_mut() {
            v.verdict = Some(ArtifactVerdict {
                verdict,
                text: note.clone(),
                at: now(),
                delivered: None,
            });
        }
        save_meta(&root, &ws, &item).map_err(|e| io_failed("cannot keep the verdict", &e))?;
        if let Some(slot) = core
            .artifact_items
            .get_mut(&ws)
            .and_then(|items| items.iter_mut().find(|i| i.id == item.id))
        {
            slot.clone_from(&item);
        }
        core.sync_artifact_attention(&ws);
        (ws, item, note)
    };
    let publisher = item.versions.last().and_then(|v| v.publisher.clone());
    let message = verdict_message(&item.title, n, hash, verdict, note.as_deref());
    let delivered = deliver(shared, publisher.as_deref(), &message);
    // Saved twice on purpose: the verdict first (above), so it survives a crash before delivery,
    // then again with `delivered` set. The second save is best effort: a lost mark only shows
    // the verdict as undelivered.
    // The artifact may have been removed meanwhile: then there is nothing to mark.
    let mut core = shared.lock();
    if let Some(root) = core.artifacts_dir.clone() {
        let marked = core
            .artifact_items
            .get_mut(&ws)
            .and_then(|items| items.iter_mut().find(|i| i.id == item.id))
            .and_then(|i| i.versions.iter_mut().find(|v| v.n == n))
            .and_then(|v| v.verdict.as_mut())
            .map(|v| v.delivered = Some(delivered))
            .is_some();
        if marked {
            if let Some(item) = core
                .artifact_items
                .get(&ws)
                .and_then(|items| items.iter().find(|i| i.id == item.id))
            {
                let _ = save_meta(&root, &ws, item);
            }
            core.sync_artifact_attention(&ws);
        }
    }
    Ok(Some(json!({ "delivered": delivered })))
}

/// `artifact_rm` (REQ-763).
pub fn rm(shared: &Shared, id: &str) -> Done {
    let mut core = shared.lock();
    let root = root_of(&core)?;
    let ws = core
        .find_artifact(id)
        .map(|(ws, _)| ws.clone())
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no artifact {id}")))?;
    fs::remove_dir_all(item_dir(&root, &ws, id))
        .map_err(|e| io_failed("cannot remove the artifact", &e))?;
    if let Some(items) = core.artifact_items.get_mut(&ws) {
        items.retain(|i| i.id != id);
    }
    core.sync_artifact_attention(&ws);
    Ok(None)
}
