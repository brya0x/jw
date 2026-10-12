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
fun ChatView(chat: ChatState, fallbackTitle: String, paneName: String, waiting: Boolean, modifier: Modifier = Modifier) {
    val j = LocalJw.current
    val list = rememberLazyListState()
    val open = remember { mutableStateMapOf<Int, Boolean>() }
    var stick by remember { mutableStateOf(true) }
    // Where the person leaves the list decides whether it follows new blocks. The first reading (nothing scrolled yet,
    // the list still at its top) says nothing about the person, so it is skipped.
    LaunchedEffect(list) { snapshotFlow { list.isScrollInProgress }.drop(1).collect { if (!it) stick = !list.canScrollForward } }
    val last = chat.blocks.lastOrNull()
    LaunchedEffect(chat.blocks.size, last?.i, last?.result) {
        if (stick && chat.blocks.isNotEmpty()) {
            list.scrollToItem(chat.blocks.size - 1)
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
                        "text" -> Text(inlineCode(b.text, j), color = j.fg, fontSize = 12.5.sp, lineHeight = 19.sp)
                        "think" -> Text("✻ pensó ${b.result ?: ""}".trim(), color = j.dim, fontSize = 11.5.sp)
                        "tool" -> ToolCard(b, open[b.i] == true, b.i == pending) { open[b.i] = open[b.i] != true }
                    }
                }
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
private fun ToolCard(b: ChatBlock, open: Boolean, pending: Boolean, onToggle: () -> Unit) {
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

/** `code` between backticks drawn as code; the rest as it is. */
internal fun inlineCode(text: String, j: JwColors): AnnotatedString = buildAnnotatedString {
    text.split('`').forEachIndexed { n, part ->
        if (n % 2 == 1) withStyle(SpanStyle(color = j.cyan, background = j.panel, fontFamily = JwMono)) { append(part) } else append(part)
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
