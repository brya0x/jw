package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.WorktreeInfo
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-533 to REQ-535 and REQ-539: what a row of the worktrees that exist says, and opening several. */
class WorktreesTest {
    private val open = WorktreeInfo("/c/app-wt/login", "feat/login", "4f2a9c1", workspace = "w1")
    private val changed = WorktreeInfo("/c/app-wt/fix", "fix/login", "9a8b7c6", changes = 3)
    private val clean = WorktreeInfo("/c/app-wt/clean", "clean", "1d2e3f4", changes = 0)
    private val detached = WorktreeInfo("/c/app-wt/bisect", null, "1d2e3f4", changes = 0)
    private val gone = WorktreeInfo("/c/app-wt/old", "old", "aa00bb1", missing = true)

    @Test
    fun aRowSaysItsBranchOrItsCommitAndWhatIsWorthKnowing() {
        assertEquals(listOf("feat/login", "fix/login", "clean", "(1d2e3f4)", "old"), listOf(open, changed, clean, detached, gone).map { it.shown() })
        assertEquals(listOf("abierto", "3 cambios", "", "sin rama", "carpeta no existe"), listOf(open, changed, clean, detached, gone).map { it.note() })
    }

    @Test
    fun onlyAWorktreeWithItsFolderAndNoWorkspaceCanBeOpened() {
        assertEquals(listOf(false, true, true, true, false), listOf(open, changed, clean, detached, gone).map { it.openable() })
    }

    @Test
    fun whatOpeningSaidIsTheTerminalClientsLine() {
        assertEquals("abierto, sin setup ni copiar archivos" to true, OpenedWorktrees(1, emptyList()).said())
        assertEquals("abiertos 3, sin setup ni copiar archivos" to true, OpenedWorktrees(3, emptyList()).said())
        assertEquals("abiertos 2; 1 no: ya está abierto" to false, OpenedWorktrees(2, listOf("ya está abierto")).said())
        assertEquals("git falló" to false, OpenedWorktrees(0, listOf("git falló")).said())
        assertEquals("no hay más cerrados" to false, OpenedWorktrees(0, emptyList()).said())
    }

    /** A source whose PC opens `a` and `c` and refuses `b`. */
    private class Pc : NodeSource {
        val asked = mutableListOf<String>()
        override val epoch = "e"
        override fun session() = error("not used")
        override fun node(id: Id) = null
        override fun openWorktree(repo: String, path: String, reply: (Result<Id>) -> Unit) {
            asked += path
            reply(if (path == "b") Result.failure(Exception("ya está abierto")) else Result.success("w-$path"))
        }
    }

    @Test
    fun severalAreOpenedOneAfterTheOtherAndTheFailuresAreKept() {
        val pc = Pc()
        var result: OpenedWorktrees? = null
        pc.openWorktrees("/c/app", listOf("a", "b", "c")) { result = it }
        assertEquals(listOf("a", "b", "c"), pc.asked, "in order, and a refusal does not stop the rest")
        assertEquals(OpenedWorktrees(2, listOf("ya está abierto")), result)
        var none: OpenedWorktrees? = null
        pc.openWorktrees("/c/app", emptyList()) { none = it }
        assertEquals(OpenedWorktrees(0, emptyList()), none)
    }
}
