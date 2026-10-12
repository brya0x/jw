package dev.jw.mobile

import dev.jw.proto.EntryKind
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

/** REQ-77 to 80: the PC's files asked for, answered, refused, or lost with the connection. */
class FilesSessionTest {
    private val fake = FakeBridge()
    private val store = FakeStore(StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    private fun session() = Session(scope, store, Bridge(), "Pixel", fake.endpoint(), { ReconnectPolicy() }, pollMs = 100_000, tickMs = 50)
    private suspend fun Session.connected() { withTimeout(20_000) { state.first { it is UiState.Connected } } }

    private fun ls(req: String, path: String = "/home/me") =
        """{"t":"ls","req":"$req","path":"$path","parent":"/home","entries":[{"name":"a","kind":"dir","size":null,"mtime":1,"target":null,"md":false},{"name":"b.md","kind":"file","size":3,"mtime":2,"target":null,"md":true}],"total":2,"truncated":false}"""

    private fun read(req: String) =
        """{"t":"read","req":"$req","path":"$req","kind":"text","size":2,"truncated":false,"text":"hi","blocks":null}"""

    @Test fun aListingAndAFileLandWhenTheyAreForWhatWasAsked() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"ls"""")) { ws.send(ls("/old")); ws.send(ls("~")) }
            if (t.contains(""""t":"read"""")) ws.send(read("/home/me/b.md"))
        }
        val s = session(); s.start(); s.connected()
        s.requestLs("~")
        val l = (withTimeout(10_000) { s.fs.first { it is FsState.Loaded } } as FsState.Loaded).listing
        assertEquals("~", l.req)   // the answer for "/old" was dropped
        assertEquals(listOf(EntryKind.Dir, EntryKind.File), l.entries.map { it.kind })
        s.requestRead("/home/me/b.md")
        val f = (withTimeout(10_000) { s.file.first { it is FileState.Loaded } } as FileState.Loaded).file
        assertEquals("hi", f.text)
        s.clearFiles()
        assertEquals(FsState.Idle, s.fs.value)
        assertEquals(FileState.Idle, s.file.value)
    }

    @Test fun anErrorIsTheAnswerOfItsOwnAsk() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"ls"""")) ws.send("""{"t":"error","code":"unsupported","msg":"permission denied","ask":"ls","req":"/root"}""")
            if (t.contains(""""t":"read"""")) ws.send("""{"t":"error","code":"unsupported","msg":"busy: wait for the answer before asking again","ask":"read","req":"/x"}""")
        }
        val s = session(); s.start(); s.connected()
        s.requestLs("/root")
        assertEquals("permission denied", (withTimeout(10_000) { s.fs.first { it is FsState.Failed } } as FsState.Failed).why)
        // A busy PC is said, not swallowed: the person can try again.
        s.requestRead("/x")
        assertTrue(withTimeout(10_000) { s.file.first { it is FileState.Failed } } is FileState.Failed)
    }

    @Test fun anAskLostWithTheConnectionIsAskedAgain() = runBlocking<Unit> {
        val asks = java.util.concurrent.atomic.AtomicInteger()
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()) }
        fake.onLater = { t, ws ->
            if (t.contains(""""t":"ls"""")) { if (asks.getAndIncrement() == 0) ws.close(1001, null) else ws.send(ls("~")) }
        }
        val s = session(); s.start(); s.connected()
        s.requestLs("~")
        withTimeout(20_000) { s.fs.first { it is FsState.Loaded } }
        assertEquals(2, fake.received.count { it == """{"t":"ls","path":"~"}""" })
    }
}
