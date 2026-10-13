package dev.jw.next.client

import dev.jw.next.contract.AttentionKind
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-33 to REQ-35 against the shared fixtures. */
class ClientCoreTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun msg(name: String): DaemonMsg =
        ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, name).readText())

    private fun session(name: String) = (msg(name) as DaemonMsg.Snapshot).node as Node.Session

    @Test
    fun aRowShowsTheMostUrgentUnseenMarkWithACount() {
        val rows = homeRows(session("snapshot-session-attention.json"), Seen())
        val first = rows[0]
        assertEquals("!", first.mark?.glyph, "permission beats everything else")
        assertEquals(7, first.unseen)
        assertEquals("pide permiso", first.line)
        assertEquals(AttentionKind.Permission, first.go?.kind)
        // The folder has only a failed command: a cross, not the green prompt.
        assertEquals("✗", rows[1].mark?.glyph)
        assertEquals("comando terminó · salió 2", rows[1].line)
    }

    @Test
    fun aRowWithNothingWaitingShowsNothing() {
        val rows = homeRows(session("snapshot-session.json"), Seen())
        assertTrue(rows.all { it.mark == null && it.line == null && it.go == null })
    }

    @Test
    fun openingTheNamedNodeMarksItSeenOnThisPhoneOnly() {
        val s = session("snapshot-session-attention.json")
        val permission = s.attention.first { it.kind == AttentionKind.Permission }
        val seen = Seen().mark(permission.node, permission.rev, s.epoch)
        val rows = homeRows(s, seen)
        // Everything raised on that pane at or below its revision is seen; the rest still waits.
        assertTrue(rows[0].unseen < 7)
        assertNotNull(rows[0].mark)
        assertEquals(avisos(s, seen).size, rows.sumOf { it.unseen })
    }

    @Test
    fun aSeenRecordFromAnotherEpochIsEmpty() {
        val seen = Seen().mark("n", 5, "epoch-a")
        assertEquals(5, seen.forEpoch("epoch-a").nodes["n"])
        assertTrue(seen.forEpoch("epoch-b").nodes.isEmpty())
    }

    @Test
    fun aSeenRecordSurvivesJsonAndADamagedOneIsEmpty() {
        val seen = Seen().mark("n", 5, "e")
        assertEquals(seen, Seen.fromJson(seen.toJson()))
        assertEquals(Seen(), Seen.fromJson("{not json"))
        assertEquals(Seen(), Seen.fromJson(null))
    }

    @Test
    fun tappingAnAttentionRoutesToItsPane() {
        val s = session("snapshot-session-attention.json")
        val a = s.workspaces[0].attention.first { it.kind == AttentionKind.Permission }
        assertEquals(Route(s.workspaces[0].id, a.node), routeOf(s, a))
        val merged = s.workspaces[0].attention.first { it.kind == AttentionKind.Merged }
        assertEquals(Route(s.workspaces[0].id, null), routeOf(s, merged), "a workspace-level attention opens the workspace")
    }

    @Test
    fun theCacheAppliesPatchesAndAsksToReopenOnAGap() {
        val cache = NodeCache()
        val hello = msg("hello.json") as DaemonMsg.Hello
        cache.apply(hello)
        val snap = msg("snapshot-pane.json") as DaemonMsg.Snapshot
        cache.apply(snap)
        val id = snap.node.id
        assertEquals(9, cache.rev(id))

        val patch = msg("patch-pane-screen.json") as DaemonMsg.Patch
        assertEquals(CacheResult.Applied, cache.apply(patch))
        val pane = assertIs<Node.Pane>(cache.node(id))
        assertEquals(10, pane.rev)
        assertEquals("$ ls", pane.screen.lines[2])

        // The same patch again does not fit (the cache is at 10, the patch starts at 9).
        assertEquals(CacheResult.Reopen(id), cache.apply(patch))
        assertNull(cache.node(id))
    }

    @Test
    fun aNewEpochEmptiesTheCache() {
        val cache = NodeCache()
        cache.apply(msg("hello.json"))
        val snap = msg("snapshot-pane.json") as DaemonMsg.Snapshot
        cache.apply(snap)
        cache.setEpoch("another-epoch")
        assertNull(cache.node(snap.node.id))
    }

    @Test
    fun goneRemovesTheNode() {
        val cache = NodeCache()
        cache.apply(msg("hello.json"))
        val snap = msg("snapshot-pane.json") as DaemonMsg.Snapshot
        cache.apply(snap)
        cache.apply(DaemonMsg.Gone(snap.node.id))
        assertNull(cache.node(snap.node.id))
    }
}

