package dev.jw.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

/**
 * A run of cells of one row that share a style (`Run` in `proto.rs`): the first column, the text, the colours
 * (0 default, `1 shl 24 or i` a palette index, `2 shl 24 or 0xrrggbb` a colour) and the attributes
 * (bold 1, dim 2, italic 4, underline 8, inverse 16).
 */
class TextRun(val col: Int, val text: String, val fg: Int, val bg: Int, val attrs: Int)

/** What a watched pane sends: its screen as styled rows, the rows above it on request, its size and its end. */
sealed interface PaneEvent {
    val pane: Long

    /** The pane's last output, then its exact screen: replayed into an empty terminal of `cols` by `rows` it is the pane. */
    class Snapshot(override val pane: Long, val cols: Int, val rows: Int, val bytes: ByteArray) : PaneEvent {
        override fun toString() = "Snapshot(pane=$pane, ${cols}x$rows, ${bytes.size} bytes)"
    }

    class Output(override val pane: Long, val bytes: ByteArray) : PaneEvent {
        override fun toString() = "Output(pane=$pane, ${bytes.size} bytes)"
    }

    /** The pane's screen, replacing the one held: `cursor` is null when the program hid it. */
    class Text(
        override val pane: Long, val cols: Int, val rows: Int, val alt: Boolean, val appCursor: Boolean,
        val cursor: Pair<Int, Int>?, val lines: List<List<TextRun>>,
    ) : PaneEvent {
        override fun toString() = "Text(pane=$pane, ${cols}x$rows)"
    }

    /** The answer to [Panes.textHistoryFrame]: rows of the scrollback, oldest first. */
    class History(override val pane: Long, val lines: List<List<TextRun>>) : PaneEvent {
        override fun toString() = "History(pane=$pane, ${lines.size} rows)"
    }

    class Resized(override val pane: Long, val cols: Int, val rows: Int) : PaneEvent

    class Exited(override val pane: Long, val status: Int) : PaneEvent
}

/** The pane messages of `crates/proto/src/proto.rs` that the terminal needs, and the ones the app sends for it. */
object Panes {
    private val json = Json { ignoreUnknownKeys = true }

    /** null for any message that is not about one pane's contents (`Tree`, `Panes`, `Title`, `Screen`, `History`...). */
    fun parse(frame: Frame): PaneEvent? {
        val (head, raw) = when (frame) {
            is Frame.Json -> frame.text to null
            is Frame.Raw -> frame.head to frame.bytes
        }
        val o = try {
            json.parseToJsonElement(head).jsonObject
        } catch (_: Exception) {
            return null
        }
        fun num(k: String) = (o[k] as? JsonPrimitive)?.longOrNull
        val pane = num("pane") ?: return null
        return when ((o["t"] as? JsonPrimitive)?.contentOrNull) {
            "Snapshot" -> Snapshot(o, pane, raw ?: return null)
            "Output" -> PaneEvent.Output(pane, raw ?: return null)
            "Resized" -> {
                val (c, r) = size(o) ?: return null
                PaneEvent.Resized(pane, c, r)
            }
            "Text" -> {
                val (c, r) = size(o) ?: return null
                val cursor = (o["cursor"] as? JsonArray)?.let { a ->
                    val row = (a.getOrNull(0) as? JsonPrimitive)?.intOrNull
                    val col = (a.getOrNull(1) as? JsonPrimitive)?.intOrNull
                    if (row != null && col != null) row to col else null
                }
                val lines = rows(o["lines"], MAX_SCREEN_ROWS) ?: return null
                PaneEvent.Text(
                    pane, c, r, (o["alt"] as? JsonPrimitive)?.booleanOrNull ?: false,
                    (o["app_cursor"] as? JsonPrimitive)?.booleanOrNull ?: false, cursor, lines,
                )
            }
            "TextHistory" -> PaneEvent.History(pane, rows(o["lines"], MAX_HISTORY_ROWS) ?: return null)
            "Exited" -> PaneEvent.Exited(pane, (o["status"] as? JsonPrimitive)?.intOrNull ?: return null)
            else -> null
        }
    }

    private const val MAX_SCREEN_ROWS = 1000
    private const val MAX_HISTORY_ROWS = 10_000
    private const val MAX_RUNS = 2000

