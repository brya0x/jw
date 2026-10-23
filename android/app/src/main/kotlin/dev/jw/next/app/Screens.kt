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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.Aviso
import dev.jw.next.client.ChatAction
import dev.jw.next.client.ChatOpen
import dev.jw.next.client.AvisosTab
import dev.jw.next.client.JobsMark
import dev.jw.next.client.endNotice
import dev.jw.next.client.endedRemovals
import dev.jw.next.client.jobStates
import dev.jw.next.client.jobsMark
import dev.jw.next.client.jobsOf
import dev.jw.next.client.CloseSheet
import dev.jw.next.client.ClientState
import dev.jw.next.client.lineReference
import dev.jw.next.client.HomeItem
import dev.jw.next.client.HomeKind
import dev.jw.next.client.HomeRow
import dev.jw.next.client.MoveTo
import dev.jw.next.client.Navigator
import dev.jw.next.client.Need
import dev.jw.next.client.RemoveSheet
import dev.jw.next.client.Screen
import dev.jw.next.client.WorkspaceTarget
import dev.jw.next.client.WsAction
import dev.jw.next.client.avisos
import dev.jw.next.client.hasInk
import dev.jw.next.client.programName
import dev.jw.next.client.starting
import dev.jw.next.client.byUrgency
import dev.jw.next.client.closeSheet
import dev.jw.next.client.homeItems
import dev.jw.next.client.shown
import dev.jw.next.client.shownName
import dev.jw.next.client.mark
import dev.jw.next.client.menuTitle
import dev.jw.next.client.moveTo
import dev.jw.next.client.need
import dev.jw.next.client.removeSheet
import dev.jw.next.client.target
import dev.jw.next.client.workspaceMenu
import dev.jw.next.client.repo
import dev.jw.next.client.outputText
import dev.jw.next.client.shortPath
import dev.jw.next.client.what
import dev.jw.next.contract.Attention
import dev.jw.next.contract.BlocksMode
import dev.jw.next.contract.Id
import dev.jw.next.contract.JobState
import dev.jw.next.contract.Node
import dev.jw.next.contract.PaneSummary
import dev.jw.next.contract.ViewKind
import dev.jw.next.contract.ViewSummary
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private val Gutter = 16.dp
private val MarkColumn = 28.dp
private const val PANE_WAIT_MS = 5_000L

/** How long the field of a conversation just made, resumed or promoted is waited for before the phone stops (REQ-815). */
private const val CHAT_FIELD_WAIT_MS = 50_000L

/** How many lines the composer grows to before it scrolls. */
private const val COMPOSER_LINES = 5

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

/** Opens the search of workspaces (REQ-231). */
@Composable
private fun SearchButton(open: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = open).semantics { contentDescription = "Buscar workspace" }, contentAlignment = Alignment.Center) {
        GlyphIcon(Glyph.Search, Palette.fg)
    }
}

/** Opens the sheet of a workspace made new, as a long press on the title does. */
@Composable
private fun PlusButton(open: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = open).semantics { contentDescription = "Nuevo" }, contentAlignment = Alignment.Center) {
        Mono("+", Palette.blue, 20, bold = true)
    }
}

/** Opens the avisos; the count is what waits for the person and has not been seen on this phone (REQ-215), and the jobs have a mark of their own (REQ-659). */
@Composable
private fun BellButton(waiting: Int, jobs: JobsMark, now: Long, open: () -> Unit) {
    val said = (if (waiting > 0) "Avisos: $waiting esperando" else "Avisos") + jobs.said()
    Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = open).semantics { contentDescription = said }, contentAlignment = Alignment.Center) {
        GlyphIcon(Glyph.Bell, Palette.fg)
        JobsDot(jobs, now)
        if (waiting > 0) {
            Box(Modifier.align(Alignment.TopEnd).padding(top = 6.dp, end = 4.dp).defaultMinSize(minWidth = 16.dp).background(Palette.yellow, RoundedCornerShape(8.dp)).padding(horizontal = 4.dp), contentAlignment = Alignment.Center) {
                Mono("$waiting", Palette.bg, 10, bold = true)
            }
        }
    }
}

@Composable
private fun GearButton(open: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = open).semantics { contentDescription = "Ajustes" }, contentAlignment = Alignment.Center) {
        GlyphIcon(Glyph.Gear, Palette.fg)
    }
}

/** Opens the list of PCs from the screen that cannot find the PC (REQ-90). */
@Composable
internal fun PcButton(open: () -> Unit) {
    Box(Modifier.defaultMinSize(minWidth = 56.dp, minHeight = 48.dp).clickable(onClick = open), contentAlignment = Alignment.Center) {
        Mono("PCs", Palette.blue, 13, bold = true)
    }
}

