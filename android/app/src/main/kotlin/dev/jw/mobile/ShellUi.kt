package dev.jw.mobile

import android.graphics.Typeface
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.res.ResourcesCompat
import dev.jw.proto.ShellBlock
import kotlinx.coroutines.flow.drop
import kotlin.math.floor

/** The rows of a block's output in one piece of the list: a canvas this tall is never too big for the screen. */
private const val CHUNK_ROWS = 60

/** What of a block's output is laid out for the view: the rows of its text, and which output and width they are for. */
private class Laid(val revision: Int, val viewCols: Int, val text: PaneText?, val rows: List<VRow>)

private sealed interface Piece {
    class Header(val block: ShellBlock) : Piece
    class Chunk(val block: ShellBlock, val laid: Laid, val from: Int, val to: Int) : Piece
    class Note(val block: ShellBlock, val text: String) : Piece
    object Gap : Piece
}

/**
 * A shell as its commands, as Warp draws them (REQ-92): a card each, from the bottom up, so the newest sits right above the input;
 * the command, how it ended and how long it took, and its output complete, with its colours, laid out for the phone's width. A
 * command that fails has a red edge; one that runs says so and can be stopped. The view stays on the newest until the person
 * scrolls up.
 */
@Composable
fun ShellView(
    shell: ShellState, rev: Long, outOf: (Int) -> BlockOut?, folder: String, paneName: String,
    onCopy: (String) -> Unit, onRerun: (String) -> Unit, onStop: () -> Unit, modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    val ctx = LocalContext.current
    val density = LocalDensity.current
    val regular = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_regular) ?: Typeface.MONOSPACE }
    val bold = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_bold) ?: Typeface.DEFAULT_BOLD }
    val colors = remember(j) { TermColors(j) }
    val list = rememberLazyListState()
    var stick by remember { mutableStateOf(true) }
    // At the newest unless the person went up: the first reading (the list not scrolled yet) says nothing of them.
    LaunchedEffect(list) { snapshotFlow { list.isScrollInProgress }.drop(1).collect { if (!it) stick = list.firstVisibleItemIndex <= 1 } }

    Column(modifier.background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("❯", color = j.green, fontSize = 12.sp, fontWeight = FontWeight.Bold)
            Text(folder, color = j.fg, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            PaneChip(paneName, j.dim)
        }
        androidx.compose.foundation.layout.Box(Modifier.fillMaxWidth().heightIn(min = 1.dp).background(j.line))
        if (shell.blocks.isEmpty()) {
            Text("Escribe un comando abajo.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(14.dp))
            return@Column
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val sizePx = with(density) { TEXT_SIZE_SP.sp.toPx() }
            val metrics = remember(regular, sizePx) { CellMetrics.of(regular, sizePx) }
            val painter = remember(colors, regular, bold, sizePx) { RowPainter(colors, j, regular, bold, sizePx) }
            val viewCols = floor((with(density) { maxWidth.toPx() } - 2 * with(density) { PAD.dp.toPx() }) / metrics.cw).toInt().coerceAtLeast(1)
            val laid = remember { HashMap<Int, Laid>() }
            val pieces = remember(shell.blocks, rev, viewCols) {
                // A block's output is laid out again only when it changed or the width did; the oldest first, the list reversed below.
                laid.keys.retainAll(shell.blocks.map { it.i }.toSet())
                val out = ArrayList<Piece>()
                for (b in shell.blocks) {
                    out.add(Piece.Header(b))
                    val o = outOf(b.i)
                    if (b.full) { out.add(Piece.Note(b, "un programa a pantalla completa: sin salida")); out.add(Piece.Gap); continue }
                    if (b.truncated) out.add(Piece.Note(b, "… el principio de la salida se recortó"))
                    if (o != null) {
                        val cur = laid[b.i]
                        val l = if (cur != null && cur.revision == o.revision && cur.viewCols == viewCols) cur else {
                            val t = o.text()
                            Laid(o.revision, viewCols, t, wrapView(t, viewCols, false).rows).also { laid[b.i] = it }
                        }
                        // A command that printed nothing has no body.
                        if (l.rows.any { it.cols.isNotEmpty() }) {
                            for (from in l.rows.indices step CHUNK_ROWS) out.add(Piece.Chunk(b, l, from, minOf(from + CHUNK_ROWS, l.rows.size)))
                        }
                    }
                    out.add(Piece.Gap)
                }
                out.reversed()
            }
            val newest = shell.blocks.last()
            LaunchedEffect(shell.blocks.size, newest.running, rev) { if (stick) list.scrollToItem(0) }
            LazyColumn(
                state = list, reverseLayout = true, modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(horizontal = PAD.dp, vertical = 8.dp),
            ) {
                items(pieces.size) { k ->
                    when (val p = pieces[k]) {
                        is Piece.Header -> BlockHeader(p.block, outOf(p.block.i), onCopy, onRerun, onStop)
                        is Piece.Chunk -> {
                            val n = p.to - p.from
                            Canvas(Modifier.fillMaxWidth().height(with(density) { (n * metrics.ch).toDp() }).background(j.bg)) {
                                p.laid.text?.let { t ->
                                    drawIntoCanvas { c ->
                                        for (r in 0 until n) {
                                            val row = p.laid.rows[p.from + r]
                                            painter.draw(c.nativeCanvas, t.line(row.line), row.cols, r * metrics.ch, metrics)
                                        }
                                    }
                                }
                            }
                        }
                        is Piece.Note -> Text(p.text, color = j.dim, fontSize = 11.sp, modifier = Modifier.fillMaxWidth().background(j.bg).padding(horizontal = 4.dp, vertical = 4.dp))
                        Piece.Gap -> Spacer(Modifier.height(10.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun BlockHeader(b: ShellBlock, out: BlockOut?, onCopy: (String) -> Unit, onRerun: (String) -> Unit, onStop: () -> Unit) {
    val j = LocalJw.current
    val failed = !b.running && b.code != null && b.code != 0
    Column(
        Modifier.fillMaxWidth().background(j.panel, RoundedCornerShape(8.dp))
            .border(BorderStroke(if (failed) 1.5.dp else 1.dp, if (failed) j.red.copy(alpha = 0.7f) else j.line), RoundedCornerShape(8.dp))
            .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("❯", color = j.green, fontWeight = FontWeight.Bold, fontSize = 12.sp)
            Text(b.cmd, color = j.fg, fontSize = 12.sp, fontFamily = JwMono, maxLines = 4, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            val (label, color) = when {
                b.running -> "corriendo ${b.ms / 1000} s" to j.yellow
                b.code == 0 -> "✓ ${duration(b.ms)}" to j.green
                b.code != null -> "✗ ${b.code}" to j.red
                else -> "interrumpido" to j.dim
            }
            Text(label, color = color, fontSize = 10.5.sp, maxLines = 1, modifier = Modifier.border(BorderStroke(1.dp, color), RoundedCornerShape(10.dp)).padding(horizontal = 7.dp, vertical = 2.dp))
        }
        Text(b.cwd, color = j.dim, fontSize = 10.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val text = out?.text()?.let { t -> t.read { p -> (0 until p.rows).joinToString("\n") { p.line(it).text() } }.trimEnd() } ?: ""
            if (text.isNotEmpty()) BlockButton("copiar salida") { onCopy(text) }
            if (b.running) BlockButton("^C parar", j.red, onStop) else if (b.cmd.isNotBlank()) BlockButton("repetir") { onRerun(b.cmd) }
        }
    }
}

@Composable
private fun BlockButton(label: String, color: Color = LocalJw.current.dim, onClick: () -> Unit) {
    val j = LocalJw.current
    Text(
        label, color = color, fontSize = 11.sp,
        modifier = Modifier.heightIn(min = 36.dp).clickable(role = Role.Button, onClick = onClick)
            .border(BorderStroke(1.dp, j.line), RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 9.dp),
    )
}

/** How long a command took: under a second in ms, else in seconds with a decimal. */
internal fun duration(ms: Long): String = if (ms < 1000) "$ms ms" else "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0)

private const val TEXT_SIZE_SP = 11f
private const val PAD = 12
