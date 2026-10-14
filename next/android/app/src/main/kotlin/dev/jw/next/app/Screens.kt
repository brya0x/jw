@file:OptIn(ExperimentalFoundationApi::class)

package dev.jw.next.app

import android.content.ClipData
import androidx.compose.foundation.background
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.Aviso
import dev.jw.next.client.CloseSheet
import dev.jw.next.client.ClientState
import dev.jw.next.client.HomeRow
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.Tone
import dev.jw.next.client.avisos
import dev.jw.next.client.byUrgency
import dev.jw.next.client.closeSheet
import dev.jw.next.client.homeRows
import dev.jw.next.client.shown
import dev.jw.next.client.shownName
import dev.jw.next.client.mark
import dev.jw.next.client.outputText
import dev.jw.next.client.shortPath
import dev.jw.next.client.what
import dev.jw.next.contract.Attention
import dev.jw.next.contract.BlocksMode
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.PaneSummary
import kotlinx.coroutines.launch

private val Gutter = 16.dp
private val MarkColumn = 28.dp

@Composable
internal fun Mono(
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
internal fun Hairline() = Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))

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
internal fun Header(
    title: String,
    subtitle: String?,
    onBack: (() -> Unit)? = null,
    onTitleLongClick: (() -> Unit)? = null,
    action: (@Composable () -> Unit)? = null,
) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp), verticalAlignment = Alignment.CenterVertically) {
            if (onBack != null) {
                Box(Modifier.width(48.dp).height(56.dp).clickable(onClick = onBack), contentAlignment = Alignment.Center) {
                    Mono("‹", Palette.fg, 24)
                }
            } else {
                Spacer(Modifier.width(Gutter))
            }
            val hold = if (onTitleLongClick != null) Modifier.combinedClickable(onClick = {}, onLongClick = onTitleLongClick) else Modifier
            Column(Modifier.weight(1f).then(hold).padding(end = Gutter)) {
                Mono(title, bold = true, size = 15)
                if (subtitle != null) Mono(subtitle, Palette.dim, 11)
            }
            action?.invoke()
        }
        Hairline()
    }
}

/** Opens the list of PCs from Home. */
@Composable
internal fun PcButton(open: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 56.dp, minHeight = 48.dp).clickable(onClick = open), contentAlignment = Alignment.Center) {
        Mono("PCs", Palette.blue, 13, bold = true)
    }
}

@Composable
fun HomeScreen(source: AppSource, state: ClientState, nav: Navigator, pcs: (() -> Unit)? = null, set: (ClientState) -> Unit) {
    // Reading the tick is what redraws the screen when a node arrives from the daemon.
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    val waiting = avisos(session, state.seen)
    val onAvisos = state.screen == Screen.Avisos
    // A long press on a workspace: its sheet, then the field of its name (REQ-183, REQ-184).
    var menu by remember { mutableStateOf<Naming.Workspace?>(null) }
    var naming by remember { mutableStateOf<Naming?>(null) }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            if (onAvisos) Header("avisos", "lo que espera algo de ti") else Header("jwn · ${session.name}", source.status, action = pcs?.let { open -> { PcButton(open) } })
            Box(Modifier.weight(1f)) {
                if (onAvisos) AvisosList(waiting, nav, state, set) else WorkspaceList(homeRows(session, state.seen), nav, state, set) { id ->
                    session.workspaces.firstOrNull { it.id == id }?.let { menu = Naming.Workspace(it.id, it.shown()) }
                }
            }
            val current = if (onAvisos) MainTab.Avisos else MainTab.Workspaces
            MainTabBar(current, waiting.size) { nav.goTab(state, current, it, set) }
        }
        menu?.let { target -> WorkspaceMenuSheet(target, dismiss = { menu = null }) { naming = target; menu = null } }
        naming?.let { RenameSheet(source, it) { naming = null } }
    }
}

internal enum class MainTab { Workspaces, Avisos, Diff, Settings }

/** What a tap on a tab of the bar does: go there, unless it is the one that is open. */
internal fun Navigator.goTab(state: ClientState, current: MainTab, tab: MainTab, set: (ClientState) -> Unit) {
    if (tab == current) return
    when (tab) {
        MainTab.Workspaces -> set(show(state, Screen.Home))
        MainTab.Avisos -> set(show(state, Screen.Avisos))
        MainTab.Diff -> changes(state)?.let(set)
        MainTab.Settings -> set(show(state, Screen.Settings))
    }
}

