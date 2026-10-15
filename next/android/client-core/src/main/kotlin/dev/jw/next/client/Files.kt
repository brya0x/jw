package dev.jw.next.client

import dev.jw.next.contract.FileKind
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.GitMark

/** The tone of a mark: the app draws each in its own colour. */
enum class MarkTone { Changed, New, Bad, Ignored }

/** What is drawn at the right of a row for what git says of it. */
data class FileMark(val glyph: String, val tone: MarkTone)

/** A row of the files screen; [path] is relative to the workspace's folder. */
data class FileRow(val name: String, val path: String, val kind: FileKind, val size: Long?, val git: GitMark?, val ignored: Boolean) {
    /** What git says of it, or that it ignores it (`◌`), for the right of the row. */
    val mark: FileMark?
        get() = when {
            git == GitMark.Conflict -> FileMark("!", MarkTone.Bad)
            git == GitMark.Modified -> FileMark("M", MarkTone.Changed)
            git == GitMark.Deleted -> FileMark("D", MarkTone.Bad)
            git == GitMark.Added -> FileMark("A", MarkTone.New)
            git == GitMark.Untracked -> FileMark("?", MarkTone.New)
            ignored -> FileMark("◌", MarkTone.Ignored)
            else -> null
        }

    /** The second line of a file's row: its size; a link says so; a folder says nothing, the PC does not count what is in it. */
    val detail: String?
        get() = when (kind) {
            FileKind.File -> size?.let(::formatSize)
            FileKind.Link -> "enlace"
            FileKind.Dir -> null
        }
}

/** A size as the files screen writes it: bytes, kilobytes (at least one) or megabytes. */
fun formatSize(bytes: Long): String = when {
    bytes < 1024 -> "$bytes B"
    bytes < 1024 * 1024 -> "${maxOf(1, (bytes + 512) / 1024)} KB"
    else -> "%.1f MB".format(java.util.Locale.ROOT, bytes / (1024.0 * 1024.0))
}

/**
 * What the files screen shows for one reading of a folder of a workspace (REQ-323 to REQ-328): the
 * entries as the PC sent them (folders first), without those that begin with a dot unless [hidden],
 * the way back up, and the crumbs from the workspace's folder, which is as high as it goes.
 */
data class FilesView(val workspace: String, val path: String, val rows: List<FileRow>, val total: Int, val sent: Int, val hidden: Boolean) {
    /** The folder above, or null at the workspace's own folder: the walk goes no higher. */
    val parent: String? get() = if (path.isEmpty()) null else path.substringBeforeLast('/', "")

    /** The PC has more entries than it sent. */
    val cut: Boolean get() = total > sent

    /** Where the screen says it is: `/` for the workspace's folder, else its path under it. */
    val where: String get() = "/$path"

    val crumbs: List<Pair<String, String>> get() = crumbsOf(workspace, path)
}

/** The workspace's name and then each folder down to [path], each with the path it goes to. */
fun crumbsOf(workspace: String, path: String): List<Pair<String, String>> {
    val names = path.split('/').filter { it.isNotEmpty() }
    val paths = names.runningFold("") { at, name -> if (at.isEmpty()) name else "$at/$name" }
    return listOf(workspace to paths.first()) + names.zip(paths.drop(1))
}

fun filesView(workspaceName: String, listing: FileListing, hidden: Boolean): FilesView {
    val rows = listing.entries
        .filter { hidden || !it.name.startsWith('.') }
        .map { FileRow(it.name, if (listing.path.isEmpty()) it.name else "${listing.path}/${it.name}", it.kind, it.size, it.git, it.ignored) }
    return FilesView(workspaceName, listing.path, rows, listing.total, listing.entries.size, hidden)
}
