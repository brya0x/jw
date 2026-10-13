package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.flow.drop
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.animation.core.animateFloat
import androidx.compose.ui.draw.drawBehind
import kotlinx.coroutines.delay
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.ChatBlock

/**
 * An agent's conversation as blocks instead of its screen (REQ-90): what the person said and Claude answered, its
 * thoughts as one dim line, and each tool call as a card that opens to its command, diff or output. The strip on top
 * has the name the person gave the conversation in Claude (`/rename`), under it the title Claude gave it, and the
 * pane's name. The view stays on the last block until the person scrolls up.
 */
@Composable
fun ChatView(
    chat: ChatState, fallbackTitle: String, paneName: String, waiting: Boolean, working: Boolean = false, onStop: () -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    val list = rememberLazyListState()
    val open = remember { mutableStateMapOf<Int, Boolean>() }
    var stick by remember { mutableStateOf(true) }
    // Where the person leaves the list decides whether it follows new blocks. The first reading (nothing scrolled yet,
    // the list still at its top) says nothing about the person, so it is skipped.
    LaunchedEffect(list) { snapshotFlow { list.isScrollInProgress }.drop(1).collect { if (!it) stick = !list.canScrollForward } }
    val last = chat.blocks.lastOrNull()
    // After the blocks: the tasks still running and the live row while the agent works.
    val extras = (if (chat.bg > 0) 1 else 0) + (if (working) 1 else 0)
    LaunchedEffect(chat.blocks.size, last?.i, last?.result, extras) {
        if (stick && chat.blocks.isNotEmpty()) {
            list.scrollToItem(chat.blocks.size - 1 + extras)
            // A last block taller than the view is shown by its end, not its start.
            list.scrollBy(LAST_LINE)
        }
    }
    // The last call that has not answered is the one waiting for a permission.
    val pending = if (waiting) chat.blocks.lastOrNull { it.kind == "tool" && it.result == null }?.i else null

    Column(modifier.background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 40.dp).background(j.bg).padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("✻", color = j.magenta, fontSize = 12.sp)
            val name = chat.name
            Column(Modifier.weight(1f)) {
                Text(name ?: chat.title ?: fallbackTitle, color = j.fg, fontSize = 12.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (name != null) chat.title?.let { Text(it, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
            PaneChip(paneName, j.magenta)
        }
        Box(Modifier.fillMaxWidth().heightIn(min = 1.dp).background(j.line))
        if (chat.blocks.isEmpty()) {
            Text("Sin mensajes todavía.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(14.dp))
            return@Column
        }
        SelectionContainer(Modifier.weight(1f)) {
            LazyColumn(
                state = list, modifier = Modifier.fillMaxWidth(),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                items(chat.blocks, key = { it.i }) { b ->
                    when (b.kind) {
                        "user" -> UserBubble(b.text)
                        "text" -> Text(
                            inlineCode(b.text + if (working && b.i == last?.i) " ▍" else "", j), color = j.fg, fontSize = 12.5.sp, lineHeight = 19.sp,
                        )
                        "notice" -> NoticeRow(b)
                        "queued" -> QueuedBubble(b.text)
                        "think" -> Text("✻ pensó ${b.result ?: ""}".trim(), color = j.dim, fontSize = 11.5.sp)
                        "tool" -> ToolCard(b, open[b.i] == true, b.i == pending, running = working && b.i != pending) { open[b.i] = open[b.i] != true }
                    }
                }
                if (chat.bg > 0) item(key = "bg") { BackgroundRow(chat.bg) }
                if (working) item(key = "live") { LiveRow(chat.turn, writing = last?.kind == "text", onStop) }
            }
        }
    }
}

@Composable
private fun UserBubble(text: String) {
    val j = LocalJw.current
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Text(
            text, color = j.fg, fontSize = 12.5.sp, lineHeight = 19.sp,
            modifier = Modifier.fillMaxWidth(0.88f).background(j.sel, RoundedCornerShape(10.dp, 10.dp, 2.dp, 10.dp)).padding(horizontal = 10.dp, vertical = 8.dp),
        )
    }
}

@Composable
private fun ToolCard(b: ChatBlock, open: Boolean, pending: Boolean, running: Boolean = false, onToggle: () -> Unit) {
    val j = LocalJw.current
    Column(Modifier.fillMaxWidth().background(j.panel, RoundedCornerShape(8.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(8.dp))) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 42.dp)
                .clickable(role = Role.Button, onClickLabel = if (open) "cerrar" else "abrir", onClick = onToggle)
                .semantics { stateDescription = if (open) "abierto" else "cerrado" }.padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(b.name, color = j.blue, fontWeight = FontWeight.Bold, fontSize = 12.sp)
            Text(b.arg, color = j.fg, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            val result = b.result
            val (label, color) = when {
                pending -> "espera permiso" to j.yellow
                result == null && running -> "◌ en curso" to j.yellow
                b.error -> (result ?: "falló") to j.red
                result != null -> result to j.dim
                else -> "…" to j.dim
            }
            Text(label, color = color, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 150.dp))
            Text(if (open) "▾" else "▸", color = j.dim, fontSize = 12.sp)
        }
        val body = b.body
        if (open && body != null) {
            Box(Modifier.fillMaxWidth().heightIn(max = 170.dp).background(j.bg).verticalScroll(rememberScrollState())) {
                Text(diffColored(body, j), color = j.dim, fontSize = 11.5.sp, fontFamily = JwMono, modifier = Modifier.padding(10.dp))
            }
        }
    }
}

