package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.jw.next.client.AgentRow
import dev.jw.next.client.AGENT_TAIL_PREVIEW
import dev.jw.next.client.AgentSection
import dev.jw.next.contract.TaskKind
import dev.jw.next.contract.TaskState

private val SPIN = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
private val Pad = 12.dp

/** The chip of what runs by the model: how many run, a tap opens or closes the panel (REQ-602). A touch target of 48 dp around a smaller pill. */
@Composable
internal fun AgentsChip(running: Int, open: Boolean, toggle: () -> Unit) {
    Box(
        Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = toggle)
            .semantics { role = Role.Button; contentDescription = "En marcha: $running corriendo" },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.defaultMinSize(minHeight = 26.dp).background(if (open) Palette.line else Palette.sel, CircleShape).padding(horizontal = 9.dp), contentAlignment = Alignment.Center) {
            Mono("⚙ $running", if (running > 0) Palette.yellow else Palette.dim, 11, bold = true)
        }
    }
}

/**
 * What runs, under the bar of the tab, never more than 300 dp: a small heading for each section (subagents, shells, monitors),
 * one row for each task, and a tap opens that row in place with what it is and a button to stop it (REQ-602).
 */
@Composable
internal fun AgentsPanel(rows: List<AgentRow>, lit: Set<String>, tick: Long, toggle: (String) -> Unit, stop: (String) -> Unit) {
    val running = rows.count { it.running }
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Mono("en marcha · " + if (running > 0) "$running corriendo" else "nada", Palette.dim, 12, modifier = Modifier.padding(horizontal = Pad, vertical = 6.dp))
        Column(Modifier.heightIn(max = 300.dp).verticalScroll(rememberScrollState()).padding(horizontal = Pad), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (rows.isEmpty()) Mono("nada corre ahora", Palette.dim, 12, modifier = Modifier.padding(bottom = 8.dp))
            AgentSection.entries.forEach { section ->
                val of = rows.filter { it.section == section }
                if (of.isNotEmpty()) {
                    Mono("${section.title} · ${of.size}", Palette.dim, 11, modifier = Modifier.padding(top = 4.dp))
                    // The rows are keyed by task, so one that opens or ends does not make its neighbours flicker.
                    of.forEach { r -> key(r.id) { AgentRowView(r, r.id in lit, tick, { toggle(r.id) }, { stop(r.id) }) } }
                }
            }
        }
        Hairline()
    }
}

@Composable
private fun AgentRowView(r: AgentRow, open: Boolean, tick: Long, toggle: () -> Unit, stop: () -> Unit) {
    val (mark, color) = when (r.state) {
        // A monitor that runs does not turn: it watches.
        TaskState.Run -> if (r.section == AgentSection.Monitors) (if (r.kind == TaskKind.Loop) "↻" else "◉") to Palette.yellow else SPIN[(tick % SPIN.length).toInt()].toString() to Palette.yellow
        TaskState.Done -> "✓" to Palette.green
        TaskState.Failed -> "✗" to Palette.red
        TaskState.Stopped -> "■" to Palette.dim
    }
    Column(Modifier.fillMaxWidth().background(Palette.sel, RoundedCornerShape(8.dp))) {
        Column(
            Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = toggle).padding(horizontal = 12.dp, vertical = 6.dp)
                .semantics { role = Role.Button; contentDescription = "${r.name}, ${r.doing.ifEmpty { r.said.orEmpty() }}. ${r.stats}" },
            verticalArrangement = Arrangement.Center,
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Mono(mark, color, 13, bold = true)
                Mono(r.name, Palette.fg, 13, bold = true, modifier = Modifier.weight(1f))
                Mono(r.stats, Palette.dim, 11)
            }
            if (r.doing.isNotEmpty()) Mono("→ ${r.doing}", Palette.dim, 12, lines = 2)
        }
        if (open) Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            when (r.section) {
                AgentSection.Agents -> {
                    Mono(r.label, Palette.fg, 12, bold = true, lines = 2)
                    r.prompt?.let { Mono("prompt", Palette.dim, 11); Mono(it, Palette.fg, 12, lines = 5) }
                    if (r.steps.isNotEmpty()) Mono("pasos", Palette.dim, 11)
                    r.steps.forEachIndexed { i, s ->
                        val last = i == r.steps.lastIndex && r.running
                        val tool = s.substringBefore(' ')
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Mono(if (last) mark else "✓", if (last) Palette.yellow else Palette.green, 12)
                            Mono(tool, Palette.cyan, 12)
                            Mono(s.removePrefix(tool).trim(), Palette.fg, 12, lines = 1, modifier = Modifier.weight(1f))
                        }
                    }
                    r.said?.let { Mono("dijo", Palette.dim, 11); Mono(it, Palette.fg, 12, lines = 6) }
                }
                AgentSection.Shells -> {
                    Mono("comando", Palette.dim, 11); Mono(r.name, Palette.fg, 12, lines = 4)
                    r.said?.let { Mono(if (r.running) "ahora" else "terminó", Palette.dim, 11); Mono(it, Palette.fg, 12, lines = 4) }
                    OutputView(r.tail)
                }
                AgentSection.Monitors -> {
                    Mono(if (r.kind == TaskKind.Loop) "repite" else "vigila", Palette.dim, 11); Mono(r.name, Palette.fg, 12, lines = 4)
                    r.said?.let { Mono("último", Palette.dim, 11); Mono(it, Palette.fg, 12, lines = 4) }
                    if (r.kind != TaskKind.Loop) OutputView(r.tail)
                }
            }
            if (r.running) Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).background(Palette.panel, RoundedCornerShape(8.dp)).clickable(onClick = stop),
                contentAlignment = Alignment.Center,
            ) { Mono("■ Detener", Palette.red, 13, bold = true) }
        }
    }
}

/** What a shell or a monitor printed last (REQ-607): the last lines in a dark box, one each, and a button of 48 dp that opens all of them wrapped. */
@Composable
private fun OutputView(tail: List<String>) {
    var all by remember { mutableStateOf(false) }
    Mono("salida", Palette.dim, 11)
    if (tail.isEmpty()) {
        Mono("sin salida todavía", Palette.dim, 12)
        return
    }
    Column(Modifier.fillMaxWidth().background(Palette.bg, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp, vertical = 6.dp)) {
        (if (all) tail else tail.takeLast(AGENT_TAIL_PREVIEW)).forEach { Mono(it, Palette.fg, 11, lines = if (all) Int.MAX_VALUE else 1) }
    }
    Box(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable { all = !all }
            .semantics { role = Role.Button; contentDescription = if (all) "Cerrar la salida" else "Ver la salida" },
        contentAlignment = Alignment.CenterStart,
    ) { Mono(if (all) "▤ Cerrar salida" else "▤ Ver salida", Palette.blue, 13) }
}
