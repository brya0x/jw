package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.answeredNote
import dev.jw.next.client.convKilo
import dev.jw.next.client.FoldRow
import dev.jw.next.client.convDone
import dev.jw.next.client.foldRows
import dev.jw.next.client.convSecs
import dev.jw.next.client.convTools
import dev.jw.next.client.char
import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.DiffLine
import dev.jw.next.contract.DiffSign
import dev.jw.next.contract.NoteTone
import dev.jw.next.contract.ToolState
import kotlinx.coroutines.delay

private val Pad = 12.dp

/** The dot of a tool or a subagent: yellow while it runs or asks, green when it worked, red when it was denied or failed (REQ-398). */
@Composable
private fun stateColor(s: ToolState): Color = when (s) {
    ToolState.Run, ToolState.Ask -> Palette.yellow
    ToolState.Ok -> Palette.green
    ToolState.Denied, ToolState.Failed -> Palette.red
}

/** The braille spinner, moving only while [on] (a running item exists), so a still conversation costs nothing. */
@Composable
internal fun spinner(on: Boolean): String {
    var frame by remember { mutableIntStateOf(0) }
    LaunchedEffect(on) { while (on) { delay(110); frame = (frame + 1) % Spin.size } }
    return Spin[frame]
}

/**
 * The conversation's items from the oldest, the newest at the bottom (REQ-440). A tap picks an item (REQ-449); a tap on a
 * tool, a subagent or a shell line also opens or closes what it printed.
 */
@Composable
internal fun ConversationItems(
    items: List<ConvItem>,
    accent: Color,
    picked: Int?,
    opened: Set<Int>,
    unfolded: Set<Int>,
    state: LazyListState,
    modifier: Modifier,
    toggleFold: (Int) -> Unit,
    pick: (Int) -> Unit,
) {
    val running = items.any { (it is ConvItem.Tool && it.state == ToolState.Run) || (it is ConvItem.Agent && it.state == ToolState.Run) || (it is ConvItem.Shell && it.state == ToolState.Run) }
    val spin = spinner(running)
    // Reversed, so the newest is at the bottom and stays there as it grows.
    // The tools of a run are one row (REQ-558); the key of a run is below zero, so it is not that of an item.
    val rows = remember(items, unfolded) { foldRows(items, unfolded).asReversed() }
    LazyColumn(modifier, state = state, reverseLayout = true, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        items(rows, key = { if (it is FoldRow.Item) it.index else -1 - (it as FoldRow.Run).run.first }) { row ->
            when (row) {
                is FoldRow.Item -> ItemView(items[row.index], accent, spin, picked == row.index, row.index in opened) { pick(row.index) }
                is FoldRow.Run -> FoldView(row) { toggleFold(row.run.first) }
            }
        }
    }
}

/** The row of a run of tools: what they did, and a tap opens or closes it (REQ-558). */
@Composable
private fun FoldView(row: FoldRow.Run, click: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = click).padding(horizontal = Pad)
            .semantics(mergeDescendants = true) { role = Role.Button; contentDescription = (if (row.open) "Cerrar: " else "Abrir: ") + row.run.title },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Mono(if (row.open) "▾" else "▸", Palette.dim, 12)
        Mono(row.run.title, Palette.dim, 12)
    }
}

