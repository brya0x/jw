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
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-85 to 88: what the person asks the PC to make: done, refused, or lost with the connection. */
class MakeSessionTest {
    private val fake = FakeBridge()
    private val store = FakeStore(StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    private fun session() = Session(scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy() }, pollMs = 100_000, tickMs = 50)
    private suspend fun Session.connected() { withTimeout(20_000) { state.first { it is UiState.Connected } } }
    private suspend inline fun <reified T : MakeState> Session.until() = withTimeout(10_000) { make.first { it is T } } as T

    @Test fun aWorktreeIsMadeAndTheListIsAskedAgain() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"worktree"""")) ws.send("""{"t":"worktree","project":"jw","name":"feat-x","ws":"id1","branch":"feat/x"}""")
        }
        val s = session(); s.start(); s.connected()
        s.requestWorktree("jw", "feat-x", "feat/x", "haz esto", "main")
        val d = s.until<MakeState.Done>()
        assertEquals(Make.Worktree to "id1", d.what to d.ws)
        // What was asked, with the session and the task.
        assertTrue(fake.received.any { it == """{"t":"worktree","project":"jw","name":"feat-x","branch":"feat/x","task":"haz esto","session":"main"}""" })
        // The list is asked for in the session (REQ-88: the new workspace shows).
        withTimeout(10_000) { while (fake.received.count { it.startsWith("""{"t":"workspaces"""") } < 2) kotlinx.coroutines.delay(20) }
        s.clearMake()
        assertEquals(MakeState.Idle, s.make.value)
    }

    @Test fun aNewSessionSwitchesTheListToIt() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"session_new"""")) ws.send("""{"t":"session_created","name":"trabajo","ws":"dir:/a#trabajo"}""")
        }
        val s = session(); s.start(); s.connected()
        s.requestNewSession("trabajo", "/a")
        val d = s.until<MakeState.Done>()
        assertEquals("trabajo", d.session)
        withTimeout(10_000) { while (fake.received.none { it == """{"t":"workspaces","session":"trabajo"}""" }) kotlinx.coroutines.delay(20) }
    }

    @Test fun aRefusalIsSaidAndOnlyOneMakeRunsAtATime() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"open"""")) ws.send("""{"t":"error","code":"unsupported","msg":"not a folder","ask":"opened","req":"/f"}""")
        }
        val s = session(); s.start(); s.connected()
        s.requestOpen("/f", "main")
        s.requestOpen("/other", "main")   // ignored: one is out
        assertEquals("not a folder", s.until<MakeState.Failed>().why)
        assertEquals(1, fake.received.count { it.contains(""""t":"open"""") })
    }

    @Test fun readOnlyIsSaidInTheSheet() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"worktree"""")) ws.send("""{"t":"error","code":"read_only","msg":"this bridge is read-only","ask":"worktree","project":"jw","name":"feat-x"}""")
        }
        val s = session(); s.start(); s.connected()
        s.requestWorktree("jw", "feat-x", null, null, "main")
        val f = s.until<MakeState.Failed>()
        assertTrue(f.why, f.why.startsWith("solo lectura"))
    }

    @Test fun aMakeLostWithTheConnectionIsNotAskedAgain() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        val asks = java.util.concurrent.atomic.AtomicInteger()
        fake.onLater = { t, ws -> if (t.contains(""""t":"worktree"""") && asks.getAndIncrement() == 0) ws.close(1001, null) }
        val s = session(); s.start(); s.connected()
        s.requestWorktree("jw", "feat-x", null, null, "main")
        val f = s.until<MakeState.Failed>()
        assertTrue(f.why.contains("se perdió la conexión"))
        // Asked once: a second ask would make the worktree twice.
        kotlinx.coroutines.delay(300)
        assertEquals(1, fake.received.count { it.contains(""""t":"worktree"""") })
    }
}
