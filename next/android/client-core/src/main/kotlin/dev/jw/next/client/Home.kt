package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.AttentionKind
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceKind
import dev.jw.next.contract.WorkspaceSummary

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

/** What a row of Home is: a repository's root, a worktree of one, or a folder that is not a repository (REQ-222). */
enum class HomeKind { Root, Worktree, Folder }

/** One row of Home (REQ-33). */
data class HomeRow(
    val id: Id,
    val kind: HomeKind,
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

/**
 * What a workspace is called on the screen (REQ-185): a worktree that has the name it got when
 * it was made (`repo: branch`) shows its branch, and a name the person chose shows as chosen.
 */
fun shownName(name: String, kind: WorkspaceKind, branch: String?): String =
    if (kind == WorkspaceKind.Worktree && branch != null && name.endsWith(": $branch")) branch else name

/** The name of a summary as the lists draw it: a worktree has `↳` before it. */
fun WorkspaceSummary.shown(): String = shownName(name, kind, branch)

private fun WorkspaceSummary.homeKind() = when {
    kind == WorkspaceKind.Worktree -> HomeKind.Worktree
    project != null -> HomeKind.Root
    else -> HomeKind.Folder
}

fun homeRows(session: Node.Session, seen: Seen): List<HomeRow> = session.workspaces.map { w ->
    val unseen = w.attention.filterNot(seen::isSeen).byUrgency()
    val top = unseen.firstOrNull()
    val kind = w.homeKind()
    HomeRow(
        id = w.id,
        kind = kind,
        name = (if (kind == HomeKind.Worktree) "↳ " else "") + w.shown(),
        detail = when (kind) { HomeKind.Root -> "raíz"; HomeKind.Worktree -> "worktree"; HomeKind.Folder -> "carpeta" } +
            " · ${w.paneCount} ${if (w.paneCount == 1) "panel" else "paneles"}",
        paneCount = w.paneCount,
        mark = top?.mark(),
        unseen = unseen.size,
        line = top?.what(),
        go = top,
    )
}

/** A line of Home's list: a heading, or a workspace's row. */
sealed interface HomeItem {
    data class Heading(val text: String) : HomeItem
    data class Row(val row: HomeRow) : HomeItem
}

/**
 * Home's list as it is drawn (REQ-219 to REQ-221): the daemon sends each repository's root and
 * worktrees together and the folders that are not repositories last. A worktree whose root is
 * not what comes before it gets the repository's name above it, and the first folder gets
 * `carpetas` when something comes before it.
 */
fun homeItems(session: Node.Session, seen: Seen): List<HomeItem> {
    val rows = homeRows(session, seen)
    val items = mutableListOf<HomeItem>()
    var folders = false
    session.workspaces.forEachIndexed { i, w ->
        val before = session.workspaces.getOrNull(i - 1)
        when {
            rows[i].kind == HomeKind.Folder && !folders -> {
                folders = true
                if (before != null) items += HomeItem.Heading("carpetas")
            }
            rows[i].kind == HomeKind.Worktree && w.project != null && before?.project != w.project -> items += HomeItem.Heading(repoName(w.project!!))
        }
        items += HomeItem.Row(rows[i])
    }
    return items
}

fun avisos(session: Node.Session, seen: Seen): List<Aviso> = session.workspaces.flatMap { w ->
    w.attention.filterNot(seen::isSeen).byUrgency().map { Aviso(w.id, w.shown(), it) }
}

/** Where tapping an attention goes: the workspace, and the pane when the attention names one. */
data class Route(val workspace: Id, val pane: Id?)

fun routeOf(session: Node.Session, a: Attention): Route? {
    val owner = session.workspaces.firstOrNull { w -> w.attention.any { it == a } } ?: return null
    return Route(owner.id, a.node.takeIf { it != owner.id })
}
