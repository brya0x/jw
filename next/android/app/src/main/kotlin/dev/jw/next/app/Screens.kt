package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.Aviso
import dev.jw.next.client.ClientState
import dev.jw.next.client.HomeRow
import dev.jw.next.client.Navigator
import dev.jw.next.client.NodeSource
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

@Composable
private fun Mono(text: String, color: Color = Palette.fg, size: Int = 13, bold: Boolean = false, modifier: Modifier = Modifier, lines: Int = 1) =
    Text(
        text, color = color, fontFamily = MonoFont, fontSize = size.sp, fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
        maxLines = lines, overflow = TextOverflow.Ellipsis, modifier = modifier,
    )

@Composable
private fun MarkView(a: Attention?, count: Int) {
    if (a == null) return
    val m = a.mark()
    Row(verticalAlignment = Alignment.CenterVertically) {
        Mono(m.glyph, m.tone.color(), 15, bold = true)
        if (count > 1) Mono(" $count", Palette.dim, 11)
    }
}

@Composable
private fun Bar(title: String, subtitle: String? = null, onBack: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().background(Palette.panel).heightIn(min = 52.dp).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (onBack != null) {
            Box(Modifier.size(44.dp).clickable(onClick = onBack), contentAlignment = Alignment.Center) { Mono("‹", Palette.fg, 22) }
        }
        Column(Modifier.weight(1f)) {
            Mono(title, bold = true, size = 14)
            if (subtitle != null) Mono(subtitle, Palette.dim, 11)
        }
    }
}

@Composable
fun HomeScreen(source: NodeSource, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    val session = source.session()
    val waiting = avisos(session, state.seen)
    val onAvisos = state.screen == Screen.Avisos
    Column(Modifier.fillMaxSize()) {
        Bar(if (onAvisos) "avisos" else "jwn · ${session.name}", if (onAvisos) "lo que espera algo de ti" else null)
        Box(Modifier.weight(1f)) {
            if (onAvisos) AvisosList(waiting, nav, state, set) else WorkspaceList(homeRows(session, state.seen), nav, state, set)
        }
        Row(Modifier.fillMaxWidth().background(Palette.panel).border(1.dp, Palette.line).height(52.dp)) {
            TabCell("workspaces", "≡", !onAvisos, Modifier.weight(1f)) { set(nav.show(state, Screen.Home)) }
            TabCell("avisos" + if (waiting.isNotEmpty()) " ${waiting.size}" else "", "?", onAvisos, Modifier.weight(1f)) { set(nav.show(state, Screen.Avisos)) }
        }
    }
}

@Composable
private fun TabCell(label: String, glyph: String, on: Boolean, modifier: Modifier, click: () -> Unit) {
    Column(modifier.fillMaxSize().clickable(onClick = click), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Mono(glyph, if (on) Palette.blue else Palette.dim, 15, bold = true)
        Mono(label, if (on) Palette.blue else Palette.dim, 11)
    }
}

@Composable
private fun WorkspaceList(rows: List<HomeRow>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        item { Mono("workspaces · ${rows.size}", Palette.dim, 11, modifier = Modifier.padding(14.dp, 14.dp, 14.dp, 6.dp)) }
        items(rows, key = { it.id }) { row ->
            Column {
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable { set(nav.openWorkspace(state, row.id)) }.padding(horizontal = 14.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Column(Modifier.weight(1f)) {
                        Mono(row.name, bold = true, size = 14)
                        Mono(row.detail, Palette.dim, 11)
                    }
                    MarkView(row.go, row.unseen)
                }
                val go = row.go
                if (go != null && row.line != null) {
                    val tone = go.mark().tone
                    Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Mono(row.line!!, if (tone == Tone.Good) Palette.dim else tone.color(), 12, modifier = Modifier.weight(1f))
                        GoButton { set(nav.go(state, go)) }
                    }
                }
                Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))
            }
        }
        item { Legend() }
    }
}

