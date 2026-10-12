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
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.runtime.derivedStateOf
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
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
import androidx.compose.ui.graphics.compositeOver
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
    val block: ShellBlock
    class Header(override val block: ShellBlock) : Piece
    class Chunk(override val block: ShellBlock, val laid: Laid, val from: Int, val to: Int) : Piece
    class Note(override val block: ShellBlock, val text: String) : Piece
    class Tail(override val block: ShellBlock) : Piece
}

/**
 * A shell as its commands, as Warp draws them (REQ-92, REQ-94): flat blocks from the bottom up, the newest right above the
 * editor. No border or button of its own: the folder and the time dim, the command in bold, and the output complete, with its
 * colours, laid out for the phone's width. A command that fails has a red bar and tint, one that runs a yellow bar. A tap
 * chooses a block and a bar offers what can be done with it. While an output taller than the screen is read its command stays
 * at the top, and a button goes to the end. The view stays on the newest until the person scrolls up.
 */
@Composable
fun ShellView(
    shell: ShellState, rev: Long, outOf: (Int) -> BlockOut?,
    onCopy: (String) -> Unit, onRerun: (String) -> Unit, onShare: (String) -> Unit, modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    val ctx = LocalContext.current
    val density = LocalDensity.current
    val regular = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_regular) ?: Typeface.MONOSPACE }
    val bold = remember { ResourcesCompat.getFont(ctx, R.font.jetbrains_mono_bold) ?: Typeface.DEFAULT_BOLD }
    val colors = remember(j) { TermColors(j) }
    val list = rememberLazyListState()
    var stick by remember { mutableStateOf(true) }
    var chosen by remember { mutableStateOf<Int?>(null) }
    // At the newest unless the person went up: the first reading (the list not scrolled yet) says nothing of them.
    LaunchedEffect(list) { snapshotFlow { list.isScrollInProgress }.drop(1).collect { if (!it) stick = list.firstVisibleItemIndex <= 1 } }

    if (shell.blocks.isEmpty()) {
        Text("Escribe un comando abajo.", color = j.dim, fontSize = 12.sp, modifier = modifier.background(j.bg).padding(14.dp))
        return
    }
    BoxWithConstraints(modifier.background(j.bg)) {
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
                if (b.full) { out.add(Piece.Note(b, "un programa a pantalla completa: sin salida")); out.add(Piece.Tail(b)); continue }
                if (b.truncated) out.add(Piece.Note(b, "… el principio de la salida se recortó"))
                if (o != null) {
                    val cur = laid[b.i]
                    val l = if (cur != null && cur.revision == o.revision && cur.viewCols == viewCols) cur else {
                        val t = o.text()
                        Laid(o.revision, viewCols, t, ColumnGrid.rows(t, viewCols)).also { laid[b.i] = it }
                    }
                    // A command that printed nothing has no body.
                    if (l.rows.any { it.cols.isNotEmpty() }) {
                        for (from in l.rows.indices step CHUNK_ROWS) out.add(Piece.Chunk(b, l, from, minOf(from + CHUNK_ROWS, l.rows.size)))
                    }
                }
                out.add(Piece.Tail(b))
            }
            out.reversed()
        }
        val newest = shell.blocks.last()
        LaunchedEffect(shell.blocks.size, newest.running, rev) { if (stick) list.scrollToItem(0) }
        fun textOf(i: Int): String = outOf(i)?.text()?.let { t -> t.read { p -> (0 until p.rows).joinToString("\n") { p.line(it).text() } }.trimEnd() } ?: ""

        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f).fillMaxWidth()) {
                LazyColumn(state = list, reverseLayout = true, modifier = Modifier.fillMaxSize()) {
                    items(pieces.size) { k ->
                        val p = pieces[k]
                        val b = p.block
                        val tap = Modifier.clickable { chosen = if (chosen == b.i) null else b.i }
                        val bg = blockTint(b, chosen == b.i, j)
                        val bar = blockBar(b, chosen == b.i, j)
                        when (p) {
                            is Piece.Header -> BlockHead(b, bg, bar, tap)
                            is Piece.Chunk -> {
                                val n = p.to - p.from
                                Box(Modifier.fillMaxWidth().background(bg).drawBar(bar).then(tap).padding(horizontal = PAD.dp)) {
                                    Canvas(Modifier.fillMaxWidth().height(with(density) { (n * metrics.ch).toDp() })) {
                                        p.laid.text?.let { t ->
                                            drawIntoCanvas { c ->
                                                for (r in 0 until n) {
                                                    val row = p.laid.rows[p.from + r]
                                                    painter.draw(c.nativeCanvas, row.own ?: t.line(row.line), row.cols, r * metrics.ch, metrics)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            is Piece.Note -> Text(p.text, color = j.dim, fontSize = 11.sp, modifier = Modifier.fillMaxWidth().background(bg).drawBar(bar).then(tap).padding(horizontal = PAD.dp, vertical = 4.dp))
                            is Piece.Tail -> Spacer(Modifier.fillMaxWidth().height(10.dp).background(bg).drawBar(bar))
                        }
                    }
                }
                // The command of the output being read stays at the top while the output is taller than the screen.
                val top by remember(pieces) {
                    derivedStateOf {
                        val last = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: return@derivedStateOf null
                        (pieces.getOrNull(last) as? Piece.Chunk)?.takeIf { it.laid.rows.size > CHUNK_ROWS }
                    }
                }
                top?.let { c ->
                    Row(
                        Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                            .padding(horizontal = PAD.dp + 4.dp, vertical = 8.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(c.block.cmd, color = j.fg, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text("${c.laid.rows.size} líneas", color = j.dim, fontSize = 10.5.sp, maxLines = 1)
                    }
                }
                if (!stick) {
                    Text(
                        "↓ ir al final", color = j.fg, fontSize = 12.sp,
                        modifier = Modifier.align(Alignment.BottomEnd).padding(14.dp)
                            .background(j.panel, RoundedCornerShape(22.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(22.dp))
                            .clickable(role = Role.Button) { stick = true; chosen = null }.padding(horizontal = 16.dp, vertical = 13.dp),
                    )
                }
            }
            val pick = chosen?.let { i -> shell.blocks.firstOrNull { it.i == i } }
            if (pick != null) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.Center) {
                    Row(
                        Modifier.background(j.panel, RoundedCornerShape(14.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(14.dp)).padding(4.dp),
                        horizontalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        BlockAction("›_", "Comando") { onCopy(pick.cmd); chosen = null }
                        BlockAction("⧉", "Salida") { onCopy(textOf(pick.i)); chosen = null }
                        BlockAction("↻", "Repetir", enabled = pick.cmd.isNotBlank() && !pick.running) { onRerun(pick.cmd); chosen = null }
                        BlockAction("↥", "Enviar") { onShare(textOf(pick.i).ifEmpty { pick.cmd }); chosen = null }
                    }
                }
            }
        }
    }
}

/** What a block's background is: tinted red when it failed, yellow while it runs, the selection's when chosen. */
private fun blockTint(b: ShellBlock, chosen: Boolean, j: JwColors): Color = when {
    chosen -> j.sel
    b.running -> j.yellow.copy(alpha = 0.06f).compositeOver(j.bg)
    !b.running && b.code != null && b.code != 0 -> j.red.copy(alpha = 0.09f).compositeOver(j.bg)
    else -> j.bg
}

private fun blockBar(b: ShellBlock, chosen: Boolean, j: JwColors): Color? = when {
    chosen -> j.blue
    b.running -> j.yellow
    !b.running && b.code != null && b.code != 0 -> j.red
    else -> null
}

private fun Modifier.drawBar(bar: Color?): Modifier = if (bar == null) this else drawBehind { drawRect(bar, size = Size(3.dp.toPx(), size.height)) }

/** The block's dim line (folder, how it ended and how long), then its command in bold. */
@Composable
private fun BlockHead(b: ShellBlock, bg: Color, bar: Color?, tap: Modifier) {
    val j = LocalJw.current
    val (label, color) = when {
        b.running -> "corriendo ${b.ms / 1000} s" to j.yellow
        b.code == 0 -> duration(b.ms) to j.green
        b.code != null -> "salió ${b.code} · ${duration(b.ms)}" to j.red
        else -> "interrumpido" to j.dim
    }
    Column(
        Modifier.fillMaxWidth().background(bg).drawBar(bar)
            .drawBehind { drawLine(j.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) }
            .then(tap).padding(start = PAD.dp + 4.dp, end = PAD.dp + 4.dp, top = 10.dp, bottom = 6.dp),
        verticalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
            Text(b.cwd, color = j.dim, fontSize = 10.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Text(label, color = color, fontSize = 10.5.sp, maxLines = 1)
        }
        Text(b.cmd, color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono, fontWeight = FontWeight.Bold, maxLines = 6, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun BlockAction(glyph: String, label: String, enabled: Boolean = true, onClick: () -> Unit) {
    val j = LocalJw.current
    Column(
        Modifier.width(74.dp).heightIn(min = 52.dp).clickable(enabled = enabled, role = Role.Button, onClickLabel = label, onClick = onClick).padding(vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center,
    ) {
        Text(glyph, color = if (enabled) j.fg else j.dim, fontSize = 16.sp)
        Text(label, color = if (enabled) j.fg else j.dim, fontSize = 10.5.sp)
    }
}

/** How long a command took: under a second in ms, else in seconds with a decimal. */
internal fun duration(ms: Long): String = if (ms < 1000) "$ms ms" else "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0)

private const val TEXT_SIZE_SP = 11f
private const val PAD = 12
