package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.jw.next.client.outline
import dev.jw.next.client.viewerSub
import dev.jw.next.contract.AskOption
import dev.jw.next.contract.MarkdownDoc
import kotlinx.coroutines.launch

private val Gutter = 16.dp

/** The ways to answer the plan the viewer shows: the question's own [options], whether it still [waits] and what [answer] does with a key (REQ-480). */
internal class PlanAnswers(val options: List<AskOption>, val waits: Boolean, val answer: (String, String?) -> Unit)

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
    var writing by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Mono("plan listo", Palette.yellow, 11)
        plan.options.forEachIndexed { i, o ->
            // «Seguir planeando» asks what to do next first (REQ-595).
            if (o.key == "no" && writing) {
                InstructionBox(busy = false, back = { writing = false }) { text -> plan.answer(o.key, text) }
                return@forEachIndexed
            }
            Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                    .background(if (i == 0) Palette.green else Palette.sel, RoundedCornerShape(8.dp))
                    .clickable { if (o.key == "no") writing = true else plan.answer(o.key, null) }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                contentAlignment = Alignment.CenterStart,
            ) { Mono(o.label, if (i == 0) Palette.panel else if (o.key == "no") Palette.red else Palette.fg, 13, bold = i == 0, lines = 3) }
        }
    }
}
