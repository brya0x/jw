@file:OptIn(ExperimentalSerializationApi::class)

package dev.jw.next.contract

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonClassDiscriminator

/*
 * A Markdown document as blocks (spec: *The Markdown viewer*): what `read_markdown` and `read_plan`
 * reply. The daemon reads the Markdown once; the phone draws these blocks and parses nothing.
 */

/** A document: its [title], the file it is when it is one ([path]) and its [blocks]; [truncated] when it was longer than the daemon reads. */
@Serializable
data class MarkdownDoc(
    val title: String,
    val path: String? = null,
    val truncated: Boolean = false,
    val blocks: List<MdBlock>,
)

/** A piece of the document. */
@Serializable
@JsonClassDiscriminator("t")
sealed interface MdBlock {
    @Serializable @SerialName("heading")
    data class Heading(val level: Int, val text: List<Inline>) : MdBlock

    @Serializable @SerialName("para")
    data class Para(val text: List<Inline>) : MdBlock

    @Serializable @SerialName("quote")
    data class Quote(val blocks: List<MdBlock>) : MdBlock

    /** [start] is the number of the first item when the list is numbered. */
    @Serializable @SerialName("list")
    data class ListOf(val start: Int? = null, val items: List<MdItem>) : MdBlock

    @Serializable @SerialName("code")
    data class Code(val lang: String, val text: String) : MdBlock

    @Serializable @SerialName("table")
    data class Table(val head: List<List<Inline>>, val rows: List<List<List<Inline>>>) : MdBlock

    @Serializable @SerialName("rule")
    data object Rule : MdBlock
}

/** An item of a list; [task] is set when it has a checkbox: whether it is done. */
@Serializable
data class MdItem(val task: Boolean? = null, val blocks: List<MdBlock>)

/** A run of text with one style; [href] is where it links to. */
@Serializable
data class Inline(
    val t: String,
    val b: Boolean = false,
    val i: Boolean = false,
    val code: Boolean = false,
    val href: String? = null,
)
