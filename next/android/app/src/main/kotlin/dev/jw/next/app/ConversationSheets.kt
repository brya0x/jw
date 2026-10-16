package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.ACCENTS
import dev.jw.next.client.EFFORTS
import dev.jw.next.client.MODELS
import dev.jw.next.client.PanelRow
import dev.jw.next.client.PanelView
import dev.jw.next.client.RowAct
import dev.jw.next.client.Tone
import dev.jw.next.client.char
import dev.jw.next.client.label
import dev.jw.next.client.listAgo
import dev.jw.next.client.tag
import dev.jw.next.contract.Accent
import dev.jw.next.contract.Ask
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.Node

/** A sheet from the bottom over the conversation: a tap outside it or Back closes it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConvSheet(dismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    ModalBottomSheet(
        onDismissRequest = dismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Palette.panel,
        scrimColor = Color.Black.copy(alpha = 0.55f),
        shape = RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp),
        dragHandle = null,
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
            content = content,
        )
    }
}

/** What the question needs from the person (REQ-441): what the tool is about to do, and a button of at least 48 dp for each option, the first one primary. */
@Composable
internal fun AskSheet(ask: Ask, busy: Boolean, dismiss: () -> Unit, answer: (String) -> Unit) {
    ConvSheet(dismiss) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Mono(ask.title, Palette.fg, 14, bold = true, lines = 3, modifier = Modifier.weight(1f))
            Mono(ask.kind.tag(), Palette.yellow, 11)
        }
        ask.command?.let { Mono(it, Palette.cyan, 12, lines = 8, modifier = Modifier.fillMaxWidth().background(Palette.bg, RoundedCornerShape(6.dp)).padding(8.dp)) }
        if (ask.diff.isNotEmpty()) Diff(ask.diff.take(12))
        ask.plan.forEach { Mono(it, Palette.fg, 13, lines = 6) }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            ask.options.forEachIndexed { i, o ->
                val primary = i == 0
                val refuse = o.key == "no"
                Box(
                    Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                        .background(if (primary) Palette.green else Palette.sel, RoundedCornerShape(8.dp))
                        .clickable(enabled = !busy) { answer(o.key) }.padding(horizontal = 12.dp, vertical = 10.dp),
                    contentAlignment = Alignment.CenterStart,
                ) { Mono(o.label, if (primary) Palette.panel else if (refuse) Palette.red else Palette.fg, 13, bold = primary, lines = 3) }
            }
        }
    }
}

@Composable
private fun Segments(options: List<Pair<String, Boolean>>, accent: Color, pick: (Int) -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEachIndexed { i, (label, on) ->
            Box(
                Modifier.defaultMinSize(minHeight = 40.dp).background(if (on) accent else Palette.sel, CircleShape).clickable { pick(i) }.padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center,
            ) { Mono(label, if (on) Palette.panel else Palette.fg, 12, bold = on) }
        }
    }
}

/** The name, the six colours, the model, the effort and the mode, each change sent at once (REQ-444). */
@Composable
internal fun SettingsSheet(node: Node.Conversation, dismiss: () -> Unit, set: (name: String?, accent: Accent?, model: String?, effort: String?, mode: ConvMode?) -> Unit) {
    var name by remember(node.id) { mutableStateOf(node.name) }
    val accent = Palette.accent(node.accent)
    val save = { if (name.isNotBlank() && name != node.name) set(name.trim(), null, null, null, null) }
    ConvSheet({ save(); dismiss() }) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Mono("Conversación", Palette.fg, 14, bold = true)
            Mono("Listo", accent, 14, bold = true, modifier = Modifier.clickable { save(); dismiss() }.defaultMinSize(minHeight = 32.dp))
        }
        Mono("nombre", Palette.dim, 11)
        BasicTextField(
            value = name,
            onValueChange = { name = it },
            singleLine = true,
            textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp),
            cursorBrush = SolidColor(Palette.blue),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
            modifier = Modifier.fillMaxWidth(),
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().defaultMinSize(minHeight = 40.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).background(Palette.bg, RoundedCornerShape(6.dp)).padding(horizontal = 10.dp), contentAlignment = Alignment.CenterStart) { inner() }
            },
        )
        Mono("color", Palette.dim, 11)
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            ACCENTS.forEach { (a, word) ->
                Box(
                    Modifier.size(36.dp).background(Palette.accent(a), CircleShape)
                        .border(2.dp, if (a == node.accent) Palette.fg else Color.Transparent, CircleShape)
                        .clickable { set(null, a, null, null, null) }
                        .semantics { contentDescription = word },
                ) {}
            }
        }
        Mono("modelo", Palette.dim, 11)
        Segments(MODELS.map { it to node.model.contains(it) }, accent) { set(null, null, MODELS[it], null, null) }
        Mono("esfuerzo", Palette.dim, 11)
        Segments(EFFORTS.map { it to (it == node.effort) }, accent) { set(null, null, null, EFFORTS[it], null) }
        Mono("modo", Palette.dim, 11)
        val modes = ConvMode.entries
        Segments(modes.map { it.label() to (it == node.mode) }, accent) { set(null, null, null, null, modes[it]) }
    }
}

/** What `+` offers (REQ-445). */
@Composable
internal fun PlusSheet(dismiss: () -> Unit, pick: (String) -> Unit) {
    ConvSheet(dismiss) {
        Mono("adjuntar", Palette.fg, 14, bold = true)
        SheetAction("@", "Archivo del workspace", Palette.fg) { pick("file") }
        SheetAction("◉", "Foto", Palette.fg) { pick("photo") }
        SheetAction("▣", "Galería", Palette.fg) { pick("gallery") }
        SheetAction("/", "Comandos", Palette.fg) { pick("commands") }
        SheetAction("!", "Shell", Palette.yellow) { pick("shell") }
    }
}

/** One of jwn's lists as a sheet (REQ-450): rows with what they are, a tap does what the row says. */
@Composable
internal fun PanelSheet(view: PanelView?, now: Long, dismiss: () -> Unit, choose: (RowAct) -> Unit) {
    ConvSheet(dismiss) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Mono(view?.title ?: "cargando…", Palette.fg, 14, bold = true)
            Mono("Cerrar", Palette.dim, 13, modifier = Modifier.clickable(onClick = dismiss).defaultMinSize(minHeight = 32.dp))
        }
        when {
            view == null -> Mono("cargando…", Palette.dim, 12)
            view.rows.isEmpty() -> Mono(view.empty.ifEmpty { "Nada que ver." }, Palette.dim, 12, lines = 3)
            else -> Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                view.rows.forEach { r -> PanelRowView(r, now) { choose(r.act) } }
            }
        }
    }
}

@Composable
private fun PanelRowView(r: PanelRow, now: Long, click: () -> Unit) {
    Column(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).clickable(enabled = r.act != RowAct.Nothing, onClick = click).padding(vertical = 8.dp, horizontal = 4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mono(r.label, Palette.fg, 13, bold = true, lines = 2, modifier = Modifier.weight(1f, fill = false))
            r.at?.let { Mono(listAgo(now, it), Palette.dim, 11) }
            r.tag?.let { Mono(it, (r.tone ?: Tone.Dim).color(), 11) }
        }
        if (r.detail.isNotBlank()) Mono(r.detail, Palette.dim, 12, lines = 2)
    }
    Hairline()
}