/** How a piece of a message is drawn: as it is, in bold (`**text**`) or as code (`` `text` ``). */
internal enum class Mark { Plain, Bold, Code }

/**
 * A message cut into the pieces [inlineCode] draws: code between backticks (nothing inside it is read), bold between double
 * asterisks, and the rest as it is. A mark that is never closed is text, so a stray `**` or backtick shows as it was written.
 */
internal fun markedPieces(text: String): List<Pair<Mark, String>> {
    val out = ArrayList<Pair<Mark, String>>()
    val plain = StringBuilder()
    fun flush() { if (plain.isNotEmpty()) { out.add(Mark.Plain to plain.toString()); plain.clear() } }
    var i = 0
    while (i < text.length) {
        val c = text[i]
        if (c == '`') {
            val end = text.indexOf('`', i + 1)
            if (end > i + 1) { flush(); out.add(Mark.Code to text.substring(i + 1, end)); i = end + 1; continue }
        } else if (c == '*' && text.startsWith("**", i)) {
            val end = text.indexOf("**", i + 2)
            if (end > i + 2 && !text[i + 2].isWhitespace()) { flush(); out.add(Mark.Bold to text.substring(i + 2, end)); i = end + 2; continue }
        }
        plain.append(c)
        i++
    }
    flush()
    return out
}

/** A message with its `code` and its **bold** drawn; the rest as it is. */
internal fun inlineCode(text: String, j: JwColors): AnnotatedString = buildAnnotatedString {
    for ((mark, part) in markedPieces(text)) {
        when (mark) {
            Mark.Code -> withStyle(SpanStyle(color = j.cyan, background = j.panel, fontFamily = JwMono)) { append(part) }
            Mark.Bold -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(part) }
            Mark.Plain -> append(part)
        }
    }
}

/** A body that is a diff: the lines `- ` in red and `+ ` in green; anything else as it is. */
internal fun diffColored(body: String, j: JwColors): AnnotatedString = buildAnnotatedString {
    body.split('\n').forEachIndexed { n, line ->
        if (n > 0) append('\n')
        val c: Color? = when {
            line.startsWith("+ ") -> j.green
            line.startsWith("- ") -> j.red
            else -> null
        }
        if (c != null) withStyle(SpanStyle(color = c)) { append(line) } else append(line)
    }
}

/** More than any list is long: a scroll by this much stops at the end. */
private const val LAST_LINE = 1_000_000f

