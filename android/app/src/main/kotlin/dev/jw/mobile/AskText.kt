package dev.jw.mobile

/**
 * One choice of an agent's question, as its screen numbers it: its [number] (what is typed to take it), its [label], the
 * [description] under it, whether the agent's cursor is on it ([selected]) and, in a question of several choices, whether
 * it is marked ([checked], null when the question has no check boxes).
 */
class AskOption(val number: Int, val label: String, val description: String, val selected: Boolean, val checked: Boolean? = null)

/** What an agent that waits is asking: the question and its numbered choices, read off its screen. */
class AskText(val question: String, val options: List<AskOption>) {
    /** Several choices can be marked: the options have check boxes. */
    val multi get() = options.any { it.checked != null }

    /** The option that takes a written answer (`Type something`, `Other`), when the question has one. */
    val other get() = options.lastOrNull { OTHER.containsMatchIn(it.label) }

    companion object {
        private val OPTION = Regex("""^([❯>›➤▶]\s*)?(\d{1,2})[.)]\s+(.+)$""")
        private val CONFIRM = Regex("""(\(y/n\)|\[y/n]|\[Y/n]|\[y/N]|\(Y/n\)|\(y/N\))""")
        private val BOX = Regex("""^[\s│┃|╭╮╰╯┌┐└┘─━═╌┄\-]*$""")
        private val OTHER = Regex("""(?i)(type something|other|chat about this|escribe)""")
        private val HINT = Regex("""(?i)(enter to|esc to|↑/↓|to navigate|to select|to cancel)""")
        private const val MAX_QUESTION_LINES = 3
        private const val MAX_DESCRIPTION_LINES = 3

        /** Without the frame an agent draws round its dialog. */
        private fun clean(row: String) = row.trim().trim('│', '┃', '|').trim()

        /** A check box at the start of an option's text: marked, not marked, or none. */
        private fun checkOf(text: String): Pair<Boolean?, String> {
            val m = Regex("""^(\[[ xX✓✔]?]|\([ xX✓✔]?\)|☐|☑|☒)\s*""").find(text) ?: return null to text
            val mark = m.groupValues[1]
            return (mark.any { it in "xX✓✔☑☒" }) to text.substring(m.value.length)
        }

        /**
         * The question on the screen's [rows], or null when none can be told from them: the last numbered choices (each with the
         * lines under it that describe it) and the lines above them, else the last line that asks `(y/n)`. An agent draws it at
         * the bottom of its screen, so the last one wins.
         */
        fun from(rows: List<String>): AskText? {
            val lines = rows.map(::clean)
            val opt = lines.map { OPTION.matchEntire(it) }
            val last = opt.indexOfLast { it != null }
            if (last < 0) {
                val i = lines.indexOfLast { CONFIRM.containsMatchIn(it) }
                return if (i < 0) null else AskText(lines[i], emptyList())
            }
            // Up from the last choice, each one the number before the one under it, with only its description lines between.
            val at = arrayListOf(last)
            while (opt[at.first()]!!.groupValues[2].toInt() > 1) {
                val want = opt[at.first()]!!.groupValues[2].toInt() - 1
                val j = (at.first() - 1 downTo maxOf(0, at.first() - 1 - MAX_DESCRIPTION_LINES - 1)).firstOrNull { opt[it] != null } ?: return null
                if (opt[j]!!.groupValues[2].toInt() != want) return null
                at.add(0, j)
            }
            val start = at.first()
            // A lone `1.` in some output is not a question: it needs the agent's cursor on it, or a second choice.
            if (at.size == 1 && opt[start]!!.groupValues[1].isEmpty()) return null
            val options = at.mapIndexed { n, line ->
                val m = opt[line]!!
                val (checked, label) = checkOf(m.groupValues[3])
                val below = (line + 1 until (if (n + 1 < at.size) at[n + 1] else minOf(lines.size, line + 1 + MAX_DESCRIPTION_LINES)))
                    .map { lines[it] }
                    .takeWhile { it.isNotEmpty() && !BOX.matches(it) && !HINT.containsMatchIn(it) }
                    .filter { it.isNotEmpty() && !BOX.matches(it) && !HINT.containsMatchIn(it) }
                AskOption(m.groupValues[2].toInt(), label.trim(), below.joinToString(" "), m.groupValues[1].isNotEmpty(), checked)
            }
            val question = ArrayList<String>()
            var q = start - 1
            while (q >= 0 && question.size < MAX_QUESTION_LINES) {
                val l = lines[q]
                if (BOX.matches(l)) { if (question.isNotEmpty()) break else { q--; continue } }
                question.add(0, l)
                q--
            }
            return AskText(question.joinToString("\n"), options)
        }
    }
}

/** The rows of the screen as text, for [AskText.from]. */
fun PaneText.screenRows(): List<String> = read { t -> (0 until t.rows).map { t.line(it).text() } }
