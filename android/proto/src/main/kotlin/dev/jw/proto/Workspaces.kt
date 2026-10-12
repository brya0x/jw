package dev.jw.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

enum class Kind { Root, Worktree, Folder }

/** One item of the bridge's `workspaces` answer (spec Interfaces, W2). Items arrive in sidebar order. */
class Workspace(
    val id: String,
    val kind: Kind,
    val name: String,
    val path: String,
    val project: String,
    val branch: String,
    /** The PR number jw recorded for a worktree; no live state. */
    val pr: Int?,
    val session: String,
    /** A worktree's root `id`, else null. */
    val parent: String?,
)

class SessionInfo(val name: String, val workspaces: Int)

enum class AgentState { Working, Waiting, Idle }

/** `PaneInfo` as `crates/proto/src/proto.rs` serialises it. `stream` joins it to [Workspace.id]. */
class PaneInfo(
    val pane: Long,
    val stream: String,
    val role: String,
    val exited: Int?,
    val fg: String?,
    val busy: Boolean,
    val bell: Boolean,
    val agent: AgentState?,
    val cwd: String?,
)

enum class Mark(val glyph: Char) { Working('✻'), Waiting('?'), Dev('⚡') }

object Workspaces {
    private val json = Json { ignoreUnknownKeys = true }

    fun parseItems(arr: JsonArray?): List<Workspace> = arr.orEmpty().mapNotNull { el ->
        val o = el as? JsonObject ?: return@mapNotNull null
        fun str(k: String) = (o[k] as? JsonPrimitive)?.contentOrNull
        val kind = when (str("kind")) {
            "root" -> Kind.Root
            "worktree" -> Kind.Worktree
            "folder" -> Kind.Folder
            else -> return@mapNotNull null
        }
        Workspace(
            id = str("id") ?: return@mapNotNull null,
            kind = kind,
            name = str("name") ?: "",
            path = str("path") ?: "",
            project = str("project") ?: "",
            branch = str("branch") ?: "",
            pr = (o["pr"] as? JsonPrimitive)?.intOrNull,
            session = str("session") ?: "",
            parent = str("parent"),
        )
    }

