package dev.jw.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The aviso card: the question and its choices, from the rows of an agent's screen. */
class AskTextTest {
    @Test fun aPermissionDialogGivesItsQuestionAndChoices() {
        val a = AskText.from(
            listOf(
                "● Bash(rm -rf build)",
                "",
                "╭────────────────────────────╮",
                "│ Bash command               │",
                "│   rm -rf build             │",
                "│ Do you want to proceed?    │",
                "│ ❯ 1. Yes                   │",
                "│   2. Yes, and don't ask    │",
                "│   3. No, and tell Claude   │",
                "╰────────────────────────────╯",
                "",
            ),
        )!!
        assertEquals("Bash command\nrm -rf build\nDo you want to proceed?", a.question)
        assertEquals(listOf("1. Yes", "2. Yes, and don't ask", "3. No, and tell Claude"), a.options.map { it.label })
        assertEquals(listOf(true, false, false), a.options.map { it.selected })
    }

    @Test fun aYesNoLineIsTheQuestionWithoutChoices() {
        val a = AskText.from(listOf("compiling…", "Overwrite config.toml? (y/n)", ""))!!
        assertEquals("Overwrite config.toml? (y/n)", a.question)
        assertEquals(0, a.options.size)
    }

    @Test fun aNumberedListInOutputIsNotAQuestion() {
        assertNull(AskText.from(listOf("steps:", "1. install", "", "done")))
        assertNull(AskText.from(listOf("nothing to ask", "")))
    }

    @Test fun theLastQuestionOnTheScreenWins() {
        val a = AskText.from(listOf("Old?", "❯ 1. A", "  2. B", "", "New?", "❯ 1. C", "  2. D"))!!
        assertEquals("New?", a.question)
        assertEquals(listOf("1. C", "2. D"), a.options.map { it.label })
    }
}
