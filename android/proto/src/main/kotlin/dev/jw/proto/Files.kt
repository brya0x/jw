package dev.jw.proto

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.longOrNull

enum class EntryKind { Dir, File, Link }

/** One entry of a directory of the PC (REQ-77). `target` is what a link reaches: `dir`, `file` or `missing`. */
class FsEntry(
    val name: String,
    val kind: EntryKind,
    val size: Long?,
    val mtime: Long?,
    val target: String?,
    val md: Boolean,
) {
    /** Opens as a folder: a folder, or a link to one. */
    val isFolder get() = kind == EntryKind.Dir || (kind == EntryKind.Link && target == "dir")

    override fun toString() = "FsEntry($kind)"
}

/** The answer to `ls{path}`; `req` is the path as the phone asked, `path` the one the PC resolved. */
class FsListing(
    val req: String,
    val path: String,
    val parent: String?,
    val entries: List<FsEntry>,
    val total: Int,
    val truncated: Boolean,
) {
    override fun toString() = "FsListing(${entries.size} of $total)"
}

/** A piece of inline text with its style (REQ-81). */
class MdSpan(val t: String, val b: Boolean, val i: Boolean, val s: Boolean, val code: Boolean, val href: String?) {
    override fun toString() = "MdSpan"
}

sealed interface MdBlock {
    class Heading(val level: Int, val spans: List<MdSpan>) : MdBlock
    class Paragraph(val spans: List<MdSpan>) : MdBlock
    class Code(val lang: String, val text: String) : MdBlock
    class Ul(val items: List<List<MdBlock>>) : MdBlock
    class Ol(val start: Long, val items: List<List<MdBlock>>) : MdBlock
    class Quote(val blocks: List<MdBlock>) : MdBlock
    data object Rule : MdBlock
    class Table(val head: List<List<MdSpan>>, val rows: List<List<List<MdSpan>>>) : MdBlock
}

enum class FileKind { Text, Md, Binary }

/** The answer to `read{path}` (REQ-79): the text (at most 1 MiB; `truncated`), or none for a file that is not text. */
class FsFile(
    val req: String,
    val path: String,
    val kind: FileKind,
    val size: Long,
    val truncated: Boolean,
    val text: String?,
    val blocks: List<MdBlock>?,
) {
    override fun toString() = "FsFile($kind, size=$size)"
}

object Files {
    /** Blocks nest only as deep as the PC allows (16); a deeper one is a malformed answer and is dropped. */
    private const val MAX_DEPTH = 40

    private fun JsonObject.str(k: String) = (this[k] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
    private fun JsonObject.long(k: String) = (this[k] as? JsonPrimitive)?.longOrNull
    private fun JsonObject.bool(k: String) = (this[k] as? JsonPrimitive)?.booleanOrNull ?: false

    fun parseListing(obj: JsonObject): FsListing? {
        val path = obj.str("path") ?: return null
        val entries = (obj["entries"] as? JsonArray) ?: return null
        return FsListing(
            req = obj.str("req") ?: path,
            path = path,
            parent = obj.str("parent"),
            entries = entries.mapNotNull { (it as? JsonObject)?.let(::entry) },
            total = (obj["total"] as? JsonPrimitive)?.intOrNull ?: entries.size,
            truncated = obj.bool("truncated"),
        )
    }

    private fun entry(o: JsonObject): FsEntry? {
        val name = o.str("name") ?: return null
        return FsEntry(
            name = name,
            kind = when (o.str("kind")) {
                "dir" -> EntryKind.Dir
                "link" -> EntryKind.Link
                else -> EntryKind.File
            },
            size = o.long("size"),
            mtime = o.long("mtime"),
            target = o.str("target"),
            md = o.bool("md"),
        )
    }

    fun parseFile(obj: JsonObject): FsFile? {
        val path = obj.str("path") ?: return null
        val kind = when (obj.str("kind")) {
            "md" -> FileKind.Md
            "binary" -> FileKind.Binary
            "text" -> FileKind.Text
            else -> return null
        }
        return FsFile(
            req = obj.str("req") ?: path,
            path = path,
            kind = kind,
            size = obj.long("size") ?: 0L,
            truncated = obj.bool("truncated"),
            text = obj.str("text"),
            blocks = (obj["blocks"] as? JsonArray)?.let { blocks(it, 0) },
        )
    }

    private fun blocks(a: JsonArray, depth: Int): List<MdBlock> =
        if (depth > MAX_DEPTH) emptyList() else a.mapNotNull { (it as? JsonObject)?.let { o -> block(o, depth) } }

    private fun spans(a: JsonElementOrNull): List<MdSpan> = (a as? JsonArray)?.mapNotNull { e ->
        val o = e as? JsonObject ?: return@mapNotNull null
        MdSpan(o.str("t") ?: "", o.bool("b"), o.bool("i"), o.bool("s"), o.bool("code"), o.str("href"))
    }.orEmpty()

    private fun items(a: JsonElementOrNull, depth: Int): List<List<MdBlock>> =
        (a as? JsonArray)?.mapNotNull { (it as? JsonArray)?.let { b -> blocks(b, depth + 1) } }.orEmpty()

    private fun block(o: JsonObject, depth: Int): MdBlock? = when (o.str("k")) {
        "h" -> MdBlock.Heading(((o["level"] as? JsonPrimitive)?.intOrNull ?: 1).coerceIn(1, 6), spans(o["spans"]))
        "p" -> MdBlock.Paragraph(spans(o["spans"]))
        "code" -> MdBlock.Code(o.str("lang") ?: "", o.str("text") ?: "")
        "ul" -> MdBlock.Ul(items(o["items"], depth))
        "ol" -> MdBlock.Ol(o.long("start") ?: 1L, items(o["items"], depth))
        "quote" -> MdBlock.Quote((o["blocks"] as? JsonArray)?.let { blocks(it, depth + 1) }.orEmpty())
        "hr" -> MdBlock.Rule
        "table" -> MdBlock.Table(
            (o["head"] as? JsonArray)?.map { spans(it) }.orEmpty(),
            (o["rows"] as? JsonArray)?.map { r -> (r as? JsonArray)?.map { spans(it) }.orEmpty() }.orEmpty(),
        )
        else -> null
    }
}

private typealias JsonElementOrNull = kotlinx.serialization.json.JsonElement?
