package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.Job
import dev.jw.next.contract.JobKind
import dev.jw.next.contract.JobState
import dev.jw.next.contract.Node

/** The two tabs of Avisos: what waits for the person, and what runs and ran (REQ-659). */
enum class AvisosTab { Alertas, Trabajos }

/** What a job is called in the list. */
fun Job.title(): String = when (kind) {
    JobKind.RemoveWorkspace -> "quitar worktree $name"
    JobKind.SyncWorkspace -> "subir $name"
}

/** What a job is, under its title. */
fun Job.kindLabel(): String = when (kind) {
    JobKind.RemoveWorkspace -> "quitar worktree"
    JobKind.SyncWorkspace -> "subir rama"
}

/** How long something took, as the terminal client says it: `4,2 s`, `38 s`, `3 min`. */
fun took(ms: Long): String = when {
    ms < 10_000 -> "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0).replace('.', ',')
    ms < 120_000 -> "${Math.round(ms / 1000.0)} s"
    else -> "${ms / 60_000} min"
}

/**
 * How long a job runs at [now] (the PC's clock and the phone's can differ, so never below zero,
 * RISK-76), or how long it took.
 */
fun Job.time(now: Long): Long = durationMs ?: maxOf(0L, now - startedAt)

/** How long ago a job that ended did, at [now]; null while it runs. */
fun Job.ago(now: Long): Long? = durationMs?.let { maxOf(0L, now - startedAt - it) }

/** What the sheet of a job says of it: what can be done with a job that runs, the result of one that ended, or why it failed. */
fun Job.said(): String = when (state) {
    JobState.Running ->
        if (cancellable) "Se puede cancelar sin perder nada." else "No se cancela: dejaría el worktree a medias."
    JobState.Done -> when (kind) {
        JobKind.RemoveWorkspace -> "quitado · la rama se queda"
        JobKind.SyncWorkspace -> "subido $name"
    }
    JobState.Failed -> detail ?: "falló sin decir por qué"
}

/** The jobs of the session as Avisos lists them: those that run, then those that ended, each newest first. */
data class JobsList(val running: List<Job>, val ended: List<Job>)

fun jobsOf(session: Node.Session): JobsList {
    val (running, ended) = session.jobs.partition { it.state == JobState.Running }
    return JobsList(running, ended)
}

/** The mark beside the bell (REQ-659): how many jobs run, else what ended that this phone has not shown. */
sealed interface JobsMark {
    data object None : JobsMark
    data class Running(val count: Int) : JobsMark
    data class Ended(val failed: Boolean, val count: Int) : JobsMark
}

fun jobsMark(session: Node.Session, seen: Seen): JobsMark {
    val (running, ended) = jobsOf(session)
    if (running.isNotEmpty()) return JobsMark.Running(running.size)
    val unseen = ended.filterNot { seen.isJobSeen(it.id) }
    return if (unseen.isEmpty()) JobsMark.None else JobsMark.Ended(unseen.any { it.state == JobState.Failed }, unseen.size)
}

/** The state each job of the session is in, to tell later which ones ended. */
fun jobStates(session: Node.Session): Map<Id, JobState> = session.jobs.associate { it.id to it.state }

/**
 * The removals that ended since [before] was taken: those that were running and are not, and those that
 * were not there at all and are over (a clean worktree can go between two looks): what a notice says
 * (REQ-661). A phone that has not looked yet has no [before], and says nothing of what was over when it came.
 */
fun endedRemovals(before: Map<Id, JobState>, session: Node.Session): List<Job> =
    session.jobs.filter { it.kind == JobKind.RemoveWorkspace && it.state != JobState.Running && before[it.id] != it.state }

/** The notice that says how a removal ended. */
fun Job.endNotice(): String = when (state) {
    JobState.Done -> "✓ quitado $name · ${took(durationMs ?: 0)}"
    else -> "✗ no se pudo quitar $name"
}

/** A removal that failed and was not dismissed: the row of the workspace says so (REQ-660). */
fun removalFailed(session: Node.Session, workspace: Id): Boolean =
    session.jobs.any { it.kind == JobKind.RemoveWorkspace && it.state == JobState.Failed && it.workspace == workspace }
