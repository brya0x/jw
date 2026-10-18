package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.OlderItems
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-567 and REQ-569: the older items the phone holds beside the node's window. */
class OlderTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun node(first: Int, vararg items: String, epoch: String = "e1"): Node.Conversation {
        val base = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, "snapshot-conversation-running.json").readText()) as DaemonMsg.Snapshot).node as Node.Conversation
        return base.copy(first = first, items = items.map { ConvItem.Note(it) }, older = 5, epoch = epoch)
    }

    private fun page(vararg items: String, older: Long? = null) = Result.success(OlderItems(items.map { ConvItem.Note(it) }, older))

    private fun names(items: List<ConvItem>) = items.map { (it as ConvItem.Note).text }

    /** The older items go before the node's, and what leaves the head of the window goes to the end of them: the number of an item does not change. */
    @Test
    fun theOlderItemsAndWhatLeavesTheWindowStayInARow() {
        val h = OlderHeld()
        val n1 = node(10, "d", "e", "f")
        h.sync(n1)
        val ask1 = h.ask(n1, true)
        assertEquals(5L, ask1?.before)
        assertTrue(h.replied(ask1!!, page("a", "b", "c", older = 3)))
        assertEquals(listOf("a", "b", "c", "d", "e", "f"), names(h.all(n1)))
        assertEquals(7, h.first(n1), "the number of `a`; `d` is still 10")
        // two items come and two leave the window
        val n2 = node(12, "f", "g", "h")
        h.sync(n2)
        h.sync(n2)
        assertEquals(listOf("a", "b", "c", "d", "e", "f", "g", "h"), names(h.all(n2)))
        assertEquals(7, h.first(n2), "the same: `f` is 12 in both")
        // a node that starts somewhere else is another conversation
        val n3 = node(2, "x")
        h.sync(n3)
        assertEquals(listOf("x"), names(h.all(n3)))
    }

    @Test
    fun aWindowThatDoesNotJoinOrAnotherEpochStartsAgain() {
        val h = OlderHeld()
        h.sync(node(10, "d"))
        h.replied(h.ask(node(10, "d"), true)!!, page("c"))
        val gap = node(40, "z")
        h.sync(gap)
        assertEquals(listOf("z"), names(h.all(gap)))
        val h2 = OlderHeld()
        h2.sync(node(10, "d"))
        val inFlight = h2.ask(node(10, "d"), true)!!
        val other = node(10, "d", epoch = "e2")
        h2.sync(other)
        assertEquals(listOf("d"), names(h2.all(other)))
        // the answer to a request of before the epoch changed is not taken, though the next one asks from the same time
        val again = h2.ask(other, true)!!
        assertFalse(h2.replied(inFlight, page("viejo")))
        assertTrue(h2.replied(again, page("c")))
        assertEquals(listOf("c", "d"), names(h2.all(other)))
    }

    @Test
    fun oneRequestAtATimeAStaleAnswerIsDroppedAndAFailureWaitsForATap() {
        val h = OlderHeld()
        val n = node(10, "d")
        h.sync(n)
        assertNull(h.ask(n, false), "not at the oldest end")
        val ask = h.ask(n, true)!!
        assertEquals(5L, ask.before)
        assertNull(h.ask(n, true), "one is out")
        assertFalse(h.replied(OlderAsk(4, 99), page("x")), "not the one asked")
        assertTrue(h.replied(ask, Result.failure(IllegalStateException("no"))))
        assertEquals(OlderRow.Failed, h.row(n))
        assertNull(h.ask(n, true), "a failure waits")
        h.retry()
        assertEquals(5L, h.ask(n, true)?.before)
    }

    @Test
    fun theCursorGoesBackAndTheStartOfTheFileEndsTheAsking() {
        val h = OlderHeld()
        val n = node(10, "d")
        h.sync(n)
        val ask = h.ask(n, true)!!
        // an answer that points forward is taken as the end: asking again would not stop
        h.replied(ask, page("c", older = 9))
        assertNull(h.ask(n, true))
        assertEquals(OlderRow.Start, h.row(n))
        assertNull(h.row(n.copy(older = null)), "no transcript, no row")
    }
}
