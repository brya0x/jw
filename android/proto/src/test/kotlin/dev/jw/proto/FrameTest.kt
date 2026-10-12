package dev.jw.proto

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameTest {
    @Test fun jsonFrameFromHandBuiltBytes() {
        val body = byteArrayOf(0) + """{"t":"x"}""".toByteArray()
        val f = FrameCodec.decode(body) as Frame.Json
        assertEquals("""{"t":"x"}""", f.text)
        assertArrayEquals(body, FrameCodec.encode(f))
    }

    @Test fun rawFrameWith256Bytes() {
        val head = """{"pane":1}""".toByteArray()
        val raw = ByteArray(256) { it.toByte() }
        val body = byteArrayOf(1, 0, 0, 0, head.size.toByte()) + head + raw
        val f = FrameCodec.decode(body) as Frame.Raw
        assertEquals("""{"pane":1}""", f.head)
        assertArrayEquals(raw, f.bytes)
        assertArrayEquals(body, FrameCodec.encode(f))
    }

    @Test fun headLengthIsBigEndian() {
        val head = "h".repeat(300)
        val enc = FrameCodec.encode(Frame.Raw(head, byteArrayOf(9)))
        assertEquals(1, enc[0].toInt())
        assertEquals(1, enc[3].toInt())
        assertEquals(44, enc[4].toInt())
    }

    @Test fun rejectsBadFrames() {
        for (bad in listOf(byteArrayOf(), byteArrayOf(2, 1), byteArrayOf(1, 0, 0), byteArrayOf(1, 0, 0, 0, 9, 1))) {
            try {
                FrameCodec.decode(bad)
                throw AssertionError("accepted")
            } catch (_: FrameException) {
            }
        }
    }

    @Test fun toStringHidesBytes() {
        val s = Frame.Raw("{}", "secret".toByteArray()).toString()
        assertTrue(!s.contains("secret"))
    }
}
