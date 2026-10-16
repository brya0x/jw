package dev.jw.next.client

import dev.jw.next.contract.Inline
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.MdBlock

/** A heading of a document: its [level] (1 to 6), its [text] and the top-level [block] it is, which the screen scrolls to. */
data class Heading(val level: Int, val text: String, val block: Int)

/** What a run of text says, without its styles. */
fun plain(text: List<Inline>): String = text.joinToString("") { it.t }

/** The headings of the document, in order, for the sheet that lists them (REQ-481). */
fun outline(doc: MarkdownDoc): List<Heading> =
    doc.blocks.mapIndexedNotNull { i, b -> (b as? MdBlock.Heading)?.let { Heading(it.level.coerceIn(1, 6), plain(it.text), i) } }

/** The line under the title of the viewer: what it is while it waits, how many titles it has and whether it was cut. */
fun viewerSub(doc: MarkdownDoc, plan: Boolean, waits: Boolean): String {
    val n = outline(doc).size
    val titles = when (n) {
        0 -> "sin títulos"
        1 -> "1 título"
        else -> "$n títulos"
    }
    return listOfNotNull(if (plan && waits) "plan listo" else null, titles, if (doc.truncated) "cortado" else null).joinToString(" · ")
}

/** Whether the phone opens [name] in the viewer: a `.md` or a `.markdown`, whatever the case (REQ-479). */
fun isMarkdown(name: String): Boolean = name.substringAfterLast('.', "").lowercase() in setOf("md", "markdown")
