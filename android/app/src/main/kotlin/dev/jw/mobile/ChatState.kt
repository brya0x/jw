package dev.jw.mobile

import dev.jw.proto.ChatBlock
import dev.jw.proto.PaneEvent

/** The conversation of an agent as the PC sent it: its title and its blocks, oldest first (REQ-90). */
class ChatState(val known: Boolean, val title: String?, val blocks: List<ChatBlock>) {
    companion object {
        /** The most blocks held; the oldest go first. */
        const val KEEP = 400

        /** What [old] becomes with [e]: all of it when the PC says reset, else the blocks that came put in by their number. */
        fun merge(old: ChatState?, e: PaneEvent.Chat): ChatState {
            val title = e.title ?: old?.title
            if (e.reset || old == null) return ChatState(e.known, title, e.blocks.sortedBy { it.i }.takeLast(KEEP))
            val byNumber = old.blocks.associateByTo(java.util.TreeMap()) { it.i }
            for (b in e.blocks) byNumber[b.i] = b
            return ChatState(old.known, title, byNumber.values.toList().takeLast(KEEP))
        }
    }
}
