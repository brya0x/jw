package dev.jw.proto

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import java.security.MessageDigest

enum class FileStatus { Added, Deleted, Modified, Renamed }

enum class LineKind { Ctx, Add, Del }

class DiffLine(val kind: LineKind, val text: String, val old: Int?, val new: Int?)

class DiffHunk(val context: String, val oldStart: Int, val newStart: Int, val lines: List<DiffLine>)

/**
 * One changed file of a `diff` answer (REQ-13). `added` and `deleted` count the lines before any cut;
 * [unreadable] says why the content is not here. `toString` has counts only: the text is code.
 */
class DiffFile(
    val path: String,
    val oldPath: String?,
    val status: FileStatus,
    val binary: Boolean,
    val added: Int,
    val deleted: Int,
    val truncated: Boolean,
    val unreadable: String?,
    val hunks: List<DiffHunk>,
) {
    /**
     * Changes whenever anything the phone shows of the file changes, so a "viewed" mark made against it stops
     * applying once the file is edited (REQ-25), even when the counts stay the same.
     */
    val key: String by lazy {
        val md = MessageDigest.getInstance("SHA-256")
        fun add(s: String?) { md.update((s ?: "\u0000").toByteArray()); md.update(0) }
        add(path); add(oldPath); add(status.name); add(binary.toString()); add(unreadable); add(truncated.toString())
        for (h in hunks) {
            add(h.context)
            for (l in h.lines) { md.update(l.kind.ordinal.toByte()); add(l.text) }
        }
        md.digest().joinToString("") { "%02x".format(it) }.take(24)
    }

    override fun toString() = "DiffFile(status=$status, +$added -$deleted)"
}

/** The answer to `diff{ws}`: the workspace it is for, the server's title (`<name> vs <base>`) and the files, by path. */
class Diff(val ws: String, val title: String, val truncated: Boolean, val files: List<DiffFile>) {
    override fun toString() = "Diff(files=${files.size}, truncated=$truncated)"

    companion object {
        /** Null when `obj` is not a diff answer: a missing `ws` or `files`. Unknown statuses and kinds degrade, not fail. */
        fun parse(obj: JsonObject): Diff? {
            val ws = obj.str("ws") ?: return null
            val files = (obj["files"] as? JsonArray) ?: return null
            return Diff(
                ws,
                obj.str("title") ?: "",
                obj.bool("truncated"),
                files.mapNotNull { (it as? JsonObject)?.let(::file) },
            )
        }

        private fun file(o: JsonObject): DiffFile? {
            val path = o.str("path") ?: return null
            return DiffFile(
                path = path,
                oldPath = o.str("old_path"),
                status = when (o.str("status")) {
                    "added" -> FileStatus.Added
                    "deleted" -> FileStatus.Deleted
                    "renamed" -> FileStatus.Renamed
                    else -> FileStatus.Modified
                },
                binary = o.bool("binary"),
                added = o.int("added") ?: 0,
                deleted = o.int("deleted") ?: 0,
                truncated = o.bool("truncated"),
                unreadable = o.str("unreadable"),
                hunks = (o["hunks"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let(::hunk) } ?: emptyList(),
            )
        }

        private fun hunk(o: JsonObject) = DiffHunk(
            o.str("context") ?: "",
            o.int("old_start") ?: 0,
            o.int("new_start") ?: 0,
            (o["lines"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let(::line) } ?: emptyList(),
        )

        private fun line(o: JsonObject) = DiffLine(
            when (o.str("k")) {
                "add" -> LineKind.Add
                "del" -> LineKind.Del
                else -> LineKind.Ctx
            },
            o.str("text") ?: "",
            o.int("old"),
            o.int("new"),
        )

        private fun JsonObject.str(k: String) = (this[k] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
        private fun JsonObject.int(k: String) = (this[k] as? JsonPrimitive)?.intOrNull
        private fun JsonObject.bool(k: String) = (this[k] as? JsonPrimitive)?.booleanOrNull ?: false
    }
}

