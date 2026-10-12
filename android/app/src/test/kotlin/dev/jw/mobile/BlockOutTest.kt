package dev.jw.mobile

import dev.jw.term.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-92: what a command printed is parsed into a terminal of its own and drawn complete, for the phone's width. */
class BlockOutTest {
    private fun rows(o: BlockOut) = o.text().let { t -> t.read { p -> (0 until p.rows).map { p.line(it).text() } } }

    @Test fun theBytesAreParsedColoursAndLinesAndTheBlankEndIsCut() {
        val o = BlockOut(80)
        o.feed("\u001b[1;32mok\u001b[0m first\r\nsecond\r\n\r\n\r\n".toByteArray())
        assertEquals(listOf("ok first", "second"), rows(o))
        val l = o.text().read { it.line(0) }
        assertEquals(Color.index(2), l.fg[0])
        assertTrue(l.flags[0] and dev.jw.term.Attr.BOLD != 0)
    }

    @Test fun moreBytesAddToWhatIsHeldAndTheTextFollows() {
        val o = BlockOut(80)
        o.feed("one\r\n".toByteArray())
        val first = o.revision
        assertEquals(listOf("one"), rows(o))
        o.feed("tw".toByteArray())
        o.feed("o\r\n".toByteArray())
        assertTrue(o.revision > first)
        assertEquals(listOf("one", "two"), rows(o))
    }

    @Test fun anOutputLongerThanTheScreenKeepsItsEarlyRows() {
        val o = BlockOut(40)
        o.feed((1..200).joinToString("") { "line $it\r\n" }.toByteArray())
        val r = rows(o)
        assertEquals("line 1", r.first())
        assertEquals("line 200", r.last())
        assertEquals(200, r.size)
    }

    @Test fun aProgressBarThatOverwritesItsLineEndsAsItsLastState() {
        val o = BlockOut(40)
        o.feed("10%\r50%\r100%\r\ndone\r\n".toByteArray())
        assertEquals(listOf("100%", "done"), rows(o))
    }

    @Test fun anEmptyOutputIsOneBlankRowNotNone() {
        assertEquals(1, BlockOut(40).text().rows)
    }

    @Test fun outputLaidOutForAWideTerminalIsLaidOutAgainForANarrowPhone() {
        // 195 columns wide, as the PC's: a row of names far apart, which the phone shrinks and breaks at spaces.
        val o = BlockOut(195)
        o.feed((".".padEnd(30) + ".git".padEnd(30) + ".gitignore".padEnd(30) + "AGENTS.md\r\n").toByteArray())
        val view = wrapView(o.text(), 44, false).rows
        assertTrue("fits the phone: ${view.size} rows", view.size in 1..3)
    }
}
