package dev.jw.mobile

import dev.jw.proto.ChatBlock
import dev.jw.proto.ChatTurn
import dev.jw.proto.PaneEvent

/** The conversation of an agent as the PC sent it: its name, its title and its blocks, oldest first (REQ-90). */
class ChatState(
    val known: Boolean, val name: String?, val title: String?, val blocks: List<ChatBlock>,
    /** The turn in progress and the background tasks that have not ended (REQ-96, REQ-97). */
    val turn: ChatTurn? = null, val bg: Int = 0,
    /** The plan Claude drafted for the conversation, when the PC sent it (REQ-95). */
    val plan: String? = null,
) {
    companion object {
        /** The most blocks held; the oldest go first. */
        const val KEEP = 400

        /** What [old] becomes with [e]: all of it when the PC says reset, else the blocks that came put in by their number. */
        fun merge(old: ChatState?, e: PaneEvent.Chat): ChatState {
            val title = e.title ?: old?.title
            val name = e.name ?: old?.name
            val turn = e.turn ?: old?.turn.takeIf { !e.reset }
            val bg = e.bg ?: old?.bg?.takeIf { !e.reset } ?: 0
            val plan = e.plan ?: old?.plan?.takeIf { !e.reset }
            if (e.reset || old == null) return ChatState(e.known, name, title, e.blocks.sortedBy { it.i }.takeLast(KEEP), turn, bg, plan)
            val byNumber = old.blocks.associateByTo(java.util.TreeMap()) { it.i }
            for (b in e.blocks) byNumber[b.i] = b
            return ChatState(old.known, name, title, byNumber.values.toList().takeLast(KEEP), turn, bg, plan)
        }
    }
}

/**
 * What a shell says of itself and its commands, as the PC sent it: whether it is one jw marked, whether a command runs, the line typed at
 * its prompt and the commands, oldest first (REQ-92). What each printed is in its [BlockOut].
 */
class ShellState(val known: Boolean, val running: Boolean, val input: String?, val blocks: List<dev.jw.proto.ShellBlock>) {
    companion object {
        const val KEEP = 200

        /** What [old] becomes with [e]: the line stays when the message has none (it is not news), the blocks that came are put in by their number. */
        fun merge(old: ShellState?, e: PaneEvent.Shell): ShellState {
            val byNumber = (old?.blocks ?: emptyList()).associateByTo(java.util.TreeMap()) { it.i }
            for (b in e.blocks) byNumber[b.i] = b
            return ShellState(e.known, e.running, e.input ?: old?.input, byNumber.values.toList().takeLast(KEEP))
        }
    }
}
