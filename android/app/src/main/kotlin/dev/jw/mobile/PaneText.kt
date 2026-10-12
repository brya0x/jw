package dev.jw.mobile

import dev.jw.proto.PaneEvent
import dev.jw.proto.TextRun
import dev.jw.term.Attr
import dev.jw.term.Line
import dev.jw.term.Width

/**
 * A pane as the PC sent it (`PaneEvent.Text`, `PaneEvent.History`): the rows of its screen and, once asked for,
 * the rows above them, as cells with their styles. No terminal runs on the phone, so nothing here knows escape codes.
 */
class PaneText(cols: Int, rows: Int) {
    var cols = cols
        private set
    var rows = rows
        private set
    var cursorRow = 0
        private set
    var cursorCol = 0
        private set
    var cursorVisible = false
        private set
    var altScreen = false
        private set

    /**
     * The row a shell's prompt starts at while it waits at it (REQ-92): from there down is the prompt and the line being
     * typed, which the phone leaves out, as its input is that line. Null for any other pane, and while a command runs.
     */
    var promptRow: Int? = null
        private set

    /** The program asked for application cursor keys, which change what the arrows send. */
    var appCursor = false
        private set
    private var screen: List<Line> = emptyList()
    private var history: List<Line> = emptyList()

    /** The rows of scrollback held; zero until the person scrolls up and the PC answers. */
    val scrollbackSize: Int get() = history.size

    /** The row `i` of the screen, or of the scrollback when it is negative (-1 is the row just above the screen). */
    fun line(i: Int): Line = if (i >= 0) screen[i] else history[history.size + i]

    fun <T> read(block: (PaneText) -> T): T = synchronized(this) { block(this) }

    fun setScreen(e: PaneEvent.Text) = setScreen(e.cols, e.rows, e.alt, e.cursor, e.lines.map { lineOf(it, e.cols) }, e.appCursor, e.prompt)

    internal fun setScreen(cols: Int, rows: Int, alt: Boolean, cursor: Pair<Int, Int>?, lines: List<Line>, appCursor: Boolean = false, prompt: Int? = null) = synchronized(this) {
        promptRow = prompt?.coerceIn(0, rows)
        this.appCursor = appCursor
        this.cols = cols
        this.rows = rows
        altScreen = alt
        cursorVisible = cursor != null
        cursorRow = (cursor?.first ?: 0).coerceIn(0, rows - 1)
        cursorCol = (cursor?.second ?: 0).coerceIn(0, cols)
        screen = List(rows) { lines.getOrNull(it) ?: Line(cols) }
    }

    fun setHistory(e: PaneEvent.History) = synchronized(this) { history = e.lines.map { lineOf(it, cols) } }

    companion object {
        /** A row of cells from its runs: a wide character takes two cells, a combining mark joins the cell before it. */
        fun lineOf(runs: List<TextRun>, cols: Int): Line {
            val line = Line(cols)
            for (r in runs) {
                var c = r.col
                var base = -1
                var i = 0
                while (i < r.text.length) {
                    val cp = r.text.codePointAt(i)
                    i += Character.charCount(cp)
                    val w = Width.of(cp)
                    if (w == 0) {
                        if (base >= 0) line.addMark(base, cp)
                        continue
                    }
                    if (c < 0 || c + w > cols) break
                    line.ch[c] = cp
                    line.fg[c] = r.fg
                    line.bg[c] = r.bg
                    line.flags[c] = r.attrs and 0x1f or if (w == 2) Attr.WIDE else 0
                    if (w == 2) {
                        line.fg[c + 1] = r.fg
                        line.bg[c + 1] = r.bg
                        line.flags[c + 1] = r.attrs and 0x1f or Attr.CONT
                    }
                    base = c
                    c += w
                }
            }
            return line
        }
    }
}
