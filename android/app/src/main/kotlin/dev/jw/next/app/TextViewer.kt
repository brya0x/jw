package dev.jw.next.app

import android.content.ClipData
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.jw.next.client.Draft
import dev.jw.next.client.Syntax
import dev.jw.next.client.expandTabs
import dev.jw.next.client.formatSize
import dev.jw.next.client.lineReference
import dev.jw.next.client.syntaxOf
import dev.jw.next.client.textSub
import dev.jw.next.contract.Run
import dev.jw.next.contract.TextDoc
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private val CodeSize = 11.5.sp
private val CodeLine = 16.sp

/** Over the whole screen: the platform's own width is not used and a tap outside does not close it. */
private val FullScreen = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)

/**
 * A text file over the whole screen (REQ-618, REQ-642): a bar with Back, the name, how many lines and what it weighs;
 * the lines in monospace with their numbers, coloured by the syntax the PC marked in the theme's accents, text that a
 * long press selects. A line that is long goes on in the next row under one number and nothing scrolls sideways.
 * A tap on a number picks that line, another picks all that is between, and a bar offers to hand the lines to the
 * workspace's agent as a reference ([refer], REQ-639), to copy it or to let go. [rel] is the file as the PC was asked
 * for it. Back closes it.
 */
@Composable
internal fun TextViewerDialog(doc: TextDoc, rel: String, refer: (Draft) -> Unit, close: () -> Unit) {
    Dialog(onDismissRequest = close, properties = FullScreen) {
        Column(Modifier.fillMaxSize().background(Palette.bg)) { TextViewer(doc, rel, refer, close) }
    }
}

@Composable
internal fun ColumnScope.TextViewer(doc: TextDoc, rel: String, refer: (Draft) -> Unit, close: () -> Unit) {
    val digits = remember(doc) { doc.lines.size.toString().length }
    var anchor by remember(doc) { mutableStateOf<Int?>(null) }
    var tip by remember(doc) { mutableIntStateOf(0) }
    var note by remember(doc) { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboard.current
    LaunchedEffect(note) { if (note != null) { delay(2500); note = null } }
    val first = anchor?.let { minOf(it, tip) }
    val last = anchor?.let { maxOf(it, tip) }
    val pick = { n: Int ->
        val a = anchor
        when {
            a == null -> { anchor = n; tip = n }
            a == tip && n == a -> anchor = null
            else -> tip = n
        }
    }
    Header(doc.title, textSub(doc), onBack = close)
    SelectionContainer(Modifier.weight(1f)) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp)) {
            itemsIndexed(doc.lines) { i, runs ->
                TextLine(i + 1, digits, runs, picked = first != null && i in first..last!!) { pick(i) }
            }
        }
    }
    if (doc.truncated) {
        Mono(
            "se muestra el primer MiB de ${formatSize(doc.size)}",
            Palette.yellow, 12, lines = 2,
            modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
    note?.let { Mono(it, Palette.yellow, 12, lines = 2, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 16.dp, vertical = 8.dp)) }
    if (first != null && last != null) {
        val from = first + 1
        val to = last + 1
        Row(
            Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 12.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Mono(if (to > from) "L$from–$to" else "L$from", Palette.blue, 13, bold = true)
                Mono(if (to > from) "${to - from + 1} líneas" else "1 línea", Palette.dim, 12)
            }
            BarButton("@ al agente", Palette.blue) { refer(Draft(rel, from, to)) }
            BarButton("copiar", Palette.fg) {
                val text = lineReference(null, null, rel, from, to)
                scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("jwn", text))) }
                note = "copiado · ${text.trim()}"
                anchor = null
            }
            BarButton("×", Palette.dim, "Soltar las líneas") { anchor = null }
        }
    }
}

/** A button of the bar of picked lines: as high as a touch target must be. */
@Composable
private fun BarButton(label: String, color: Color, description: String? = null, click: () -> Unit) {
    Box(
        Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable(onClick = click)
            .semantics { role = Role.Button; if (description != null) contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { Mono(label, color, 12, modifier = Modifier.padding(horizontal = 8.dp)) }
}

/** One line: its number, a button that picks it and that a selection leaves out, and its text, which goes on in the next row when it is long. */
@Composable
private fun TextLine(n: Int, digits: Int, runs: List<Run>, picked: Boolean, pick: () -> Unit) {
    val text = remember(runs, Palette.fg) { codeText(runs) }
    Row(Modifier.fillMaxWidth().background(if (picked) Palette.sel else Color.Transparent)) {
        DisableSelection {
            Box(
                Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 24.dp).background(if (picked) Palette.sel else Palette.panel)
                    .clickable(onClickLabel = if (picked) "Soltar la línea $n" else "Elegir la línea $n", role = Role.Button, onClick = pick)
                    .semantics { contentDescription = "Línea $n"; selected = picked }
                    .padding(horizontal = 8.dp),
                contentAlignment = Alignment.TopEnd,
            ) {
                Text(
                    n.toString().padStart(digits),
                    color = if (picked) Palette.blue else Palette.dim,
                    fontFamily = MonoFont,
                    fontSize = CodeSize,
                    lineHeight = CodeLine,
                    letterSpacing = 0.sp,
                    softWrap = false,
                )
            }
        }
        Text(
            text,
            fontFamily = MonoFont,
            fontSize = CodeSize,
            lineHeight = CodeLine,
            letterSpacing = 0.sp,
            modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
        )
    }
}

/** The runs of a line as text with the accent of each one's syntax (REQ-622); a missing or unknown class is plain. */
private fun codeText(runs: List<Run>): AnnotatedString = buildAnnotatedString {
    for (run in runs) {
        val style = when (syntaxOf(run.k)) {
            Syntax.Keyword -> SpanStyle(color = Palette.magenta)
            Syntax.Str -> SpanStyle(color = Palette.green)
            Syntax.Number -> SpanStyle(color = Palette.yellow)
            Syntax.Type, Syntax.Attribute -> SpanStyle(color = Palette.cyan)
            Syntax.Function -> SpanStyle(color = Palette.blue)
            Syntax.Comment -> SpanStyle(color = Palette.dim, fontStyle = FontStyle.Italic)
            null -> SpanStyle(color = Palette.fg)
        }
        withStyle(style) { append(expandTabs(run.t)) }
    }
}

/** A file the PC would not show as text (REQ-613): its name, and that it is binary with the weight the listing knew. */
@Composable
internal fun RefusalDialog(name: String, note: String, close: () -> Unit) {
    Dialog(onDismissRequest = close, properties = FullScreen) {
        Column(Modifier.fillMaxSize().background(Palette.bg)) {
            Header(name, null, onBack = close)
            Column(
                Modifier.weight(1f).fillMaxWidth().padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) { Mono(note, Palette.yellow, 13, lines = 3) }
        }
    }
}
