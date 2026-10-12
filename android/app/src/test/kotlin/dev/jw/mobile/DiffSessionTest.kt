package dev.jw.mobile

import dev.jw.proto.DiffScope
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

    private fun answer(ws: String, scope: String = "general") =
        """{"t":"diff","ws":"$ws","scope":"$scope","title":"demo vs main","truncated":false,"files":[{"path":"a.rs","status":"modified","added":1,"deleted":0,"hunks":[{"context":"","old_start":1,"new_start":1,"lines":[{"k":"add","text":"x","old":null,"new":1}]}]}]}"""

    private fun commitsAnswer(ws: String) = """{"t":"commits","ws":"$ws","commits":[{"sha":"0123456789abcdef","short":"0123456","subject":"s","when":1}],"more":false,"no_base":false}"""

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
        assertTrue(fake.received.any { it == """{"t":"diff","ws":"w1","scope":"general"}""" })
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
                ws.send("""{"t":"error","code":"unsupported","msg":"busy: wait for the answer before asking again","ask":"diff","ws":"w1","scope":"general"}""")
                ws.send("""{"t":"error","code":"unsupported","msg":"demo is not a git repository","ask":"diff","ws":"w1","scope":"general"}""")
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
        assertEquals(2, fake.received.count { it == """{"t":"diff","ws":"w1","scope":"general"}""" })
    }

    @Test fun leavingTheScreenLetsGoOfTheAnswer() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws -> if (t.contains(""""t":"diff"""")) ws.send(answer("w1")) }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1"); s.settled()
        s.clearDiff()
        assertEquals(DiffState.Idle, s.diff.value)
    }

    @Test fun aViewIsAnswerForItsOwnScopeAndAnOldViewIsDropped() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"diff"""")) {
                // The general one is slow to come: the phone has moved to Uncommitted by then.
                ws.send(answer("w1", "general")); ws.send(answer("w1", "uncommitted"))
            }
        }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1", DiffScope.General)
        s.requestDiff("w1", DiffScope.Uncommitted)
        val d = (withTimeout(10_000) { s.diff.first { it is DiffState.Loaded } } as DiffState.Loaded).diff
        assertEquals(DiffScope.Uncommitted, d.scope)
    }

    @Test fun askingsAreSentOneAtATimeAndTheLatestOfEachKindWins() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        val gate = java.util.concurrent.atomic.AtomicReference<okhttp3.WebSocket?>()
        fake.onLater = { t, ws -> if (t.contains(""""t":"diff"""") || t.contains(""""t":"commits"""")) gate.set(ws) }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1", DiffScope.General)
        s.requestDiff("w1", DiffScope.Uncommitted)   // waits: the first is out
        s.requestDiff("w1", DiffScope.Commit("abc1234")) // replaces the one waiting
        s.requestCommits("w1")
        kotlinx.coroutines.delay(500)
        val asked = fake.received.filter { it.contains(""""t":"diff"""") || it.contains(""""t":"commits"""") }
        assertEquals(listOf("""{"t":"diff","ws":"w1","scope":"general"}"""), asked)
        // The first answers: the next goes, and it is the latest diff, not the one that was replaced.
        gate.get()!!.send(answer("w1", "general"))
        kotlinx.coroutines.delay(500)
        val next = fake.received.filter { it.contains(""""t":"diff"""") || it.contains(""""t":"commits"""") }.last()
        assertEquals("""{"t":"diff","ws":"w1","scope":"commit:abc1234"}""", next)
        gate.get()!!.send(answer("w1", "commit:abc1234"))
        kotlinx.coroutines.delay(500)
        assertEquals("""{"t":"commits","ws":"w1"}""", fake.received.filter { it.contains(""""t":"commits"""") }.single())
        gate.get()!!.send(commitsAnswer("w1"))
        withTimeout(10_000) { s.commits.first { it is CommitsState.Loaded } }
    }

    @Test fun aFailedCommitListIsItsOwnFailureNotTheDiffs() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"commits"""")) ws.send("""{"t":"error","code":"unsupported","msg":"git failed","ask":"commits","ws":"w1"}""")
            if (t.contains(""""t":"diff"""")) ws.send(answer("w1"))
        }
        val s = session(); s.start(); s.connected()
        s.requestDiff("w1"); s.requestCommits("w1")
        val c = withTimeout(10_000) { s.commits.first { it is CommitsState.Failed } } as CommitsState.Failed
        assertEquals("git failed", c.why)
        assertTrue(s.diff.value is DiffState.Loaded)
    }
}
