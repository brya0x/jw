package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.TextDoc
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class TextViewTest {
    private fun fixture(): TextDoc {
        val dir = File(System.getProperty("contract.dir"), "fixtures/daemon/reply-ok-read-file.json")
        val reply = ContractJson.decodeFromString(DaemonMsg.serializer(), dir.readText()) as DaemonMsg.Reply
        return ContractJson.decodeFromJsonElement(TextDoc.serializer(), reply.result!!)
    }

    /** REQ-622: each class of the PC has a syntax, and a missing or unknown one is plain. */
    @Test
    fun everyClassOfThePcHasASyntaxAndAnUnknownOneIsPlain() {
        assertEquals(
            listOf(Syntax.Keyword, Syntax.Str, Syntax.Number, Syntax.Type, Syntax.Function, Syntax.Comment, Syntax.Attribute),
            listOf("kw", "str", "num", "ty", "fn", "cm", "attr").map { syntaxOf(it) },
        )
        assertNull(syntaxOf(null))
        assertNull(syntaxOf("from-the-future"))
    }

    /** REQ-614: a tab is four columns. */
    @Test
    fun aTabIsFourColumns() {
        assertEquals("    a", expandTabs("\ta"))
    }

    /** REQ-618: under the title, the lines, the weight and that it was cut. */
    @Test
    fun theLineUnderTheTitleSaysTheLinesTheWeightAndTheCut() {
        val doc = fixture()
        assertEquals("5 líneas · 214 B · cortado", textSub(doc))
        assertEquals("5 líneas · 214 B", textSub(doc.copy(truncated = false)))
    }

    /** REQ-613: a file that is not text says so, with its weight when the listing gave it. */
    @Test
    fun aBinaryFileSaysItIsBinaryWithItsWeightWhenKnown() {
        assertEquals("binario · 48 KB · no se puede mostrar", binaryNote(48 * 1024))
        assertEquals("binario · no se puede mostrar", binaryNote(null))
        assertEquals("binario · no se puede mostrar", BinaryFile(binaryNote(null)).message)
    }
}
