package dev.jw.mobile

import dev.jw.proto.Control
import dev.jw.proto.DiffFile
import dev.jw.proto.DiffScope
import dev.jw.proto.ServerMessage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-74: a viewed mark is of what was shown, per workspace and view, and does not outlive its file. */
class ViewedMarksTest {
    private class Mem(var set: Set<String> = emptySet()) : MarkStore {
        override fun load() = set
        override fun save(marks: Set<String>) { set = marks }
    }

    private fun file(path: String, text: String = "x"): DiffFile {
        val json = """{"t":"diff","ws":"w","scope":"general","files":[{"path":"$path","status":"modified","hunks":[{"lines":[{"k":"add","text":"$text","new":1}]}]}]}"""
        return ((Control.parse(json) as ServerMessage.DiffAnswer).diff).files[0]
    }

    @Test fun aMarkSurvivesARestartAndAnEditResetsIt() {
        val mem = Mem()
        val a = ViewedMarks(mem)
        a.toggle("w", DiffScope.General, file("a.rs"))
        val b = ViewedMarks(mem)
        assertTrue(b.isViewed("w", DiffScope.General, file("a.rs")))
        assertFalse(b.isViewed("w", DiffScope.General, file("a.rs", text = "edited")))
        assertFalse(b.isViewed("w", DiffScope.Uncommitted, file("a.rs")))
        assertFalse(b.isViewed("other", DiffScope.General, file("a.rs")))
        b.toggle("w", DiffScope.General, file("a.rs"))
        assertFalse(b.isViewed("w", DiffScope.General, file("a.rs")))
    }

    @Test fun pruneDropsWhatTheViewNoLongerHasAndOnlyThatView() {
        val m = ViewedMarks(Mem())
        m.toggle("w", DiffScope.General, file("keep.rs"))
        m.toggle("w", DiffScope.General, file("gone.rs"))
        m.toggle("w", DiffScope.Uncommitted, file("gone.rs"))
        m.prune("w", DiffScope.General, listOf(file("keep.rs")))
        assertTrue(m.isViewed("w", DiffScope.General, file("keep.rs")))
        assertFalse(m.isViewed("w", DiffScope.General, file("gone.rs")))
        assertTrue(m.isViewed("w", DiffScope.Uncommitted, file("gone.rs")))
    }

    @Test fun countsFollowTheMarks() {
        val m = ViewedMarks(Mem())
        val files = listOf(file("a"), file("b"))
        m.toggle("w", DiffScope.General, files[0])
        assertEquals(1, m.count("w", DiffScope.General, files))
        m.toggle("w", DiffScope.General, files[1])
        assertEquals(2, m.count("w", DiffScope.General, files))
    }

    @Test fun theStoreDoesNotGrowWithoutBound() {
        val mem = Mem()
        val m = ViewedMarks(mem)
        for (i in 0 until 4100) m.toggle("w$i", DiffScope.General, file("f"))
        assertTrue(mem.set.size <= 4000)
        assertTrue(m.isViewed("w4099", DiffScope.General, file("f")))
        assertFalse(m.isViewed("w0", DiffScope.General, file("f")))
    }
}
