package dev.jw.mobile

import dev.jw.term.Terminal
import org.junit.Assert.assertEquals
import org.junit.Test

/** Where a pane taller than the phone's view rests (REQ-21): its prompt or footer must be on screen. */
class BottomAnchorTest {
    private val e = "\u001b"
    private fun term(rows: Int, s: String) = Terminal(40, rows).also { it.feed(s.toByteArray()) }.toPaneText()

    // The terminal is as wide as the view, so each row of it is one row of the view.
    private fun anchor(t: PaneText, visible: Int) = wrapView(t, 40).firstRow(visible, 0)

    @Test fun aShellPromptNearTheTopEndsTheView() {
        assertEquals(0, anchor(term(112, "a\r\nb\r\nprompt"), 20))
        assertEquals(0, anchor(term(10, "x"), 20))
    }

    @Test fun textBelowTheCursorStaysVisible() {
        val t = term(112, "top\r\n\r\n\r\nfooter$e[3A")
        assertEquals(1, anchor(t, 3)) // rows 1..3 end at the footer
        val tall = term(112, (1..30).joinToString("\r\n") { "l$it" } + "\r\nfooter$e[3A")
        // footer is on row 30 though the cursor is at row 28: the view ends there
        assertEquals(30 + 1 - 20, anchor(tall, 20))
    }

    @Test fun aFullScreenAppDrawnFromTheTopStartsAtTheTop() {
        // An agent's dialog on the alternate screen with its cursor hidden, in a pane taller than the phone: the top shows.
        val t = term(55, "$e[?1049h$e[?25l$e[1;1Hhead$e[20;1Hdialog line")
        assertEquals(0, anchor(t, 38))
        // Dragged down the pane it can reach the rest, and never above its first row.
        assertEquals(0 to -(55 - 38), wrapView(t, 40).scrollRange(38))
    }

    @Test fun aFullScreenAppFollowsItsCursorWhenItIsBelowTheView() {
        val t = term(55, "$e[?1049h$e[50;1Hinput")
        assertEquals(49 + 1 - 38, anchor(t, 38)) // row 50 is index 49
    }

    @Test fun aMainScreenCannotBeDraggedBelowItsRestingPlace() {
        val t = term(112, "a\r\nb\r\nprompt")
        val (up, down) = wrapView(t, 40).scrollRange(20)
        assertEquals(0, down)
        assertEquals(0, up) // nothing in the history yet
    }
}
