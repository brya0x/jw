package dev.jw.next.contract

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

class PatchException(message: String) : Exception("cannot apply patch: $message")

/**
 * Apply [ops] in order to a copy of [doc], then set its `rev` to [to]. A patch never edits
 * `rev` itself: the client holds the document at the patch's `from`, applies the ops and
 * moves the revision. Any failure means the cache is unusable: reopen for a snapshot.
 */
fun applyPatch(doc: JsonElement, ops: List<PatchOp>, to: Long): JsonObject {
    var current = doc
    for (op in ops) current = applyOp(current, op)
    val obj = current as? JsonObject ?: throw PatchException("the document is not an object")
    return JsonObject(obj + ("rev" to JsonPrimitive(to)))
}

private fun unescape(key: String) = key.replace("~1", "/").replace("~0", "~")

private fun applyOp(doc: JsonElement, op: PatchOp): JsonElement {
    val path = op.path
    if (path.isEmpty()) {
        return (op as? PatchOp.Replace)?.value ?: throw PatchException("bad path \"$path\"")
    }
    if (!path.startsWith("/")) throw PatchException("bad path \"$path\"")
    val keys = path.substring(1).split("/").map(::unescape)
    return edit(doc, keys, op)
}

/** Walk down [keys]; at the last one apply [op] to the container, rebuilding on the way back. */
private fun edit(node: JsonElement, keys: List<String>, op: PatchOp): JsonElement {
    val key = keys.first()
    val last = keys.size == 1
    return when (node) {
        is JsonObject -> {
            val map = node.toMutableMap()
            when {
                !last -> map[key] = edit(map[key] ?: throw PatchException("no parent at ${op.path}"), keys.drop(1), op)
                op is PatchOp.Add -> map[key] = op.value
                op is PatchOp.Remove -> map.remove(key) ?: throw PatchException("no such key at ${op.path}")
                op is PatchOp.Replace -> {
                    if (key !in map) throw PatchException("no such key at ${op.path}")
                    map[key] = op.value
                }
            }
            JsonObject(map)
        }
        is JsonArray -> {
            val list = node.toMutableList()
            if (last && op is PatchOp.Add && key == "-") {
                list.add(op.value)
                return JsonArray(list)
            }
            val i = key.toIntOrNull()?.takeIf { it >= 0 } ?: throw PatchException("bad index at ${op.path}")
            when {
                !last -> list[i.takeIf { it < list.size } ?: throw PatchException("no parent at ${op.path}")] =
                    edit(list[i], keys.drop(1), op)
                op is PatchOp.Add -> {
                    if (i > list.size) throw PatchException("index past the end at ${op.path}")
                    list.add(i, op.value)
                }
                op is PatchOp.Remove -> {
                    if (i >= list.size) throw PatchException("no such index at ${op.path}")
                    list.removeAt(i)
                }
                op is PatchOp.Replace -> {
                    if (i >= list.size) throw PatchException("no such index at ${op.path}")
                    list[i] = op.value
                }
            }
            JsonArray(list)
        }
        else -> throw PatchException("parent is not a container at ${op.path}")
    }
}
