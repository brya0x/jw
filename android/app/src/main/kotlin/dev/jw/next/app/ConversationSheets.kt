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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.wrapContentHeight
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Brush
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
import dev.jw.next.client.label
import dev.jw.next.client.keepPlanningNote
import dev.jw.next.client.listAgo
import dev.jw.next.client.outline
import dev.jw.next.client.tag
import dev.jw.next.contract.Accent
import dev.jw.next.contract.Ask
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.AskKind
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

/** What the question needs from the person (REQ-441): what the tool is about to do, and a button of at least 48 dp for each option, the first one primary; a question ends with the row «Otra…» that opens a field for the person's own words (REQ-752). */
@Composable
internal fun AskSheet(ask: Ask, busy: Boolean, dismiss: () -> Unit, viewPlan: () -> Unit, preview: MarkdownDoc? = null, answer: (String, String?) -> Unit) {
    // A choice of several: the options marked, sent together as their numbers with commas (REQ-435).
    var marked by remember(ask.id) { mutableStateOf(setOf<Int>()) }
    // «Seguir planeando» of a plan asks what to do next before it answers (REQ-595).
    var writing by remember(ask.id) { mutableStateOf(false) }
    // «Otra…» of a question: the words, and whether their card is open in place of the options (REQ-752).
    val other = ask.kind == AskKind.Question
    var otherText by remember(ask.id) { mutableStateOf("") }
    var otherOpen by remember(ask.id) { mutableStateOf(false) }
    val said = other && otherText.isNotBlank()
    val keys = { marked.sorted().joinToString(",") }
    ConvSheet(dismiss) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Mono(ask.title, Palette.fg, 14, bold = true, lines = 3, modifier = Modifier.weight(1f))
            Mono(ask.kind.tag(), Palette.yellow, 11)
        }
        // It was waiting when the daemon stopped, and the answer goes as a message (REQ-855).
        if (ask.restored) Mono("↻ se retomó tras reiniciar · tu respuesta irá como un mensaje", Palette.yellow, 12, lines = 3)
        ask.command?.let { Mono(it, Palette.cyan, 12, lines = 8, modifier = Modifier.fillMaxWidth().background(Palette.bg, RoundedCornerShape(6.dp)).padding(8.dp)) }
        if (ask.diff.isNotEmpty()) Diff(ask.diff.take(12))
        // The sheet keeps the beginning of a plan, formatted once the PC has read it, and the viewer has all of it (REQ-480, REQ-597).
        if (ask.kind == AskKind.Plan) PlanPreview(ask, preview)
        if (ask.kind == AskKind.Plan) SheetAction("▤", "Ver el plan completo" + preview?.let { d -> outline(d).size.takeIf { it > 0 }?.let { " · $it títulos" }.orEmpty() }.orEmpty(), Palette.blue, click = viewPlan)
        else ask.plan.forEach { Mono(it, Palette.fg, 13, lines = 6) }
        if (other && otherOpen) OtherBox(
            otherText, { otherText = it }, note = if (ask.multi) "se envía con lo marcado (${marked.size})" else null,
            canSend = !busy && said, back = { otherOpen = false },
        ) { answer(if (ask.multi) keys() else "", otherText.trim()) }
        else Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            ask.options.forEachIndexed { i, o ->
                val primary = i == 0 && !ask.multi
                val refuse = o.key == "no"
                val on = i in marked
                val keepPlanning = ask.kind == AskKind.Plan && refuse
                if (keepPlanning && writing) {
                    InstructionBox(busy, back = { writing = false }) { text -> answer(o.key, text) }
                    return@forEachIndexed
                }
                Box(
                    Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                        .background(if (primary) Palette.green else Palette.sel, RoundedCornerShape(8.dp))
                        .clickable(enabled = !busy) {
                            if (keepPlanning) writing = true
                            else if (!ask.multi) answer(o.key, null) else marked = if (on) marked - i else marked + i
                        }
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    contentAlignment = Alignment.CenterStart,
                ) {
                    Mono((if (ask.multi) (if (on) "☑ " else "☐ ") else "") + o.label, if (primary) Palette.panel else if (refuse) Palette.red else Palette.fg, 13, bold = primary || on, lines = 3)
                }
            }
            if (other) Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                    .background(Palette.sel, RoundedCornerShape(8.dp))
                    .clickable(enabled = !busy) { otherOpen = true }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                contentAlignment = Alignment.CenterStart,
            ) { Mono((if (ask.multi) (if (said) "☑ " else "☐ ") else "") + "✎ " + (if (said) "Otra: ${otherText.trim()}" else "Otra…"), Palette.blue, 13, bold = said, lines = 3) }
            // The count is what is marked and the words, when there are some.
            val count = marked.size + if (said) 1 else 0
            if (ask.multi) Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                    .background(if (count == 0) Palette.sel else Palette.green, RoundedCornerShape(8.dp))
                    .clickable(enabled = !busy && count > 0) { answer(keys(), otherText.trim().takeIf { said }) },
                contentAlignment = Alignment.Center,
            ) { Mono(if (count == 0) "marca una o más" else "Enviar ($count)", if (count == 0) Palette.dim else Palette.panel, 13, bold = true) }
        }
    }
}

