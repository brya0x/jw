//! The hub: the model, the node documents built from it, and who is watching which.
//! Every change goes through `refresh`, which rebuilds the documents, bumps the revision
//! of each one that changed and sends a patch to whoever has it open.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use contract::patch::diff;
use contract::{
    Attention, AttentionKind, DaemonMsg, ErrorCode, Head, Id, Node, PaneInput, PaneSummary,
    PatchOp, Screen, Session, Workspace, WorkspaceKind, WorkspaceSummary,
};
use serde_json::Value;

use crate::browse::is_git;
use crate::diff::{self, Scope};
use crate::model::{Model, Own};
use crate::persist;
use crate::pty::{self, Event, Term};

/// The node document versions this daemon can serve.
pub const SUPPORTED: [(&str, &[u32]); 6] = [
    ("session", &[1]),
    ("workspace", &[1]),
    ("pane", &[1]),
    ("blocks", &[1]),
    ("diff", &[1]),
    ("commits", &[1]),
];

/// How long a diff or commits node nobody has open is kept (REQ-102).
const IDLE: Duration = Duration::from_secs(60);

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

/// What a `diff` node is built from: the last read of one workspace and scope.
struct DiffEntry {
    workspace: Id,
    scope: Scope,
    read: diff::Read,
    /// The document must be built again (its read or its marks changed).
    dirty: bool,
    /// The last moment a client was seen to have it open, or when it was made.
    idle_since: Instant,
}

struct CommitsEntry {
    workspace: Id,
    list: diff::CommitList,
    dirty: bool,
    idle_since: Instant,
}

/// A watched node to read again, off the lock.
pub struct DiffJob {
    pub node: Id,
    pub dir: PathBuf,
    pub scope: Scope,
}

/// What `refresh_diff` has to read again.
pub enum Reread {
    Diff(Scope),
    Commits,
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
    /// What each pane's shell holds at its prompt, for the panes that have said where it starts.
    inputs: HashMap<Id, PaneInput>,
    /// The last blocks read from each shell (with their version), and the id of the node
    /// each pane's blocks are served as.
    blocks_views: HashMap<Id, (u64, contract::BlocksMode, Vec<contract::Block>)>,
    blocks_ids: HashMap<Id, Id>,
    /// The `diff` and `commits` nodes in use, by node id.
    diffs: HashMap<Id, DiffEntry>,
    commit_lists: HashMap<Id, CommitsEntry>,
    /// Where terminals report to, and the shell they start.
    pub events: Sender<Event>,
    pub shell: String,
    /// Extra environment for every pane's shell.
    pub env: Vec<(String, String)>,
    /// Where the shell integration scripts are installed; none turns blocks off.
    pub integration: Option<PathBuf>,
    /// The phone link: it is used without this lock held, since it waits.
    pub mobile: Arc<crate::mobile::Hub>,
    /// The settings file an agent is started with, so its hooks tell this daemon what it does.
    pub hooks: Option<PathBuf>,
}

