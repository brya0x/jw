package dev.jw.mobile

import dev.jw.proto.PaneEvent
import dev.jw.proto.PaneInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A pane changes what it is as its programs change, and its input stays the terminal's (REQ-93). */
class PaneKindTest {
    private fun pane(role: String, fg: String?) = PaneInfo(1, "s", role, null, fg, false, false, null, null)
    private fun chat(known: Boolean) = ChatState.merge(null, PaneEvent.Chat(1, known, null, null, true, emptyList()))
    private fun shell(known: Boolean = true, running: Boolean = false, input: String? = "") =
        ShellState.merge(null, PaneEvent.Shell(1, known, running, input))

    @Test fun aShellIsText() {
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null))
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "nvim"), null))
    }

    @Test fun aShellThatStartsAnAgentWaitsThenIsTextUntilItHasAConversationAndGoesBack() {
        // At the prompt.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null))
        // The agent command was run: claude is in the foreground and the PC has not said anything of it yet.
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("shell", "claude"), null))
        // The PC says it has no transcript yet: its screen.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "claude"), chat(false)))
        // Its first message makes the conversation.
        assertEquals(PaneKind.Chat, PaneKinds.of(pane("shell", "claude"), chat(true)))
        // It exits: the shell is text again, whatever the PC had said of the agent.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), chat(true)))
        // Any agent program counts, and an agent pane always may.
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("agent", "node"), null))
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("shell", "-claude"), null))
    }

    @Test fun aShellsInputIsTheLineItTellsWhileItIsAtItsPrompt() {
        val p = pane("shell", "zsh")
        val s = shell(input = "git st")
        assertEquals("git st", PaneKinds.input(p, s) { "never read" })
        assertTrue(PaneKinds.live(PaneKind.Text, "git st"))
        // A shell that is not marked, or has none yet, keeps its line until it is sent.
        assertNull(PaneKinds.input(p, shell(known = false)) { "x" })
        assertNull(PaneKinds.input(p, null) { "x" })
        assertFalse(PaneKinds.live(PaneKind.Text, null))
        // A command that runs has no line: what is typed goes to it when sent.
        assertNull(PaneKinds.input(p, shell(running = true, input = "old")) { "x" })
    }

    @Test fun anAgentsInputIsTheBoxOnItsScreenAtEveryStep() {
        val s = shell(input = "stale")
        val p = pane("shell", "claude")
        // No transcript yet: the screen's box is the input as soon as it is found.
        assertEquals("hola", PaneKinds.input(p, s) { "hola" })
        assertTrue(PaneKinds.live(PaneKind.Text, "hola"))
        // A dialog, no box: the keys are not sent as they are typed.
        assertNull(PaneKinds.input(p, s) { null })
        assertFalse(PaneKinds.live(PaneKind.Text, null))
        // The conversation is live even when its box cannot be read at that moment.
        assertTrue(PaneKinds.live(PaneKind.Chat, null))
        // Any other pane keeps its own line until it is sent.
        assertNull(PaneKinds.input(pane("dev", "node"), null) { "x" })
    }
}
