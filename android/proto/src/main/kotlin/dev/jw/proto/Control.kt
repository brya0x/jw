package dev.jw.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/** Error codes of `{"t":"error"}` (spec, Error codes). */
object ErrorCode {
    const val UNEXPECTED = "unexpected"
    const val TIMEOUT = "timeout"
    const val LIMITED = "limited"
    const val EXPIRED = "expired"
    const val UNKNOWN = "unknown"
    const val DENIED = "denied"
    const val REVOKED = "revoked"
    const val READ_ONLY = "read_only"
    const val UNSUPPORTED = "unsupported"
    const val PROTOCOL = "protocol"
    const val DAEMON = "daemon"

    /** After these the stored device is dead: drop it and scan again, never retry. */
    fun isFinal(code: String) = code == UNKNOWN || code == REVOKED
}

/** The first message the phone sends. Tokens and codes never reach `toString`. */
sealed interface Hello {
    class Pair(val token: String, val name: String, val code: String) : Hello {
        override fun toString() = "Pair(name=$name)"
    }

    class Auth(val device: String) : Hello {
        override fun toString() = "Auth"
    }
}

/** Palette roles from `ready.palettes` (or the built-in fallback, the values of the TUI's One Dark and One Light), as `#rrggbb`. */
class Palette(val roles: Map<String, String>) {
    operator fun get(role: String): String? = roles[role]
}

/** One theme of `ready.palettes`, for showing them: its name, side and the colours that were valid. */
class ThemeInfo(val name: String, val dark: Boolean, val colors: Map<String, String>)