/** Words like the status line of the terminal client; the open one is underlined. */
@Composable
internal fun MainTabBar(current: MainTab, waiting: Int, go: (MainTab) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsPadding()) {
        Hairline()
        Row(Modifier.fillMaxWidth().height(48.dp)) {
            TabCell("workspaces", null, current == MainTab.Workspaces, Modifier.weight(1f)) { go(MainTab.Workspaces) }
            TabCell("avisos", waiting.takeIf { it > 0 }, current == MainTab.Avisos, Modifier.weight(1f)) { go(MainTab.Avisos) }
            TabCell("± diff", null, current == MainTab.Diff, Modifier.weight(1f)) { go(MainTab.Diff) }
            TabCell("ajustes", null, current == MainTab.Settings, Modifier.weight(1f)) { go(MainTab.Settings) }
        }
    }
}

@Composable
private fun TabCell(label: String, count: Int?, on: Boolean, modifier: Modifier, click: () -> Unit) {
    Column(modifier.fillMaxSize().clickable(onClick = click), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Mono(label, if (on) Palette.blue else Palette.dim, 12, bold = on)
            if (count != null) Mono("$count", Palette.yellow, 12, bold = true)
        }
    }
}

/** A dim section label with a hairline that fills the rest of the line. */
@Composable
internal fun Section(text: String) {
    Row(Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 18.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Mono(text, Palette.dim, 11)
        Spacer(Modifier.width(10.dp))
        Box(Modifier.weight(1f).height(1.dp).background(Palette.line))
    }
}

