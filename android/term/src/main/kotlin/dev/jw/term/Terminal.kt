package dev.jw.term

/** What a program asked the terminal to report for the mouse (`?9`, `?1000`, `?1002`, `?1003`). */
enum class MouseMode { None, Press, PressRelease, ButtonMotion, AnyMotion }

enum class MouseEncoding { Default, Utf8, Sgr }

/**
 * A VT100/xterm screen: bytes in, a grid of cells out. It answers nothing (the daemon already
 * answers what a program asks) and keeps no title (the daemon sends it), so it is only a picture
 * of the pane. The network thread calls [feed], the UI thread reads inside [read].
 */
class Terminal(cols: Int, rows: Int, private val scrollbackMax: Int = 5000) {
    var cols = cols.coerceAtLeast(1)
        private set
    var rows = rows.coerceAtLeast(1)
        private set

    private var main = Array(this.rows) { Line(this.cols) }
    private var alt = Array(this.rows) { Line(this.cols) }
    private val history = ArrayDeque<Line>()
    private var grid = main

    var cursorRow = 0
        private set
    var cursorCol = 0
        private set
    var cursorVisible = true
        private set

    /** The cursor is on the last column and the next character wraps to the next row. */
    val wrapPending: Boolean get() = wrapNext
    var altScreen = false
        private set
    var appCursor = false
        private set
    var appKeypad = false
        private set
    var bracketedPaste = false
        private set
    var mouseMode = MouseMode.None
        private set
    var mouseEncoding = MouseEncoding.Default
        private set

    /** Bumped by every change; the surface redraws when it moves. */
    @Volatile
    var version = 0L
        private set

    private var fg = 0
    private var bg = 0
    private var flags = 0
    private var wrapNext = false
    private var autowrap = true
    private var origin = false
    private var insert = false
    private var top = 0
    private var bottom = this.rows - 1
    private var tabs = defaultTabs(this.cols)
    private var lastChar = 0

    private class Saved(val row: Int, val col: Int, val fg: Int, val bg: Int, val flags: Int, val wrapNext: Boolean, val origin: Boolean, val charset: IntArray, val shifted: Int)
    private var saved: Saved? = null
    private var savedMain: Saved? = null

    // 0 is ASCII and 1 DEC special graphics, for G0 and G1, and which of the two is in use.
    private var charset = intArrayOf(0, 0)
    private var shifted = 0

    private var state = GROUND
    private val params = IntArray(32)
    private val colon = BooleanArray(32)
    private var nparams = 0
    private var cur = -1
    private var priv = 0
    private var inter = 0
    private var utf = 0
    private var utfNeed = 0

    val scrollbackSize: Int get() = history.size

    /** The row `i` of the screen, or of the scrollback when it is negative (-1 is the line just above the screen). */
    fun line(i: Int): Line = if (i >= 0) grid[i] else history[history.size + i]

    fun <T> read(block: (Terminal) -> T): T = synchronized(this) { block(this) }

    fun feed(bytes: ByteArray, off: Int = 0, len: Int = bytes.size - off) = synchronized(this) {
        for (i in off until off + len) byte(bytes[i].toInt() and 0xff)
        version++
    }

    fun resize(newCols: Int, newRows: Int) = synchronized(this) {
        val c = newCols.coerceAtLeast(1)
        val r = newRows.coerceAtLeast(1)
        if (c == cols && r == rows) return@synchronized
        // Shrinking keeps the cursor on screen by sending rows from the top to the scrollback.
        val fromTop = if (altScreen || r >= rows) 0 else (cursorRow - (r - 1)).coerceIn(0, rows - r)
        for (i in 0 until fromTop) pushHistory(main[i])
        main = rebuild(main.drop(fromTop), c, r)
        alt = rebuild(alt.toList(), c, r)
        cursorRow -= fromTop
        grid = if (altScreen) alt else main
        cols = c
        rows = r
        top = 0
        bottom = r - 1
        cursorRow = cursorRow.coerceIn(0, r - 1)
        cursorCol = cursorCol.coerceIn(0, c - 1)
        wrapNext = false
        tabs = defaultTabs(c)
        version++
    }

