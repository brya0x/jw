package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-856: the chip of an engine that runs another claude than the installed one. */
class EngineTest {
    private val conv = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon/snapshot-conversation-idle.json").readText()) as DaemonMsg.Snapshot).node as Node.Conversation

    @Test
    fun theChipSaysWhichClaudeTheEngineRunsAndWhichIsInstalledOnlyWhenTheNodeSaysBoth() {
        assertEquals("motor 2.1.309 ↑ 2.1.312", engineChip(conv))
        assertNull(engineChip(conv.copy(engineNewer = null)), "an engine that runs the installed one has no chip")
        assertNull(engineChip(conv.copy(engineVersion = null)), "nor one that has not said its version")
    }

    @Test
    fun theSheetSaysWhatToDo() {
        val said = engineSaid(conv)
        assertTrue("2.1.309" in said && "2.1.312" in said && "Cierra la conversación y ábrela" in said, said)
        assertEquals("", engineSaid(conv.copy(engineNewer = null)))
    }
}
