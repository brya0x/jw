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
import dev.jw.term.Terminal
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import dev.jw.term.Color as Cell

/** A span of the screen and its scrollback, from where the finger went down to where it is now; lines are [Terminal.line] indexes. */
class Selection(val startLine: Int, val startCol: Int, val endLine: Int, val endCol: Int) {
    /** The text it covers, a line at a time, without the spaces at the end of a line. */
    fun text(t: Terminal): String = t.read {
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
 * A pane's terminal, drawn cell by cell on a canvas (REQ-21). It shows the pane at its own size, scaled down to
 * fit the width until the letters would be smaller than [MIN_SP]; past that it pans sideways. Dragging up and
 * down scrolls the history; a long press selects. With [fit] on, the grid is as big as the view and [onSize] says
 * how big (REQ-24).
 */
@Composable
fun TerminalView(
    term: Terminal?,
    rev: Long,
    fit: Boolean,
    selection: Selection?,
    onSelection: (Selection?) -> Unit,
    onSize: (cols: Int, rows: Int) -> Unit,
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
    var scrollLines by remember { mutableFloatStateOf(0f) }
    var panX by remember { mutableFloatStateOf(0f) }
    val onSel by rememberUpdatedState(onSelection)
    val onTapNow by rememberUpdatedState(onTap)

    BoxWithConstraints(modifier.clipToBounds().semantics { contentDescription = "Terminal" }) {
        val wPx = with(density) { maxWidth.toPx() }
        val hPx = with(density) { maxHeight.toPx() }
        paint.typeface = regular
        val advance = run { paint.textSize = 100f; paint.measureText("M") / 100f }
        val minPx = with(density) { MIN_SP.sp.toPx() }
        val fitPx = with(density) { FIT_SP.sp.toPx() }
        val maxPx = with(density) { MAX_SP.sp.toPx() }
        val size = when {
            fit -> fitPx
            term == null -> fitPx
            else -> (wPx / (term.cols * advance)).coerceIn(minPx, maxPx)
        }
        paint.textSize = size
        val cw = advance * size
        val fm = paint.fontMetrics
        val ch = (fm.descent - fm.ascent) * 1.12f
        val visibleCols = floor(wPx / cw).toInt().coerceAtLeast(1)
        val visibleRows = floor(hPx / ch).toInt().coerceAtLeast(1)
        if (fit) LaunchedEffect(visibleCols, visibleRows) { kotlinx.coroutines.delay(250); onSize(visibleCols, visibleRows) }

        val pane = term
        fun cellAt(p: Offset): Pair<Int, Int>? {
            val t = pane ?: return null
            return t.read {
                val firstLine = firstLine(it, visibleRows, scrollLines)
                val line = (firstLine + floor(p.y / ch).toInt()).coerceIn(-it.scrollbackSize, it.rows - 1)
                val col = floor((p.x + panX) / cw).toInt().coerceIn(0, it.cols - 1)
                line to col
            }
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
                                val d = c.positionChange()
                                val t = pane
                                if (t != null) {
                                    val maxScroll = t.read { bottomAnchor(it, visibleRows) + it.scrollbackSize }
                                    scrollLines = (scrollLines + d.y / ch).coerceIn(0f, maxScroll.toFloat())
                                    val overflow = max(0f, t.cols * cw - wPx)
                                    panX = (panX - d.x).coerceIn(0f, overflow)
                                }
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
            t.read { tm ->
                drawIntoCanvas { canvas ->
                    val nc = canvas.nativeCanvas
                    val first = firstLine(tm, visibleRows, scrollLines)
                    val base = -fm.ascent + (ch - (fm.descent - fm.ascent)) / 2
                    val sel = selection
                    for (vr in 0 until visibleRows) {
                        val li = first + vr
                        if (li < -tm.scrollbackSize || li >= tm.rows) continue
                        val line = tm.line(li)
                        val y = vr * ch
                        var c = 0
                        while (c < line.cols) {
                            val fl = line.flags[c]
                            val inv = fl and Attr.INVERSE != 0
                            var fgc = colors.resolve(line.fg[c], j.fg)
                            var bgc = colors.resolve(line.bg[c], Color.Unspecified)
                            if (inv) { val f = fgc; fgc = if (bgc == Color.Unspecified) j.bg else bgc; bgc = f }
                            val selected = sel != null && inSelection(sel, li, c)
                            val x = c * cw - panX
                            val span = if (fl and Attr.WIDE != 0) 2 else 1
                            if (x + span * cw >= 0 && x <= wPx) {
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
                            }
                            c += span
                        }
                    }
                    if (tm.cursorVisible && scrollLines == 0f) {
                        val vr = tm.cursorRow - first
                        if (vr in 0 until visibleRows) {
                            val cx = (tm.cursorCol.coerceAtMost(tm.cols - 1)) * cw - panX
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
 * The [Terminal.line] index of the first row drawn, `scroll` lines up into the history; see [bottomAnchor].
 */
private fun firstLine(t: Terminal, visibleRows: Int, scroll: Float): Int = bottomAnchor(t, visibleRows) - scroll.toInt()

/**
 * The first row of the view at rest. A pane taller than the view (the PC's window) ends at its last used row (the
 * cursor or the lowest row with text, so an agent's footer shows), not at its empty bottom; a full-screen app ends at the bottom.
 */
internal fun bottomAnchor(t: Terminal, visibleRows: Int): Int {
    if (t.rows <= visibleRows) return 0
    val maxFirst = t.rows - visibleRows
    if (t.altScreen) return maxFirst
    var last = t.cursorRow
    for (r in t.rows - 1 downTo last + 1) {
        if (t.line(r).ch.any { it != 0 && it != ' '.code }) { last = r; break }
    }
    return (last + 1 - visibleRows).coerceIn(0, maxFirst)
}

private fun inSelection(s: Selection, line: Int, col: Int): Boolean {
    val forward = s.startLine < s.endLine || (s.startLine == s.endLine && s.startCol <= s.endCol)
    val (l0, c0, l1, c1) = if (forward) listOf(s.startLine, s.startCol, s.endLine, s.endCol) else listOf(s.endLine, s.endCol, s.startLine, s.startCol)
    if (line < l0 || line > l1) return false
    if (line == l0 && col < c0) return false
    if (line == l1 && col > c1) return false
    return true
}

private const val MIN_SP = 9f
private const val FIT_SP = 11f
private const val MAX_SP = 14f
