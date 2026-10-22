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
    Attention, AttentionKind, DaemonMsg, ErrorCode, Head, Id, Job, JobKind, JobState, Node,
    PaneInput, PaneSummary, PatchOp, Screen, Session, Workspace, WorkspaceSummary,
};
use serde_json::Value;

use crate::diff::{self, Scope};
use crate::history;
use crate::model::{Model, Own};
use crate::persist;
use crate::pty::{self, Event, Term};

/// The node document versions this daemon can serve.
pub const SUPPORTED: [(&str, &[u32]); 7] = [
    ("session", &[1]),
    ("workspace", &[1]),
    ("pane", &[1]),
    ("blocks", &[1]),
    ("diff", &[1]),
    ("commits", &[1]),
    ("conversation", &[1]),
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
    /// `claude` typed in a pane is the binary's own (a conversation), in a shell that has the
    /// integration; then its attention comes from the engine and it is given no hooks (REQ-374).
    pub swap: bool,
    /// The folder of the personal settings of projects (REQ-483).
    pub projects_dir: PathBuf,
    /// Where each pane's blocks are kept so they come back with the daemon; none keeps nothing.
    pub history_dir: Option<PathBuf>,
    /// The conversations of the panes that run `claude`, by pane id, and where they are kept.
    pub convs: HashMap<Id, crate::engine::Conv>,
    pub conv_dir: Option<PathBuf>,
    /// The conversations get the Claude in Chrome tools unless they say otherwise (REQ-634).
    pub chrome: bool,
    /// `gh`, for what the person asks of GitHub; and the daemon's state folder, where the files
    /// that brief a review are written (REQ-584).
    pub gh: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    /// The daemon is being stopped: the shells that end then are not programs that ended, and
    /// what a restart brings back stays as it was.
    pub stopping: bool,
    /// The operation that runs for each workspace and can be stopped: a push, a review being made
    /// (REQ-604).
    ops: HashMap<Id, crate::cancel::Cancel>,
    /// The slow things the daemon does or did, newest first, when each began, and the workspaces
    /// whose worktree is being removed. None is saved: a restart loses them (REQ-653).
    jobs: Vec<Job>,
    job_clock: HashMap<Id, Instant>,
    removing: HashSet<Id>,
    /// The panes whose blocks changed since they were saved, and when each was last saved.
    history_dirty: HashSet<Id>,
    history_written: HashMap<Id, Instant>,
    /// How many blocks a pane had, and whether the last was running, when it was last saved.
    history_shape: HashMap<Id, (usize, bool)>,
}

/// How many jobs that ended are kept for the clients to show (REQ-653).
const FINISHED_JOBS: usize = 20;

