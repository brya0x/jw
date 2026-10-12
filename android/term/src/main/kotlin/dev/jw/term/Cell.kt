package dev.jw.term

/** A colour in one Int: kind in the top byte (0 default, 1 palette index, 2 RGB), the value below it. */
object Color {
    const val DEFAULT = 0
    fun index(i: Int) = (1 shl 24) or (i and 0xff)
    fun rgb(r: Int, g: Int, b: Int) = (2 shl 24) or ((r and 0xff) shl 16) or ((g and 0xff) shl 8) or (b and 0xff)
    fun kind(c: Int) = c ushr 24
    fun value(c: Int) = c and 0xffffff
}

object Attr {
    const val BOLD = 1
    const val DIM = 2
    const val ITALIC = 4
    const val UNDERLINE = 8
    const val INVERSE = 16

    /** The first cell of a two-column character; the cell after it is a [CONT]. */
    const val WIDE = 32
    const val CONT = 64
}

/** One row of the grid: parallel arrays, a code point per cell (0 is empty) and the combining marks that follow it. */
class Line(val cols: Int) {
    val ch = IntArray(cols)
    val fg = IntArray(cols)
    val bg = IntArray(cols)
    val flags = IntArray(cols)
    private var marks: HashMap<Int, String>? = null

    fun marksAt(col: Int): String? = marks?.get(col)

    fun addMark(col: Int, cp: Int) {
        val m = marks ?: HashMap<Int, String>().also { marks = it }
        m[col] = (m[col] ?: "") + String(Character.toChars(cp))
    }

    fun removeMarks(col: Int) {
        marks?.remove(col)
    }

    /** Moves a cell, its marks with it. */
    fun copyCell(from: Int, to: Int) {
        ch[to] = ch[from]
        fg[to] = fg[from]
        bg[to] = bg[from]
        flags[to] = flags[from]
        val m = marks ?: return
        val mk = m[from]
        if (mk == null) m.remove(to) else m[to] = mk
    }

    fun clear(from: Int, to: Int, bg: Int) {
        for (c in from until to) {
            ch[c] = 0
            fg[c] = 0
            this.bg[c] = bg
            flags[c] = 0
        }
        marks?.keys?.removeAll { it in from until to }
    }

    /** The text of the row, wide characters once and empty cells as spaces, without trailing spaces. */
    fun text(): String {
        val sb = StringBuilder()
        for (c in 0 until cols) {
            if (flags[c] and Attr.CONT != 0) continue
            if (ch[c] == 0) sb.append(' ') else sb.appendCodePoint(ch[c])
            marks?.get(c)?.let(sb::append)
        }
        return sb.toString().trimEnd(' ')
    }

    fun copyFrom(o: Line) {
        val n = minOf(cols, o.cols)
        System.arraycopy(o.ch, 0, ch, 0, n)
        System.arraycopy(o.fg, 0, fg, 0, n)
        System.arraycopy(o.bg, 0, bg, 0, n)
        System.arraycopy(o.flags, 0, flags, 0, n)
        o.marks?.let { m -> marks = HashMap(m.filterKeys { it < n }) }
        if (n < o.cols && n > 0 && flags[n - 1] and Attr.WIDE != 0) clear(n - 1, n, 0)
    }
}
