package dev.jw.proto

import kotlinx.serialization.json.Json
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

/** Palette roles from `ready.palettes` (or the built-in fallback), as `#rrggbb`. */
class Palette(val roles: Map<String, String>) {
    operator fun get(role: String): String? = roles[role]
}

class Palettes(val dark: Palette, val light: Palette) {
    companion object {
        val ROLES = listOf(
            "bg", "panel", "fg", "dim", "line", "sel",
            "blue", "green", "yellow", "red", "magenta", "cyan",
        )

        /** One Dark and One Light, the roles of the approved prototype. */
        val FALLBACK = Palettes(
            Palette(
                mapOf(
                    "bg" to "#282c34", "panel" to "#21252b", "fg" to "#abb2bf",
                    "dim" to "#5c6370", "line" to "#3e4451", "sel" to "#2c313a",
                    "blue" to "#61afef", "green" to "#98c379", "yellow" to "#e5c07b",
                    "red" to "#e06c75", "magenta" to "#c678dd", "cyan" to "#56b6c2",
                ),
            ),
            Palette(
                mapOf(
                    "bg" to "#fafafa", "panel" to "#f0f0f1", "fg" to "#383a42",
                    "dim" to "#a0a1a7", "line" to "#dbdbdc", "sel" to "#e5e5e6",
                    "blue" to "#4078f2", "green" to "#50a14f", "yellow" to "#c18401",
                    "red" to "#e45649", "magenta" to "#a626a4", "cyan" to "#0184bc",
                ),
            ),
        )

        /**
         * W1 has not landed, so the shape is assumed: `{"dark":{role:"#rrggbb"},"light":{..}}`.
         * Anything missing or malformed falls back role by role.
         */
        fun parse(el: JsonElement?): Palettes {
            val obj = el as? JsonObject ?: return FALLBACK
            fun side(key: String, fb: Palette): Palette {
                val o = obj[key] as? JsonObject ?: return fb
                val roles = fb.roles.toMutableMap()
                for (r in ROLES) {
                    val v = (o[r] as? JsonPrimitive)?.contentOrNull
                    if (v != null && HEX.matches(v)) roles[r] = v
                }
                return Palette(roles)
            }
            return Palettes(side("dark", FALLBACK.dark), side("light", FALLBACK.light))
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

    class Error(val code: String, val msg: String) : ServerMessage

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
                )
                else -> ServerMessage.Other(tag)
            }
        } catch (_: Exception) {
            null
        }
    }
}
