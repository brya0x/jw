//! The hub: the model, the node documents built from it, and who is watching which.
//! Every change goes through `refresh`, which rebuilds the documents, bumps the revision
//! of each one that changed and sends a patch to whoever has it open.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Sender, SyncSender, TrySendError};

use contract::patch::diff;
use contract::{
    Attention, DaemonMsg, ErrorCode, Head, Id, Node, PaneSummary, PatchOp, Screen, Session,
    Workspace, WorkspaceSummary,
};
use serde_json::Value;

use crate::model::{Model, Own};
use crate::persist;
use crate::pty::{self, Event, Term};

/// The node document versions this daemon can serve.
pub const SUPPORTED: [(&str, &[u32]); 4] = [
    ("session", &[1]),
    ("workspace", &[1]),
    ("pane", &[1]),
    ("blocks", &[1]),
];

/// How many patches per node are kept for a resuming client.
pub const RETAIN: usize = 256;

/// Messages waiting for one client; a client that falls this far behind is dropped.
pub const OUTBOX: usize = 4096;

struct PatchRec {
    from: u64,
    to: u64,
    ops: Vec<PatchOp>,
}

struct Doc {
    value: Value,
    rev: u64,
    history: VecDeque<PatchRec>,
}

struct ClientState {
    tx: SyncSender<DaemonMsg>,
    /// To cut a client that cannot keep up.
    stream: wire::Stream,
    /// Set by `hello`: node type -> the version served to this client.
    accepted: Option<BTreeMap<String, u32>>,
    open: HashSet<Id>,
}

pub struct Core {
    pub epoch: String,
    pub model: Model,
    save_path: Option<PathBuf>,
    last_saved: String,
    docs: HashMap<Id, Doc>,
    clients: HashMap<u64, ClientState>,
    next_client: u64,
    /// The running shells, by pane id, and the last screen read from each.
    terms: HashMap<Id, Arc<Term>>,
    screens: HashMap<Id, Screen>,
    /// The last blocks read from each shell (with their version), and the id of the node
    /// each pane's blocks are served as.
    blocks_views: HashMap<Id, (u64, contract::BlocksMode, Vec<contract::Block>)>,
    blocks_ids: HashMap<Id, Id>,
    /// Where terminals report to, and the shell they start.
    pub events: Sender<Event>,
    pub shell: String,
    /// Extra environment for every pane's shell.
    pub env: Vec<(String, String)>,
    /// Where the shell integration scripts are installed; none turns blocks off.
    pub integration: Option<PathBuf>,
}

impl Core {
    pub fn new(
        model: Model,
        save_path: Option<PathBuf>,
        events: Sender<Event>,
        shell: String,
        env: Vec<(String, String)>,
        integration: Option<PathBuf>,
    ) -> Core {
        let mut core = Core {
            epoch: crate::model::new_id(),
            model,
            save_path,
            last_saved: String::new(),
            docs: HashMap::new(),
            clients: HashMap::new(),
            next_client: 1,
            terms: HashMap::new(),
            screens: HashMap::new(),
            blocks_views: HashMap::new(),
            blocks_ids: HashMap::new(),
            events,
            shell,
            env,
            integration,
        };
        core.refresh();
        core
    }

    /// Every running terminal, for a look at what is in its foreground.
    pub fn terms_snapshot(&self) -> Vec<(Id, Arc<Term>)> {
        self.terms
            .iter()
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect()
    }

    /// Record the programs found in the panes; true when any changed. A program that ended
    /// becomes the pane's `last_program`, and the title follows what runs.
    pub fn set_running(&mut self, updates: &HashMap<Id, Option<Vec<String>>>) -> bool {
        let mut changed = false;
        for p in self
            .model
            .workspaces
            .iter_mut()
            .flat_map(|w| w.panes.iter_mut())
        {
            let Some(new) = updates.get(&p.id) else {
                continue;
            };
            if p.running == *new {
                continue;
            }
            if new.is_none() {
                p.last_program = p.running.take();
            }
            // An agent with a known conversation is what a restart brings back.
            p.resume = new.as_deref().and_then(crate::launch::agent_resume);
            p.title = new
                .as_deref()
                .and_then(crate::procs::title_for)
                .unwrap_or_else(|| crate::panes::basename(&p.shell));
            p.running = new.clone();
            changed = true;
        }
        changed
    }

