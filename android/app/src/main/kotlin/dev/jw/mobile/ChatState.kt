package dev.jw.mobile

import dev.jw.proto.ChatBlock
import dev.jw.proto.PaneEvent

/** The conversation of an agent as the PC sent it: its name, its title and its blocks, oldest first (REQ-90). */
class ChatState(val known: Boolean, val name: String?, val title: String?, val blocks: List<ChatBlock>) {
    companion object {
        /** The most blocks held; the oldest go first. */
        const val KEEP = 400

        /** What [old] becomes with [e]: all of it when the PC says reset, else the blocks that came put in by their number. */
        fun merge(old: ChatState?, e: PaneEvent.Chat): ChatState {
            val title = e.title ?: old?.title
            val name = e.name ?: old?.name
            if (e.reset || old == null) return ChatState(e.known, name, title, e.blocks.sortedBy { it.i }.takeLast(KEEP))
            val byNumber = old.blocks.associateByTo(java.util.TreeMap()) { it.i }
            for (b in e.blocks) byNumber[b.i] = b
            return ChatState(old.known, name, title, byNumber.values.toList().takeLast(KEEP))
        }
    }
}

/** What a shell says of itself, as the PC sent it: whether it is one jw marked, whether a command runs, and the line typed at its prompt (REQ-92). */
class ShellState(val known: Boolean, val running: Boolean, val input: String?) {
    companion object {
        /** What [old] becomes with [e]: the line stays when the message has none (it is not news). */
        fun merge(old: ShellState?, e: PaneEvent.Shell) = ShellState(e.known, e.running, e.input ?: old?.input)
    }
}