@Composable
fun HomeScreen(source: AppSource, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    // Reading the tick is what redraws the screen when a node arrives from the daemon.
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    val waiting = avisos(session, state.seen)
    val onAvisos = state.screen == Screen.Avisos
    // The jobs: the clock moves while one runs; the tab that shows is seen; a removal that ends is said (REQ-659 to REQ-661).
    val jobsMark = jobsMark(session, state.seen)
    val now = rememberClock(session.jobs.any { it.state == JobState.Running })
    var jobOpen by remember { mutableStateOf<Id?>(null) }
    // What the session says settles the state: the jobs that are seen, and what is folded or chosen in Avisos that the PC no longer lists is forgotten (REQ-700).
    LaunchedEffect(version, state.screen, state.avisosTab, state.seen, state.folded, state.avisosFilter) {
        val next = nav.forgetUnlisted(nav.seeJobs(state))
        if (next != state) set(next)
    }
    LaunchedEffect(version) {
        source.jobStates?.let { before -> endedRemovals(before, session).firstOrNull()?.let { source.say(it.endNotice()) } }
        source.jobStates = jobStates(session)
    }
    // A long press on a workspace: its sheet, then the field of its name or the question before it is let go of (REQ-183, REQ-184, REQ-227).
    var menu by remember { mutableStateOf<WorkspaceTarget?>(null) }
    var naming by remember { mutableStateOf<Naming?>(null) }
    var removing by remember { mutableStateOf<RemoveSheet?>(null) }
    // «mover a un grupo…», the sheet of a group (a long press on its row) and a new group (REQ-698).
    var moving by remember { mutableStateOf<MoveTo?>(null) }
    var groupOpen by remember { mutableStateOf<Id?>(null) }
    var newGroup by remember { mutableStateOf(false) }
    // A long press on the title, or the `+`: a folder to open, a group, a conversation of its own (REQ-201, REQ-814); a worktree is made from a workspace (REQ-213).
    var newMenu by remember { mutableStateOf(false) }
    // The sheet of a conversation of its own (a long press on its row), taking it to a project, and the conversation just made or promoted that waits to be opened (REQ-815, REQ-817, REQ-818).
    var chatSheet by remember { mutableStateOf<Id?>(null) }
    var promoting by remember { mutableStateOf<Id?>(null) }
    var promoteError by remember { mutableStateOf<String?>(null) }
    var pendingChat by remember { mutableStateOf<ChatOpen?>(null) }
    var finding by remember { mutableStateOf(false) }
    var worktree by remember { mutableStateOf<WorktreeAt?>(null) }
    var project by remember { mutableStateOf<String?>(null) }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            if (onAvisos) {
                val ended = jobsOf(session).ended
                Header("avisos", "lo que espera algo de ti · lo que corre", onBack = { set(nav.back(state)) }, action = {
                    if (state.avisosTab == AvisosTab.Trabajos && ended.isNotEmpty()) {
                        Mono(
                            "limpiar", Palette.dim, 12,
                            modifier = Modifier.clickable { source.dismissJobs { r -> r.onFailure { source.say("✗ ${it.message ?: "el PC no pudo descartar los trabajos"}") } } }
                                .defaultMinSize(minHeight = 48.dp).padding(horizontal = 12.dp, vertical = 14.dp),
                        )
                    }
                })
            } else Header("jwn · ${session.name}", source.status, onTitleLongClick = { newMenu = true }, action = {
                SearchButton { finding = true }
                PlusButton { newMenu = true }
                BellButton(waiting.size, jobsMark, now) { set(nav.openAvisos(state)) }
                GearButton { set(nav.show(state, Screen.Settings)) }
            })
            Box(Modifier.weight(1f)) {
                if (onAvisos) AvisosBody(session, waiting, state, nav, set, now) { jobOpen = it.id } else WorkspaceList(
                    homeItems(session, state.seen, state.folded), nav, state, set, now,
                    removing = { source.say("ya se está quitando") }, create = { newMenu = true },
                    holdGroup = { groupOpen = it }, holdChat = { chatSheet = it },
                ) { id ->
                    session.workspaces.firstOrNull { it.id == id }?.let { menu = session.target(it) }
                }
            }
        }
        menu?.let { target ->
            WorkspaceMenuSheet(target, session.moveTo(target.id)?.note, dismiss = { menu = null }) { action ->
                menu = null
                choose(
                    target, action, rename = { naming = Naming.Workspace(target.id, target.shown) }, newWorktree = { worktree = WorktreeAt(it, false) },
                    existing = { worktree = WorktreeAt(it, true) }, project = { project = it }, remove = { removing = it },
                    move = { moving = session.moveTo(target.id) },
                )
            }
        }
        moving?.let { m -> MoveSheet(source, m, unfold = { set(state.copy(folded = state.folded - it)) }) { moving = null } }
        session.groups.firstOrNull { it.id == groupOpen }?.let { GroupSheet(source, session, it) { groupOpen = null } }
        if (newGroup) NewGroupSheet(source, session.groups) { newGroup = false }
        chatSheet?.let { id ->
            session.chat(id)?.let { w ->
                ChatSheet(w.name, w.chat?.last != null, dismiss = { chatSheet = null }) { action ->
                    when (action) {
                        ChatAction.Rename -> naming = Naming.Workspace(w.id, w.name)
                        ChatAction.Promote -> { promoteError = null; promoting = w.id }
                        ChatAction.Archive -> source.archiveChat(w.id) { r -> r.onFailure { source.say("✗ ${it.message ?: "el PC no pudo archivar la conversación"}") } }
                    }
                }
            }
        }
        promoting?.let { id ->
            session.chat(id)?.let { w ->
                PromoteSheet(session, w.name, promoteError, dismiss = { promoting = null; promoteError = null }) { target ->
                    promoteError = null
                    source.promoteChat(w.id, target.id) { r ->
                        r.onSuccess { p -> promoting = null; pendingChat = ChatOpen(target.id, p.pane, p.text) }
                            .onFailure { promoteError = it.message ?: "el PC no pudo llevar la conversación" }
                    }
                }
            }
        }
        OpenChatWhenShown(source, pendingChat, state, nav, set) { pendingChat = null }
        naming?.let { RenameSheet(source, it) { naming = null } }
        removing?.let { RemoveWorkspaceSheet(source, it, cancel = { removing = null }) { removing = null } }
        session.jobs.firstOrNull { it.id == jobOpen }?.let { JobSheet(source, session, it, state, nav, set, now) { jobOpen = null } }
        Notice(source)
        if (finding) FindSheet(source, state, nav, dismiss = { finding = false }) { finding = false; set(it) }
        if (newMenu) NewWorkspaceSheet(
            dismiss = { newMenu = false },
            folders = { newMenu = false; set(nav.show(state, Screen.Folders)) },
            group = { newMenu = false; newGroup = true },
            chat = {
                newMenu = false
                source.chatNew { r -> r.onSuccess { pendingChat = ChatOpen(it, null, null) }.onFailure { source.say("✗ ${it.message ?: "el PC no pudo hacer la conversación"}") } }
            },
        )
        worktree?.let { WorktreeSheet(source, it, state, nav, set) { worktree = null } }
        project?.let { ProjectDialog(source, it) { project = null } }
    }
}