    private fun rebuild(list: List<Line>, c: Int, r: Int) = Array(r) { i ->
        Line(c).also { if (i < list.size) it.copyFrom(list[i]) }
    }

    // ---- bytes ----

    private fun byte(b: Int) {
        if (utfNeed > 0) {
            if (b and 0xC0 == 0x80) {
                utf = (utf shl 6) or (b and 0x3F)
                if (--utfNeed == 0) put(translate(utf))
                return
            }
            utfNeed = 0
            put(0xFFFD)
        }
        if (b >= 0x80) {
            if (state != GROUND) return
            when {
                b < 0xC2 -> put(0xFFFD)
                b < 0xE0 -> { utf = b and 0x1F; utfNeed = 1 }
                b < 0xF0 -> { utf = b and 0x0F; utfNeed = 2 }
                b < 0xF5 -> { utf = b and 0x07; utfNeed = 3 }
                else -> put(0xFFFD)
            }
            return
        }
        if (b == 0x18 || b == 0x1A) { state = GROUND; return }
        if (b == 0x1B) {
            state = if (state == OSC || state == STR) STR_ESC else ESC
            inter = 0
            return
        }
        when (state) {
            GROUND -> if (b < 0x20 || b == 0x7F) control(b) else put(translate(b))
            ESC -> escape(b)
            ESC_INT -> when {
                b in 0x20..0x2F -> inter = (inter shl 8) or b
                b < 0x20 -> control(b)
                else -> { designate(inter, b); state = GROUND }
            }
            CSI -> csi(b)
            OSC -> if (b == 0x07) state = GROUND
            STR -> {}
            STR_ESC -> {
                state = GROUND
                if (b != '\\'.code) { state = ESC; escape(b) }
            }
        }
    }

    private fun control(b: Int) {
        when (b) {
            0x08 -> { if (cursorCol > 0) cursorCol--; wrapNext = false }
            0x09 -> tab(1)
            0x0A, 0x0B, 0x0C -> lineFeed()
            0x0D -> { cursorCol = 0; wrapNext = false }
            0x0E -> shifted = 1
            0x0F -> shifted = 0
        }
    }

    private fun escape(b: Int) {
        when (b) {
            in 0x20..0x2F -> { inter = b; state = ESC_INT; return }
            '['.code -> { startCsi(); return }
            ']'.code -> { state = OSC; return }
            'P'.code, 'X'.code, '^'.code, '_'.code -> { state = STR; return }
            '7'.code -> saved = save()
            '8'.code -> saved?.let(::restore)
            'D'.code -> lineFeed()
            'E'.code -> { cursorCol = 0; lineFeed() }
            'M'.code -> reverseIndex()
            'H'.code -> tabs[cursorCol] = true
            'c'.code -> reset()
            '='.code -> appKeypad = true
            '>'.code -> appKeypad = false
            else -> if (b < 0x20) { control(b); return }
        }
        state = GROUND
    }

    private fun designate(intermediate: Int, final: Int) {
        val set = if (final == '0'.code) 1 else 0
        when (intermediate) {
            '('.code -> charset[0] = set
            ')'.code -> charset[1] = set
        }
    }

    private fun startCsi() {
        state = CSI
        nparams = 0
        cur = -1
        priv = 0
        inter = 0
        colon.fill(false)
    }

    private fun csi(b: Int) {
        when {
            b in '0'.code..'9'.code -> cur = minOf((if (cur < 0) 0 else cur) * 10 + (b - '0'.code), 65535)
            b == ';'.code || b == ':'.code -> {
                if (nparams < params.size) {
                    params[nparams] = cur
                    colon[nparams] = b == ':'.code
                    nparams++
                }
                cur = -1
            }
            b in '<'.code..'?'.code -> priv = b
            b in 0x20..0x2F -> inter = (inter shl 8) or b
            b < 0x20 -> control(b)
            b in 0x40..0x7E -> {
                if ((cur >= 0 || nparams > 0) && nparams < params.size) params[nparams++] = cur
                state = GROUND
                dispatch(b)
            }
            else -> state = GROUND
        }
    }

