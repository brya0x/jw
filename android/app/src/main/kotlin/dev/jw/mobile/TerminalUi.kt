package dev.jw.mobile

import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitLongPressOrCancellation
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.core.content.res.ResourcesCompat
import dev.jw.term.Attr
import dev.jw.term.Line
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import dev.jw.term.Color as Cell

/** A span of the screen and its scrollback, from where the finger went down to where it is now; lines are [PaneText.line] indexes. */
class Selection(val startLine: Int, val startCol: Int, val endLine: Int, val endCol: Int) {
    /** The text it covers, a line at a time, without the spaces at the end of a line. */
    fun text(t: PaneText): String = t.read {
        val forward = startLine < endLine || (startLine == endLine && startCol <= endCol)
        val (l0, c0, l1, c1) = if (forward) listOf(startLine, startCol, endLine, endCol) else listOf(endLine, endCol, startLine, startCol)
        val lo = max(l0, -it.scrollbackSize)
        val hi = min(l1, it.rows - 1)
        (lo..hi).joinToString("\n") { i ->
            val l = it.line(i)
            val from = if (i == l0) c0 else 0
            val to = if (i == l1) c1 else l.cols - 1
            val sb = StringBuilder()
            for (c in from.coerceAtLeast(0)..to.coerceAtMost(l.cols - 1)) {
                if (l.flags[c] and Attr.CONT != 0) continue
                if (l.ch[c] == 0) sb.append(' ') else sb.appendCodePoint(l.ch[c])
                l.marksAt(c)?.let(sb::append)
            }
            sb.toString().trimEnd()
        }
    }
}

/** The 16 ANSI colours from jw's roles, the 6x6x6 cube and the grey ramp of xterm, and RGB as it comes. */
class TermColors(private val j: JwColors) {
    private val ansi = arrayOf(
        mix(j.bg, j.fg, 0.2f), j.red, j.green, j.yellow, j.blue, j.magenta, j.cyan, mix(j.fg, j.bg, 0.15f),
        j.dim, mix(j.red, j.fg, 0.25f), mix(j.green, j.fg, 0.25f), mix(j.yellow, j.fg, 0.25f),
        mix(j.blue, j.fg, 0.25f), mix(j.magenta, j.fg, 0.25f), mix(j.cyan, j.fg, 0.25f), j.fg,
    )

    fun resolve(code: Int, default: Color): Color = when (Cell.kind(code)) {
        1 -> index(Cell.value(code))
        2 -> Color(0xff000000.toInt() or Cell.value(code))
        else -> default
    }

    private fun index(i: Int): Color = when {
        i < 16 -> ansi[i]
        i < 232 -> {
            val n = i - 16
            fun lvl(v: Int) = if (v == 0) 0 else 55 + v * 40
            Color(lvl(n / 36), lvl(n / 6 % 6), lvl(n % 6))
        }
        else -> (8 + (i - 232) * 10).let { Color(it, it, it) }
    }

    private fun mix(a: Color, b: Color, t: Float) = Color(a.red + (b.red - a.red) * t, a.green + (b.green - a.green) * t, a.blue + (b.blue - a.blue) * t)
}

/**
 * A pane's text, drawn cell by cell on a canvas (REQ-21): the rows of the pane's screen and scrollback, each
 * broken at the width of this view ([wrapRows]), so nothing is wider than the phone and the pane itself is never
 * resized for it (REQ-24). Dragging up and down scrolls the history; a long press selects.
 */