/** A dim section label with a hairline that fills the rest of the line. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun Section(text: String, onLongClick: (() -> Unit)? = null, indent: Dp = 0.dp) {
    val press = if (onLongClick != null) Modifier.combinedClickable(onClick = {}, onLongClick = onLongClick) else Modifier
    Row(Modifier.fillMaxWidth().then(press).padding(start = Gutter + indent, end = Gutter, top = 18.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Mono(text, Palette.dim, 11)
        Spacer(Modifier.width(10.dp))
        Box(Modifier.weight(1f).height(1.dp).background(Palette.line))
    }
}

@Composable
private fun WorkspaceList(
    items: List<HomeItem>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit, now: Long, removing: () -> Unit, create: () -> Unit,
    holdGroup: (Id) -> Unit, holdChat: (Id) -> Unit, hold: (Id) -> Unit,
) {
    // The number before a row is its place among the rows that are drawn (REQ-697): a folded group leaves no gap.
    val numbers = remember(items) { items.filterIsInstance<HomeItem.Row>().mapIndexed { i, r -> r.row.id to i + 1 }.toMap() }
    LazyColumn(Modifier.fillMaxSize()) {
        item { Section("workspaces · ${items.count { it is HomeItem.Row }}", create) }
        items(items, key = { it.key }) { item ->
            when (item) {
                is HomeItem.Heading -> Section(item.text, indent = if (item.grouped) 22.dp else 0.dp)
                is HomeItem.Group -> GroupRow(item, toggle = { set(nav.toggleGroup(state, item.id)) }, hold = { holdGroup(item.id) })
                is HomeItem.Empty -> EmptyGroup()
                HomeItem.Ungrouped -> Section("sin grupo")
                is HomeItem.Chats -> Section("conversaciones · ${item.count}")
                is HomeItem.Chat -> ChatRow(
                    item.row, current = item.row.id == state.last, nowMs = now,
                    open = { set(nav.openWorkspace(state, item.row.id)) }, hold = { holdChat(item.row.id) },
                )
                HomeItem.NoChats -> NoChatsLine()
                is HomeItem.ArchivedLink -> ArchivedLinkRow(item.count) { set(nav.show(state, Screen.Archived)) }
                is HomeItem.Row -> WorkspaceRow(
                    item.row, number = numbers[item.row.id] ?: item.row.number, grouped = item.grouped, current = item.row.id == state.last, now = now,
                    open = { if (item.row.removing) removing() else set(nav.openWorkspace(state, item.row.id)) },
                    hold = { if (item.row.removing) removing() else hold(item.row.id) },
                    go = { a -> set(nav.go(state, a)) },
                )
            }
        }
        item { Legend(create) }
    }
}

/**
 * A row as `docs/prototypes/mobile.html` draws it: its number, `●` (a root or a folder) or `↳`
 * (a worktree, further in, REQ-224) lit when it has panes, the name over its detail, the tag
 * and the mark of its most urgent attention; under the detail, that attention as a line of
 * coloured text (REQ-247). The workspace opened last is the one with the blue bar.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun WorkspaceRow(row: HomeRow, number: Int, grouped: Boolean, current: Boolean, now: Long, open: () -> Unit, hold: () -> Unit, go: (Attention) -> Unit) {
    // A worktree is further in than its root, and what is under a group's row further in than the group (REQ-224, REQ-697).
    val indent = (if (row.kind == HomeKind.Worktree) 20.dp else 0.dp) + (if (grouped) 14.dp else 0.dp)
    val bar = Palette.blue
    Column(Modifier.fillMaxWidth().background(if (current) Palette.sel else Color.Transparent).drawBehind { if (current) drawRect(bar, size = Size(3.dp.toPx(), size.height)) }) {
        Row(
            Modifier.fillMaxWidth().defaultMinSize(minHeight = 50.dp).combinedClickable(onClick = open, onLongClick = hold).padding(start = Gutter - 2.dp + indent, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Mono("$number", Palette.dim, 11, modifier = Modifier.width(14.dp))
            Mono(if (row.kind == HomeKind.Worktree) "↳" else "●", if (row.paneCount > 0) Palette.green else Palette.dim, 14, modifier = Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Mono(row.name, if (row.removing) Palette.dim else Palette.fg, bold = true, size = 14)
                Mono(row.detail, Palette.dim, 11)
                val attention = row.go
                val line = row.line
                if (row.removing) {
                    Mono("${spinOf(now)} quitando…", Palette.blue, 12, modifier = Modifier.padding(vertical = 3.dp))
                } else if (row.removalFailed) {
                    Mono("✗ no se pudo quitar", Palette.red, 12, modifier = Modifier.padding(vertical = 3.dp))
                } else if (attention != null && line != null) {
                    Mono(line, attention.need().color(), 12, modifier = Modifier.combinedClickable(onClick = { go(attention) }, onLongClick = hold).padding(vertical = 3.dp))
                } else if (row.working) {
                    Mono("Claude trabaja", Palette.blue, 12, modifier = Modifier.padding(vertical = 3.dp))
                }
            }
            if (row.kind != HomeKind.Worktree) Tag(row.kind.label, Palette.dim)
            row.mark?.let { m ->
                Mono(m.glyph, m.tone.color(), 14, bold = true)
                if (row.unseen > 1) Mono("${row.unseen}", Palette.dim, 11)
            }
            if (row.working) CircularProgressIndicator(Modifier.size(14.dp), color = Palette.blue, strokeWidth = 2.dp)
        }
    }
}

@Composable
internal fun AvisosList(items: List<Aviso>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
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
                    // The group before the workspace, when it has one (REQ-699).
                    Row {
                        item.groupName?.let { Mono("$it › ", Palette.dim, 14) }
                        Mono(item.workspace, bold = true, size = 14)
                    }
                    Mono(item.attention.what(), item.attention.need().color(), 12, modifier = Modifier.padding(top = 2.dp))
                }
            }
            Hairline()
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Legend(create: () -> Unit) {
    val items = listOf("●" to "abierto", "!" to "permiso", "?" to "pregunta", "◆" to "plan", "❯" to "comando", "✓" to "agente listo", "⚑" to "mergeado", "✗" to "falló")
    val lines = listOf("te espera" to Need.Decide, "falló" to Need.Failed, "por revisar" to Need.Review)
    FlowRow(
        Modifier.fillMaxWidth().combinedClickable(onClick = {}, onLongClick = create).padding(Gutter),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        items.forEach { (g, t) -> Mono("$g $t", Palette.dim, 11) }
        Mono("línea:", Palette.dim, 11)
        lines.forEach { (t, need) -> Mono(t, need.color(), 11) }
    }
}

@Composable
fun WorkspaceScreen(source: AppSource, state: ClientState, screen: Screen.Workspace, nav: Navigator, set: (ClientState) -> Unit) {
    val version = source.tick.intValue
    val ws = remember(version, screen.id) { source.node(screen.id) as? Node.Workspace }
    LaunchedEffect(screen.id) { source.open(screen.id) }
    // The root folder is asked for as the workspace opens, so the files screen has it when it is tapped (REQ-636).
    LaunchedEffect(screen.id) { FileListings.preload(source, screen.id) }
    LaunchedEffect(ws?.panes?.map { it.id }, ws?.views?.map { it.id }) {
        ws?.panes?.forEach { source.open(it.id) }
        set(nav.settle(state))
    }
    // A long press opens a sheet (REQ-182, REQ-183); its choices lead to the field of a name (REQ-184) or to asking
    // before a pane is closed (REQ-148 to REQ-150): the sheet, what the PC said, and whether it is answering.
    var paneMenu by remember(screen.id) { mutableStateOf<PaneSummary?>(null) }
    var moving by remember { mutableStateOf<MoveTo?>(null) }
    var wsMenu by remember(screen.id) { mutableStateOf(false) }
    // A conversation of its own: its sheet (`⋯`, or a long press on the title) and taking it to a project (REQ-817, REQ-818). One that is archived, or deleted, from here or from the PC, leaves this screen.
    var chatSheet by remember(screen.id) { mutableStateOf(false) }
    var promoting by remember(screen.id) { mutableStateOf(false) }
    var promoteError by remember(screen.id) { mutableStateOf<String?>(null) }
    var pendingChat by remember(screen.id) { mutableStateOf<ChatOpen?>(null) }
    val chat = ws?.chat
    LaunchedEffect(chat?.archived) { if (chat?.archived == true) set(nav.back(state)) }
    // A conversation just made, resumed or promoted: the panel the PC named (else the first) is the one shown, and its field takes the focus
    // once it has its conversation, with the summary in it (REQ-815, REQ-817); the wait ends after a while whatever happens.
    val chatOpen = state.chatOpen?.takeIf { it.workspace == screen.id }
    val chatTarget = chatOpen?.let { it.pane ?: ws?.panes?.firstOrNull()?.id }
    val latestState by rememberUpdatedState(state)
    LaunchedEffect(chatOpen, version, ws?.panes?.map { it.id }) {
        if (chatOpen == null || ws == null || chatTarget == null) return@LaunchedEffect
        if (ws.panes.any { it.id == chatTarget } && screen.pane != chatTarget) set(nav.selectPane(state, chatTarget))
    }
    LaunchedEffect(chatOpen) { if (chatOpen != null) { delay(CHAT_FIELD_WAIT_MS); set(nav.spendChatOpen(latestState)) } }
    var removing by remember(screen.id) { mutableStateOf<RemoveSheet?>(null) }
    // A shell asked for: the answer, then the pane once the workspace shows it (REQ-234, REQ-235).
    var asking by remember(screen.id) { mutableStateOf(false) }
    var waitingPane by remember(screen.id) { mutableStateOf<String?>(null) }
    var addError by remember(screen.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(version, waitingPane) { waitingPane?.let { p -> nav.selectWhenShown(state, screen.id, p)?.let { set(it); waitingPane = null } } }
    // The diff or a viewer asked for as a tab (REQ-740, REQ-741): the same wait as a shell's.
    var wantedFile by remember(screen.id) { mutableStateOf<String?>(null) }
    val askView = { kind: ViewKind, path: String? ->
        if (kind == ViewKind.Viewer) wantedFile = path
        asking = true
        addError = null
        source.openView(screen.id, kind, path) { result ->
            result.onSuccess { waitingPane = it }.onFailure { addError = it.message ?: "el PC no pudo abrir el panel" }
            asking = false
        }
    }
    // A shell that ends at once takes its pane with it before the workspace lists it: the `+` must not wait for ever.
    LaunchedEffect(waitingPane) { if (waitingPane != null) { delay(PANE_WAIT_MS); waitingPane = null } }
    var worktree by remember(screen.id) { mutableStateOf<WorktreeAt?>(null) }
    var project by remember(screen.id) { mutableStateOf<String?>(null) }
    var naming by remember(screen.id) { mutableStateOf<Naming?>(null) }
    var closing by remember(screen.id) { mutableStateOf<CloseSheet?>(null) }
    var closeError by remember(screen.id) { mutableStateOf<String?>(null) }
    var closeBusy by remember(screen.id) { mutableStateOf(false) }
    var settingsAsked by remember(screen.id) { mutableIntStateOf(0) }
    // Lines picked in the viewer of a file (REQ-639): the pane of the workspace that runs a `claude` shows and writes them in
    // its field; a workspace with none has them copied.
    val draft = state.draft
    var referenceNote by remember(screen.id) { mutableStateOf<String?>(null) }
    val draftScope = rememberCoroutineScope()
    val draftClipboard = LocalClipboard.current
    LaunchedEffect(draft, version, ws?.panes?.map { it.id }) {
        if (draft == null || ws == null) return@LaunchedEffect
        val nodes = ws.panes.map { source.node(it.id) as? Node.Pane }
        val agent = nodes.indexOfFirst { it?.conversation != null }
        if (agent >= 0) {
            if (ws.panes[agent].id != screen.pane) set(nav.selectPane(state, ws.panes[agent].id))
        } else if (nodes.all { it != null }) {
            val text = lineReference(null, null, draft.path, draft.from, draft.to)
            draftScope.launch { draftClipboard.setClipEntry(ClipEntry(ClipData.newPlainText("jwn", text))) }
            referenceNote = "no hay un agente aquí: la referencia quedó copiada"
            set(nav.spendDraft(state))
        }
    }
    LaunchedEffect(referenceNote) { if (referenceNote != null) { delay(3500); referenceNote = null } }
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize()) {
        val title = ws?.let { shownName(it.name, it.kind, it.branch) } ?: "workspace"
        Header(title, ws?.let { (if (it.branch != null) "⎇ ${it.branch}  " else "") + it.path }, onBack = { set(nav.back(state)) }, onTitleLongClick = { if (ws != null) { if (ws.chat != null) chatSheet = true else wsMenu = true } }) {
            Row {
                if (chat != null) Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { chatSheet = true }.semantics { contentDescription = "Conversación" }, contentAlignment = Alignment.Center) {
                    Mono("⋯", Palette.blue, 20, bold = true)
                }
                Box(Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { set(nav.files(state, screen.id)) }.semantics { contentDescription = "Archivos" }, contentAlignment = Alignment.Center) {
                    GlyphIcon(Glyph.Folder, Palette.blue, 20)
                }
                // A conversation of its own has no git: no changes to see (REQ-796).
                if (chat == null) Box(
                    Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable {
                        ws?.views?.firstOrNull { it.kind == ViewKind.Diff }?.let { set(nav.selectPane(state, it.id)) } ?: askView(ViewKind.Diff, null)
                    }.semantics { contentDescription = "Cambios" },
                    contentAlignment = Alignment.Center,
                ) {
                    Mono("±", Palette.blue, 18, bold = true)
                }
            }
        }
        if (ws == null) {
            Box(Modifier.weight(1f).padding(Gutter)) { Mono("Abriendo el workspace…", Palette.dim, 13, lines = 3) }
            return@Column
        }
        // A pane that runs `claude` is named by its conversation, in its colour; a tap on its open tab is the settings (REQ-444).
        val tabPane = remember(version, screen.pane) { screen.pane?.let { source.node(it) as? Node.Pane } }
        val convId = tabPane?.conversation
        val tabConv = remember(version, convId) { convId?.let { source.node(it) as? Node.Conversation } }
        PaneTabs(
            ws.panes, ws.views, screen.pane, state, adding = asking || waitingPane != null,
            closeView = { v -> source.closeView(v.id, null) { r -> r.onFailure { addError = it.message ?: "el PC no pudo cerrar el panel" } } },
            select = { id -> if (id == screen.pane && tabConv != null) settingsAsked++ else set(nav.selectPane(state, id)) },
            hold = { paneMenu = it },
            named = tabConv?.let { c -> screen.pane?.let { it to ("claude · " + c.name) } },
            underline = tabConv?.let { Palette.accent(it.accent) },
        ) {
            asking = true
            addError = null
            source.newPane(screen.id) { result ->
                result.onSuccess { waitingPane = it }.onFailure { addError = it.message ?: "el PC no pudo abrir el panel" }
                asking = false
            }
        }
        addError?.let { Mono(it, Palette.red, 12, lines = 3, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp)) }
        referenceNote?.let { Mono(it, Palette.yellow, 12, lines = 2, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp)) }
        // The diff or a viewer is a tab like a pane, after the panes (REQ-740).
        val tabView = ws.views.firstOrNull { it.id == screen.pane }
        if (tabView != null) {
            when (tabView.kind) {
                ViewKind.Diff -> DiffPanel(source, screen.id, leave = { set(nav.back(state)) }) { path -> askView(ViewKind.Viewer, path) }
                ViewKind.Viewer -> ViewerPanel(
                    source, screen.id, tabView, wantedFile,
                    closeFile = { f -> source.closeView(tabView.id, f) { r -> r.onFailure { addError = it.message ?: "el PC no pudo cerrar el archivo" } } },
                    refer = { d -> set(nav.refer(state, screen.id, d)) },
                )
            }
            return@Column
        }
        val pane = tabPane
        // The conversation of a pane that runs `claude` takes the place of its blocks (REQ-440).
        LaunchedEffect(convId) { convId?.let { source.open(it) } }
        if (pane != null && convId != null) {
            when (val conv = tabConv) {
                null -> Box(Modifier.weight(1f).padding(Gutter)) { PaneSkeleton() }
                else -> ConversationPane(
                    source, conv, pane, screen.id, ws.path, ws.branch, settingsAsked, draft,
                    focusField = chatOpen != null && screen.pane == chatTarget, fieldText = chatOpen?.text, fieldDone = { set(nav.spendChatOpen(state)) },
                ) { set(nav.spendDraft(state)) }
            }
            return@Column
        }
        val blocksId = pane?.blocks
        LaunchedEffect(blocksId) { blocksId?.let { source.open(it) } }
        val blocks = remember(version, blocksId) { blocksId?.let { source.node(it) as? Node.Blocks } }
        var picked by remember(screen.pane) { mutableStateOf<Long?>(null) }
        val clipboard = LocalClipboard.current
        val scope = rememberCoroutineScope()
        val copy = { text: String -> scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("jwn", text))) }; Unit }
        val block = blocks?.blocks?.firstOrNull { it.id == picked }
        val shown = blocks?.takeIf { it.mode == BlocksMode.Blocks }
        // A program with the whole screen that has painted nothing is starting: its pane would show the shell's screen as it
        // was. Once it has painted it never starts again (it may clear its screen), and one that paints nothing is not waited for.
        val running = blocks?.blocks?.lastOrNull()?.takeIf { it.running }
        val ink = running?.hasInk() == true
        var painted by remember(screen.pane, running?.id) { mutableStateOf(ink) }
        LaunchedEffect(ink) { if (ink) painted = true }
        var gaveUp by remember(screen.pane, running?.id) { mutableStateOf(false) }
        val starting = blocks?.starting()
        LaunchedEffect(screen.pane, starting?.id) { if (starting != null) { delay(LOADING_MAX_MS); gaveUp = true } }
        val booting = starting?.takeIf { !painted && !gaveUp }
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
            if (blocks != null) FullScreenBanner(running?.let { programName(it.cmd) } ?: pane?.running?.argv?.firstOrNull())
            Box(
                Modifier.weight(1f).fillMaxWidth().background(Palette.bg).verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState()).padding(horizontal = Gutter - 4.dp, vertical = 10.dp),
            ) {
                if (pane == null || waitingBlocks) PaneSkeleton()
                else if (booting != null) Booting(programName(booting.cmd))
                else Text(
                    remember(pane.screen) { screenText(pane.screen) },
                    fontFamily = MonoFont, fontSize = 12.sp, lineHeight = 18.sp, color = Palette.fg, softWrap = false,
                )
            }
        }
        // A Claude opens where the shell is, not at the workspace's folder (REQ-633).
        if (source.writable && pane != null && shown != null && running == null) {
            Hairline()
            Row(
                Modifier.fillMaxWidth().background(Palette.panel).defaultMinSize(minHeight = 48.dp)
                    .clickable(role = Role.Button, onClickLabel = "abrir Claude en ${shortPath(pane.cwd)}") {
                        source.openConversation(pane.id) { r -> r.onFailure { addError = it.message ?: "el PC no pudo abrir Claude" } }
                    }
                    .padding(horizontal = Gutter),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Mono("claude aquí ›", Palette.green, 13, bold = true)
                Mono(shortPath(pane.cwd), Palette.dim, 12)
            }
        }
        val last = shown?.blocks?.lastOrNull()
        Composer(
            enabled = source.writable && screen.pane != null,
            running = running != null,
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
        val target = ws.target()
        WorkspaceMenuSheet(target, source.session().moveTo(target.id)?.note, dismiss = { wsMenu = false }) { action ->
            wsMenu = false
            choose(
                target, action, rename = { naming = Naming.Workspace(target.id, target.shown) }, newWorktree = { worktree = WorktreeAt(it, false) },
                existing = { worktree = WorktreeAt(it, true) }, project = { project = it }, remove = { removing = it },
                move = { moving = source.session().moveTo(target.id) },
            )
        }
    }
    moving?.let { m -> MoveSheet(source, m, unfold = { set(state.copy(folded = state.folded - it)) }) { moving = null } }
    worktree?.let { WorktreeSheet(source, it, state, nav, set) { worktree = null } }
    project?.let { ProjectDialog(source, it) { project = null } }
    removing?.let { RemoveWorkspaceSheet(source, it, cancel = { removing = null }) { removing = null; set(nav.back(state)) } }
    naming?.let { RenameSheet(source, it) { naming = null } }
    if (chatSheet && ws != null && chat != null) {
        ChatSheet(ws.name, chat.last != null, dismiss = { chatSheet = false }) { action ->
            when (action) {
                ChatAction.Rename -> naming = Naming.Workspace(screen.id, ws.name)
                ChatAction.Promote -> { promoteError = null; promoting = true }
                ChatAction.Archive -> source.archiveChat(screen.id) { r -> r.onSuccess { set(nav.back(state)) }.onFailure { source.say("✗ ${it.message ?: "el PC no pudo archivar la conversación"}") } }
            }
        }
    }
    if (promoting && ws != null) {
        PromoteSheet(source.session(), ws.name, promoteError, dismiss = { promoting = false; promoteError = null }) { target ->
            promoteError = null
            source.promoteChat(screen.id, target.id) { r ->
                r.onSuccess { p -> promoting = false; pendingChat = ChatOpen(target.id, p.pane, p.text) }
                    .onFailure { promoteError = it.message ?: "el PC no pudo llevar la conversación" }
            }
        }
    }
    OpenChatWhenShown(source, pendingChat, state, nav, set) { pendingChat = null }
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

/** What a long press on a workspace offers, one sheet for each kind (REQ-183, REQ-227): [pick] gets the choice. */
@Composable
internal fun BoxScope.WorkspaceMenuSheet(target: WorkspaceTarget, moveNote: String?, dismiss: () -> Unit, pick: (WsAction) -> Unit) {
    Sheet(dismiss) {
        Mono(menuTitle(target), Palette.fg, 14, bold = true, lines = 2)
        workspaceMenu(target.kind, target.repo).forEach { item ->
            val color = if (item.danger) Palette.red else Palette.fg
            // «mover a un grupo…» says where the workspace is and what moving takes (REQ-698).
            val note = if (item.action == WsAction.MoveToGroup) moveNote else item.note
            SheetAction(item.glyph, item.label + (note?.let { " · $it" } ?: ""), color) { pick(item.action) }
        }
    }
}

