package dev.jw.mobile

import dev.jw.proto.ReconnectPolicy
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-13: the changes of a workspace asked for, answered, refused or lost with the connection. */
class DiffSessionTest {
    private val fake = FakeBridge()
    private val store = FakeStore(StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    private fun session() = Session(scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy() }, pollMs = 100_000, tickMs = 50)

    private fun answer(ws: String) =
        """{"t":"diff","ws":"$ws","title":"demo vs main","truncated":false,"files":[{"path":"a.rs","status":"modified","added":1,"deleted":0,"hunks":[{"context":"","old_start":1,"new_start":1,"lines":[{"k":"add","text":"x","old":null,"new":1}]}]}]}"""

    private suspend fun Session.connected() { withTimeout(20_000) { state.first { it is UiState.Connected } } }
    private suspend fun Session.settled() = withTimeout(10_000) { diff.first { it !is DiffState.Loading } }

    @Test fun theAnswerForTheAskedWorkspaceLands() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws -> if (t.contains(""""t":"diff"""")) ws.send(answer("w1")) }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1")
        val d = (s.settled() as DiffState.Loaded).diff
        assertEquals("demo vs main", d.title)
        assertEquals(1, d.files.size)
        assertTrue(fake.received.any { it == """{"t":"diff","ws":"w1"}""" })
    }

    @Test fun anAnswerForAnotherWorkspaceIsDropped() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws -> if (t.contains(""""t":"diff"""")) { ws.send(answer("old")); ws.send(answer("w2")) } }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w2")
        assertEquals("w2", (s.settled() as DiffState.Loaded).diff.ws)
    }

    @Test fun anErrorWhileAskedIsTheAnswerButBusyIsNot() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"diff"""")) {
                ws.send("""{"t":"error","code":"unsupported","msg":"busy: wait for the answer before asking again"}""")
                ws.send("""{"t":"error","code":"unsupported","msg":"demo is not a git repository"}""")
            }
        }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1")
        val f = s.settled() as DiffState.Failed
        assertEquals("demo is not a git repository", f.why)
        assertFalse(f.toString().contains("git"))
    }

    @Test fun aRequestLostWithTheConnectionIsAskedAgain() = runBlocking<Unit> {
        // The first ask meets a connection that drops; the same ask, again on the next one, is answered.
        val asks = java.util.concurrent.atomic.AtomicInteger()
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"diff"""")) { if (asks.getAndIncrement() == 0) ws.close(1001, null) else ws.send(answer("w1")) }
        }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1")
        assertEquals("w1", (s.settled() as DiffState.Loaded).diff.ws)
        assertEquals(2, fake.received.count { it == """{"t":"diff","ws":"w1"}""" })
    }

    @Test fun leavingTheScreenLetsGoOfTheAnswer() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws -> if (t.contains(""""t":"diff"""")) ws.send(answer("w1")) }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1"); s.settled()
        s.clearDiff()
        assertEquals(DiffState.Idle, s.diff.value)
    }
}
