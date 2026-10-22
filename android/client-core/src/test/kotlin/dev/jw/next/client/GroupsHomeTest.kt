package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.AttentionKind
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Group
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * REQ-669 to REQ-678 on the phone: Home's list with groups, what a folded group says, the sheet
 * of a move, the chips of Avisos and the groups that are folded. The cases are those of
 * `crates/jw/src/tui/side.rs`, on `snapshot-session-groups.json`: `salesassist` with `app` and its
 * worktree `feat/login`, `ideas` with nothing, and `notes` in none.
 */
class GroupsHomeTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session-groups.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val app = session.workspaces[0]
    private val login = session.workspaces[1]
    private val notes = session.workspaces[2]
    private val f1 = session.groups[0].id
    private val f2 = session.groups[1].id

    private fun shape(s: Node.Session, folded: Set<String> = emptySet()) = homeItems(s, Seen(), folded).map {
        when (it) {
            is HomeItem.Heading -> "# ${it.text}${if (it.grouped) " (in)" else ""}"
            is HomeItem.Row -> "${it.row.kind}:${it.row.id.takeLast(2)}${if (it.grouped) " (in)" else ""}"
            is HomeItem.Group -> "@${it.id.takeLast(2)} ${if (it.open) "open" else "folded"} ${it.count}"
            is HomeItem.Empty -> "vacío"
            HomeItem.Ungrouped -> "sin grupo"
        }
    }

    private fun attention(kind: AttentionKind, node: String, rev: Long = 3) = Attention(kind, node, rev)

    @Test
    fun theGroupsComeInOrderWithTheirWorkspacesVacioAndSinGrupoLastAndNoCarpetas() {
        assertEquals(
            listOf("@f1 open 2", "Root:b1 (in)", "Worktree:b2 (in)", "@f2 open 0", "vacío", "sin grupo", "Folder:b3 (in)"),
            shape(session),
        )
    }

    @Test
    fun aFoldedGroupIsOnlyItsRowAndAnEmptyOneThatIsFoldedHasNoVacio() {
        assertEquals(listOf("@f1 folded 2", "@f2 open 0", "vacío", "sin grupo", "Folder:b3 (in)"), shape(session, setOf(f1)))
        assertEquals(listOf("@f1 folded 2", "@f2 folded 0", "sin grupo", "Folder:b3 (in)"), shape(session, setOf(f1, f2)))
    }

    @Test
    fun aWorkspaceWhoseGroupIsNotListedIsInNoneAndAGroupWithAWorktreeButNoRootNamesTheRepository() {
        val stray = session.copy(workspaces = listOf(app.copy(group = "gone"), notes))
        assertEquals(listOf("@f1 open 0", "vacío", "@f2 open 0", "vacío", "sin grupo", "Root:b1 (in)", "Folder:b3 (in)"), shape(stray))
        val rootless = session.copy(workspaces = listOf(login))
        assertEquals(listOf("@f1 open 1", "# app (in)", "Worktree:b2 (in)", "@f2 open 0", "vacío"), shape(rootless))
    }

    @Test
    fun withNoGroupTheListIsAsItWasBefore() {
        val none = session.copy(groups = emptyList(), workspaces = session.workspaces.map { it.copy(group = null) })
        assertEquals(listOf("Root:b1", "Worktree:b2", "# carpetas", "Folder:b3"), shape(none))
        // and a daemon that does not say groups at all: a workspace names one the session does not list
        assertEquals(listOf("Root:b1", "Worktree:b2", "# carpetas", "Folder:b3"), shape(session.copy(groups = emptyList())))
    }

    @Test
    fun aFoldedGroupSaysWhatRunsAndEachKindOfAttentionTheMostUrgentFirstWithThreeKindsAndTheRest() {
        val busy = session.copy(
            workspaces = listOf(
                app.copy(
                    working = true,
                    attention = listOf(
                        attention(AttentionKind.Permission, "n1"), attention(AttentionKind.Question, "n2"), attention(AttentionKind.Plan, "n3"),
                        attention(AttentionKind.CiFailed, "n4"), attention(AttentionKind.Merged, "n5"),
                    ),
                ),
                login.copy(removing = true, attention = listOf(attention(AttentionKind.Permission, "n6"))),
                notes,
            ),
        )
        val group = homeItems(busy, Seen(), setOf(f1)).filterIsInstance<HomeItem.Group>().first()
        assertEquals(2, group.running, "a conversation that works and a worktree being removed")
        assertEquals(listOf("!" to 2, "?" to 1, "◆" to 1), group.shownMarks.map { it.mark.glyph to it.count })
        assertEquals(2, group.moreMarks, "the CI that failed and what was merged")
        // seen attention is not counted, and what is seen of all of them leaves the row quiet
        val seen = Seen().mark("n1", 3, "e").mark("n6", 3, "e")
        assertEquals(listOf("?", "◆", "✗"), groupSays(busy, seen, f1).second.map { it.mark.glyph }.take(3))
        assertEquals(0 to emptyList(), groupSays(session, Seen(), f2))
    }

    @Test
    fun theSheetOfAMoveSaysWhereTheWorkspaceIsAndWhatMovingTakes() {
        val m = session.moveTo(login.id)!!
        assertEquals("salesassist", m.current?.name)
        assertEquals(2, m.together)
        assertEquals("ahora en «salesassist» · mueve la raíz y sus 1 worktree (2 en total)", m.note)
        val alone = session.moveTo(notes.id)!!
        assertNull(alone.current)
        assertEquals("ahora sin grupo · mueve solo este workspace", alone.note)
        assertEquals(listOf(f1, f2), alone.groups.map { it.id })
        assertNull(session.moveTo("nobody"))
        val three = session.copy(workspaces = session.workspaces + login.copy(id = "w9"))
        assertEquals("ahora en «salesassist» · mueve la raíz y sus 2 worktrees (3 en total)", three.moveTo(app.id)!!.note)
    }

    @Test
    fun aNameIsRefusedWhenEmptyLongOrTakenWithoutRegardToCaseExceptByTheGroupBeingRenamed() {
        val groups = session.groups
        assertNull(groupNameError(" nuevo ", groups))
        assertEquals("el grupo necesita un nombre", groupNameError("  ", groups))
        assertNull(groupNameError("x".repeat(40), groups))
        assertEquals("el nombre pasa de 40 letras", groupNameError("x".repeat(41), groups))
        assertEquals("ya hay un grupo «SalesAssist»", groupNameError("SalesAssist", groups))
        assertNull(groupNameError("SALESASSIST", groups, except = f1), "its own name")
    }

    @Test
    fun theChipsCountEachGroupAndTheFilterLetsThroughOnlyItsAlerts() {
        val waiting = session.copy(
            workspaces = listOf(
                app.copy(attention = listOf(attention(AttentionKind.Permission, "n1"))),
                login.copy(attention = listOf(attention(AttentionKind.Question, "n2"), attention(AttentionKind.Plan, "n3"))),
                notes.copy(attention = listOf(attention(AttentionKind.CiFailed, "n4"))),
            ),
        )
        val items = avisos(waiting, Seen())
        assertEquals(listOf("salesassist", "salesassist", "salesassist", null), items.map { it.groupName }.sortedBy { it == null })
        assertEquals(
            listOf(Triple(null, "todos", 4), Triple(f1, "salesassist", 3), Triple(f2, "ideas", 0), Triple(NO_GROUP, "sin grupo", 1)),
            avisoChips(waiting, items).map { Triple(it.filter, it.label, it.count) },
        )
        assertEquals(3, items.filtered(f1).size)
        assertEquals(listOf("notes"), items.filtered(NO_GROUP).map { it.workspace })
        assertEquals(4, items.filtered(null).size)
        // with no group in the session, no chip but «todos»
        assertEquals(listOf("todos"), avisoChips(session.copy(groups = emptyList()), items).map { it.label })
    }

    private class Pc(override val epoch: String, var now: Node.Session) : NodeSource {
        val sent = mutableListOf<String>()
        override fun session() = now
        override fun node(id: String): Node? = null
        override fun createGroup(name: String, reply: (Result<String>) -> Unit) {
            sent += "create $name"
            reply(Result.success("g-new"))
        }
        override fun moveToGroup(workspace: String, group: String?, reply: (Result<Unit>) -> Unit) {
            sent += "move $workspace $group"
            reply(Result.success(Unit))
        }
    }

    @Test
    fun foldingATapOpeningAWorkspaceAndForgettingAGroupThePcNoLongerLists() {
        val pc = Pc("e", session)
        val nav = Navigator(pc)
        var s = nav.toggleGroup(ClientState(), f1)
        assertEquals(setOf(f1), s.folded)
        s = nav.toggleGroup(nav.toggleGroup(s, f2), f1)
        assertEquals(setOf(f2), s.folded)
        // opening a workspace of a folded group opens the group, and keeps what else is folded
        s = nav.toggleGroup(s, f1)
        s = nav.filterAvisos(s, f1)
        s = nav.openWorkspace(s, login.id)
        assertEquals(setOf(f2), s.folded)
        assertEquals(f1, s.avisosFilter, "the chip is kept")
        // a group the PC no longer lists is forgotten, once it has said hello
        pc.now = session.copy(groups = listOf(Group(f1, "salesassist")))
        assertEquals(emptySet(), nav.pruneFolded(s).folded)
        assertEquals(setOf(f2), Navigator(Pc("", pc.now)).pruneFolded(s).folded, "before hello the session is empty: nothing is forgotten")
    }

    @Test
    fun aGroupMadeWithAWorkspaceReadsTheIdOfTheGroupFromTheFirstReplyAndSendsTheMoveAfterIt() {
        val pc = Pc("e", session)
        var result: Result<Unit>? = null
        pc.createGroupWith("nuevo", notes.id) { result = it }
        assertEquals(listOf("create nuevo", "move ${notes.id} g-new"), pc.sent)
        assertTrue(result!!.isSuccess)
        // a source with no PC says so and sends nothing
        val offline = object : NodeSource {
            override val epoch = ""
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        var failed: Result<Unit>? = null
        offline.createGroupWith("x", notes.id) { failed = it }
        assertTrue(failed!!.isFailure)
    }
}
