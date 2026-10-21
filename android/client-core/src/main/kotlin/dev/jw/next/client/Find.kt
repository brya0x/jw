package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

/** A row of the search: a workspace as the prototype's sheet draws it (REQ-231). */
data class FindRow(val row: HomeRow, val path: String)

/**
 * The workspaces of [session] for the search (REQ-232): the ones opened on this phone first, the
 * latest first ([recent]), then the rest in the order of Home's list; and, when [query] is not
 * blank, only those whose name, branch, repository or path contain it, without regard to case.
 * A worktree is named `repo/branch`, as the prototype does.
 */
fun findWorkspaces(session: Node.Session, seen: Seen, recent: List<Id>, query: String): List<FindRow> {
    val rows = homeRows(session, seen)
    val byId = session.workspaces.zip(rows).associateBy { it.first.id }
    val ordered = recent.mapNotNull { byId[it] } + session.workspaces.mapNotNull { w -> byId[w.id]?.takeIf { w.id !in recent } }
    val q = query.trim().lowercase()
    return ordered
        .filter { (w, row) -> q.isEmpty() || listOfNotNull(row.name, w.branch, w.project?.let(::repoName), w.path).any { q in it.lowercase() } }
        .map { (w, row) ->
            val name = if (row.kind == HomeKind.Worktree) w.project?.let { repoName(it) + "/" }.orEmpty() + row.name else row.name
            FindRow(row.copy(name = name), w.path)
        }
}
