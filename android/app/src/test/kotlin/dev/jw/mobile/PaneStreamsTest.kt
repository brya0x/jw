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

/** The panes of a watched workspace as the PC's text (REQ-21), which the app never resizes (REQ-24). */
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

    /** A `Text` frame: each row is a list of `[col, text, fg, bg, attrs]` runs. */
    private fun text(pane: Long, cols: Int, rows: Int, vararg lines: String, cursor: String = "null") =
        FakeBridge.frame(
            """{"t":"Text","pane":$pane,"cols":$cols,"rows":$rows,"alt":false,"app_cursor":false,"cursor":$cursor,"lines":[${lines.joinToString(",")}]}""",
        )

    private fun attached(f: ByteArray) = json(f)?.contains("Attach") == true

    @Test fun watchAsksForTextThenAttachesAndTheTextFillsTheScreen() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws ->
            if (attached(f)) {
                ws.send(text(7, 40, 3, """[[0,"hello",0,0,0]]""", """[[0,"red",16777217,0,1]]""", "[]", cursor = "[1,3]"))
            }
        }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        assertEquals("""{"t":"TextView"}""", next())
        assertEquals("""{"t":"Attach","stream":"dir:/x"}""", next())
        val t = s.screen(7)
        assertEquals("hello", t.read { it.line(0).text() })
        assertEquals("red", t.read { it.line(1).text() })
        // `1 shl 24 or 1` is palette index 1, bold: the styles reach the cells.
        assertEquals(1 shl 24 or 1, t.read { it.line(1).fg[0] })
        assertEquals(dev.jw.term.Attr.BOLD, t.read { it.line(1).flags[0] })
        assertEquals(40, t.cols)
        assertEquals(1 to 3, t.read { it.cursorRow to it.cursorCol })
    }

    @Test fun aWideCharacterTakesTwoCellsAndAMarkJoinsItsBase() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws -> if (attached(f)) ws.send(text(7, 20, 1, """[[0,"a漢e\u0301b",0,0,0]]""")) }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        val t = s.screen(7)
        assertEquals("a漢e\u0301b", t.read { it.line(0).text() })
        assertEquals('漢'.code, t.read { it.line(0).ch[1] })
        assertTrue(t.read { it.line(0).flags[1] and dev.jw.term.Attr.WIDE != 0 && it.line(0).flags[2] and dev.jw.term.Attr.CONT != 0 })
        assertEquals('b'.code, t.read { it.line(0).ch[4] })
    }

    @Test fun aNewScreenReplacesTheOldAndAnExitReachesTheView() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws ->
            if (attached(f)) {
                ws.send(text(7, 40, 5, """[[0,"x",0,0,0]]"""))
                ws.send(text(7, 20, 8, """[[0,"y",0,0,0]]"""))
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
        assertEquals("y", s.terminal(7)!!.read { it.line(0).text() })
    }

    @Test fun scrollingUpAsksForTheHistoryAndItFillsTheRowsAboveTheScreen() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws ->
            val t = json(f) ?: ""
            if (attached(f)) ws.send(text(7, 40, 2, """[[0,"now",0,0,0]]"""))
            if (t.contains("TextHistory")) {
                ws.send(FakeBridge.frame("""{"t":"TextHistory","pane":7,"lines":[[[0,"older",0,0,0]],[[0,"old",0,0,0]]]}"""))
            }
        }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        val t = s.screen(7)
        assertEquals(0, t.scrollbackSize)
        s.requestHistory(7)
        withTimeout(10_000) { while (t.scrollbackSize == 0) kotlinx.coroutines.delay(10) }
        assertEquals("""{"t":"TextHistory","pane":7,"count":2000}""", generateSequence { next() }.first { it.contains("TextHistory") })
        assertEquals("old", t.read { it.line(-1).text() })
        assertEquals("older", t.read { it.line(-2).text() })
    }

    @Test fun leavingDetachesAndForgetsTheScreens() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onFrame = { f, ws -> if (attached(f)) ws.send(text(7, 40, 5, """[[0,"x",0,0,0]]""")) }
        val s = session()
        s.start()
        s.connected()
        s.watch("dir:/x")
        next()
        next()
        s.screen(7)
        s.watch(null)
        assertEquals("""{"t":"Detach"}""", next())
        assertNull(s.terminal(7))
        s.watch("dir:/y")
        assertEquals("""{"t":"TextView"}""", next())
        assertEquals("""{"t":"Attach","stream":"dir:/y"}""", next())
    }

    @Test fun terminalBytesStayOutOfToString() {
        assertTrue(!dev.jw.proto.PaneEvent.Output(1, "secret".toByteArray()).toString().contains("secret"))
    }
}
