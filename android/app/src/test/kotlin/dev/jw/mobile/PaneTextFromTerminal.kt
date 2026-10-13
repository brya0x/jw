package dev.jw.mobile

import dev.jw.term.Terminal

/** The pane a terminal that was fed these bytes would be, for the tests that write a screen as escape codes. */
fun Terminal.toPaneText(): PaneText = read { t ->
    PaneText(t.cols, t.rows).also {
        it.setScreen(t.cols, t.rows, t.altScreen, if (t.cursorVisible) t.cursorRow to t.cursorCol else null, (0 until t.rows).map(t::line))
    }
}
