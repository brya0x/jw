package dev.jw.next.client

import dev.jw.next.contract.DiffFile
import dev.jw.next.contract.Hunk

/**
 * Folders before the files of the folder they are in, folders and files by name: the order a
 * tree lists them in, and the order the changed files are shown in.
 */
fun treeCompare(a: String, b: String): Int {
    val x = a.split('/')
    val y = b.split('/')
    var i = 0
    while (true) {
        if (i >= x.size || i >= y.size) return x.size.compareTo(y.size)
        val dx = i < x.size - 1
        val dy = i < y.size - 1
        if (dx != dy) return if (dx) -1 else 1
        if (x[i] != y[i]) return x[i].compareTo(y[i])
        i++
    }
}

/** A row of the list of changed files: a folder (a chain of single-child folders is one row) or a file. */
sealed interface TreeRow {
    /** [key] is the folder path with a trailing slash, the thing folding is remembered by. */
    data class Dir(
        val key: String,
        val label: String,
        val depth: Int,
        val files: Int,
        val added: Int,
        val deleted: Int,
        val shut: Boolean,
    ) : TreeRow

    data class File(val file: DiffFile, val depth: Int) : TreeRow
}

/** The rows of [files] in tree order; a folder that [shut] says is folded hides what is under it. */
fun treeRows(files: List<DiffFile>, shut: (String) -> Boolean): List<TreeRow> {
    val sorted = files.sortedWith { a, b -> treeCompare(a.path, b.path) }
    val parts = sorted.map { it.path.split('/') }
    val out = mutableListOf<TreeRow>()

    fun walk(lo: Int, hi: Int, base: Int, depth: Int, hidden: Boolean) {
        var i = lo
        while (i < hi) {
            if (parts[i].size == base + 1) {
                if (!hidden) out += TreeRow.File(sorted[i], depth)
                i++
                continue
            }
            var end = i
            while (end < hi && parts[end].size > base + 1 && parts[end][base] == parts[i][base]) end++
            var chain = 1
            while ((i until end).all { parts[it].size > base + chain + 1 && parts[it][base + chain] == parts[i][base + chain] }) chain++
            val key = parts[i].subList(0, base + chain).joinToString("/") + "/"
            val folded = shut(key)
            if (!hidden) {
                val group = sorted.subList(i, end)
                out += TreeRow.Dir(
                    key = key,
                    label = parts[i].subList(base, base + chain).joinToString("/"),
                    depth = depth,
                    files = group.size,
                    added = group.sumOf { it.added },
                    deleted = group.sumOf { it.deleted },
                    shut = folded,
                )
            }
            walk(i, end, base + chain, depth + 1, hidden || folded)
            i = end
        }
    }
    walk(0, sorted.size, 0, 0, false)
    return out
}

enum class LineKind { Add, Del, Ctx }

/** What the file screen draws: a hunk's header, then its numbered lines. */
sealed interface HunkRow {
    data class Head(val text: String) : HunkRow
    data class Line(val kind: LineKind, val old: Int?, val new: Int?, val text: String) : HunkRow
}

/** The lines of [file]'s hunks, numbered from each hunk's starts. */
fun hunkRows(file: DiffFile): List<HunkRow> = file.hunks.flatMap { hunk ->
    var old = hunk.oldStart
    var new = hunk.newStart
    listOf<HunkRow>(HunkRow.Head(hunkHead(hunk))) + hunk.lines.map { line ->
        val text = line.drop(1)
        when (line.firstOrNull()) {
            '+' -> HunkRow.Line(LineKind.Add, null, new++, text)
            '-' -> HunkRow.Line(LineKind.Del, old++, null, text)
            else -> HunkRow.Line(LineKind.Ctx, old++, new++, text)
        }
    }
}

private fun hunkHead(h: Hunk): String =
    "@@ -${h.oldStart},${h.oldLines} +${h.newStart},${h.newLines} @@ ${h.context}".trimEnd()

/** How long ago a Unix time was, in the words a commit's row says it. */
fun ago(nowSecs: Long, thenSecs: Long): String {
    val secs = (nowSecs - thenSecs).coerceAtLeast(0)
    return when {
        secs < 60 -> "ahora"
        secs < 3600 -> "hace ${secs / 60} min"
        secs < 86400 -> "hace ${secs / 3600} h"
        else -> "hace ${secs / 86400} d"
    }
}

/** Why a file has no lines to show, in the words the screen says it with. */
fun whyUnread(reason: String): String = when (reason) {
    "symlink" -> "enlace simbólico (no se sigue)"
    "too_big" -> "demasiado grande"
    "special" -> "no es un archivo normal"
    "repository" -> "es otro repositorio"
    "gone" -> "ya no está"
    else -> reason
}
