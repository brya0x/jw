package dev.jw.mobile

import dev.jw.term.Attr
import dev.jw.term.Line

/**
 * What `ls` prints in columns for a wide terminal, laid out again for the phone's width (REQ-21). `ls` fills its columns
 * downwards and pads each to the same width, so a row cut at the phone's width reads in the wrong order; here the names are
 * taken in the order `ls` meant, and set in columns of the phone's width the way `ls` would. A block of rows is such a grid
 * when every row but the last has the same number of names (three at least), they start at the same columns, and one of
 * them is wider than the view; anything else is [wrapView]'s as it was.
 */
internal object ColumnGrid {
    private const val GAP = 2
    private const val MIN_ITEMS = 3

    /** The rows of [t] for a view [viewCols] wide: [wrapView]'s, with the grids set again. */
    fun rows(t: PaneText, viewCols: Int): List<VRow> {
        val wrapped = wrapView(t, viewCols, false).rows
        val grids = find(t, viewCols)
        if (grids.isEmpty()) return wrapped
        val out = ArrayList<VRow>(wrapped.size)
        val done = HashSet<Grid>()
        for (r in wrapped) {
            val g = grids.firstOrNull { r.line in it.first..it.last }
            if (g == null) out.add(r) else if (done.add(g)) out.addAll(g.rows)
        }
        return out
    }

    private class Grid(val first: Int, val last: Int, val rows: List<VRow>)

    /** One name in a row: the cells it takes, and where it starts. */
    private class Item(val start: Int, val cells: IntArray)

    private fun find(t: PaneText, viewCols: Int): List<Grid> {
        val found = ArrayList<Grid>()
        val items = ArrayList<List<Item>?>()
        for (li in 0 until t.rows) items.add(itemsOf(t.line(li)))
        var li = 0
        while (li < items.size) {
            if (items[li] == null) { li++; continue }
            var end = li
            while (end + 1 < items.size && items[end + 1] != null) end++
            grid(t, li, end, items, viewCols)?.let { found.add(it) }
            li = end + 1
        }
        return found
    }

    private fun grid(t: PaneText, from: Int, to: Int, items: List<List<Item>?>, viewCols: Int): Grid? {
        val rows = (from..to).map { items[it]!! }
        val per = rows.first().size
        if (per < MIN_ITEMS || rows.dropLast(1).any { it.size != per } || rows.last().size > per) return null
        val starts = rows.first().map { it.start }
        if (rows.any { r -> r.indices.any { r[it].start != starts[it] } }) return null
        val wide = (from..to).any { li -> t.line(li).let { l -> RowWrap.width(l, cellsOf(l)) } > viewCols }
        if (!wide) return null
        // The names in the order `ls` meant: down the first column, then the next.
        val ordered = ArrayList<Pair<Line, Item>>()
        for (c in 0 until per) for ((k, r) in rows.withIndex()) if (c < r.size) ordered.add(t.line(from + k) to r[c])
        val width = ordered.maxOf { (l, it) -> RowWrap.width(l, it.cells) }
        val colw = width + GAP
        val across = ((viewCols + GAP) / colw).coerceAtLeast(1)
        if (width > viewCols) return null
        val height = (ordered.size + across - 1) / across
        val lines = Array(height) { Line(viewCols) }
        val used = IntArray(height)
        for ((n, p) in ordered.withIndex()) {
            val (src, item) = p
            val r = n % height
            var x = (n / height) * colw
            for (c in item.cells) {
                if (x >= viewCols) break
                val dst = lines[r]
                dst.ch[x] = src.ch[c]; dst.fg[x] = src.fg[c]; dst.bg[x] = src.bg[c]; dst.flags[x] = src.flags[c]
                src.marksAt(c)?.forEach { dst.addMark(x, it.code) }
                x++
            }
            used[r] = maxOf(used[r], x)
        }
        // A wide character's second cell is a continuation: leave it out of the cells drawn.
        val out = lines.mapIndexed { r, l -> VRow(from, (0 until used[r]).filter { l.flags[it] and Attr.CONT == 0 }.toIntArray(), l) }
        return Grid(from, to, out)
    }

    private fun cellsOf(l: Line): IntArray {
        var used = 0
        for (c in l.cols - 1 downTo 0) if ((l.ch[c] != 0 && l.ch[c] != ' '.code) || l.bg[c] != 0) { used = c + 1; break }
        return (0 until used).filter { l.flags[it] and Attr.CONT == 0 }.toIntArray()
    }

    /** The names of a row of plain text, or null when it is not one: blank, with a background, or with fewer than two names. */
    private fun itemsOf(l: Line): List<Item>? {
        val cells = cellsOf(l)
        if (cells.isEmpty() || cells.any { l.bg[it] != 0 }) return null
        fun blank(c: Int) = l.ch[c] == 0 || l.ch[c] == ' '.code
        val out = ArrayList<Item>()
        var i = 0
        while (i < cells.size) {
            if (blank(cells[i])) { i++; continue }
            var j = i
            var e = i
            // A name ends at the first gap of two blanks or more.
            while (j < cells.size) {
                if (!blank(cells[j])) { e = j; j++; continue }
                if (j + 1 >= cells.size || blank(cells[j + 1])) break
                j++
            }
            out.add(Item(cells[i], cells.copyOfRange(i, e + 1)))
            i = e + 1
        }
        return out.takeIf { it.size >= 2 }
    }
}
