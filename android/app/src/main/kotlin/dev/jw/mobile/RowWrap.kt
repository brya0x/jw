package dev.jw.mobile

import dev.jw.term.Attr
import dev.jw.term.Line

/** A row of the view: the terminal columns of [line] drawn on it, left to right (the second half of a wide character is not one). */
internal class VRow(val line: Int, val cols: IntArray)

/**
 * How a terminal row too wide for the view is drawn (REQ-21). A row that fits is drawn as it is. A wider one is not
 * cut at the view's width, which breaks words and leaves the gaps a program laid out for a wide terminal (`ls` in
 * columns): its long runs of spaces shrink, and it breaks at spaces. A line of box-drawing is one row, cut at the width.
 */
internal object RowWrap {
    /** A run of this many spaces or more is shrunk to [SQUEEZE_TO]; the indentation at the start keeps [INDENT_MAX]. */
    private const val SQUEEZE_FROM = 4
    private const val SQUEEZE_TO = 2
    private const val INDENT_MAX = 6
    private const val RULE_CHARS = "─━═╌┄│┃╭╮╰╯┌┐└┘├┤┬┴┼"
    private const val NBSP = 0xA0

    fun span(l: Line, c: Int) = if (l.flags[c] and Attr.WIDE != 0) 2 else 1

    /** The width the columns [cols] of [l] take. */
    fun width(l: Line, cols: IntArray) = cols.sumOf { span(l, it) }

    /** The rows [l] (row [line], its first [used] columns) is drawn as on a view [viewCols] wide; [keep] is a column that must stay (the cursor), or -1. */
    fun rows(line: Int, l: Line, used: Int, viewCols: Int, keep: Int): List<VRow> {
        val cells = (0 until used).filter { l.flags[it] and Attr.CONT == 0 }.toIntArray()
        if (width(l, cells) <= viewCols) return listOf(VRow(line, cells))
        if (isRule(l, cells)) return listOf(VRow(line, cut(l, cells, viewCols)))
        return wrap(line, l, squeeze(l, cells, keep), viewCols)
    }

    /** The terminal column under [x] cells from the left of [row]: its last when [x] is past the end. */
    fun colAt(l: Line, row: VRow, x: Int): Int {
        var at = 0
        for (c in row.cols) {
            at += span(l, c)
            if (x < at) return c
        }
        return row.cols.lastOrNull() ?: 0
    }

    /** How many cells from the left of [row] the column [col] starts at, or -1 when the row does not draw it. */
    fun xOf(l: Line, row: VRow, col: Int): Int {
        var at = 0
        for (c in row.cols) {
            if (c == col) return at
            at += span(l, c)
        }
        return -1
    }

    private fun isSpace(l: Line, c: Int) = (l.ch[c] == 0 || l.ch[c] == ' '.code || l.ch[c] == NBSP) && l.bg[c] == 0

    /** A line of box-drawing: it starts with a rule and is mostly rules (a label in it, as Claude's borders carry, does not make it text). */
    private fun isRule(l: Line, cells: IntArray): Boolean {
        val marks = cells.filter { !isSpace(l, it) }
        if (marks.size < 8 || l.ch[marks.first()].toChar() !in RULE_CHARS) return false
        return marks.count { l.ch[it] in 0..0xFFFF && l.ch[it].toChar() in RULE_CHARS } * 10 >= marks.size * 6
    }

    private fun cut(l: Line, cells: IntArray, viewCols: Int): IntArray {
        var w = 0
        var n = 0
        for (c in cells) {
            if (w + span(l, c) > viewCols) break
            w += span(l, c)
            n++
        }
        return cells.copyOf(n)
    }

    /** [cells] with each long run of spaces shrunk, never the column [keep]. */
    private fun squeeze(l: Line, cells: IntArray, keep: Int): IntArray {
        val out = ArrayList<Int>(cells.size)
        var i = 0
        while (i < cells.size) {
            if (!isSpace(l, cells[i])) { out.add(cells[i]); i++; continue }
            var j = i
            while (j < cells.size && isSpace(l, cells[j])) j++
            val limit = if (i == 0) INDENT_MAX else SQUEEZE_TO
            val shrink = if (i == 0) j - i > INDENT_MAX else j - i >= SQUEEZE_FROM
            for (k in i until j) if (!shrink || k - i < limit || cells[k] == keep) out.add(cells[k])
            i = j
        }
        return out.toIntArray()
    }

    /** [cells] on rows of at most [viewCols], breaking after a space when there is one in reach, in a word when there is none. */
    private fun wrap(line: Int, l: Line, cells: IntArray, viewCols: Int): List<VRow> {
        val rows = ArrayList<VRow>()
        var start = 0
        while (start < cells.size) {
            var w = 0
            var end = start
            while (end < cells.size && w + span(l, cells[end]) <= viewCols) { w += span(l, cells[end]); end++ }
            if (end == cells.size) { rows.add(VRow(line, cells.copyOfRange(start, end))); break }
            var brk = end - 1
            while (brk > start && !isSpace(l, cells[brk])) brk--
            if (brk <= start) {
                // No space to break at: a word longer than the view, or the view too narrow to hold any: cut it.
                rows.add(VRow(line, cells.copyOfRange(start, maxOf(end, start + 1))))
                start = maxOf(end, start + 1)
                continue
            }
            var stop = brk
            while (stop > start && isSpace(l, cells[stop - 1])) stop--
            rows.add(VRow(line, cells.copyOfRange(start, stop)))
            start = brk
            while (start < cells.size && isSpace(l, cells[start])) start++
        }
        return rows
    }
}
