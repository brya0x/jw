package dev.jw.next.client

import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.Node
import dev.jw.next.contract.Task
import dev.jw.next.contract.TaskKind
import dev.jw.next.contract.TaskState

/*
 * What the engine runs while the person works, as the phone's panel shows it (spec REQ-602): in
 * three sections, the subagents, the shells and the monitors, the newest first in each, with what
 * each does and, opened, what it was asked and the steps it took or the command it runs. Pure
 * functions of the node.
 */

/** The three kinds of what runs, in the order the panel draws them. */
enum class AgentSection(val title: String) {
    Agents("subagentes"),
    Shells("shells"),
    Monitors("monitores"),
}

/** The section a task is in: a `Bash` is a shell; what the engine calls a monitor, a loop or does not name is watching or repeating. */
fun TaskKind.section() = when (this) {
    TaskKind.Agent -> AgentSection.Agents
    TaskKind.Bash -> AgentSection.Shells
    TaskKind.Monitor, TaskKind.Loop, TaskKind.Other -> AgentSection.Monitors
}

/** One line of the panel: a subagent, a shell or a monitor, with what its tool call in the items says when it has one. */
data class AgentRow(
    val id: String,
    val section: AgentSection,
    /** `Explore (haiku-5.5)` for a subagent; the command for a shell and for a monitor. */
    val name: String,
    /** What the engine calls it. */
    val label: String,
    val state: TaskState,
    val kind: TaskKind,
    /** `1:12 · 38.4k` for a subagent; only the time for the others. */
    val stats: String,
    /** What a subagent does (what the engine says, else its last step) while it runs, and how it ended after; empty for the others. */
    val doing: String,
    val prompt: String?,
    /** The steps a subagent took, oldest first, the last few. */
    val steps: List<String>,
    /** What it says: when a subagent ended, what it said; for a shell or a monitor what it does or last said, never the command again. */
    val said: String?,
    /** The last lines a shell or a monitor printed, newest last, as plain text (REQ-607); empty for a subagent, a loop, and until it prints. */
    val tail: List<String> = emptyList(),
) {
    val running get() = state == TaskState.Run
}

/** The most lines of one section. */
const val AGENT_ROWS = 6

/** The lines of the output of a shell or a monitor that a row shows before it is opened wide. */
const val AGENT_TAIL_PREVIEW = 4

/** The steps of one subagent that the panel keeps. */
const val AGENT_STEPS = 5

fun agentClock(ms: Long): String {
    val s = maxOf(0, ms) / 1000
    return "${s / 60}:${(s % 60).toString().padStart(2, '0')}"
}

fun agentTokens(n: Long): String = if (n < 1000) n.toString() else {
    val tenth = n / 100
    if (tenth % 10 == 0L) "${tenth / 10}k" else "${tenth / 10}.${tenth % 10}k"
}

/** The tasks of [conv] by section, the newest first in each, with the item of the tool call of each that has one; [now] is the clock of the phone, in milliseconds. */
fun agentRows(conv: Node.Conversation, now: Long): List<AgentRow> =
    AgentSection.entries.flatMap { section ->
        conv.tasks.filter { it.kind.section() == section }.sortedByDescending { it.started }.take(AGENT_ROWS).map { agentRow(conv, it, now) }
    }

private fun agentRow(conv: Node.Conversation, t: Task, now: Long): AgentRow {
    val section = t.kind.section()
    val card = if (section == AgentSection.Agents) t.item?.let { id -> conv.items.filterIsInstance<ConvItem.Agent>().lastOrNull { it.id == id } } else null
    // The command is what the tool call was given; the engine's own words for the task are a description.
    val command = if (section == AgentSection.Agents) null else t.item?.let { id -> conv.items.filterIsInstance<ConvItem.Tool>().lastOrNull { it.id == id }?.arg?.takeIf { it.isNotBlank() } }
    val elapsed = if (t.state == TaskState.Run) maxOf(0, now - t.started) else 0
    val name = when {
        card != null -> card.model?.let { "${card.name} ($it)" } ?: card.name
        section == AgentSection.Agents -> "agente"
        else -> command ?: t.label
    }
    val shown = command ?: t.label
    val ended = when (t.state) {
        TaskState.Done -> "listo"
        TaskState.Stopped -> "detenido"
        TaskState.Failed -> "falló"
        TaskState.Run -> "en marcha"
    }
    return AgentRow(
        id = t.id,
        section = section,
        name = name,
        label = t.label,
        state = t.state,
        kind = t.kind,
        stats = when {
            card != null -> "${agentClock(if (t.state == TaskState.Run) maxOf(elapsed, card.ms) else card.ms)} · ${agentTokens(card.tokens)}"
            t.state == TaskState.Run -> agentClock(elapsed)
            else -> ""
        },
        doing = if (section != AgentSection.Agents) "" else when (t.state) {
            TaskState.Run -> t.info ?: card?.steps?.lastOrNull() ?: "empezando"
            else -> ended
        },
        prompt = card?.prompt,
        steps = card?.steps?.takeLast(AGENT_STEPS).orEmpty(),
        said = when {
            section == AgentSection.Agents -> if (t.state == TaskState.Run) null else t.info
            else -> t.info?.takeIf { it.isNotBlank() && it != shown } ?: t.label.takeIf { it != shown && t.state == TaskState.Run } ?: ended
        },
        tail = if (section == AgentSection.Agents || t.kind == TaskKind.Loop) emptyList() else t.tail,
    )
}

/** How many of each kind run: what the footer of the terminal says and the chip counts all of. */
fun agentsRunning(conv: Node.Conversation): Int = conv.tasks.count { it.state == TaskState.Run }

/** Whether the chip of what runs is shown: while something runs, and while the panel is open so that it can be closed. */
fun agentsChip(conv: Node.Conversation, open: Boolean): Int? {
    val running = agentsRunning(conv)
    return if (running > 0 || open) running else null
}
