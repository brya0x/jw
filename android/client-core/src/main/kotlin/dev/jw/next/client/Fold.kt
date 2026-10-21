package dev.jw.next.client

import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.ToolState

/*
 * The tools of a conversation folded into one row for each run, as Claude Code draws them (spec
 * REQ-558 and REQ-559): «Buscó 1 patrón, ejecutó 3 comandos». Pure: which items are a run and what
 * its row says. The terminal client has the same rule in `fold.rs`; no fixture shares the words.
 */

/** What a tool does, for the words of the row; the order they are declared in is the order a row says them. */
enum class ToolClass(val verb: String, val one: String, val many: String) {
    Search("Buscó", "patrón", "patrones"),
    Read("Leyó", "archivo", "archivos"),
    List("Listó", "carpeta", "carpetas"),
    Shell("Ejecutó", "comando", "comandos"),
}

/** The class of a tool by its name; none for one that is not folded (an edit has its diff, a subagent its card). */
fun toolClass(name: String): ToolClass? = when (name) {
    "Bash" -> ToolClass.Shell
    "Grep", "Glob" -> ToolClass.Search
    "Read" -> ToolClass.Read
    "LS" -> ToolClass.List
    else -> null
}

/** The class of an item that folds: a tool of a class that has ended (one that runs, or waits for an answer, is drawn by itself). */
private fun ConvItem.foldClass(): ToolClass? =
    if (this is ConvItem.Tool && state != ToolState.Run && state != ToolState.Ask) toolClass(name) else null

/** A run of tools that fold together: the indexes of its first and last item, and what it says. */
data class FoldRun(val first: Int, val last: Int, val title: String)

/** The runs of [items]: each stretch of folding tools, even when it is one tool. */
fun foldRuns(items: List<ConvItem>): List<FoldRun> {
    val out = mutableListOf<FoldRun>()
    var i = 0
    while (i < items.size) {
        if (items[i].foldClass() == null) {
            i++
            continue
        }
        val first = i
        while (i < items.size && items[i].foldClass() != null) i++
        out += FoldRun(first, i - 1, foldTitle(items.subList(first, i)))
    }
    return out
}

/** What a run says: its classes in a fixed order, the first with a capital. */
fun foldTitle(run: List<ConvItem>): String =
    ToolClass.entries.mapNotNull { c ->
        val n = run.count { it.foldClass() == c }
        if (n == 0) null else "${c.verb} $n ${if (n == 1) c.one else c.many}"
    }.mapIndexed { k, part -> if (k == 0) part else part.replaceFirstChar { it.lowercaseChar() } }.joinToString(", ")

/** A row of the list: an item as it is, or the row of a run. */
sealed interface FoldRow {
    data class Item(val index: Int) : FoldRow
    data class Run(val run: FoldRun, val open: Boolean) : FoldRow
}

/** The rows of [items]: a run is its row, and after it the items of the runs in [unfolded] (the index of their first item). */
fun foldRows(items: List<ConvItem>, unfolded: Set<Int>): List<FoldRow> {
    val runs = foldRuns(items).associateBy { it.first }
    val out = mutableListOf<FoldRow>()
    var i = 0
    while (i < items.size) {
        val run = runs[i]
        if (run == null) {
            out += FoldRow.Item(i)
            i++
            continue
        }
        val open = run.first in unfolded
        out += FoldRow.Run(run, open)
        if (open) for (k in run.first..run.last) out += FoldRow.Item(k)
        i = run.last + 1
    }
    return out
}
