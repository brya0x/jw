package dev.jw.next.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.jw.next.client.BinaryFile
import dev.jw.next.client.ClientState
import dev.jw.next.client.FileMark
import dev.jw.next.client.FileRow
import dev.jw.next.client.FilesView
import dev.jw.next.client.MarkTone
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.binaryNote
import dev.jw.next.client.crumbsOf
import dev.jw.next.client.filesView
import dev.jw.next.client.isMarkdown
import dev.jw.next.contract.FileKind
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.TextDoc
import kotlinx.coroutines.delay
import java.util.concurrent.atomic.AtomicInteger

private val Gutter = 16.dp

/** The last listing of each folder, so a folder opens with what it held and the new answer replaces it (REQ-636). */
internal object FileListings {
    private const val MAX = 64
    private val kept = object : LinkedHashMap<String, FileListing>(MAX, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, FileListing>) = size > MAX
    }

    fun get(workspace: String, path: String): FileListing? = synchronized(kept) { kept["$workspace\u0000$path"] }

    fun put(workspace: String, path: String, listing: FileListing) {
        synchronized(kept) { kept["$workspace\u0000$path"] = listing }
    }

    fun preload(source: AppSource, workspace: String) {
        source.listFiles(workspace, null) { r -> r.onSuccess { put(workspace, "", it) } }
    }
}

/**
 * The files of a workspace, folder by folder and no higher than its own (REQ-323 to REQ-328): the
 * PC lists one folder at a time with what git says of each entry. Back goes up a folder, and
 * out of the screen at the workspace's own. A `.md` opens in the viewer (REQ-479); any other file
 * opens as text, coloured, over the whole screen (REQ-618), or says that it is binary (REQ-613).
 */
@Composable
fun FilesScreen(source: AppSource, state: ClientState, screen: Screen.Files, nav: Navigator, set: (ClientState) -> Unit) {
    val version = source.tick.intValue
    val name = remember(version, screen.workspace) { source.session().workspaces.firstOrNull { it.id == screen.workspace }?.name ?: "workspace" }
    var path by remember(screen.workspace) { mutableStateOf("") }
    var hidden by remember(screen.workspace) { mutableStateOf(false) }
    var listing by remember(screen.workspace) { mutableStateOf<FileListing?>(null) }
    var error by remember(screen.workspace) { mutableStateOf<String?>(null) }
    var again by remember(screen.workspace) { mutableIntStateOf(0) }
    var viewing by remember(screen.workspace) { mutableStateOf<MarkdownDoc?>(null) }
    var reading by remember(screen.workspace) { mutableStateOf<TextDoc?>(null) }
    var refused by remember(screen.workspace) { mutableStateOf<Pair<String, String>?>(null) }
    var note by remember(screen.workspace) { mutableStateOf<String?>(null) }
    val asks = remember(screen.workspace) { AtomicInteger() }
    val view = listing?.let { filesView(name, it, hidden) }
    val leave = { set(nav.leaveFiles(state)) }
    BackHandler { view?.parent?.let { path = it } ?: leave() }
    LaunchedEffect(note) { if (note != null) { delay(3500); note = null } }
    val openFile = { row: FileRow ->
        if (isMarkdown(row.name)) {
            source.readMarkdown(screen.workspace, row.path) { result ->
                result.onSuccess { viewing = it }.onFailure { note = it.message ?: "el PC no pudo leer ${row.name}" }
            }
        } else {
            source.readFile(screen.workspace, row.path) { result ->
                result.onSuccess { reading = it }.onFailure {
                    if (it is BinaryFile) refused = row.name to binaryNote(row.size) else note = it.message ?: "el PC no pudo leer ${row.name}"
                }
            }
        }
    }
    LaunchedEffect(screen.workspace, path, again) {
        error = null
        // What the folder held last time is shown while the new answer comes.
        listing = FileListings.get(screen.workspace, path)
        val asked = asks.incrementAndGet()
        source.listFiles(screen.workspace, path.ifEmpty { null }) { result ->
            // A slower answer to an earlier tap must not replace the folder asked for last.
            if (asked == asks.get()) {
                result.onSuccess { FileListings.put(screen.workspace, path, it); listing = it }
                    .onFailure { if (listing == null) error = it.message ?: "el PC no pudo leer la carpeta" }
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        Header("$name · archivos", "/$path", onBack = leave, action = {
            Box(Modifier.defaultMinSize(minWidth = 72.dp, minHeight = 48.dp).clickable { hidden = !hidden }, contentAlignment = Alignment.Center) {
                Mono(".ocultos", if (hidden) Palette.blue else Palette.dim, 12)
            }
        })
        Crumbs(crumbsOf(name, path), path) { path = it }
        note?.let { Mono(it, Palette.yellow, 12, lines = 2, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp)) }
        Box(Modifier.weight(1f)) {
            when {
                error != null -> Column(
                    Modifier.fillMaxSize().padding(Gutter),
                    verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Mono(error.orEmpty(), Palette.red, 13, lines = 4)
                    SheetAction("↻", "leer otra vez", Palette.blue) { again++ }
                    view?.parent?.let { up -> SheetAction("‹", "subir", Palette.blue) { path = up } }
                }
                view == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Mono("leyendo…", Palette.dim, 13) }
                else -> FileList(view, { path = it }, openFile)
            }
        }
    }
    viewing?.let { doc -> ViewerDialog(doc, null) { viewing = null } }
    reading?.let { doc -> TextViewerDialog(doc) { reading = null } }
    refused?.let { (file, why) -> RefusalDialog(file, why) { refused = null } }
}

