package dev.jw.mobile

import dev.jw.proto.ChatBlock
import dev.jw.proto.PaneEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** REQ-90: the blocks of a conversation merge by their number, and the title and the cap are kept. */
class ChatStateTest {
    private fun block(i: Int, text: String = "m$i", result: String? = null) = ChatBlock(i, "tool", text, "Bash", "ls", result, null, false)
    private fun event(reset: Boolean, vararg blocks: ChatBlock, title: String? = null, name: String? = null, known: Boolean = true) =
        PaneEvent.Chat(7, known, name, title, reset, blocks.toList())

    @Test fun aResetIsAllThePhoneHasAndANewBlockIsAppended() {
        val a = ChatState.merge(null, event(true, block(1), block(0), title = "Chat"))
        assertEquals(listOf(0, 1), a.blocks.map { it.i })
        assertEquals("Chat", a.title)
        val b = ChatState.merge(a, event(false, block(2)))
        assertEquals(listOf(0, 1, 2), b.blocks.map { it.i })
        assertEquals("Chat", b.title)
    }

    @Test fun aBlockWithANumberTheyHoldReplacesIt() {
        val a = ChatState.merge(null, event(true, block(0), block(1)))
        val b = ChatState.merge(a, event(false, block(1, result = "ok")))
        assertEquals(2, b.blocks.size)
        assertNull(b.blocks[0].result)
        assertEquals("ok", b.blocks[1].result)
    }

    @Test fun aNewTitleReplacesTheOldAndAResetStartsAgain() {
        val a = ChatState.merge(null, event(true, block(0), title = "Uno"))
        assertEquals("Dos", ChatState.merge(a, event(false, title = "Dos")).title)
        val again = ChatState.merge(a, event(true, block(5)))
        assertEquals(listOf(5), again.blocks.map { it.i })
        assertEquals("Uno", again.title)
    }

    @Test fun theOldestBlocksGoPastTheCap() {
        val many = (0 until ChatState.KEEP + 5).map { block(it) }.toTypedArray()
        val a = ChatState.merge(null, event(true, *many))
        assertEquals(ChatState.KEEP, a.blocks.size)
        assertEquals(5, a.blocks.first().i)
    }

    @Test fun theNameTheTitleAndTheCommandsOfAShellAreKept() {
        val a = ChatState.merge(null, event(true, block(0), name = "mobile-fixes", title = "Arreglar el socket"))
        assertEquals("mobile-fixes" to "Arreglar el socket", a.name to a.title)
        // A block with no name in the message keeps the one held; a new name replaces it.
        val b = ChatState.merge(a, event(false, block(1)))
        assertEquals("mobile-fixes", b.name)
        assertEquals("otro", ChatState.merge(b, event(false, name = "otro")).name)
    }

    private fun cmd(i: Int, out: String = "", code: Int? = null, running: Boolean = false) =
        dev.jw.proto.ShellBlock(i, "cmd$i", "/w", out, 0, code, 10, running, false)

    @Test fun theBlocksOfAShellMergeByNumberAndTheLineBeingTypedFollows() {
        val a = ShellState.merge(null, PaneEvent.Shell(7, true, true, listOf(cmd(0, running = true)), "ls"))
        assertEquals(listOf(0), a.blocks.map { it.i })
        assertEquals("ls", a.input)
        // The running command ends: same number, replaced; the line is not news, so it stays.
        val b = ShellState.merge(a, PaneEvent.Shell(7, true, false, listOf(cmd(0, "ok", 0), cmd(1)), null))
        assertEquals(listOf(0, 1), b.blocks.map { it.i })
        assertEquals(0, b.blocks[0].code)
        assertEquals("ls", b.input)
        // The line cleared is news too.
        assertEquals("", ShellState.merge(b, PaneEvent.Shell(7, true, false, emptyList(), "")).input)
        // A shell that is not marked is known false.
        assertEquals(false, ShellState.merge(null, PaneEvent.Shell(7, false, true, emptyList(), "")).known)
    }

    @Test fun aPaneThatIsNoClaudeSessionIsKnownFalse() {
        assertEquals(false, ChatState.merge(null, event(true, known = false)).known)
    }
}
