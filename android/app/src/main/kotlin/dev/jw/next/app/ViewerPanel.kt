package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.jw.next.client.BinaryFile
import dev.jw.next.client.Draft
import dev.jw.next.client.binaryNote
import dev.jw.next.client.isMarkdown
import dev.jw.next.contract.Id
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.TextDoc
import dev.jw.next.contract.ViewSummary

/**
 * The files of a workspace's viewer, in the body of its tab (REQ-724): a chip for each file when there are several, and the
 * one chosen read from the PC and shown with the same viewers the folder screen has. Back closes that file (the last closes
 * the tab). The tabs themselves are the PC's: a file the terminal client opened is a chip here too.
 */
@Composable
internal fun ColumnScope.ViewerPanel(source: AppSource, workspace: Id, view: ViewSummary, wanted: String?, closeFile: (String) -> Unit, refer: (Draft) -> Unit) {
    val files = view.files
    // The file the person asked for is the one shown, whichever the PC lists last.
    var chosen by remember(view.id, wanted) { mutableStateOf(wanted) }
    val path = files.firstOrNull { it == chosen } ?: files.lastOrNull()
    if (files.size > 1) {
        Row(Modifier.fillMaxWidth().background(Palette.panel).horizontalScroll(rememberScrollState())) {
            files.forEach { f ->
                Box(
                    Modifier.defaultMinSize(minHeight = 40.dp).clickable { chosen = f }.padding(horizontal = 12.dp),
                    contentAlignment = Alignment.Center,
                ) { Mono(f.substringAfterLast('/'), if (f == path) Palette.fg else Palette.dim, 12, bold = f == path) }
            }
        }
        Hairline()
    }
    if (path == null) {
        Box(Modifier.weight(1f).padding(16.dp)) { Mono("este panel no tiene archivos", Palette.dim, 13, lines = 2) }
        return
    }
    var markdown by remember(workspace, path) { mutableStateOf<MarkdownDoc?>(null) }
    var text by remember(workspace, path) { mutableStateOf<TextDoc?>(null) }
    var note by remember(workspace, path) { mutableStateOf<String?>(null) }
    LaunchedEffect(workspace, path) {
        if (isMarkdown(path)) {
            source.readMarkdown(workspace, path) { r -> r.onSuccess { markdown = it }.onFailure { note = it.message ?: "el PC no pudo leer $path" } }
        } else {
            source.readFile(workspace, path) { r ->
                r.onSuccess { text = it }.onFailure { note = if (it is BinaryFile) binaryNote(null) else it.message ?: "el PC no pudo leer $path" }
            }
        }
    }
    val md = markdown
    val tx = text
    when {
        md != null -> Viewer(md, null) { closeFile(path) }
        tx != null -> TextViewer(tx, path, refer) { closeFile(path) }
        else -> Box(Modifier.weight(1f).padding(16.dp)) { Mono(note ?: "leyendo $path…", if (note != null) Palette.red else Palette.dim, 13, lines = 3) }
    }
}
