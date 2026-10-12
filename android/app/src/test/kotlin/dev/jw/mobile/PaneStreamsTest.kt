package dev.jw.mobile

import dev.jw.proto.Frame
import dev.jw.proto.FrameCodec
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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.TimeUnit

/** The panes of a watched workspace as terminals (REQ-21), and the sizes the app asks for (REQ-24). */
class PaneStreamsTest {
    private val fake = FakeBridge()
    private val store = FakeStore(StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    private fun session() = Session(scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy() }, pollMs = 100_000, tickMs = 50)

    private fun json(b: ByteArray) = (FrameCodec.decode(b) as? Frame.Json)?.text

    /** The next frame the phone sent that is not the `List` of its first connection. */
    private fun next(): String? {
        while (true) {
            val f = fake.frames.poll(5, TimeUnit.SECONDS) ?: return null
            val t = json(f)
            if (t != """{"t":"List"}""") return t ?: (FrameCodec.decode(f) as Frame.Raw).head
        }
    }

    /** Connected and past what the session sends on `ready`, so a test's own `watch` is the only one. */
    private suspend fun Session.connected() {
        withTimeout(20_000) { state.first { it is UiState.Connected } }
        kotlinx.coroutines.delay(300)
    }
    private suspend fun Session.screen(pane: Long) = withTimeout(10_000) {
        while (terminal(pane) == null) kotlinx.coroutines.delay(10)
        terminal(pane)!!
    }

    private fun snapshot(pane: Long, cols: Int, rows: Int, text: String) =
        FakeBridge.raw("""{"t":"Snapshot","pane":$pane,"role":"shell","cols":$cols,"rows":$rows,"bytes":[]}""", text.toByteArray())

    @Test fun watchAttachesAndTheSnapshotAndOutputFillTheTerminal() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws ->
            if (json(f)?.contains("Attach") == true) {
                ws.send(snapshot(7, 40, 5, "hello\r\nworld"))
                ws.send(FakeBridge.raw("""{"t":"Output","pane":7,"bytes":[]}""", "!\r\n\u001b[31mred".toByteArray()))
            }
        }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        assertEquals("""{"t":"Attach","stream":"dir:/x"}""", next())
        val t = s.screen(7)
        withTimeout(10_000) { while (t.read { it.line(2).text() } != "red") kotlinx.coroutines.delay(10) }
        assertEquals("hello", t.read { it.line(0).text() })
        assertEquals("world!", t.read { it.line(1).text() })
        assertEquals(40, t.cols)
    }

    @Test fun aSizeChangeAndAnExitReachTheTerminal() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws ->
            if (json(f)?.contains("Attach") == true) {
                ws.send(snapshot(7, 40, 5, "x"))
                ws.send(FakeBridge.frame("""{"t":"Resized","pane":7,"cols":20,"rows":8}"""))
                ws.send(FakeBridge.frame("""{"t":"Exited","pane":7,"status":3}"""))
            }
        }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        withTimeout(10_000) { while (s.exitStatus(7) == null) kotlinx.coroutines.delay(10) }
        assertEquals(3, s.exitStatus(7))
        assertEquals(20, s.terminal(7)!!.cols)
        assertEquals(8, s.terminal(7)!!.rows)
    }

    @Test fun leavingDetachesAndForgetsTheScreens() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws -> if (json(f)?.contains("Attach") == true) ws.send(snapshot(7, 40, 5, "x")) }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        next()
        s.screen(7)
        s.watch(null)
        assertEquals("""{"t":"Detach"}""", next())
        assertNull(s.terminal(7))
        s.watch("dir:/y")
        assertEquals("""{"t":"Attach","stream":"dir:/y"}""", next())
    }

    @Test fun fitAsksOnceAndRestoresTheSizeThePaneHad() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws -> if (json(f)?.contains("Attach") == true) ws.send(snapshot(7, 100, 30, "x")) }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        next()
        s.screen(7)
        s.fit(7, 50, 20)
        assertEquals("""{"t":"Resize","pane":7,"cols":50,"rows":20}""", next())
        s.fit(7, 48, 18)
        assertEquals("""{"t":"Resize","pane":7,"cols":48,"rows":18}""", next())
        s.restoreSizes()
        assertEquals("""{"t":"Resize","pane":7,"cols":100,"rows":30}""", next())
        s.restoreSizes()
        assertNull("nothing left to restore", fake.frames.poll(300, TimeUnit.MILLISECONDS))
    }

    @Test fun fitDoesNothingInReadOnlyOrWhenTheSizeIsTheSame() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready(ro = true)) }
        fake.onFrame = { f, ws -> if (json(f)?.contains("Attach") == true) ws.send(snapshot(7, 100, 30, "x")) }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        next()
        s.screen(7)
        s.fit(7, 50, 20)
        assertNull(fake.frames.poll(300, TimeUnit.MILLISECONDS))
    }

    @Test fun terminalBytesStayOutOfToString() {
        assertTrue(!dev.jw.proto.PaneEvent.Output(1, "secret".toByteArray()).toString().contains("secret"))
    }
}
