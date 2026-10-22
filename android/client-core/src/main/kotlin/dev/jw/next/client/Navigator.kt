package dev.jw.next.client

import dev.jw.next.contract.Accent
import dev.jw.next.contract.Attention
import dev.jw.next.contract.ConfigFrom
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ConversationList
import dev.jw.next.contract.Image
import dev.jw.next.contract.DiffScope
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.Id
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.TextDoc
import dev.jw.next.contract.OlderItems
import dev.jw.next.contract.OpenKind
import dev.jw.next.contract.Node
import dev.jw.next.contract.ProjectEdit
import dev.jw.next.contract.ProjectInfo
import dev.jw.next.contract.WorktreeInfo

/** Where the nodes come from: fixtures now, a live daemon later. */
interface NodeSource {
    val epoch: String
    fun session(): Node.Session

    /** An opened node, or null when it is not (yet) cached. */
    fun node(id: Id): Node?

    /** Ask for a node to be sent; the fixtures hold everything already. */
    fun open(id: Id) {}

    /** Tell the daemon this phone looked at [id] at [rev], so every client clears it. */
    fun markSeen(id: Id, rev: Long) {}

    /** Type [data] into a pane's shell (text; `\r` is Enter, `\u0003` is ^C). */
    fun write(pane: Id, data: String) {}

    /** The revision of a cached node, without decoding it. */
    fun rev(id: Id): Long? = node(id)?.rev

