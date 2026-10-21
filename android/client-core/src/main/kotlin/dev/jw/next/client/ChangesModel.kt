package dev.jw.next.client

import dev.jw.next.contract.DiffFile
import dev.jw.next.contract.DiffScope
import dev.jw.next.contract.DiffState
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.Seen

enum class ChangesView { Uncommitted, Commits, General }

/** What the screen has to show for the view the person is on. */
sealed interface Content {
    /** The node was asked for and has not arrived. */
    data object Reading : Content

    /** The daemon refused or git failed; the other views are still reachable. */
    data class Failed(val why: String) : Content

    /** `origin` has no default branch: General has nothing to compare with. */
    data object NoBase : Content

    data class Files(val diff: Node.Diff) : Content
    data class CommitList(val list: Node.Commits) : Content
}

/** Whether the uncommitted view is following the PC, or has just been patched. */
enum class Live { Following, Updated }

/**
 * The changes screen of one workspace (REQ-113 to REQ-121): which view, commit and file the
 * person is on, and the nodes the PC made for them. The documents are the PC's; this keeps
 * only where the person is. [onChange] says the screen must be read again, whichever thread
 * the answer came on.
 */
class ChangesModel(
    private val source: NodeSource,
    val workspace: Id,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private sealed interface Key {
        data object Uncommitted : Key
        data object General : Key
        data object Commits : Key
        data class Commit(val sha: String) : Key
    }

    private class Slot(var node: Id? = null, var error: String? = null)

    private val lock = Any()
    private val slots = mutableMapOf<Key, Slot>()
    private val shut = mutableSetOf<String>()

    @Volatile
    var onChange: () -> Unit = {}

    @Volatile
    var view = ChangesView.Uncommitted
        private set

    /** The commit open in the Commits view. */
    @Volatile
    var commit: String? = null
        private set

    /** The path of the file open on its own screen. */
    @Volatile
    var file: String? = null
        private set

    private var lastRev: Long? = null
    private var updatedUntil = 0L
    private var ownPatch = false

    init {
        want(Key.Uncommitted)
    }

    /** The node a view shows before anything is chosen inside it. */
    private fun root(view: ChangesView): Key = when (view) {
        ChangesView.Uncommitted -> Key.Uncommitted
        ChangesView.General -> Key.General
        ChangesView.Commits -> Key.Commits
    }

    private fun key(): Key = commit?.takeIf { view == ChangesView.Commits }?.let { Key.Commit(it) } ?: root(view)

    /** Ask for the node of [key] once. */
    private fun want(key: Key) {
        val reply = { result: Result<Id> ->
            synchronized(lock) {
                slots[key]?.let { slot ->
                    result.onSuccess { id -> slot.node = id; slot.error = null }
                        .onFailure { slot.error = it.message ?: "no se pudo leer" }
                }
            }
            result.getOrNull()?.let(source::open)
            onChange()
        }
        synchronized(lock) {
            if (slots.containsKey(key)) return
            slots[key] = Slot()
        }
        when (key) {
            Key.Uncommitted -> source.openDiff(workspace, DiffScope.Uncommitted, null, reply)
            Key.General -> source.openDiff(workspace, DiffScope.General, null, reply)
            Key.Commits -> source.openCommits(workspace, reply)
            is Key.Commit -> source.openDiff(workspace, DiffScope.Commit, key.sha, reply)
        }
    }

    fun select(view: ChangesView) {
        this.view = view
        commit = null
        file = null
        want(key())
        onChange()
    }

    fun openCommit(sha: String) {
        commit = sha
        file = null
        want(Key.Commit(sha))
        onChange()
    }

    /** The commit [delta] places from the open one in the list, when the list is here. */
    private fun neighbour(delta: Int): String? {
        val shown = commit ?: return null
        val commits = commitList()?.commits ?: return null
        return commits.getOrNull(commits.indexOfFirst { it.sha == shown } + delta)?.sha
    }

    /** `◂` and `▸`: the previous or next commit of the list. */
    fun stepCommit(delta: Int) {
        neighbour(delta)?.let(::openCommit)
    }

    fun hasCommit(delta: Int): Boolean = neighbour(delta) != null

    fun openFile(path: String) {
        file = path
        onChange()
    }

    /** Back: the file, then the commit, then the screen. True when it was used here. */
    fun back(): Boolean = when {
        file != null -> { file = null; onChange(); true }
        view == ChangesView.Commits && commit != null -> { commit = null; onChange(); true }
        else -> false
    }

    fun toggleFolder(key: String) {
        val full = "${this.key()}|$key"
        if (!shut.remove(full)) shut += full
        onChange()
    }

    fun isShut(key: String) = "${this.key()}|$key" in shut

    /** A view that failed is asked for again; a view that did not is read again. */
    fun retry() {
        val key = key()
        val failed = synchronized(lock) { slots[key]?.error != null }
        if (failed) {
            synchronized(lock) { slots.remove(key) }
            want(key)
        } else {
            node(key)?.let(source::refreshDiff)
        }
        onChange()
    }

    private fun node(key: Key): Id? = synchronized(lock) { slots[key]?.node }

    /** Mark the file looked at, or forget it when it already is; the PC keeps the mark. */
    fun toggleViewed(path: String) {
        val node = node(key()) ?: return
        val diff = source.node(node) as? Node.Diff ?: return
        val file = diff.files.firstOrNull { it.path == path } ?: return
        markOnly(node, path, file.seen != Seen.Yes)
    }

    private fun markOnly(node: Id, path: String, viewed: Boolean) {
        ownPatch = true
        source.setViewed(node, path, viewed)
    }

    /** Stop watching what this screen opened. */
    fun close() {
        val nodes = synchronized(lock) { slots.values.mapNotNull { it.node } }
        nodes.forEach(source::close)
    }

    fun content(): Content {
        val slot = synchronized(lock) { slots[key()]?.let { it.node to it.error } }
        slot?.second?.let { return Content.Failed(it) }
        return when (val node = slot?.first?.let(source::node)) {
            is Node.Diff -> if (node.state == DiffState.NoBase) Content.NoBase else Content.Files(node)
            is Node.Commits -> Content.CommitList(node)
            else -> Content.Reading
        }
    }

    /** What the tabs show once each view is known: the files or commits it holds, `!` when it failed. */
    fun count(of: ChangesView): String? {
        val (node, error) = synchronized(lock) { slots[root(of)]?.let { it.node to it.error } } ?: return null
        if (error != null) return "!"
        return when (val doc = node?.let(source::node)) {
            is Node.Diff -> doc.files.size.toString()
            is Node.Commits -> doc.commits.size.toString()
            else -> "…"
        }
    }

    private fun commitList(): Node.Commits? = node(Key.Commits)?.let { source.node(it) as? Node.Commits }

    /** The file open on its own screen, in the diff on show. */
    fun openFileIn(diff: Node.Diff): DiffFile? = file?.let { path -> diff.files.firstOrNull { it.path == path } }

    /** The screen read the nodes again: say when the uncommitted view was patched by someone else. */
    fun onNodes() {
        val node = node(Key.Uncommitted) ?: return
        val rev = source.rev(node) ?: return
        if (lastRev != null && lastRev != rev && !ownPatch) updatedUntil = clock() + UPDATED_MS
        if (lastRev != rev) ownPatch = false
        lastRev = rev
    }

    fun live(): Live = if (clock() < updatedUntil) Live.Updated else Live.Following

    /** Milliseconds until [live] goes back to following, or 0. */
    fun updatedFor(): Long = (updatedUntil - clock()).coerceAtLeast(0)

    companion object {
        const val UPDATED_MS = 2500L
    }
}
