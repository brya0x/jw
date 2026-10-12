package dev.jw.mobile

import dev.jw.proto.DiffFile

/** One row of the file tree of a view (REQ-89): a folder, or a file under it. */
internal sealed interface TreeRow {
    /** A folder, or a chain of folders with one child each shown as one row. [key] is its path with a trailing `/`. */
    class Dir(val key: String, val label: String, val depth: Int, val files: Int, val added: Int, val deleted: Int, val closed: Boolean) : TreeRow

    class File(val file: DiffFile, val depth: Int) : TreeRow
}

/** Folders before files at every level, each by name: the order of the rows and of "archivo n/m". */
internal fun treeSorted(files: List<DiffFile>): List<DiffFile> = files.sortedWith { a, b -> treeCompare(a.path, b.path) }

private fun treeCompare(a: String, b: String): Int {
    val x = a.split('/')
    val y = b.split('/')
    var i = 0
    while (i < x.size && i < y.size) {
        val dirX = i < x.size - 1
        val dirY = i < y.size - 1
        if (dirX != dirY) return if (dirX) -1 else 1
        if (x[i] != y[i]) return x[i].compareTo(y[i])
        i++
    }
    return x.size.compareTo(y.size)
}

/** The rows of [files] (already in [treeSorted] order) without what a folder in [closed] hides. */
internal fun treeRows(files: List<DiffFile>, closed: Set<String>): List<TreeRow> {
    val parts = files.map { it.path.split('/') }
    val out = ArrayList<TreeRow>()

    fun walk(from: Int, to: Int, base: Int, depth: Int, hidden: Boolean) {
        var i = from
        while (i < to) {
            if (parts[i].size == base + 1) {
                if (!hidden) out += TreeRow.File(files[i], depth)
                i++
                continue
            }
            var end = i
            while (end < to && parts[end].size > base + 1 && parts[end][base] == parts[i][base]) end++
            // A folder with no file of its own and one folder in it joins the row below.
            var k = 1
            while ((i until end).all { parts[it].size > base + k + 1 && parts[it][base + k] == parts[i][base + k] }) k++
            val key = parts[i].subList(0, base + k).joinToString("/") + "/"
            val shut = key in closed
            if (!hidden) {
                val group = files.subList(i, end)
                out += TreeRow.Dir(key, parts[i].subList(base, base + k).joinToString("/"), depth, group.size, group.sumOf { it.added }, group.sumOf { it.deleted }, shut)
            }
            walk(i, end, base + k, depth + 1, hidden || shut)
            i = end
        }
    }
    walk(0, files.size, 0, 0, false)
    return out
}
