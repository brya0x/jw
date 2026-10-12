package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** REQ-85 to 87: the asks that make things on the PC, and what comes back. */
class MakeTest {
    @Test fun theRequests() {
        assertEquals("""{"t":"open","path":"/a/b","session":"main"}""", Control.open("/a/b", "main"))
        assertEquals("""{"t":"open","path":"/a/b"}""", Control.open("/a/b", null))
        assertEquals("""{"t":"session_new","name":"trabajo","dir":"/a"}""", Control.sessionNew("trabajo", "/a"))
        assertEquals(
            """{"t":"worktree","project":"jw","name":"feat-x","branch":"feat/x","task":"haz esto","session":"main"}""",
            Control.worktree("jw", "feat-x", "feat/x", "haz esto", "main"),
        )
        // What is empty is not sent: the PC's own default is meant.
        assertEquals("""{"t":"worktree","project":"jw","name":"feat-x"}""", Control.worktree("jw", "feat-x", " ", "", null))
    }

    @Test fun theAnswers() {
        val o = Control.parse("""{"t":"opened","req":"/a","ws":"dir:/a","name":"a"}""") as ServerMessage.Opened
        assertEquals(Triple("/a", "dir:/a", "a"), Triple(o.req, o.ws, o.name))
        val s = Control.parse("""{"t":"session_created","name":"w","ws":"dir:/a#w"}""") as ServerMessage.SessionCreated
        assertEquals("dir:/a#w", s.ws)
        val w = Control.parse("""{"t":"worktree","project":"jw","name":"feat-x","ws":"id1","branch":"feat/x"}""") as ServerMessage.WorktreeMade
        assertEquals(listOf("jw", "feat-x", "id1", "feat/x"), listOf(w.project, w.name, w.ws, w.branch))
        assertNull(Control.parse("""{"t":"opened"}"""))
    }

    @Test fun anErrorNamesItsAsk() {
        val e = Control.parse("""{"t":"error","code":"read_only","msg":"this bridge is read-only","ask":"worktree","project":"jw","name":"feat-x"}""") as ServerMessage.Error
        assertEquals(Triple("worktree", "jw", "feat-x"), Triple(e.ask, e.project, e.name))
        assertNull((Control.parse("""{"t":"error","code":"x","msg":"m"}""") as ServerMessage.Error).name)
    }
}
