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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.jw.next.client.AgentRow
import dev.jw.next.contract.TaskState

private val SPIN = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
private val Pad = 12.dp

/** The chip of the agents by the model: how many run, a tap opens or closes the panel (REQ-602). A touch target of 40 dp around a smaller pill. */
@Composable
internal fun AgentsChip(running: Int, open: Boolean, toggle: () -> Unit) {
    Box(
        Modifier.defaultMinSize(minWidth = 40.dp, minHeight = 40.dp).clickable(onClick = toggle)
            .semantics { role = Role.Button; contentDescription = "Agentes: $running corriendo" },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.defaultMinSize(minHeight = 26.dp).background(if (open) Palette.line else Palette.sel, CircleShape).padding(horizontal = 9.dp), contentAlignment = Alignment.Center) {
            Mono("⚙ $running", if (running > 0) Palette.yellow else Palette.dim, 11, bold = true)
        }
    }
}

/**
 * The agents under the bar of the tab, never more than 280 dp: one row for each with what it does now, and a tap opens that
 * row in place with its prompt, its steps, what it said and a button to stop it (REQ-602).
 */
@Composable
internal fun AgentsPanel(rows: List<AgentRow>, lit: Set<String>, tick: Long, toggle: (String) -> Unit, stop: (String) -> Unit) {
    val running = rows.count { it.running }
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Mono("agentes · " + if (running > 0) "$running corriendo" else "ninguno", Palette.dim, 12, modifier = Modifier.padding(horizontal = Pad, vertical = 6.dp))
        Column(Modifier.heightIn(max = 280.dp).verticalScroll(rememberScrollState()).padding(horizontal = Pad), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (rows.isEmpty()) Mono("nada corre ahora", Palette.dim, 12, modifier = Modifier.padding(bottom = 8.dp))
            rows.forEach { r -> AgentRowView(r, r.id in lit, tick, { toggle(r.id) }, { stop(r.id) }) }
        }
        Hairline()
    }
}

@Composable
private fun AgentRowView(r: AgentRow, open: Boolean, tick: Long, toggle: () -> Unit, stop: () -> Unit) {
    val (mark, color) = when (r.state) {
        TaskState.Run -> SPIN[(tick % SPIN.length).toInt()].toString() to Palette.yellow
        TaskState.Done -> "✓" to Palette.green
        TaskState.Stopped, TaskState.Failed -> "■" to Palette.red
    }
    Column(Modifier.fillMaxWidth().background(Palette.sel, RoundedCornerShape(8.dp))) {
        Column(
            Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = toggle).padding(horizontal = 12.dp, vertical = 6.dp)
                .semantics { role = Role.Button; contentDescription = "${r.name}, ${r.label}, ${if (r.running) "trabajando" else "terminó"}. ${r.stats}" },
            verticalArrangement = Arrangement.Center,
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Mono(mark, color, 13, bold = true)
                Mono(r.name, Palette.fg, 13, bold = true)
                Mono(r.label, Palette.dim, 12, lines = 1, modifier = Modifier.weight(1f))
            }
            if (r.stats.isNotEmpty() || r.now != null) Mono(listOfNotNull(r.stats.ifEmpty { null }, r.now).joinToString(" · "), Palette.dim, 11, lines = 2)
        }
        if (open) Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            r.prompt?.let { Mono("prompt  $it", Palette.dim, 12, lines = 5) }
            r.steps.forEachIndexed { i, s ->
                val last = i == r.steps.lastIndex && r.running
                val tool = s.substringBefore(' ')
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Mono(if (last) mark else "✓", if (last) Palette.yellow else Palette.green, 12)
                    Mono(tool, Palette.cyan, 12)
                    Mono(s.removePrefix(tool).trim(), Palette.fg, 12, lines = 1, modifier = Modifier.weight(1f))
                }
            }
            r.said?.let { Mono("dijo  $it", Palette.fg, 12, lines = 6) }
            if (r.running) Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).background(Palette.panel, RoundedCornerShape(8.dp)).clickable(onClick = stop),
                contentAlignment = Alignment.Center,
            ) { Mono("■ Detener", Palette.red, 13, bold = true) }
        }
    }
}
