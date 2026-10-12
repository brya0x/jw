package dev.jw.mobile

import dev.jw.proto.PaneInfo
import dev.jw.proto.mayHaveConversation

/** How the pane on screen is drawn (REQ-21, REQ-91, REQ-92). */
enum class PaneKind {
    /** A Claude that has not been answered about yet: nothing to draw. */
    Loading,

    /** The conversation of a Claude session, as blocks. */
    Chat,

    /** The commands of a shell jw marks, as cards. */
    Shell,

    /** The terminal's text. */
    Text,
}

/**
 * What a pane is drawn as, and which text is the terminal's own input (REQ-93), from what the PC has said about it. It
 * follows the pane as it changes: a shell that starts claude goes from [PaneKind.Shell] to [PaneKind.Text] (a Claude
 * with no transcript yet) and then to [PaneKind.Chat], and the input stays the terminal's all along.
 */
object PaneKinds {
    fun of(pane: PaneInfo, chat: ChatState?, shell: ShellState?): PaneKind = when {
        pane.mayHaveConversation() -> when {
            chat == null -> PaneKind.Loading
            chat.known -> PaneKind.Chat
            else -> PaneKind.Text
        }
        pane.role == "shell" && shell?.known == true && shell.blocks.lastOrNull()?.let { it.running && it.full } != true -> PaneKind.Shell
        else -> PaneKind.Text
    }

    /**
     * The terminal's input text, or null when it cannot be told: a shell's line as it tells it, and for a Claude (its
     * conversation or its screen before it has one) the box read off the screen, [screenBox].
     */
    fun input(kind: PaneKind, pane: PaneInfo, shell: ShellState?, screenBox: () -> String?): String? = when {
        kind == PaneKind.Shell -> shell?.input
        pane.mayHaveConversation() -> screenBox()
        else -> null
    }

    /** Whether what is typed on the phone goes to the terminal as it is typed: the pane's input is the terminal's. */
    fun live(kind: PaneKind, input: String?): Boolean = kind == PaneKind.Chat || kind == PaneKind.Shell || input != null
}
