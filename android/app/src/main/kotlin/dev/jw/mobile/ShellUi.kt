package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.ShellBlock
import kotlinx.coroutines.flow.drop

/** The lines of a block's output shown before it is opened. */
private const val PREVIEW_LINES = 3

/**
 * A shell as its commands, a card each (REQ-92): the command, its folder, how it ended and how long it took, and its
 * output (the first lines, the rest when the card is opened). A running one says so and can be stopped. The strip on
 * top has the folder and the pane's name. The view stays on the last card until the person scrolls up.
 */
@Composable
fun ShellView(
    shell: ShellState, folder: String, paneName: String, onCopy: (String) -> Unit, onRerun: (String) -> Unit, onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    val list = rememberLazyListState()
    val open = remember { mutableStateMapOf<Int, Boolean>() }
    var stick by remember { mutableStateOf(true) }
    LaunchedEffect(list) { snapshotFlow { list.isScrollInProgress }.drop(1).collect { if (!it) stick = !list.canScrollForward } }
    val last = shell.blocks.lastOrNull()
    LaunchedEffect(shell.blocks.size, last?.i, last?.out, last?.running) {
        if (stick && shell.blocks.isNotEmpty()) {
            list.scrollToItem(shell.blocks.size - 1)
            list.scrollBy(1_000_000f)
        }
    }
    Column(modifier.background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("❯", color = j.green, fontSize = 12.sp, fontWeight = FontWeight.Bold)
            Text(folder, color = j.fg, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            PaneChip(paneName, j.dim)
        }
        Box(Modifier.fillMaxWidth().heightIn(min = 1.dp).background(j.line))
        if (shell.blocks.isEmpty()) {
            Text("Ningún comando todavía.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(14.dp))
            return@Column
        }
        SelectionContainer(Modifier.weight(1f)) {
            LazyColumn(
                state = list, modifier = Modifier.fillMaxWidth(), contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                items(shell.blocks, key = { it.i }) { b ->
                    BlockCard(b, open[b.i] == true, { open[b.i] = open[b.i] != true }, onCopy, onRerun, onStop)
                }
            }
        }
    }
}

@Composable
private fun BlockCard(b: ShellBlock, open: Boolean, onToggle: () -> Unit, onCopy: (String) -> Unit, onRerun: (String) -> Unit, onStop: () -> Unit) {
    val j = LocalJw.current
    val lines = b.out.split('\n').filter { b.out.isNotEmpty() }
    val shown = if (open) lines else lines.take(PREVIEW_LINES)
    Column(Modifier.fillMaxWidth().background(j.panel, RoundedCornerShape(8.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(8.dp))) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("❯", color = j.green, fontWeight = FontWeight.Bold, fontSize = 12.sp)
            Text(b.cmd, color = j.fg, fontSize = 12.sp, fontFamily = JwMono, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            val (label, color) = when {
                b.running -> "corriendo ${secs(b.ms)}" to j.yellow
                b.code == 0 -> "✓ ${duration(b.ms)}" to j.green
                b.code != null -> "✗ ${b.code}" to j.red
                else -> "interrumpido" to j.dim
            }
            Text(label, color = color, fontSize = 10.5.sp, maxLines = 1, modifier = Modifier.border(BorderStroke(1.dp, color), RoundedCornerShape(10.dp)).padding(horizontal = 7.dp, vertical = 2.dp))
        }
        Text(b.cwd, color = j.dim, fontSize = 10.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 10.dp, end = 10.dp, bottom = 6.dp))
        if (b.full) {
            Text("un programa a pantalla completa: sin salida", color = j.dim, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp))
        } else if (shown.isNotEmpty()) {
            Text(
                shown.joinToString("\n"), color = j.dim, fontSize = 11.5.sp, fontFamily = JwMono,
                modifier = Modifier.fillMaxWidth().background(j.bg).padding(10.dp),
            )
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (lines.size > PREVIEW_LINES || b.lines > lines.size) {
                BlockButton(if (open) "ver menos" else "ver todo (${maxOf(b.lines, lines.size)} líneas)", onClick = onToggle)
            }
            if (b.out.isNotEmpty()) BlockButton("copiar salida") { onCopy(b.out) }
            if (b.running) BlockButton("^C parar", j.red, onStop) else if (b.cmd.isNotBlank()) BlockButton("repetir") { onRerun(b.cmd) }
        }
    }
}

@Composable
private fun BlockButton(label: String, color: androidx.compose.ui.graphics.Color = LocalJw.current.dim, onClick: () -> Unit) {
    val j = LocalJw.current
    Text(
        label, color = color, fontSize = 11.sp,
        modifier = Modifier.heightIn(min = 36.dp).clickable(role = Role.Button, onClick = onClick)
            .border(BorderStroke(1.dp, j.line), RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 9.dp),
    )
}

private fun secs(ms: Long) = "${ms / 1000} s"

/** How long a command took: under a second in ms, else in seconds with a decimal. */
internal fun duration(ms: Long): String = if (ms < 1000) "$ms ms" else "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0)