    /**
     * Ask the daemon for the `diff` node of [workspace] for [scope] ([commit] names the commit
     * of the `commit` scope). [reply] gets the node's id, or the daemon's message.
     */
    fun openDiff(workspace: Id, scope: DiffScope, commit: String?, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Ask the daemon for the `commits` node of [workspace]. */
    fun openCommits(workspace: Id, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Read a `diff` or `commits` node again now. */
    fun refreshDiff(node: Id) {}

    /** Record that [path] of a `diff` node was looked at, or forget it: the PC keeps it for every client. */
    fun setViewed(node: Id, path: String, viewed: Boolean) {}

    /** Stop watching [id]: the daemon stops patching it and stops reading git for it. */
    fun close(id: Id) {}

    /** Name a workspace (empty: its automatic name); [reply] gets the daemon's message when it refuses. */
    fun renameWorkspace(workspace: Id, name: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Make a group; [reply] gets its id. */
    fun createGroup(name: String, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Make a group and put the repository of [workspace] in it: the id of the group is read from the first reply, not guessed (REQ-676). */
    fun createGroupWith(name: String, workspace: Id, reply: (Result<Unit>) -> Unit) {
        createGroup(name) { made ->
            made.fold({ group -> moveToGroup(workspace, group, reply) }, { reply(Result.failure(it)) })
        }
    }

    fun renameGroup(group: Id, name: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Delete a group: its workspaces stay open and belong to none. */
    fun deleteGroup(group: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Put the repository of [workspace] in [group], or in none when it is null. */
    fun moveToGroup(workspace: Id, group: Id?, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Swap a group with its neighbour: [delta] is -1 (up) or 1 (down). */
    fun moveGroup(group: Id, delta: Int, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Name a pane (empty: the automatic name). */
    fun renamePane(pane: Id, name: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Read the subfolders of [path] (the PC's home when null); [hidden] adds those starting with a dot. */
    fun listDir(path: String?, hidden: Boolean, reply: (Result<DirListing>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Read one folder of [workspace] ([path] relative to its folder, its own folder when null); [reply] gets the PC's message when it refuses. */
    fun listFiles(workspace: Id, path: String?, reply: (Result<FileListing>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Open [path] as a workspace with a shell in it; [reply] gets the workspace's id, or the PC's message. */
    fun openFolder(path: String, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Read the settings of the repository [repo] is in (its `.jw.toml`); [reply] gets the PC's message when it cannot be used. */
    fun projectConfig(repo: String, reply: (Result<ProjectInfo>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Put [set] in the file [to] says of the settings of the repository [repo] is in, leaving the rest of the file as it was; [reply] gets where it went and whether the repository's own file hides it, or the PC's message (REQ-525). */
    fun editProjectConfig(repo: String, to: ConfigFrom, set: ProjectEdit, reply: (Result<Pair<String, Boolean>>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** The worktrees git knows of the repository [repo] is in, but its main one; [reply] gets the PC's message when it cannot list them (REQ-533). */
    fun listWorktrees(repo: String, reply: (Result<List<WorktreeInfo>>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Open the worktree at [path] that [listWorktrees] listed as a workspace with no panel; [reply] gets its id, or the PC's message (REQ-534). */
    fun openWorktree(repo: String, path: String, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Open each of [paths] with [openWorktree], one after the other; [reply] gets how many opened and why the others did not. */
    fun openWorktrees(repo: String, paths: List<String>, reply: (OpenedWorktrees) -> Unit) {
        fun next(i: Int, opened: Int, failed: List<String>) {
            if (i == paths.size) return reply(OpenedWorktrees(opened, failed))
            openWorktree(repo, paths[i]) { r ->
                r.fold({ next(i + 1, opened + 1, failed) }, { next(i + 1, opened, failed + (it.message ?: "el PC no pudo abrir el worktree")) })
            }
        }
        next(0, 0, emptyList())
    }

    /** Start the one panel of a workspace that has none, a shell or `claude` as [open] says or, when it is null, as its settings do; a worktree's setup runs in it first. */
    fun startWorkspace(workspace: Id, open: OpenKind?, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Make a worktree of [repo] on [branch] from [base], in the folder [folder] of the repository's worktree folder, and open it with [open] (null: as its settings say). */
    fun createWorktree(repo: String, branch: String, base: String?, folder: String, open: OpenKind?, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Let go of a workspace; with [removeWorktree] a worktree's folder goes too. [reply] gets the PC's message when it refuses (git, for a worktree with changes). */
    fun removeWorkspace(workspace: Id, removeWorktree: Boolean, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Forget a job that ended, for every client; [reply] gets the PC's message when it refuses. */
    fun dismissJob(job: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Forget every job that ended, for every client. */
    fun dismissJobs(reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Stop what runs for [workspace] and can be stopped: a push (REQ-604). */
    fun cancel(workspace: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Open a pane with a shell in [workspace]; [reply] gets the new pane's id, or the PC's message. */
    fun newPane(workspace: Id, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Close a pane (its shell ends); [reply] gets the daemon's message when it refuses. */
    fun closePane(pane: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Say something to Claude in a conversation, or run a command of its `/` list; [reply] gets the daemon's message when it refuses (a command that needs Claude's screen). */
    fun conversationSend(node: Id, text: String, images: List<Image>, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Answer the question that waits in a conversation with the option [key]; the PC writes that it came from the phone. [text] goes with the refusal of a plan: what to do next (REQ-595). */
    fun conversationAnswer(node: Id, ask: String, key: String, text: String? = null, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Stop the turn that runs. */
    fun conversationInterrupt(node: Id) {}

    /** Stop a task the engine runs in the background (a shell, a monitor, a subagent); [task] is its id in the node's tasks. */
    fun conversationStopTask(node: Id, task: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Change one thing of a conversation: its name, colour, model, effort or mode (what is null stays). */
    fun conversationSet(node: Id, name: String? = null, accent: Accent? = null, model: String? = null, effort: String? = null, mode: ConvMode? = null, reply: (Result<Unit>) -> Unit = {}) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Run a shell line of the person's in the pane's folder. */
    fun conversationShell(node: Id, command: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** The conversations Claude saved for the folder of [workspace], newest first. */
    fun listConversations(workspace: Id, reply: (Result<ConversationList>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** A Markdown file of [workspace] ([path] relative to its folder) as blocks, or the PC's message when it refuses (REQ-466). */
    fun readMarkdown(workspace: Id, path: String, reply: (Result<MarkdownDoc>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** A text file of [workspace] ([path] relative to its folder) as lines, or the PC's message when it refuses; a file that is not text fails with [BinaryFile] (REQ-610, REQ-613). */
    fun readFile(workspace: Id, path: String, reply: (Result<TextDoc>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Up to [count] of the items of the conversation [node] that came before the time [before] (REQ-567). */
    fun readOlder(node: Id, before: Long, count: Int, reply: (Result<OlderItems>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** The plan that waits as [ask] in the conversation [node], whole, as blocks (REQ-467). */
    fun readPlan(node: Id, ask: String, reply: (Result<MarkdownDoc>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Open a new conversation in [pane], in the folder its shell is in (REQ-630, REQ-633). */
    fun openConversation(pane: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Retake the saved conversation [id] in [pane]. */
    fun resumeConversation(pane: Id, id: String, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }
}

sealed interface Screen {
    data object Home : Screen
    data object Avisos : Screen

    /** The mode and the theme (REQ-157). */
    data object Settings : Screen
    data class Workspace(val id: Id, val pane: Id?) : Screen

    /** The changes of one workspace (REQ-113). */
    data class Changes(val workspace: Id) : Screen

    /** The PC's folders, to choose one to open (REQ-201). */
    data object Folders : Screen

    /** The files of one workspace, folder by folder and no higher than its own (REQ-323). */
    data class Files(val workspace: Id) : Screen
}

/**
 * [last] is the workspace opened most recently: the one the changes of the `±` open. [recent] is
 * every workspace opened on this phone, the latest first, for the search (REQ-232).
 */
data class ClientState(
    val screen: Screen = Screen.Home,
    val seen: Seen = Seen(),
    val last: Id? = null,
    val recent: List<Id> = emptyList(),
    /** Lines picked in the viewer of a file, waiting to be written in the field of the workspace's agent (REQ-639). */
    val draft: Draft? = null,
    /** The tab of Avisos that shows. */
    val avisosTab: AvisosTab = AvisosTab.Alertas,
    /** The groups that are folded on this phone, for the PC it is connected to (REQ-678). */
    val folded: Set<Id> = emptySet(),
    /** The chip of Avisos: null for every alert, [NO_GROUP], or a group's id (REQ-677). */
    val avisosFilter: Id? = null,
)

/** Lines [from] to [to] (from 1) of the file [path] of a workspace, as a reference for the agent (REQ-639). */
data class Draft(val path: String, val from: Int, val to: Int)

/**
 * Moves between screens and keeps the seen record: opening the node an attention names is
 * what marks it seen (REQ-34), on this phone only (REQ-35).
 */
/** How many opened workspaces the phone remembers for the search. */
private const val RECENT = 30

class Navigator(private val source: NodeSource) {
    fun openWorkspace(s: ClientState, id: Id, pane: Id? = null): ClientState {
        val ws = source.node(id) as? Node.Workspace
        val target = pane ?: ws?.panes?.firstOrNull()?.id
        var seen = markNode(s.seen, id)
        if (target != null) seen = markNode(seen, target)
        // The workspace's group opens, so that the list shows where it is on the way back (REQ-675).
        val group = source.session().workspaces.firstOrNull { it.id == id }?.group
        return ClientState(
            Screen.Workspace(id, target), seen, id, listOf(id) + s.recent.filter { it != id }.take(RECENT - 1),
            folded = group?.let { s.folded - it } ?: s.folded, avisosFilter = s.avisosFilter,
        )
    }

    /** A tap on the row of a group folds it or opens it (REQ-675). */
    fun toggleGroup(s: ClientState, group: Id): ClientState =
        s.copy(folded = if (group in s.folded) s.folded - group else s.folded + group)

    /**
     * A group the PC no longer lists is forgotten (REQ-678), folded or chosen in Avisos, once the PC
     * has sent its session: until then, between its hello and the document, the session is an empty
     * stand-in (no id, no epoch) and what was folded is still the person's.
     */
    fun forgetUnlisted(s: ClientState): ClientState {
        val session = source.session()
        if (source.epoch.isEmpty() || session.id.isEmpty() || session.epoch != source.epoch) return s
        val listed = session.groups.map { it.id }.toSet()
        val folded = if (listed.containsAll(s.folded)) s.folded else s.folded.filter { it in listed }.toSet()
        val filter = s.avisosFilter?.takeIf { it == NO_GROUP || it in listed }
        return if (folded == s.folded && filter == s.avisosFilter) s else s.copy(folded = folded, avisosFilter = filter)
    }

    /** The chip of Avisos that is pressed (REQ-677). */
    fun filterAvisos(s: ClientState, by: Id?): ClientState = s.copy(avisosFilter = by)

    fun selectPane(s: ClientState, pane: Id): ClientState {
        val at = s.screen as? Screen.Workspace ?: return s
        return s.copy(screen = at.copy(pane = pane), seen = markNode(s.seen, pane))
    }

    /** A tap on the line of an attention in a Home row, or on an Avisos item: open the pane it names (REQ-34). */
    fun go(s: ClientState, a: Attention): ClientState {
        val route = routeOf(source.session(), a) ?: return s
        return openWorkspace(s, route.workspace, route.pane)
    }

    /**
     * A workspace opened before its document arrived has no pane yet: once the document is
     * here, select its first pane (and mark it seen), as opening it would have. The same when
     * the selected pane is gone (it was closed): the first one takes its place, or none.
     */
    fun settle(s: ClientState): ClientState {
        val at = s.screen as? Screen.Workspace ?: return s
        val ws = source.node(at.id) as? Node.Workspace ?: return s
        if (at.pane != null && ws.panes.any { it.id == at.pane }) return s
        val first = ws.panes.firstOrNull()?.id
        val seen = if (first != null) markNode(s.seen, first) else s.seen
        return s.copy(screen = at.copy(pane = first), seen = seen)
    }

    /**
     * Opens [id] once the session shows it: a workspace the phone just asked for is not in the
     * session until the daemon says so. Null until then.
     */
    fun openWhenShown(s: ClientState, id: Id): ClientState? =
        if (source.session().workspaces.any { it.id == id }) openWorkspace(s, id) else null

    /**
     * Selects [pane] of [workspace] once the workspace's document lists it: a pane the phone just
     * asked for is not there until the daemon says so (REQ-235). Null until then.
     */
    fun selectWhenShown(s: ClientState, workspace: Id, pane: Id): ClientState? {
        val shown = (source.node(workspace) as? Node.Workspace)?.panes.orEmpty().any { it.id == pane }
        return if (shown) selectPane(s, pane) else null
    }

    fun show(s: ClientState, screen: Screen): ClientState = s.copy(screen = screen)

    /**
     * Avisos on [tab], or on the one that has something when none is asked for: the alerts that wait,
     * else the jobs when they run or ended unseen (REQ-659). Showing the jobs is seeing what ended.
     */
    fun openAvisos(s: ClientState, tab: AvisosTab? = null): ClientState {
        val session = source.session()
        val chosen = tab ?: if (avisos(session, s.seen).isEmpty() && jobsMark(session, s.seen) != JobsMark.None) AvisosTab.Trabajos else AvisosTab.Alertas
        return seeJobs(s.copy(screen = Screen.Avisos, avisosTab = chosen))
    }

    /** What the jobs tab shows is seen, whenever it is on screen: a job that ends while it is open is seen too. */
    fun seeJobs(s: ClientState): ClientState {
        if (s.screen != Screen.Avisos || s.avisosTab != AvisosTab.Trabajos) return s
        // Before the daemon has said hello there is no epoch, and a record is not emptied for it.
        if (source.epoch.isEmpty()) return s
        val ended = jobsOf(source.session()).ended.map { it.id }
        return if (ended.toSet() == s.seen.jobs) s else s.copy(seen = s.seen.seeJobs(ended, source.epoch))
    }

    fun back(s: ClientState): ClientState = s.copy(screen = Screen.Home)

    /** The files of [workspace]. */
    fun files(s: ClientState, workspace: Id): ClientState = s.copy(screen = Screen.Files(workspace), last = workspace)

    /** The lines of [draft] go to the agent of [workspace]: its screen opens, and writes them in the agent's field (REQ-639). */
    fun refer(s: ClientState, workspace: Id, draft: Draft): ClientState = openWorkspace(s, workspace).copy(draft = draft)

    /** The draft was written, or could not be: it is not asked for again. */
    fun spendDraft(s: ClientState): ClientState = s.copy(draft = null)

    /** Leaves the files of a workspace for the workspace itself; anywhere else it is [back]. */
    fun leaveFiles(s: ClientState): ClientState {
        val at = s.screen as? Screen.Files ?: return back(s)
        return openWorkspace(s, at.workspace)
    }

    /** Leaves the changes of a workspace for the workspace itself; anywhere else it is [back]. */
    fun leaveChanges(s: ClientState): ClientState {
        val at = s.screen as? Screen.Changes ?: return back(s)
        return openWorkspace(s, at.workspace)
    }

    /**
     * The `diff` tab: the changes of the workspace opened last, else the first one.
     * Null while the session has no workspace.
     */
    fun changes(s: ClientState): ClientState? {
        val ws = s.last?.takeIf { id -> source.session().workspaces.any { it.id == id } }
            ?: source.session().workspaces.firstOrNull()?.id
            ?: return null
        return s.copy(screen = Screen.Changes(ws), last = ws)
    }

    /** Seen at the node's revision when it is cached, else at the newest attention raised on it. */
    private fun markNode(seen: Seen, id: Id): Seen {
        val session = source.session()
        val rev = source.node(id)?.rev
            ?: session.workspaces.flatMap { it.attention }.filter { it.node == id }.maxOfOrNull { it.rev }
            ?: return seen
        source.markSeen(id, rev)
        return seen.mark(id, rev, source.epoch)
    }
}
