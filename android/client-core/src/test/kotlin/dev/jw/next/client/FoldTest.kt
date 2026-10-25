package dev.jw.next.client

import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.ToolState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** REQ-558: consecutive tools are one row, and what is not a tool of a class that has ended breaks the run. */
class FoldTest {
    private fun tool(name: String, state: ToolState = ToolState.Ok, lost: Boolean = false) = ConvItem.Tool(name, name, "", state, lost = lost)
    private val text = ConvItem.Text("x")

    @Test
    fun aStretchOfFoldingToolsIsOneRunAndAnEditOrATooThatRunsEndsIt() {
        val items = listOf(text, tool("Bash"), tool("Grep"), tool("Bash", ToolState.Failed), tool("Edit"), tool("Read"), tool("Bash", ToolState.Run), tool("Bash"), text)
        assertEquals(
            listOf(
                FoldRun(1, 3, "Buscó 1 patrón, ejecutó 2 comandos"),
                FoldRun(5, 5, "Leyó 1 archivo"),
                FoldRun(7, 7, "Ejecutó 1 comando"),
            ),
            foldRuns(items),
        )
    }

    /** REQ-889: a tool with no result may still run as far as anyone saw: it is not folded with the commands that ended. */
    @Test
    fun aToolWithNoResultIsNotFoldedIntoARun() {
        val items = listOf(tool("Bash"), tool("Bash", ToolState.Failed, lost = true), tool("Bash"))
        assertEquals(listOf(FoldRun(0, 0, "Ejecutó 1 comando"), FoldRun(2, 2, "Ejecutó 1 comando")), foldRuns(items))
    }

    @Test
    fun theClassesAreSaidInAFixedOrderWithThePluralAndOnlyTheFirstHasACapital() {
        val items = listOf(tool("Bash"), tool("Bash"), tool("Bash"), tool("LS"), tool("Read"), tool("Read"), tool("Glob"), tool("Grep"))
        assertEquals("Buscó 2 patrones, leyó 2 archivos, listó 1 carpeta, ejecutó 3 comandos", foldTitle(items))
        assertEquals(emptyList(), foldRuns(emptyList()))
        assertNull(toolClass("Edit"))
        assertNull(toolClass("Agent"))
    }

    @Test
    fun aRunIsItsRowAndItsItemsFollowOnlyWhileItIsOpen() {
        val items = listOf(text, tool("Bash"), tool("Grep"), tool("Edit"))
        val closed = foldRows(items, emptySet())
        assertEquals(listOf(FoldRow.Item(0), FoldRow.Run(FoldRun(1, 2, "Buscó 1 patrón, ejecutó 1 comando"), false), FoldRow.Item(3)), closed)
        val open = foldRows(items, setOf(1))
        assertEquals(listOf(0, -1, 1, 2, 3), open.map { if (it is FoldRow.Item) it.index else -1 })
        assertEquals(true, (open[1] as FoldRow.Run).open)
    }
}
