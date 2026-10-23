package dev.jw.next.app

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.jw.next.client.ChatAction
import dev.jw.next.client.ChatOpen
import dev.jw.next.client.ClientState
import dev.jw.next.client.DeleteChatSheet
import dev.jw.next.client.HomeRow
import dev.jw.next.client.Navigator
import dev.jw.next.client.archivedChats
import dev.jw.next.client.chatAgo
import dev.jw.next.client.chatMenu
import dev.jw.next.client.chatTargets
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceSummary
import kotlinx.coroutines.delay

private val Gutter = 16.dp

/** How long a conversation just made, resumed or promoted is waited for before the phone stops (REQ-815). */
private const val CHAT_OPEN_WAIT_MS = 50_000L

/** The row of a live conversation of its own in Home: `✦`, its name, its last line, the mark of what waits and when it was said (REQ-813). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun ChatRow(row: HomeRow, current: Boolean, open: () -> Unit, hold: () -> Unit) {
    val bar = Palette.blue
    Column(Modifier.fillMaxWidth().background(if (current) Palette.sel else Color.Transparent).drawBehind { if (current) drawRect(bar, size = Size(3.dp.toPx(), size.height)) }) {
        Row(
            Modifier.fillMaxWidth().defaultMinSize(minHeight = 54.dp).combinedClickable(onClick = open, onLongClick = hold)
                .padding(start = Gutter - 2.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Mono("✦", Palette.green, 14, modifier = Modifier.width(26.dp))
            Column(Modifier.weight(1f)) {
                Mono(row.name, Palette.fg, bold = true, size = 14)
                Mono(row.chatLast ?: "vacía · escribe para empezar", Palette.dim, 11)
            }
            row.mark?.let { m ->
                Mono(m.glyph, m.tone.color(), 14, bold = true)
                if (row.unseen > 1) Mono("${row.unseen}", Palette.dim, 11)
            }
            if (row.working) CircularProgressIndicator(Modifier.size(14.dp), color = Palette.blue, strokeWidth = 2.dp)
            // Home's clock only runs with a job, so the age reads the time itself: the session's ticks redraw the row.
            else row.chatActive?.let { Mono(chatAgo(System.currentTimeMillis() / 1000, it), Palette.dim, 11) }
        }
    }
}

/** «archivadas · n ›», at the end of the section (REQ-813). */
@Composable
internal fun ArchivedLinkRow(count: Int, open: () -> Unit) {
    Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = open).padding(horizontal = Gutter), verticalAlignment = Alignment.CenterVertically) {
        Mono("archivadas · $count ›", Palette.dim, 12)
    }
}

/** What the section says while it has no live conversation (REQ-813). */
@Composable
internal fun NoChatsLine() {
    Mono("ninguna · el + de arriba abre una", Palette.dim, 12, lines = 2, modifier = Modifier.padding(horizontal = Gutter, vertical = 10.dp))
}

/**
 * The sheet of a conversation of its own, from a long press on its row or from `⋯` in its header: its own, never the workspace's,
 * which offers «mover a un grupo…» and «cerrar» (REQ-818).
 */
@Composable
internal fun BoxScope.ChatSheet(name: String, hasMessages: Boolean, dismiss: () -> Unit, choose: (ChatAction) -> Unit) {
    Sheet(dismiss) {
        Mono("conversación «$name»", Palette.fg, 14, bold = true, lines = 2)
        chatMenu(hasMessages).forEach { item ->
            SheetAction(item.glyph, item.label) { dismiss(); choose(item.action) }
            item.note?.let { Mono(it, Palette.dim, 11, lines = 2) }
        }
    }
}

/** «llevar a un proyecto…»: the workspaces a conversation can be taken to, then the PC makes the new conversation (REQ-817). */
@Composable
internal fun BoxScope.PromoteSheet(session: Node.Session, name: String, error: String?, dismiss: () -> Unit, pick: (WorkspaceSummary) -> Unit) {
    Sheet(dismiss) {
        Mono("llevar «$name» a un proyecto", Palette.fg, 14, bold = true, lines = 2)
        Mono("Una conversación nueva en el workspace que elijas, con un resumen de esta. Esta se queda donde está.", Palette.dim, 12, lines = 3)
        session.chatTargets().forEach { w -> SheetAction("→", w.name) { pick(w) } }
        error?.let { Mono(it, Palette.red, 12, lines = 3) }
    }
}

