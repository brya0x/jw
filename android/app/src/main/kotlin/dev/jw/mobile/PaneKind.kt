package dev.jw.mobile

import dev.jw.proto.PaneInfo
import dev.jw.proto.mayHaveConversation

/** How the pane on screen is drawn (REQ-21, REQ-91). */
enum class PaneKind {
    /** A Claude that has not been answered about yet: nothing to draw. */
    Loading,

    /** The conversation of a Claude session, as blocks. */
    Chat,

    /** The commands of a shell jw marked, as cards. */
    Shell,

    /** The terminal's text: a shell that is not one of jw's, a program that draws the whole screen, a Claude with no transcript yet, anything else. */
    Text,
}

/**
 * What a pane is drawn as, and which text is the terminal's own input (REQ-93), from what the PC has said about it. It
 * follows the pane as it changes: a shell jw marked is its commands as cards; when it starts claude (the foreground program
 * becomes an agent) the pane waits for the PC, stays text until the agent has a transcript, then is its conversation; when it
 * exits, cards again. A program that draws the whole screen in it is text for as long as it runs. The input is the terminal's all along.
 */
object PaneKinds {
    fun of(pane: PaneInfo, chat: ChatState?, shell: ShellState?): PaneKind = when {
        pane.mayHaveConversation() -> when {
            chat == null -> PaneKind.Loading
            chat.known -> PaneKind.Chat
            else -> PaneKind.Text
        }
        pane.role == "shell" && shell?.known == true && !(shell.running && shell.blocks.lastOrNull()?.full == true) -> PaneKind.Shell
        else -> PaneKind.Text
    }

    /**
     * The terminal's input text, or null when it cannot be told: for a Claude (its conversation, or its screen before it
     * has one) the box read off the screen, [screenBox]; for a shell at its prompt, the line it tells. A shell that runs
     * a command has no line: what is typed goes to the command, when it is sent.
     */
    fun input(pane: PaneInfo, shell: ShellState?, screenBox: () -> String?): String? = when {
        pane.mayHaveConversation() -> screenBox()
        pane.role == "shell" && shell?.known == true && !shell.running -> shell.input
        else -> null
    }

    /** Whether what is typed on the phone goes to the terminal as it is typed: the pane's input is the terminal's. */
    fun live(kind: PaneKind, input: String?): Boolean = kind == PaneKind.Chat || input != null
}