    private fun p(i: Int, default: Int): Int = if (i < nparams && params[i] > 0) params[i] else default

    private fun dispatch(f: Int) {
        if (priv != 0 || inter != 0) {
            if (priv == '?'.code && inter == 0 && (f == 'h'.code || f == 'l'.code)) {
                for (i in 0 until nparams) setMode(params[i], f == 'h'.code)
            }
            return
        }
        when (f) {
            '@'.code -> insertChars(p(0, 1))
            'A'.code -> up(p(0, 1))
            'B'.code, 'e'.code -> down(p(0, 1))
            'C'.code, 'a'.code -> { cursorCol = minOf(cols - 1, cursorCol + p(0, 1)); wrapNext = false }
            'D'.code -> { cursorCol = maxOf(0, cursorCol - p(0, 1)); wrapNext = false }
            'E'.code -> { down(p(0, 1)); cursorCol = 0 }
            'F'.code -> { up(p(0, 1)); cursorCol = 0 }
            'G'.code, '`'.code -> { cursorCol = (p(0, 1) - 1).coerceIn(0, cols - 1); wrapNext = false }
            'H'.code, 'f'.code -> moveTo(p(0, 1) - 1, p(1, 1) - 1)
            'I'.code -> tab(p(0, 1))
            'J'.code -> eraseDisplay(p0())
            'K'.code -> eraseLine(p0())
            'L'.code -> insertLines(p(0, 1))
            'M'.code -> deleteLines(p(0, 1))
            'P'.code -> deleteChars(p(0, 1))
            'S'.code -> scrollUp(p(0, 1))
            'T'.code -> scrollDown(p(0, 1))
            'X'.code -> { grid[cursorRow].clear(cursorCol, minOf(cols, cursorCol + p(0, 1)), bg); fixWide(cursorRow); wrapNext = false }
            'Z'.code -> repeat(p(0, 1)) { while (cursorCol > 0 && !tabs[--cursorCol]) Unit }
            'b'.code -> if (lastChar != 0) repeat(minOf(p(0, 1), 65535)) { put(lastChar) }
            'd'.code -> moveTo(p(0, 1) - 1, cursorCol)
            'g'.code -> when (p0()) {
                0 -> tabs[cursorCol] = false
                3 -> tabs.fill(false)
            }
            'h'.code, 'l'.code -> for (i in 0 until nparams) if (params[i] == 4) insert = f == 'h'.code
            'm'.code -> sgr()
            'r'.code -> {
                val t = p(0, 1) - 1
                val bt = p(1, rows) - 1
                if (t < bt && bt < rows) { top = t; bottom = bt; moveTo(0, 0) }
            }
            's'.code -> saved = save()
            'u'.code -> saved?.let(::restore)
        }
    }

    private fun p0() = if (nparams == 0 || params[0] < 0) 0 else params[0]

    // ---- cursor and scrolling ----

    private fun moveTo(row: Int, col: Int) {
        cursorRow = if (origin) (top + row).coerceIn(top, bottom) else row.coerceIn(0, rows - 1)
        cursorCol = col.coerceIn(0, cols - 1)
        wrapNext = false
    }

    /** Up and down stop at the scroll region's edge when the cursor starts inside it. */
    private fun up(n: Int) {
        cursorRow = maxOf(if (cursorRow >= top) top else 0, cursorRow - n)
        wrapNext = false
    }

    private fun down(n: Int) {
        cursorRow = minOf(if (cursorRow <= bottom) bottom else rows - 1, cursorRow + n)
        wrapNext = false
    }

    private fun lineFeed() {
        wrapNext = false
        if (cursorRow == bottom) scrollUp(1) else if (cursorRow < rows - 1) cursorRow++
    }

    private fun reverseIndex() {
        wrapNext = false
        if (cursorRow == top) scrollDown(1) else if (cursorRow > 0) cursorRow--
    }