    /// How `shell` is started with the integration (nothing when blocks are off).
    pub fn integration_for(&self, shell: &str) -> crate::integration::Setup {
        let Some(dir) = &self.integration else {
            return Default::default();
        };
        let lookup = |k: &str| {
            self.env
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var(k).ok())
        };
        crate::integration::setup(shell, dir, &lookup)
    }

    pub fn term(&self, pane: &str) -> Option<Arc<Term>> {
        self.terms.get(pane).cloned()
    }

    pub fn attach_term(&mut self, pane: &Id, term: Arc<Term>) {
        self.screens.insert(pane.clone(), term.screen());
        self.terms.insert(pane.clone(), term);
    }

    /// Re-read the screen of a pane whose terminal changed.
    pub fn read_screen(&mut self, pane: &str) {
        if let Some(term) = self.terms.get(pane) {
            self.screens.insert(pane.to_owned(), term.screen());
        }
    }

    /// Re-read the blocks of a pane whose terminal changed them. A shell started with the
    /// integration has a blocks node of its own.
    pub fn read_blocks(&mut self, pane: &str) {
        let Some(term) = self.terms.get(pane) else {
            return;
        };
        let seen = term.blocks_version();
        if self
            .blocks_views
            .get(pane)
            .is_some_and(|(v, _, _)| *v == seen)
        {
            return;
        }
        if let Some((version, mode, blocks)) = term.blocks_view() {
            self.blocks_ids
                .entry(pane.to_owned())
                .or_insert_with(crate::model::new_id);
            self.blocks_views
                .insert(pane.to_owned(), (version, mode, blocks));
        }
    }

    /// Forget terminals whose pane is no longer in the model, and end their shells.
    fn drop_orphan_terms(&mut self) -> Vec<Arc<Term>> {
        let live: HashSet<&str> = self
            .model
            .workspaces
            .iter()
            .flat_map(|w| w.panes.iter().map(|p| p.id.as_str()))
            .collect();
        let dead: Vec<Id> = self
            .terms
            .keys()
            .filter(|k| !live.contains(k.as_str()))
            .cloned()
            .collect();
        self.screens.retain(|k, _| live.contains(k.as_str()));
        self.blocks_views.retain(|k, _| live.contains(k.as_str()));
        self.blocks_ids.retain(|k, _| live.contains(k.as_str()));
        dead.iter().filter_map(|k| self.terms.remove(k)).collect()
    }

    /// End every shell (the daemon is stopping).
    pub fn shutdown(&mut self) {
        for term in self.terms.drain().map(|(_, t)| t) {
            term.kill();
        }
    }

    pub fn add_client(&mut self, tx: SyncSender<DaemonMsg>, stream: wire::Stream) -> u64 {
        let id = self.next_client;
        self.next_client += 1;
        self.clients.insert(
            id,
            ClientState {
                tx,
                stream,
                accepted: None,
                open: HashSet::new(),
            },
        );
        id
    }

    pub fn remove_client(&mut self, id: u64) {
        self.clients.remove(&id);
    }

    pub fn has_said_hello(&self, client: u64) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|c| c.accepted.is_some())
    }

    /// Queue a message; a client whose outbox is full or closed is cut off.
    pub fn send(&mut self, client: u64, msg: DaemonMsg) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        match c.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                let _ = c.stream.shutdown();
                self.clients.remove(&client);
            }
        }
    }

    pub fn error(&mut self, client: u64, code: ErrorCode, message: &str) {
        self.send(
            client,
            DaemonMsg::Error {
                code,
                message: message.into(),
            },
        );
    }

    /// Answer a client's `hello`: for each node type, the highest version in common.
    pub fn hello(&mut self, client: u64, accepts: &BTreeMap<String, Vec<u32>>) {
        let accepted: BTreeMap<String, u32> = SUPPORTED
            .iter()
            .filter_map(|(ty, served)| {
                let best = served
                    .iter()
                    .filter(|v| accepts.get(*ty).is_some_and(|a| a.contains(v)))
                    .max()?;
                Some(((*ty).to_owned(), *best))
            })
            .collect();
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        c.accepted = Some(accepted.clone());
        let hello = DaemonMsg::Hello {
            daemon: contract::Peer {
                name: identity::get().name.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            epoch: self.epoch.clone(),
            root: self.model.session_id.clone(),
            accepted,
        };
        self.send(client, hello);
    }

    pub fn open(&mut self, client: u64, node: &str, epoch: Option<&str>, since: Option<u64>) {
        let Some(doc) = self.docs.get(node) else {
            self.send(client, DaemonMsg::Gone { node: node.into() });
            return;
        };
        let ty = doc.value["type"].as_str().unwrap_or_default().to_owned();
        let served = doc.value["v"].as_u64().map(|v| v as u32);
        let reply = match self
            .clients
            .get(&client)
            .and_then(|c| c.accepted.as_ref()?.get(&ty))
        {
            Some(v) if Some(*v) == served => self.resume(doc, epoch, since),
            _ => {
                self.error(
                    client,
                    ErrorCode::Unsupported,
                    &format!("no common version of {ty}"),
                );
                return;
            }
        };
        if let Some(c) = self.clients.get_mut(&client) {
            c.open.insert(node.to_owned());
        }
        self.send(client, reply);
    }

    /// One patch for a client that holds an older revision of this epoch, else a snapshot.
    fn resume(&self, doc: &Doc, epoch: Option<&str>, since: Option<u64>) -> DaemonMsg {
        let node_id = doc.value["id"].as_str().unwrap_or_default().to_owned();
        if let (Some(e), Some(since)) = (epoch, since)
            && e == self.epoch
        {
            if since == doc.rev {
                return DaemonMsg::Patch {
                    node: node_id,
                    epoch: self.epoch.clone(),
                    from: since,
                    to: since,
                    ops: vec![],
                };
            }
            let tail: Vec<&PatchRec> = doc.history.iter().filter(|p| p.from >= since).collect();
            let contiguous = tail.first().is_some_and(|p| p.from == since)
                && tail.windows(2).all(|w| w[0].to == w[1].from)
                && tail.last().is_some_and(|p| p.to == doc.rev);
            if contiguous {
                let ops = tail.iter().flat_map(|p| p.ops.iter().cloned()).collect();
                return DaemonMsg::Patch {
                    node: node_id,
                    epoch: self.epoch.clone(),
                    from: since,
                    to: doc.rev,
                    ops,
                };
            }
        }
        let node: Node = serde_json::from_value(doc.value.clone()).expect("documents are nodes");
        DaemonMsg::Snapshot { node }
    }

    pub fn close(&mut self, client: u64, node: &str) {
        if let Some(c) = self.clients.get_mut(&client) {
            c.open.remove(node);
        }
    }

    /// Rebuild every document from the model; patch what changed; save the model.
    pub fn refresh(&mut self) {
        self.assign_attention_revs();
        let built = self.build();
        let mut seen = HashSet::new();
        let mut patches: Vec<(Id, u64, u64, Vec<PatchOp>)> = Vec::new();
        for node in built {
            let mut value = serde_json::to_value(&node).expect("nodes serialise");
            let id = contract::node_id(&node).clone();
            seen.insert(id.clone());
            match self.docs.get_mut(&id) {
                None => {
                    value["rev"] = 1.into();
                    self.docs.insert(
                        id,
                        Doc {
                            value,
                            rev: 1,
                            history: VecDeque::new(),
                        },
                    );
                }
                Some(doc) => {
                    // `build` wrote the old revision, so a changed document differs in content only.
                    let ops = diff(&doc.value, &value);
                    if ops.is_empty() {
                        continue;
                    }
                    let (from, to) = (doc.rev, doc.rev + 1);
                    value["rev"] = to.into();
                    doc.value = value;
                    doc.rev = to;
                    doc.history.push_back(PatchRec {
                        from,
                        to,
                        ops: ops.clone(),
                    });
                    if doc.history.len() > RETAIN {
                        doc.history.pop_front();
                    }
                    patches.push((id, from, to, ops));
                }
            }
        }
        let epoch = self.epoch.clone();
        for (node, from, to, ops) in patches {
            self.broadcast(
                &node,
                DaemonMsg::Patch {
                    node: node.clone(),
                    epoch: epoch.clone(),
                    from,
                    to,
                    ops,
                },
            );
        }
        let gone: Vec<Id> = self
            .docs
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for node in gone {
            self.docs.remove(&node);
            self.broadcast(&node, DaemonMsg::Gone { node: node.clone() });
            for c in self.clients.values_mut() {
                c.open.remove(&node);
            }
        }
        self.save();
        for term in self.drop_orphan_terms() {
            term.kill();
        }
    }

    fn broadcast(&mut self, node: &str, msg: DaemonMsg) {
        let watchers: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.open.contains(node))
            .map(|(id, _)| *id)
            .collect();
        for w in watchers {
            self.send(w, msg.clone());
        }
    }

    fn save(&mut self) {
        let Some(path) = &self.save_path else { return };
        let text = persist::to_text(&self.model);
        if text != self.last_saved {
            match persist::save_text(path, &text) {
                Ok(()) => self.last_saved = text,
                Err(e) => eprintln!("cannot save {}: {e}", path.display()),
            }
        }
    }

    /// A new attention shows up in the next revision of its node; record which one.
    fn assign_attention_revs(&mut self) {
        let next = |docs: &HashMap<Id, Doc>, id: &str| docs.get(id).map_or(0, |d| d.rev) + 1;
        let docs = &self.docs;
        for ws in &mut self.model.workspaces {
            let rev = next(docs, &ws.id);
            ws.attention
                .iter_mut()
                .filter(|o| o.rev.is_none())
                .for_each(|o| o.rev = Some(rev));
            for p in &mut ws.panes {
                let rev = next(docs, &p.id);
                p.attention
                    .iter_mut()
                    .filter(|o| o.rev.is_none())
                    .for_each(|o| o.rev = Some(rev));
            }
        }
    }

    fn build(&self) -> Vec<Node> {
        let rev = |id: &str| self.docs.get(id).map_or(0, |d| d.rev);
        let head = |id: &Id, attention: Vec<Attention>| Head {
            id: id.clone(),
            v: 1,
            epoch: self.epoch.clone(),
            rev: rev(id),
            attention,
        };
        let mut nodes = Vec::new();
        let mut summaries = Vec::new();
        let mut session_attention = Vec::new();
        for ws in &self.model.workspaces {
            let mut attention = raised(&ws.attention, &ws.id);
            let mut panes = Vec::new();
            for p in &ws.panes {
                let own = raised(&p.attention, &p.id);
                attention.extend(own.iter().cloned());
                nodes.push(Node::Pane(contract::Pane {
                    head: head(&p.id, own.clone()),
                    workspace: ws.id.clone(),
                    title: p.title.clone(),
                    shell: p.shell.clone(),
                    cwd: p.cwd.clone(),
                    running: p.running.clone().map(|argv| contract::Program {
                        argv,
                        exit_code: None,
                    }),
                    last_program: p.last_program.clone().map(|argv| contract::Program {
                        argv,
                        exit_code: None,
                    }),
                    blocks: self
                        .blocks_ids
                        .get(&p.id)
                        .filter(|_| self.blocks_views.contains_key(&p.id))
                        .cloned(),
                    screen: self
                        .screens
                        .get(&p.id)
                        .cloned()
                        .unwrap_or_else(pty::blank_screen),
                }));
                if let (Some(id), Some((_, mode, blocks))) =
                    (self.blocks_ids.get(&p.id), self.blocks_views.get(&p.id))
                {
                    nodes.push(Node::Blocks(contract::Blocks {
                        head: head(id, Vec::new()),
                        pane: p.id.clone(),
                        mode: if p.running.as_deref().is_some_and(crate::procs::takes_screen) {
                            contract::BlocksMode::Full
                        } else {
                            *mode
                        },
                        blocks: blocks.clone(),
                    }));
                }
                panes.push(PaneSummary {
                    id: p.id.clone(),
                    title: p.title.clone(),
                    attention: own,
                });
            }
            session_attention.extend(attention.iter().cloned());
            summaries.push(WorkspaceSummary {
                id: ws.id.clone(),
                name: ws.name.clone(),
                kind: ws.kind,
                path: ws.path.clone(),
                branch: ws.branch.clone(),
                pane_count: ws.panes.len() as u32,
                attention: attention.clone(),
            });
            nodes.push(Node::Workspace(Workspace {
                head: head(&ws.id, attention),
                name: ws.name.clone(),
                kind: ws.kind,
                path: ws.path.clone(),
                project: ws.project.clone(),
                branch: ws.branch.clone(),
                panes,
            }));
        }
        nodes.push(Node::Session(Session {
            head: head(&self.model.session_id, session_attention),
            name: self.model.name.clone(),
            workspaces: summaries,
        }));
        nodes
    }
}

