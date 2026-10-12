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

/** Which changes of a workspace: the TUI's three views (REQ-13). `word` is what goes on the wire and comes back echoed. */
sealed interface DiffScope {
    val word: String

    data object General : DiffScope { override val word = "general" }

    data object Uncommitted : DiffScope { override val word = "uncommitted" }

    /** One commit by its hex id, as the `commits` answer gave it. */
    data class Commit(val sha: String) : DiffScope { override val word get() = "commit:$sha" }

    companion object {
        fun parse(word: String?): DiffScope? = when {
            word == null || word == "general" -> General
            word == "uncommitted" -> Uncommitted
            word.startsWith("commit:") -> Commit(word.removePrefix("commit:"))
            else -> null
        }
    }
}

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
    /** Not tracked by git: the `?` of the Uncommitted view. */
    val untracked: Boolean,
    val hunks: List<DiffHunk>,
) {
    /**
     * Changes whenever anything the phone shows of the file changes, so a "viewed" mark made against it stops
     * applying once the file is edited (REQ-25), even when the counts stay the same.
     */
    val key: String by lazy {
        val md = MessageDigest.getInstance("SHA-256")
        fun add(s: String?) { md.update((s ?: "\u0000").toByteArray()); md.update(0) }
        add(path); add(oldPath); add(status.name); add(untracked.toString()); add(binary.toString()); add(unreadable); add(truncated.toString())
        for (h in hunks) {
            add(h.context)
            for (l in h.lines) { md.update(l.kind.ordinal.toByte()); add(l.text) }
        }
        md.digest().joinToString("") { "%02x".format(it) }.take(24)
    }

    override fun toString() = "DiffFile(status=$status, +$added -$deleted)"
}

/**
 * The answer to `diff{ws, scope}`: the workspace and view it is for, the server's title and the files, by path.
 * [tooLarge] is set when the view passed the server's cap: [files] then only names files (no hunks, no counts) and it
 * says how many there are. [noBase] is General with no default branch on origin (REQ-226).
 */
class Diff(
    val ws: String,
    val scope: DiffScope,
    val title: String,
    val truncated: Boolean,
    val tooLarge: Int?,
    val noBase: Boolean,
    val files: List<DiffFile>,
) {
    override fun toString() = "Diff(files=${files.size}, truncated=$truncated)"

    companion object {
        /** Null when `obj` is not a diff answer: a missing `ws`, `files` or a scope that is not one. Unknown statuses and kinds degrade, not fail. */
        fun parse(obj: JsonObject): Diff? {
            val ws = obj.str("ws") ?: return null
            val files = (obj["files"] as? JsonArray) ?: return null
            return Diff(
                ws,
                DiffScope.parse(obj.str("scope")) ?: return null,
                obj.str("title") ?: "",
                obj.bool("truncated"),
                obj.int("too_large"),
                obj.bool("no_base"),
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
                untracked = o.bool("untracked"),
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


/** A commit of the branch, for the Commits view. `whenSecs` is the committer time in seconds since the epoch. */
class CommitInfo(val sha: String, val short: String, val subject: String, val whenSecs: Long) {
    override fun toString() = "CommitInfo($short)"
}

/** The answer to `commits{ws}`: newest first; [more] says older ones exist; [noBase] says they are `HEAD`'s, not the branch's. */
class CommitList(val ws: String, val commits: List<CommitInfo>, val more: Boolean, val noBase: Boolean) {
    override fun toString() = "CommitList(${commits.size}, more=$more)"

    companion object {
        fun parse(obj: JsonObject): CommitList? {
            val ws = (obj["ws"] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull ?: return null
            val list = (obj["commits"] as? JsonArray) ?: return null
            fun str(o: JsonObject, k: String) = (o[k] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
            return CommitList(
                ws,
                list.mapNotNull { e ->
                    val o = e as? JsonObject ?: return@mapNotNull null
                    CommitInfo(
                        str(o, "sha") ?: return@mapNotNull null,
                        str(o, "short") ?: return@mapNotNull null,
                        str(o, "subject") ?: "",
                        (o["when"] as? JsonPrimitive)?.contentOrNull?.toLongOrNull() ?: 0L,
                    )
                },
                (obj["more"] as? JsonPrimitive)?.booleanOrNull ?: false,
                (obj["no_base"] as? JsonPrimitive)?.booleanOrNull ?: false,
            )
        }
    }
}
