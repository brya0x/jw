package dev.jw.next.app

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import dev.jw.next.client.AvisoChip
import dev.jw.next.client.HomeItem
import dev.jw.next.client.MoveTo
import dev.jw.next.client.NodeSource
import dev.jw.next.client.groupNameError
import dev.jw.next.contract.Group
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

private val Gutter = 16.dp

/**
 * The row of a group (REQ-675): `▾` or `▸`, its name and, at the right, how many workspaces it
 * holds when it is open or what they say when it is folded (REQ-670): a spinner and how many work,
 * then a mark for each kind of attention, three at most and `+n` for the rest. A tap folds or
 * opens it, a long press opens its sheet.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun GroupRow(item: HomeItem.Group, toggle: () -> Unit, hold: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 50.dp).background(Palette.panel)
            .combinedClickable(onClick = toggle, onLongClick = hold).padding(horizontal = Gutter, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Mono(if (item.open) "▾" else "▸", Palette.dim, 14, modifier = Modifier.width(14.dp))
        Mono(item.name, Palette.fg, 14, bold = true, modifier = Modifier.weight(1f))
        if (item.open) {
            Mono("${item.count}", Palette.dim, 11)
        } else {
            if (item.running > 0) {
                CircularProgressIndicator(Modifier.size(12.dp), color = Palette.blue, strokeWidth = 2.dp)
                Mono("${item.running}", Palette.blue, 12)
            }
            item.shownMarks.forEach { Mono(it.mark.glyph + if (it.count > 1) "${it.count}" else "", it.mark.tone.color(), 13, bold = true) }
            if (item.moreMarks > 0) Mono("+${item.moreMarks}", Palette.dim, 11)
        }
    }
}

/** What an open group with no workspace says. */
@Composable
internal fun EmptyGroup() {
    Mono(
        "vacío · mantén pulsado un workspace y elige «mover a un grupo…»",
        Palette.dim, 11, lines = 2,
        modifier = Modifier.fillMaxWidth().padding(start = Gutter + 22.dp, end = Gutter, top = 10.dp, bottom = 10.dp),
    )
}

/** The last line of the list: it makes a group, the first one included (REQ-676). */
@Composable
internal fun NewGroupLine(click: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 14.dp).defaultMinSize(minHeight = 48.dp)
            .border(1.dp, Palette.line, RoundedCornerShape(10.dp)).clickable(onClick = click),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) { Mono("＋ grupo nuevo", Palette.dim, 13) }
}

/** The chips over the alerts of Avisos (REQ-677): all, each group with its count, and «sin grupo». */
@Composable
internal fun AvisoChips(chips: List<AvisoChip>, selected: Id?, pick: (Id?) -> Unit) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = Gutter, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        chips.forEach { chip -> Pill("${chip.label} ${chip.count}", chip.filter == selected) { pick(chip.filter) } }
    }
}

/** The field of a group's name, focused when [focused] is true; [done] is the keyboard's. */
@Composable
private fun NameInput(value: TextFieldValue, change: (TextFieldValue) -> Unit, focused: Boolean, done: () -> Unit) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(focused) { if (focused) focus.requestFocus() }
    NameField(value, { change(it.copy(text = it.text.take(MAX_NAME))) }, "nombre del grupo", focus, done)
}

private const val MAX_NAME = dev.jw.next.client.MAX_GROUP_NAME

/**
 * «mover a un grupo…» (REQ-676): the groups with a mark on the one the repository is in, «sin
 * grupo» and «grupo nuevo…» (a name and «crear y mover»). It says what moving takes, closes when the
 * PC has moved it, and opens the group it went to; what the PC refuses stays on the sheet.
 */
@Composable
internal fun BoxScope.MoveSheet(source: NodeSource, move: MoveTo, unfold: (Id) -> Unit, dismiss: () -> Unit) {
    var adding by remember(move) { mutableStateOf(false) }
    var text by remember(move) { mutableStateOf(TextFieldValue("")) }
    var error by remember(move) { mutableStateOf<String?>(null) }
    var busy by remember(move) { mutableStateOf(false) }
    val finish: (Result<Unit>) -> Unit = { result ->
        busy = false
        result.onSuccess { dismiss() }.onFailure { error = it.message ?: "el PC no pudo mover el workspace" }
    }
    val send: (Id?) -> Unit = { group ->
        if (!busy) {
            busy = true
            error = null
            // The group opens at the tap, so that the workspace is seen where it lands; a move that fails leaves it open (REQ-676).
            group?.let(unfold)
            source.moveToGroup(move.workspace, group, finish)
        }
    }
    val create = {
        val why = groupNameError(text.text, move.groups)
        if (why != null) {
            error = why
        } else if (!busy) {
            busy = true
            error = null
            source.createGroupWith(text.text.trim(), move.workspace, finish)
        }
    }
    Sheet(dismiss) {
        Mono("mover «${move.shown}»", Palette.fg, 14, bold = true, lines = 2)
        Mono(move.note, Palette.dim, 12, lines = 3)
        move.groups.forEach { g ->
            val here = move.current?.id == g.id
            SheetAction(if (here) "✓" else " ", g.name, if (here) Palette.blue else Palette.fg, enabled = !busy) { if (here) dismiss() else send(g.id) }
        }
        SheetAction(if (move.current == null) "✓" else " ", "sin grupo", if (move.current == null) Palette.blue else Palette.fg, enabled = !busy) {
            if (move.current == null) dismiss() else send(null)
        }
        if (!adding) {
            SheetAction("＋", "grupo nuevo…", Palette.fg, enabled = !busy) { adding = true }
        } else {
            NameInput(text, { text = it; error = null }, focused = true, done = create)
            SheetAction("↵", if (busy) "moviendo…" else "crear y mover", Palette.blue, enabled = !busy, click = create)
        }
        error?.let { Mono(it, Palette.red, 12, lines = 4) }
    }
}

