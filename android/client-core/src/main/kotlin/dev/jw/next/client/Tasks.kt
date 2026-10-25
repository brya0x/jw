package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.LoopStop
import dev.jw.next.contract.Node
import dev.jw.next.contract.StopKind
import dev.jw.next.contract.TaskItem
import dev.jw.next.contract.TaskOp
import dev.jw.next.contract.TaskPhase
import dev.jw.next.contract.TaskStatus
import dev.jw.next.contract.TasksSummary

/**
 * What the phone knows of the task lists (N180g, REQ-948 to REQ-950, REQ-953): the row's progress, the
 * rows of the «tareas» tab, the card of the loop, what a task's sheet offers, the notices of a loop that
 * stopped, and when the node is asked for again. All of it is pure: the screens only draw it.
 */

/** The tab of a workspace that shows its tasks: it is not the id of a pane or of a view (REQ-949). */
const val TASKS_TAB: Id = "@tasks"

/** How many laps the loop's sheet offers, and the one that is chosen first (REQ-949). */
val LOOP_LAPS = listOf(10, 20, 40)
const val DEFAULT_LAPS = 20

/** `3/8`: how far a list is, counting the final task even while it is not in the file. */
fun TasksSummary.progress(): String = "$done/$total"

/** One task of the tab: its level, what it is and waits for, why it is blocked and what was noted under it. */
data class TaskRow(
    val id: String,
    val level: Int,
    val glyph: String,
    /** Null is the ordinary text colour. */
    val tone: Tone?,
    val title: String,
    /** What it waits for, `todas` for the final task. */
    val waits: String?,
    val why: String?,
    val notes: List<String>,
    /** What the row says apart: the evidence that is missing, or that the final task is not in the file yet. */
    val hint: String?,
    /** The loop handed it out and it is not done: a spinner. */
    val running: Boolean,
    val status: TaskStatus,
    val isFinal: Boolean,
)

/** The tasks by level, and in the order of the file within one: the order of work (REQ-949). */
fun taskRows(doc: Node.Tasks): List<TaskRow> =
    doc.tasks.withIndex().sortedWith(compareBy({ it.value.level }, { it.index })).map { it.value.row() }

private fun TaskItem.row(): TaskRow {
    val (glyph, tone) = when (state) {
        TaskPhase.Done -> "✓" to Tone.Good
        TaskPhase.Running -> "▶" to null
        TaskPhase.Ready -> "○" to null
        TaskPhase.Waiting -> "◌" to Tone.Dim
        TaskPhase.Blocked -> "✗" to Tone.Warn
    }
    val hint = when {
        auto -> "se añade al archivo al activar el bucle"
        isFinal && state == TaskPhase.Done && notes.isEmpty() -> "sin evidencia anotada"
        else -> null
    }
    return TaskRow(
        id = id, level = level, glyph = glyph, tone = tone, title = title,
        waits = when {
            after.isEmpty() -> null
            after == listOf("*") -> "todas"
            else -> after.joinToString(", ")
        },
        why = why, notes = notes, hint = hint, running = state == TaskPhase.Running, status = status, isFinal = isFinal,
    )
}

enum class LoopButton { Start, Stop }

/** The card at the top of the tab: how the loop is, and the button that turns it on or off (REQ-949). */
data class LoopCard(
    val title: String,
    val line: String,
    val tone: Tone?,
    val button: LoopButton,
    val done: Int,
    val total: Int,
    /** The loop runs and does not wait for the person: a spinner. */
    val running: Boolean,
)

fun loopCard(doc: Node.Tasks): LoopCard {
    val done = doc.tasks.count { it.state == TaskPhase.Done }
    val lp = doc.loop
    val stop = lp.stop
    return when {
        lp.on -> LoopCard(
            title = "vuelta ${lp.laps}/${lp.max}",
            line = if (lp.waiting) "espera tu respuesta" else "en marcha" + (lp.task?.let { " · $it" } ?: ""),
            tone = if (lp.waiting) Tone.Danger else null,
            button = LoopButton.Stop, done = done, total = doc.tasks.size, running = !lp.waiting,
        )
        stop != null && stop.kind == StopKind.Done ->
            LoopCard("terminado", stop.text, Tone.Good, LoopButton.Start, done, doc.tasks.size, false)
        stop != null && (stop.kind == StopKind.Person || stop.kind == StopKind.Pane) ->
            LoopCard("parado", stop.text, Tone.Dim, LoopButton.Start, done, doc.tasks.size, false)
        stop != null -> LoopCard("parado", stop.text, Tone.Warn, LoopButton.Start, done, doc.tasks.size, false)
        else -> LoopCard("bucle apagado", "la lista no corre sola: actívala", Tone.Dim, LoopButton.Start, done, doc.tasks.size, false)
    }
}

/** What the sheet of a task offers (REQ-949): done and block while it waits, done or reopen while it is blocked, reopen when done. */
enum class TaskAction(val label: String, val op: TaskOp, val needsText: Boolean) {
    Done("hecha", TaskOp.Done, false),
    Block("bloquear…", TaskOp.Block, true),
    Reopen("reabrir", TaskOp.Reopen, false),
}

fun taskActions(t: TaskRow): List<TaskAction> = when (t.status) {
    TaskStatus.Todo -> listOf(TaskAction.Done, TaskAction.Block)
    TaskStatus.Blocked -> listOf(TaskAction.Done, TaskAction.Reopen)
    TaskStatus.Done -> listOf(TaskAction.Reopen)
}

/** A loop that stopped or ended, for the banner (REQ-950). */
data class TasksNotice(val workspace: Id, val text: String, val good: Boolean)

/**
 * What the sessions say of the loops, from one session to the next (REQ-950): a stop that is new is a notice, once;
 * the first session of a connection says nothing, since what stopped before the phone came is not news.
 */
class TasksNotices {
    private var last: Map<Id, LoopStop?>? = null

    fun update(session: Node.Session): List<TasksNotice> {
        val now = session.workspaces.associate { it.id to it.tasks?.stop }
        val before = last
        last = now
        if (before == null) return emptyList()
        return session.workspaces.mapNotNull { w ->
            val stop = w.tasks?.stop ?: return@mapNotNull null
            // A loop the person turned off was seen by whoever pressed the button (REQ-928).
            if (before[w.id] == stop || stop.kind == StopKind.Person) null
            else TasksNotice(w.id, "${w.shown()}: ${stop.text}", stop.kind == StopKind.Done)
        }
    }
}

/**
 * When the tab asks for its node: when it is not there while the row says there is a list, and again, not at every
 * recomposition, when it goes (REQ-958).
 */
class TasksWatch(private val againMs: Long = 3000) {
    private var askedAt: Long? = null

    fun want(have: Boolean, listed: Boolean, nowMs: Long): Boolean {
        if (have) {
            askedAt = null
            return false
        }
        val at = askedAt
        if (!listed || (at != null && nowMs < at + againMs)) return false
        askedAt = nowMs
        return true
    }
}