    fun parseSessions(arr: JsonArray?): List<SessionInfo> = arr.orEmpty().mapNotNull { el ->
        val o = el as? JsonObject ?: return@mapNotNull null
        SessionInfo(
            (o["name"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null,
            (o["workspaces"] as? JsonPrimitive)?.intOrNull ?: 0,
        )
    }

    /** `{"t":"Panes","panes":[...]}` from a kind-0 frame; null when it is some other message. */
    fun parsePanes(frame: Frame): List<PaneInfo>? {
        val text = (frame as? Frame.Json)?.text ?: return null
        val obj = try {
            json.parseToJsonElement(text).jsonObject
        } catch (_: Exception) {
            return null
        }
        if (obj["t"]?.jsonPrimitive?.contentOrNull != "Panes") return null
        return (obj["panes"] as? JsonArray).orEmpty().mapNotNull { el ->
            val o = el as? JsonObject ?: return@mapNotNull null
            fun str(k: String) = (o[k] as? JsonPrimitive)?.contentOrNull
            fun bool(k: String) = (o[k] as? JsonPrimitive)?.booleanOrNull ?: false
            PaneInfo(
                pane = (o["pane"] as? JsonPrimitive)?.longOrNull ?: return@mapNotNull null,
                stream = str("stream") ?: return@mapNotNull null,
                role = str("role") ?: "",
                exited = (o["exited"] as? JsonPrimitive)?.intOrNull,
                fg = str("fg"),
                busy = bool("busy"),
                bell = bool("bell"),
                agent = when (str("agent")) {
                    "working" -> AgentState.Working
                    "waiting" -> AgentState.Waiting
                    "idle" -> AgentState.Idle
                    else -> null
                },
                cwd = str("cwd"),
            )
        }
    }

    /** The relay request for the pane list: `ClientMsg::List`, kind 0. Allowed in read-only. */
    fun listFrame(): Frame = Frame.Json("""{"t":"List"}""")

    /** `ClientMsg::Input`: the JSON head carries an empty `bytes`, the bytes follow raw (kind 1). */
    fun inputFrame(pane: Long, bytes: ByteArray): Frame =
        Frame.Raw(buildJsonObject { put("t", "Input"); put("pane", pane); put("bytes", JsonArray(emptyList())) }.toString(), bytes)

    /** Panes of one workspace: the join is by id, `PaneInfo.stream == Workspace.id`. */
    fun panesOf(panes: List<PaneInfo>, id: String): List<PaneInfo> = panes.filter { it.stream == id }

    /** The marks the TUI draws (`marks` in tui/src/tui/mod.rs), minus the merged-PR flag the bridge cannot know. */
    fun marks(panes: List<PaneInfo>): List<Mark> {
        // Panes with no hooks fall back to output and bell, like the TUI.
        fun guess(p: PaneInfo) = p.agent == null && p.role == "agent"
        val out = ArrayList<Mark>(3)
        if (panes.any { it.agent == AgentState.Working || (guess(it) && it.busy) }) out += Mark.Working
        if (panes.any { it.agent == AgentState.Waiting || (guess(it) && it.bell && !it.busy) }) out += Mark.Waiting
        if (panes.any { it.role.startsWith("dev:") && it.exited == null }) out += Mark.Dev
        return out
    }

    /** The pane to answer: the first whose agent (from hooks) is Waiting. */
    fun waitingPane(panes: List<PaneInfo>): PaneInfo? = panes.firstOrNull { it.agent == AgentState.Waiting }
}

/** Ask for `List` every [intervalMs] only while [active]; pure, the caller owns the clock. */
class RefreshRule(private val intervalMs: Long = 2_000) {
    private var last: Long? = null

    fun due(nowMs: Long, active: Boolean): Boolean {
        if (!active) {
            last = null
            return false
        }
        val l = last
        if (l != null && nowMs - l < intervalMs) return false
        last = nowMs
        return true
    }
}

/** When the y and n buttons may be enabled (REQ-20, RISK-4). */
object AnswerRule {
    /** Three polls (`pollMs` is 2 s): one late `Panes` must not turn the buttons dim and green again. */
    const val MAX_AGE_MS = 6_000L

    /**
     * Only while the latest `Panes` (at most [MAX_AGE_MS] old) says Waiting for this pane, the phone is not
     * read-only and this prompt has not been answered yet.
     */
    fun enabled(pane: PaneInfo?, panesAtMs: Long?, nowMs: Long, readOnly: Boolean, alreadyAnswered: Boolean): Boolean =
        !readOnly && !alreadyAnswered && pane?.agent == AgentState.Waiting &&
            panesAtMs != null && nowMs - panesAtMs in 0..MAX_AGE_MS
}

/** What y and n send. Defaults: Enter approves, Esc rejects (OPEN-5); both can be changed in Settings. */
class AnswerChoice(val id: String, val label: String, val bytes: ByteArray)

object AnswerChoices {
    val APPROVE = listOf(
        AnswerChoice("enter", "Enter", byteArrayOf(0x0d)),
        AnswerChoice("y", "y", "y".toByteArray()),
        AnswerChoice("y-enter", "y + Enter", "y\r".toByteArray()),
    )
    val REJECT = listOf(
        AnswerChoice("esc", "Esc", byteArrayOf(0x1b)),
        AnswerChoice("n", "n", "n".toByteArray()),
        AnswerChoice("n-enter", "n + Enter", "n\r".toByteArray()),
    )

    fun approve(id: String?) = APPROVE.firstOrNull { it.id == id } ?: APPROVE[0]
    fun reject(id: String?) = REJECT.firstOrNull { it.id == id } ?: REJECT[0]
}
