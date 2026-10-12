package dev.jw.mobile

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** The prototype's stroke icons (`const I` in `docs/specs/mobile-prototype.html`), same paths, 24-unit grid. */
enum class Ic(val d: String) {
    Back("M15 5l-7 7 7 7"),
    Zap("M13 3L5 14h6l-1 7 8-11h-6z"),
    X("M6 6l12 12M18 6L6 18"),
    Cam("M4 8h3l1.5-2h7L17 8h3a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1z M8.5 13a3.5 3.5 0 1 0 7 0a3.5 3.5 0 1 0 -7 0"),
    Scan("M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3M4 12h16"),
    Search("M21 21l-4.3-4.3M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14z"),
    Plus("M12 5v14M5 12h14"),
    Folder("M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"),
    Link("M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"),
}

@Composable
fun Icon(ic: Ic, color: Color, size: Dp = 20.dp, stroke: Dp = 2.dp) {
    val path = remember(ic) { PathParser().parsePathString(ic.d).toPath() }
    Canvas(Modifier.size(size)) {
        val k = this.size.width / 24f
        val s = stroke.toPx() / k
        withTransform({ scale(k, k, Offset.Zero) }) {
            drawPath(path, color, style = Stroke(s, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}
