package dev.jw.next.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp

/** The two icons of Home's header, drawn as in `docs/prototypes/mobile.html` (a 24-unit grid, a thin round stroke). */
internal enum class Glyph { Bell, Gear }

@Composable
internal fun GlyphIcon(glyph: Glyph, color: Color, size: Int = 22) {
    Canvas(Modifier.size(size.dp)) {
        val k = this.size.width / 24f
        val stroke = Stroke(1.8f * k, cap = StrokeCap.Round, join = StrokeJoin.Round)
        fun at(x: Float, y: Float) = Offset(x * k, y * k)
        when (glyph) {
            Glyph.Bell -> {
                val bell = Path().apply {
                    moveTo(6f * k, 16f * k)
                    lineTo(6f * k, 11f * k)
                    arcTo(Rect(6f * k, 5f * k, 18f * k, 17f * k), 180f, 180f, false)
                    lineTo(18f * k, 16f * k)
                    lineTo(19.5f * k, 18f * k)
                    lineTo(4.5f * k, 18f * k)
                    close()
                }
                drawPath(bell, color, style = stroke)
                drawLine(color, at(10f, 21f), at(14f, 21f), stroke.width, StrokeCap.Round)
            }
            Glyph.Gear -> {
                drawCircle(color, 3f * k, at(12f, 12f), style = stroke)
                listOf(
                    12f to 3f to (12f to 6f), 12f to 18f to (12f to 21f), 3f to 12f to (6f to 12f), 18f to 12f to (21f to 12f),
                    5.6f to 5.6f to (7.7f to 7.7f), 16.3f to 16.3f to (18.4f to 18.4f), 18.4f to 5.6f to (16.3f to 7.7f), 7.7f to 16.3f to (5.6f to 18.4f),
                ).forEach { (a, b) -> drawLine(color, at(a.first, a.second), at(b.first, b.second), stroke.width, StrokeCap.Round) }
            }
        }
    }
}