@Composable
private fun ItemView(item: ConvItem, accent: Color, spin: String, picked: Boolean, open: Boolean, click: () -> Unit) {
    val bar = if (picked) Modifier.drawBehind { drawRect(accent, size = Size(3.dp.toPx(), size.height)) } else Modifier
    Box(Modifier.fillMaxWidth().then(bar).clickable(onClick = click)) {
        when (item) {
            is ConvItem.User -> UserRow(item, accent)
            is ConvItem.Text -> SelectionContainer {
                val blocks = item.blocks
                if (blocks != null) MdBlocks(blocks, Modifier.padding(horizontal = Pad))
                else Text(inline(item.text + if (item.live) " ▍" else ""), color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp, lineHeight = 19.sp, modifier = Modifier.padding(horizontal = Pad))
            }
            is ConvItem.Tool -> ToolRow(item, spin, open)
            is ConvItem.Agent -> AgentRow(item, spin, open)
            is ConvItem.Shell -> ShellRow(item, open)
            is ConvItem.Out -> Column(Modifier.padding(horizontal = Pad)) {
                item.lines.forEachIndexed { k, l -> Mono((if (k == 0) "⎿ " else "  ") + l, Palette.dim, 12, lines = 6) }
            }
            is ConvItem.Note -> Mono("⎿ ${item.text}", item.tone.color(), 12, lines = 4, modifier = Modifier.padding(horizontal = Pad))
            is ConvItem.Result -> Mono("✻ ${convDone(item)}", Palette.dim, 12, modifier = Modifier.padding(horizontal = Pad))
            is ConvItem.Divider -> Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.weight(1f).height(1.dp).background(Palette.line))
                Mono(item.text, Palette.dim, 11)
                Box(Modifier.weight(1f).height(1.dp).background(Palette.line))
            }
        }
    }
}

@Composable
private fun NoteTone?.color(): Color = when (this) {
    NoteTone.Error -> Palette.red
    NoteTone.Warn -> Palette.yellow
    null -> Palette.dim
}

/** `` `code` `` in cyan, as the prototype draws it. */
@Composable
private fun inline(text: String): AnnotatedString {
    val code = Palette.cyan
    return buildAnnotatedString {
        text.split('`').forEachIndexed { k, part ->
            if (k % 2 == 1) withStyle(SpanStyle(color = code)) { append(part) } else append(part)
        }
    }
}