impl Core {
    pub fn new(
        model: Model,
        save_path: Option<PathBuf>,
        events: Sender<Event>,
        shell: String,
        env: Vec<(String, String)>,
        integration: Option<PathBuf>,
        mobile: Arc<crate::mobile::Hub>,
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
            inputs: HashMap::new(),
            blocks_views: HashMap::new(),
            blocks_ids: HashMap::new(),
            diffs: HashMap::new(),
            commit_lists: HashMap::new(),
            events,
            shell,
            env,
            integration,
            mobile,
            hooks: None,
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
                // An agent that is gone has nothing left to ask.
                if p.running.as_deref().is_some_and(crate::launch::is_agent) {
                    p.attention
                        .retain(|o| !crate::agents::KINDS.contains(&o.kind));
                }
                p.last_program = p.running.take();
            }
            // An agent with a known conversation is what a restart brings back.
            p.resume = new.as_deref().and_then(crate::launch::agent_resume);
            p.title = p.name.clone().unwrap_or_else(|| {
                new.as_deref()
                    .and_then(crate::procs::title_for)
                    .unwrap_or_else(|| crate::panes::basename(&p.shell))
            });
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
        self.set_input(pane, term.input());
        self.terms.insert(pane.clone(), term);
    }

    /// Re-read the screen of a pane whose terminal changed.
    pub fn read_screen(&mut self, pane: &str) {
        if let Some(term) = self.terms.get(pane) {
            self.screens.insert(pane.to_owned(), term.screen());
            let input = term.input();
            self.set_input(pane, input);
        }
    }

    fn set_input(&mut self, pane: &str, input: Option<PaneInput>) {
        match input {
            Some(i) => self.inputs.insert(pane.to_owned(), i),
            None => self.inputs.remove(pane),
        };
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

    /// The workspace with this id, or the `not_found` a command answers with.
    pub fn workspace(
        &self,
        id: &str,
    ) -> Result<&crate::model::Workspace, crate::commands::ErrorBody> {
        self.model
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| crate::commands::err(ErrorCode::NotFound, format!("no workspace {id}")))
    }

    pub fn workspace_dir(&self, workspace: &str) -> Option<PathBuf> {
        self.model
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .map(|w| PathBuf::from(&w.path))
    }

    /// The `diff` node of a workspace and scope, if one is in use.
    pub fn find_diff(&mut self, workspace: &str, scope: &Scope) -> Option<Id> {
        self.diffs
            .iter_mut()
            .find(|(_, e)| e.workspace == workspace && e.scope == *scope)
            .map(|(id, e)| {
                e.idle_since = Instant::now();
                id.clone()
            })
    }

    /// Make the `diff` node for what was just read, unless another client made it meanwhile.
    pub fn add_diff(&mut self, workspace: &Id, scope: Scope, read: diff::Read) -> Id {
        if let Some(id) = self.find_diff(workspace, &scope) {
            return id;
        }
        self.prune_viewed(workspace, &scope, &read);
        let id = crate::model::new_id();
        self.diffs.insert(
            id.clone(),
            DiffEntry {
                workspace: workspace.clone(),
                scope,
                read,
                dirty: true,
                idle_since: Instant::now(),
            },
        );
        self.refresh();
        id
    }

    pub fn find_commits(&mut self, workspace: &str) -> Option<Id> {
        self.commit_lists
            .iter_mut()
            .find(|(_, e)| e.workspace == workspace)
            .map(|(id, e)| {
                e.idle_since = Instant::now();
                id.clone()
            })
    }

    pub fn add_commits(&mut self, workspace: &Id, list: diff::CommitList) -> Id {
        if let Some(id) = self.find_commits(workspace) {
            return id;
        }
        let id = crate::model::new_id();
        self.commit_lists.insert(
            id.clone(),
            CommitsEntry {
                workspace: workspace.clone(),
                list,
                dirty: true,
                idle_since: Instant::now(),
            },
        );
        self.refresh();
        id
    }

    /// Where and how to read a `diff` or `commits` node again.
    pub fn reread_target(&self, node: &str) -> Option<(PathBuf, Reread)> {
        if let Some(e) = self.diffs.get(node) {
            return Some((
                self.workspace_dir(&e.workspace)?,
                Reread::Diff(e.scope.clone()),
            ));
        }
        let e = self.commit_lists.get(node)?;
        Some((self.workspace_dir(&e.workspace)?, Reread::Commits))
    }

    /// A node was read again: patch it when what was read differs.
    pub fn diff_update(&mut self, node: &str, read: diff::Read) {
        let Some(e) = self.diffs.get(node).filter(|e| e.read != read) else {
            return;
        };
        let (workspace, scope) = (e.workspace.clone(), e.scope.clone());
        self.prune_viewed(&workspace, &scope, &read);
        if let Some(e) = self.diffs.get_mut(node) {
            e.read = read;
            e.dirty = true;
        }
        self.refresh();
    }

    pub fn commits_update(&mut self, node: &str, list: diff::CommitList) {
        let Some(e) = self.commit_lists.get_mut(node).filter(|e| e.list != list) else {
            return;
        };
        e.list = list;
        e.dirty = true;
        self.refresh();
    }

    /// Marks of files that left an `uncommitted` or `general` diff are dropped (REQ-101); a
    /// commit's are kept, and a read that is not the whole diff proves nothing.
    fn prune_viewed(&mut self, workspace: &str, scope: &Scope, read: &diff::Read) {
        if matches!(scope, Scope::Commit(_))
            || read.too_large
            || read.state != contract::DiffState::Ok
        {
            return;
        }
        let key = viewed_key(workspace, scope);
        if let Some(marks) = self.model.viewed.get_mut(&key) {
            marks.retain(|path, _| read.files.iter().any(|f| f.path == *path));
            if marks.is_empty() {
                self.model.viewed.remove(&key);
            }
        }
    }

    /// The worktrees a person is working in (a branch and a pane): what GitHub says of their
    /// pull requests is looked up for these only.
    pub fn github_jobs(&self) -> Vec<crate::github::Job> {
        self.model
            .workspaces
            .iter()
            .filter(|w| !w.panes.is_empty())
            .filter_map(|w| {
                Some(crate::github::Job {
                    workspace: w.id.clone(),
                    dir: PathBuf::from(&w.path),
                    branch: w.branch.clone()?,
                })
            })
            .collect()
    }

    /// A pull request was looked up: raise `merged` or `ci_failed` on the workspace when what is
    /// seen differs from last time, and end what was raised before. Nothing changing raises
    /// nothing, so an attention a person has seen does not come back.
    pub fn github_apply(&mut self, workspace: &str, seen: Option<crate::github::Pr>) {
        let Some(ws) = self
            .model
            .workspaces
            .iter_mut()
            .find(|w| w.id == workspace)
            .filter(|w| w.github != seen)
        else {
            return;
        };
        ws.attention
            .retain(|o| !matches!(o.kind, AttentionKind::Merged | AttentionKind::CiFailed));
        let kind = seen.as_ref().and_then(|pr| match (pr.merged, pr.failing) {
            (true, _) => Some(AttentionKind::Merged),
            (_, true) => Some(AttentionKind::CiFailed),
            _ => None,
        });
        if let Some(kind) = kind {
            ws.attention.push(Own {
                kind,
                exit_code: None,
                rev: None,
            });
        }
        ws.github = seen;
        self.refresh();
    }

    /// Record that `path` of a `diff` node was looked at, or forget it (REQ-100).
    pub fn set_viewed(
        &mut self,
        node: &str,
        path: &str,
        viewed: bool,
    ) -> Result<(), (ErrorCode, String)> {
        let e = self
            .diffs
            .get_mut(node)
            .ok_or_else(|| (ErrorCode::NotFound, format!("no diff node {node}")))?;
        if e.read.too_large {
            return Err((
                ErrorCode::Unsupported,
                "a diff over the limit cannot be marked".into(),
            ));
        }
        let hash = e
            .read
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.hash.clone())
            .ok_or_else(|| (ErrorCode::NotFound, format!("no file {path} in {node}")))?;
        e.dirty = true;
        let key = viewed_key(&e.workspace, &e.scope);
        if viewed {
            self.model
                .viewed
                .entry(key)
                .or_default()
                .insert(path.to_owned(), hash);
        } else if let Some(marks) = self.model.viewed.get_mut(&key) {
            marks.remove(path);
            if marks.is_empty() {
                self.model.viewed.remove(&key);
            }
        }
        self.refresh();
        Ok(())
    }

    /// The `uncommitted` nodes a client has open, to be read again by the poll. Nodes nobody
    /// has had open for `IDLE` go (REQ-102).
    pub fn diff_jobs(&mut self) -> Vec<DiffJob> {
        let now = Instant::now();
        let watched: HashSet<Id> = self
            .clients
            .values()
            .flat_map(|c| c.open.iter().cloned())
            .collect();
        for (id, e) in &mut self.diffs {
            if watched.contains(id) {
                e.idle_since = now;
            }
        }
        for (id, e) in &mut self.commit_lists {
            if watched.contains(id) {
                e.idle_since = now;
            }
        }
        let before = self.diffs.len() + self.commit_lists.len();
        self.diffs
            .retain(|_, e| now.duration_since(e.idle_since) < IDLE);
        self.commit_lists
            .retain(|_, e| now.duration_since(e.idle_since) < IDLE);
        if self.diffs.len() + self.commit_lists.len() != before {
            self.refresh();
        }
        self.diffs
            .iter()
            .filter(|(id, e)| e.scope == Scope::Uncommitted && watched.contains(*id))
            .filter_map(|(id, e)| {
                Some(DiffJob {
                    node: id.clone(),
                    dir: self.workspace_dir(&e.workspace)?,
                    scope: e.scope.clone(),
                })
            })
            .collect()
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
        self.inputs.retain(|k, _| live.contains(k.as_str()));
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
        for ws in &mut self.model.workspaces {
            let ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
            if !crate::layout::agrees(&ws.layout, &ids) {
                ws.layout = crate::layout::reconcile(ws.layout.take(), &ids);
            }
        }
        self.assign_attention_revs();
        let live: HashSet<Id> = self.model.workspaces.iter().map(|w| w.id.clone()).collect();
        self.diffs.retain(|_, e| live.contains(&e.workspace));
        self.commit_lists.retain(|_, e| live.contains(&e.workspace));
        self.model
            .viewed
            .retain(|key, _| key.split('|').next().is_some_and(|w| live.contains(w)));
        let (built, kept) = self.build();
        let mut seen: HashSet<Id> = kept.into_iter().collect();
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
        self.diffs.values_mut().for_each(|e| e.dirty = false);
        self.commit_lists.values_mut().for_each(|e| e.dirty = false);
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

    /// Every node document, and the ids of the diff and commits nodes whose documents are
    /// still current and so were not built again.
    fn build(&self) -> (Vec<Node>, Vec<Id>) {
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
                    input: self.inputs.get(&p.id).cloned().map(Box::new),
                }));
                if let (Some(id), Some((_, mode, blocks))) =
                    (self.blocks_ids.get(&p.id), self.blocks_views.get(&p.id))
                {
                    nodes.push(Node::Blocks(contract::Blocks {
                        head: head(id, Vec::new()),
                        pane: p.id.clone(),
                        mode: crate::procs::mode_of(*mode, blocks, p.running.as_deref()),
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
            // A folder that is a repository's root is its own project; a worktree's is its repository.
            let project = ws.project.clone().or_else(|| {
                (ws.kind == WorkspaceKind::Folder && is_git(Path::new(&ws.path)))
                    .then(|| ws.path.clone())
            });
            summaries.push(WorkspaceSummary {
                id: ws.id.clone(),
                name: ws.name.clone(),
                kind: ws.kind,
                path: ws.path.clone(),
                project: project.clone(),
                branch: ws.branch.clone(),
                pane_count: ws.panes.len() as u32,
                attention: attention.clone(),
            });
            nodes.push(Node::Workspace(Workspace {
                head: head(&ws.id, attention),
                name: ws.name.clone(),
                kind: ws.kind,
                path: ws.path.clone(),
                project,
                branch: ws.branch.clone(),
                panes,
                layout: ws.layout.clone(),
            }));
        }
        nodes.push(Node::Session(Session {
            head: head(&self.model.session_id, session_attention),
            name: self.model.name.clone(),
            workspaces: crate::grouping::group(summaries),
        }));
        let mut kept = Vec::new();
        for (id, e) in &self.diffs {
            if !e.dirty && self.docs.contains_key(id) {
                kept.push(id.clone());
                continue;
            }
            let marks = self.model.viewed.get(&viewed_key(&e.workspace, &e.scope));
            let seen = |f: &contract::DiffFile| {
                let hash = marks?.get(&f.path)?;
                Some(if *hash == f.hash {
                    contract::Seen::Yes
                } else {
                    contract::Seen::Changed
                })
            };
            nodes.push(Node::Diff(contract::Diff {
                head: head(id, Vec::new()),
                workspace: e.workspace.clone(),
                scope: e.scope.kind(),
                commit: match &e.scope {
                    Scope::Commit(sha) => Some(sha.clone()),
                    _ => None,
                },
                base: e.read.base.clone(),
                state: e.read.state,
                too_large: e.read.too_large,
                omitted: e.read.omitted,
                files: e
                    .read
                    .files
                    .iter()
                    .map(|f| contract::DiffFile {
                        seen: if e.read.too_large { None } else { seen(f) },
                        ..f.clone()
                    })
                    .collect(),
            }));
        }
        for (id, e) in &self.commit_lists {
            if !e.dirty && self.docs.contains_key(id) {
                kept.push(id.clone());
                continue;
            }
            nodes.push(Node::Commits(contract::Commits {
                head: head(id, Vec::new()),
                workspace: e.workspace.clone(),
                base: e.list.base.clone(),
                more: e.list.more,
                commits: e.list.commits.clone(),
            }));
        }
        (nodes, kept)
    }
}

/// The key a diff's marks of having looked are kept under in the model.
fn viewed_key(workspace: &str, scope: &Scope) -> String {
    format!("{workspace}|{}", scope.key())
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
            Arc::new(crate::mobile::Hub::new(
                PathBuf::new(),
                PathBuf::new(),
                crate::mobile::LinkConfig::off(),
            )),
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
                name: None,
                shell: "/bin/sh".into(),
                cwd: "/tmp".into(),
                attention: vec![],
                running: None,
                last_program: None,
                resume: None,
            }],
            layout: None,
            attention: vec![],
            github: None,
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

    fn nothing_read() -> diff::Read {
        diff::Read {
            base: None,
            state: contract::DiffState::Ok,
            too_large: false,
            omitted: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn only_watched_nodes_are_read_and_idle_ones_go() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let live = core.add_diff(&ws, Scope::Uncommitted, nothing_read());
        assert_eq!(
            core.add_diff(&ws, Scope::Uncommitted, nothing_read()),
            live,
            "one node for one workspace and scope"
        );
        let other = core.add_diff(&ws, Scope::General, nothing_read());
        assert!(
            core.diff_jobs().is_empty(),
            "nobody has the node open, so git is not run for it"
        );

        let (tx, _rx) = std::sync::mpsc::sync_channel(64);
        let (stream, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let client = core.add_client(tx, stream.into());
        core.hello(client, &wire::default_accepts());
        core.open(client, &live, None, None);
        core.open(client, &other, None, None);
        let jobs = core.diff_jobs();
        assert_eq!(
            jobs.iter().map(|j| j.node.as_str()).collect::<Vec<_>>(),
            [live.as_str()],
            "only the uncommitted scope is polled"
        );

        // Both closed, and unseen for longer than the limit: both go, with their documents.
        core.close(client, &live);
        core.close(client, &other);
        let long_ago = Instant::now()
            .checked_sub(IDLE + Duration::from_secs(1))
            .expect("the machine has been up for a minute");
        for e in core.diffs.values_mut() {
            e.idle_since = long_ago;
        }
        assert!(core.diff_jobs().is_empty());
        assert!(core.diffs.is_empty());
        assert!(!core.docs.contains_key(&live) && !core.docs.contains_key(&other));
    }

    #[test]
    fn a_node_that_has_not_changed_is_not_built_again() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let node = core.add_diff(&ws, Scope::Uncommitted, nothing_read());
        let rev = core.docs[&node].rev;
        core.refresh();
        core.diff_update(&node, nothing_read());
        assert_eq!(core.docs[&node].rev, rev, "the same read changes nothing");
        let mut changed = nothing_read();
        changed.base = Some("origin/main".into());
        core.diff_update(&node, changed);
        assert_eq!(core.docs[&node].rev, rev + 1);
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
