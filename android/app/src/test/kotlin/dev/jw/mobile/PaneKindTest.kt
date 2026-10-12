package dev.jw.mobile

import dev.jw.proto.PaneEvent
import dev.jw.proto.PaneInfo
import dev.jw.proto.ShellBlock
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A pane changes what it is as its programs change, and its input stays the terminal's (REQ-93). */
class PaneKindTest {
    private fun pane(role: String, fg: String?) = PaneInfo(1, "s", role, null, fg, false, false, null, null)
    private fun chat(known: Boolean) = ChatState.merge(null, PaneEvent.Chat(1, known, null, null, true, emptyList()))
    private fun block(running: Boolean = false, full: Boolean = false) = ShellBlock(0, "x", "/w", null, 0, running, full, false)
    private fun shell(known: Boolean = true, running: Boolean = false, input: String? = "", vararg blocks: ShellBlock) =
        ShellState.merge(null, PaneEvent.Shell(1, known, running, input, blocks.toList()))

    @Test fun aMarkedShellIsCardsAndAnUnmarkedOneIsText() {
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), null, shell()))
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null, shell(known = false)))
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null, null))
        // A program that draws the whole screen in it is text for as long as it runs.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "nvim"), null, shell(running = true, blocks = arrayOf(block(running = true, full = true)))))
        // Another command that runs is still cards.
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "cargo"), null, shell(running = true, blocks = arrayOf(block(running = true)))))
    }

    @Test fun aShellThatStartsAnAgentWaitsThenIsTextUntilItHasAConversationAndGoesBackToCards() {
        val s = shell(running = true, blocks = arrayOf(block(running = true)))
        // At the prompt: cards.
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), null, s))
        // The agent command was run: claude is in the foreground and the PC has not said anything of it yet.
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("shell", "claude"), null, s))
        // The PC says it has no transcript yet: the screen, not the cards of the shell under it.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "claude"), chat(false), s))
        // Its first message makes the conversation.
        assertEquals(PaneKind.Chat, PaneKinds.of(pane("shell", "claude"), chat(true), s))
        // It exits: the cards are back, whatever the PC had said of the agent.
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), chat(true), s))
        // Any agent program counts, and an agent pane always may.
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("agent", "node"), null, null))
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("shell", "-claude"), null, null))
    }

    @Test fun aShellsInputIsTheLineItTellsWhileItIsAtItsPrompt() {
        val p = pane("shell", "zsh")
        val s = shell(input = "git st")
        assertEquals("git st", PaneKinds.input(p, s) { "never read" })
        assertTrue(PaneKinds.live(PaneKind.Shell, "git st"))
        // A shell that is not marked, or has none yet, keeps its line until it is sent.
        assertNull(PaneKinds.input(p, shell(known = false)) { "x" })
        assertNull(PaneKinds.input(p, null) { "x" })
        assertFalse(PaneKinds.live(PaneKind.Text, null))
        // A command that runs has no line: what is typed goes to it when sent.
        assertNull(PaneKinds.input(p, shell(running = true, input = "old")) { "x" })
        assertFalse(PaneKinds.live(PaneKind.Shell, null))
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
