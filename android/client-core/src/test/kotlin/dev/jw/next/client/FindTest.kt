package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-231 to REQ-233: the search over the workspaces of the session. */
class FindTest {
    private val base = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val worktree = base.workspaces[0]
    private val notes = base.workspaces[1]
    private val root = notes.copy(id = "r1", name = "app", path = "/home/u/code/app", project = "/home/u/code/app")
    private val session = base.copy(workspaces = listOf(root, worktree, notes))

    private fun names(recent: List<String>, q: String) = findWorkspaces(session, Seen(), recent, q).map { it.row.name }

    @Test
    fun withNoTextEveryWorkspaceComesInTheOrderOfTheList() {
        assertEquals(listOf("app", "app/feat/login", "notes"), names(emptyList(), ""))
        assertEquals(listOf("app", "app/feat/login", "notes"), names(emptyList(), "   "))
    }

    @Test
    fun theOnesOpenedOnThePhoneComeFirstTheLatestFirst() {
        assertEquals(listOf("notes", "app/feat/login", "app"), names(listOf(notes.id, worktree.id), ""))
        // An id the session no longer has is ignored.
        assertEquals(listOf("app/feat/login", "app", "notes"), names(listOf("gone", worktree.id), ""))
    }

    @Test
    fun theTextMatchesTheNameTheBranchTheRepositoryAndThePathWithoutRegardToCase() {
        assertEquals(listOf("notes"), names(emptyList(), "NOTE"))
        assertEquals(listOf("app/feat/login"), names(emptyList(), "feat/log"))
        assertEquals(listOf("app", "app/feat/login"), names(emptyList(), "code/app"))
        assertEquals(listOf("app", "app/feat/login"), names(emptyList(), "APP"))
        assertEquals(emptyList(), names(emptyList(), "zzz"))
    }

    @Test
    fun aRowCarriesItsKindItsPathAndItsId() {
        val found = findWorkspaces(session, Seen(), emptyList(), "login").single()
        assertEquals(HomeKind.Worktree, found.row.kind)
        assertEquals(worktree.id, found.row.id)
        assertEquals("/home/u/code/app-login", found.path)
    }

    @Test
    fun openingAWorkspaceRemembersItLatestFirstOnceAndNotForever() {
        val source = object : NodeSource {
            override val epoch = session.epoch
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        val nav = Navigator(source)
        var s = ClientState()
        for (id in listOf(root.id, worktree.id, root.id)) s = nav.openWorkspace(s, id)
        assertEquals(listOf(root.id, worktree.id), s.recent)
        for (i in 0 until 40) s = nav.openWorkspace(s, "w$i")
        assertEquals(30, s.recent.size)
        assertEquals("w39", s.recent.first())
    }
}
