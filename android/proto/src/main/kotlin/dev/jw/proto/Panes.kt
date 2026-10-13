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

/**
 * One block of an agent's conversation (`ChatBlock` in `proto.rs`): `kind` is `user` or `text` (a message: [text]),
 * `think` ([result] says how long), `notice` (the PC telling that a subagent or a background task ended: [name] is `done`, `failed` or
 * `stopped`, [text] its title, [arg] `agent` or `task`, [result] how long it took), `queued` (a message the person sent while Claude
 * works, [text]), `command` (a slash command the person ran, [text] is `/commit`, or `!ls` for a shell line), `commandout` (what a local
 * command printed, [text]) or `tool` ([name], [arg] the one line that sums the call up, [body] the command,
 * the diff or the output, [result] the line that sums its result up, null until it answered). [i] numbers the
 * blocks from the start of the conversation; a block with a number already held is that block, changed.
 */
class ChatBlock(
    val i: Int, val kind: String, val text: String, val name: String, val arg: String,
    val result: String?, val body: String?, val error: Boolean,
)

/** How the turn Claude is in stands (`ChatTurn` in `proto.rs`): when the person's last message was sent (ms since the epoch) and the output tokens since. */
class ChatTurn(val since: Long, val tokens: Long)

/**
 * One command of a shell (`ShellBlock` in `proto.rs`): what was typed, the folder, how it ended ([code], null while it runs or if it
 * never said) and how long it took ([ms], so far while [running]). [full] says a program drew the whole screen (no output is kept),
 * [truncated] that the start of its output was let go. [i] numbers the pane's commands from its start: a block with a number
 * already held is that block, changed. Its output comes as [PaneEvent.ShellOut].
 */