@Composable
private fun WorkspaceList(rows: List<HomeRow>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit, hold: (Id) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        item { Section("workspaces · ${rows.size}") }
        items(rows, key = { it.id }) { row ->
            Row(
                Modifier.fillMaxWidth().combinedClickable(onClick = { set(nav.openWorkspace(state, row.id)) }, onLongClick = { hold(row.id) }).padding(start = Gutter - 4.dp, end = Gutter, top = 10.dp, bottom = 8.dp),
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
    // A long press opens a sheet (REQ-182, REQ-183); its choices lead to the field of a name (REQ-184) or to asking
    // before a pane is closed (REQ-148 to REQ-150): the sheet, what the PC said, and whether it is answering.
    var paneMenu by remember(screen.id) { mutableStateOf<PaneSummary?>(null) }
    var wsMenu by remember(screen.id) { mutableStateOf(false) }
    var naming by remember(screen.id) { mutableStateOf<Naming?>(null) }
    var closing by remember(screen.id) { mutableStateOf<CloseSheet?>(null) }
    var closeError by remember(screen.id) { mutableStateOf<String?>(null) }
    var closeBusy by remember(screen.id) { mutableStateOf(false) }
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize()) {
        val title = ws?.let { shownName(it.name, it.kind, it.branch) } ?: "workspace"
        Header(title, ws?.let { (if (it.branch != null) "⎇ ${it.branch}  " else "") + it.path }, onBack = { set(nav.back(state)) }, onTitleLongClick = { if (ws != null) wsMenu = true }) {
            Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { nav.changes(state)?.let(set) }, contentAlignment = Alignment.Center) {
                Mono("±", Palette.blue, 18, bold = true)
            }
        }
        if (ws == null) {
            Box(Modifier.weight(1f).padding(Gutter)) { Mono("Abriendo el workspace…", Palette.dim, 13, lines = 3) }
            return@Column
        }
        PaneTabs(ws.panes, screen.pane, state, select = { set(nav.selectPane(state, it)) }) { paneMenu = it }
        val pane = remember(version, screen.pane) { screen.pane?.let { source.node(it) as? Node.Pane } }
        val blocksId = pane?.blocks
        LaunchedEffect(blocksId) { blocksId?.let { source.open(it) } }
        val blocks = remember(version, blocksId) { blocksId?.let { source.node(it) as? Node.Blocks } }
        var picked by remember(screen.pane) { mutableStateOf<Long?>(null) }
        val clipboard = LocalClipboard.current
        val scope = rememberCoroutineScope()
        val copy = { text: String -> scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("jwn", text))) }; Unit }
        val block = blocks?.blocks?.firstOrNull { it.id == picked }
        val shown = blocks?.takeIf { it.mode == BlocksMode.Blocks }
        // A shell with blocks that have not arrived: its screen would show for a moment and then give way to them.
        val waitingBlocks = blocksId != null && blocks == null
        if (shown != null) {
            BlocksList(shown.blocks, picked, Modifier.weight(1f)) { picked = it }
            if (block != null) {
                BlockActions(
                    canRepeat = source.writable,
                    copyCmd = { copy(block.cmd) },
                    copyOut = { copy(block.outputText()) },
                    repeat = {
                        screen.pane?.let { source.write(it, block.cmd + "\r") }
                        picked = null
                    },
                )
            }
        } else {
            if (blocks != null) FullScreenBanner(pane?.running?.argv?.firstOrNull())
            Box(
                Modifier.weight(1f).fillMaxWidth().background(Palette.bg).verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState()).padding(horizontal = Gutter - 4.dp, vertical = 10.dp),
            ) {
                if (pane == null || waitingBlocks) PaneSkeleton()
                else Text(
                    terminalText(pane.screen.lines, pane.screen.cursor.row, pane.screen.cursor.col, pane.screen.cursor.visible),
                    fontFamily = MonoFont, fontSize = 12.sp, lineHeight = 18.sp, color = Palette.fg, softWrap = false,
                )
            }
        }
        val last = shown?.blocks?.lastOrNull()
        Composer(
            enabled = source.writable && screen.pane != null,
            running = last?.running == true,
            chips = if (shown == null) null else {
                {
                    if (last?.running == true) Chip("◌ ${last.cmd}", Palette.yellow)
                    else {
                        pane?.let { Chip(shortPath(it.cwd), Palette.fg) }
                        ws.branch?.let { Chip("⎇ $it", Palette.magenta) }
                    }
                }
            },
            stop = { screen.pane?.let { source.write(it, "\u0003") } },
        ) { line ->
            screen.pane?.let { source.write(it, line + "\r") }
        }
    }
    paneMenu?.let { held ->
        PaneMenuSheet(held, dismiss = { paneMenu = null }, rename = { naming = Naming.Pane(held.id, held.title); paneMenu = null }) {
            closeError = null
            closing = closeSheet(source, held.id)
            paneMenu = null
        }
    }
    if (wsMenu && ws != null) {
        val target = Naming.Workspace(ws.id, shownName(ws.name, ws.kind, ws.branch))
        WorkspaceMenuSheet(target, dismiss = { wsMenu = false }) { naming = target; wsMenu = false }
    }
    naming?.let { RenameSheet(source, it) { naming = null } }
    closing?.let { sheet ->
        ClosePaneSheet(sheet, closeError, closeBusy, cancel = { closing = null }) {
            closeBusy = true
            closeError = null
            source.closePane(sheet.pane) { result ->
                closeBusy = false
                result.onSuccess { closing = null }.onFailure { closeError = it.message ?: "el PC no pudo cerrar el panel" }
            }
        }
    }
    }
}

/** The question before a pane is closed: it always asks, and says the workspace stays (REQ-148, REQ-149). */
@Composable
private fun BoxScope.ClosePaneSheet(sheet: CloseSheet, error: String?, busy: Boolean, cancel: () -> Unit, confirm: () -> Unit) {
    Sheet(cancel) {
        Mono(sheet.heading, Palette.fg, 14, bold = true, lines = 2)
        Mono(sheet.body, Palette.dim, 12, lines = 6)
        if (error != null) Mono(error, Palette.red, 12, lines = 4)
        SheetAction("×", if (busy) "cerrando…" else "cerrar el panel", Palette.red, enabled = !busy, click = confirm)
        Mono("cancelar", Palette.fg, 14, modifier = Modifier.fillMaxWidth().clickable(onClick = cancel).padding(vertical = 12.dp))
    }
}

