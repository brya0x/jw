package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.Aviso
import dev.jw.next.client.ClientState
import dev.jw.next.client.HomeRow
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.Tone
import dev.jw.next.client.avisos
import dev.jw.next.client.byUrgency
import dev.jw.next.client.homeRows
import dev.jw.next.client.mark
import dev.jw.next.client.what
import dev.jw.next.contract.Attention
import dev.jw.next.contract.Node
import dev.jw.next.contract.PaneSummary

private val Gutter = 16.dp
private val MarkColumn = 28.dp

@Composable
private fun Mono(
    text: String,
    color: Color = Palette.fg,
    size: Int = 13,
    bold: Boolean = false,
    modifier: Modifier = Modifier,
    lines: Int = 1,
) = Text(
    text,
    color = color,
    fontFamily = MonoFont,
    fontSize = size.sp,
    fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
    maxLines = lines,
    overflow = TextOverflow.Ellipsis,
    modifier = modifier,
)

@Composable
private fun Hairline() = Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))

/** The glyph column: one fixed width so the marks of every row line up. */
@Composable
private fun MarkColumn(a: Attention?, count: Int) {
    Column(Modifier.width(MarkColumn), horizontalAlignment = Alignment.CenterHorizontally) {
        if (a != null) {
            val m = a.mark()
            Mono(m.glyph, m.tone.color(), 17, bold = true)
            if (count > 1) Mono("$count", Palette.dim, 10)
        }
    }
}

/** What an attention says, tinted only when it needs a decision or went wrong. */
private fun Attention.lineColor(): Color = when (val tone = mark().tone) {
    Tone.Good, Tone.Dim, Tone.Merged -> Palette.dim
    else -> tone.color()
}

@Composable
private fun Header(title: String, subtitle: String?, onBack: (() -> Unit)? = null) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp), verticalAlignment = Alignment.CenterVertically) {
            if (onBack != null) {
                Box(Modifier.width(48.dp).height(56.dp).clickable(onClick = onBack), contentAlignment = Alignment.Center) {
                    Mono("‹", Palette.fg, 24)
                }
            } else {
                Spacer(Modifier.width(Gutter))
            }
            Column(Modifier.weight(1f).padding(end = Gutter)) {
                Mono(title, bold = true, size = 15)
                if (subtitle != null) Mono(subtitle, Palette.dim, 11)
            }
        }
        Hairline()
    }
}

@Composable
fun HomeScreen(source: AppSource, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    // Reading the tick is what redraws the screen when a node arrives from the daemon.
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    val waiting = avisos(session, state.seen)
    val onAvisos = state.screen == Screen.Avisos
    Column(Modifier.fillMaxSize()) {
        if (onAvisos) Header("avisos", "lo que espera algo de ti") else Header("jwn · ${session.name}", source.status)
        Box(Modifier.weight(1f)) {
            if (onAvisos) AvisosList(waiting, nav, state, set) else WorkspaceList(homeRows(session, state.seen), nav, state, set)
        }
        TabBar(onAvisos, waiting.size, { set(nav.show(state, Screen.Home)) }, { set(nav.show(state, Screen.Avisos)) })
    }
}

/** Two words like the status line of the terminal client; the open one is underlined. */
@Composable
private fun TabBar(onAvisos: Boolean, waiting: Int, home: () -> Unit, avisos: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsPadding()) {
        Hairline()
        Row(Modifier.fillMaxWidth().height(48.dp)) {
            TabCell("workspaces", null, !onAvisos, Modifier.weight(1f), home)
            TabCell("avisos", waiting.takeIf { it > 0 }, onAvisos, Modifier.weight(1f), avisos)
        }
    }
}

@Composable
private fun TabCell(label: String, count: Int?, on: Boolean, modifier: Modifier, click: () -> Unit) {
    Column(modifier.fillMaxSize().clickable(onClick = click), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Mono(label, if (on) Palette.blue else Palette.dim, 13, bold = on)
            if (count != null) Mono("$count", Palette.yellow, 12, bold = true)
        }
    }
}

/** A dim section label with a hairline that fills the rest of the line. */
@Composable
private fun Section(text: String) {
    Row(Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 18.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Mono(text, Palette.dim, 11)
        Spacer(Modifier.width(10.dp))
        Box(Modifier.weight(1f).height(1.dp).background(Palette.line))
    }
}

@Composable
private fun WorkspaceList(rows: List<HomeRow>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        item { Section("workspaces · ${rows.size}") }
        items(rows, key = { it.id }) { row ->
            Row(
                Modifier.fillMaxWidth().clickable { set(nav.openWorkspace(state, row.id)) }.padding(start = Gutter - 4.dp, end = Gutter, top = 10.dp, bottom = 8.dp),
                verticalAlignment = Alignment.Top,
            ) {
                MarkColumn(row.go, row.unseen)
                Column(Modifier.weight(1f).padding(start = 4.dp)) {
                    Mono(row.name, bold = true, size = 14)
                    Mono(row.detail, Palette.dim, 11, modifier = Modifier.padding(top = 2.dp))
                    val go = row.go
                    if (go != null && row.line != null) {
                        Row(Modifier.fillMaxWidth().padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                            Mono(row.line!!, go.lineColor(), 12, modifier = Modifier.weight(1f))
                            GoControl { set(nav.go(state, go)) }
                        }
                    }
                }
            }
            Hairline()
        }
        item { Legend() }
    }
}

