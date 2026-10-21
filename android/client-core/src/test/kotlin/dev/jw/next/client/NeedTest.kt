package dev.jw.next.client

import dev.jw.next.contract.Attention
import dev.jw.next.contract.AttentionKind
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-247: what an attention asks of the person, which colours the line that says it. */
class NeedTest {
    private fun need(kind: AttentionKind, exit: Int? = null) = Attention(kind, "n", 1, exit).need()

    @Test
    fun aPermissionAQuestionAndAPlanAskForADecision() {
        for (kind in listOf(AttentionKind.Permission, AttentionKind.Question, AttentionKind.Plan)) {
            assertEquals(Need.Decide, need(kind), "$kind")
        }
    }

    @Test
    fun aFailedCiAndACommandThatExitedWithAnErrorFailed() {
        assertEquals(Need.Failed, need(AttentionKind.CiFailed))
        assertEquals(Need.Failed, need(AttentionKind.ShellDone, exit = 2))
    }

    @Test
    fun whatEndedWellOrOnlyWaitsIsToBeLookedAt() {
        assertEquals(Need.Review, need(AttentionKind.ShellDone, exit = 0))
        assertEquals(Need.Review, need(AttentionKind.ShellDone), "no exit code is a command that ended")
        for (kind in listOf(AttentionKind.AgentDone, AttentionKind.Merged, AttentionKind.Screen)) {
            assertEquals(Need.Review, need(kind), "$kind")
        }
    }
}
