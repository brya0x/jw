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

    private fun cmd(i: Int, code: Int? = null, running: Boolean = false) = dev.jw.proto.ShellBlock(i, "cmd$i", "/w", code, 10, running, false, false)

    @Test fun whatAShellSaysOfItselfAndItsCommandsFollowsAndTheLineStaysWhenItIsNoNews() {
        val a = ShellState.merge(null, PaneEvent.Shell(7, true, true, "ls", listOf(cmd(0, running = true))))
        assertEquals(Triple(true, true, "ls"), Triple(a.known, a.running, a.input))
        assertEquals(listOf(0), a.blocks.map { it.i })
        // The command ends: same number, replaced; a new one is added; the line is not news, so it stays.
        val b = ShellState.merge(a, PaneEvent.Shell(7, true, false, null, listOf(cmd(0, 0), cmd(1, running = true))))
        assertEquals(listOf(0, 1), b.blocks.map { it.i })
        assertEquals(0, b.blocks[0].code)
        assertEquals("ls", b.input)
        // The line cleared is news too.
        assertEquals("", ShellState.merge(b, PaneEvent.Shell(7, true, false, "", emptyList())).input)
        // A shell that is not marked is known false.
        assertEquals(false, ShellState.merge(null, PaneEvent.Shell(7, false, false, "", emptyList())).known)
    }

    @Test fun theOldestCommandsGoPastTheCap() {
        val many = (0 until ShellState.KEEP + 3).map { cmd(it) }
        val s = ShellState.merge(null, PaneEvent.Shell(7, true, false, "", many))
        assertEquals(ShellState.KEEP, s.blocks.size)
        assertEquals(3, s.blocks.first().i)
    }

    @Test fun aPaneThatIsNoClaudeSessionIsKnownFalse() {
        assertEquals(false, ChatState.merge(null, event(true, known = false)).known)
    }

    @Test fun theTurnAndTheBackgroundTasksStayUntilThePcSaysAnother() {
        val a = ChatState.merge(null, PaneEvent.Chat(7, true, null, null, true, listOf(block(0)), dev.jw.proto.ChatTurn(1000, 42), 2))
        assertEquals(42L, a.turn?.tokens)
        assertEquals(2, a.bg)
        // A message with neither is no news: both stay.
        val b = ChatState.merge(a, event(false, block(1)))
        assertEquals(1000L, b.turn?.since)
        assertEquals(2, b.bg)
        // The tasks ended: the PC says 0.
        assertEquals(0, ChatState.merge(b, PaneEvent.Chat(7, true, null, null, false, emptyList(), null, 0)).bg)
        // A reset that says nothing of them starts them again.
        val c = ChatState.merge(b, event(true, block(5)))
        assertNull(c.turn)
        assertEquals(0, c.bg)
    }

    @Test fun thePlanStaysUntilANewOneComesOrTheChatStartsAgain() {
        val a = ChatState.merge(null, PaneEvent.Chat(7, true, null, null, true, emptyList(), plan = "# Plan"))
        assertEquals("# Plan", a.plan)
        assertEquals("# Plan", ChatState.merge(a, event(false, block(1))).plan)
        assertEquals("# Otro", ChatState.merge(a, PaneEvent.Chat(7, true, null, null, false, emptyList(), plan = "# Otro")).plan)
        assertNull(ChatState.merge(a, event(true, block(2))).plan)
    }
}
