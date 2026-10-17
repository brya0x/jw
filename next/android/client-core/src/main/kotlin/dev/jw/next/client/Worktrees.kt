package dev.jw.next.client

import dev.jw.next.contract.WorktreeInfo

/** The worktrees that exist (REQ-533 to REQ-535, REQ-539): what a row says and what opening said. */

/** Its folder is there and no workspace has it. */
fun WorktreeInfo.openable(): Boolean = !missing && workspace == null

/** The name in a list: its branch, or its commit when the head is detached. */
fun WorktreeInfo.shown(): String = branch ?: "($head)"

/** What is worth saying of it beside its name: open, gone, without a branch or with changes. */
fun WorktreeInfo.note(): String = when {
    workspace != null -> "abierto"
    missing -> "carpeta no existe"
    branch == null -> "sin rama"
    (changes ?: 0) > 0 -> "$changes cambios"
    else -> ""
}

/** What opening some worktrees did: how many opened and the PC's reason for each that did not. */
data class OpenedWorktrees(val opened: Int, val failed: List<String>) {
    /** The line the sheet shows, as the terminal client says it, and whether it is good news. */
    fun said(): Pair<String, Boolean> = when {
        failed.isNotEmpty() && opened == 0 -> failed.first() to false
        failed.isNotEmpty() -> "abiertos $opened; ${failed.size} no: ${failed.first()}" to false
        opened == 0 -> "no hay más cerrados" to false
        opened == 1 -> "abierto, sin setup ni copiar archivos" to true
        else -> "abiertos $opened, sin setup ni copiar archivos" to true
    }
}