    private fun pushHistory(l: Line) {
        if (scrollbackMax == 0) return
        if (history.size >= scrollbackMax) history.removeFirst()
        history.addLast(l)
    }

    /** Rows `from` to the region's bottom move up `n`; the ones that leave from the screen's top join the scrollback. */
    private fun shiftUp(from: Int, n: Int, keep: Boolean) {
        for (i in 0 until minOf(n, bottom - from + 1)) {
            val gone = grid[from]
            if (keep) pushHistory(gone)
            for (r in from until bottom) grid[r] = grid[r + 1]
            grid[bottom] = if (keep) Line(cols) else gone
            grid[bottom].clear(0, cols, bg)
        }
    }

    private fun shiftDown(from: Int, n: Int) {
        for (i in 0 until minOf(n, bottom - from + 1)) {
            val gone = grid[bottom]
            for (r in bottom downTo from + 1) grid[r] = grid[r - 1]
            grid[from] = gone
            gone.clear(0, cols, bg)
        }
    }

    private fun scrollUp(n: Int) = shiftUp(top, n, top == 0 && !altScreen)

    private fun scrollDown(n: Int) = shiftDown(top, n)

    private fun insertLines(n: Int) {
        if (cursorRow !in top..bottom) return
        shiftDown(cursorRow, n)
        cursorCol = 0
        wrapNext = false
    }

    private fun deleteLines(n: Int) {
        if (cursorRow !in top..bottom) return
        shiftUp(cursorRow, n, false)
        cursorCol = 0
        wrapNext = false
    }

    private fun tab(n: Int) {
        repeat(n) {
            var c = cursorCol + 1
            while (c < cols - 1 && !tabs[c]) c++
            cursorCol = minOf(c, cols - 1)
        }
        wrapNext = false
    }

    // ---- editing ----

    /** A wide character left with only one half becomes blank. */
    private fun fixWide(row: Int) {
        val l = grid[row]
        for (c in 0 until cols) {
            if (l.flags[c] and Attr.WIDE != 0 && (c + 1 >= cols || l.flags[c + 1] and Attr.CONT == 0)) l.clear(c, c + 1, 0)
            if (l.flags[c] and Attr.CONT != 0 && (c == 0 || l.flags[c - 1] and Attr.WIDE == 0)) l.clear(c, c + 1, 0)
        }
    }

    private fun insertChars(n: Int) {
        val l = grid[cursorRow]
        val k = minOf(n, cols - cursorCol)
        for (c in cols - 1 downTo cursorCol + k) l.copyCell(c - k, c)
        l.clear(cursorCol, cursorCol + k, bg)
        fixWide(cursorRow)
        wrapNext = false
    }

    private fun deleteChars(n: Int) {
        val l = grid[cursorRow]
        val k = minOf(n, cols - cursorCol)
        for (c in cursorCol until cols - k) l.copyCell(c + k, c)
        l.clear(cols - k, cols, bg)
        fixWide(cursorRow)
        wrapNext = false
    }

    private fun eraseLine(mode: Int) {
        val l = grid[cursorRow]
        when (mode) {
            0 -> l.clear(cursorCol, cols, bg)
            1 -> l.clear(0, minOf(cols, cursorCol + 1), bg)
            2 -> l.clear(0, cols, bg)
        }
        fixWide(cursorRow)
        wrapNext = false
    }

    private fun eraseDisplay(mode: Int) {
        when (mode) {
            0 -> {
                eraseLine(0)
                for (r in cursorRow + 1 until rows) grid[r].clear(0, cols, bg)
            }
            1 -> {
                eraseLine(1)
                for (r in 0 until cursorRow) grid[r].clear(0, cols, bg)
            }
            2 -> for (r in 0 until rows) grid[r].clear(0, cols, bg)
            3 -> history.clear()
        }
        wrapNext = false
    }

