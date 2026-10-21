package dev.jw.next.client

import dev.jw.next.contract.Block
import dev.jw.next.contract.Screen
import dev.jw.next.contract.Span
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.intOrNull

enum class BlockState { Ok, Failed, Running, Interrupted }

fun Block.state(): BlockState = when {
    interrupted -> BlockState.Interrupted
    running -> BlockState.Running
    (code ?: 0) != 0 -> BlockState.Failed
    else -> BlockState.Ok
}

fun formatMs(ms: Long): String = if (ms < 1000) "$ms ms" else "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0)

/** How the block is going, right of its folder: the time, how it ended, or how long it has run. */
fun Block.timeLabel(nowMs: Long): String = when (state()) {
    BlockState.Running -> "corriendo ${maxOf(0, nowMs - started) / 1000} s"
    BlockState.Failed -> "salió $code · ${formatMs(ms ?: 0)}"
    BlockState.Ok -> formatMs(ms ?: 0)
    BlockState.Interrupted -> ms?.takeIf { it >= 1000 }?.let { "interrumpido · ${formatMs(it)}" } ?: "interrumpido"
}

/** The line where the daemon restarted: what is above it was kept, what is below is new (spec REQ-301). */
fun Block.restartLabel(nowMs: Long): String = "el daemon se reinició · ${ago(nowMs / 1000, started / 1000)}"

/** What a block printed, as text (the "Salida" action). */
fun Block.outputText(): String = out.joinToString("\n") { row -> row.joinToString("") { it.t } }

/** The rows of a pane's screen to draw: its colours while it has them (REQ-527), else each line as plain text. */
fun Screen.styledRows(): List<List<Span>> = spans.ifEmpty { lines.map { listOf(Span(it)) } }

/** The cell the cursor is drawn on: [col] of [row], after [padding] spaces when the row's text ends before it. */
data class CursorCell(val row: Int, val col: Int, val padding: Int)

/** Where the cursor is on [styledRows]; null while it is hidden or is not on a row. */
fun Screen.cursorCell(): CursorCell? {
    if (!cursor.visible || cursor.row < 0 || cursor.col < 0) return null
    val row = styledRows().getOrNull(cursor.row) ?: return null
    return CursorCell(cursor.row, cursor.col, maxOf(0, cursor.col + 1 - row.sumOf { it.t.length }))
}

/** The last two folders of a path, so it fits a block's header. */
fun shortPath(path: String): String {
    val parts = path.split('/').filter { it.isNotEmpty() }
    return if (parts.size > 2) "…/" + parts.takeLast(2).joinToString("/") else path
}

/** A colour a span asks for: one of the sixteen named ones (the theme's) or a fixed RGB. */
sealed interface SpanColor {
    data class Named(val index: Int) : SpanColor
    data class Rgb(val argb: Int) : SpanColor
}

/** A palette index (a number) or `#rrggbb` (a string); anything else is no colour. */
fun JsonPrimitive.spanColor(): SpanColor? {
    if (isString) {
        val hex = content.removePrefix("#")
        if (hex.length != 6) return null
        val rgb = hex.toIntOrNull(16) ?: return null
        return SpanColor.Rgb(0xFF000000.toInt() or rgb)
    }
    val i = intOrNull ?: return null
    return when (i) {
        in 0..15 -> SpanColor.Named(i)
        in 16..231 -> {
            val n = i - 16
            val level = { v: Int -> if (v == 0) 0 else 55 + 40 * v }
            SpanColor.Rgb(rgb(level(n / 36), level(n % 36 / 6), level(n % 6)))
        }
        in 232..255 -> {
            val v = 8 + 10 * (i - 232)
            SpanColor.Rgb(rgb(v, v, v))
        }
        else -> null
    }
}

private fun rgb(r: Int, g: Int, b: Int) = 0xFF000000.toInt() or (r shl 16) or (g shl 8) or b
