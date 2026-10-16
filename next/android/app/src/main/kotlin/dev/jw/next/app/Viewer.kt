package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.jw.next.client.outline
import dev.jw.next.client.plain
import dev.jw.next.client.viewerSub
import dev.jw.next.contract.AskOption
import dev.jw.next.contract.Inline
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.MdBlock
import dev.jw.next.contract.MdItem
import kotlinx.coroutines.launch

private val Gutter = 16.dp

/** The ways to answer the plan the viewer shows: the question's own [options], whether it still [waits] and what [answer] does with a key (REQ-480). */
internal class PlanAnswers(val options: List<AskOption>, val waits: Boolean, val answer: (String) -> Unit)

/**
 * A Markdown document over the whole screen (REQ-481): a bar with Back, the title and `☰` for the
 * headings, the blocks the PC parsed drawn in the theme's colours, text that a long press selects,
 * and, for a plan that waits, its answers at the foot. Back closes it.
 */
@Composable
internal fun ViewerDialog(doc: MarkdownDoc, plan: PlanAnswers?, close: () -> Unit) {
    Dialog(onDismissRequest = close, properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)) {
        Column(Modifier.fillMaxSize().background(Palette.bg)) { Viewer(doc, plan, close) }
    }
}

@Composable
private fun ColumnScope.Viewer(doc: MarkdownDoc, plan: PlanAnswers?, close: () -> Unit) {
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    var headings by remember { mutableStateOf(false) }
    val titles = remember(doc) { outline(doc) }
    Header(doc.title, viewerSub(doc, plan != null, plan?.waits == true), onBack = close, action = {
        if (titles.isNotEmpty()) Box(
            Modifier.defaultMinSize(minWidth = 56.dp, minHeight = 56.dp).clickable { headings = true }.semantics { contentDescription = "Títulos del documento" },
            contentAlignment = Alignment.Center,
        ) { Mono("☰", Palette.fg, 20) }
    })
    SelectionContainer(Modifier.weight(1f)) {
        LazyColumn(
            Modifier.fillMaxSize(),
            state = list,
            contentPadding = PaddingValues(horizontal = Gutter, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            items(doc.blocks) { block -> BlockView(block) }
            if (doc.truncated) item { Mono("el documento sigue más allá de lo que el PC lee", Palette.dim, 11, lines = 2) }
        }
    }
    if (plan != null && plan.waits) PlanBar(plan)
    if (headings) ConvSheet(dismiss = { headings = false }) {
        titles.forEach { h ->
            Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                    .clickable { headings = false; scope.launch { list.animateScrollToItem(h.block) } }
                    .padding(start = ((h.level - 1).coerceAtMost(3) * 14).dp),
                contentAlignment = Alignment.CenterStart,
            ) { Mono(h.text, headingColor(h.level), 13, bold = h.level == 1, lines = 2) }
        }
    }
}

/** The answers of a plan that waits, under the document: the first one primary (REQ-480). */
@Composable
private fun PlanBar(plan: PlanAnswers) {
    Column(Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Mono("plan listo", Palette.yellow, 11)
        plan.options.forEachIndexed { i, o ->
            Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                    .background(if (i == 0) Palette.green else Palette.sel, RoundedCornerShape(8.dp))
                    .clickable { plan.answer(o.key) }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                contentAlignment = Alignment.CenterStart,
            ) { Mono(o.label, if (i == 0) Palette.panel else if (o.key == "no") Palette.red else Palette.fg, 13, bold = i == 0, lines = 3) }
        }
    }
}

private fun headingColor(level: Int): Color = when (level) {
    1 -> Palette.blue
    2 -> Palette.cyan
    3 -> Palette.green
    else -> Palette.fg
}

/** The runs of a text as one string with their styles: bold, italic, code in cyan and links underlined in blue. */
private fun styled(text: List<Inline>, base: Color): AnnotatedString = buildAnnotatedString {
    for (run in text) {
        val style = SpanStyle(
            color = when {
                run.href != null -> Palette.blue
                run.code -> Palette.cyan
                else -> base
            },
            fontWeight = if (run.b) FontWeight.Bold else null,
            fontStyle = if (run.i) FontStyle.Italic else null,
            textDecoration = if (run.href != null) TextDecoration.Underline else null,
        )
        withStyle(style) { append(run.t) }
    }
}

@Composable
private fun Body(text: List<Inline>, base: Color = Palette.fg, size: Int = 13, bold: Boolean = false, modifier: Modifier = Modifier) = Text(
    styled(text, base),
    fontFamily = MonoFont,
    fontSize = size.sp,
    lineHeight = (size * 1.5f).sp,
    fontWeight = if (bold) FontWeight.Bold else null,
    modifier = modifier,
)

@Composable
private fun BlockView(block: MdBlock) {
    when (block) {
        is MdBlock.Heading -> Body(
            block.text,
            headingColor(block.level),
            size = when (block.level) { 1 -> 17; 2 -> 15; else -> 13 },
            bold = true,
            modifier = Modifier.semantics { heading() }.padding(top = if (block.level == 1) 0.dp else 6.dp),
        )
        is MdBlock.Para -> Body(block.text)
        is MdBlock.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
            Box(Modifier.width(3.dp).fillMaxHeight().background(Palette.yellow))
            Column(Modifier.padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) { block.blocks.forEach { BlockView(it) } }
        }
        is MdBlock.ListOf -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEachIndexed { i, item -> ItemView(item, block.start?.let { "${it + i}." }) }
        }
        is MdBlock.Code -> Column(Modifier.fillMaxWidth().background(Palette.panel, RoundedCornerShape(6.dp)).padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Mono(block.lang.ifEmpty { "código" }, Palette.dim, 10)
            Text(
                block.text.trimEnd('\n'),
                color = Palette.fg,
                fontFamily = MonoFont,
                fontSize = 11.5.sp,
                softWrap = false,
                modifier = Modifier.horizontalScroll(rememberScrollState()),
            )
        }
        is MdBlock.Table -> TableView(block)
        MdBlock.Rule -> Hairline()
    }
}

/** An item of a list: its mark (a number, a bullet or a checkbox) and its blocks. */
@Composable
private fun ItemView(item: MdItem, number: String?) {
    val (mark, tone) = when {
        item.task == true -> "☑" to Palette.green
        item.task == false -> "☐" to Palette.dim
        number != null -> number to Palette.dim
        else -> "•" to Palette.cyan
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Mono(mark, tone, 13, modifier = Modifier.defaultMinSize(minWidth = 16.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) { item.blocks.forEach { BlockView(it) } }
    }
}

/** A table that scrolls sideways inside the screen when it is wider than it (the page itself never does). */
@Composable
private fun TableView(table: MdBlock.Table) {
    val rows = listOf(table.head) + table.rows
    val columns = rows.maxOf { it.size }
    // A monospaced cell is as wide as its longest text; a long one wraps at 36 characters.
    val widths = (0 until columns).map { c -> (rows.maxOf { r -> r.getOrNull(c)?.let { plain(it).length } ?: 0 }.coerceIn(4, 36) * 8 + 20).dp }
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        rows.forEachIndexed { r, row ->
            Row(if (r == 0) Modifier.background(Palette.panel) else Modifier) {
                for (c in 0 until columns) {
                    Body(
                        row.getOrNull(c) ?: emptyList(),
                        if (r == 0) Palette.cyan else Palette.fg,
                        size = 12,
                        bold = r == 0,
                        modifier = Modifier.width(widths[c]).padding(horizontal = 6.dp, vertical = 4.dp),
                    )
                }
            }
            Hairline()
        }
    }
}
