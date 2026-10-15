package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.DirEntry
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.Node
import dev.jw.next.contract.ProjectInfo
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-201 to REQ-211 and REQ-347 to REQ-352: the folder browser and the worktree sheet, against the shared fixtures. */
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
        assertNull(notes.repo(), "a folder that is not a repository has none")
        assertEquals(listOf("/home/u/code/app"), repos(session))
        assertEquals("app", repoName("/home/u/code/app/"))
    }

    @Test
    fun theNextFreeNameSkipsTheFoldersThereAndTheWorkspacesOpenInIt() {
        val there = DirListing("/home/u/code/app-wt", "/home/u/code", "/home/u", listOf(DirEntry("ws-1"), DirEntry("login")), 2)
        assertEquals(setOf("ws-1", "login"), takenNames("/home/u/code/app-wt", there, session))
        assertEquals("ws-2", freeName(setOf("ws-1", "login")))
        assertEquals("ws-1", freeName(emptySet()))
        assertEquals(emptySet(), takenNames("/home/u/code/app-wt", null, session))
        val open = session.copy(workspaces = session.workspaces + worktree.copy(path = "/home/u/code/app-wt/login"))
        assertEquals(setOf("login"), takenNames("/home/u/code/app-wt", null, open))
        assertEquals(setOf("login"), takenNames("/home/u/code/app-wt/", null, open), "a trailing slash of the root changes nothing")
        assertEquals(emptySet(), takenNames("/home/u/code/app-trees", null, open), "the folder of the settings' root, not <repo>-wt")
    }

    private val defaults = ProjectInfo(null, "/home/u/code/app-wt", "feat/{name}", emptyList(), listOf("zsh"))
    private val ready = WorktreeSettings.Ready(defaults)

    @Test
    fun aNameSaysWhatItMakesOrWhyItCannotBeUsed() {
        assertEquals(WorktreePlan.Ok("feat/ws-1", "ws-1", "app-wt/ws-1", defaults), planWorktree(ready, "ws-1", emptySet()))
        for (bad in listOf("", "Mi Rama", "-x", "a/b", "a".repeat(41))) {
            assertIs<WorktreePlan.Wrong>(planWorktree(ready, bad, emptySet()), bad)
        }
        assertEquals(WorktreePlan.Wrong("app-wt/ws-1 ya existe"), planWorktree(ready, "ws-1", setOf("ws-1")))
    }

    @Test
    fun theSheetWaitsForTheSettingsAndShowsTheRefusalInPlaceOfThePlan() {
        assertEquals(WorktreePlan.Reading, planWorktree(WorktreeSettings.Reading, "ws-1", emptySet()))
        assertEquals(WorktreePlan.Reading, planWorktree(WorktreeSettings.Reading, "Mal nombre", emptySet()), "nothing is judged before the answer")
        val why = ".jw.toml, line 2: «{nme}» is no marker (use {name})"
        assertEquals(WorktreePlan.Wrong(why), planWorktree(WorktreeSettings.Refused(why), "ws-1", emptySet()))
    }

    @Test
    fun theBranchTheFolderTheSetupAndThePanesComeFromTheSettings() {
        val info = ProjectInfo("/home/u/code/app/.jw.toml", "/home/u/code/app-trees", "bryan/{name}", listOf("bun install", "cp .env.example .env"), listOf("nvim", "claude"))
        val plan = planWorktree(WorktreeSettings.Ready(info), "ws-3", setOf("ws-1"))
        assertIs<WorktreePlan.Ok>(plan)
        assertEquals("bryan/ws-3", plan.branch)
        assertEquals("ws-3", plan.folder)
        assertEquals("app-trees/ws-3", plan.place)
        assertEquals(listOf("bun install", "cp .env.example .env"), plan.info.setup)
        assertEquals(listOf("nvim", "claude"), plan.info.panes)
        assertEquals(WorktreePlan.Wrong("app-trees/ws-1 ya existe"), planWorktree(WorktreeSettings.Ready(info), "ws-1", setOf("ws-1")))
    }

    /** What the PC was asked, in order, and what each answer was. */
    private class Calls(val start: Result<Unit>, val pane: Result<String> = Result.success("p1")) {
        val asked = mutableListOf<String>()
        fun run(made: Result<String>): Result<String>? {
            var out: Result<String>? = null
            worktreeStarted(
                made,
                { ws, reply -> asked += "start $ws"; reply(start) },
                { ws, reply -> asked += "pane $ws"; reply(pane) },
            ) { out = it }
            return out
        }
    }

    @Test
    fun aWorktreeMadeIsStartedAndOnlyWhenThePcRefusesItGetsOneShell() {
        val ok = Calls(Result.success(Unit))
        assertEquals(Result.success("w1"), ok.run(Result.success("w1")))
        assertEquals(listOf("start w1"), ok.asked, "no new_pane when the panes started")

        val refused = Calls(Result.failure(Exception(".jw.toml, line 3: unclosed array")))
        val failed = refused.run(Result.success("w1"))
        assertEquals(listOf("start w1", "pane w1"), refused.asked)
        assertEquals("se abrió con un shell, pero .jw.toml, line 3: unclosed array", failed?.exceptionOrNull()?.message)

        val neither = Calls(Result.failure(Exception("x")), Result.failure(Exception("y")))
        assertTrue(neither.run(Result.success("w1"))?.exceptionOrNull()?.message.orEmpty().startsWith("se abrió, pero sin paneles ni shell"))

        val none = Calls(Result.success(Unit))
        val git = Result.failure<String>(Exception("fatal: a branch named 'feat/a' already exists"))
        assertEquals(git, none.run(git))
        assertTrue(none.asked.isEmpty(), "nothing was made, so nothing is started")
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
        source.projectConfig("/x") { got += if (it.isFailure) 1 else 0 }
        source.startWorkspace("w") { got += if (it.isFailure) 1 else 0 }
        assertEquals(5, got)
    }
}