    private fun put(cp: Int) {
        val w = Width.of(cp)
        if (w == 0) {
            val col = if (wrapNext) cursorCol else cursorCol - 1
            if (col >= 0) {
                val l = grid[cursorRow]
                l.addMark(if (l.flags[col] and Attr.CONT != 0 && col > 0) col - 1 else col, cp)
            }
            return
        }
        if (wrapNext) {
            if (autowrap) {
                cursorCol = 0
                lineFeed()
            }
            wrapNext = false
        }
        if (w == 2 && cursorCol == cols - 1) {
            if (!autowrap) return
            grid[cursorRow].clear(cursorCol, cols, bg)
            cursorCol = 0
            lineFeed()
        }
        val l = grid[cursorRow]
        if (insert) insertChars(w)
        // Overwriting half of a wide character blanks the other half.
        if (l.flags[cursorCol] and Attr.CONT != 0 && cursorCol > 0) l.clear(cursorCol - 1, cursorCol, bg)
        if (l.flags[cursorCol] and Attr.WIDE != 0 && cursorCol + 1 < cols) l.clear(cursorCol + 1, cursorCol + 2, bg)
        l.removeMarks(cursorCol)
        l.ch[cursorCol] = cp
        l.fg[cursorCol] = fg
        l.bg[cursorCol] = bg
        l.flags[cursorCol] = flags or if (w == 2) Attr.WIDE else 0
        if (w == 2) {
            if (cursorCol + 2 < cols && l.flags[cursorCol + 2] and Attr.CONT != 0) l.clear(cursorCol + 2, cursorCol + 3, bg)
            l.removeMarks(cursorCol + 1)
            l.ch[cursorCol + 1] = 0
            l.fg[cursorCol + 1] = fg
            l.bg[cursorCol + 1] = bg
            l.flags[cursorCol + 1] = flags or Attr.CONT
        }
        lastChar = cp
        if (cursorCol + w >= cols) {
            cursorCol = cols - 1
            wrapNext = true
        } else {
            cursorCol += w
        }
    }

    private fun translate(b: Int): Int =
        if (charset[shifted] == 1 && b in 0x5F..0x7E) DEC_GRAPHICS[b - 0x5F] else b

    // ---- attributes and modes ----

    private fun sgr() {
        if (nparams == 0) { fg = 0; bg = 0; flags = 0; return }
        var i = 0
        while (i < nparams) {
            val v = maxOf(params[i], 0)
            when (v) {
                0 -> { fg = 0; bg = 0; flags = 0 }
                1 -> flags = flags or Attr.BOLD
                2 -> flags = flags or Attr.DIM
                3 -> flags = flags or Attr.ITALIC
                4 -> flags = if (colon[i] && i + 1 < nparams && params[i + 1] == 0) flags and Attr.UNDERLINE.inv() else flags or Attr.UNDERLINE
                7 -> flags = flags or Attr.INVERSE
                21 -> flags = flags or Attr.UNDERLINE
                22 -> flags = flags and (Attr.BOLD or Attr.DIM).inv()
                23 -> flags = flags and Attr.ITALIC.inv()
                24 -> flags = flags and Attr.UNDERLINE.inv()
                27 -> flags = flags and Attr.INVERSE.inv()
                in 30..37 -> fg = Color.index(v - 30)
                38, 48 -> i += extendedColor(i, v == 38)
                39 -> fg = 0
                in 40..47 -> bg = Color.index(v - 40)
                49 -> bg = 0
                in 90..97 -> fg = Color.index(v - 90 + 8)
                in 100..107 -> bg = Color.index(v - 100 + 8)
            }
            i++
        }
    }

    /** `38;5;n`, `38;2;r;g;b` and `38:2:colourspace:r:g:b` (or 48): sets the colour and says how many parameters it used. */
    private fun extendedColor(i: Int, isFg: Boolean): Int {
        val kind = if (i + 1 < nparams) params[i + 1] else return 0
        val c: Int
        val used: Int
        if (kind == 5 && i + 2 < nparams) {
            c = Color.index(maxOf(params[i + 2], 0)); used = 2
        } else if (kind == 2) {
            val skip = if (colon[i] && i + 5 < nparams) 1 else 0
            if (i + 4 + skip >= nparams) return nparams - i - 1
            c = Color.rgb(maxOf(params[i + 2 + skip], 0), maxOf(params[i + 3 + skip], 0), maxOf(params[i + 4 + skip], 0)); used = 4 + skip
        } else return 0
        if (isFg) fg = c else bg = c
        return used
    }