fn raised(own: &[Own], node: &Id) -> Vec<Attention> {
    own.iter()
        .map(|o| Attention {
            kind: o.kind,
            node: node.clone(),
            rev: o.rev.unwrap_or(0),
            exit_code: o.exit_code,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use contract::{AttentionKind, WorkspaceKind};

    use super::*;
    use crate::model::{Pane, Workspace, new_id};

    fn test_core(model: Model) -> Core {
        Core::new(
            model,
            None,
            std::sync::mpsc::channel().0,
            "/bin/sh".into(),
            Vec::new(),
            None,
        )
    }

    fn model_with_pane() -> (Model, Id, Id) {
        let mut model = Model::new();
        let (ws, pane) = (new_id(), new_id());
        model.workspaces.push(Workspace {
            id: ws.clone(),
            name: "w".into(),
            kind: WorkspaceKind::Folder,
            path: "/tmp".into(),
            project: None,
            branch: None,
            panes: vec![Pane {
                id: pane.clone(),
                title: "sh".into(),
                shell: "/bin/sh".into(),
                cwd: "/tmp".into(),
                attention: vec![],
                running: None,
                last_program: None,
                resume: None,
            }],
            attention: vec![],
        });
        (model, ws, pane)
    }

    fn attention_of(core: &Core, id: &str) -> Vec<Value> {
        core.docs[id].value["attention"].as_array().unwrap().clone()
    }

    #[test]
    fn attention_bubbles_to_every_ancestor_once_computed_by_the_daemon() {
        let (model, ws, pane) = model_with_pane();
        let mut core = test_core(model);
        let session = core.model.session_id.clone();
        assert!(attention_of(&core, &session).is_empty());

        core.model.workspaces[0].panes[0].attention.push(Own {
            kind: AttentionKind::ShellDone,
            exit_code: Some(2),
            rev: None,
        });
        core.refresh();
        for node in [&session, &ws] {
            let a = attention_of(&core, node);
            assert_eq!(a.len(), 1, "{node}");
            assert_eq!(a[0]["kind"], "shell_done");
            assert_eq!(
                a[0]["node"],
                pane.as_str(),
                "it opens the pane, wherever it bubbled to"
            );
            assert_eq!(a[0]["exit_code"], 2);
        }
        // Raised in the revision the pane's own node got it (the pane was at 1).
        assert_eq!(attention_of(&core, &session)[0]["rev"], 2);
        assert_eq!(core.docs[&pane].rev, 2);
        assert_eq!(core.docs[&session].rev, 2);
        // The workspace summary in the session carries it too.
        assert_eq!(
            core.docs[&session].value["workspaces"][0]["attention"][0]["kind"],
            "shell_done"
        );

        core.model.workspaces[0].panes[0].attention.clear();
        core.refresh();
        assert!(attention_of(&core, &session).is_empty());
        assert_eq!(core.docs[&session].rev, 3);
    }

    #[test]
    fn an_unchanged_model_bumps_nothing() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let before = core.docs[&ws].rev;
        core.refresh();
        core.refresh();
        assert_eq!(core.docs[&ws].rev, before);
    }

    #[test]
    fn history_is_bounded() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        for i in 0..RETAIN + 10 {
            core.model.workspaces[0].name = format!("n{i}");
            core.refresh();
        }
        assert_eq!(core.docs[&ws].history.len(), RETAIN);
    }
}
