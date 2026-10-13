package dev.jw.mobile

import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import dev.jw.term.Attr
import dev.jw.term.Line

/** The size of a cell and where a row's text sits in it, for the font and size the rows are drawn with. */
internal class CellMetrics(val cw: Float, val ch: Float, val base: Float, val size: Float) {
    companion object {
        fun of(regular: Typeface, sizePx: Float): CellMetrics {
            val p = Paint(Paint.ANTI_ALIAS_FLAG)
            p.typeface = regular
            p.textSize = 100f
            val advance = p.measureText("M") / 100f
            p.textSize = sizePx
            val fm = p.fontMetrics
            val ch = (fm.descent - fm.ascent) * 1.12f
            return CellMetrics(advance * sizePx, ch, -fm.ascent + (ch - (fm.descent - fm.ascent)) / 2, sizePx)
        }
    }
}

/** Draws the cells of a row of a pane's text, with their colours and attributes (REQ-21): the terminal's rows and a block's alike. */
internal class RowPainter(private val colors: TermColors, private val j: JwColors, private val regular: Typeface, private val bold: Typeface, size: Float) {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).also { it.textSize = size }

    /** The row [cols] of [line] at [y]; [selected] says which columns are in the selection. */
    fun draw(nc: android.graphics.Canvas, line: Line, cols: IntArray, y: Float, m: CellMetrics, selected: ((Int) -> Boolean)? = null) {
        var xi = 0
        for (c in cols) {
            if (c >= line.cols) break
            val fl = line.flags[c]
            val inv = fl and Attr.INVERSE != 0
            var fgc = colors.resolve(line.fg[c], j.fg)
            var bgc = colors.resolve(line.bg[c], Color.Unspecified)
            if (inv) { val f = fgc; fgc = if (bgc == Color.Unspecified) j.bg else bgc; bgc = f }
            val x = xi * m.cw
            val span = if (fl and Attr.WIDE != 0) 2 else 1
            if (bgc != Color.Unspecified) {
                paint.color = bgc.toArgb(); paint.style = Paint.Style.FILL
                nc.drawRect(x, y, x + span * m.cw + 0.5f, y + m.ch, paint)
            }
            if (selected != null && selected(c)) {
                paint.color = j.blue.copy(alpha = 0.35f).toArgb()
                nc.drawRect(x, y, x + span * m.cw, y + m.ch, paint)
            }
            val cp = line.ch[c]
            if (cp != 0 && cp != ' '.code && fl and Attr.CONT == 0) {
                paint.typeface = if (fl and Attr.BOLD != 0) bold else regular
                paint.textSkewX = if (fl and Attr.ITALIC != 0) -0.2f else 0f
                paint.color = (if (fl and Attr.DIM != 0) fgc.copy(alpha = 0.6f) else fgc).toArgb()
                val s = StringBuilder().appendCodePoint(cp).append(line.marksAt(c) ?: "").toString()
                nc.drawText(s, x, y + m.base, paint)
            }
            if (fl and Attr.UNDERLINE != 0 && fl and Attr.CONT == 0) {
                paint.color = fgc.toArgb()
                nc.drawRect(x, y + m.ch - 2f, x + span * m.cw, y + m.ch - 1f, paint)
            }
            xi += span
        }
    }

    /** A block of colour over one cell at column offset [xi] of the row at [y]: the cursor. */
    fun cursor(nc: android.graphics.Canvas, x: Float, y: Float, m: CellMetrics) {
        paint.style = Paint.Style.FILL
        paint.color = j.fg.copy(alpha = 0.55f).toArgb()
        nc.drawRect(x, y, x + m.cw, y + m.ch, paint)
    }
}