/** What a long press on a pane's tab offers (REQ-182). */
@Composable
private fun BoxScope.PaneMenuSheet(pane: PaneSummary, dismiss: () -> Unit, rename: () -> Unit, close: () -> Unit) {
    Sheet(dismiss) {
        Mono("panel «${pane.title}»", Palette.fg, 14, bold = true, lines = 2)
        SheetAction("✎", "nombrar el panel", click = rename)
        SheetAction("×", "cerrar el panel", Palette.red, click = close)
    }
}

/** What a long press on a workspace offers (REQ-183): its name; closing it is the PC's. */
@Composable
internal fun BoxScope.WorkspaceMenuSheet(workspace: Naming.Workspace, dismiss: () -> Unit, rename: () -> Unit) {
    Sheet(dismiss) {
        Mono("workspace «${workspace.shown}»", Palette.fg, 14, bold = true, lines = 2)
        SheetAction("✎", "renombrar el workspace", click = rename)
        Mono("Cerrar un workspace o quitar un worktree se hace en el PC.", Palette.dim, 12, lines = 3)
    }
}

/** What the pane looks like before its document arrives: grey bars where the lines will be, breathing. */
@Composable
private fun PaneSkeleton() {
    val alpha by rememberInfiniteTransition(label = "skeleton").animateFloat(
        initialValue = 0.25f,
        targetValue = 0.6f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Reverse),
        label = "alpha",
    )
    Column(Modifier.fillMaxWidth().semantics { contentDescription = "Abriendo el panel" }, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        listOf(0.55f, 0.8f, 0.4f, 0.65f, 0.3f).forEach { w ->
            Box(Modifier.fillMaxWidth(w).height(12.dp).background(Palette.line.copy(alpha = alpha), RoundedCornerShape(4.dp)))
        }
    }
}

/** A program with the whole screen (nvim) takes the pane; the blocks come back when it exits. */
@Composable
private fun FullScreenBanner(program: String?) {
    Row(Modifier.fillMaxWidth().height(36.dp).background(Palette.sel).padding(horizontal = Gutter), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Mono(program ?: "programa", Palette.yellow, 11)
        Mono("ocupa toda la pantalla · los bloques vuelven al salir", Palette.dim, 11)
    }
}

/** The line a person types for the shell; Enter (or the button) sends it with a `\r`. */
@Composable
private fun Composer(
    enabled: Boolean,
    running: Boolean = false,
    chips: (@Composable () -> Unit)? = null,
    stop: () -> Unit = {},
    send: (String) -> Unit,
) {
    var text by remember { mutableStateOf("") }
    val go = { if (enabled) { send(text); text = "" } }
    // The field rides up with the keyboard: the app draws edge to edge, so nothing else moves it.
    Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsPadding().imePadding()) {
        if (chips != null) {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = Gutter, end = Gutter, top = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) { chips() }
        }
        Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Mono("❯", if (enabled) Palette.green else Palette.dim, 14, bold = true)
            Spacer(Modifier.width(10.dp))
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                enabled = enabled,
                singleLine = true,
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp),
                cursorBrush = SolidColor(Palette.blue),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(onSend = { go() }),
                modifier = Modifier.weight(1f).heightIn(min = 48.dp).wrapContentHeight(Alignment.CenterVertically),
                decorationBox = { inner ->
                    if (text.isEmpty()) {
                        val hint = when {
                            !enabled -> "solo lectura: datos de ejemplo"
                            running -> "lo que escribas va al programa"
                            else -> "escribe un comando"
                        }
                        Mono(hint, Palette.dim, 13)
                    }
                    inner()
                },
            )
            if (running && enabled) {
                Box(Modifier.size(48.dp).clickable(onClick = stop), contentAlignment = Alignment.Center) {
                    Mono("■", Palette.red, 18, bold = true)
                }
            }
            Box(Modifier.size(48.dp).clickable(enabled = enabled, onClick = go), contentAlignment = Alignment.Center) {
                Mono("↵", if (enabled) Palette.blue else Palette.dim, 16, bold = true)
            }
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
private fun PaneTabs(panes: List<PaneSummary>, current: String?, state: ClientState, select: (String) -> Unit, hold: (PaneSummary) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
            panes.forEach { p ->
                val on = p.id == current
                val unseen = p.attention.filterNot(state.seen::isSeen).byUrgency()
                Column(Modifier.combinedClickable(onClick = { select(p.id) }, onLongClick = { hold(p) })) {
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
