package dev.jw.mobile

import dev.jw.proto.ChatBlock
import dev.jw.proto.PaneEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** REQ-90: the blocks of a conversation merge by their number, and the title and the cap are kept. */
class ChatStateTest {
    private fun block(i: Int, text: String = "m$i", result: String? = null) = ChatBlock(i, "tool", text, "Bash", "ls", result, null, false)
    private fun event(reset: Boolean, vararg blocks: ChatBlock, title: String? = null, known: Boolean = true) =
        PaneEvent.Chat(7, known, title, reset, blocks.toList())

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

    @Test fun aPaneThatIsNoClaudeSessionIsKnownFalse() {
        assertEquals(false, ChatState.merge(null, event(true, known = false)).known)
    }
}
