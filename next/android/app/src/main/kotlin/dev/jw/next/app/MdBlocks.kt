package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
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
import dev.jw.next.client.plain
import dev.jw.next.contract.Inline
import dev.jw.next.contract.MdBlock
import dev.jw.next.contract.MdItem

/**
 * Markdown blocks the PC parsed, drawn the same way for the viewer (REQ-481) and for what Claude
 * wrote in the conversation (REQ-560): the theme's colours, tables that scroll sideways inside the
 * screen.
 */
@Composable
internal fun MdBlocks(blocks: List<MdBlock>, modifier: Modifier = Modifier) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) { blocks.forEach { BlockView(it) } }
}

internal fun headingColor(level: Int): Color = when (level) {
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
internal fun BlockView(block: MdBlock) {
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
            block.items.forEachIndexed { i, item -> MdItemView(item, block.start?.let { "${it + i}." }) }
        }
        is MdBlock.Code -> Column(Modifier.fillMaxWidth().background(Palette.panel, RoundedCornerShape(6.dp)).padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Mono(block.lang.ifEmpty { "código" }, Palette.dim, 10)
            Text(
                block.text.trimEnd('\n'),
                color = Palette.fg,
                fontFamily = MonoFont,
                fontSize = 11.5.sp,
            )
        }
        is MdBlock.Table -> TableView(block)
        MdBlock.Rule -> Hairline()
    }
}

/** An item of a list: its mark (a number, a bullet or a checkbox) and its blocks. */
@Composable
private fun MdItemView(item: MdItem, number: String?) {
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
