package dev.jw.mobile

import dev.jw.term.Attr
import dev.jw.term.Key
import dev.jw.term.Keys
import dev.jw.term.Line

/** The keys that make the terminal's input go from one text to another, as the phone's input is edited (REQ-93). */
object MirrorTyping {
    /**
     * Typing at the end adds the characters; deleting at the end is a backspace each; any other edit (a paste, an
     * autocorrect, a change in the middle) clears the line with Ctrl-U and types the new text whole.
     */
    fun bytes(old: String, new: String): ByteArray = when {
        old == new -> ByteArray(0)
        new.startsWith(old) -> Keys.text(new.substring(old.length))
        old.startsWith(new) -> {
            val n = old.codePointCount(0, old.length) - new.codePointCount(0, new.length)
            (0 until n).fold(ByteArray(0)) { acc, _ -> acc + Keys.key(Key.Backspace) }
        }
        else -> (Keys.ctrl('u') ?: ByteArray(0)) + Keys.text(new)
    }
}

/** Reads what is typed in the input box of Claude's screen (REQ-93), the one heuristic of the mirror: Claude gives no other way. */
object ClaudeInput {
    private val PROMPT = Regex("""^[>❯›](?: (.*))?$""")
    private val BOX = Regex("""^[\s│┃|╭╮╰╯┌┐└┘─━═╌┄\-]*$""")

    /**
     * The text in the box above the footer, or null when the screen has none (a dialog, a full-screen view). The box
     * is a prompt row (`>` or `❯`) just under a border; the rows below it up to the next border are the rest of a
     * longer text. The placeholder Claude draws in an empty box is dim and is not text.
     */
    fun read(t: PaneText): String? = t.read { p ->
        val rows = (0 until p.rows).map { p.line(it) }
        val full = rows.map { clean(it, skipDim = false) }
        val typed = rows.map { clean(it, skipDim = true) }
        for (r in rows.indices.reversed()) {
            if (r == 0 || !BOX.matches(full[r - 1]) || full[r - 1].isBlank()) continue
            if (!PROMPT.matches(full[r])) continue
            val first = PROMPT.matchEntire(typed[r])?.groupValues?.get(1) ?: ""
            val rest = ArrayList<String>()
            var i = r + 1
            while (i < rows.size && !(BOX.matches(full[i]) && full[i].isNotBlank()) && full[i].isNotBlank()) {
                rest.add(typed[i]); i++
            }
            return@read (listOf(first) + rest).joinToString("\n").trimEnd()
        }
        null
    }

    /** A row as text without the frame Claude draws round its box and, with [skipDim], without its dim cells. */
    private fun clean(l: Line, skipDim: Boolean): String {
        val sb = StringBuilder()
        for (c in 0 until l.cols) {
            val f = l.flags[c]
            if (f and Attr.CONT != 0) continue
            if (skipDim && f and Attr.DIM != 0) { sb.append(' '); continue }
            if (l.ch[c] == 0) sb.append(' ') else sb.appendCodePoint(l.ch[c])
            l.marksAt(c)?.let(sb::append)
        }
        return sb.toString().trim().trim('│', '┃', '|').trim().let { if (it.isEmpty()) "" else it }
    }
}