    private fun setMode(m: Int, on: Boolean) {
        when (m) {
            1 -> appCursor = on
            6 -> { origin = on; moveTo(0, 0) }
            7 -> autowrap = on
            25 -> cursorVisible = on
            47, 1047 -> useAlt(on, clear = on && m == 1047, keepCursor = false)
            1048 -> if (on) savedMain = save() else savedMain?.let(::restore)
            1049 -> useAlt(on, clear = on, keepCursor = true)
            2004 -> bracketedPaste = on
            9 -> mouseMode = if (on) MouseMode.Press else MouseMode.None
            1000 -> mouseMode = if (on) MouseMode.PressRelease else MouseMode.None
            1002 -> mouseMode = if (on) MouseMode.ButtonMotion else MouseMode.None
            1003 -> mouseMode = if (on) MouseMode.AnyMotion else MouseMode.None
            1005 -> mouseEncoding = if (on) MouseEncoding.Utf8 else MouseEncoding.Default
            1006 -> mouseEncoding = if (on) MouseEncoding.Sgr else MouseEncoding.Default
        }
    }

    /** 1049 saves the cursor on the way in and puts it back on the way out; 47 and 1047 do not. */
    private fun useAlt(on: Boolean, clear: Boolean, keepCursor: Boolean) {
        if (on == altScreen) return
        if (on) {
            if (keepCursor) savedMain = save()
            altScreen = true
            grid = alt
            if (clear) for (l in grid) l.clear(0, cols, 0)
        } else {
            altScreen = false
            grid = main
            if (keepCursor) savedMain?.let(::restore)
        }
        wrapNext = false
    }

    private fun save() = Saved(cursorRow, cursorCol, fg, bg, flags, wrapNext, origin, charset.copyOf(), shifted)

    private fun restore(s: Saved) {
        cursorRow = s.row.coerceIn(0, rows - 1)
        cursorCol = s.col.coerceIn(0, cols - 1)
        fg = s.fg
        bg = s.bg
        flags = s.flags
        wrapNext = s.wrapNext
        origin = s.origin
        charset = s.charset.copyOf()
        shifted = s.shifted
    }

    private fun reset() {
        for (g in arrayOf(main, alt)) for (l in g) l.clear(0, cols, 0)
        history.clear()
        grid = main
        altScreen = false
        cursorRow = 0
        cursorCol = 0
        cursorVisible = true
        fg = 0
        bg = 0
        flags = 0
        wrapNext = false
        autowrap = true
        origin = false
        insert = false
        appCursor = false
        appKeypad = false
        bracketedPaste = false
        mouseMode = MouseMode.None
        mouseEncoding = MouseEncoding.Default
        top = 0
        bottom = rows - 1
        charset = intArrayOf(0, 0)
        shifted = 0
        saved = null
        savedMain = null
        tabs = defaultTabs(cols)
    }

    private companion object {
        const val GROUND = 0
        const val ESC = 1
        const val ESC_INT = 2
        const val CSI = 3
        const val OSC = 4
        const val STR = 5
        const val STR_ESC = 6

        fun defaultTabs(cols: Int) = BooleanArray(cols).also { t -> for (i in 8 until t.size step 8) t[i] = true }

        /** 0x5F to 0x7E under DEC special graphics. */
        val DEC_GRAPHICS = intArrayOf(
            0x00A0, 0x25C6, 0x2592, 0x2409, 0x240C, 0x240D, 0x240A, 0x00B0, 0x00B1, 0x2424, 0x240B, 0x2518, 0x2510, 0x250C, 0x2514, 0x253C,
            0x23BA, 0x23BB, 0x2500, 0x23BC, 0x23BD, 0x251C, 0x2524, 0x2534, 0x252C, 0x2502, 0x2264, 0x2265, 0x03C0, 0x2260, 0x00A3, 0x00B7,
        )
    }
}
