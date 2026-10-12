package dev.jw.mobile

import dev.jw.term.Terminal
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-21, REQ-24: a pane wider than the phone is laid out for the phone's width, never panned and never resized. */
class WrapViewTest {
    private fun term(cols: Int, rows: Int, text: String) = Terminal(cols, rows).also { it.feed(text.toByteArray()) }.toPaneText()

    /** What a row of the view draws, as text. */
    private fun drawn(t: PaneText, row: VRow): String = t.read { p ->
        val l = p.line(row.line)
        row.cols.joinToString("") { c -> if (l.ch[c] == 0) " " else String(Character.toChars(l.ch[c])) }
    }

    private fun rowsOf(t: PaneText, view: Int, line: Int = 0) = wrapView(t, view).rows.filter { it.line == line }.map { drawn(t, it) }

    @Test fun aWordLongerThanTheViewIsCutAtItsWidth() {
        val t = term(100, 10, "a".repeat(70) + "\r\nshort")
        assertEquals(listOf("a".repeat(30), "a".repeat(30), "a".repeat(10)), rowsOf(t, 30))
        assertTrue(wrapView(t, 30).rows.all { r -> t.read { RowWrap.width(it.line(r.line), r.cols) } <= 30 })
    }

    @Test fun aRowThatFitsIsDrawnAsItIsGapsIncluded() {
        val t = term(80, 5, "a    b\u001b[?25l")
        assertEquals(listOf("a    b"), rowsOf(t, 20))
    }

    @Test fun theBlankEndOfARowIsNotDrawn() {
        val t = term(200, 5, "hi\u001b[?25l")
        assertEquals(listOf("hi"), rowsOf(t, 40))
    }

    /** `ls -a` in a terminal 195 wide: names in columns with wide gaps, which a phone breaks in the middle of a word. */
    @Test fun columnsLaidOutForAWideTerminalBreakAtSpacesAndLoseTheirGaps() {
        fun cell(s: String) = s.padEnd(20)
        val names = listOf(".", ".git", ".gitignore", "AGENTS.md", "Cargo.lock", "CLAUDE.md", "docs", "LICENSE", "scripts")
        val t = term(195, 10, names.joinToString("") { cell(it) }.trimEnd() + "\u001b[?25l")
        val rows = rowsOf(t, 44)
        // Every name is whole on some row, and no row is wider than the view.
        for (n in names.drop(1)) assertTrue("$n whole in $rows", rows.any { r -> r.split(" ").contains(n) })
        assertTrue(rows.all { it.length <= 44 })
        // The gaps are two spaces at most.
        assertTrue("no long gaps in $rows", rows.none { "   " in it })
        assertTrue(rows.size in 2..4)
    }

    @Test fun aBorderIsOneRowEvenWhenItCarriesALabel() {
        val t = term(196, 5, "─".repeat(150) + " mobile-fixes " + "─".repeat(32) + "\u001b[?25l")
        val rows = rowsOf(t, 44)
        assertEquals(1, rows.size)
        assertTrue(rows[0].all { it == '─' })
        assertEquals(44, rows[0].length)
    }

    @Test fun aWideCharacterIsNotSplitAcrossRows() {
        val t = term(40, 5, "ab" + "漢".repeat(5) + "\u001b[?25l")
        val w = wrapView(t, 5)
        val rows = w.rows.filter { it.line == 0 }
        assertTrue("no row is wider than the view", rows.all { r -> t.read { RowWrap.width(it.line(0), r.cols) } <= 5 })
        assertEquals(7, rows.sumOf { it.cols.size })
    }

    @Test fun theCursorInAGapIsStillOnTheRow() {
        // The cursor is at column 30, in the middle of a long gap that is shrunk.
        val t = term(80, 5, "start" + " ".repeat(40) + "end" + "\u001b[1;31H")
        val w = wrapView(t, 20)
        assertTrue(w.rows.any { 30 in it.cols })
    }

    @Test fun aColumnIsFoundUnderAnXAndBack() {
        val t = term(195, 5, "one" + " ".repeat(30) + "two" + " ".repeat(30) + "three\u001b[?25l")
        val w = wrapView(t, 20)
        t.read { p ->
            val l = p.line(0)
            val row = w.rows.first { r -> "two" in drawn(t, r) }
            val twoAt = drawn(t, row).indexOf("two")
            assertEquals('t'.code, l.ch[RowWrap.colAt(l, row, twoAt)])
            assertEquals(twoAt, RowWrap.xOf(l, row, row.cols[twoAt]))
            assertEquals(-1, RowWrap.xOf(l, row, 999))
        }
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

    @Test fun noRowIsEmptyOfDrawnColumnsExceptABlankOne() {
        val t = term(195, 5, "a" + " ".repeat(100) + "b\u001b[?25l")
        assertFalse(wrapView(t, 10).rows.filter { it.line == 0 }.any { it.cols.isEmpty() })
    }

    /** A shell's prompt, which the phone leaves out until a command is sent (its input is the line, outside the text). */
    @Test fun theRowsFromTheShellsPromptDownAreLeftOut() {
        val t = term(40, 8, "prompt % ls\r\none  two\r\n\r\nmobile-fixes on main\r\n> ").let { screen ->
            val lines = screen.read { p -> (0 until p.rows).map { p.line(it) } }
            PaneText(40, 8).also { it.setScreen(40, 8, false, 4 to 2, lines, false, 3) }
        }
        val rows = wrapView(t, 40).rows
        assertEquals(listOf(0, 1, 2), rows.map { it.line })
        assertEquals("prompt % ls", drawn(t, rows[0]))
        // The view rests on what is left: the prompt is not the last row, nor is the cursor drawn on one.
        assertEquals(0, wrapView(t, 40).firstRow(10, 0))
        // With no prompt row (a command runs, or it is no shell of jw's) every row is there.
        val all = PaneText(40, 8).also { it.setScreen(40, 8, false, 4 to 2, t.read { p -> (0 until p.rows).map { p.line(it) } }) }
        assertTrue(wrapView(all, 40).rows.any { it.line == 4 })
    }
}
