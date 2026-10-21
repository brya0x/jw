package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.Id
import kotlinx.serialization.Serializable

/**
 * What this phone has looked at: node id to the revision it was at when opened. The daemon
 * never sees it (REQ-35). An attention is seen once its `rev` is not above the recorded one.
 * Revisions only mean something inside one daemon epoch, so a different epoch empties it.
 */
@Serializable
data class Seen(val epoch: String? = null, val nodes: Map<Id, Long> = emptyMap()) {
    fun isSeen(a: Attention): Boolean = a.rev <= (nodes[a.node] ?: 0L)

    fun mark(node: Id, rev: Long, epoch: String): Seen {
        val base = forEpoch(epoch)
        return base.copy(nodes = base.nodes + (node to maxOf(rev, base.nodes[node] ?: 0L)))
    }

    /** The record to use under [epoch]: this one if it is the same epoch, else an empty one. */
    fun forEpoch(epoch: String): Seen = if (this.epoch == epoch) this else Seen(epoch)

    fun toJson(): String = ContractJson.encodeToString(serializer(), this)

    companion object {
        /** A damaged or missing record is an empty one: everything shows as unseen, nothing breaks. */
        fun fromJson(text: String?): Seen =
            text?.let { runCatching { ContractJson.decodeFromString(serializer(), it) }.getOrNull() } ?: Seen()
    }
}
