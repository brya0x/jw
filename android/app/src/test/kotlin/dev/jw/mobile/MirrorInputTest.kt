package dev.jw.mobile

import dev.jw.term.Terminal
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** REQ-93: the phone's input and the terminal's are one text. */
class MirrorInputTest {
    private val e = "\u001b"
    private fun screen(text: String) = Terminal(60, 12).also { it.feed(text.toByteArray()) }.toPaneText()

    @Test fun typingAtTheEndAddsAndDeletingAtTheEndBackspaces() {
        assertArrayEquals(byteArrayOf(), MirrorTyping.bytes("ab", "ab"))
        assertArrayEquals("c".toByteArray(), MirrorTyping.bytes("ab", "abc"))
        assertArrayEquals(byteArrayOf(0x7f, 0x7f), MirrorTyping.bytes("abc", "a"))
        assertArrayEquals("hola".toByteArray(), MirrorTyping.bytes("", "hola"))
    }

    @Test fun anyOtherEditClearsTheLineAndTypesItAgain() {
        assertArrayEquals(byteArrayOf(0x15) + "axc".toByteArray(), MirrorTyping.bytes("abc", "axc"))
        // A character of two code units is one backspace.
        assertArrayEquals(byteArrayOf(0x7f), MirrorTyping.bytes("a😀", "a"))
    }

    @Test fun theTextInClaudesBoxIsRead() {
        val s = screen("● hecho\r\n$e[2m────────────────────$e[0m\r\n> revisa el test\r\n$e[2m────────────────────$e[0m\r\n  ? for shortcuts")
        assertEquals("revisa el test", ClaudeInput.read(s))
    }

    @Test fun aBoxWithTheFrameDrawnRoundItIsRead() {
        val s = screen("╭──────────────╮\r\n│ > hola mundo │\r\n╰──────────────╯\r\n")
        assertEquals("hola mundo", ClaudeInput.read(s))
    }

    @Test fun thePlaceholderOfAnEmptyBoxIsNotText() {
        val s = screen("────────────────\r\n> $e[2mTry \"fix the tests\"$e[0m\r\n────────────────\r\n")
        assertEquals("", ClaudeInput.read(s))
    }

    @Test fun aLongerTextKeepsItsRows() {
        val s = screen("────────────────\r\n❯ primera\r\n  segunda\r\n────────────────\r\n")
        assertEquals("primera\nsegunda", ClaudeInput.read(s))
    }

    @Test fun aScreenWithNoBoxHasNoInput() {
        assertNull(ClaudeInput.read(screen("solo texto\r\nsin caja\r\n")))
        assertNull(ClaudeInput.read(screen("$e[2m────────────────$e[0m\r\nDo you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n")))
    }
}
