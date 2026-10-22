package dev.jw.next.client

import dev.jw.next.contract.Group
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

/** One kind of attention a group's workspaces have, with how many (REQ-692). */
data class GroupMark(val mark: Mark, val count: Int)

/**
 * What the workspaces of a group say, as a folded row draws it (REQ-692): how many work (a
 * conversation thinks, or a worktree is being removed) and each kind of unseen attention they have,
 * the most urgent first.
 */
fun groupSays(session: Node.Session, seen: Seen, group: Id): Pair<Int, List<GroupMark>> {
    val mine = session.workspaces.filter { it.group == group }
    val running = mine.count { it.working || it.removing }
    val marks = linkedMapOf<Mark, Int>()
    mine.flatMap { it.attention.filterNot(seen::isSeen) }.byUrgency().forEach { a ->
        marks.merge(a.mark(), 1, Int::plus)
    }
    return running to marks.map { (mark, count) -> GroupMark(mark, count) }
}

/** The most characters a group's name has (REQ-686); the PC refuses more and an empty one. */
const val MAX_GROUP_NAME = 40

/** Why [name] is not a group's name, or null when it is (the PC decides it too, REQ-686). */
fun groupNameError(name: String, groups: List<Group>, except: Id? = null): String? {
    val clean = name.trim()
    return when {
        clean.isEmpty() -> "el grupo necesita un nombre"
        clean.length > MAX_GROUP_NAME -> "el nombre pasa de $MAX_GROUP_NAME letras"
        groups.any { it.id != except && it.name.equals(clean, ignoreCase = true) } -> "ya hay un grupo «$clean»"
        else -> null
    }
}

/** What the sheet «mover a un grupo…» of a workspace needs (REQ-698). */
data class MoveTo(
    val workspace: Id,
    val shown: String,
    /** The group its repository is in now, if any. */
    val current: Group?,
    /** How many workspaces move with it: its whole repository (REQ-688). */
    val together: Int,
    val groups: List<Group>,
) {
    /** What moving takes: the repository whole, or only this workspace. */
    val scope: String
        get() = if (together > 1) "mueve la raíz y sus ${together - 1} worktree${if (together > 2) "s" else ""} ($together en total)" else "mueve solo este workspace"

    /** Where it is now, then what moving takes: the note of the menu's line. */
    val note: String get() = (current?.let { "ahora en «${it.name}»" } ?: "ahora sin grupo") + " · " + scope
}

fun Node.Session.moveTo(workspace: Id): MoveTo? {
    val w = workspaces.firstOrNull { it.id == workspace } ?: return null
    val together = if (w.project != null) workspaces.count { it.project == w.project } else 1
    return MoveTo(w.id, w.shown(), groups.firstOrNull { it.id == w.group }, together.coerceAtLeast(1), groups)
}

/** A chip of the filter of Avisos (REQ-699): [filter] is null for all, [NO_GROUP] for those in none, or a group's id. */
data class AvisoChip(val filter: Id?, val label: String, val count: Int)

/** The filter value of the alerts of workspaces that are in no group. */
const val NO_GROUP = ""

/** The chips over the list of alerts: all, each group with its count, and «sin grupo» when some alert is in none. */
fun avisoChips(session: Node.Session, items: List<Aviso>): List<AvisoChip> =
    listOf(AvisoChip(null, "todos", items.size)) +
        session.groups.map { g -> AvisoChip(g.id, g.name, items.count { it.group == g.id }) } +
        (if (session.groups.isNotEmpty() && items.any { it.group == null }) listOf(AvisoChip(NO_GROUP, "sin grupo", items.count { it.group == null })) else emptyList())

/** The alerts the filter lets through: every one when it is null. */
fun List<Aviso>.filtered(by: Id?): List<Aviso> = when (by) {
    null -> this
    NO_GROUP -> filter { it.group == null }
    else -> filter { it.group == by }
}
