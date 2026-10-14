package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

/**
 * What the sheet that asks before a pane is closed says (REQ-148, REQ-149). The phone always
 * asks, and only ever closes a pane: closing a workspace or removing a worktree is the PC's.
 */
data class CloseSheet(val pane: Id, val title: String, val running: String?) {
    val heading: String get() = "cerrar el panel «$title»"

    val body: String
        get() {
            val effect = if (running != null) "$running sigue corriendo en este panel. Cerrarlo lo detiene." else "Cerrarlo termina su shell."
            return "$effect El workspace se queda, aunque sea sin paneles. Cerrar un workspace o quitar un worktree se hace en el PC."
        }
}

/** The sheet for [pane], or null while its document has not arrived. */
fun closeSheet(source: NodeSource, pane: Id): CloseSheet? {
    val p = source.node(pane) as? Node.Pane ?: return null
    val program = p.running?.argv?.firstOrNull()?.substringAfterLast('/')
    val command = (p.blocks?.let(source::node) as? Node.Blocks)?.blocks?.lastOrNull()?.takeIf { it.running }
        ?.cmd?.trim()?.substringBefore(' ')
    return CloseSheet(pane, p.title, program ?: command)
}
