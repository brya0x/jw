package dev.jw.mobile

import dev.jw.term.Terminal

/**
 * What one command of a shell printed, parsed into a terminal of its own (REQ-95): Warp's grid for each command. The bytes are
 * as the program printed them, laid out for the width the PC's terminal had, so a closed block never changes; how it is drawn
 * for the phone's width is [RowWrap]'s.
 */
class BlockOut(private val cols: Int) {
    private val term = Terminal(cols, ROWS, SCROLLBACK)

    /** Moves with every feed. */
    @Volatile var revision = 0
        private set
    private var cached: Pair<Int, PaneText>? = null

    fun feed(bytes: ByteArray) {
        term.feed(bytes)
        revision++
    }

    /** The output as rows of text, the scrollback and then the screen, the blank end cut. */
    @Synchronized fun text(): PaneText {
        cached?.takeIf { it.first == revision }?.let { return it.second }
        val (lines, n) = term.read { t ->
            val all = (-t.scrollbackSize until t.rows).map { t.line(it) }
            val used = all.indexOfLast { l -> (0 until l.cols).any { l.ch[it] != 0 && l.ch[it] != ' '.code || l.bg[it] != 0 } } + 1
            all.take(used) to used
        }
        val text = PaneText(cols, maxOf(n, 1)).also { it.setScreen(cols, maxOf(n, 1), false, null, lines) }
        cached = revision to text
        return text
    }

    private companion object {
        const val ROWS = 24
        const val SCROLLBACK = 3000
    }
}