class ShellBlock(
    val i: Int, val cmd: String, val cwd: String, val code: Int?, val ms: Long, val running: Boolean, val full: Boolean,
    val truncated: Boolean,
)

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

    /**
     * The conversation of the agent in the pane (`DaemonMsg::Chat`): `reset` says these are all the blocks held,
     * else they are the ones new or changed. `known` is false for a pane that is no Claude session, which has no
     * conversation; `title` is the name Claude gave it.
     */
    class Chat(
        override val pane: Long, val known: Boolean, val name: String?, val title: String?, val reset: Boolean,
        val blocks: List<ChatBlock>, val turn: ChatTurn? = null, val bg: Int? = null, val plan: String? = null, val working: Boolean? = null,
    ) : PaneEvent {
        override fun toString() = "Chat(pane=$pane, known=$known, ${blocks.size} blocks)"
    }

    /**
     * What the shell in the pane says of itself and its commands (`DaemonMsg::Shell`), when it changes: `known` is false while it
     * has marked nothing (it is not one jw started: the pane is plain text), `running` says a command runs, `input` is the line being
     * typed at the prompt, when it is new, and `blocks` are the commands that are new or changed.
     */
    class Shell(
        override val pane: Long, val known: Boolean, val running: Boolean, val input: String?, val blocks: List<ShellBlock>,
    ) : PaneEvent {
        override fun toString() = "Shell(pane=$pane, known=$known, running=$running, ${blocks.size} blocks)"
    }

    /** Bytes of the output of block [block] of the shell in the pane, as the command printed them, to add to what is held. */
    class ShellOut(override val pane: Long, val block: Int, val bytes: ByteArray) : PaneEvent {
        override fun toString() = "ShellOut(pane=$pane, block=$block, ${bytes.size} bytes)"
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
            "Chat" -> {
                val blocks = (o["blocks"] as? JsonArray)?.takeIf { it.size <= MAX_CHAT_BLOCKS }?.map { b ->
                    val bo = b as? JsonObject ?: return null
                    fun str(k: String) = (bo[k] as? JsonPrimitive)?.contentOrNull
                    ChatBlock(
                        (bo["i"] as? JsonPrimitive)?.intOrNull ?: return null, str("kind") ?: return null, str("text") ?: "",
                        str("name") ?: "", str("arg") ?: "", str("result"), str("body"),
                        (bo["error"] as? JsonPrimitive)?.booleanOrNull ?: false,
                    )
                } ?: return null
                PaneEvent.Chat(
                    pane, (o["known"] as? JsonPrimitive)?.booleanOrNull ?: false, (o["name"] as? JsonPrimitive)?.contentOrNull,
                    (o["title"] as? JsonPrimitive)?.contentOrNull, (o["reset"] as? JsonPrimitive)?.booleanOrNull ?: false, blocks,
                    (o["turn"] as? JsonObject)?.let { t ->
                        ChatTurn((t["since"] as? JsonPrimitive)?.longOrNull ?: return null, (t["tokens"] as? JsonPrimitive)?.longOrNull ?: 0)
                    },
                    (o["bg"] as? JsonPrimitive)?.intOrNull,
                    (o["plan"] as? JsonPrimitive)?.contentOrNull,
                    (o["working"] as? JsonPrimitive)?.booleanOrNull,
                )
            }
            "Shell" -> {
                val blocks = (o["blocks"] as? JsonArray)?.takeIf { it.size <= MAX_CHAT_BLOCKS }?.map { b ->
                    val bo = b as? JsonObject ?: return null
                    fun str(k: String) = (bo[k] as? JsonPrimitive)?.contentOrNull
                    fun bool(k: String) = (bo[k] as? JsonPrimitive)?.booleanOrNull ?: false
                    ShellBlock(
                        (bo["i"] as? JsonPrimitive)?.intOrNull ?: return null, str("cmd") ?: "", str("cwd") ?: "",
                        (bo["code"] as? JsonPrimitive)?.intOrNull, (bo["ms"] as? JsonPrimitive)?.longOrNull ?: 0,
                        bool("running"), bool("full"), bool("truncated"),
                    )
                } ?: return null
                PaneEvent.Shell(
                    pane, (o["known"] as? JsonPrimitive)?.booleanOrNull ?: false, (o["running"] as? JsonPrimitive)?.booleanOrNull ?: false,
                    (o["input"] as? JsonPrimitive)?.contentOrNull, blocks,
                )
            }
            "ShellOut" -> PaneEvent.ShellOut(pane, (num("block") ?: return null).toInt(), raw ?: return null)
            "TextHistory" -> PaneEvent.History(pane, rows(o["lines"], MAX_HISTORY_ROWS) ?: return null)
            "Exited" -> PaneEvent.Exited(pane, (o["status"] as? JsonPrimitive)?.intOrNull ?: return null)
            else -> null
        }
    }

    private const val MAX_SCREEN_ROWS = 1000
    private const val MAX_HISTORY_ROWS = 10_000
    private const val MAX_RUNS = 2000
    private const val MAX_CHAT_BLOCKS = 1000

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

    /** `ClientMsg::ChatWatch`: the conversation of the agent in `pane`, answered as [PaneEvent.Chat]. Allowed in read-only. */
    fun chatWatchFrame(pane: Long): Frame = Frame.Json(buildJsonObject { put("t", "ChatWatch"); put("pane", pane) }.toString())

    /** `ClientMsg::ChatUnwatch`. Allowed in read-only. */
    fun chatUnwatchFrame(pane: Long): Frame = Frame.Json(buildJsonObject { put("t", "ChatUnwatch"); put("pane", pane) }.toString())

    /** `ClientMsg::ShellWatch`: the commands of the shell in `pane`, answered as [PaneEvent.Shell]. Allowed in read-only. */
    fun shellWatchFrame(pane: Long): Frame = Frame.Json(buildJsonObject { put("t", "ShellWatch"); put("pane", pane) }.toString())

    /** `ClientMsg::ShellUnwatch`. Allowed in read-only. */
    fun shellUnwatchFrame(pane: Long): Frame = Frame.Json(buildJsonObject { put("t", "ShellUnwatch"); put("pane", pane) }.toString())

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
