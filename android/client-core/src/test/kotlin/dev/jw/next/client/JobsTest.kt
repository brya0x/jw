package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.JobState
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** REQ-659 and REQ-660: what the phone makes of the jobs the session carries. */
class JobsTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session-jobs.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val running = session.jobs[0]
    private val pushed = session.jobs[1]
    private val failed = session.jobs[2]

    @Test
    fun anUpdateOfClaudeHasNoWorkspaceAndIsNamedByWhatItDid() {
        val update = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session-update-claude.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
        val (done, broke) = update.jobs
        assertEquals(null, done.workspace)
        assertEquals("actualizar claude 2.1.309 → 2.1.312", done.title())
        assertEquals("actualizar claude", done.kindLabel())
        assertEquals("actualizar claude", broke.title())
        assertEquals("EACCES: permission denied", broke.said())
        assertEquals(JobsMark.Ended(failed = true, count = 2), jobsMark(update, Seen()))
    }

    private fun without(vararg ids: String) = session.copy(jobs = session.jobs.filterNot { it.id in ids })

    private class Pc(override val epoch: String, var now: Node.Session) : NodeSource {
        override fun session() = now
        override fun node(id: String): Node? = null
    }

    @Test
    fun theJobsAreListedRunningFirstAndNamedByWhatTheyDo() {
        val list = jobsOf(session)
        assertEquals(listOf(running), list.running)
        assertEquals(listOf(pushed, failed), list.ended, "the daemon's order, newest first")
        assertEquals("quitar worktree app: feat/login", running.title())
        assertEquals("subir feat/notes", pushed.title())
        assertEquals("subir rama", pushed.kindLabel())
    }

    @Test
    fun timesAreSaidInSecondsAndMinutesAndNeverNegative() {
        assertEquals("4,2 s", took(4_200))
        assertEquals("0,0 s", took(0))
        assertEquals("38 s", took(38_400))
        assertEquals("3 min", took(180_000))
        assertEquals(3_400L, running.time(running.startedAt + 3_400))
        assertEquals(0L, running.time(running.startedAt - 5_000), "a phone whose clock is behind (RISK-76)")
        assertEquals(2_300L, pushed.time(0), "a job that ended says how long it took")
        assertEquals(7_700L, pushed.ago(pushed.startedAt + 2_300 + 7_700))
        assertEquals(null, running.ago(0))
    }

    @Test
    fun whatAJobSaysIsItsResultItsErrorOrWhatCanBeDone() {
        assertEquals("No se cancela: dejaría el worktree a medias.", running.said())
        assertEquals("subido feat/notes", pushed.said())
        assertTrue(failed.said().contains("--force"), failed.said())
        assertEquals("Se puede cancelar sin perder nada.", running.copy(cancellable = true).said())
        assertEquals("quitado · la rama se queda", running.copy(state = JobState.Done).said())
        assertEquals("falló sin decir por qué", failed.copy(detail = null).said())
    }

    @Test
    fun theMarkBesideTheBellIsWhatRunsThenWhatEndedUnseen() {
        assertEquals(JobsMark.Running(1), jobsMark(session, Seen()))
        val ended = without(running.id)
        assertEquals(JobsMark.Ended(failed = true, count = 2), jobsMark(ended, Seen()))
        val seen = Seen().seeJobs(ended.jobs.map { it.id }, "e")
        assertEquals(JobsMark.None, jobsMark(ended, seen))
        // One that ends later is unseen again, and the failure makes the mark red.
        assertEquals(JobsMark.Ended(failed = false, count = 1), jobsMark(without(running.id, failed.id), Seen().seeJobs(emptyList(), "e")))
        assertEquals(JobsMark.None, jobsMark(session.copy(jobs = emptyList()), Seen()))
    }

    @Test
    fun whatWasSeenIsKeptPerEpochAndOnlyForTheJobsThatAreStillThere() {
        val seen = Seen().seeJobs(listOf("a", "b"), "e1")
        assertTrue(seen.isJobSeen("a"))
        assertEquals(seen, Seen.fromJson(seen.toJson()))
        assertFalse(seen.seeJobs(listOf("b"), "e1").isJobSeen("a"), "a job that is gone is forgotten")
        assertFalse(seen.seeJobs(listOf("b"), "e2").isJobSeen("a"), "another daemon, other jobs")
        // A record from before the jobs has none.
        assertEquals(Seen("e", mapOf("n" to 3L)), Seen.fromJson("""{"epoch":"e","nodes":{"n":3}}"""))
    }

    @Test
    fun aRemovalThatEndsIsSaidAndOneThatWasNotRunningIsNot() {
        val before = jobStates(session)
        assertEquals(emptyList(), endedRemovals(before, session))
        val done = session.copy(jobs = session.jobs.map { if (it.id == running.id) it.copy(state = JobState.Done, durationMs = 4_200) else it })
        assertEquals(listOf("✓ quitado app: feat/login · 4,2 s"), endedRemovals(before, done).map { it.endNotice() })
        val bad = session.copy(jobs = session.jobs.map { if (it.id == running.id) it.copy(state = JobState.Failed, detail = "x") else it })
        assertEquals(listOf("✗ no se pudo quitar app: feat/login"), endedRemovals(before, bad).map { it.endNotice() })
        // One that came and went between two looks is said too; one the phone had seen end already is not said again.
        assertEquals(listOf("✓ quitado app: feat/login · 4,2 s", "✗ no se pudo quitar app: feat/old"), endedRemovals(emptyMap(), done).map { it.endNotice() })
        assertEquals(emptyList(), endedRemovals(jobStates(done), done))
        // A push that ends is not a removal.
        assertEquals(emptyList(), endedRemovals(jobStates(session) + (pushed.id to JobState.Running), session))
    }

    @Test
    fun theRowOfAWorkspaceBeingRemovedSaysSoAndAFailureMarksTheRowUntilItIsDismissed() {
        val rows = homeRows(session, Seen())
        assertTrue(rows[0].removing && !rows[0].removalFailed)
        assertFalse(rows[1].removing)
        val back = session.copy(
            workspaces = session.workspaces.map { it.copy(removing = false) },
            jobs = session.jobs.map { if (it.id == running.id) it.copy(state = JobState.Failed, detail = "x") else it },
        )
        assertTrue(homeRows(back, Seen())[0].removalFailed)
        assertTrue(removalFailed(back, running.workspace!!))
        assertFalse(removalFailed(session.copy(jobs = emptyList()), running.workspace!!), "dismissed")
        assertFalse(removalFailed(back, pushed.workspace!!), "a push that went well is no failure of the row")
    }

    @Test
    fun avisosOpensOnTheTabThatHasSomethingAndShowingTheJobsSeesWhatEnded() {
        val pc = Pc("e", session.copy(attention = emptyList(), workspaces = session.workspaces.map { it.copy(attention = emptyList()) }))
        val nav = Navigator(pc)
        // Nothing waits, and a job runs: the jobs.
        val s0 = nav.openAvisos(ClientState())
        assertEquals(Screen.Avisos, s0.screen)
        assertEquals(AvisosTab.Trabajos, s0.avisosTab)
        // The tab shows what ended, and that is seeing it.
        assertEquals(setOf(pushed.id, failed.id), s0.seen.jobs)
        assertEquals(JobsMark.Running(1), jobsMark(pc.now, s0.seen))
        // An alert waiting wins; a tab can be asked for.
        val withAlert = session.workspaces[0].copy(attention = listOf(dev.jw.next.contract.Attention(dev.jw.next.contract.AttentionKind.Permission, "n", 1)))
        pc.now = session.copy(workspaces = listOf(withAlert) + session.workspaces.drop(1))
        assertEquals(AvisosTab.Alertas, nav.openAvisos(ClientState()).avisosTab)
        val asked = nav.openAvisos(ClientState(), AvisosTab.Trabajos)
        assertEquals(AvisosTab.Trabajos, asked.avisosTab)
        // A job that ends while the tab is open is seen at once; on the other tab nothing is.
        pc.now = pc.now.copy(jobs = pc.now.jobs.map { if (it.id == running.id) it.copy(state = JobState.Done, durationMs = 1) else it })
        assertEquals(setOf(running.id, pushed.id, failed.id), nav.seeJobs(asked).seen.jobs)
        val alerts = nav.openAvisos(ClientState(), AvisosTab.Alertas)
        assertEquals(alerts, nav.seeJobs(alerts))
        assertEquals(emptySet(), alerts.seen.jobs)
    }

    @Test
    fun theJobsTabDoesNotEmptyTheRecordBeforeTheDaemonHasSaidItsEpoch() {
        val seen = Seen("e", mapOf("n" to 3L))
        val nav = Navigator(Pc("", session))
        val s = ClientState(Screen.Avisos, seen, avisosTab = AvisosTab.Trabajos)
        assertEquals(s, nav.seeJobs(s))
    }

    @Test
    fun aSourceWithNoPcRefusesToDismissOrCancel() {
        val pc = Pc("e", session)
        val answers = mutableListOf<Boolean>()
        pc.dismissJob("j") { answers += it.isFailure }
        pc.dismissJobs { answers += it.isFailure }
        pc.cancel("w") { answers += it.isFailure }
        assertEquals(listOf(true, true, true), answers)
    }
}