/** Text with a chevron; a border would shout over the row, and a tall target would open a gap under every message. */
@Composable
private fun GoControl(click: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 52.dp, minHeight = 32.dp).clickable(onClick = click).padding(start = 12.dp), contentAlignment = Alignment.CenterEnd) {
        Mono("ir ›", Palette.blue, 13, bold = true)
    }
}

@Composable
private fun AvisosList(items: List<Aviso>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
    if (items.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(Gutter), contentAlignment = Alignment.Center) {
            Mono("Nada espera. Cuando algo termine o pregunte, aparece aquí.", Palette.dim, 13, lines = 4)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(items, key = { it.workspaceId + it.attention.kind + it.attention.rev }) { item ->
            Row(Modifier.fillMaxWidth().clickable { set(nav.go(state, item.attention)) }.padding(start = Gutter - 4.dp, end = Gutter, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                MarkColumn(item.attention, 1)
                Column(Modifier.weight(1f).padding(start = 4.dp)) {
                    Mono(item.workspace, bold = true, size = 14)
                    Mono(item.attention.what(), item.attention.lineColor(), 12, modifier = Modifier.padding(top = 2.dp))
                }
                GoControl { set(nav.go(state, item.attention)) }
            }
            Hairline()
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Legend() {
    val items = listOf("!" to "permiso", "?" to "pregunta", "◆" to "plan", "❯" to "comando", "✓" to "agente listo", "⚑" to "mergeado", "✗" to "falló")
    FlowRow(
        Modifier.fillMaxWidth().padding(Gutter),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) { items.forEach { (g, t) -> Mono("$g $t", Palette.dim, 11) } }
}

@Composable
fun WorkspaceScreen(source: AppSource, state: ClientState, screen: Screen.Workspace, nav: Navigator, set: (ClientState) -> Unit) {
    val version = source.tick.intValue
    val ws = remember(version, screen.id) { source.node(screen.id) as? Node.Workspace }
    LaunchedEffect(screen.id) { source.open(screen.id) }
    LaunchedEffect(ws?.panes?.map { it.id }) {
        ws?.panes?.forEach { source.open(it.id) }
        set(nav.settle(state))
    }
    Column(Modifier.fillMaxSize()) {
        Header(ws?.name ?: "workspace", ws?.let { (if (it.branch != null) "↳ ${it.branch}  " else "") + it.path }, onBack = { set(nav.back(state)) })
        if (ws == null) {
            Box(Modifier.weight(1f).padding(Gutter)) { Mono("Abriendo el workspace…", Palette.dim, 13, lines = 3) }
            return@Column
        }
        PaneTabs(ws.panes, screen.pane, state) { set(nav.selectPane(state, it)) }
        val pane = remember(version, screen.pane) { screen.pane?.let { source.node(it) as? Node.Pane } }
        Box(
            Modifier.weight(1f).fillMaxWidth().background(Palette.bg).verticalScroll(rememberScrollState())
                .horizontalScroll(rememberScrollState()).padding(horizontal = Gutter - 4.dp, vertical = 10.dp),
        ) {
            if (pane == null) Mono("Abriendo el panel…", Palette.dim, 12)
            else Text(
                terminalText(pane.screen.lines, pane.screen.cursor.row, pane.screen.cursor.col, pane.screen.cursor.visible),
                fontFamily = MonoFont, fontSize = 12.sp, lineHeight = 18.sp, color = Palette.fg, softWrap = false,
            )
        }
        KeyRow()
        Row(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsPadding().padding(horizontal = Gutter, vertical = 14.dp)) {
            Mono("❯", Palette.dim, 13, bold = true)
            Spacer(Modifier.width(10.dp))
            Mono("solo lectura por ahora", Palette.dim, 12)
        }
    }
}

/** The screen's lines with the cursor cell drawn as a block, as a terminal would. */
private fun terminalText(lines: List<String>, row: Int, col: Int, visible: Boolean): AnnotatedString = buildAnnotatedString {
    lines.forEachIndexed { i, line ->
        if (visible && i == row) {
            val padded = line.padEnd(col + 1)
            append(padded.substring(0, col))
            withStyle(SpanStyle(background = Palette.fg, color = Palette.bg)) { append(padded[col]) }
            append(padded.substring(col + 1))
        } else {
            append(line)
        }
        if (i < lines.lastIndex) append('\n')
    }
}

/** Open pane underlined in blue; an unseen mark sits beside the name. */
@Composable
private fun PaneTabs(panes: List<PaneSummary>, current: String?, state: ClientState, select: (String) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
            panes.forEach { p ->
                val on = p.id == current
                val unseen = p.attention.filterNot(state.seen::isSeen).byUrgency()
                Column(Modifier.clickable { select(p.id) }) {
                    Row(
                        Modifier.defaultMinSize(minHeight = 44.dp).padding(horizontal = Gutter),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Mono(p.title, if (on) Palette.fg else Palette.dim, 13, bold = on)
                        unseen.firstOrNull()?.let { a -> a.mark().let { Mono(it.glyph, it.tone.color(), 14, bold = true) } }
                    }
                    Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
                }
            }
        }
        Hairline()
    }
}

/** One slim line, no boxes (REQ-38). */
@Composable
private fun KeyRow() {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Hairline()
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp)) {
            listOf("^␣", "esc", "tab", "↑", "↓", "←", "→", "^C").forEachIndexed { i, k ->
                Box(Modifier.defaultMinSize(minWidth = 38.dp, minHeight = 34.dp).padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                    Mono(k, if (i == 0) Palette.yellow else Palette.dim, 12)
                }
            }
        }
    }
}