/** What a choice of the sheet of a workspace leads to: the field of its name, a worktree to make, or the question before a removal. */
private fun choose(target: WorkspaceTarget, action: WsAction, rename: () -> Unit, newWorktree: (String) -> Unit, existing: (String) -> Unit, project: (String) -> Unit, remove: (RemoveSheet) -> Unit, move: () -> Unit) {
    when (action) {
        WsAction.MoveToGroup -> move()
        WsAction.Rename -> rename()
        WsAction.NewWorktree -> target.repo?.let(newWorktree)
        WsAction.ExistingWorktrees -> target.repo?.let(existing)
        WsAction.Project -> target.repo?.let(project)
        else -> removeSheet(target, action)?.let(remove)
    }
}

/**
 * The question before a workspace is let go of (REQ-228): what it does and its button. Confirming closes it
 * at once: the PC does the work as a job and the jobs say how it went (REQ-661); what it refuses at the
 * start (it is being removed already) is said as a notice.
 */
@Composable
private fun BoxScope.RemoveWorkspaceSheet(source: AppSource, sheet: RemoveSheet, cancel: () -> Unit, done: () -> Unit) {
    Sheet(cancel) {
        Mono(sheet.heading, Palette.fg, 14, bold = true, lines = 3)
        Mono(sheet.body, Palette.dim, 12, lines = 8)
        SheetAction("×", sheet.action, Palette.red) {
            if (sheet.mode.removesWorktree) source.say("quitando ${sheet.name}… míralo en avisos › trabajos")
            source.removeWorkspace(sheet.workspace, sheet.mode.removesWorktree) { result ->
                result.onFailure { source.say("✗ ${it.message ?: "el PC no pudo quitar el workspace"}") }
            }
            done()
        }
        Mono("cancelar", Palette.fg, 14, modifier = Modifier.fillMaxWidth().clickable(onClick = cancel).padding(vertical = 12.dp))
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

private const val LOADING_MAX_MS = 20_000L
internal val Spin = listOf("⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏")

/** What the pane says while the program that has its screen has not painted anything: a spinner and its name. */
@Composable
private fun Booting(program: String) {
    var frame by remember { mutableIntStateOf(0) }
    LaunchedEffect(Unit) { while (true) { delay(110); frame = (frame + 1) % Spin.size } }
    Column(
        Modifier.fillMaxWidth().padding(top = 96.dp).semantics { contentDescription = "Iniciando $program" },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Mono(Spin[frame], Palette.yellow, 18)
        Mono("iniciando $program…", Palette.fg, 13)
        Mono("toca ■ para cancelar", Palette.dim, 11)
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
    // Everything in it keeps the keyboard up, the send button too, so a line after a line needs no tap on the field.
    Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsUnlessKeyboard().keepsKeyboard()) {
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
                // The keyboard's enter is a new line (REQ-238); only the button sends.
                maxLines = COMPOSER_LINES,
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp),
                cursorBrush = SolidColor(Palette.blue),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Default),
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
            Box(Modifier.size(48.dp).clickable(enabled = enabled, onClick = go).semantics { contentDescription = "Enviar" }, contentAlignment = Alignment.Center) {
                GlyphIcon(Glyph.Send, if (enabled) Palette.blue else Palette.dim)
            }
        }
    }
}

