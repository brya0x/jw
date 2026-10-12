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

    @Test fun splitFollowsTheTuiRule() {
        val wide = (Panes.splitFrame(4, "/w", 120, 30) as Frame.Json).text
        assertEquals(
            """{"t":"Split","pane":4,"dir":"right","new":{"role":"shell","cmd":null,"resume":null,"cwd":"/w","env":{},"cols":60,"rows":30}}""",
            wide,
        )
        val tall = (Panes.splitFrame(4, "/w", 80, 50) as Frame.Json).text
        assertTrue(tall.contains(""""dir":"down"""") && tall.contains(""""cols":80,"rows":25"""))
    }

    @Test fun killAndName() {
        assertEquals("""{"t":"Kill","pane":9}""", (Panes.killFrame(9) as Frame.Json).text)
        assertEquals("""{"t":"Name","pane":9,"name":"dev"}""", (Panes.nameFrame(9, " dev ") as Frame.Json).text)
        assertEquals("""{"t":"Name","pane":9,"name":null}""", (Panes.nameFrame(9, "  ") as Frame.Json).text)
    }

    @Test fun parsesAnAgentsConversationAsBlocks() {
        val c = Panes.parse(
            Frame.Json(
                """{"t":"Chat","pane":7,"known":true,"title":"Arreglar el socket","reset":true,"blocks":[
                {"i":0,"kind":"user","text":"hola","name":"","arg":"","result":null,"body":null,"error":false},
                {"i":1,"kind":"tool","text":"","name":"Edit","arg":"a.rs","result":"+1 −2","body":"- x\n+ z","error":false}]}""",
            ),
        ) as PaneEvent.Chat
        assertEquals(7, c.pane); assertTrue(c.known); assertTrue(c.reset)
        assertEquals("Arreglar el socket", c.title)
        assertEquals(listOf("user", "tool"), c.blocks.map { it.kind })
        assertEquals("hola", c.blocks[0].text)
        assertNull(c.blocks[0].result)
        assertEquals("+1 −2", c.blocks[1].result)
        assertEquals("- x\n+ z", c.blocks[1].body)
        // A pane that is no Claude session: known false, no blocks.
        val none = Panes.parse(Frame.Json("""{"t":"Chat","pane":8,"known":false,"title":null,"reset":true,"blocks":[]}""")) as PaneEvent.Chat
        assertFalse(none.known); assertEquals(0, none.blocks.size)
        // A block that is not an object, or has no number, refuses the message.
        assertNull(Panes.parse(Frame.Json("""{"t":"Chat","pane":8,"known":true,"reset":false,"blocks":[1]}""")))
        assertNull(Panes.parse(Frame.Json("""{"t":"Chat","pane":8,"known":true,"reset":false,"blocks":[{"kind":"user"}]}""")))
    }

    @Test fun theChatCarriesTheNameThePersonGaveIt() {
        val c = Panes.parse(Frame.Json("""{"t":"Chat","pane":7,"known":true,"name":"mobile-fixes","title":"Arreglar","reset":true,"blocks":[]}""")) as PaneEvent.Chat
        assertEquals("mobile-fixes", c.name); assertEquals("Arreglar", c.title)
        assertNull((Panes.parse(Frame.Json("""{"t":"Chat","pane":7,"known":true,"name":null,"title":null,"reset":true,"blocks":[]}""")) as PaneEvent.Chat).name)
    }

    @Test fun theChatCarriesTheTurnTheTasksAndTheNewKindsOfBlock() {
        val c = Panes.parse(
            Frame.Json(
                """{"t":"Chat","pane":7,"known":true,"turn":{"since":1791828000500,"tokens":42},"bg":2,"reset":false,"blocks":[
                {"i":3,"kind":"notice","text":"Scout","name":"done","arg":"agent","result":"19 s","body":null,"error":false},
                {"i":4,"kind":"queued","text":"y esto","name":"","arg":"","result":null,"body":null,"error":false}]}""",
            ),
        ) as PaneEvent.Chat
        assertEquals(1791828000500L, c.turn?.since); assertEquals(42L, c.turn?.tokens); assertEquals(2, c.bg)
        assertEquals(listOf("notice", "queued"), c.blocks.map { it.kind })
        assertEquals("done", c.blocks[0].name); assertEquals("19 s", c.blocks[0].result)
        // Neither says nothing: they are not news.
        val none = Panes.parse(Frame.Json("""{"t":"Chat","pane":7,"known":true,"reset":false,"blocks":[]}""")) as PaneEvent.Chat
        assertNull(none.turn); assertNull(none.bg); assertNull(none.plan)
        val withPlan = Panes.parse(Frame.Json("""{"t":"Chat","pane":7,"known":true,"plan":"# Plan\n1. Daemon","reset":false,"blocks":[]}""")) as PaneEvent.Chat
        assertEquals("# Plan\n1. Daemon", withPlan.plan)
    }

    @Test fun parsesWhatAShellSaysOfItselfAndItsCommands() {
        val s = Panes.parse(
            Frame.Json(
                """{"t":"Shell","pane":9,"known":true,"running":true,"input":"git st","blocks":[
                {"i":0,"cmd":"cargo test","cwd":"/w","code":0,"ms":6200,"running":false,"full":false,"truncated":false},
                {"i":1,"cmd":"nvim a","cwd":"/w","code":null,"ms":12000,"running":true,"full":true,"truncated":true}]}""",
            ),
        ) as PaneEvent.Shell
        assertEquals(9, s.pane); assertTrue(s.known); assertTrue(s.running); assertEquals("git st", s.input)
        assertEquals(listOf("cargo test", "nvim a"), s.blocks.map { it.cmd })
        assertEquals(0, s.blocks[0].code); assertNull(s.blocks[1].code)
        assertTrue(s.blocks[1].running); assertTrue(s.blocks[1].full); assertTrue(s.blocks[1].truncated)
        assertEquals(6200L, s.blocks[0].ms)
        // No line in the message: not news. A block with no number refuses the message.
        assertNull((Panes.parse(Frame.Json("""{"t":"Shell","pane":9,"known":false,"running":false,"blocks":[]}""")) as PaneEvent.Shell).input)
        assertNull(Panes.parse(Frame.Json("""{"t":"Shell","pane":9,"known":true,"running":false,"blocks":[{"cmd":"x"}]}""")))
        assertEquals("""{"t":"ShellWatch","pane":9}""", (Panes.shellWatchFrame(9) as Frame.Json).text)
        assertEquals("""{"t":"ShellUnwatch","pane":9}""", (Panes.shellUnwatchFrame(9) as Frame.Json).text)
    }

    @Test fun parsesTheBytesACommandPrinted() {
        val o = Panes.parse(Frame.Raw("""{"t":"ShellOut","pane":9,"block":3,"bytes":[]}""", byteArrayOf(65, 66))) as PaneEvent.ShellOut
        assertEquals(9, o.pane); assertEquals(3, o.block); assertArrayEquals(byteArrayOf(65, 66), o.bytes)
        // Bytes with no number of block, or no bytes, are not an event.
        assertNull(Panes.parse(Frame.Raw("""{"t":"ShellOut","pane":9,"bytes":[]}""", byteArrayOf(1))))
        assertNull(Panes.parse(Frame.Json("""{"t":"ShellOut","pane":9,"block":3}""")))
    }

    @Test fun chatFramesNameThePane() {
        assertEquals("""{"t":"ChatWatch","pane":9}""", (Panes.chatWatchFrame(9) as Frame.Json).text)
        assertEquals("""{"t":"ChatUnwatch","pane":9}""", (Panes.chatUnwatchFrame(9) as Frame.Json).text)
    }
}