@Composable
fun TerminalView(
    term: PaneText?,
    rev: Long,
    onScrolledUp: () -> Unit,
    selection: Selection?,
    onSelection: (Selection?) -> Unit,
    onTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    val colors = remember(j) { TermColors(j) }
    val ctx = LocalContext.current
    val density = LocalDensity.current
    val regular = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_regular) ?: Typeface.MONOSPACE }
    val bold = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_bold) ?: Typeface.DEFAULT_BOLD }
    val paint = remember { Paint(Paint.ANTI_ALIAS_FLAG) }
    var scrollRows by remember { mutableFloatStateOf(0f) }
    val onSel by rememberUpdatedState(onSelection)
    val onTapNow by rememberUpdatedState(onTap)

    BoxWithConstraints(modifier.clipToBounds().semantics { contentDescription = "Terminal" }) {
        val wPx = with(density) { maxWidth.toPx() }
        val hPx = with(density) { maxHeight.toPx() }
        paint.typeface = regular
        val advance = run { paint.textSize = 100f; paint.measureText("M") / 100f }
        val size = with(density) { TEXT_SP.sp.toPx() }
        paint.textSize = size
        val cw = advance * size
        val fm = paint.fontMetrics
        val ch = (fm.descent - fm.ascent) * 1.12f
        val visibleCols = floor(wPx / cw).toInt().coerceAtLeast(1)
        val visibleRows = floor(hPx / ch).toInt().coerceAtLeast(1)

        val pane = term
        // The scrollback is asked for, and drawn, only while the view is up in it.
        val scrolled = scrollRows != 0f
        val layout = remember(pane, rev, visibleCols, scrolled) { pane?.read { wrapView(it, visibleCols, withHistory = scrolled) } }
        val askedUp by rememberUpdatedState(onScrolledUp)
        LaunchedEffect(scrolled, pane) { if (scrolled) askedUp() }
        fun firstRow(): Int = layout?.firstRow(visibleRows, scrollRows.toInt()) ?: 0
        fun cellAt(p: Offset): Pair<Int, Int>? {
            val l = layout ?: return null
            val row = l.rows.getOrNull(firstRow() + floor(p.y / ch).toInt().coerceAtLeast(0)) ?: l.rows.lastOrNull() ?: return null
            val col = RowWrap.colAt(pane!!.read { it.line(row.line) }, row, floor(p.x / cw).toInt().coerceAtLeast(0))
            return row.line to col
        }

        androidx.compose.foundation.Canvas(
            Modifier.fillMaxSize().pointerInput(pane) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    val lp = awaitLongPressOrCancellation(down.id)
                    if (lp != null) {
                        val start = cellAt(lp.position)
                        if (start != null) {
                            onSel(Selection(start.first, start.second, start.first, start.second))
                            drag(lp.id) { c ->
                                cellAt(c.position)?.let { e -> onSel(Selection(start.first, start.second, e.first, e.second)) }
                                c.consume()
                            }
                        }
                    } else {
                        val still = currentEvent.changes.firstOrNull { it.id == down.id }
                        if (still == null || !still.pressed) {
                            onSel(null)
                            onTapNow()
                        } else {
                            drag(down.id) { c ->
                                val (up, down) = layout?.scrollRange(visibleRows) ?: (0 to 0)
                                scrollRows = (scrollRows + c.positionChange().y / ch).coerceIn(down.toFloat(), up.toFloat())
                                c.consume()
                            }
                        }
                    }
                }
            },
        ) {
            rev.hashCode()
            drawRect(j.bg)
            val t = pane ?: return@Canvas
            val l = layout ?: return@Canvas
            t.read { tm ->
                drawIntoCanvas { canvas ->
                    val nc = canvas.nativeCanvas
                    val first = firstRow()
                    val base = -fm.ascent + (ch - (fm.descent - fm.ascent)) / 2
                    val sel = selection
                    for (vr in 0 until visibleRows) {
                        val row = l.rows.getOrNull(first + vr) ?: continue
                        if (row.line >= tm.rows || row.line < -tm.scrollbackSize) continue
                        val line = tm.line(row.line)
                        val y = vr * ch
                        var xi = 0
                        for (c in row.cols) {
                            if (c >= line.cols) break
                            val fl = line.flags[c]
                            val inv = fl and Attr.INVERSE != 0
                            var fgc = colors.resolve(line.fg[c], j.fg)
                            var bgc = colors.resolve(line.bg[c], Color.Unspecified)
                            if (inv) { val f = fgc; fgc = if (bgc == Color.Unspecified) j.bg else bgc; bgc = f }
                            val selected = sel != null && inSelection(sel, row.line, c)
                            val x = xi * cw
                            val span = if (fl and Attr.WIDE != 0) 2 else 1
                            if (bgc != Color.Unspecified) {
                                paint.color = bgc.toArgb(); paint.style = Paint.Style.FILL
                                nc.drawRect(x, y, x + span * cw + 0.5f, y + ch, paint)
                            }
                            if (selected) {
                                paint.color = j.blue.copy(alpha = 0.35f).toArgb()
                                nc.drawRect(x, y, x + span * cw, y + ch, paint)
                            }
                            val cp = line.ch[c]
                            if (cp != 0 && cp != ' '.code && fl and Attr.CONT == 0) {
                                paint.typeface = if (fl and Attr.BOLD != 0) bold else regular
                                paint.textSkewX = if (fl and Attr.ITALIC != 0) -0.2f else 0f
                                paint.color = (if (fl and Attr.DIM != 0) fgc.copy(alpha = 0.6f) else fgc).toArgb()
                                val s = StringBuilder().appendCodePoint(cp).append(line.marksAt(c) ?: "").toString()
                                nc.drawText(s, x, y + base, paint)
                            }
                            if (fl and Attr.UNDERLINE != 0 && fl and Attr.CONT == 0) {
                                paint.color = fgc.toArgb()
                                nc.drawRect(x, y + ch - 2f, x + span * cw, y + ch - 1f, paint)
                            }
                            xi += span
                        }
                    }
                    if (tm.cursorVisible && scrollRows == 0f) {
                        val at = l.rows.indexOfFirst { it.line == tm.cursorRow && tm.cursorCol in it.cols }
                        val vr = at - first
                        if (at >= 0 && vr in 0 until visibleRows) {
                            val cx = RowWrap.xOf(tm.line(tm.cursorRow), l.rows[at], tm.cursorCol) * cw
                            paint.color = j.fg.copy(alpha = 0.55f).toArgb()
                            nc.drawRect(cx, vr * ch, cx + cw, (vr + 1) * ch, paint)
                        }
                    }
                }
            }
        }
    }
}