@Composable
private fun GoButton(click: () -> Unit) {
    Box(
        Modifier.height(36.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).clickable(onClick = click).padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) { Mono("ir ›", Palette.fg, 12) }
}

@Composable
private fun AvisosList(items: List<Aviso>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
    if (items.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
            Mono("Nada espera. Cuando algo termine o pregunte, aparece aquí.", Palette.dim, 13, lines = 3)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(items, key = { it.workspaceId + it.attention.kind + it.attention.rev }) { item ->
            Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    MarkView(item.attention, 1)
                    Mono(item.workspace, bold = true, size = 14, modifier = Modifier.weight(1f))
                }
                Row(Modifier.fillMaxWidth().padding(top = 6.dp, start = 22.dp), verticalAlignment = Alignment.CenterVertically) {
                    Mono(item.attention.what(), Palette.fg, 12, modifier = Modifier.weight(1f))
                    GoButton { set(nav.go(state, item.attention)) }
                }
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))
        }
    }
}

@Composable
private fun Legend() {
    val items = listOf("!" to "permiso", "?" to "pregunta", "◆" to "plan", "❯" to "comando", "✓" to "agente listo", "⚑" to "mergeado", "✗" to "falló")
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(14.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        items.forEach { (g, t) -> Mono("$g $t", Palette.dim, 11) }
    }
}

@Composable
fun WorkspaceScreen(source: NodeSource, state: ClientState, screen: Screen.Workspace, nav: Navigator, set: (ClientState) -> Unit) {
    val ws = source.node(screen.id) as? Node.Workspace
    Column(Modifier.fillMaxSize()) {
        Bar(ws?.name ?: "workspace", ws?.let { (if (it.branch != null) "⎇ ${it.branch} · " else "") + it.path }, onBack = { set(nav.back(state)) })
        if (ws == null) {
            Box(Modifier.weight(1f).padding(24.dp)) { Mono("Este workspace no está en los fixtures.", Palette.dim, 13, lines = 3) }
            return@Column
        }
        PaneTabs(ws.panes, screen.pane, state) { set(nav.selectPane(state, it)) }
        val pane = screen.pane?.let { source.node(it) as? Node.Pane }
        Box(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(10.dp)) {
            if (pane == null) Mono("Sin pantalla en los fixtures.", Palette.dim, 12)
            else Mono(pane.screen.lines.joinToString("\n"), Palette.fg, 12, lines = pane.screen.lines.size.coerceAtLeast(1))
        }
        KeyRow()
        Row(Modifier.fillMaxWidth().padding(8.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).padding(12.dp)) {
            Mono("❯ solo lectura: datos de ejemplo", Palette.dim, 12)
        }
    }
}

@Composable
private fun PaneTabs(panes: List<PaneSummary>, current: String?, state: ClientState, select: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().background(Palette.panel).horizontalScroll(rememberScrollState()), verticalAlignment = Alignment.CenterVertically) {
        panes.forEach { p ->
            val on = p.id == current
            val unseen = p.attention.filterNot(state.seen::isSeen).byUrgency()
            Row(
                Modifier.heightIn(min = 44.dp).clickable { select(p.id) }.background(if (on) Palette.sel else Color.Transparent).padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Mono(p.title, if (on) Palette.fg else Palette.dim, 13, bold = on)
                MarkView(unseen.firstOrNull(), unseen.size)
            }
        }
    }
}

/** One slim line, no boxes (REQ-38). */
@Composable
private fun KeyRow() {
    Row(Modifier.fillMaxWidth().background(Palette.panel).border(1.dp, Palette.line).horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 2.dp)) {
        listOf("^␣", "esc", "tab", "↑", "↓", "←", "→", "^C").forEachIndexed { i, k ->
            Box(Modifier.defaultMinSize(minWidth = 36.dp, minHeight = 32.dp).padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                Mono(k, if (i == 0) Palette.yellow else Palette.dim, 12)
            }
        }
    }
}
