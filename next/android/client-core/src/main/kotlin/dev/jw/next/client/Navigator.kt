package dev.jw.next.client

import dev.jw.next.contract.Accent
import dev.jw.next.contract.Attention
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ConversationList
import dev.jw.next.contract.Image
import dev.jw.next.contract.DiffScope
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.Id
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.Node
import dev.jw.next.contract.ProjectInfo

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

    /** Start the panes of a workspace that has none: its repository's layout and setup. */
    fun startWorkspace(workspace: Id, reply: (Result<Unit>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Make a worktree of [repo] on [branch] from [base], in the folder [folder] of the repository's worktree folder, and start its panes. */
    fun createWorktree(repo: String, branch: String, base: String?, folder: String, reply: (Result<Id>) -> Unit) {
        reply(Result.failure(UnsupportedOperationException("sin conexión a un PC")))
    }

    /** Let go of a workspace; with [removeWorktree] a worktree's folder goes too. [reply] gets the PC's message when it refuses (git, for a worktree with changes). */
    fun removeWorkspace(workspace: Id, removeWorktree: Boolean, reply: (Result<Unit>) -> Unit) {
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

    /** Answer the question that waits in a conversation with the option [key]; the PC writes that it came from the phone. */
    fun conversationAnswer(node: Id, ask: String, key: String, reply: (Result<Unit>) -> Unit) {
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

    /** The plan that waits as [ask] in the conversation [node], whole, as blocks (REQ-467). */
    fun readPlan(node: Id, ask: String, reply: (Result<MarkdownDoc>) -> Unit) {
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
)

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
        return ClientState(Screen.Workspace(id, target), seen, id, listOf(id) + s.recent.filter { it != id }.take(RECENT - 1))
    }

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

    fun back(s: ClientState): ClientState = s.copy(screen = Screen.Home)

    /** The files of [workspace]. */
    fun files(s: ClientState, workspace: Id): ClientState = s.copy(screen = Screen.Files(workspace), last = workspace)

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
