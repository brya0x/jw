package dev.jw.term

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What xterm does and the vt100 crate does not (or does differently), so the golden streams cannot say it. */
class XtermTest {
    private val e = "\u001b"

    private fun term(cols: Int, rows: Int, s: String, scrollback: Int = 5000) =
        Terminal(cols, rows, scrollback).also { it.feed(s.toByteArray()) }

    private fun Terminal.row(i: Int) = read { line(i).text() }

    @Test fun insertMode() {
        val t = term(12, 2, "abcdef$e[1G$e[4hXY$e[4lZ")
        assertEquals("XYZbcdef", t.row(0))
    }

    @Test fun customTabStops() {
        val t = term(30, 3, "$e[3g$e[1;5H$e" + "H$e[1;1H\tx")
        assertEquals("    x", t.row(0))
        assertEquals(5, t.cursorCol)
    }

    @Test fun decLineDrawing() {
        val t = term(20, 2, "$e(0lqqqk$e(B lqk$e(0x$e(B\r\n$e)0\u000elqk\u000f lq")
        assertEquals("┌───┐ lqk│", t.row(0))
        assertEquals("┌─┐ lq", t.row(1))
    }

    @Test fun tabForwardAndBackward() {
        val t = term(30, 1, "\tx$e[1I$e[Zy")
        assertEquals("        y", t.row(0))
    }

    @Test fun withoutAutowrapTheLastColumnIsOverwritten() {
        assertEquals("012345678Z", term(10, 2, "$e[?7l0123456789ZZ").row(0))
    }

    @Test fun repeatsThePreviousCharacter() {
        assertEquals("abbbbz", term(10, 1, "ab$e[3bz").row(0))
    }

    @Test fun hvpMovesTheCursorLikeCup() {
        assertEquals("  X", term(10, 3, "$e[2;3fX").row(1))
    }

    @Test fun erasingKeepsOnlyTheBackground() {
        val t = term(6, 2, "$e[1;4;41mabc$e[2K")
        t.read {
            val l = it.line(0)
            assertEquals(Color.index(1), l.bg[0])
            assertEquals(0, l.flags[0])
        }
    }

    @Test fun colonSeparatedColours() {
        val t = term(10, 1, "$e[38:2::1:2:3ma$e[48:2:9:8:7mb$e[38:5:99mc")
        t.read {
            val l = it.line(0)
            assertEquals(Color.rgb(1, 2, 3), l.fg[0])
            assertEquals(Color.rgb(9, 8, 7), l.bg[1])
            assertEquals(Color.index(99), l.fg[2])
        }
    }

    @Test fun badUtf8BecomesReplacementCharacters() {
        val t = Terminal(20, 1)
        t.feed(byteArrayOf('a'.code.toByte(), 0xC3.toByte(), '('.code.toByte(), 0xFF.toByte(), 'b'.code.toByte()))
        assertEquals("a�(�b", t.row(0))
    }

    @Test fun scrolledLinesGoToTheScrollbackWhichAltDoesNotKeep() {
        val t = term(5, 2, "a\r\nb\r\nc\r\nd")
        assertEquals(2, t.scrollbackSize)
        assertEquals("a", t.read { it.line(-2).text() })
        assertEquals("b", t.read { it.line(-1).text() })
        t.feed("$e[?1049h1\r\n2\r\n3\r\n4".toByteArray())
        assertEquals(2, t.scrollbackSize)
        t.feed("$e[?1049l".toByteArray())
        assertFalse(t.altScreen)
        assertEquals("c", t.row(0))
    }

    @Test fun scrollbackIsBounded() {
        val t = term(5, 2, (1..50).joinToString("\r\n"), scrollback = 10)
        assertEquals(10, t.scrollbackSize)
    }

    @Test fun mouseAndKeyModes() {
        val t = term(10, 2, "$e[?1002h$e[?1006h$e[?1h$e[?2004h")
        assertEquals(MouseMode.ButtonMotion, t.mouseMode)
        assertEquals(MouseEncoding.Sgr, t.mouseEncoding)
        assertTrue(t.appCursor)
        assertTrue(t.bracketedPaste)
        t.feed("$e[?1002l$e[?1006l".toByteArray())
        assertEquals(MouseMode.None, t.mouseMode)
        assertEquals(MouseEncoding.Default, t.mouseEncoding)
    }

    @Test fun resizeKeepsTheCursorRowOnScreen() {
        val t = term(10, 6, "1\r\n2\r\n3\r\n4\r\n5\r\n6")
        t.resize(10, 3)
        assertEquals(3, t.rows)
        assertEquals("4", t.row(0))
        assertEquals("6", t.row(2))
        assertEquals(2, t.cursorRow)
        assertEquals("3", t.read { it.line(-1).text() })
        t.resize(4, 5)
        assertEquals(4, t.cols)
        assertEquals(5, t.rows)
        assertEquals("4", t.row(0))
    }

    @Test fun resizeClipsAWideCharacterThatWouldBeCut() {
        val t = term(6, 1, "abcd中")
        t.resize(5, 1)
        assertEquals("abcd", t.row(0))
    }

    @Test fun combiningMarksJoinTheCellBeforeThem() {
        val t = term(5, 1, "éx")
        assertEquals("éx", t.row(0))
        assertEquals(2, t.cursorCol)
    }

    @Test fun theTerminalKeepsNoBytesOfTheStreamInItsToString() {
        val t = term(5, 1, "secret")
        assertFalse(t.toString().contains("secret"))
    }
}
