package dev.jw.proto

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Fixtures copy what `a_paired_phone_is_told_the_workspaces_and_sessions_of_the_pcs_jw`
 * (crates/jw/tests/mobile.rs) expects: folder Notes, root jw, worktrees alpha (PR 43) and beta (no PR).
 */
class WorkspacesTest {
    private val workspacesJson = """{"t":"workspaces","session":"main","items":[
      {"id":"f1","kind":"folder","name":"Notes","path":"/nowhere/notes","project":"","branch":"","pr":null,"session":"main","parent":null},
      {"id":"r1","kind":"root","name":"jw","path":"/nowhere/jw","project":"jw","branch":"main","pr":null,"session":"main","parent":null},
      {"id":"w-alpha","kind":"worktree","name":"alpha","path":"/nowhere/jw-alpha","project":"jw","branch":"feat/alpha","pr":43,"session":"main","parent":"r1"},
      {"id":"w-beta","kind":"worktree","name":"beta","path":"/nowhere/jw-beta","project":"jw","branch":"feat/beta","pr":null,"session":"main","parent":"r1"}]}"""

    // PaneInfo exactly as serde writes it: every field present, `exited` null when running.
    private fun pane(n: Long, stream: String, role: String, agent: String? = null, exited: String = "null", busy: Boolean = false, bell: Boolean = false) =
        """{"pane":$n,"stream":"$stream","role":"$role","exited":$exited,"fg":null,"busy":$busy,"bell":$bell,"agent":${agent?.let { "\"$it\"" } ?: "null"},"cwd":"/x"}"""

    private fun panes(vararg p: String) = Workspaces.parsePanes(Frame.Json("""{"t":"Panes","panes":[${p.joinToString(",")}]}"""))!!

    @Test fun parsesWorkspacesInSidebarOrder() {
        val m = Control.parse(workspacesJson) as ServerMessage.Workspaces
        assertEquals("main", m.session)
        assertEquals(listOf("Notes" to Kind.Folder, "jw" to Kind.Root, "alpha" to Kind.Worktree, "beta" to Kind.Worktree), m.items.map { it.name to it.kind })
        assertNull(m.items[0].parent)
        assertEquals("r1", m.items[2].parent)
        assertEquals(43, m.items[2].pr)
        assertNull(m.items[3].pr)
        assertEquals("feat/alpha", m.items[2].branch)
    }

    @Test fun parsesSessions() {
        val m = Control.parse("""{"t":"sessions","items":[{"name":"main","workspaces":3}]}""") as ServerMessage.Sessions
        assertEquals("main", m.items.single().name)
        assertEquals(3, m.items.single().workspaces)
    }

    @Test fun requestsAreWhatTheBridgeExpects() {
        assertEquals("""{"t":"workspaces"}""", Control.workspaces())
        assertEquals("""{"t":"workspaces","session":"work"}""", Control.workspaces("work"))
        assertEquals("""{"t":"sessions"}""", Control.sessions())
    }

    @Test fun listAndInputFramesAreWhatTheDaemonReads() {
        assertArrayEquals(byteArrayOf(0) + """{"t":"List"}""".toByteArray(), FrameCodec.encode(Workspaces.listFrame()))
        val enc = FrameCodec.encode(Workspaces.inputFrame(7, byteArrayOf(0x0d)))
        val f = FrameCodec.decode(enc) as Frame.Raw
        assertEquals("""{"t":"Input","pane":7,"bytes":[]}""", f.head)
        assertArrayEquals(byteArrayOf(0x0d), f.bytes)
        assertEquals(1, enc[0].toInt())
    }

    @Test fun parsesPanesAndAgentStates() {
        val ps = panes(
            pane(1, "r1", "agent", "working"), pane(2, "r1", "dev:web"), pane(3, "w-alpha", "agent", "waiting"),
            pane(4, "w-beta", "agent", "idle", exited = "0"),
        )
        assertEquals(listOf(AgentState.Working, null, AgentState.Waiting, AgentState.Idle), ps.map { it.agent })
        assertEquals(0, ps[3].exited)
        assertNull(ps[0].exited)
        assertEquals("/x", ps[0].cwd)
    }

    @Test fun oldPaneInfoWithoutOptionalFieldsStillParses() {
        val ps = Workspaces.parsePanes(Frame.Json("""{"t":"Panes","panes":[{"pane":1,"stream":"s","role":"shell","exited":null}]}"""))!!
        assertFalse(ps[0].busy)
        assertNull(ps[0].agent)
    }

    @Test fun otherFramesAreNotPanes() {
        assertNull(Workspaces.parsePanes(Frame.Json("""{"t":"Output"}""")))
        assertNull(Workspaces.parsePanes(Frame.Raw("{}", byteArrayOf())))
        assertNull(Workspaces.parsePanes(Frame.Json("garbage")))
    }

