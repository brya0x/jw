package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.DirEntry
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull

/** REQ-201 to REQ-209: the folder browser and the worktree name, against the shared fixtures. */
class FoldersTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val worktree = session.workspaces[0]
    private val notes = session.workspaces[1]

    private fun listing(path: String, vararg dirs: DirEntry, total: Int = dirs.size) =
        DirListing(path, path.substringBeforeLast('/').ifEmpty { "/" }, "/home/u", dirs.toList(), total)

    @Test
    fun aFolderRowKnowsWhetherItIsOpenAndWhetherItIsARepository() {
        val view = folderView(listing("/home/u", DirEntry("code", true), DirEntry("notes"), DirEntry("music")), session)
        assertEquals(listOf("/home/u/code", "/home/u/notes", "/home/u/music"), view.rows.map { it.path })
        assertEquals(listOf(true, false, false), view.rows.map { it.git })
        assertEquals(listOf(null, notes.id, null), view.rows.map { it.open })
        assertNull(view.here)
        assertEquals(notes.id, folderView(listing("/home/u/notes"), session).here)
        assertEquals(false, view.cut)
    }

    @Test
    fun aListingCutByThePcSaysSo() {
        val view = folderView(listing("/home/u", DirEntry("a"), total = 1214), session)
        assertEquals(true, view.cut)
        assertEquals(1214, view.total)
    }

    @Test
    fun theCrumbsStartAtTheHomeOrTheRoot() {
        assertEquals(listOf("~" to "/home/u", "ws" to "/home/u/ws", "jw" to "/home/u/ws/jw"), folderView(listing("/home/u/ws/jw"), session).crumbs)
        assertEquals(listOf("~" to "/home/u"), folderView(listing("/home/u"), session).crumbs)
        assertEquals(listOf("/" to "/", "etc" to "/etc"), folderView(DirListing("/etc", "/", "/home/u", emptyList(), 0), session).crumbs)
        assertEquals("~/ws", tildePath("/home/u/ws", "/home/u"))
        assertEquals("/opt", tildePath("/opt", "/home/u"))
    }

    @Test
    fun aWorktreeIsMadeOfTheRepositoryAndFromTheBranchOfTheWorkspace() {
        assertEquals("/home/u/code/app", worktree.repo())
        assertEquals("/home/u/notes", notes.repo())
        assertEquals(listOf("/home/u/code/app", "/home/u/notes"), repos(session))
        assertEquals("/home/u/code/app-wt", worktreesDir("/home/u/code/app"))
        assertEquals("app", repoName("/home/u/code/app/"))
    }

    @Test
    fun theNextFreeNameSkipsTheFoldersThereAndTheWorkspacesOpenInIt() {
        val there = DirListing("/home/u/code/app-wt", "/home/u/code", "/home/u", listOf(DirEntry("ws-1"), DirEntry("login")), 2)
        assertEquals(setOf("ws-1", "login"), takenNames("/home/u/code/app", there, session))
        assertEquals("ws-2", freeName(setOf("ws-1", "login")))
        assertEquals("ws-1", freeName(emptySet()))
        assertEquals(emptySet(), takenNames("/home/u/code/app", null, session))
        val open = session.copy(workspaces = session.workspaces + worktree.copy(path = "/home/u/code/app-wt/login"))
        assertEquals(setOf("login"), takenNames("/home/u/code/app", null, open))
    }

    @Test
    fun aNameSaysWhatItMakesOrWhyItCannotBeUsed() {
        val ok = planWorktree("/home/u/code/app", "ws-1", emptySet())
        assertEquals(WorktreePlan.Ok("feat/ws-1", "ws-1"), ok)
        for (bad in listOf("", "Mi Rama", "-x", "a/b", "a".repeat(41))) {
            assertIs<WorktreePlan.Wrong>(planWorktree("/home/u/code/app", bad, emptySet()), bad)
        }
        assertEquals(WorktreePlan.Wrong("app-wt/ws-1 ya existe"), planWorktree("/home/u/code/app", "ws-1", setOf("ws-1")))
    }

    @Test
    fun aWorkspaceJustMadeIsOpenedOnceTheSessionShowsIt() {
        val source = object : NodeSource {
            override val epoch = session.epoch
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        val nav = Navigator(source)
        assertNull(nav.openWhenShown(ClientState(), "not-yet"))
        assertEquals(Screen.Workspace(notes.id, null), nav.openWhenShown(ClientState(), notes.id)?.screen)
    }

    @Test
    fun aSourceWithNoPcAnswersWithItsMessage() {
        val source = object : NodeSource {
            override val epoch = ""
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        var got = 0
        source.listDir(null, false) { got += if (it.isFailure) 1 else 0 }
        source.openFolder("/x") { got += if (it.isFailure) 1 else 0 }
        source.createWorktree("/x", "feat/a", null, "a") { got += if (it.isFailure) 1 else 0 }
        assertEquals(3, got)
    }
}
