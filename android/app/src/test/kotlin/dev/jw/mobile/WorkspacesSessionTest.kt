package dev.jw.mobile

import dev.jw.proto.AgentState
import dev.jw.proto.Frame
import dev.jw.proto.FrameCodec
import dev.jw.proto.ReconnectPolicy
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.TimeUnit

class WorkspacesSessionTest {
    private val fake = FakeBridge()
    private val store = FakeStore(StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    /** Real time plus a jump the tests can add, to age a snapshot without waiting. */
    @Volatile private var jump = 0L
    private fun now() = System.currentTimeMillis() + jump

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    private fun session() = Session(
        scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy() },
        clock = ::now, pollMs = 100, tickMs = 20,
    )

    private fun frameJson(b: ByteArray) = (FrameCodec.decode(b) as? Frame.Json)?.text

    private fun serve(agent: String = "waiting", readOnly: Boolean = false) {
        fake.enqueue { _, ws ->
            ws.send(FakeBridge.ready(ro = readOnly))
        }
        fake.onLater = { text, ws ->
            when {
                text.contains("workspaces") -> ws.send(FakeBridge.WORKSPACES)
                text.contains("sessions") -> ws.send(FakeBridge.SESSIONS)
            }
        }
    }

    private suspend fun Session.connected() = withTimeout(20_000) { state.first { it is UiState.Connected } }
    private suspend fun Session.until(p: (Model) -> Boolean) = withTimeout(20_000) { model.first(p) }

    @Test fun askesForSessionsWorkspacesAndPanesOnReady() = runBlocking<Unit> {
        serve()
        val s = session()
        s.start()
        s.connected()
        val m = s.until { it.items.isNotEmpty() && it.sessions.isNotEmpty() }
        assertEquals("jw", m.items.single().name)
        assertEquals("main", m.session)
        assertEquals("""{"t":"List"}""", frameJson(fake.frames.poll(5, TimeUnit.SECONDS)!!))
    }

    @Test fun panesFromListFillTheModel() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.send(FakeBridge.frame(FakeBridge.panes("working"))) }
        val s = session()
        s.start()
        val m = s.until { it.panes.isNotEmpty() }
        assertEquals(AgentState.Working, m.panes.single().agent)
        assertTrue(m.panesAt != null)
    }

    @Test fun listIsPolledOnlyWhileVisibleAndInForeground() = runBlocking<Unit> {
        serve()
        val s = session()
        s.start()
        s.connected()
        fake.frames.poll(5, TimeUnit.SECONDS) // the one sent at ready
        delay(300)
        fake.frames.clear()
        delay(500)
        assertNull("hidden: no List", fake.frames.poll())
        s.setVisible(true)
        assertTrue(fake.frames.poll(3, TimeUnit.SECONDS) != null)
        assertTrue("and again", fake.frames.poll(3, TimeUnit.SECONDS) != null)
        s.setForeground(false)
        delay(300)
        fake.frames.clear()
        delay(500)
        assertNull("background: no List", fake.frames.poll())
    }

    @Test fun approveSendsEnterOnceAndRejectEsc() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.send(FakeBridge.frame(FakeBridge.panes("waiting"))) }
        val s = session()
        s.start()
        s.until { it.panes.isNotEmpty() }
        fake.frames.poll(5, TimeUnit.SECONDS) // List
        s.answer(7, true)
        val f = FrameCodec.decode(fake.frames.poll(5, TimeUnit.SECONDS)!!) as Frame.Raw
        assertEquals("""{"t":"Input","pane":7,"bytes":[]}""", f.head)
        assertArrayEquals(byteArrayOf(13), f.bytes)
        s.answer(7, false)
        assertNull("sent once per snapshot", fake.frames.poll(500, TimeUnit.MILLISECONDS))
    }

    @Test fun staleSnapshotDoesNotSend() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.send(FakeBridge.frame(FakeBridge.panes("waiting"))) }
        val s = session()
        s.start()
        s.until { it.panes.isNotEmpty() }
        fake.frames.poll(5, TimeUnit.SECONDS)
        jump = 2_500
        s.answer(7, true)
        assertNull(fake.frames.poll(500, TimeUnit.MILLISECONDS))
    }

    @Test fun readOnlyPhoneNeverSendsInput() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready(ro = true)); ws.send(FakeBridge.frame(FakeBridge.panes("waiting"))) }
        val s = session()
        s.start()
        s.until { it.panes.isNotEmpty() }
        fake.frames.poll(5, TimeUnit.SECONDS)
        s.answer(7, true)
        assertNull(fake.frames.poll(500, TimeUnit.MILLISECONDS))
    }

    @Test fun reconnectAsksAgain() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.close(1001, "bye") }
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        val s = Session(scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy(baseMs = 20, maxMs = 20) }, clock = ::now)
        s.start()
        withTimeout(20_000) { while (fake.server.requestCount < 2) delay(20) }
        s.connected()
        assertTrue(fake.received.size >= 3)
    }

    @Test fun aWorkspaceAddedOnThePcShowsUpWhileTheListIsOnScreen() = runBlocking<Unit> {
        // The PC's answer changes between two asks: a second workspace appears.
        var asks = 0
        val extra = """{"id":"w9","kind":"worktree","name":"new","path":"/p2","project":"jw","branch":"feat/new","pr":null,"session":"main","parent":"r1"}"""
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { text, ws ->
            when {
                text.contains("\"workspaces\"") -> {
                    asks++
                    ws.send(
                        if (asks == 1) FakeBridge.WORKSPACES
                        else FakeBridge.WORKSPACES.replace("}]}", "},$extra]}"),
                    )
                }
                text.contains("sessions") -> ws.send(FakeBridge.SESSIONS)
            }
        }
        val s = session()
        s.setVisible(true)
        s.start()
        s.connected()
        val m = s.until { it.items.any { w -> w.id == "w9" } }
        assertEquals(2, m.items.size)
    }

    @Test fun nothingIsAskedWhileTheListIsNotOnScreen() = runBlocking<Unit> {
        serve()
        val s = session()
        s.setVisible(false)
        s.start()
        s.connected()
        s.until { it.items.isNotEmpty() }
        val before = fake.received.size
        delay(600)
        assertEquals("no refresh while hidden", before, fake.received.size)
    }
}
