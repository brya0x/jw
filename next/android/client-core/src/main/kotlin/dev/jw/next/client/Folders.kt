package dev.jw.next.client

import dev.jw.next.contract.DirListing
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.ProjectInfo
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

/** The names a worktree cannot take in [root]: the folders there and the workspaces already open in it (REQ-350). */
fun takenNames(root: String, there: DirListing?, session: Node.Session): Set<String> {
    val dir = root.trimEnd('/')
    return there?.dirs.orEmpty().map { it.name }.toSet() +
        session.workspaces.filter { it.path.substringBeforeLast('/') == dir }.map { it.path.substringAfterLast('/') }
}

/** The first `ws-N` not in [taken]. */
fun freeName(taken: Set<String>): String = generateSequence(1) { it + 1 }.map { "ws-$it" }.first { it !in taken }

/** What the sheet knows of the repository's `.jw.toml`: the PC's answer to `project_config` (REQ-347, REQ-348). */
sealed interface WorktreeSettings {
    data object Reading : WorktreeSettings
    data class Ready(val info: ProjectInfo) : WorktreeSettings
    data class Refused(val why: String) : WorktreeSettings
}

/** What a name will make, or why it cannot be used (REQ-210, REQ-211, REQ-347 to REQ-349). */
sealed interface WorktreePlan {
    /** [place] is the last folder of the settings' `root` and the name; [info] says the setup and the panes. */
    data class Ok(val branch: String, val folder: String, val place: String, val info: ProjectInfo) : WorktreePlan
    data object Reading : WorktreePlan
    data class Wrong(val why: String) : WorktreePlan
}

fun planWorktree(settings: WorktreeSettings, name: String, taken: Set<String>): WorktreePlan {
    val info = when (settings) {
        WorktreeSettings.Reading -> return WorktreePlan.Reading
        is WorktreeSettings.Refused -> return WorktreePlan.Wrong(settings.why)
        is WorktreeSettings.Ready -> settings.info
    }
    val valid = name.isNotEmpty() && name.length <= 40 && name.all { it in 'a'..'z' || it in '0'..'9' || it == '-' } && !name.startsWith('-')
    val place = "${repoName(info.root)}/$name"
    return when {
        !valid -> WorktreePlan.Wrong("nombre no válido: usa minúsculas, números y guiones")
        name in taken -> WorktreePlan.Wrong("$place ya existe")
        else -> WorktreePlan.Ok(info.branch.replace("{name}", name), name, place, info)
    }
}

/**
 * A worktree just made gets the panes of its repository (REQ-351); when the PC refuses [start], it gets one shell and
 * [reply] says so with the PC's message (REQ-352).
 */
fun worktreeStarted(
    made: Result<Id>,
    start: (Id, (Result<Unit>) -> Unit) -> Unit,
    shell: (Id, (Result<Id>) -> Unit) -> Unit,
    reply: (Result<Id>) -> Unit,
) {
    val workspace = made.getOrElse { return reply(made) }
    start(workspace) { started ->
        if (started.isSuccess) return@start reply(made)
        val why = started.exceptionOrNull()?.message ?: "el PC no pudo abrir los paneles"
        shell(workspace) { pane ->
            reply(Result.failure(Exception(if (pane.isSuccess) "se abrió con un shell, pero $why" else "se abrió, pero sin paneles ni shell: $why")))
        }
    }
}
