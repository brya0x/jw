package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-148 to REQ-150 against the shared fixtures. */
class ClosePaneTest {
    private class Pc : NodeSource {
        private val dir = File(System.getProperty("contract.dir"), "fixtures/daemon")
        val nodes: MutableMap<String, Node> = dir.listFiles { f -> f.name.startsWith("snapshot-") && "blocks-full" !in f.name }.orEmpty()
            .map { (ContractJson.decodeFromString(DaemonMsg.serializer(), it.readText()) as DaemonMsg.Snapshot).node }
            .associateBy { it.id }.toMutableMap()
        private val sessionNode = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, "snapshot-session-attention.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
        override val epoch = sessionNode.epoch
        override fun session() = sessionNode
        override fun node(id: String) = if (id == sessionNode.id) sessionNode else nodes[id]
    }

    private val pc = Pc()
    private val workspace = pc.session().workspaces[0].id
    private val panes get() = (pc.node(workspace) as Node.Workspace).panes

    @Test
    fun theSheetNamesWhatRunsAndSaysWhatStaysAndWhatThePcDoes() {
        // The first pane runs nvim; closing it stops that.
        val sheet = closeSheet(pc, panes[0].id)!!
        assertEquals("cerrar el panel «zsh»", sheet.heading)
        assertEquals("nvim", sheet.running)
        assertTrue(sheet.body.startsWith("nvim sigue corriendo en este panel. Cerrarlo lo detiene."), sheet.body)
        assertTrue("El workspace se queda, aunque sea sin paneles." in sheet.body)
        assertTrue("Cerrar un workspace o quitar un worktree se hace en el PC." in sheet.body)
        // The second is a shell with a command still going.
        assertEquals("cargo", closeSheet(pc, panes[1].id)!!.running)
    }

    @Test
    fun aShellAtItsPromptStillAsksAndSaysItsShellEnds() {
        val id = panes[0].id
        pc.nodes[id] = (pc.node(id) as Node.Pane).copy(running = null)
        val sheet = closeSheet(pc, id)!!
        assertNull(sheet.running)
        assertTrue(sheet.body.startsWith("Cerrarlo termina su shell."), sheet.body)
    }

    @Test
    fun noSheetUntilThePaneDocumentArrives() {
        assertNull(closeSheet(pc, "not-a-pane"))
    }

    @Test
    fun aSourceWithNoPcAnswersWithItsMessage() {
        var got: Result<Unit>? = null
        pc.closePane(panes[0].id) { got = it }
        assertEquals("sin conexión a un PC", got!!.exceptionOrNull()!!.message)
    }

    @Test
    fun theSelectedPaneThatWentIsReplacedByTheFirstOrByNone() {
        val nav = Navigator(pc)
        val s0 = nav.openWorkspace(ClientState(), workspace, panes[1].id)
        assertEquals(panes[1].id, (s0.screen as Screen.Workspace).pane)
        // The PC closed the second pane: the first takes its place.
        pc.nodes[workspace] = (pc.node(workspace) as Node.Workspace).let { it.copy(panes = it.panes.filter { p -> p.id != panes[1].id }) }
        val s1 = nav.settle(s0)
        assertEquals(panes.first().id, assertIs<Screen.Workspace>(s1.screen).pane)
        // And none when it was the last.
        pc.nodes[workspace] = (pc.node(workspace) as Node.Workspace).copy(panes = emptyList())
        assertNull(assertIs<Screen.Workspace>(nav.settle(s1).screen).pane)
    }

    @Test
    fun aPaneStillThereIsLeftAlone() {
        val nav = Navigator(pc)
        val s0 = nav.openWorkspace(ClientState(), workspace, panes[1].id)
        assertEquals(s0, nav.settle(s0))
    }
}
