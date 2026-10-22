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

/** What an attention asks of the person; the UI colours the line that says it by this (REQ-247). */
enum class Need { Decide, Failed, Review }

fun Attention.need(): Need = when (kind) {
    AttentionKind.Permission, AttentionKind.Question, AttentionKind.Plan -> Need.Decide
    AttentionKind.CiFailed -> Need.Failed
    AttentionKind.ShellDone -> if ((exitCode ?: 0) != 0) Need.Failed else Need.Review
    AttentionKind.Screen, AttentionKind.AgentDone, AttentionKind.Merged -> Need.Review
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
enum class HomeKind(val label: String) { Root("raíz"), Worktree("worktree"), Folder("carpeta") }

/** One row of Home (REQ-33). */
data class HomeRow(
    val id: Id,
    val kind: HomeKind,
    /** Its place in the list, from 1, for the number before the row. */
    val number: Int,
    val name: String,
    val detail: String,
    val paneCount: Int,
    /** The mark of the most urgent unseen attention, if any. */
    val mark: Mark?,
    /** How many unseen attentions, for the count beside the mark. */
    val unseen: Int,
    /** The line under the row's detail and the attention a tap on it opens. */
    val line: String?,
    val go: Attention?,
    /** A conversation of the workspace works: a spinner beside the mark and a line that says so (REQ-579). */
    val working: Boolean = false,
    /** Its worktree is being removed: a spinner, the name dim, and nothing acts on it (REQ-660). */
    val removing: Boolean = false,
    /** Its removal failed and the job was not dismissed: a red cross; the cause is in the jobs (REQ-660). */
    val removalFailed: Boolean = false,
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

/**
 * A folder is a root when it says so (`project` is set) or, for a daemon that does not say it,
 * when a worktree names it as its repository.
 */
internal fun Node.Session.homeKind(w: WorkspaceSummary): HomeKind =
    homeKind(w.kind, w.project, namedAsRepo = workspaces.any { it.kind == WorkspaceKind.Worktree && it.project == w.path })

internal fun homeKind(kind: WorkspaceKind, project: String?, namedAsRepo: Boolean): HomeKind = when {
    kind == WorkspaceKind.Worktree -> HomeKind.Worktree
    project != null || namedAsRepo -> HomeKind.Root
    else -> HomeKind.Folder
}

fun homeRows(session: Node.Session, seen: Seen): List<HomeRow> = session.workspaces.mapIndexed { i, w ->
    val unseen = w.attention.filterNot(seen::isSeen).byUrgency()
    val top = unseen.firstOrNull()
    val kind = session.homeKind(w)
    HomeRow(
        id = w.id,
        kind = kind,
        number = i + 1,
        name = w.shown(),
        detail = "${kind.label} · ${w.paneCount} ${if (w.paneCount == 1) "panel" else "paneles"}",
        paneCount = w.paneCount,
        mark = top?.mark(),
        unseen = unseen.size,
        line = top?.what(),
        go = top,
        working = w.working,
        removing = w.removing,
        removalFailed = !w.removing && removalFailed(session, w.id),
    )
}

/** A line of Home's list: a heading, or a workspace's row. */
sealed interface HomeItem {
    /** What keeps the line in place in a list. */
    val key: String

    data class Heading(val text: String) : HomeItem {
        override val key get() = "heading:$text"
    }

    data class Row(val row: HomeRow) : HomeItem {
        override val key get() = row.id
    }
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
        val project = w.project
        // What comes before is this worktree's root or another worktree of its repository.
        val follows = before != null && (before.project == project || before.path == project)
        when {
            rows[i].kind == HomeKind.Folder && !folders -> {
                folders = true
                if (before != null) items += HomeItem.Heading("carpetas")
            }
            rows[i].kind == HomeKind.Worktree && project != null && !follows -> items += HomeItem.Heading(repoName(project))
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
