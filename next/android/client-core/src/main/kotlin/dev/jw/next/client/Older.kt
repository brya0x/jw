package dev.jw.next.client

import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.Node
import dev.jw.next.contract.OlderItems

/** What the row above the first item says (REQ-569). */
enum class OlderRow(val text: String) {
    Hint("▲ hay anteriores · sube para cargar"),
    Loading("✻ cargando anteriores…"),
    Failed("✗ no se pudo leer el archivo · toca para reintentar"),
    Start("── inicio de la conversación ──"),
    Capped("── más antiguo en el archivo de Claude ──"),
}

/** How many items one request asks for, and the most that are kept. */
const val OLDER_PAGE = 100
private const val OLDER_CAP = 3000

/**
 * The items of a conversation that came before the ones its node holds (REQ-567, REQ-569): the phone asks for them when the list
 * reaches its oldest end and keeps them beside the node's window. The list is the older items and then the node's, and the number of
 * the first is [first]. When the window slides, what leaves its head goes to the end of the older items, so the two stay in a row and
 * the number of an item does not change; another epoch, or a window that does not join them, drops them.
 */
class OlderHeld {
    private var older: List<ConvItem> = emptyList()
    private var epoch = ""

    /** The `first` the node says when its window starts right after the older items. */
    private var anchor = 0
    private var window: List<ConvItem> = emptyList()

    /** `null` until something was loaded: the node says where to ask from. Then the last answer says; `exhausted` when there is none. */
    private var next: Long? = null
    private var loaded = false
    private var exhausted = false
    private var busy: Long? = null
    private var failed = false

    /** The older items, oldest first. */
    val items: List<ConvItem> get() = older

    /** The number of the first item of [items] and then [node]'s: negative when the conversation had fewer before the window than were loaded. */
    @Synchronized fun first(node: Node.Conversation): Int = node.first - older.size

    /** The list to draw: the older items and the node's. */
    @Synchronized fun all(node: Node.Conversation): List<ConvItem> = if (older.isEmpty()) node.items else older + node.items

    /** Take what the node says now. Calling it again with the same node changes nothing. */
    @Synchronized fun sync(node: Node.Conversation) {
        if (epoch != node.epoch) {
            epoch = node.epoch
            reset(node)
        }
        if (older.isNotEmpty()) {
            val d = node.first - anchor
            when {
                d == 0 -> Unit
                d > 0 && d <= window.size && sameItem(node.items.firstOrNull(), window.getOrNull(d)) -> {
                    older = older + window.take(d)
                    anchor = node.first
                }
                else -> reset(node)
            }
        }
        if (older.isEmpty()) anchor = node.first
        window = node.items
    }

    private fun reset(node: Node.Conversation) {
        older = emptyList()
        loaded = false
        exhausted = false
        next = null
        busy = null
        failed = false
        anchor = node.first
    }

    /** The time to ask with now, when the list is at its oldest end, the node has a transcript, nothing is asked and the last try did not fail. */
    @Synchronized fun ask(node: Node.Conversation, atEnd: Boolean): Long? {
        if (!atEnd || busy != null || failed || exhausted || older.size >= OLDER_CAP) return null
        val before = (if (loaded) next else node.older) ?: return null
        busy = before
        return before
    }

    /** The answer to the request with [before]; whether it was taken. One that is not the last request is not. */
    @Synchronized fun replied(before: Long, result: Result<OlderItems>): Boolean {
        if (busy != before) return false
        busy = null
        val page = result.getOrElse {
            failed = true
            return true
        }
        older = page.items + older
        loaded = true
        // The cursor must go back: an answer that does not would be asked for for ever.
        next = page.older?.takeIf { it < before }
        exhausted = next == null
        return true
    }

    /** A tap on the row after a failure asks again. */
    @Synchronized fun retry() {
        failed = false
    }

    /** The row above the first item, for a conversation that has a transcript. */
    @Synchronized fun row(node: Node.Conversation): OlderRow? {
        if (node.older == null) return null
        return when {
            busy != null -> OlderRow.Loading
            failed -> OlderRow.Failed
            older.size >= OLDER_CAP -> OlderRow.Capped
            exhausted -> OlderRow.Start
            else -> OlderRow.Hint
        }
    }

    /** Whether two items are the same one, though what is in it may have grown or ended. */
    private fun sameItem(a: ConvItem?, b: ConvItem?): Boolean = when {
        a == null || b == null -> a == null && b == null
        a is ConvItem.Tool && b is ConvItem.Tool -> a.id == b.id
        a is ConvItem.Agent && b is ConvItem.Agent -> a.id == b.id
        else -> a::class == b::class
    }
}
