package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.AttentionKind
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceKind

/** The mark drawn for an attention: a glyph and a tone the UI colours. */
data class Mark(val glyph: String, val tone: Tone)

enum class Tone { Danger, Warn, Plan, Good, Dim, Merged }

private val urgency = listOf(
    AttentionKind.Permission, AttentionKind.Question, AttentionKind.Plan, AttentionKind.CiFailed,
    AttentionKind.ShellDone, AttentionKind.Screen, AttentionKind.AgentDone, AttentionKind.Merged,
)

fun Attention.mark(): Mark = when (kind) {
    AttentionKind.Permission -> Mark("!", Tone.Danger)
    AttentionKind.Question -> Mark("?", Tone.Warn)
    AttentionKind.Plan -> Mark("◆", Tone.Plan)
    AttentionKind.CiFailed -> Mark("✗", Tone.Danger)
    AttentionKind.ShellDone -> if ((exitCode ?: 0) != 0) Mark("✗", Tone.Danger) else Mark("❯", Tone.Good)
    AttentionKind.Screen -> Mark("…", Tone.Dim)
    AttentionKind.AgentDone -> Mark("✓", Tone.Good)
    AttentionKind.Merged -> Mark("⚑", Tone.Merged)
}

/** What an attention says, in Spanish like the rest of the app. */
fun Attention.what(): String = when (kind) {
    AttentionKind.Permission -> "pide permiso"
    AttentionKind.Question -> "hace una pregunta"
    AttentionKind.Plan -> "plan listo"
    AttentionKind.CiFailed -> "CI falló"
    AttentionKind.ShellDone -> "comando terminó · salió ${exitCode ?: 0}"
    AttentionKind.Screen -> "espera en pantalla"
    AttentionKind.AgentDone -> "agente listo"
    AttentionKind.Merged -> "mergeado"
}

/** Most urgent first, the order of the legend in the prototype. */
fun List<Attention>.byUrgency(): List<Attention> = sortedBy { urgency.indexOf(it.kind) }

/** One row of Home (REQ-33). */
data class HomeRow(
    val id: Id,
    val name: String,
    val detail: String,
    val paneCount: Int,
    /** The mark of the most urgent unseen attention, if any. */
    val mark: Mark?,
    /** How many unseen attentions, for the count beside the mark. */
    val unseen: Int,
    /** The line under the row and the attention its go control opens. */
    val line: String?,
    val go: Attention?,
)

/** An item of Avisos: one unseen attention and the workspace it belongs to. */
data class Aviso(val workspaceId: Id, val workspace: String, val attention: Attention)

fun homeRows(session: Node.Session, seen: Seen): List<HomeRow> = session.workspaces.map { w ->
    val unseen = w.attention.filterNot(seen::isSeen).byUrgency()
    val top = unseen.firstOrNull()
    HomeRow(
        id = w.id,
        name = w.name,
        detail = (if (w.kind == WorkspaceKind.Worktree) "↳ ${w.branch ?: ""}" else "carpeta") +
            " · ${w.paneCount} ${if (w.paneCount == 1) "panel" else "paneles"}",
        paneCount = w.paneCount,
        mark = top?.mark(),
        unseen = unseen.size,
        line = top?.what(),
        go = top,
    )
}

fun avisos(session: Node.Session, seen: Seen): List<Aviso> = session.workspaces.flatMap { w ->
    w.attention.filterNot(seen::isSeen).byUrgency().map { Aviso(w.id, w.name, it) }
}

/** Where tapping an attention goes: the workspace, and the pane when the attention names one. */
data class Route(val workspace: Id, val pane: Id?)

fun routeOf(session: Node.Session, a: Attention): Route? {
    val owner = session.workspaces.firstOrNull { w -> w.attention.any { it == a } } ?: return null
    return Route(owner.id, a.node.takeIf { it != owner.id })
}
