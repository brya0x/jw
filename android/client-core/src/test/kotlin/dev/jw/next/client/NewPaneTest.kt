package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-234 and REQ-235: a shell more from the phone. */
class NewPaneTest {
    private val dir = File(System.getProperty("contract.dir"), "fixtures/daemon")
    private fun node(name: String) = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, name).readText()) as DaemonMsg.Snapshot).node
    private val session = node("snapshot-session.json") as Node.Session
    private val workspace = node("snapshot-workspace.json") as Node.Workspace
    private val source = object : NodeSource {
        override val epoch = session.epoch
        override fun session() = session
        override fun node(id: String): Node? = if (id == workspace.id) workspace else null
    }
    private val nav = Navigator(source)
    private val at = ClientState(screen = Screen.Workspace(workspace.id, null))

    @Test
    fun aPaneTheWorkspaceListsIsSelectedAndOneItDoesNotIsWaitedFor() {
        val pane = workspace.panes.last().id
        assertEquals(Screen.Workspace(workspace.id, pane), nav.selectWhenShown(at, workspace.id, pane)?.screen)
        assertNull(nav.selectWhenShown(at, workspace.id, "not-yet"))
        assertNull(nav.selectWhenShown(at, "no-workspace", pane))
    }

    @Test
    fun aSourceWithNoPcAnswersWithItsMessage() {
        var failed = false
        source.newPane(workspace.id) { failed = it.isFailure }
        assertTrue(failed)
    }
}