    @Test fun marksFollowTheTui() {
        fun m(vararg p: String) = Workspaces.marks(panes(*p))
        assertEquals(listOf(Mark.Working), m(pane(1, "a", "agent", "working")))
        assertEquals(listOf(Mark.Waiting), m(pane(1, "a", "agent", "waiting")))
        assertEquals(listOf(Mark.Dev), m(pane(1, "a", "dev:web")))
        assertEquals(emptyList<Mark>(), m(pane(1, "a", "dev:web", exited = "1")))
        assertEquals(listOf(Mark.Working, Mark.Waiting, Mark.Dev), m(pane(1, "a", "agent", "working"), pane(2, "a", "agent", "waiting"), pane(3, "a", "dev:x")))
        // no hooks: output and bell decide, only for the agent role
        assertEquals(listOf(Mark.Working), m(pane(1, "a", "agent", busy = true)))
        assertEquals(listOf(Mark.Waiting), m(pane(1, "a", "agent", bell = true)))
        assertEquals(emptyList<Mark>(), m(pane(1, "a", "agent", bell = true, busy = true)).filter { it == Mark.Waiting })
        assertEquals(emptyList<Mark>(), m(pane(1, "a", "shell", busy = true, bell = true)))
        // an idle agent with hooks shows nothing, even if busy output says otherwise
        assertEquals(emptyList<Mark>(), m(pane(1, "a", "agent", "idle", busy = true)))
    }

    @Test fun joinIsByStreamId() {
        val ps = panes(pane(1, "r1", "agent"), pane(2, "w-alpha", "agent"), pane(3, "w-alpha", "shell"), pane(4, "f1", "shell"))
        assertEquals(listOf(2L, 3L), Workspaces.panesOf(ps, "w-alpha").map { it.pane })
        // UNCONFIRMED by the server agent (2026-10-11): the join for kind root (r1) and folder (f1).
        // Only worktrees are confirmed; this test pins the join by id for all three kinds until that is answered.
        assertEquals(1, Workspaces.panesOf(ps, "r1").size)
        assertEquals(1, Workspaces.panesOf(ps, "f1").size)
        assertEquals(0, Workspaces.panesOf(ps, "w-beta").size)
    }

    @Test fun waitingPaneNeedsHooks() {
        val ps = panes(pane(1, "a", "agent", bell = true), pane(2, "a", "agent", "waiting"))
        assertEquals(2L, Workspaces.waitingPane(ps)!!.pane)
        assertNull(Workspaces.waitingPane(panes(pane(1, "a", "agent", bell = true))))
    }

    @Test fun refreshEveryTwoSecondsOnlyWhileActive() {
        val r = RefreshRule(2_000)
        assertTrue(r.due(0, true))
        assertFalse(r.due(1_999, true))
        assertTrue(r.due(2_000, true))
        assertFalse(r.due(10_000, false))
        // after going inactive, the first active tick asks at once
        assertTrue(r.due(10_001, true))
        assertFalse(r.due(11_000, true))
    }

    @Test fun answerButtonsRule() {
        val w = panes(pane(1, "a", "agent", "waiting"))[0]
        val idle = panes(pane(1, "a", "agent", "idle"))[0]
        assertTrue(AnswerRule.enabled(w, 1_000, 7_000, false, false))
        assertTrue("a poll (2 s) may be late without dimming", AnswerRule.enabled(w, 1_000, 4_500, false, false))
        assertFalse("older than 6 s", AnswerRule.enabled(w, 1_000, 7_001, false, false))
        assertFalse("not waiting", AnswerRule.enabled(idle, 1_000, 1_500, false, false))
        assertFalse("no pane", AnswerRule.enabled(null, 1_000, 1_500, false, false))
        assertFalse("read only", AnswerRule.enabled(w, 1_000, 1_500, true, false))
        assertFalse("already answered", AnswerRule.enabled(w, 1_000, 1_500, false, true))
        assertFalse("no snapshot yet", AnswerRule.enabled(w, null, 1_500, false, false))
    }

    @Test fun defaultAnswersAreEnterAndEsc() {
        assertArrayEquals(byteArrayOf(13), AnswerChoices.approve(null).bytes)
        assertArrayEquals(byteArrayOf(27), AnswerChoices.reject("nope").bytes)
        assertArrayEquals("y\r".toByteArray(), AnswerChoices.approve("y-enter").bytes)
    }

    @Test fun anAgentPaneOrAClaudeInAShellMayHaveAConversation() {
        fun pane(role: String, fg: String?) = PaneInfo(1, "s", role, null, fg, false, false, null, null)
        assertTrue(pane("agent", null).mayHaveConversation())
        assertTrue(pane("agent", "zsh").mayHaveConversation())
        assertTrue(pane("shell", "claude").mayHaveConversation())
        assertTrue(pane("shell", "-claude").mayHaveConversation())
        assertFalse(pane("shell", "zsh").mayHaveConversation())
        assertFalse(pane("shell", "nvim").mayHaveConversation())
        assertFalse(pane("shell", null).mayHaveConversation())
    }
}