/** The pane's name in the strip above its content: a dot of its colour and the name. */
@Composable
internal fun PaneChip(name: String, color: Color) {
    val j = LocalJw.current
    Row(Modifier.widthIn(max = 150.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
        Box(Modifier.width(7.dp).heightIn(min = 7.dp).background(color, RoundedCornerShape(4.dp)))
        Text(name, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A subagent or a background task that ended (REQ-97): a bar and an icon of the status colour, its title and what ran it. */
@Composable
private fun NoticeRow(b: ChatBlock) {
    val j = LocalJw.current
    val (icon, color, word) = when (b.name) {
        "done" -> Triple("✓", j.green, "listo")
        "stopped" -> Triple("■", j.dim, "detenida")
        else -> Triple("✕", j.red, "falló")
    }
    val what = if (b.arg == "agent") "subagente" else "tarea en segundo plano"
    val shape = RoundedCornerShape(8.dp)
    Row(
        Modifier.fillMaxWidth().background(j.panel, shape).border(BorderStroke(1.dp, j.line), shape).drawBehind { drawRect(color, size = androidx.compose.ui.geometry.Size(3.dp.toPx(), size.height)) }
            .padding(horizontal = 12.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(icon, color = color, fontSize = 14.sp)
        Column(Modifier.weight(1f)) {
            Text(b.text, color = j.fg, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOfNotNull(what, if (b.name == "done") "terminó" else word, b.result).joinToString(" · "), color = j.dim, fontSize = 10.5.sp)
        }
        Text(word, color = color, fontSize = 10.5.sp)
    }
}

/** A message the person sent while the agent works: it is the person's when the turn takes it (REQ-97). */
@Composable
private fun QueuedBubble(text: String) {
    val j = LocalJw.current
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Column(
            Modifier.fillMaxWidth(0.88f).background(j.sel, RoundedCornerShape(10.dp, 10.dp, 2.dp, 10.dp))
                .border(BorderStroke(1.dp, j.line), RoundedCornerShape(10.dp, 10.dp, 2.dp, 10.dp)).padding(horizontal = 10.dp, vertical = 8.dp),
        ) {
            Text("en cola · se envía cuando Claude termine", color = j.yellow, fontSize = 10.5.sp)
            Text(text, color = j.dim, fontSize = 12.5.sp, lineHeight = 19.sp, modifier = Modifier.padding(top = 3.dp))
        }
    }
}

@Composable
private fun BackgroundRow(n: Int) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.sel, RoundedCornerShape(10.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(10.dp)).padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text("◌", color = j.yellow, fontSize = 14.sp)
        Text(if (n == 1) "1 tarea en segundo plano" else "$n tareas en segundo plano", color = j.fg, fontSize = 12.sp)
    }
}

/**
 * The live row (REQ-96): `Pensando…` or, while Claude's text is the last block, `Escribiendo…`, with the seconds since the
 * person's message and the output tokens, and a stop that sends Esc.
 */
@Composable
private fun LiveRow(turn: dev.jw.proto.ChatTurn?, writing: Boolean, onStop: () -> Unit) {
    val j = LocalJw.current
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) { while (true) { delay(1000); now = System.currentTimeMillis() } }
    val pulse = androidx.compose.animation.core.rememberInfiniteTransition(label = "live")
    val alpha by pulse.animateFloat(
        0.35f, 1f, androidx.compose.animation.core.infiniteRepeatable(androidx.compose.animation.core.tween(1200), androidx.compose.animation.core.RepeatMode.Reverse), label = "alpha",
    )
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("✻", color = j.magenta.copy(alpha = alpha), fontSize = 16.sp)
        Text(if (writing) "Escribiendo…" else "Pensando…", color = j.magenta, fontSize = 12.5.sp)
        val secs = turn?.let { ((now - it.since) / 1000).coerceAtLeast(0) }
        val detail = listOfNotNull(secs?.let { "$it s" }, turn?.tokens?.takeIf { it > 0 }?.let { "↓ ${tokens(it)} tokens" }).joinToString(" · ")
        if (detail.isNotEmpty()) Text(detail, color = j.dim, fontSize = 11.sp, modifier = Modifier.weight(1f)) else Box(Modifier.weight(1f))
        Text(
            "■ parar", color = j.red, fontSize = 11.sp,
            modifier = Modifier.heightIn(min = 36.dp).clickable(role = Role.Button, onClickLabel = "Interrumpir con Escape", onClick = onStop)
                .background(j.panel, RoundedCornerShape(18.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(18.dp)).padding(horizontal = 12.dp, vertical = 10.dp),
        )
    }
}

/** Tokens as a short count: 1,4k. */
internal fun tokens(n: Long): String = if (n < 1000) "$n" else "%.1fk".format(java.util.Locale.ROOT, n / 1000.0).replace('.', ',')
