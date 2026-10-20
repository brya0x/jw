package dev.jw.next.app

import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
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
import dev.jw.next.client.Syntax
import dev.jw.next.client.expandTabs
import dev.jw.next.client.formatSize
import dev.jw.next.client.syntaxOf
import dev.jw.next.client.textSub
import dev.jw.next.client.widestLine
import dev.jw.next.contract.Run
import dev.jw.next.contract.TextDoc

private val CodeSize = 11.5.sp
private val CodeLine = 16.sp

/** A character of [MonoFont] is this many of its sizes wide. */
private const val Advance = 0.6f

/** Over the whole screen: the platform's own width is not used and a tap outside does not close it. */
private val FullScreen = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)

/**
 * A text file over the whole screen (REQ-618): a bar with Back, the name, how many lines and
 * what it weighs, and `⏎` to wrap the lines; the lines in monospace with their numbers, coloured by
 * the syntax the PC marked in the theme's accents, text that a long press selects. Lines are cut
 * and scroll sideways together until they are wrapped. Back closes it.
 */
@Composable
internal fun TextViewerDialog(doc: TextDoc, close: () -> Unit) {
    Dialog(onDismissRequest = close, properties = FullScreen) {
        Column(Modifier.fillMaxSize().background(Palette.bg)) { TextViewer(doc, close) }
    }
}

@Composable
private fun ColumnScope.TextViewer(doc: TextDoc, close: () -> Unit) {
    var wrap by remember { mutableStateOf(false) }
    val side = rememberScrollState()
    val digits = remember(doc) { doc.lines.size.toString().length }
    val across = remember(doc) { widestLine(doc) }
    Header(doc.title, textSub(doc), onBack = close, action = {
        Box(
            Modifier.defaultMinSize(minWidth = 56.dp, minHeight = 56.dp).clickable { wrap = !wrap }
                .semantics { role = Role.Button; contentDescription = if (wrap) "Cortar las líneas" else "Ajustar las líneas" },
            contentAlignment = Alignment.Center,
        ) { Mono("⏎", if (wrap) Palette.blue else Palette.dim, 20) }
    })
    SelectionContainer(Modifier.weight(1f)) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp)) {
            itemsIndexed(doc.lines) { i, runs -> TextLine(i + 1, digits, runs, wrap, across, side) }
        }
    }
    if (doc.truncated) {
        Mono(
            "se muestra el primer MiB de ${formatSize(doc.size)}",
            Palette.yellow, 12, lines = 2,
            modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
}

/** One line: its number, which a selection leaves out, and its text; the lines that are cut scroll sideways by the same [side], and are as wide as the widest so they agree on how far. */
@Composable
private fun TextLine(n: Int, digits: Int, runs: List<Run>, wrap: Boolean, across: Int, side: ScrollState) {
    val text = remember(runs, Palette.fg) { codeText(runs) }
    val block = with(LocalDensity.current) { (across * CodeSize.value * Advance).sp.toDp() }
    Row(Modifier.fillMaxWidth().heightIn(min = 16.dp)) {
        DisableSelection {
            Text(
                n.toString().padStart(digits),
                color = Palette.dim,
                fontFamily = MonoFont,
                fontSize = CodeSize,
                lineHeight = CodeLine,
                letterSpacing = 0.sp,
                softWrap = false,
                modifier = Modifier.background(Palette.panel).padding(horizontal = 8.dp),
            )
        }
        Box(Modifier.weight(1f).then(if (wrap) Modifier else Modifier.horizontalScroll(side))) {
            Text(
                text,
                fontFamily = MonoFont,
                fontSize = CodeSize,
                lineHeight = CodeLine,
                letterSpacing = 0.sp,
                softWrap = wrap,
                modifier = Modifier.padding(horizontal = 8.dp).then(if (wrap) Modifier else Modifier.widthIn(min = block)),
            )
        }
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
