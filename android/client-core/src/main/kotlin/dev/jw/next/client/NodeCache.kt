package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.PatchException
import dev.jw.next.contract.applyPatch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** What the cache needs from its owner after a message. */
sealed interface CacheResult {
    data object Applied : CacheResult

    /** The patch did not fit what is cached: drop it and open the node again for a snapshot. */
    data class Reopen(val node: Id) : CacheResult
}

/**
 * The nodes a client has opened, kept as documents so patches apply to them. Nodes are
 * cached by (epoch, id): a daemon that restarted is a different epoch and shares nothing.
 */
class NodeCache {
    var epoch: String? = null
        private set
    private val docs = mutableMapOf<Id, JsonObject>()

    fun setEpoch(epoch: String) {
        if (this.epoch != epoch) {
            docs.clear()
            this.epoch = epoch
        }
    }

    fun node(id: Id): Node? =
        docs[id]?.let { ContractJson.decodeFromJsonElement(Node.serializer(), it) }

    fun rev(id: Id): Long? = (docs[id]?.get("rev") as? JsonPrimitive)?.content?.toLongOrNull()

    fun apply(msg: DaemonMsg): CacheResult {
        when (msg) {
            is DaemonMsg.Hello -> setEpoch(msg.epoch)
            is DaemonMsg.Snapshot -> {
                docs[msg.node.id] = ContractJson.encodeToJsonElement(Node.serializer(), msg.node) as JsonObject
            }
            is DaemonMsg.Patch -> {
                val doc = docs[msg.node]
                if (doc == null || epoch != msg.epoch || rev(msg.node) != msg.from) return reopen(msg.node)
                try {
                    docs[msg.node] = applyPatch(doc, msg.ops, msg.to)
                } catch (_: PatchException) {
                    return reopen(msg.node)
                }
            }
            is DaemonMsg.Gone -> docs.remove(msg.node)
            is DaemonMsg.Reply, is DaemonMsg.Error -> {}
        }
        return CacheResult.Applied
    }

    private fun reopen(node: Id): CacheResult {
        docs.remove(node)
        return CacheResult.Reopen(node)
    }
}
