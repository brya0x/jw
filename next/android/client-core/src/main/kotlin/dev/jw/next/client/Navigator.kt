package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

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
}

sealed interface Screen {
    data object Home : Screen
    data object Avisos : Screen
    data class Workspace(val id: Id, val pane: Id?) : Screen
}

data class ClientState(val screen: Screen = Screen.Home, val seen: Seen = Seen())

/**
 * Moves between screens and keeps the seen record: opening the node an attention names is
 * what marks it seen (REQ-34), on this phone only (REQ-35).
 */
class Navigator(private val source: NodeSource) {
    fun openWorkspace(s: ClientState, id: Id, pane: Id? = null): ClientState {
        val ws = source.node(id) as? Node.Workspace
        val target = pane ?: ws?.panes?.firstOrNull()?.id
        var seen = markNode(s.seen, id)
        if (target != null) seen = markNode(seen, target)
        return ClientState(Screen.Workspace(id, target), seen)
    }

    fun selectPane(s: ClientState, pane: Id): ClientState {
        val at = s.screen as? Screen.Workspace ?: return s
        return ClientState(at.copy(pane = pane), markNode(s.seen, pane))
    }

    /** The go control of a row or an Avisos item. */
    fun go(s: ClientState, a: Attention): ClientState {
        val route = routeOf(source.session(), a) ?: return s
        return openWorkspace(s, route.workspace, route.pane)
    }

    /**
     * A workspace opened before its document arrived has no pane yet: once the document is
     * here, select its first pane (and mark it seen), as opening it would have.
     */
    fun settle(s: ClientState): ClientState {
        val at = s.screen as? Screen.Workspace ?: return s
        if (at.pane != null) return s
        val first = (source.node(at.id) as? Node.Workspace)?.panes?.firstOrNull()?.id ?: return s
        return ClientState(at.copy(pane = first), markNode(s.seen, first))
    }

    fun show(s: ClientState, screen: Screen): ClientState = ClientState(screen, s.seen)

    fun back(s: ClientState): ClientState = ClientState(Screen.Home, s.seen)

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
