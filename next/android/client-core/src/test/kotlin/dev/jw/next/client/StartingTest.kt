package dev.jw.next.client

import dev.jw.next.contract.Block
import dev.jw.next.contract.BlocksMode
import dev.jw.next.contract.Node
import dev.jw.next.contract.Span
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-259: a program that has the screen and has painted nothing is starting. */
class StartingTest {
    private fun block(cmd: String = "claude", running: Boolean = true, full: Boolean = false, vararg rows: String) = Block(
        id = 1, cmd = cmd, cwd = "/w", started = 0, running = running, full = full,
        out = rows.map { listOf(Span(it)) },
    )

    private fun node(mode: BlocksMode, vararg blocks: Block) =
        Node.Blocks(id = "b", v = 1, epoch = "e", rev = 1, attention = emptyList(), pane = "p", mode = mode, blocks = blocks.toList())

    @Test
    fun aRunningProgramOnTheNormalScreenWithNothingDrawnIsStarting() {
        assertNotNull(node(BlocksMode.Full, block(rows = arrayOf("", "   "))).starting())
        assertEquals("claude", programName(node(BlocksMode.Full, block()).starting()!!.cmd))
    }

    @Test
    fun onceARowHasAnythingButBlanksItIsNotStarting() {
        assertNull(node(BlocksMode.Full, block(rows = arrayOf("╭─ claude"))).starting())
    }

    @Test
    fun anAlternateScreenABlocksViewAndAFinishedCommandAreNeverStarting() {
        assertNull(node(BlocksMode.Full, block(full = true)).starting(), "an alternate screen is blank and its own")
        assertNull(node(BlocksMode.Blocks, block()).starting(), "the blocks are drawn")
        assertNull(node(BlocksMode.Full, block(running = false)).starting())
        assertNull(node(BlocksMode.Full).starting(), "no block at all")
    }

    @Test
    fun aCommandNamesItsProgramByTheFileNameOfItsFirstWord() {
        assertEquals("claude", programName("claude --resume 12"))
        assertEquals("claude", programName("/usr/local/bin/claude"))
        assertEquals("nvim", programName("  nvim ."))
        assertTrue(programName("").isEmpty())
    }
}