/**
 * The sheet of a group (REQ-676): its name to change, a move up and down (not offered at the ends),
 * and its deletion, which says first that no workspace is closed.
 */
@Composable
internal fun BoxScope.GroupSheet(source: NodeSource, session: Node.Session, group: Group, dismiss: () -> Unit) {
    val at = session.groups.indexOfFirst { it.id == group.id }
    val members = session.workspaces.count { it.group == group.id }
    var text by remember(group.id) { mutableStateOf(TextFieldValue(group.name, TextRange(group.name.length))) }
    var error by remember(group.id) { mutableStateOf<String?>(null) }
    var busy by remember(group.id) { mutableStateOf(false) }
    var asking by remember(group.id) { mutableStateOf(false) }
    val finish: (Result<Unit>) -> Unit = { result ->
        busy = false
        result.onSuccess { dismiss() }.onFailure { error = it.message ?: "el PC no pudo hacerlo" }
    }
    val run: (((Result<Unit>) -> Unit) -> Unit) -> Unit = { send ->
        if (!busy) {
            busy = true
            error = null
            send(finish)
        }
    }
    val save = {
        val why = groupNameError(text.text, session.groups, except = group.id)
        if (why != null) error = why else run { source.renameGroup(group.id, text.text.trim(), it) }
    }
    Sheet(dismiss) {
        if (asking) {
            Mono("borrar «${group.name}»", Palette.fg, 14, bold = true, lines = 2)
            Mono(
                when (members) {
                    0 -> "Está vacío."
                    1 -> "Su workspace no se cierra: pasa a «sin grupo»."
                    else -> "Sus $members workspaces no se cierran: pasan a «sin grupo»."
                },
                Palette.dim, 12, lines = 4,
            )
            error?.let { Mono(it, Palette.red, 12, lines = 4) }
            SheetAction("×", if (busy) "borrando…" else "borrar el grupo", Palette.red, enabled = !busy) { run { source.deleteGroup(group.id, it) } }
            Mono("cancelar", Palette.fg, 14, modifier = Modifier.fillMaxWidth().clickable { asking = false }.padding(vertical = 12.dp))
        } else {
            Mono(group.name, Palette.fg, 14, bold = true, lines = 2)
            Mono("$members workspace${if (members == 1) "" else "s"}", Palette.dim, 12)
            NameInput(text, { text = it; error = null }, focused = false, done = save)
            error?.let { Mono(it, Palette.red, 12, lines = 4) }
            SheetAction("↵", if (busy) "guardando…" else "guardar el nombre", Palette.blue, enabled = !busy, click = save)
            SheetAction("↑", "subir", enabled = !busy && at > 0) { run { source.moveGroup(group.id, -1, it) } }
            SheetAction("↓", "bajar", enabled = !busy && at in 0 until session.groups.size - 1) { run { source.moveGroup(group.id, 1, it) } }
            SheetAction("×", "borrar el grupo…", Palette.red, enabled = !busy) { asking = true }
        }
    }
}

/** «＋ grupo nuevo»: a name, and the group begins empty (REQ-676). */
@Composable
internal fun BoxScope.NewGroupSheet(source: NodeSource, groups: List<Group>, dismiss: () -> Unit) {
    var text by remember { mutableStateOf(TextFieldValue("")) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val create = {
        val why = groupNameError(text.text, groups)
        if (why != null) {
            error = why
        } else if (!busy) {
            busy = true
            error = null
            source.createGroup(text.text.trim()) { result ->
                busy = false
                result.onSuccess { dismiss() }.onFailure { error = it.message ?: "el PC no pudo crear el grupo" }
            }
        }
    }
    Sheet(dismiss) {
        Mono("grupo nuevo", Palette.fg, 14, bold = true)
        Mono("Un grupo reúne los workspaces de un mismo producto. Empieza vacío.", Palette.dim, 12, lines = 3)
        NameInput(text, { text = it; error = null }, focused = true, done = create)
        error?.let { Mono(it, Palette.red, 12, lines = 4) }
        SheetAction("↵", if (busy) "creando…" else "crear", Palette.blue, enabled = !busy, click = create)
    }
}