/** Open pane underlined in blue; an unseen mark sits beside the name. */
@Composable
private fun PaneTabs(
    panes: List<PaneSummary>, views: List<ViewSummary>, current: String?, state: ClientState, adding: Boolean, closeView: (ViewSummary) -> Unit,
    select: (String) -> Unit, hold: (PaneSummary) -> Unit, named: Pair<String, String>?, underline: Color?, add: () -> Unit,
) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth()) {
            Row(Modifier.weight(1f).horizontalScroll(rememberScrollState())) {
                panes.forEach { p ->
                    val on = p.id == current
                    val unseen = p.attention.filterNot(state.seen::isSeen).byUrgency()
                    Column(Modifier.combinedClickable(onClick = { select(p.id) }, onLongClick = { hold(p) })) {
                        Row(
                            Modifier.defaultMinSize(minHeight = 44.dp).padding(horizontal = Gutter),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Mono(if (named?.first == p.id) named.second else p.title, if (on) Palette.fg else Palette.dim, 13, bold = on)
                            unseen.firstOrNull()?.let { a -> a.mark().let { Mono(it.glyph, it.tone.color(), 14, bold = true) } }
                        }
                        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) (underline ?: Palette.blue) else Color.Transparent))
                    }
                }
                // The diff and the viewer come after the panes, each with a ✕ that closes it (REQ-740).
                views.forEach { v ->
                    val on = v.id == current
                    Column(Modifier.clickable { select(v.id) }) {
                        Row(
                            Modifier.defaultMinSize(minHeight = 44.dp).padding(start = Gutter),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Mono(if (v.kind == ViewKind.Diff) "± diff" else "▤ ${v.title}", if (on) Palette.fg else Palette.dim, 13, bold = on)
                            Box(
                                Modifier.defaultMinSize(minWidth = 40.dp, minHeight = 44.dp).clickable { closeView(v) }.semantics { contentDescription = "Cerrar el panel" },
                                contentAlignment = Alignment.Center,
                            ) { Mono("✕", Palette.dim, 12) }
                        }
                        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
                    }
                }
            }
            // A shell more (REQ-234): fixed at the end, so it is there however many tabs there are.
            Box(
                Modifier.width(48.dp).height(46.dp).clickable(enabled = !adding, onClick = add).semantics { contentDescription = "Panel nuevo" },
                contentAlignment = Alignment.Center,
            ) { Mono(if (adding) "…" else "+", Palette.blue, 20, bold = true) }
        }
        Hairline()
    }
}