/** The beginning of a plan that waits: its first blocks formatted and cut with a fade (the lines as they are while the PC has not read the document), never more than [PREVIEW_HEIGHT] (REQ-597). */
@Composable
private fun PlanPreview(ask: Ask, doc: MarkdownDoc?) {
    Box(Modifier.fillMaxWidth().heightIn(max = PREVIEW_HEIGHT).clipToBounds()) {
        Column(Modifier.fillMaxWidth().wrapContentHeight(align = Alignment.Top, unbounded = true), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (doc != null) doc.blocks.take(PREVIEW_BLOCKS).forEach { BlockView(it) }
            else ask.plan.take(PREVIEW_LINES).forEach { Mono(it, Palette.fg, 13, lines = 3) }
        }
        Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(40.dp).background(Brush.verticalGradient(listOf(Color.Transparent, Palette.panel))))
    }
}

private val PREVIEW_HEIGHT = 150.dp
private const val PREVIEW_BLOCKS = 6
private const val PREVIEW_LINES = 8

/** The line where what to do next is written when a plan is not approved: «Volver» goes back to the options, the button sends the text (or none when it is empty) with the refusal (REQ-595). */
@Composable
internal fun InstructionBox(busy: Boolean, back: () -> Unit, send: (String?) -> Unit) {
    var text by remember { mutableStateOf("") }
    TextCard("Seguir planeando", Palette.red, "¿qué sigue? (opcional)", text, { text = it }, null, "Seguir planeando", !busy, back) { send(keepPlanningNote(text)) }
}

/** The card of «Otra…»: the person's own words for the question, «Volver» keeps them and goes back to the options, «Enviar» is off while nothing is written (REQ-752). */
@Composable
internal fun OtherBox(text: String, onText: (String) -> Unit, note: String?, canSend: Boolean, back: () -> Unit, send: () -> Unit) {
    TextCard("Otra respuesta", Palette.blue, "escribe tu respuesta", text, onText, note, "Enviar", canSend, back, send)
}

/** A card with a heading, a field, and two buttons: «Volver» and the one that sends. */
@Composable
private fun TextCard(
    heading: String, headingColour: Color, hint: String, text: String, onText: (String) -> Unit, note: String?,
    sendLabel: String, canSend: Boolean, back: () -> Unit, send: () -> Unit,
) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Column(Modifier.fillMaxWidth().background(Palette.sel, RoundedCornerShape(8.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Mono(heading, headingColour, 13, bold = true)
        BasicTextField(
            value = text,
            onValueChange = onText,
            textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp),
            cursorBrush = SolidColor(Palette.blue),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().defaultMinSize(minHeight = 84.dp).border(1.dp, Palette.blue, RoundedCornerShape(6.dp)).background(Palette.bg, RoundedCornerShape(6.dp)).padding(10.dp)) {
                    if (text.isEmpty()) Mono(hint, Palette.dim, 13)
                    inner()
                }
            },
        )
        note?.let { Mono(it, Palette.dim, 12) }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(
                Modifier.weight(1f).defaultMinSize(minHeight = 48.dp).background(Palette.panel, RoundedCornerShape(8.dp)).clickable(onClick = back),
                contentAlignment = Alignment.Center,
            ) { Mono("Volver", Palette.fg, 13) }
            Box(
                Modifier.weight(1f).defaultMinSize(minHeight = 48.dp).background(if (canSend) Palette.green else Palette.panel, RoundedCornerShape(8.dp)).clickable(enabled = canSend, onClick = send),
                contentAlignment = Alignment.Center,
            ) { Mono(sendLabel, if (canSend) Palette.panel else Palette.dim, 13, bold = true) }
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
                view.rows.forEachIndexed { i, r ->
                    // A list with sections (REQ-803) says each at its first row.
                    val group = r.group
                    if (group != null && group != view.rows.getOrNull(i - 1)?.group) {
                        Row(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp, start = 4.dp, end = 4.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                            Mono(group.title, group.tint.colour(), 11, bold = true)
                            Mono("${view.rows.count { it.group == group }}", Palette.dim, 11)
                        }
                    }
                    PanelRowView(r, now) { choose(r.act) }
                }
            }
        }
    }
}

@Composable
private fun PanelRowView(r: PanelRow, now: Long, click: () -> Unit) {
    Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).clickable(enabled = r.act != RowAct.Nothing, onClick = click).padding(vertical = 8.dp, horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Mono(r.label, Palette.fg, 13, bold = true, lines = 2, modifier = Modifier.weight(1f, fill = false))
                r.at?.let { Mono(listAgo(now, it), Palette.dim, 11) }
                r.tag?.let { Mono(it, (r.tone ?: Tone.Dim).color(), 11) }
            }
            if (r.detail.isNotBlank()) Mono(r.detail, Palette.dim, 12, lines = 2)
        }
        // A task that runs is stopped from its row (REQ-448).
        if (r.act is RowAct.Stop) Box(Modifier.size(44.dp).semantics { contentDescription = "Detener la tarea" }, contentAlignment = Alignment.Center) {
            Mono("■", Palette.red, 16, bold = true)
        }
    }
    Hairline()
}
