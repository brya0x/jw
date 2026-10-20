package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Inline
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.MdBlock
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** REQ-479 to REQ-481 against the shared fixture of a `read_markdown` reply. */
class ViewerTest {
    private val doc = (ContractJson.decodeFromString(
        DaemonMsg.serializer(),
        File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon/reply-ok-read-markdown.json").readText(),
    ) as DaemonMsg.Reply).let { ContractJson.decodeFromJsonElement(MarkdownDoc.serializer(), it.result!!) }

    private fun heading(level: Int, text: String) = MdBlock.Heading(level, listOf(Inline(text)))

    @Test
    fun theOutlineListsTheHeadingsWithTheBlockEachIsAndWhatTheyReadWithoutTheirStyles() {
        assertEquals(listOf(Heading(1, "Plan: pruebas del módulo auth", 0)), outline(doc))
        val d = MarkdownDoc(
            "t",
            blocks = listOf(
                heading(1, "Uno"),
                MdBlock.Para(listOf(Inline("texto"))),
                MdBlock.Heading(2, listOf(Inline("Con "), Inline("código", code = true), Inline(" y "), Inline("negrita", b = true))),
                MdBlock.Quote(listOf(heading(2, "dentro de una cita"))),
                heading(9, "nivel de más"),
            ),
        )
        assertEquals(
            listOf(Heading(1, "Uno", 0), Heading(2, "Con código y negrita", 2), Heading(6, "nivel de más", 4)),
            outline(d),
            "a heading inside a quote is not a block of the document; a level past 6 is the 6th",
        )
        assertEquals(emptyList(), outline(MarkdownDoc("t", blocks = emptyList())))
    }

    @Test
    fun aBlankReasonToKeepPlanningGoesWithoutANoteAndAnyOtherIsTrimmed() {
        assertEquals(null, keepPlanningNote(""))
        assertEquals(null, keepPlanningNote(" \n\t "))
        assertEquals("revisa el módulo auth", keepPlanningNote("  revisa el módulo auth \n"))
    }

    @Test
    fun theLineUnderTheTitleSaysWhatItIsHowManyTitlesItHasAndWhetherItWasCut() {
        assertEquals("1 título", viewerSub(doc, plan = false, waits = false))
        assertEquals("plan listo · 1 título", viewerSub(doc, plan = true, waits = true))
        assertEquals("1 título", viewerSub(doc, plan = true, waits = false), "answered, it no longer waits")
        val long = MarkdownDoc("t", truncated = true, blocks = listOf(heading(1, "a"), heading(2, "b")))
        assertEquals("2 títulos · cortado", viewerSub(long, plan = false, waits = false))
        assertEquals("sin títulos", viewerSub(MarkdownDoc("t", blocks = emptyList()), plan = false, waits = false))
    }

    @Test
    fun aMarkdownFileIsOneWhoseExtensionSaysSoInAnyCase() {
        assertTrue(isMarkdown("README.md") && isMarkdown("notas.MD") && isMarkdown("a.b.markdown"))
        assertFalse(isMarkdown("md") || isMarkdown("a.mdx") || isMarkdown("Makefile") || isMarkdown("main.rs") || isMarkdown(".md.bak"))
    }
}
