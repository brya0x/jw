package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceKind
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-219 to REQ-221: Home's list grouped as the daemon sends it. */
class HomeListTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val worktree = session.workspaces[0]
    private val notes = session.workspaces[1]

    // What is above the section of the conversations of their own, which has its own tests (ChatsHomeTest).
    private fun shape(s: Node.Session) = homeItems(s, Seen()).takeWhile { it !is HomeItem.Chats }.map {
        when (it) {
            is HomeItem.Heading -> "# ${it.text}"
            is HomeItem.Row -> "${it.row.kind}:${it.row.id.takeLast(2)}"
            // the groups have their own tests, GroupsHomeTest
            is HomeItem.Group, is HomeItem.Empty, HomeItem.Ungrouped -> error("no groups here: $it")
            is HomeItem.Chats, is HomeItem.Chat, HomeItem.NoChats, is HomeItem.ArchivedLink -> error("cut above: $it")
        }
    }

    @Test
    fun aRepositorysRootIsFollowedByItsWorktreesAndNeedsNoHeading() {
        val root = notes.copy(id = "r1", name = "app", path = "/home/u/code/app", project = "/home/u/code/app")
        val grouped = session.copy(workspaces = listOf(root, worktree, notes))
        assertEquals(listOf("Root:r1", "Worktree:b1", "# carpetas", "Folder:b2"), shape(grouped))
        assertEquals("raíz · 1 panel", homeRows(grouped, Seen())[0].detail)
    }

    @Test
    fun aWorktreeWhoseRootIsNotOpenIsUnderTheRepositorysNameAndFoldersUnderCarpetas() {
        assertEquals(listOf("# app", "Worktree:b1", "# carpetas", "Folder:b2"), shape(session))
    }

    @Test
    fun foldersAloneHaveNoHeadingAndAnEmptyListIsEmpty() {
        assertEquals(listOf("Folder:b2"), shape(session.copy(workspaces = listOf(notes))))
        assertEquals(emptyList(), shape(session.copy(workspaces = emptyList())))
    }

    @Test
    fun aRowCarriesItsNumberAndTheNameWithoutTheArrow() {
        val rows = homeRows(session, Seen())
        assertEquals(WorkspaceKind.Worktree, worktree.kind)
        assertEquals(listOf(1, 2), rows.map { it.number })
        assertEquals("feat/login", rows[0].name)
        assertEquals("notes", rows[1].name)
    }

    @Test
    fun aDaemonThatSendsNoProjectForARootIsUnderstoodByItsPath() {
        val root = notes.copy(id = "r1", name = "loom", path = "/home/u/code/app", project = null)
        val old = session.copy(workspaces = listOf(root, worktree))
        assertEquals(listOf("Root:r1", "Worktree:b1"), shape(old))
    }
}