/** The question before an archived conversation is deleted: its folder goes and Claude's transcript stays (REQ-817). */
@Composable
internal fun BoxScope.DeleteChatConfirm(source: AppSource, sheet: DeleteChatSheet, cancel: () -> Unit, done: () -> Unit) {
    var error by remember(sheet.workspace) { mutableStateOf<String?>(null) }
    var busy by remember(sheet.workspace) { mutableStateOf(false) }
    Sheet(cancel) {
        Mono(sheet.heading, Palette.fg, 14, bold = true, lines = 2)
        Mono(sheet.body, Palette.dim, 12, lines = 4)
        error?.let { Mono(it, Palette.red, 12, lines = 3) }
        SheetAction("×", if (busy) "borrando…" else sheet.action, Palette.red, enabled = !busy) {
            busy = true
            error = null
            source.deleteChat(sheet.workspace) { r ->
                busy = false
                r.onSuccess { done() }.onFailure { error = it.message ?: "el PC no pudo borrar la conversación" }
            }
        }
        Mono("cancelar", Palette.dim, 13, modifier = Modifier.fillMaxWidth().clickable(onClick = cancel).padding(vertical = 10.dp))
    }
}

/**
 * Opens a conversation of its own once the session lists it as live, with its field focused (and a summary in it) when its panel has
 * its conversation: an archived one is listed and has no panel, so it is not opened before (REQ-815, REQ-817). [spent] says it was
 * opened or the wait is over.
 */
@Composable
internal fun OpenChatWhenShown(source: AppSource, pending: ChatOpen?, state: ClientState, nav: Navigator, set: (ClientState) -> Unit, spent: () -> Unit) {
    val version = source.tick.intValue
    val latest by rememberUpdatedState(state)
    LaunchedEffect(version, pending) {
        if (pending == null) return@LaunchedEffect
        nav.openChat(latest, pending.workspace, pending.pane, pending.text)?.let { set(it); spent() }
    }
    LaunchedEffect(pending) {
        if (pending != null) {
            delay(CHAT_OPEN_WAIT_MS)
            spent()
        }
    }
}

/** The archived conversations of their own: «reanudar» opens one again, `×` asks before deleting it (REQ-817). */
@Composable
internal fun ArchivedScreen(source: AppSource, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    val archived = session.archivedChats()
    var deleting by remember { mutableStateOf<DeleteChatSheet?>(null) }
    var pending by remember { mutableStateOf<ChatOpen?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val back = { set(nav.back(state)) }
    OpenChatWhenShown(source, pending, state, nav, set) { pending = null }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            Header("archivadas", "se reanudan donde las dejaste", onBack = back)
            error?.let { Mono(it, Palette.red, 12, lines = 3, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp)) }
            if (archived.isEmpty()) {
                Mono("Ninguna archivada.", Palette.dim, 13, modifier = Modifier.padding(Gutter))
            } else {
                LazyColumn(Modifier.fillMaxSize()) {
                    items(archived, key = { it.id }) { w ->
                        Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Mono(w.name, Palette.fg, bold = true, size = 14)
                                Mono(w.chat?.last ?: "vacía", Palette.dim, 11)
                            }
                            Mono(
                                "reanudar", Palette.blue, 13, bold = true,
                                modifier = Modifier.clickable {
                                    error = null
                                    source.resumeChat(w.id) { r -> r.onSuccess { pending = ChatOpen(w.id, it, null) }.onFailure { error = it.message ?: "el PC no pudo reanudar la conversación" } }
                                }.defaultMinSize(minHeight = 48.dp).padding(horizontal = 12.dp, vertical = 14.dp),
                            )
                            Mono("×", Palette.red, 18, bold = true, modifier = Modifier.clickable { deleting = DeleteChatSheet(w.id, w.name) }.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).padding(horizontal = 14.dp, vertical = 12.dp))
                        }
                        Hairline()
                    }
                }
            }
        }
        deleting?.let { DeleteChatConfirm(source, it, cancel = { deleting = null }) { deleting = null } }
        Notice(source)
    }
}

/** The id of a chat's workspace summary in [session], for the places that only have the id. */
internal fun Node.Session.chat(id: Id): WorkspaceSummary? = workspaces.firstOrNull { it.id == id && it.chat != null }