/** What the person sent: `❯` in the accent on a band; `@file` and `[Imagen#n]` in cyan. */
@Composable
private fun UserRow(item: ConvItem.User, accent: Color) {
    val cyan = Palette.cyan
    val text = buildAnnotatedString {
        Regex("""(@\S+|\[Imagen#\d+])""").let { re ->
            var at = 0
            for (m in re.findAll(item.text)) {
                append(item.text.substring(at, m.range.first))
                withStyle(SpanStyle(color = cyan)) { append(m.value) }
                at = m.range.last + 1
            }
            append(item.text.substring(at))
        }
    }
    Row(Modifier.fillMaxWidth().background(Palette.sel).padding(horizontal = Pad, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Mono("❯", accent, 13, bold = true)
        Column {
            Text(text, color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp, lineHeight = 19.sp)
            if (item.images > 0) Mono("▣ ${item.images} imagen(es)", Palette.dim, 11)
        }
    }
}

@Composable
private fun ToolRow(t: ConvItem.Tool, spin: String, open: Boolean) {
    val uri = LocalUriHandler.current
    Column {
        Row(Modifier.defaultMinSize(minHeight = 40.dp).padding(horizontal = Pad, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mono(if (t.state == ToolState.Run) spin else "●", stateColor(t.state), 13, bold = t.state == ToolState.Ask)
            Mono(t.name, Palette.fg, 13, bold = true)
            Mono(t.arg, Palette.dim, 13, modifier = Modifier.weight(1f, fill = false))
        }
        val summary = when {
            t.state == ToolState.Ask -> "Esperando tu permiso"
            t.state == ToolState.Run -> "Ejecutando…"
            t.state == ToolState.Denied -> "negado"
            else -> (t.summary ?: "") + t.answeredNote()
        }
        if (summary.isNotBlank()) Mono("⎿ $summary", if (t.state == ToolState.Denied) Palette.red else Palette.dim, 12, lines = 3, modifier = Modifier.padding(start = 30.dp, end = Pad))
        t.url?.let { url ->
            Box(
                Modifier.padding(start = 30.dp, end = Pad, top = 2.dp).defaultMinSize(minHeight = 40.dp).background(Color.Transparent, RoundedCornerShape(8.dp))
                    .clickable { uri.openUri(url) }.padding(horizontal = 12.dp),
                contentAlignment = Alignment.CenterStart,
            ) { Mono("↗ Abrir", Palette.cyan, 12, bold = true) }
        }
        if (t.diff.isNotEmpty()) Diff(if (open) t.diff else t.diff.take(DIFF_SHOWN), t.diff.size - DIFF_SHOWN)
        if (open && t.output.isNotEmpty()) Lines(t.output.take(OUTPUT_SHOWN))
    }
}

private const val DIFF_SHOWN = 8
private const val OUTPUT_SHOWN = 40

/** The lines of a diff, the added ones tinted green and the removed ones red from the theme (REQ-398, REQ-452); [more] counts those not shown. */
@Composable
internal fun Diff(lines: List<DiffLine>, more: Int = 0) {
    Column(Modifier.padding(top = 2.dp).horizontalScroll(rememberScrollState())) {
        lines.forEach { l ->
            val tint = when (l.sign) {
                DiffSign.Add -> Palette.green.copy(alpha = 0.16f)
                DiffSign.Del -> Palette.red.copy(alpha = 0.16f)
                DiffSign.Same -> Color.Transparent
            }
            Text(
                "${l.n?.toString()?.padStart(4) ?: "    "} ${l.sign.char()} ${l.text}",
                color = if (l.sign == DiffSign.Same) Palette.dim else Palette.fg, fontFamily = MonoFont, fontSize = 11.5.sp, lineHeight = 16.sp, softWrap = false,
                modifier = Modifier.fillMaxWidth().background(tint).padding(horizontal = Pad),
            )
        }
        if (more > 0 && lines.size == DIFF_SHOWN) Mono("… $more líneas más · toca para abrir", Palette.dim, 11, modifier = Modifier.padding(horizontal = Pad))
    }
}

@Composable
private fun Lines(lines: List<String>) {
    Column(Modifier.padding(top = 2.dp, start = 30.dp, end = Pad).horizontalScroll(rememberScrollState())) {
        lines.forEach { Text(it, color = Palette.dim, fontFamily = MonoFont, fontSize = 11.5.sp, lineHeight = 16.sp, softWrap = false) }
    }
}

@Composable
private fun AgentRow(a: ConvItem.Agent, spin: String, open: Boolean) {
    val totals = "${convTools(a.tools)} · ${convKilo(a.tokens)} tokens · ${convSecs(a.ms)}"
    val running = a.state == ToolState.Run || a.state == ToolState.Ask
    Column(Modifier.padding(horizontal = Pad, vertical = 6.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mono(if (running) spin else "●", stateColor(a.state), 13)
            Mono("Agent", Palette.fg, 13, bold = true)
            Mono("${a.name} · ${a.description}", Palette.dim, 13, modifier = Modifier.weight(1f, fill = false))
        }
        Mono("⎿ " + if (running) listOfNotNull(a.last, totals).joinToString(" · ") else totals, if (running) Palette.yellow else Palette.dim, 12, lines = 2, modifier = Modifier.padding(start = 18.dp))
        if (open) a.steps.forEachIndexed { k, s -> Mono((if (k == a.steps.lastIndex) "└ " else "├ ") + s, Palette.dim, 11, modifier = Modifier.padding(start = 30.dp)) }
    }
}

@Composable
private fun ShellRow(s: ConvItem.Shell, open: Boolean) {
    val edge = if (s.state == ToolState.Failed) Palette.red else Palette.green
    Row(Modifier.height(IntrinsicSize.Min).fillMaxWidth()) {
        Box(Modifier.width(3.dp).fillMaxHeight().background(edge))
        Column(Modifier.padding(start = 9.dp, end = Pad, top = 6.dp, bottom = 6.dp)) {
            Mono("! ${s.command}", Palette.fg, 13, bold = true, lines = 3)
            val shown = if (open) s.output.take(OUTPUT_SHOWN) else s.output.take(6)
            shown.forEach { Text(it, color = Palette.fg, fontFamily = MonoFont, fontSize = 12.sp, lineHeight = 17.sp) }
            if (s.output.size > shown.size) Mono("… ${s.output.size - shown.size} líneas más · toca para abrir", Palette.dim, 11)
            if (s.state != ToolState.Run) Mono("⎿ la salida irá como contexto en tu próximo mensaje", Palette.dim, 11)
        }
    }
}

