package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-227 and REQ-228: the sheet of each kind of workspace and the question before a removal. */
class RemoveTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val worktree = session.target(session.workspaces[0])
    private val folder = session.target(session.workspaces[1])

    @Test
    fun eachKindOfWorkspaceHasItsOwnSheet() {
        assertEquals(HomeKind.Worktree, worktree.kind)
        assertEquals(HomeKind.Folder, folder.kind)
        assertEquals(
            listOf(WsAction.Rename, WsAction.ExistingWorktrees, WsAction.Project, WsAction.CloseWorkspace, WsAction.RemoveWorktree),
            workspaceMenu(worktree.kind, worktree.repo).map { it.action },
            "a worktree knows its repository, so it offers the project's settings (REQ-526)",
        )
        assertEquals(listOf(WsAction.Rename, WsAction.CloseWorkspace, WsAction.RemoveWorktree), workspaceMenu(HomeKind.Worktree, null).map { it.action })
        assertEquals(listOf(WsAction.RemoveWorkspace), workspaceMenu(HomeKind.Folder, null).map { it.action })
        // Only a repository's root offers a worktree, and only when it knows the repository.
        assertEquals(
            listOf(WsAction.NewWorktree, WsAction.ExistingWorktrees, WsAction.Project, WsAction.RemoveWorkspace),
            workspaceMenu(HomeKind.Root, "/home/u/code/app").map { it.action },
        )
        assertEquals("de «app»", workspaceMenu(HomeKind.Root, "/home/u/code/app").first().note)
        assertEquals(listOf(WsAction.RemoveWorkspace), workspaceMenu(HomeKind.Root, null).map { it.action })
    }

    @Test
    fun theSheetIsTitledByTheKind() {
        assertEquals("worktree «feat/login»", menuTitle(worktree))
        assertEquals("carpeta «notes»", menuTitle(folder))
        assertEquals("workspace «app»", menuTitle(WorkspaceTarget("r", "app", HomeKind.Root, 1, "/x/app")))
    }

    @Test
    fun everyRemovalIsDangerousAndRenamingAndANewWorktreeAreNot() {
        for (kind in HomeKind.entries) {
            for (item in workspaceMenu(kind, "/x/app")) {
                assertEquals(item.action in setOf(WsAction.RemoveWorkspace, WsAction.CloseWorkspace, WsAction.RemoveWorktree), item.danger, "$kind ${item.action}")
            }
        }
    }

    @Test
    fun theQuestionSaysWhatEachRemovalDoes() {
        val folderQ = removeSheet(folder, WsAction.RemoveWorkspace)!!
        assertEquals("quitar la carpeta «notes»", folderQ.heading)
        assertTrue("La carpeta del PC no se toca." in folderQ.body, folderQ.body)
        assertEquals(false, folderQ.mode.removesWorktree)

        val keep = removeSheet(worktree, WsAction.CloseWorkspace)!!
        assertEquals("cerrar el workspace «feat/login»", keep.heading)
        assertTrue("su rama se quedan en el PC" in keep.body, keep.body)
        assertEquals(false, keep.mode.removesWorktree)

        val gone = removeSheet(worktree, WsAction.RemoveWorktree)!!
        assertEquals("quitar el worktree «feat/login»", gone.heading)
        assertTrue("git worktree remove, sin forzar" in gone.body && "lo rechaza" in gone.body, gone.body)
        assertEquals(true, gone.mode.removesWorktree)

        val root = removeSheet(WorkspaceTarget("r", "app", HomeKind.Root, 3, "/x/app"), WsAction.RemoveWorkspace)!!
        assertEquals("quitar el workspace «app»", root.heading)
        assertTrue(root.body.startsWith("Se cierran sus 3 paneles"), root.body)
        assertTrue("worktrees siguen abiertos aparte" in root.body, root.body)
    }

    @Test
    fun theQuestionCountsThePanes() {
        fun body(n: Int) = removeSheet(WorkspaceTarget("r", "n", HomeKind.Folder, n, null), WsAction.RemoveWorkspace)!!.body
        assertTrue(body(0).startsWith("No tiene paneles abiertos."), body(0))
        assertTrue(body(1).startsWith("Se cierra su panel"), body(1))
    }

    @Test
    fun renamingAndANewWorktreeAreNotRemovals() {
        assertNull(removeSheet(folder, WsAction.Rename))
        assertNull(removeSheet(folder, WsAction.NewWorktree))
    }

    @Test
    fun aSourceWithNoPcAnswersWithItsMessage() {
        val source = object : NodeSource {
            override val epoch = ""
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        var failed = false
        source.removeWorkspace("x", false) { failed = it.isFailure }
        assertTrue(failed)
    }
}
