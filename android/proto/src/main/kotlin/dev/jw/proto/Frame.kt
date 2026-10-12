package dev.jw.proto

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer

/** The protocol this app was built for; `ready.protocol` must equal it. */
const val PROTOCOL: Int = 17

/** One jw-proto frame as a WebSocket binary message: `write_frame` minus the 4-byte length. */
sealed interface Frame {
    /** Kind 0: a JSON message. */
    class Json(val text: String) : Frame {
        override fun toString() = "Json(${text.length} chars)"
    }

    /** Kind 1: a JSON head and raw bytes. */
    class Raw(val head: String, val bytes: ByteArray) : Frame {
        override fun toString() = "Raw(head=${head.length} chars, ${bytes.size} bytes)"
    }
}

class FrameException(msg: String) : Exception(msg)

object FrameCodec {
    const val MAX_FRAME = 16 * 1024 * 1024

    fun encode(frame: Frame): ByteArray {
        val out = ByteArrayOutputStream()
        when (frame) {
            is Frame.Json -> {
                out.write(0)
                out.write(frame.text.toByteArray(Charsets.UTF_8))
            }
            is Frame.Raw -> {
                val head = frame.head.toByteArray(Charsets.UTF_8)
                out.write(1)
                out.write(ByteBuffer.allocate(4).putInt(head.size).array())
                out.write(head)
                out.write(frame.bytes)
            }
        }
        return out.toByteArray()
    }

    fun decode(body: ByteArray): Frame {
        if (body.isEmpty()) throw FrameException("empty frame")
        if (body.size > MAX_FRAME) throw FrameException("frame too large")
        return when (body[0].toInt()) {
            0 -> Frame.Json(String(body, 1, body.size - 1, Charsets.UTF_8))
            1 -> {
                if (body.size < 5) throw FrameException("short frame")
                val n = ByteBuffer.wrap(body, 1, 4).int
                if (n < 0 || n > body.size - 5) throw FrameException("bad head length")
                Frame.Raw(
                    String(body, 5, n, Charsets.UTF_8),
                    body.copyOfRange(5 + n, body.size),
                )
            }
            else -> throw FrameException("unknown frame kind")
        }
    }
}
