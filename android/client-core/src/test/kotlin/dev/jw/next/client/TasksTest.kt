package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.LoopInfo
import dev.jw.next.contract.LoopStop
import dev.jw.next.contract.Node
import dev.jw.next.contract.StopKind
import dev.jw.next.contract.TaskPhase
import dev.jw.next.contract.TaskStatus
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-948 to REQ-950, REQ-958: what the phone shows of the task lists, from the shared fixtures. */
class TasksTest {
    private fun node(name: String) =
        (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/$name.json").readText()) as DaemonMsg.Snapshot).node
    private val session = node("snapshot-session-tasks") as Node.Session
    private val plain = node("snapshot-tasks") as Node.Tasks
    private val stopped = node("snapshot-tasks-stopped") as Node.Tasks

    @Test
    fun aHomeRowSaysHowFarItsListIsAndWhetherTheLoopRuns() {
        val rows = homeRows(session, Seen()).associateBy { it.name }
        assertNull(rows.getValue("app").tasks)
        assertEquals("3/8", rows.getValue("api-v2").tasks)
        assertTrue(rows.getValue("api-v2").loopOn)
        assertEquals("2/6", rows.getValue("checkout").tasks)
        assertFalse(rows.getValue("checkout").loopOn)
    }

    @Test
    fun theRowsAreByLevelAndTellWhatTheyWaitFor() {
        val rows = taskRows(plain)
        assertEquals(listOf("t1", "t2", "t3", "t4", "fin"), rows.map { it.id })
        assertEquals("t1, t2", rows[2].waits)
        assertEquals("todas", rows.last().waits)
        assertEquals("se añade al archivo al activar el bucle", rows.last().hint)
        assertEquals(Tone.Good, rows[0].tone)
    }

    @Test
    fun aBlockedTaskKeepsItsReasonAndTheFinalOneItsEvidence() {
        val rows = taskRows(stopped)
        assertEquals("needs STRIPE_KEY in .env", rows[2].why)
        assertEquals(listOf("cargo test: 212 ok, 2 fail"), rows.last().notes)
        assertNull(rows.last().hint)
    }

    @Test
    fun aFinalTaskDoneWithNoNotesSaysSoAndTheSheetOffersWhatFits() {
        val done = plain.copy(tasks = plain.tasks.map { if (it.isFinal) it.copy(state = TaskPhase.Done, status = TaskStatus.Done, auto = false) else it })
        assertEquals("sin evidencia anotada", taskRows(done).last().hint)
        val rows = taskRows(stopped)
        assertEquals(listOf(TaskAction.Done, TaskAction.Block), taskActions(rows[3]))
        assertEquals(listOf(TaskAction.Done, TaskAction.Reopen), taskActions(rows[2]))
        assertEquals(listOf(TaskAction.Reopen), taskActions(rows[0]))
    }

    @Test
    fun theLoopCardFollowsTheLoop() {
        val off = loopCard(plain)
        assertEquals(LoopButton.Start, off.button)
        assertEquals("bucle apagado", off.title)
        val blocked = loopCard(stopped)
        assertEquals(Tone.Warn, blocked.tone)
        assertEquals(LoopButton.Start, blocked.button)
        val on = loopCard(plain.copy(loop = LoopInfo(on = true, laps = 3, max = 20, task = "t3")))
        assertEquals("vuelta 3/20", on.title)
        assertEquals(LoopButton.Stop, on.button)
        assertTrue(on.running)
        val waiting = loopCard(plain.copy(loop = LoopInfo(on = true, laps = 3, max = 20, waiting = true)))
        assertFalse(waiting.running)
        assertEquals(Tone.Good, loopCard(plain.copy(loop = LoopInfo(on = false, laps = 4, max = 20, stop = LoopStop(StopKind.Done, "listo")))).tone)
        assertEquals(Tone.Dim, loopCard(plain.copy(loop = LoopInfo(on = false, laps = 4, max = 20, stop = LoopStop(StopKind.Person, "apagado")))).tone)
    }

    @Test
    fun aNewStopIsNewsOnceAndTheFirstSessionIsNot() {
        val notices = TasksNotices()
        val quiet = session.copy(workspaces = session.workspaces.map { if (it.name == "checkout") it.copy(tasks = it.tasks?.copy(stop = null)) else it })
        assertTrue(notices.update(quiet).isEmpty())
        val news = notices.update(session)
        assertEquals(1, news.size)
        assertEquals("checkout: t3 blocked: needs STRIPE_KEY in .env", news[0].text)
        assertFalse(news[0].good)
        assertTrue(notices.update(session).isEmpty())
        val off = session.copy(workspaces = session.workspaces.map { if (it.name == "api-v2") it.copy(tasks = it.tasks?.copy(on = false, stop = LoopStop(StopKind.Person, "apagado"))) else it })
        assertTrue(notices.update(off).isEmpty(), "a loop the person turned off is not news")
        val first = TasksNotices()
        assertTrue(first.update(session).isEmpty())
    }

    @Test
    fun theNodeIsAskedAgainOnlyAfterTheThrottleAndOnlyWhileTheRowHasAList() {
        val w = TasksWatch(3000)
        assertFalse(w.want(have = false, listed = false, nowMs = 0))
        assertTrue(w.want(have = false, listed = true, nowMs = 0))
        assertFalse(w.want(have = false, listed = true, nowMs = 1000))
        assertTrue(w.want(have = false, listed = true, nowMs = 3000))
        assertFalse(w.want(have = true, listed = true, nowMs = 3100))
        assertTrue(w.want(have = false, listed = true, nowMs = 3200))
    }

    private val ws = node("snapshot-workspace-views") as Node.Workspace
    private fun navigatorOver(listed: Boolean): Navigator {
        val w = ws.copy(id = session.workspaces[1].id)
        val s = if (listed) session else session.copy(workspaces = session.workspaces.map { it.copy(tasks = null) })
        return Navigator(object : NodeSource {
            override val epoch = s.epoch
            override fun session() = s
            override fun node(id: String): Node? = if (id == w.id) w else null
        })
    }

    @Test
    fun theTasksTabStaysWhileTheRowHasAListAndGivesWayToTheFirstPaneWhenItGoes() {
        val id = session.workspaces[1].id
        val at = ClientState(screen = Screen.Workspace(id, TASKS_TAB))
        assertEquals(at, navigatorOver(true).settle(at))
        assertEquals(ws.panes.first().id, (navigatorOver(false).settle(at).screen as Screen.Workspace).pane)
    }

    @Test
    fun theTasksTabLeavesNoFakeNodeInTheSeenRecord() {
        val id = session.workspaces[1].id
        val nav = navigatorOver(true)
        val at = nav.selectPane(ClientState(screen = Screen.Workspace(id, null)), TASKS_TAB)
        assertEquals(Screen.Workspace(id, TASKS_TAB), at.screen)
        assertFalse(at.seen.toJson().contains(TASKS_TAB))
        assertFalse(nav.openWorkspace(ClientState(), id, TASKS_TAB).seen.toJson().contains(TASKS_TAB))
    }
}
