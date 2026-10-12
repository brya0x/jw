package dev.jw.mobile

/** One choice of an agent's question, as its screen numbers it, and whether the agent's cursor is on it. */
class AskOption(val label: String, val selected: Boolean)

/** What an agent that waits is asking: the question and its numbered choices, read off its screen. */
class AskText(val question: String, val options: List<AskOption>) {
    companion object {
        private val OPTION = Regex("""^([❯>›➤▶]\s*)?(\d{1,2})[.)]\s+(.+)$""")
        private val CONFIRM = Regex("""(\(y/n\)|\[y/n]|\[Y/n]|\[y/N]|\(Y/n\)|\(y/N\))""")
        private val BOX = Regex("""^[\s│┃|╭╮╰╯┌┐└┘─━═╌┄\-]*$""")
        private const val MAX_QUESTION_LINES = 3

        /** Without the frame an agent draws round its dialog. */
        private fun clean(row: String) = row.trim().trim('│', '┃', '|').trim()

        /**
         * The question on the screen's [rows], or null when none can be told from them: the last run of numbered
         * choices and the lines above it, else the last line that asks `(y/n)`. An agent draws it at the bottom
         * of its screen, so the last one wins.
         */
        fun from(rows: List<String>): AskText? {
            val lines = rows.map(::clean)
            val opt = lines.map { OPTION.matchEntire(it) }
            val end = opt.indexOfLast { it != null }
            if (end < 0) {
                val i = lines.indexOfLast { CONFIRM.containsMatchIn(it) }
                return if (i < 0) null else AskText(lines[i], emptyList())
            }
            var start = end
            while (start > 0 && opt[start - 1] != null) start--
            // The numbered lines are one run; a lone `1.` in some output is not a question.
            if (end == start && !lines[start].let { OPTION.matchEntire(it)?.groupValues?.get(1)?.isNotEmpty() == true }) return null
            val options = (start..end).map { i ->
                val m = opt[i]!!
                AskOption("${m.groupValues[2]}. ${m.groupValues[3]}", m.groupValues[1].isNotEmpty())
            }
            val question = ArrayList<String>()
            var i = start - 1
            while (i >= 0 && question.size < MAX_QUESTION_LINES) {
                val l = lines[i]
                if (BOX.matches(l)) { if (question.isNotEmpty()) break else { i--; continue } }
                question.add(0, l)
                i--
            }
            return AskText(question.joinToString("\n"), options)
        }
    }
}

/** The rows of the screen as text, for [AskText.from]. */
fun PaneText.screenRows(): List<String> = read { t -> (0 until t.rows).map { t.line(it).text() } }