/** A source that reads the daemon fixtures, for the navigator tests. */
private class FixtureSource : NodeSource {
    private val dir = File(System.getProperty("contract.dir"), "fixtures/daemon")
    private val nodes: Map<String, Node> = dir.listFiles { f -> f.name.startsWith("snapshot-") }.orEmpty()
        .mapNotNull { (ContractJson.decodeFromString(DaemonMsg.serializer(), it.readText()) as DaemonMsg.Snapshot).node }
        .associateBy { it.id }
    private val sessionNode =
        (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, "snapshot-session-attention.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    override val epoch = sessionNode.epoch
    override fun session() = sessionNode
    override fun node(id: String) = if (id == sessionNode.id) sessionNode else nodes[id]
}

class NavigatorTest {
    private val source = FixtureSource()
    private val nav = Navigator(source)

    @Test
    fun goOpensThePaneAnAttentionNamesAndClearsItOnThisPhone() {
        val s0 = ClientState()
        val rows0 = homeRows(source.session(), s0.seen)
        val go = rows0[0].go!!
        val s1 = nav.go(s0, go)
        val at = assertIs<Screen.Workspace>(s1.screen)
        assertEquals(go.node, at.pane, "the pane, not only the workspace")
        assertTrue(avisos(source.session(), s1.seen).none { it.attention == go })
        assertTrue(avisos(source.session(), s1.seen).size < avisos(source.session(), s0.seen).size)
    }

    @Test
    fun openingAWorkspaceSelectsItsFirstPaneAndSwitchingPanesMarksSeen() {
        val ws = source.session().workspaces[0]
        val s1 = nav.openWorkspace(ClientState(), ws.id)
        val at = assertIs<Screen.Workspace>(s1.screen)
        val panes = (source.node(ws.id) as Node.Workspace).panes
        assertEquals(panes[0].id, at.pane)
        val s2 = nav.selectPane(s1, panes[1].id)
        assertEquals(panes[1].id, (s2.screen as Screen.Workspace).pane)
        assertTrue(s2.seen.nodes.containsKey(panes[1].id))
    }

    @Test
    fun backAndTabsKeepTheSeenRecord() {
        val s1 = nav.go(ClientState(), homeRows(source.session(), Seen())[0].go!!)
        assertEquals(s1.seen, nav.back(s1).seen)
        assertEquals(Screen.Avisos, nav.show(s1, Screen.Avisos).screen)
    }
}

class SharedSeenTest {
    private val inner = FixtureSource()

    /** A source that remembers what the phone told the daemon. */
    private class Recording(private val base: NodeSource) : NodeSource by base {
        val told = mutableListOf<Pair<String, Long>>()
        override fun markSeen(id: String, rev: Long) {
            told += id to rev
        }
    }

    @Test
    fun goingToAnAttentionTellsTheDaemonSoTheOtherClientsClearIt() {
        val source = Recording(inner)
        val nav = Navigator(source)
        val go = homeRows(source.session(), Seen())[0].go!!
        nav.go(ClientState(), go)
        assertTrue(source.told.any { (id, rev) -> id == go.node && rev >= go.rev }, "told: ${source.told}")
    }
}

class SettleTest {
    /** A source whose workspace documents arrive only after the screen was opened. */
    private class Late(private val base: NodeSource) : NodeSource by base {
        var arrived = false
        override fun node(id: String) = base.node(id)?.takeIf { arrived || it !is Node.Workspace }
    }

    @Test
    fun aWorkspaceOpenedBeforeItsDocumentGetsItsFirstPaneWhenItArrives() {
        val source = Late(FixtureSource())
        val nav = Navigator(source)
        val ws = source.session().workspaces[0].id
        val opened = nav.openWorkspace(ClientState(), ws)
        assertNull((opened.screen as Screen.Workspace).pane, "nothing to select yet")
        assertEquals(opened, nav.settle(opened), "still nothing")
        source.arrived = true
        val settled = nav.settle(opened)
        val pane = (settled.screen as Screen.Workspace).pane
        assertNotNull(pane)
        assertTrue(settled.seen.nodes.containsKey(pane), "the pane shown is marked seen")
        assertEquals(settled, nav.settle(settled), "settling twice changes nothing")
    }
}
