package dev.jw.next.client

import dev.jw.next.contract.DirListing
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceSummary

/** A path with the PC's home written as `~`. */
fun tildePath(path: String, home: String): String = when {
    path == home -> "~"
    path.startsWith("$home/") -> "~" + path.removePrefix(home)
    else -> path
}

private fun child(dir: String, name: String) = if (dir == "/") "/$name" else "$dir/$name"

/** A folder of the browser: [open] when a workspace of the session is already at its path (REQ-203). */
data class FolderRow(val name: String, val path: String, val git: Boolean, val open: Id?)

/** What the folder browser shows for one reading of a folder. */
data class FolderView(val path: String, val parent: String?, val home: String, val rows: List<FolderRow>, val total: Int, val here: Id?) {
    /** The listing was cut: the PC has more folders than it sent. */
    val cut: Boolean get() = total > rows.size

    /** The folders from the start of [path], each with the path it goes to: `~` then the names under the home, else `/`. */
    val crumbs: List<Pair<String, String>>
        get() {
            val base = if (path == home || path.startsWith("$home/")) home to "~" else "/" to "/"
            val names = path.removePrefix(base.first).split('/').filter { it.isNotEmpty() }
            val paths = names.runningFold(base.first) { at, name -> child(at, name) }
            return listOf(base.second).zip(paths.take(1)) + names.zip(paths.drop(1))
        }
}

fun folderView(listing: DirListing, session: Node.Session): FolderView {
    val open = session.workspaces.associate { it.path to it.id }
    return FolderView(
        listing.path, listing.parent, listing.home,
        listing.dirs.map { FolderRow(it.name, child(listing.path, it.name), it.git, open[child(listing.path, it.name)]) },
        listing.total, open[listing.path],
    )
}

/** The repository a workspace is the root of or a worktree of, or null for a folder that is not one. */
fun WorkspaceSummary.repo(): String? = project

fun repoName(repo: String): String = repo.trimEnd('/').substringAfterLast('/').ifEmpty { "repo" }

/** The repositories to make a worktree of, each once, in the session's order. */
fun repos(session: Node.Session): List<String> = session.workspaces.mapNotNull { it.repo() }.distinct()

/** Where the worktrees of [repo] go: `<repo>-wt`, beside it. */
fun worktreesDir(repo: String): String {
    val trimmed = repo.trimEnd('/')
    return "${trimmed.substringBeforeLast('/', "")}/${repoName(trimmed)}-wt"
}

/** The names a worktree of [repo] cannot take: the folders there and the workspaces already open in it. */
fun takenNames(repo: String, there: DirListing?, session: Node.Session): Set<String> {
    val dir = worktreesDir(repo)
    return there?.dirs.orEmpty().map { it.name }.toSet() +
        session.workspaces.filter { it.path.substringBeforeLast('/') == dir }.map { it.path.substringAfterLast('/') }
}

/** The first `ws-N` not in [taken]. */
fun freeName(taken: Set<String>): String = generateSequence(1) { it + 1 }.map { "ws-$it" }.first { it !in taken }

/** What a name will make, or why it cannot be used (REQ-206, REQ-207). */
sealed interface WorktreePlan {
    data class Ok(val branch: String, val folder: String) : WorktreePlan
    data class Wrong(val why: String) : WorktreePlan
}

fun planWorktree(repo: String, name: String, taken: Set<String>): WorktreePlan {
    val valid = name.isNotEmpty() && name.length <= 40 && name.all { it in 'a'..'z' || it in '0'..'9' || it == '-' } && !name.startsWith('-')
    return when {
        !valid -> WorktreePlan.Wrong("nombre no válido: usa minúsculas, números y guiones")
        name in taken -> WorktreePlan.Wrong("${repoName(repo)}-wt/$name ya existe")
        else -> WorktreePlan.Ok("feat/$name", name)
    }
}
