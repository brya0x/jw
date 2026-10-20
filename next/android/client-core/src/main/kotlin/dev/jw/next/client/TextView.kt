package dev.jw.next.client

import dev.jw.next.contract.TextDoc

/*
 * What the phone's viewer of text files (spec: *The file viewer*, REQ-618, REQ-619, REQ-622)
 * decides without a screen: the accent of a syntax class, how a line reads, what is said under the
 * title, and when the PC is taken not to answer.
 */

/** The kinds of token the PC marks in a file (REQ-621); each has an accent of the theme (REQ-622). */
enum class Syntax { Keyword, Str, Number, Type, Function, Comment, Attribute }

/** The syntax of a run's class `k`, or none for a missing class or one this phone does not know: it is drawn as plain text (REQ-622). */
fun syntaxOf(k: String?): Syntax? = when (k) {
    "kw" -> Syntax.Keyword
    "str" -> Syntax.Str
    "num" -> Syntax.Number
    "ty" -> Syntax.Type
    "fn" -> Syntax.Function
    "cm" -> Syntax.Comment
    "attr" -> Syntax.Attribute
    else -> null
}

/** A tab is four columns (REQ-614). */
fun expandTabs(text: String): String = text.replace("\t", "    ")

/** The characters of the longest line once its tabs are spaces; at least one, so a block as wide as the file can be made. */
fun widestLine(doc: TextDoc): Int = maxOf(1, doc.lines.maxOfOrNull { line -> line.sumOf { expandTabs(it.t).length } } ?: 0)

/** The line under the title: how many lines the file has, what it weighs and whether it was cut. */
fun textSub(doc: TextDoc): String =
    listOfNotNull("${doc.lines.size} líneas", formatSize(doc.size), if (doc.truncated) "cortado" else null).joinToString(" · ")

/** What the PC refused to read because it is not text (REQ-613): [note] is what the screen says. */
class BinaryFile(val note: String) : Exception(note)

/** What the screen says of a file the PC will not show as text, with its weight when the listing gave it. */
fun binaryNote(size: Long?): String = listOfNotNull("binario", size?.let(::formatSize), "no se puede mostrar").joinToString(" · ")

/** How long `read_file` is waited for before the PC is taken not to answer (REQ-619). */
const val FILE_WAIT_MS = 10_000L

/** What is said when `read_file` is not answered: an older PC does not know the command. */
const val NO_FILE_ANSWER = "el PC no responde: quizá haga falta actualizarlo"
