package dev.jw.mobile

import dev.jw.term.Terminal
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-24: a pane wider than the phone is broken at the phone's width, never panned and never resized. */
class WrapViewTest {
    private fun term(cols: Int, rows: Int, text: String) = Terminal(cols, rows).also { it.feed(text.toByteArray()) }.toPaneText()

    @Test fun aRowWiderThanTheViewBreaksIntoRowsThatFit() {
        val t = term(100, 10, "a".repeat(70) + "\r\nshort")
        val w = wrapView(t, 30)
        val rows = w.rows.filter { it.line == 0 }
        assertEquals(listOf(0 to 30, 30 to 60, 60 to 70), rows.map { it.from to it.to })
        assertTrue(w.rows.all { it.to - it.from <= 30 })
    }

    @Test fun theBlankEndOfARowIsNotDrawn() {
        val t = term(200, 5, "hi\u001b[?25l")
        val row = wrapView(t, 40).rows.first { it.line == 0 }
        assertEquals(0 to 2, row.from to row.to)
    }

    @Test fun theViewRestsOnTheLastUsedRowAndScrollsUpToTheTop() {
        val t = term(80, 50, "one\r\ntwo\r\nthree")
        val w = wrapView(t, 40)
        // 3 used rows, view of 2 rows: rests on the last two, and may go up one.
        assertEquals(1, w.firstRow(2, 0))
        assertEquals(0, w.firstRow(2, 1))
        assertEquals(1, w.scrollRange(2).first)
        assertEquals(0, w.scrollRange(10).first)
    }

    @Test fun aWideCharacterIsNotSplitAcrossRows() {
        val t = term(40, 5, "ab" + "漢".repeat(5) + "\u001b[?25l")
        val w = wrapView(t, 5)
        assertTrue("no row ends in the first half of a wide cell", w.rows.filter { it.line == 0 }.all { it.to - it.from <= 5 })
        assertEquals(12, w.rows.filter { it.line == 0 }.sumOf { it.to - it.from })
    }
}
