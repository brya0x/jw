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
    private fun shell(vararg blocks: ShellBlock, known: Boolean = true, input: String? = "") =
        ShellState.merge(null, PaneEvent.Shell(1, known, true, blocks.toList(), input))
    private fun block(running: Boolean = false, full: Boolean = false) = ShellBlock(0, "x", "/w", "", 0, null, 0, running, full)

    @Test fun aMarkedShellIsCardsAndAnUnmarkedOneIsText() {
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), null, shell()))
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null, shell(known = false)))
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "zsh"), null, null))
        // A program that draws the whole screen in it is text for as long as it runs.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "nvim"), null, shell(block(running = true, full = true))))
    }

    @Test fun aShellThatStartsClaudeGoesFromCardsToTextToTheConversation() {
        val shellState = shell(block(running = true))
        // At the prompt: cards.
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), null, shellState))
        // claude is in the foreground and the PC has not said anything of it yet.
        assertEquals(PaneKind.Loading, PaneKinds.of(pane("shell", "claude"), null, shellState))
        // The PC says it has no transcript yet: the screen, not the cards of the shell under it.
        assertEquals(PaneKind.Text, PaneKinds.of(pane("shell", "claude"), chat(false), shellState))
        // Its first message makes the conversation.
        assertEquals(PaneKind.Chat, PaneKinds.of(pane("shell", "claude"), chat(true), shellState))
        // And when it exits, the cards are back.
        assertEquals(PaneKind.Shell, PaneKinds.of(pane("shell", "zsh"), null, shellState))
    }

    @Test fun theInputIsTheTerminalsAtEveryStepOfIt() {
        val s = shell(input = "git st")
        val sh = PaneKinds.of(pane("shell", "zsh"), null, s)
        assertEquals("git st", PaneKinds.input(sh, pane("shell", "zsh"), s) { "never read" })
        assertTrue(PaneKinds.live(sh, "git st"))
        // claude with no transcript yet: the box on its screen is the input, so it is live as soon as it is found.
        val p = pane("shell", "claude")
        val text = PaneKinds.of(p, chat(false), s)
        assertEquals("hola", PaneKinds.input(text, p, s) { "hola" })
        assertTrue(PaneKinds.live(text, "hola"))
        // No box on the screen (a dialog): the keys are not sent as they are typed.
        assertNull(PaneKinds.input(text, p, s) { null })
        assertFalse(PaneKinds.live(text, null))
        // The conversation is live even when its box cannot be read at that moment.
        assertTrue(PaneKinds.live(PaneKind.Chat, null))
        // Any other pane keeps its own line until it is sent.
        val dev = pane("dev", "node")
        assertNull(PaneKinds.input(PaneKind.Text, dev, null) { "x" })
        assertFalse(PaneKinds.live(PaneKind.Text, null))
    }
}
