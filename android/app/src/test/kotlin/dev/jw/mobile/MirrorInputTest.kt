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

    @Test fun theNoBreakSpaceClaudeDrawsAfterItsPromptIsASpace() {
        // What Claude really prints: the prompt, U+00A0, then the text.
        val s = screen("────────────────\r\n❯\u00a0pcx\r\n────────────────\r\n  ⏵⏵ auto mode on")
        assertEquals("pcx", ClaudeInput.read(s))
        assertEquals("", ClaudeInput.read(screen("────────────────\r\n❯\u00a0\r\n────────────────\r\n")))
    }

    @Test fun aBorderThatCarriesTheSessionsNameIsStillABorder() {
        // Claude writes the name of the conversation in the border above its box.
        val s = screen("──────────────────────── mobile-fixes ──\r\n❯\u00a0pcx\r\n────────────────────────────────────\r\n  ⏵⏵ auto mode on")
        assertEquals("pcx", ClaudeInput.read(s))
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

    /** The rows the daemon really sent for a Claude at work, on its alternate screen (cols 196): border, prompt, border, footer. */
    @Test fun theBoxAsTheDaemonSendsItIsRead() {
        fun runs(vararg r: Triple<Int, String, Int>) = r.map { dev.jw.proto.TextRun(it.first, it.second, it.third, 0, 0) }
        val border = "─".repeat(196)
        val rows = List(101) { emptyList<dev.jw.proto.TextRun>() }.toMutableList()
        rows[95] = runs(Triple(0, "  ⎿ \u00a0Tip: Use /theme", 43620761))
        rows[97] = runs(Triple(0, "─".repeat(150) + " mobile-fixes ──────────", 42502280))
        rows[98] = runs(Triple(0, "❯\u00a0", 43620761), Triple(2, "pcxpcx", 0))
        rows[99] = runs(Triple(0, border, 42502280))
        rows[100] = runs(Triple(0, "  ", 0), Triple(2, "⏵⏵ auto mode on", 50315527))
        val t = PaneText(196, 101)
        t.setScreen(196, 101, true, 98 to 8, rows.map { PaneText.lineOf(it, 196) })
        assertEquals("pcxpcx", ClaudeInput.read(t))
    }
}
