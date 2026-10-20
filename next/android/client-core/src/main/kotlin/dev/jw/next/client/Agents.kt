package dev.jw.next.client

import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.Node
import dev.jw.next.contract.TaskKind
import dev.jw.next.contract.TaskState

/*
 * The agents of a conversation as the phone's panel shows them (spec REQ-602): the tasks of the
 * engine, newest first, each with what it does now and, opened, what it was asked and the steps
 * it took. Pure functions of the node.
 */

/** One task of the panel, with what its card in the items says when it has one. */
data class AgentRow(
    val id: String,
    val name: String,
    val label: String,
    val state: TaskState,
    val kind: TaskKind,
    /** `14 tools · 38.4k · 1:12`, or only the time for what has no card. */
    val stats: String,
    /** What it does now (its last step, or what the engine says), when it runs. */
    val now: String?,
    val prompt: String?,
    /** The steps it took, oldest first, the last few. */
    val steps: List<String>,
    /** What it said when it ended. */
    val said: String?,
) {
    val running get() = state == TaskState.Run
}

/** The most rows the panel lists. */
const val AGENT_ROWS = 8

/** The steps of one agent that the panel keeps. */
const val AGENT_STEPS = 5

fun agentClock(ms: Long): String {
    val s = maxOf(0, ms) / 1000
    return "${s / 60}:${(s % 60).toString().padStart(2, '0')}"
}

fun agentTokens(n: Long): String = if (n < 1000) n.toString() else {
    val tenth = n / 100
    if (tenth % 10 == 0L) "${tenth / 10}k" else "${tenth / 10}.${tenth % 10}k"
}

/** The tasks of [conv], newest first, with the card of each that has one; [now] is the clock of the phone, in milliseconds. */
fun agentRows(conv: Node.Conversation, now: Long): List<AgentRow> =
    conv.tasks.sortedByDescending { it.started }.take(AGENT_ROWS).map { t ->
        val card = t.item?.let { id -> conv.items.lastOrNull { it is ConvItem.Agent && it.id == id } as ConvItem.Agent? }
        val elapsed = if (t.state == TaskState.Run) maxOf(0, now - t.started) else 0
        AgentRow(
            id = t.id,
            name = card?.name ?: when (t.kind) {
                TaskKind.Agent -> "agente"
                TaskKind.Bash -> "shell"
                TaskKind.Monitor -> "monitor"
                TaskKind.Loop -> "loop"
                TaskKind.Other -> "tarea"
            },
            label = t.label,
            state = t.state,
            kind = t.kind,
            stats = when {
                card != null -> "${card.tools} tools · ${agentTokens(card.tokens)} · ${agentClock(if (t.state == TaskState.Run) maxOf(elapsed, card.ms) else card.ms)}"
                t.state == TaskState.Run -> agentClock(elapsed)
                else -> ""
            },
            now = if (t.state == TaskState.Run) (t.info ?: card?.steps?.lastOrNull()) else null,
            prompt = card?.prompt,
            steps = card?.steps?.takeLast(AGENT_STEPS).orEmpty(),
            said = if (t.state == TaskState.Run) null else t.info,
        )
    }

/** Whether the chip of the agents is shown: while some run, and while the panel is open so that it can be closed. */
fun agentsChip(conv: Node.Conversation, open: Boolean): Int? {
    val running = conv.tasks.count { it.state == TaskState.Run }
    return if (running > 0 || open) running else null
}
