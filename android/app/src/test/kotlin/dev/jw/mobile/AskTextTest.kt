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
        assertEquals(listOf("Yes", "Yes, and don't ask", "No, and tell Claude"), a.options.map { it.label })
        assertEquals(listOf(1, 2, 3), a.options.map { it.number })
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
        assertEquals(listOf("C", "D"), a.options.map { it.label })
    }

    @Test fun descriptionsUnderTheChoicesBelongToThem() {
        val a = AskText.from(
            listOf(
                "How should the blocks be drawn?",
                "",
                "❯ 1. One terminal per block",
                "     Each command keeps its bytes.",
                "  2. Plain text",
                "     Only the screen's lines.",
                "  3. Type something.",
                "",
                "Enter to select · ↑/↓ to navigate · Esc to cancel",
            ),
        )!!
        assertEquals("How should the blocks be drawn?", a.question)
        assertEquals(listOf("One terminal per block", "Plain text", "Type something."), a.options.map { it.label })
        assertEquals(listOf("Each command keeps its bytes.", "Only the screen's lines.", ""), a.options.map { it.description })
        assertEquals(3, a.other?.number)
        assertEquals(false, a.multi)
    }

    @Test fun checkBoxesMakeItAQuestionOfSeveralChoices() {
        val a = AskText.from(listOf("What should it cover?", "❯ 1. [ ] Daemon", "  2. [✔] App", "  3. [x] Spec", "  4. [ ] Tests"))!!
        assertEquals(true, a.multi)
        assertEquals(listOf(false, true, true, false), a.options.map { it.checked })
        assertEquals(listOf("Daemon", "App", "Spec", "Tests"), a.options.map { it.label })
    }

    @Test fun aPlanPromptWhileTheAgentIsIdleIsAPromptAndAListInOutputIsNot() {
        val plan = AskText.from(
            listOf(
                "─────────────────────────",
                " Claude has written up a plan and is ready to execute. Would you like to proceed?",
                "",
                " ❯ 1. Yes, and use auto mode",
                "   2. Yes, manually approve edits",
                "   3. Tell Claude what to change",
                "      shift+tab to approve with this feedback",
                "",
                " ctrl+g to edit in VS Code · ~/.claude/plans/valiant-nibbling-owl.md",
            ),
        )!!
        assertEquals(true, plan.isPrompt)
        assertEquals(listOf("Yes, and use auto mode", "Yes, manually approve edits", "Tell Claude what to change"), plan.options.map { it.label })
        assertEquals("", plan.options[2].description)
        // A numbered list in what Claude wrote has no cursor on it.
        val list = AskText.from(listOf("Steps:", "1. install", "2. build", "3. run", "", "done"))
        assertEquals(false, list?.isPrompt ?: false)
    }
}