    /** `lines` of `Text` and `TextHistory`: rows of runs, `[col, text, fg, bg, attrs]` each; null when malformed. */
    private fun rows(el: kotlinx.serialization.json.JsonElement?, max: Int): List<List<TextRun>>? {
        val a = el as? JsonArray ?: return null
        if (a.size > max) return null
        return a.map { row ->
            val runs = row as? JsonArray ?: return null
            if (runs.size > MAX_RUNS) return null
            runs.map { r ->
                val t = r as? JsonArray ?: return null
                val n = IntArray(5)
                for (i in intArrayOf(0, 2, 3, 4)) n[i] = (t.getOrNull(i) as? JsonPrimitive)?.longOrNull?.toInt() ?: return null
                TextRun(n[0], (t.getOrNull(1) as? JsonPrimitive)?.contentOrNull ?: return null, n[2], n[3], n[4])
            }
        }
    }

    @Suppress("FunctionName")
    private fun Snapshot(o: JsonObject, pane: Long, bytes: ByteArray): PaneEvent? {
        val (c, r) = size(o) ?: return null
        return PaneEvent.Snapshot(pane, c, r, bytes)
    }

    private fun size(o: JsonObject): Pair<Int, Int>? {
        val c = (o["cols"] as? JsonPrimitive)?.intOrNull ?: return null
        val r = (o["rows"] as? JsonPrimitive)?.intOrNull ?: return null
        return if (c in 1..1000 && r in 1..1000) c to r else null
    }

    /** `ClientMsg::Attach`: the workspace's panes, each as a `Snapshot`, then their output. Allowed in read-only. */
    fun attachFrame(stream: String): Frame =
        Frame.Json(buildJsonObject { put("t", "Attach"); put("stream", stream) }.toString())

    /** `ClientMsg::TextView`: this connection gets panes as [PaneEvent.Text], not as a terminal's bytes. Send it before [attachFrame]. Allowed in read-only. */
    fun textViewFrame(): Frame = Frame.Json("""{"t":"TextView"}""")

    /** `ClientMsg::TextHistory`: the last `count` rows of the pane's scrollback, answered as [PaneEvent.History]. Allowed in read-only. */
    fun textHistoryFrame(pane: Long, count: Int): Frame =
        Frame.Json(buildJsonObject { put("t", "TextHistory"); put("pane", pane); put("count", count) }.toString())

    /** `ClientMsg::Detach`: stop every pane's output; the panes keep running. Allowed in read-only. */
    fun detachFrame(): Frame = Frame.Json("""{"t":"Detach"}""")

    /** `ClientMsg::Resize`. Dropped by the bridge in read-only. */
    fun resizeFrame(pane: Long, cols: Int, rows: Int): Frame =
        Frame.Json(buildJsonObject { put("t", "Resize"); put("pane", pane); put("cols", cols); put("rows", rows) }.toString())

    /**
     * `ClientMsg::Split`: a shell beside `pane`, in `cwd`, the way `^␣ t` does it (side by side when the pane is wide,
     * one over the other when not; cells are about twice as tall as wide). `cols` and `rows` are the pane's.
     */
    fun splitFrame(pane: Long, cwd: String, cols: Int, rows: Int): Frame {
        val right = cols >= rows * 2
        val (c, r) = if (right) maxOf(cols / 2, 1) to rows else cols to maxOf(rows / 2, 1)
        return Frame.Json(
            buildJsonObject {
                put("t", "Split")
                put("pane", pane)
                put("dir", if (right) "right" else "down")
                put(
                    "new",
                    buildJsonObject {
                        put("role", "shell")
                        put("cmd", JsonNull)
                        put("resume", JsonNull)
                        put("cwd", cwd)
                        put("env", buildJsonObject {})
                        put("cols", c)
                        put("rows", r)
                    },
                )
            }.toString(),
        )
    }

    /** `ClientMsg::Kill`: ends the pane's process and forgets the pane. */
    fun killFrame(pane: Long): Frame = Frame.Json(buildJsonObject { put("t", "Kill"); put("pane", pane) }.toString())

    /** `ClientMsg::Name`: the pane's name; blank goes back to its automatic title. */
    fun nameFrame(pane: Long, name: String): Frame = Frame.Json(
        buildJsonObject {
            put("t", "Name")
            put("pane", pane)
            if (name.isBlank()) put("name", JsonNull) else put("name", name.trim())
        }.toString(),
    )
}