/**
 * The rows of a pane as the view draws them, and where the view rests: [end] rows in, at the bottom of the view (a
 * shell or an agent on the main screen), or at the top of the rows ([topRest], a full-screen app drawn from the top).
 * Only a full-screen app ([alt]) can be dragged down the pane past its rest.
 */
internal class WrapView(val rows: List<VRow>, private val end: Int, private val alt: Boolean, private val topRest: Boolean) {
    private fun base(visibleRows: Int) = if (topRest) 0 else (end - visibleRows).coerceAtLeast(0)
    private fun last(visibleRows: Int) = (rows.size - visibleRows).coerceAtLeast(0)

    /** The index of the first row drawn, `scroll` rows up from the rest (negative: down). */
    fun firstRow(visibleRows: Int, scroll: Int): Int = (base(visibleRows) - scroll).coerceIn(0, if (alt) last(visibleRows) else base(visibleRows))

    /** How far the view may be dragged: `up` rows into the history, and `down` (zero or negative) rows below the rest. */
    fun scrollRange(visibleRows: Int): Pair<Int, Int> =
        base(visibleRows) to if (alt) -(last(visibleRows) - base(visibleRows)).coerceAtLeast(0) else 0
}

/**
 * Breaks every row of the pane's screen and its recent scrollback at [cols], leaving off the blank end of a row (a
 * row of blanks is one empty row), so the text of a 200-column pane is as wide as the phone and nothing pans. At rest
 * the view ends at the last used row: the cursor or the lowest row with text, so a footer shows, not the empty bottom;
 * a full-screen app (the alternate screen) follows its cursor, or starts at its top when the cursor is hidden.
 */
internal fun wrapView(t: PaneText, cols: Int, withHistory: Boolean = true): WrapView {
    val out = ArrayList<VRow>()
    var anchor = t.cursorRow
    for (li in -(if (withHistory) minOf(t.scrollbackSize, WRAP_HISTORY) else 0) until (t.promptRow ?: t.rows)) {
        val line = t.line(li)
        var used = usedCols(line)
        if (li >= 0 && used > 0 && li > anchor && !t.altScreen) anchor = li
        if (li == t.cursorRow && t.cursorVisible) used = max(used, minOf(t.cursorCol + 1, line.cols))
        if (used == 0) { out.add(VRow(li, IntArray(0))); continue }
        out.addAll(RowWrap.rows(li, line, used, cols, if (li == t.cursorRow && t.cursorVisible) t.cursorCol else -1))
    }
    if (t.altScreen) anchor = t.cursorRow
    return WrapView(out, out.indexOfLast { it.line <= anchor } + 1, t.altScreen, t.altScreen && !t.cursorVisible)
}

/** The columns of [line] up to its last cell with text or a background. */
private fun usedCols(line: Line): Int {
    for (c in line.cols - 1 downTo 0) if ((line.ch[c] != 0 && line.ch[c] != ' '.code) || line.bg[c] != 0) return if (line.flags[c] and Attr.WIDE != 0) minOf(c + 2, line.cols) else c + 1
    return 0
}

private fun inSelection(s: Selection, line: Int, col: Int): Boolean {
    val forward = s.startLine < s.endLine || (s.startLine == s.endLine && s.startCol <= s.endCol)
    val (l0, c0, l1, c1) = if (forward) listOf(s.startLine, s.startCol, s.endLine, s.endCol) else listOf(s.endLine, s.endCol, s.startLine, s.startCol)
    if (line < l0 || line > l1) return false
    if (line == l0 && col < c0) return false
    if (line == l1 && col > c1) return false
    return true
}

private const val TEXT_SP = 11f

/** How many rows of scrollback are broken at the view's width (the terminal keeps up to 5000). */
private const val WRAP_HISTORY = 2000