@Composable
private fun Crumbs(crumbs: List<Pair<String, String>>, here: String, go: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().background(Palette.panel).horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        crumbs.forEachIndexed { i, (label, at) ->
            if (i > 0) Mono("›", Palette.dim, 12)
            Box(Modifier.defaultMinSize(minHeight = 44.dp).clickable { go(at) }.padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                Mono(label, if (at == here) Palette.fg else Palette.dim, 12)
            }
        }
    }
    Hairline()
}

@Composable
private fun FileList(view: FilesView, go: (String) -> Unit, open: (FileRow) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        view.parent?.let { up -> item(key = "..") { FileRowView("‹", Palette.dim, "..", "/$up", null, false) { go(up) } } }
        items(view.rows, key = { it.path }) { row -> FileLine(row, go, open) }
        if (view.rows.isEmpty()) item { Mono(if (view.hidden || view.total == 0) "carpeta vacía" else "solo hay archivos ocultos", Palette.dim, 13, modifier = Modifier.padding(Gutter)) }
        if (view.cut) item { Mono("se listan los primeros ${view.sent} de ${view.total}", Palette.dim, 11, lines = 2, modifier = Modifier.padding(Gutter)) }
    }
}

/** A folder goes in when it is tapped and a file is opened (REQ-479, REQ-618); a link is listed and does nothing (REQ-325). */
@Composable
private fun FileLine(row: FileRow, go: (String) -> Unit, open: (FileRow) -> Unit) {
    val (glyph, tone) = when (row.kind) {
        FileKind.Dir -> "▸" to Palette.blue
        FileKind.Link -> "↗" to Palette.dim
        FileKind.File -> "·" to Palette.dim
    }
    val click: (() -> Unit)? = when (row.kind) {
        FileKind.Dir -> ({ go(row.path) })
        FileKind.File -> ({ open(row) })
        FileKind.Link -> null
    }
    FileRowView(glyph, tone, row.name, row.detail, row.mark, row.ignored, click)
}

@Composable
private fun FileRowView(glyph: String, tone: Color, name: String, detail: String?, mark: FileMark?, dim: Boolean, click: (() -> Unit)?) {
    Row(
        Modifier.fillMaxWidth().alpha(if (dim) 0.5f else 1f).defaultMinSize(minHeight = 48.dp)
            .then(if (click != null) Modifier.clickable(onClick = click) else Modifier)
            .padding(horizontal = Gutter, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Mono(glyph, tone, 13, modifier = Modifier.padding(end = 12.dp))
        Column(Modifier.weight(1f)) {
            Mono(name, bold = click != null, size = 14)
            if (detail != null) Mono(detail, Palette.dim, 11)
        }
        if (mark != null) Mono(mark.glyph, markColor(mark.tone), 12, bold = true, modifier = Modifier.padding(start = 8.dp))
    }
    Hairline()
}

private fun markColor(tone: MarkTone): Color = when (tone) {
    MarkTone.Changed -> Palette.yellow
    MarkTone.New -> Palette.green
    MarkTone.Bad -> Palette.red
    MarkTone.Ignored -> Palette.dim
}
