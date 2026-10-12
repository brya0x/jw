package dev.jw.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

/** What a watched pane sends: its whole screen, then output, size changes and its end. */
sealed interface PaneEvent {
    val pane: Long

    /** The pane's last output, then its exact screen: replayed into an empty terminal of `cols` by `rows` it is the pane. */
    class Snapshot(override val pane: Long, val cols: Int, val rows: Int, val bytes: ByteArray) : PaneEvent {
        override fun toString() = "Snapshot(pane=$pane, ${cols}x$rows, ${bytes.size} bytes)"
    }

    class Output(override val pane: Long, val bytes: ByteArray) : PaneEvent {
        override fun toString() = "Output(pane=$pane, ${bytes.size} bytes)"
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
            "Exited" -> PaneEvent.Exited(pane, (o["status"] as? JsonPrimitive)?.intOrNull ?: return null)
            else -> null
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

    /** `ClientMsg::Detach`: stop every pane's output; the panes keep running. Allowed in read-only. */
    fun detachFrame(): Frame = Frame.Json("""{"t":"Detach"}""")

    /** `ClientMsg::Resize`. Dropped by the bridge in read-only. */
    fun resizeFrame(pane: Long, cols: Int, rows: Int): Frame =
        Frame.Json(buildJsonObject { put("t", "Resize"); put("pane", pane); put("cols", cols); put("rows", rows) }.toString())
}