class Palettes(val dark: Palette, val light: Palette, val themes: List<ThemeInfo> = emptyList()) {
    companion object {
        val ROLES = listOf(
            "bg", "panel", "fg", "dim", "line", "sel",
            "blue", "green", "yellow", "red", "magenta", "cyan",
            "add_bg", "del_bg", "add_word", "del_word",
        )

        /** One Dark and One Light, the roles of the approved prototype. */
        val FALLBACK = Palettes(
            Palette(
                mapOf(
                    "bg" to "#282c34", "panel" to "#21252b", "fg" to "#abb2bf",
                    "dim" to "#5c6370", "line" to "#3e4451", "sel" to "#2c313a",
                    "blue" to "#61afef", "green" to "#98c379", "yellow" to "#e5c07b",
                    "red" to "#e06c75", "magenta" to "#c678dd", "cyan" to "#56b6c2",
                    "add_bg" to "#37403d", "del_bg" to "#40343c", "add_word" to "#4e5f4b", "del_word" to "#67424a",
                ),
            ),
            Palette(
                mapOf(
                    "bg" to "#fafafa", "panel" to "#f0f0f1", "fg" to "#383a42",
                    "dim" to "#a0a1a7", "line" to "#dbdbdc", "sel" to "#e5e5e6",
                    "blue" to "#4078f2", "green" to "#50a14f", "yellow" to "#c18401",
                    "red" to "#e45649", "magenta" to "#a626a4", "cyan" to "#0184bc",
                    "add_bg" to "#e4eee4", "del_bg" to "#f7e5e3", "add_word" to "#c0dcc0", "del_word" to "#f3c2be",
                ),
            ),
        )

        /**
         * `ready.palettes` is an array of `{name, dark, colors:{role:"#rrggbb"}}`. The dark side is the
         * first dark theme (One Dark when present), the light side likewise. Missing or malformed
         * roles fall back role by role, so an empty array gives the built-in pair.
         */
        fun parse(el: JsonElement?): Palettes {
            val themes = (el as? JsonArray)?.mapNotNull { it as? JsonObject } ?: return FALLBACK
            fun side(dark: Boolean, fb: Palette): Palette {
                val ofSide = themes.filter { (it["dark"] as? JsonPrimitive)?.booleanOrNull == dark }
                val t = ofSide.firstOrNull {
                    (it["name"] as? JsonPrimitive)?.contentOrNull.equals(if (dark) "One Dark" else "One Light", true)
                } ?: ofSide.firstOrNull() ?: return fb
                val colors = t["colors"] as? JsonObject ?: return fb
                val roles = fb.roles.toMutableMap()
                for (r in ROLES) {
                    val v = (colors[r] as? JsonPrimitive)?.contentOrNull
                    if (v != null && HEX.matches(v)) roles[r] = v
                }
                return Palette(roles)
            }
            val all = themes.mapNotNull { t ->
                val name = (t["name"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
                val dark = (t["dark"] as? JsonPrimitive)?.booleanOrNull ?: return@mapNotNull null
                val colors = (t["colors"] as? JsonObject)?.let { c ->
                    ROLES.mapNotNull { r -> (c[r] as? JsonPrimitive)?.contentOrNull?.takeIf { HEX.matches(it) }?.let { r to it } }.toMap()
                } ?: return@mapNotNull null
                ThemeInfo(name, dark, colors)
            }
            return Palettes(side(true, FALLBACK.dark), side(false, FALLBACK.light), all)
        }

        private val HEX = Regex("#[0-9a-fA-F]{6}")
    }
}

/** What the bridge says in text JSON. */
sealed interface ServerMessage {
    class Paired(val device: String, val id: String) : ServerMessage {
        override fun toString() = "Paired(id=$id)"
    }

    class Ready(
        val bridge: Int,
        val protocol: Int,
        val readOnly: Boolean,
        val palettes: Palettes,
        val settings: JsonElement?,
    ) : ServerMessage

    /** `ask`, `ws` and `scope` say which request an error answers (absent for errors that are not an ask's). */
    class Error(
        val code: String,
        val msg: String,
        val ask: String? = null,
        val ws: String? = null,
        val scope: String? = null,
    ) : ServerMessage

    class Workspaces(val session: String, val items: List<Workspace>) : ServerMessage

    class Sessions(val items: List<SessionInfo>) : ServerMessage

    /** The answer to `diff{ws, scope}` (REQ-13). */
    class DiffAnswer(val diff: Diff) : ServerMessage

    /** The answer to `commits{ws}`. */
    class CommitsAnswer(val list: CommitList) : ServerMessage

    /**
     * Where else the PC can be reached (REQ-61). Only what is valid is kept: a tunnel host must be a quick
     * tunnel, a wifi address a private IPv4 with a port and a 64-hex fingerprint; anything else is null.
     */
    class Endpoints(val lan: LanAddr?, val tunnel: String?) : ServerMessage

    /** A message this app does not know yet; ignored. */
    class Other(val tag: String) : ServerMessage
}

object Control {
    private val json = Json { ignoreUnknownKeys = true }

    fun encode(hello: Hello): String = when (hello) {
        is Hello.Pair -> buildJsonObject {
            put("t", "pair")
            put("token", hello.token)
            put("name", hello.name)
            put("code", hello.code)
        }
        is Hello.Auth -> buildJsonObject {
            put("t", "auth")
            put("device", hello.device)
        }
    }.toString()

    /** `{"t":"workspaces"}` or, with a session, `{"t":"workspaces","session":".."}`. */
    fun workspaces(session: String? = null): String = buildJsonObject {
        put("t", "workspaces")
        if (session != null) put("session", session)
    }.toString()

    fun sessions(): String = """{"t":"sessions"}"""

    fun diff(ws: String, scope: DiffScope = DiffScope.General): String = buildJsonObject {
        put("t", "diff")
        put("ws", ws)
        put("scope", scope.word)
    }.toString()

    fun commits(ws: String): String = buildJsonObject {
        put("t", "commits")
        put("ws", ws)
    }.toString()

    /** Null when the text is not a control message at all. */
    fun parse(text: String): ServerMessage? {
        val obj = try {
            json.parseToJsonElement(text).jsonObject
        } catch (_: Exception) {
            return null
        }
        val tag = obj["t"]?.jsonPrimitive?.contentOrNull ?: return null
        return try {
            when (tag) {
                "paired" -> ServerMessage.Paired(
                    obj.getValue("device").jsonPrimitive.content,
                    obj.getValue("id").jsonPrimitive.content,
                )
                "ready" -> ServerMessage.Ready(
                    bridge = obj["bridge"]?.jsonPrimitive?.intOrNull ?: 0,
                    protocol = obj.getValue("protocol").jsonPrimitive.intOrNull ?: -1,
                    readOnly = obj["read_only"]?.jsonPrimitive?.booleanOrNull ?: false,
                    palettes = Palettes.parse(obj["palettes"]),
                    settings = obj["settings"],
                )
                "error" -> ServerMessage.Error(
                    obj["code"]?.jsonPrimitive?.contentOrNull ?: ErrorCode.UNEXPECTED,
                    obj["msg"]?.jsonPrimitive?.contentOrNull ?: "",
                    obj["ask"]?.jsonPrimitive?.contentOrNull,
                    obj["ws"]?.jsonPrimitive?.contentOrNull,
                    obj["scope"]?.jsonPrimitive?.contentOrNull,
                )
                "workspaces" -> ServerMessage.Workspaces(
                    obj["session"]?.jsonPrimitive?.contentOrNull ?: "",
                    dev.jw.proto.Workspaces.parseItems(obj["items"] as? kotlinx.serialization.json.JsonArray),
                )
                "sessions" -> ServerMessage.Sessions(
                    dev.jw.proto.Workspaces.parseSessions(obj["items"] as? kotlinx.serialization.json.JsonArray),
                )
                "diff" -> Diff.parse(obj)?.let { ServerMessage.DiffAnswer(it) } ?: ServerMessage.Other(tag)
                "commits" -> CommitList.parse(obj)?.let { ServerMessage.CommitsAnswer(it) } ?: ServerMessage.Other(tag)
                "endpoints" -> {
                    val lan = (obj["lan"] as? kotlinx.serialization.json.JsonObject)?.let { l ->
                        val host = l["host"]?.jsonPrimitive?.contentOrNull
                        val port = l["port"]?.jsonPrimitive?.intOrNull
                        val fp = l["fp"]?.jsonPrimitive?.contentOrNull
                        if (host != null && port != null && fp != null) Hosts.lan(host, port, fp) else null
                    }
                    val tunnel = obj["tunnel"]?.jsonPrimitive?.contentOrNull?.takeIf { Hosts.isTunnel(it) }
                    ServerMessage.Endpoints(lan, tunnel)
                }
                else -> ServerMessage.Other(tag)
            }
        } catch (_: Exception) {
            null
        }
    }
}
