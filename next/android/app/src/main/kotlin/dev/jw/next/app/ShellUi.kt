package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.BlockState
import dev.jw.next.client.SpanColor
import dev.jw.next.client.shortPath
import dev.jw.next.client.spanColor
import dev.jw.next.client.state
import dev.jw.next.client.timeLabel
import dev.jw.next.contract.Block
import dev.jw.next.contract.Span
import kotlinx.coroutines.delay

/** The sixteen named colours are the theme's; the rest come resolved from the contract. */
private fun SpanColor.color(): Color = when (this) {
    is SpanColor.Rgb -> Color(argb)
    is SpanColor.Named -> when (index) {
        0 -> Color(0xFF3F4451)
        1, 9 -> Palette.red
        2, 10 -> Palette.green
        3, 11 -> Palette.yellow
        4, 12 -> Palette.blue
        5, 13 -> Palette.magenta
        6, 14 -> Palette.cyan
        7 -> Palette.fg
        8 -> Palette.dim
        else -> Color.White
    }
}

/** A block's output rows as styled text; [base] colours what the spans leave unset. */
private fun output(rows: List<List<Span>>, base: Color): AnnotatedString = buildAnnotatedString {
    rows.forEachIndexed { i, row ->
        row.forEach { s ->
            withStyle(
                SpanStyle(
                    color = s.fg?.spanColor()?.color() ?: base,
                    background = s.bg?.spanColor()?.color() ?: Color.Unspecified,
                    fontWeight = if (s.b) FontWeight.Bold else null,
                    fontStyle = if (s.i) FontStyle.Italic else null,
                    textDecoration = if (s.u) TextDecoration.Underline else null,
                ),
            ) { append(s.t) }
        }
        if (i < rows.lastIndex) append('\n')
    }
}

/** The blocks of a shell, the newest at the bottom next to the editor; a tap picks one. */
@Composable
fun BlocksList(blocks: List<Block>, picked: Long?, modifier: Modifier = Modifier, pick: (Long?) -> Unit) {
    val running = blocks.lastOrNull()?.running == true
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(running) {
        while (running) {
            now = System.currentTimeMillis()
            delay(1000)
        }
    }
    LazyColumn(modifier.fillMaxSize().background(Palette.bg), reverseLayout = true) {
        items(blocks.asReversed(), key = { it.id }) { b ->
            BlockCard(b, b.id == picked, now) { pick(if (b.id == picked) null else b.id) }
        }
    }
}

@Composable
private fun BlockCard(b: Block, selected: Boolean, now: Long, click: () -> Unit) {
    val state = b.state()
    val bar = when (state) {
        BlockState.Failed -> Palette.red
        BlockState.Running -> Palette.yellow
        BlockState.Ok -> null
    }
    val tint = when (state) {
        BlockState.Failed -> Palette.red.copy(alpha = 0.09f).compositeOver(Palette.bg)
        BlockState.Running -> Palette.yellow.copy(alpha = 0.06f).compositeOver(Palette.bg)
        BlockState.Ok -> Palette.bg
    }
    val timeColor = when (state) {
        BlockState.Failed -> Palette.red
        BlockState.Running -> Palette.yellow
        BlockState.Ok -> Palette.green
    }
    Column(
        Modifier.fillMaxWidth()
            .background(if (selected) Palette.sel else tint)
            .drawBehind {
                drawRect(Palette.line, Offset.Zero, Size(size.width, 1.dp.toPx()))
                if (bar != null) drawRect(bar, Offset.Zero, Size(3.dp.toPx(), size.height))
            }
            .let { if (selected) it.border(2.dp, Palette.blue) else it }
            .clickable(onClick = click)
            .padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Mono(shortPath(b.cwd), Palette.dim, 11, modifier = Modifier.weight(1f))
            Mono(b.timeLabel(now), timeColor, 11)
        }
        Mono(b.cmd, Palette.fg, 14, bold = true, lines = 6, modifier = Modifier.padding(top = 5.dp, bottom = 7.dp))
        if (b.truncated) Mono("… el inicio se recortó", Palette.dim, 11)
        if (b.full) Mono("pantalla completa", Palette.dim, 11)
        if (b.out.isNotEmpty()) {
            Text(
                output(b.out, if (state == BlockState.Failed) Palette.red else Palette.fg),
                fontFamily = MonoFont, fontSize = 12.sp, lineHeight = 18.sp,
            )
        }
    }
}

/** What can be done with the picked block: copy its command or output, run it again. */
@Composable
fun BlockActions(canRepeat: Boolean, copyCmd: () -> Unit, copyOut: () -> Unit, repeat: () -> Unit) {
    Box(Modifier.fillMaxWidth().background(Palette.bg).padding(horizontal = 12.dp, vertical = 8.dp), contentAlignment = Alignment.Center) {
        Row(
            Modifier.background(Palette.panel, RoundedCornerShape(14.dp)).border(1.dp, Palette.line, RoundedCornerShape(14.dp)).padding(4.dp),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Action("›_", "Comando", true, copyCmd)
            Action("⧉", "Salida", true, copyOut)
            Action("↻", "Repetir", canRepeat, repeat)
        }
    }
}

@Composable
private fun Action(glyph: String, label: String, enabled: Boolean, click: () -> Unit) {
    val colour = if (enabled) Palette.fg else Palette.dim
    Column(
        Modifier.width(74.dp).height(52.dp).clickable(enabled = enabled, onClick = click),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Mono(glyph, colour, 16)
        Mono(label, colour, 10)
    }
}

/** A small rounded fact above the input: the folder, the branch, or the running program. */
@Composable
fun Chip(text: String, color: Color) {
    Box(Modifier.height(26.dp).background(Palette.sel, RoundedCornerShape(13.dp)).padding(horizontal = 10.dp), contentAlignment = Alignment.Center) {
        Mono(text, color, 11)
    }
}