/// The most often the blocks of a pane are written to disk.
const HISTORY_EVERY: Duration = Duration::from_secs(2);

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
            history_dir: None,
            convs: HashMap::new(),
            conv_dir: None,
            chrome: false,
            gh: None,
            state_dir: None,
            stopping: false,
            ops: HashMap::new(),
            jobs: Vec::new(),
            job_clock: HashMap::new(),
            removing: HashSet::new(),
            history_dirty: HashSet::new(),
            history_written: HashMap::new(),
            history_shape: HashMap::new(),
            diffs: HashMap::new(),
            commit_lists: HashMap::new(),
            events,
            shell,
            env,
            integration,
            mobile,
            hooks: None,
            swap: false,
            projects_dir: identity::config_dir().join("projects"),
        };
        core.refresh();
        core
    }

    /// The hooks an agent started by the daemon is given: none where its shell gives the
    /// conversation instead.
    pub fn agent_hooks(&self) -> Option<PathBuf> {
        let setup = self.integration_for(&self.shell);
        let integrated = !(setup.args.is_empty() && setup.env.is_empty());
        if self.swap && integrated {
            None
        } else {
            self.hooks.clone()
        }
    }

    /// Every running terminal, for a look at what is in its foreground.
    pub fn terms_snapshot(&self) -> Vec<(Id, Arc<Term>)> {
        self.terms
            .iter()
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect()
    }

    /// Where a pane's shell was started.
    pub fn pane_cwd(&self, pane: &str) -> Option<PathBuf> {
        self.model
            .workspaces
            .iter()
            .flat_map(|w| &w.panes)
            .find(|p| p.id == pane)
            .map(|p| PathBuf::from(&p.cwd))
    }

    /// The pull request that the workspace of `pane` reviews, the repository's folder and the file
    /// that briefs it (REQ-585).
    pub fn review_of_pane(&self, pane: &str) -> Option<(u64, PathBuf)> {
        let ws = self
            .model
            .workspaces
            .iter()
            .find(|w| w.panes.iter().any(|p| p.id == pane))?;
        let review = ws.review.as_ref()?;
        let repo = Path::new(ws.project.as_deref()?)
            .file_name()?
            .to_string_lossy();
        Some((
            review.pr,
            crate::review::context_path(self.state_dir.as_deref()?, &repo, review.pr),
        ))
    }

    fn pane_mut(&mut self, pane: &str) -> Option<&mut crate::model::Pane> {
        self.model
            .workspaces
            .iter_mut()
            .flat_map(|w| w.panes.iter_mut())
            .find(|p| p.id == pane)
    }

    /// A conversation raised something for the person: what the agent had raised is over.
    pub fn raise_agent(&mut self, pane: &str, kind: AttentionKind) {
        if let Some(p) = self.pane_mut(pane) {
            p.attention
                .retain(|o| !crate::agents::KINDS.contains(&o.kind));
            p.attention.push(Own {
                kind,
                exit_code: None,
                rev: None,
            });
        }
    }

    /// The question that waited was answered.
    pub fn clear_ask(&mut self, pane: &str) {
        if let Some(p) = self.pane_mut(pane) {
            p.attention.retain(|o| {
                !matches!(
                    o.kind,
                    AttentionKind::Permission | AttentionKind::Question | AttentionKind::Plan
                )
            });
        }
    }

    /// Everything an agent had raised is over (the person moved on, or the agent is gone).
    pub fn clear_agent(&mut self, pane: &str) {
        if let Some(p) = self.pane_mut(pane) {
            p.attention
                .retain(|o| !crate::agents::KINDS.contains(&o.kind));
        }
    }

    pub fn set_pane_resume(&mut self, pane: &str, resume: Option<Vec<String>>) {
        if let Some(p) = self.pane_mut(pane) {
            p.resume = resume;
        }
    }

    /// Record the folders the shells are in; true when any changed (REQ-626).
    pub fn set_folders(&mut self, folders: &HashMap<Id, String>) -> bool {
        if self.stopping {
            return false;
        }
        let mut changed = false;
        for p in self
            .model
            .workspaces
            .iter_mut()
            .flat_map(|w| w.panes.iter_mut())
        {
            if let Some(folder) = folders.get(&p.id)
                && p.cwd != *folder
            {
                p.cwd.clone_from(folder);
                changed = true;
            }
        }
        changed
    }

    /// Record the programs found in the panes; true when any changed. A program that ended
    /// becomes the pane's `last_program`, and the title follows what runs.
    pub fn set_running(&mut self, updates: &HashMap<Id, Option<Vec<String>>>) -> bool {
        if self.stopping {
            return false;
        }
        let mut changed = false;
        let mut ended: Vec<Id> = Vec::new();
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
            // An agent with a known conversation is what a restart brings back; a conversation
            // of ours says which one itself.
            let has_conv = self.convs.contains_key(&p.id);
            if !(new.is_some() && has_conv) {
                p.resume = new.as_deref().and_then(crate::launch::agent_resume);
            }
            if let Some(c) = self.convs.get_mut(&p.id) {
                if new.as_deref().is_some_and(crate::launch::is_agent) {
                    c.stub_seen = true;
                } else if c.stub_seen {
                    // The binary's `claude` that was in the foreground is gone (killed, or it
                    // crashed) without closing: the conversation goes with it (REQ-377).
                    ended.push(p.id.clone());
                }
            }
            p.title = p.name.clone().unwrap_or_else(|| {
                new.as_deref()
                    .and_then(crate::procs::title_for)
                    .unwrap_or_else(|| crate::panes::basename(&p.shell))
            });
            p.running = new.clone();
            changed = true;
        }
        for pane in ended {
            if let Some(c) = self.convs.remove(&pane)
                && let Some(e) = c.engine
            {
                e.stop();
            }
            self.clear_agent(&pane);
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
            self.history_dirty.insert(pane.to_owned());
        }
    }

    /// What a pane's blocks are shaped like: how many and whether the last is running. A command
    /// starting or ending, or `clear`, changes it; the output of a running one does not.
    fn blocks_shape(&self, pane: &str) -> Option<(usize, bool)> {
        let (_, _, blocks) = self.blocks_views.get(pane)?;
        Some((blocks.len(), blocks.last().is_some_and(|b| b.running)))
    }

    /// The files of blocks to write now, serialised here so the caller can write them without the
    /// lock: those of each pane whose blocks changed and either changed shape or were not written in
    /// the last two seconds, or all that changed when `force`. A daemon that is stopped with a
    /// signal does not get to save once more, so what matters is saved as it happens.
    pub fn history_writes(&mut self, force: bool) -> Vec<(PathBuf, Id, String)> {
        if self.history_dirty.is_empty() {
            return Vec::new();
        }
        let Some(dir) = self.history_dir.clone() else {
            return Vec::new();
        };
        let now = Instant::now();
        let due: Vec<Id> = self
            .history_dirty
            .iter()
            .filter(|p| {
                force
                    || self.history_shape.get(*p) != self.blocks_shape(p).as_ref()
                    || self
                        .history_written
                        .get(*p)
                        .is_none_or(|t| now.duration_since(*t) >= HISTORY_EVERY)
            })
            .cloned()
            .collect();
        let at = crate::blocks::now_ms();
        due.into_iter()
            .filter_map(|pane| {
                self.history_dirty.remove(&pane);
                let (_, _, blocks) = self.blocks_views.get(&pane)?;
                self.history_written.insert(pane.clone(), now);
                let text = history::to_text(blocks, at);
                if let Some(shape) = self.blocks_shape(&pane) {
                    self.history_shape.insert(pane.clone(), shape);
                }
                Some((dir.clone(), pane, text))
            })
            .collect()
    }

    /// Take away the file of each of these panes that is gone: one written after its pane was
    /// closed (the write is done without the lock, so a close can come between).
    pub fn forget_written(&mut self, panes: &[Id]) {
        let Some(dir) = &self.history_dir else {
            return;
        };
        for pane in panes {
            let live = self
                .model
                .workspaces
                .iter()
                .any(|w| w.panes.iter().any(|p| p.id == *pane));
            if !live {
                history::remove(dir, pane);
            }
        }
    }

    /// Take away the saved blocks of panes that are gone.
    fn prune_histories(&mut self) {
        let Some(dir) = &self.history_dir else {
            return;
        };
        let live: HashSet<&str> = self
            .model
            .workspaces
            .iter()
            .flat_map(|w| w.panes.iter().map(|p| p.id.as_str()))
            .collect();
        let gone: Vec<Id> = self
            .history_written
            .keys()
            .chain(self.history_dirty.iter())
            .filter(|p| !live.contains(p.as_str()))
            .cloned()
            .collect();
        for pane in gone {
            history::remove(dir, &pane);
            self.history_written.remove(&pane);
            self.history_shape.remove(&pane);
            self.history_dirty.remove(&pane);
        }
    }

    /// The workspace with this id, or the `not_found` a command answers with; one whose worktree
    /// is being removed is `conflict`, for nothing can be done with it (REQ-651).
    pub fn workspace(
        &self,
        id: &str,
    ) -> Result<&crate::model::Workspace, crate::commands::ErrorBody> {
        if self.removing.contains(id) {
            return Err(crate::commands::err(
                ErrorCode::Conflict,
                "ya se está quitando",
            ));
        }
        self.model
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| crate::commands::err(ErrorCode::NotFound, format!("no workspace {id}")))
    }

    /// A job begins: it is the newest, and the clients are told.
    pub fn begin_job(
        &mut self,
        kind: JobKind,
        workspace: &str,
        name: String,
        cancellable: bool,
    ) -> Id {
        let id = crate::model::new_id();
        self.jobs.insert(
            0,
            Job {
                id: id.clone(),
                kind,
                workspace: workspace.to_owned(),
                name,
                state: JobState::Running,
                started_at: crate::blocks::now_ms(),
                duration_ms: None,
                detail: None,
                cancellable,
            },
        );
        self.job_clock.insert(id.clone(), Instant::now());
        self.refresh();
        id
    }

    /// A job ends, well or with the reason it failed. Nothing is published: the caller does, once,
    /// with whatever else changed with it.
    fn finish_job(&mut self, job: &str, result: Result<(), String>) {
        let took = self.job_clock.remove(job).map(|t| t.elapsed());
        if let Some(j) = self
            .jobs
            .iter_mut()
            .find(|j| j.id == job && j.state == JobState::Running)
        {
            j.duration_ms = took.map(|t| t.as_millis() as u64);
            match result {
                Ok(()) => j.state = JobState::Done,
                Err(why) => {
                    j.state = JobState::Failed;
                    j.detail = Some(why);
                }
            }
        }
        let mut kept = 0;
        self.jobs.retain(|j| {
            if j.state == JobState::Running {
                return true;
            }
            kept += 1;
            kept <= FINISHED_JOBS
        });
    }

    pub fn end_job(&mut self, job: &str, result: Result<(), String>) {
        self.finish_job(job, result);
        self.refresh();
    }

    /// The worktree of `workspace` begins to be removed: it is marked and its job begins, in one
    /// step, so a second removal finds it marked (REQ-648).
    pub fn begin_removal(&mut self, workspace: &str) -> Result<Id, crate::commands::ErrorBody> {
        let name = self.workspace(workspace)?.name.clone();
        self.removing.insert(workspace.to_owned());
        Ok(self.begin_job(JobKind::RemoveWorkspace, workspace, name, false))
    }

    /// The removal ended: the workspace goes when git took its folder, and is as it was when it did
    /// not; the job says which, and the clients are told once (REQ-649).
    pub fn end_removal(&mut self, workspace: &str, job: &str, result: Result<(), String>) {
        self.removing.remove(workspace);
        if result.is_ok() {
            self.model.workspaces.retain(|w| w.id != workspace);
        }
        self.finish_job(job, result);
        self.refresh();
    }

    /// Forget a job that ended (REQ-653).
    pub fn dismiss_job(&mut self, job: &str) -> Result<(), crate::commands::ErrorBody> {
        let Some(at) = self.jobs.iter().position(|j| j.id == job) else {
            return Err(crate::commands::err(
                ErrorCode::NotFound,
                format!("no job {job}"),
            ));
        };
        if self.jobs[at].state == JobState::Running {
            return Err(crate::commands::err(
                ErrorCode::Conflict,
                "el trabajo sigue corriendo",
            ));
        }
        self.jobs.remove(at);
        self.refresh();
        Ok(())
    }

    /// Forget every job that ended (REQ-653).
    pub fn dismiss_finished_jobs(&mut self) {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.state == JobState::Running);
        if self.jobs.len() != before {
            self.refresh();
        }
    }

    pub fn workspace_dir(&self, workspace: &str) -> Option<PathBuf> {
        self.model
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .map(|w| PathBuf::from(&w.path))
    }

    /// The plan that waits as `ask` in the conversation `node`: its text and the file the engine
    /// says it is in (REQ-467).
    pub fn plan_waiting(&self, node: &str, ask: &str) -> Option<(String, Option<String>)> {
        self.convs
            .values()
            .find(|c| c.id == node)?
            .st
            .waiting_plan(ask)
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
        let gone: Vec<Id> = self
            .convs
            .keys()
            .filter(|k| !live.contains(k.as_str()))
            .cloned()
            .collect();
        for k in gone {
            if let Some(c) = self.convs.remove(&k) {
                if let Some(e) = c.engine {
                    e.stop();
                }
                if let Some(dir) = &self.conv_dir {
                    crate::engine::forget(dir, &k);
                }
            }
        }
        dead.iter().filter_map(|k| self.terms.remove(k)).collect()
    }

    /// Note that an operation that can be stopped begins for `workspace`: its token, or the refusal
    /// when one already runs there.
    pub fn begin_op(
        &mut self,
        workspace: &str,
    ) -> Result<crate::cancel::Cancel, (ErrorCode, String)> {
        if self.ops.contains_key(workspace) {
            return Err((
                ErrorCode::Conflict,
                "ya hay una operación en curso para este workspace".into(),
            ));
        }
        let token = crate::cancel::Cancel::default();
        self.ops.insert(workspace.to_owned(), token.clone());
        Ok(token)
    }

    pub fn end_op(&mut self, workspace: &str) {
        self.ops.remove(workspace);
    }

    /// Stop what runs for `workspace`, if anything does: true when something was running.
    pub fn cancel_op(&self, workspace: &str) -> bool {
        self.ops.get(workspace).map(|t| t.cancel()).is_some()
    }

    /// End every shell and close the input of every engine (the daemon is stopping); the engines
    /// that were closed are returned for `engine::finish` to wait for.
    pub fn shutdown(&mut self) -> Vec<crate::engine::Leaving> {
        let leaving = crate::engine::stop_all(&mut self.convs);
        for term in self.terms.drain().map(|(_, t)| t) {
            term.kill();
        }
        leaving
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
        DaemonMsg::Snapshot {
            node: Box::new(node),
        }
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
            if ids.is_empty() {
                ws.views.clear();
            }
            let views: Vec<Id> = ws.views.iter().map(|v| v.id.clone()).collect();
            if !crate::layout::agrees_views(&ws.layout, &ids, &views) {
                ws.layout = crate::layout::reconcile_views(ws.layout.take(), &ids, &views);
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
        self.prune_histories();
        let Some(path) = self.save_path.clone() else {
            return;
        };
        let text = self.saved_text();
        if text != self.last_saved {
            match persist::save_text(&path, &text) {
                Ok(()) => self.last_saved = text,
                Err(e) => eprintln!("cannot save {}: {e}", path.display()),
            }
        }
    }

    /// The model as the session file keeps it: the leaves of views are not saved, since the
    /// views are not (REQ-711), so a restart finds the layout of the panes alone.
    fn saved_text(&mut self) -> String {
        let mut layouts = Vec::new();
        for ws in &mut self.model.workspaces {
            if ws.views.is_empty() {
                continue;
            }
            let ids: Vec<Id> = ws.panes.iter().map(|p| p.id.clone()).collect();
            let kept = ws
                .layout
                .clone()
                .and_then(|l| crate::layout::retain(l, &ids));
            layouts.push((ws.id.clone(), std::mem::replace(&mut ws.layout, kept)));
        }
        let text = persist::to_text(&self.model);
        for (id, layout) in layouts {
            if let Some(ws) = self.model.workspaces.iter_mut().find(|w| w.id == id) {
                ws.layout = layout;
            }
        }
        text
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
                    conversation: self.convs.get(&p.id).map(|c| c.id.clone()),
                }));
                if let Some(c) = self.convs.get(&p.id) {
                    nodes.push(Node::Conversation(Box::new(
                        c.st.node(head(&c.id, Vec::new()), &p.id),
                    )));
                }
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
            let project = ws.project_path();
            let group = self
                .model
                .group_for(&crate::model::key_from(project.clone(), &ws.path));
            summaries.push(WorkspaceSummary {
                id: ws.id.clone(),
                name: ws.name.clone(),
                kind: ws.kind,
                path: ws.path.clone(),
                project: project.clone(),
                branch: ws.branch.clone(),
                pane_count: ws.panes.len() as u32,
                attention: attention.clone(),
                review: ws.review.clone(),
                working: ws
                    .panes
                    .iter()
                    .any(|p| self.convs.get(&p.id).is_some_and(|c| c.st.works())),
                removing: self.removing.contains(&ws.id),
                group,
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
                views: ws
                    .views
                    .iter()
                    .map(|v| contract::ViewSummary {
                        id: v.id.clone(),
                        kind: v.kind,
                        title: crate::views::title(v),
                        files: v.files.clone(),
                    })
                    .collect(),
                review: ws.review.clone(),
                removing: self.removing.contains(&ws.id),
            }));
        }
        let order: Vec<Id> = self.model.groups.iter().map(|g| g.id.clone()).collect();
        nodes.push(Node::Session(Session {
            head: head(&self.model.session_id, session_attention),
            name: self.model.name.clone(),
            workspaces: crate::grouping::group(summaries, &order),
            jobs: self.jobs.clone(),
            groups: self.model.groups.clone(),
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

    /// The node types the daemon serves and the ones the client library asks for are two lists
    /// that nothing else compares (the phone's `hello` is a third, in `Data.kt`).
    #[test]
    fn the_daemon_serves_the_types_the_client_library_asks_for() {
        let served: std::collections::BTreeSet<&str> =
            super::SUPPORTED.iter().map(|(t, _)| *t).collect();
        let asked = wire::default_accepts();
        let asked: std::collections::BTreeSet<&str> = asked.keys().map(String::as_str).collect();
        assert_eq!(served, asked);
    }

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
            views: vec![],
            places: Default::default(),
            adopted: false,
            ports: None,
            review: None,
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

    /// REQ-391: while the daemon stops, the programs that end are not recorded as ended, so the
    /// agent that ran in a pane is still the one a restart brings back.
    #[test]
    fn a_program_that_ends_because_the_daemon_stops_is_not_forgotten() {
        let (mut model, _, pane) = model_with_pane();
        model.workspaces[0].panes[0].running = Some(vec!["claude".into()]);
        model.workspaces[0].panes[0].resume =
            Some(vec!["claude".into(), "--resume".into(), "abc".into()]);
        let gone: HashMap<Id, Option<Vec<String>>> = HashMap::from([(pane.clone(), None)]);
        let mut live = test_core(model.clone());
        assert!(live.set_running(&gone));
        assert_eq!(
            live.model.workspaces[0].panes[0].resume, None,
            "a program that ended is over"
        );
        let mut stopping = test_core(model);
        stopping.stopping = true;
        assert!(!stopping.set_running(&gone));
        let p = &stopping.model.workspaces[0].panes[0];
        assert!(
            p.resume.is_some() && p.running.is_some(),
            "stopping forgets nothing"
        );
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

    fn jobs_of(core: &Core) -> Vec<Value> {
        let session = core.model.session_id.clone();
        core.docs[&session].value["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// REQ-648, REQ-651: a workspace whose worktree is being removed is marked in both documents,
    /// a second removal and any command that looks the workspace up are refused, and its job runs.
    #[test]
    fn a_workspace_being_removed_is_marked_and_nothing_acts_on_it() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let session = core.model.session_id.clone();
        assert!(jobs_of(&core).is_empty());
        assert!(core.docs[&session].value.get("jobs").is_none());

        let job = core.begin_removal(&ws).unwrap();
        assert_eq!(core.docs[&ws].value["removing"], true);
        assert_eq!(core.docs[&session].value["workspaces"][0]["removing"], true);
        let jobs = jobs_of(&core);
        assert_eq!(
            (
                jobs[0]["id"].as_str(),
                jobs[0]["kind"].as_str(),
                jobs[0]["state"].as_str(),
                jobs[0]["name"].as_str()
            ),
            (
                Some(job.as_str()),
                Some("remove_workspace"),
                Some("running"),
                Some("w")
            )
        );
        assert!(jobs[0]["started_at"].as_u64().unwrap() > 0);
        assert!(jobs[0].get("duration_ms").is_none());
        for refused in [
            core.begin_removal(&ws),
            core.workspace(&ws).map(|_| String::new()),
        ] {
            assert_eq!(refused.unwrap_err().code, ErrorCode::Conflict);
        }
        assert_eq!(
            core.dismiss_job(&job).unwrap_err().code,
            ErrorCode::Conflict,
            "a job that runs is not dismissed"
        );
    }

    /// REQ-649: a removal that ends well takes the workspace and a one that fails gives it back,
    /// each in the one revision that ends the job.
    #[test]
    fn a_removal_ends_in_one_revision_gone_or_as_it_was() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let session = core.model.session_id.clone();

        let job = core.begin_removal(&ws).unwrap();
        let rev = core.docs[&session].rev;
        core.end_removal(&ws, &job, Err("fatal: it has changes".into()));
        assert_eq!(core.docs[&session].rev, rev + 1, "one patch");
        let jobs = jobs_of(&core);
        assert_eq!(
            (jobs[0]["state"].as_str(), jobs[0]["detail"].as_str()),
            (Some("failed"), Some("fatal: it has changes"))
        );
        assert!(jobs[0]["duration_ms"].is_u64());
        assert!(core.docs[&ws].value.get("removing").is_none());
        assert!(core.workspace(&ws).is_ok(), "the workspace is as it was");

        let job = core.begin_removal(&ws).unwrap();
        let rev = core.docs[&session].rev;
        core.end_removal(&ws, &job, Ok(()));
        assert_eq!(core.docs[&session].rev, rev + 1, "one patch");
        let jobs = jobs_of(&core);
        assert_eq!(
            (jobs[0]["id"].as_str(), jobs[0]["state"].as_str()),
            (Some(job.as_str()), Some("done"))
        );
        assert_eq!(jobs.len(), 2, "the failed one is still there");
        assert!(!core.docs.contains_key(&ws), "its document is gone");
        assert!(core.model.workspaces.is_empty());
    }

    /// REQ-653: twenty finished jobs are kept, the oldest go first and the ones that run never;
    /// they are dismissed one by one or all at once, and a job that is not there is `not_found`.
    #[test]
    fn finished_jobs_are_capped_and_dismissed() {
        let (model, ws, _) = model_with_pane();
        let mut core = test_core(model);
        let running = core.begin_job(JobKind::SyncWorkspace, &ws, "b".into(), true);
        let mut ended = Vec::new();
        for n in 0..FINISHED_JOBS + 3 {
            let job = core.begin_job(JobKind::SyncWorkspace, &ws, format!("b{n}"), true);
            core.end_job(&job, Ok(()));
            ended.push(job);
        }
        let jobs = jobs_of(&core);
        assert_eq!(jobs.len(), FINISHED_JOBS + 1);
        assert_eq!(
            jobs[0]["id"],
            ended[ended.len() - 1].as_str(),
            "newest first"
        );
        assert!(
            jobs.iter().any(|j| j["id"] == running.as_str()),
            "a job that runs is never dropped"
        );
        assert!(jobs.iter().all(|j| j["id"] != ended[0].as_str()));

        core.dismiss_job(&ended[ended.len() - 1]).unwrap();
        assert_eq!(jobs_of(&core).len(), FINISHED_JOBS);
        assert_eq!(
            core.dismiss_job(&ended[ended.len() - 1]).unwrap_err().code,
            ErrorCode::NotFound
        );
        core.dismiss_finished_jobs();
        let left = jobs_of(&core);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0]["id"], running.as_str());
        // nothing to dismiss changes nothing
        let rev = core.docs[&core.model.session_id].rev;
        core.dismiss_finished_jobs();
        assert_eq!(core.docs[&core.model.session_id].rev, rev);
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
