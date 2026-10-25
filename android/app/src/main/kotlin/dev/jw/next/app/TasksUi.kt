package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import dev.jw.next.client.DEFAULT_LAPS
import dev.jw.next.client.LOOP_LAPS
import dev.jw.next.client.LoopButton
import dev.jw.next.client.TaskAction
import dev.jw.next.client.TaskRow
import dev.jw.next.client.TasksNotice
import dev.jw.next.client.TasksWatch
import dev.jw.next.client.loopCard
import dev.jw.next.client.taskActions
import dev.jw.next.client.taskRows
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.TaskOp
import kotlinx.coroutines.delay

private val Gutter = 16.dp

/** What the person asked of one task or of the loop, over the tab (REQ-949). */
private sealed interface Asking {
    data class Task(val row: TaskRow) : Asking
    data class Block(val row: TaskRow) : Asking
    data object Laps : Asking
}

/**
 * The «tareas» tab of a workspace: the card of its loop over its tasks by level, a sheet for a task and one for the laps
 * (REQ-949). It draws the node and edits nothing itself: every change is a command, and the node that comes back is what shows.
 */
@Composable
internal fun TasksPanel(source: AppSource, workspace: Id, listed: Boolean) {
    val version = source.tick.intValue
    var nodeId by remember(workspace) { mutableStateOf<Id?>(null) }
    val doc = remember(version, nodeId) { nodeId?.let { source.node(it) as? Node.Tasks } }
    val watch = remember(workspace) { TasksWatch() }
    // The node is asked for once, and again when it goes while the row still says there is a list (REQ-958).
    LaunchedEffect(version, doc != null, listed) {
        if (watch.want(doc != null, listed, System.currentTimeMillis())) {
            source.openTasks(workspace) { r -> r.onSuccess { nodeId = it; source.open(it) } }
        }
    }
    var asking by remember(workspace) { mutableStateOf<Asking?>(null) }
    var error by remember(workspace) { mutableStateOf<String?>(null) }
    val answer: (Result<Unit>) -> Unit = { r -> r.onFailure { error = it.message ?: "el PC no pudo hacerlo" } }
    val now = rememberClock(doc?.let { loopCard(it).running } == true)
    Box(Modifier.fillMaxSize()) {
        if (doc == null) {
            Mono("leyendo la lista…", Palette.dim, 13, modifier = Modifier.padding(Gutter))
        } else {
            val card = loopCard(doc)
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    Column(Modifier.fillMaxWidth().padding(Gutter), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (card.running) CircularProgressIndicator(Modifier.size(14.dp), color = Palette.blue, strokeWidth = 2.dp)
                            Mono(card.title, card.tone?.color() ?: Palette.fg, 14, bold = true, modifier = Modifier.weight(1f))
                            Mono("${card.done}/${card.total}", Palette.dim, 12)
                        }
                        Mono(card.line, card.tone?.color() ?: Palette.dim, 12, lines = 3)
                        Mono(
                            if (card.button == LoopButton.Start) "encender el bucle" else "apagar el bucle", Palette.blue, 13, bold = true,
                            modifier = Modifier.defaultMinSize(minHeight = 48.dp).clickable {
                                error = null
                                if (card.button == LoopButton.Start) asking = Asking.Laps else source.loopOff(workspace, answer)
                            }.padding(vertical = 14.dp),
                        )
                        doc.problems.forEach { Mono("! ${it.text}", Palette.yellow, 12, lines = 3) }
                        error?.let { Mono(it, Palette.red, 12, lines = 4) }
                    }
                    Hairline()
                }
                items(taskRows(doc), key = { it.id }) { row -> TaskLine(row, now) { error = null; asking = Asking.Task(row) } }
            }
        }
        when (val a = asking) {
            is Asking.Task -> TaskSheet(a.row, { asking = null }) { act ->
                if (act.needsText) asking = Asking.Block(a.row)
                else {
                    asking = null
                    source.editTask(workspace, act.op, a.row.id, null, answer)
                }
            }
            is Asking.Block -> BlockSheet(source, workspace, a.row, { asking = null }, answer)
            Asking.Laps -> LapsSheet({ asking = null }) { laps ->
                asking = null
                source.loopOn(workspace, laps, answer)
            }
            null -> {}
        }
    }
}

@Composable
private fun TaskLine(row: TaskRow, now: Long, open: () -> Unit) {
    Column(Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = open).padding(start = Gutter + 10.dp * (row.level - 1).coerceAtMost(4), end = Gutter, top = 8.dp, bottom = 8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mono(if (row.running) spinOf(now) else row.glyph, row.tone?.color() ?: Palette.fg, 14, bold = true, modifier = Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Mono(row.title, row.tone?.color() ?: Palette.fg, 13, lines = 3, bold = row.isFinal)
                row.waits?.let { Mono("espera: $it", Palette.dim, 11, lines = 2) }
                row.why?.let { Mono("bloqueada: $it", Palette.yellow, 12, lines = 3) }
                row.hint?.let { Mono(it, Palette.dim, 11, lines = 2) }
                row.notes.forEach { Mono("· $it", Palette.dim, 11, lines = 4) }
            }
        }
    }
}

@Composable
private fun BoxScope.TaskSheet(row: TaskRow, dismiss: () -> Unit, pick: (TaskAction) -> Unit) {
    Sheet(dismiss) {
        Mono("${row.id} · ${row.title}", Palette.fg, 14, bold = true, lines = 3)
        taskActions(row).forEach { SheetAction(if (it == TaskAction.Reopen) "↺" else if (it == TaskAction.Done) "✓" else "✗", it.label) { pick(it) } }
    }
}

@Composable
private fun BoxScope.BlockSheet(source: AppSource, workspace: Id, row: TaskRow, dismiss: () -> Unit, answer: (Result<Unit>) -> Unit) {
    var text by remember(row.id) { mutableStateOf(TextFieldValue("")) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(row.id) { focus.requestFocus() }
    val save = {
        if (text.text.isNotBlank()) {
            dismiss()
            source.editTask(workspace, TaskOp.Block, row.id, text.text.trim(), answer)
        }
    }
    Sheet(dismiss) {
        Mono("bloquear ${row.id}", Palette.fg, 14, bold = true)
        NameField(text, { text = it }, "por qué", focus) { save() }
        SheetAction("↵", "bloquear", Palette.blue, enabled = text.text.isNotBlank(), click = save)
    }
}

@Composable
private fun BoxScope.LapsSheet(dismiss: () -> Unit, start: (Int) -> Unit) {
    Sheet(dismiss) {
        Mono("encender el bucle", Palette.fg, 14, bold = true)
        Mono("El agente de este workspace recorre la lista hasta acabarla; se para al llegar a las vueltas que elijas.", Palette.dim, 12, lines = 4)
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            LOOP_LAPS.forEach { n -> Pill("$n vueltas", n == DEFAULT_LAPS) { start(n) } }
        }
    }
}

/** A loop that stopped or ended while the app is open (REQ-950): a line over the screen that opens the tasks. */
@Composable
internal fun BoxScope.TasksBanner(notice: TasksNotice, open: () -> Unit, dismiss: () -> Unit) {
    LaunchedEffect(notice) { delay(8000); dismiss() }
    Mono(
        (if (notice.good) "✓ " else "? ") + notice.text, if (notice.good) Palette.green else Palette.yellow, 12, lines = 3,
        modifier = Modifier.align(Alignment.TopCenter).fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)
            .background(Palette.sel, RoundedCornerShape(10.dp)).clickable(onClick = open).padding(12.dp),
    )
}
