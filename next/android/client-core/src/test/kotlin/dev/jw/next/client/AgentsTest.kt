package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.Task
import dev.jw.next.contract.TaskKind
import dev.jw.next.contract.TaskState
import dev.jw.next.contract.ToolState
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
        val row = agentRows(conv, now = 1_792_110_002_000 + 90_000).first { it.section == AgentSection.Agents }
        assertEquals("Explore (haiku-5.5)", row.name, "the kind of subagent and the model it runs with")
        assertEquals("1:30 · 38.4k", row.stats, "the clock of a task that runs is the phone's")
        assertEquals("Buscando el puerto del puente", row.doing)
        assertEquals("Busca cómo el daemon habla con el teléfono. No edites nada.", row.prompt)
        assertEquals(listOf("Glob crates/**/bridge*.rs", "Grep fn pair"), row.steps)
        assertNull(row.said, "it has not said anything yet")
        assertTrue(row.running)
    }

    @Test
    fun theTasksAreInThreeSectionsTheNewestFirstAndAShellIsItsCommand() {
        val base = withAgent()
        val tool = ConvItem.Tool(id = "toolu_b", name = "Bash", arg = "sleep 25; echo listo", state = ToolState.Run)
        val conv = base.copy(
            items = base.items + tool,
            tasks = listOf(
                base.tasks[0].copy(state = TaskState.Done, info = "El puente está en crates/web."),
                Task("m1", TaskKind.Monitor, "tail -f app.log | grep ERROR", TaskState.Run, 1_792_110_050_000, info = "último aviso hace 2 min"),
                Task("b1", TaskKind.Bash, "Sleep 25 seconds then echo listo", TaskState.Run, 1_792_110_100_000, info = "compilando", item = "toolu_b"),
                Task("b0", TaskKind.Bash, "npm run build", TaskState.Failed, 1_792_110_090_000),
                Task("l1", TaskKind.Loop, "revisar el CI cada 5 min", TaskState.Run, 1_792_110_060_000),
            ),
        )
        val rows = agentRows(conv, now = 1_792_110_100_000 + 5_000)
        assertEquals(listOf("aa8348e2", "b1", "b0", "l1", "m1"), rows.map { it.id }, "subagents, shells, then monitors and loops, the newest first in each")
        assertEquals(listOf(AgentSection.Agents, AgentSection.Shells, AgentSection.Shells, AgentSection.Monitors, AgentSection.Monitors), rows.map { it.section })
        val shell = rows[1]
        assertEquals("sleep 25; echo listo", shell.name, "the command of its tool call, not the engine's description")
        assertEquals("0:05", shell.stats)
        assertEquals("", shell.doing, "a shell has no arrow: it is its command")
        assertEquals("compilando", shell.said)
        assertEquals("falló", rows[2].said, "one that ended says how, and its description is not news")
        assertEquals("npm run build", rows[2].name, "no tool call in the items: the description is all there is")
        // a subagent that ended says how and what it said
        assertEquals("listo", rows[0].doing)
        assertEquals("El puente está en crates/web.", rows[0].said)
        // a subagent with no card has only its time
        val bare = conv.copy(tasks = listOf(Task("a2", TaskKind.Agent, "x", TaskState.Run, 1_792_110_100_000)))
        val r = agentRows(bare, now = 1_792_110_100_000 + 5_000).single()
        assertEquals(listOf("agente", "0:05", "empezando"), listOf(r.name, r.stats, r.doing))
        assertNull(r.prompt)
    }

    @Test
    fun aSectionKeepsItsNewestFewAndWhatTheEngineDoesNotNameIsAMonitor() {
        val many = withAgent().copy(tasks = (0L..8L).map { Task("s$it", TaskKind.Bash, "c$it", TaskState.Done, it) } + Task("o", TaskKind.Other, "x", TaskState.Run, 1))
        val rows = agentRows(many, now = 100)
        assertEquals(AGENT_ROWS, rows.count { it.section == AgentSection.Shells })
        assertEquals("s8", rows.first().id)
        assertEquals(AgentSection.Monitors, rows.last().section)
    }

    /** REQ-607: the last lines of a shell or a monitor travel in its row; a subagent and a loop show none. */
    @Test
    fun aShellAndAMonitorCarryTheLastLinesTheyPrintedAndASubagentAndALoopCarryNone() {
        val conv = withAgent()
        val shell = agentRows(conv, now = 1_792_110_100_000).single { it.section == AgentSection.Shells }
        assertEquals(listOf("   Compiling jwn v0.1.0", "test tui::agents::a_shell_shows_its_tail ... ok"), shell.tail)
        val lines = listOf("a", "b", "c")
        val more = conv.copy(
            tasks = listOf(
                Task("m1", TaskKind.Monitor, "tail", TaskState.Run, 2, tail = lines),
                Task("l1", TaskKind.Loop, "ci", TaskState.Run, 3, tail = lines),
                Task("g1", TaskKind.Agent, "x", TaskState.Run, 4, tail = lines),
            ),
        )
        val rows = agentRows(more, now = 100).associateBy { it.id }
        assertEquals(lines, rows.getValue("m1").tail)
        assertTrue(rows.getValue("l1").tail.isEmpty() && rows.getValue("g1").tail.isEmpty())
        assertEquals(emptyList(), agentRows(conv.copy(tasks = listOf(Task("b", TaskKind.Bash, "x", TaskState.Run, 1))), now = 100).single().tail, "nothing printed yet")
    }

    @Test
    fun theChipCountsEverythingThatRunsAndShowsWhileThePanelIsOpenSoThatItCanBeClosed() {
        val conv = withAgent()
        assertEquals(2, agentsChip(conv, open = false), "the subagent and the shell of the fixture")
        val shell = conv.copy(tasks = listOf(Task("b1", TaskKind.Bash, "cargo", TaskState.Run, 1), Task("m1", TaskKind.Monitor, "tail", TaskState.Run, 2), Task("b2", TaskKind.Bash, "ls", TaskState.Done, 3)))
        assertEquals(2, agentsChip(shell, open = false), "a shell and a monitor count; what ended does not")
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
