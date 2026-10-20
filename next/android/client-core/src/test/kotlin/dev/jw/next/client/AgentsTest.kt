package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.Task
import dev.jw.next.contract.TaskKind
import dev.jw.next.contract.TaskState
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-600, REQ-602: the agents of a conversation as the phone's panel lists them. */
class AgentsTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun withAgent(): Node.Conversation =
        (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, "snapshot-conversation-agents.json").readText()) as DaemonMsg.Snapshot).node as Node.Conversation

    @Test
    fun aSubagentTaskTakesItsNumbersItsPromptAndItsStepsFromItsCardAndTheTimeRuns() {
        val conv = withAgent()
        val row = agentRows(conv, now = 1_792_110_002_000 + 90_000).single()
        assertEquals("Explore", row.name)
        assertEquals("2 tools · 38.4k · 1:30", row.stats, "the clock of a task that runs is the phone's")
        assertEquals("Buscando el puerto del puente", row.now)
        assertEquals("Busca cómo el daemon habla con el teléfono. No edites nada.", row.prompt)
        assertEquals(listOf("Glob crates/**/bridge*.rs", "Grep fn pair"), row.steps)
        assertNull(row.said, "it has not said anything yet")
        assertTrue(row.running)
    }

    @Test
    fun aTaskThatEndedSaysWhatItSaidAndOneWithNoCardHasOnlyItsTime() {
        val conv = withAgent().let { c ->
            c.copy(
                tasks = listOf(
                    c.tasks[0].copy(state = TaskState.Done, info = "El puente está en crates/web."),
                    Task("b1", TaskKind.Bash, "cargo test", TaskState.Run, 1_792_110_100_000, info = "compilando"),
                ),
            )
        }
        val rows = agentRows(conv, now = 1_792_110_100_000 + 5_000)
        assertEquals(listOf("b1", "aa8348e2"), rows.map { it.id }, "newest first")
        assertEquals("shell", rows[0].name)
        assertEquals("0:05", rows[0].stats)
        assertNull(rows[0].prompt)
        assertEquals(emptyList(), rows[0].steps)
        assertEquals("El puente está en crates/web.", rows[1].said)
        assertNull(rows[1].now, "what ended does nothing now")
    }

    @Test
    fun theChipShowsWhileSomeRunAndWhileThePanelIsOpenSoThatItCanBeClosed() {
        val conv = withAgent()
        assertEquals(1, agentsChip(conv, open = false))
        val idle = conv.copy(tasks = emptyList())
        assertNull(agentsChip(idle, open = false))
        assertEquals(0, agentsChip(idle, open = true))
    }

    @Test
    fun theNumbersAreWrittenShort() {
        assertEquals("999", agentTokens(999))
        assertEquals("38.4k", agentTokens(38_400))
        assertEquals("22k", agentTokens(22_000))
        assertEquals("1:12", agentClock(72_000))
        assertEquals("0:00", agentClock(-5))
    }
}
