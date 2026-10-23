package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.ViewKind
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-740 to REQ-742: the diff and the viewer are tabs after the panes, and a view the phone asked for is selected when it shows. */
class ViewTabsTest {
    private val dir = File(System.getProperty("contract.dir"), "fixtures/daemon")
    private fun node(name: String) = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, name).readText()) as DaemonMsg.Snapshot).node
    private val session = node("snapshot-session.json") as Node.Session
    private val withViews = node("snapshot-workspace-views.json") as Node.Workspace
    private val without = withViews.copy(views = emptyList())
    private var shown: Node.Workspace = withViews
    private val source = object : NodeSource {
        override val epoch = session.epoch
        override fun session() = session
        override fun node(id: String): Node? = if (id == shown.id) shown else null
    }
    private val nav = Navigator(source)

    private val diff get() = withViews.views.first { it.kind == ViewKind.Diff }
    private val viewer get() = withViews.views.first { it.kind == ViewKind.Viewer }

    @Test
    fun aViewTabIsKeptWhileTheWorkspaceListsItAndFallsBackToTheFirstPaneWhenItGoes() {
        val at = ClientState(screen = Screen.Workspace(withViews.id, diff.id))
        assertEquals(at, nav.settle(at), "the diff's tab stays")
        shown = without
        val settled = nav.settle(at)
        assertEquals(without.panes.first().id, (settled.screen as Screen.Workspace).pane, "the view closed: the first pane")
    }

    @Test
    fun aViewTheWorkspaceListsIsSelectedAndOneItDoesNotIsWaitedFor() {
        shown = withViews
        val at = ClientState(screen = Screen.Workspace(withViews.id, null))
        assertEquals(Screen.Workspace(withViews.id, viewer.id), nav.selectWhenShown(at, withViews.id, viewer.id)?.screen)
        shown = without
        assertNull(nav.selectWhenShown(at, withViews.id, viewer.id), "not listed yet")
    }

    @Test
    fun aSourceWithNoPcAnswersAViewWithItsMessage() {
        var opened = false
        var closed = false
        source.openView(withViews.id, ViewKind.Diff, null) { opened = it.isFailure }
        source.closeView(diff.id, null) { closed = it.isFailure }
        assertTrue(opened && closed)
    }

    @Test
    fun theTabsAreThePanesAndThenTheViewsInTheOrderTheWorkspaceListsThem() {
        val order = withViews.panes.map { it.id } + withViews.views.map { it.id }
        assertEquals(listOf(diff.id, viewer.id), order.takeLast(2))
        assertEquals(setOf("diff", "viewer"), withViews.views.map { if (it.kind == ViewKind.Diff) "diff" else "viewer" }.toSet())
    }
}
