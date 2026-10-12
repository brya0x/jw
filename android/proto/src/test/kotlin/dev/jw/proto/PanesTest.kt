package dev.jw.proto

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PanesTest {
    @Test fun parsesSnapshotOutputResizedExited() {
        val s = Panes.parse(Frame.Raw("""{"t":"Snapshot","pane":7,"role":"shell","cols":80,"rows":24,"bytes":[]}""", byteArrayOf(65, 66))) as PaneEvent.Snapshot
        assertEquals(7, s.pane); assertEquals(80, s.cols); assertEquals(24, s.rows)
        assertArrayEquals(byteArrayOf(65, 66), s.bytes)
        val o = Panes.parse(Frame.Raw("""{"t":"Output","pane":7,"bytes":[]}""", byteArrayOf(1))) as PaneEvent.Output
        assertArrayEquals(byteArrayOf(1), o.bytes)
        val r = Panes.parse(Frame.Json("""{"t":"Resized","pane":7,"cols":100,"rows":30}""")) as PaneEvent.Resized
        assertEquals(100, r.cols); assertEquals(30, r.rows)
        assertEquals(3, (Panes.parse(Frame.Json("""{"t":"Exited","pane":7,"status":3}""")) as PaneEvent.Exited).status)
    }

    @Test fun otherMessagesAndBadShapesAreNotPaneEvents() {
        assertNull(Panes.parse(Frame.Json("""{"t":"Panes","panes":[]}""")))
        assertNull(Panes.parse(Frame.Json("""{"t":"Title","pane":1,"title":"x"}""")))
        assertNull(Panes.parse(Frame.Json("nope")))
        // A size of zero or a screen of a million cells is refused, not allocated.
        assertNull(Panes.parse(Frame.Raw("""{"t":"Snapshot","pane":1,"role":"s","cols":0,"rows":24}""", ByteArray(0))))
        assertNull(Panes.parse(Frame.Raw("""{"t":"Snapshot","pane":1,"role":"s","cols":5000,"rows":5000}""", ByteArray(0))))
        assertNull(Panes.parse(Frame.Json("""{"t":"Snapshot","pane":1,"role":"s","cols":80,"rows":24}""")))
    }

    @Test fun toStringKeepsTheTerminalBytesOut() {
        val s = PaneEvent.Snapshot(1, 80, 24, "secret".toByteArray())
        assertFalse(s.toString().contains("secret"))
        assertFalse(PaneEvent.Output(1, "secret".toByteArray()).toString().contains("secret"))
    }

    @Test fun clientFrames() {
        assertEquals("""{"t":"Attach","stream":"dir:/x"}""", (Panes.attachFrame("dir:/x") as Frame.Json).text)
        assertEquals("""{"t":"Detach"}""", (Panes.detachFrame() as Frame.Json).text)
        assertEquals("""{"t":"Resize","pane":3,"cols":50,"rows":20}""", (Panes.resizeFrame(3, 50, 20) as Frame.Json).text)
        assertTrue(Panes.attachFrame("a") is Frame.Json)
    }
}
