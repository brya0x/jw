package dev.jw.mobile

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.EntryKind
import dev.jw.proto.FileKind
import dev.jw.proto.FsEntry
import dev.jw.proto.FsFile
import dev.jw.proto.FsListing
import dev.jw.proto.MdBlock
import dev.jw.proto.MdSpan

private val Corner = RoundedCornerShape(6.dp)

/** The PC's reasons for refusing a listing or a read, in the app's words; anything else is shown as it came. */
internal fun fsReasonWords(msg: String) = when {
    msg == "permission denied" -> "sin permiso"
    msg == "no such file or folder" -> "no existe"
    msg == "not a folder" -> "no es una carpeta"
    msg == "it is a folder" -> "es una carpeta"
    msg == "it could not be read" -> "no se pudo leer"
    msg.startsWith("took longer") -> "el PC tardó demasiado"
    msg.startsWith("the PC's files are not answering") -> "el PC no responde con sus archivos"
    msg.startsWith("busy") -> "el PC está ocupado: intenta de nuevo"
    msg == "not an absolute path" || msg.startsWith("a path with ..") -> "ruta no válida"
    else -> msg
}

/** `parent/name` for the PC, whose root is `/`. */
internal fun joinPath(dir: String, name: String) = if (dir.endsWith("/")) dir + name else "$dir/$name"

/** `1.2 MB`, `4 KB`, `812 B`. */
internal fun sizeWords(n: Long): String = when {
    n < 1024 -> "$n B"
    n < 1024 * 1024 -> "${n / 1024} KB"
    n < 1024L * 1024 * 1024 -> "%.1f MB".format(n / 1024.0 / 1024.0)
    else -> "%.1f GB".format(n / 1024.0 / 1024.0 / 1024.0)
}

/** The PC's file tree from a workspace (REQ-83): a folder at a time, with the way back up, and a file opens in the viewer. */
@Composable
fun FilesScreen(session: Session, nav: Nav, workspace: String, state: UiState.Connected) {
    val j = LocalJw.current
    val path = nav.filesPath ?: return
    val fs by session.fs.collectAsState()
    var hidden by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(path) { session.requestLs(path) }
    DisposableEffect(Unit) { onDispose { session.clearFiles() } }
    BackHandler { nav.filesPath = null }

    val listing = (fs as? FsState.Loaded)?.listing?.takeIf { it.req == path }

    Column(Modifier.fillMaxSize().background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 4.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(48.dp).clickable(role = Role.Button, onClickLabel = "Volver") { nav.filesPath = null }, contentAlignment = Alignment.Center) { Icon(Ic.Back, j.fg, 22.dp) }
            Column(Modifier.weight(1f)) {
                Text("$workspace · archivos", color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(listing?.path ?: path, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Box(
                Modifier.heightIn(min = 48.dp).clickable(role = Role.Switch, onClickLabel = "Archivos ocultos") { hidden = !hidden }.padding(horizontal = 10.dp),
                contentAlignment = Alignment.Center,
            ) { Text(".ocultos", color = if (hidden) j.blue else j.dim, fontSize = 12.sp) }
        }
        Crumbs(listing?.path ?: path) { nav.filesPath = it }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (val f = fs) {
                is FsState.Failed -> if (f.req == path) Failure(fsReasonWords(f.why), "subir", if (listing?.parent != null || path != "/") ({ nav.filesPath = parentOf(path) ?: "/" }) else null) { session.requestLs(path) }
                else -> if (listing == null) LoadingBox("leyendo $path") else EntryList(listing, hidden, nav, session)
            }
        }
        StatusBar("NORMAL", j.blue, "toca un archivo", route = state.route)
    }
}

private fun parentOf(p: String): String? = p.trimEnd('/').substringBeforeLast('/', "").ifEmpty { if (p == "/") null else "/" }

@Composable
private fun Crumbs(path: String, onGo: (String) -> Unit) {
    val j = LocalJw.current
    val parts = path.split('/').filter { it.isNotEmpty() }
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp).heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        var acc = ""
        Box(Modifier.widthIn(min = 48.dp).heightIn(min = 48.dp).clickable(role = Role.Button) { onGo("/") }.padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
            Text("/", color = if (parts.isEmpty()) j.fg else j.dim, fontSize = 12.sp)
        }
        parts.forEachIndexed { i, name ->
            acc += "/$name"
            val to = acc
            Text("›", color = j.dim, fontSize = 12.sp)
            Box(Modifier.widthIn(min = 48.dp).heightIn(min = 48.dp).clickable(role = Role.Button) { onGo(to) }.padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                Text(name, color = if (i == parts.lastIndex) j.fg else j.dim, fontSize = 12.sp, maxLines = 1)
            }
        }
    }
}

@Composable
private fun EntryList(l: FsListing, hidden: Boolean, nav: Nav, session: Session) {
    val j = LocalJw.current
    val shown = remember(l, hidden) { l.entries.filter { hidden || !it.name.startsWith(".") } }
    LazyColumn(Modifier.fillMaxSize()) {
        l.parent?.let { up -> item(key = "..") { EntryRow("‹", "..", up, true, false) { nav.filesPath = up } } }
        items(shown, key = { it.name }) { e ->
            val full = joinPath(l.path, e.name)
            EntryRow(
                glyph = when { e.isFolder -> "▸"; e.kind == EntryKind.Link -> "↗"; else -> "·" },
                name = e.name,
                sub = entrySub(e),
                folder = e.isFolder,
                md = e.md,
            ) { if (e.isFolder) nav.filesPath = full else nav.viewerPath = full }
        }
        if (shown.isEmpty()) item(key = "empty") { Text("carpeta vacía", color = j.dim, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth().padding(30.dp)) }
        if (l.truncated) item(key = "cut") { NoteLine("se listan los primeros ${l.entries.size} de ${l.total} elementos") }
    }
}

private fun entrySub(e: FsEntry): String = when {
    e.kind == EntryKind.Link -> "enlace · " + when (e.target) { "dir" -> "a una carpeta"; "file" -> "a un archivo"; else -> "roto" }
    e.kind == EntryKind.File -> e.size?.let(::sizeWords).orEmpty()
    else -> ""
}

@Composable
private fun EntryRow(glyph: String, name: String, sub: String, folder: Boolean, md: Boolean, onClick: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .clickable(role = Role.Button, onClick = onClick).padding(horizontal = 14.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(glyph, color = if (folder) j.blue else if (md) j.cyan else j.dim, modifier = Modifier.width(18.dp), textAlign = TextAlign.Center)
        Column(Modifier.weight(1f)) {
            Text(name, color = j.fg, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (sub.isNotEmpty()) Text(sub, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (md) Text("md", color = j.cyan, fontSize = 10.5.sp, modifier = Modifier.border(BorderStroke(1.dp, j.line), RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 1.dp))
    }
}

@Composable
private fun NoteLine(text: String) {
    val j = LocalJw.current
    Text(
        text, color = j.yellow, fontSize = 11.5.sp,
        modifier = Modifier.fillMaxWidth().background(j.sel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }.padding(horizontal = 14.dp, vertical = 10.dp),
    )
}

@Composable
private fun LoadingBox(what: String) {
    val j = LocalJw.current
    Column(Modifier.fillMaxWidth().padding(44.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        CircularProgressIndicator(color = j.blue, strokeWidth = 2.dp, modifier = Modifier.size(22.dp).semantics { contentDescription = "Cargando" })
        Text(what, color = j.dim, fontSize = 12.sp, textAlign = TextAlign.Center)
    }
}

@Composable
private fun Failure(why: String, upLabel: String, onUp: (() -> Unit)?, onRetry: () -> Unit) {
    val j = LocalJw.current
    Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 44.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(why, color = j.red, fontSize = 14.sp, textAlign = TextAlign.Center)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.clickable(role = Role.Button, onClick = onRetry).minimumTouch().border(BorderStroke(1.dp, j.line), Corner).padding(horizontal = 18.dp), contentAlignment = Alignment.Center) { Text("reintentar", color = j.fg, fontSize = 13.sp) }
            if (onUp != null) Box(Modifier.clickable(role = Role.Button, onClick = onUp).minimumTouch().border(BorderStroke(1.dp, j.line), Corner).padding(horizontal = 18.dp), contentAlignment = Alignment.Center) { Text(upLabel, color = j.fg, fontSize = 13.sp) }
        }
    }
}

/** A press target of at least 48 dp whose drawn box stays as it is (REQ-26). */
private fun Modifier.minimumTouch(): Modifier = this.then(Modifier.heightIn(min = 48.dp))

/** A file of the PC (REQ-84): text with line numbers, Markdown formatted with a switch to its source. */
@Composable
fun FileViewerScreen(session: Session, nav: Nav, state: UiState.Connected) {
    val j = LocalJw.current
    val path = nav.viewerPath ?: return
    val fileState by session.file.collectAsState()
    var source by rememberSaveable(path) { mutableStateOf(false) }
    LaunchedEffect(path) { session.requestRead(path) }
    BackHandler { nav.viewerPath = null }
    val file = (fileState as? FileState.Loaded)?.file?.takeIf { it.req == path }
    val name = path.substringAfterLast('/')

    Column(Modifier.fillMaxSize().background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 4.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(48.dp).clickable(role = Role.Button, onClickLabel = "Volver") { nav.viewerPath = null }, contentAlignment = Alignment.Center) { Icon(Ic.Back, j.fg, 22.dp) }
            Column(Modifier.weight(1f)) {
                Text(name, color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(path, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (file?.kind == FileKind.Md) {
            Row(Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }) {
                for ((label, on) in listOf("vista" to false, "fuente" to true)) {
                    val sel = source == on
                    Box(
                        Modifier.weight(1f).heightIn(min = 48.dp).clickable(role = Role.Tab) { source = on }
                            .drawBehind { if (sel) drawLine(j.blue, Offset(0f, size.height), Offset(size.width, size.height), 2.dp.toPx()) },
                        contentAlignment = Alignment.Center,
                    ) { Text(label, color = if (sel) j.blue else j.dim, fontSize = 12.sp) }
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (val s = fileState) {
                is FileState.Failed -> if (s.req == path) Failure(fsReasonWords(s.why), "", null) { session.requestRead(path) }
                else -> if (file == null) LoadingBox("leyendo $name") else FileBody(file, source)
            }
        }
        val hint = when {
            file == null -> if (fileState is FileState.Failed) "no se pudo leer" else "cargando"
            file.kind == FileKind.Binary -> "binario"
            file.truncated -> "primer 1 MiB de ${sizeWords(file.size)}"
            file.kind == FileKind.Md -> if (source) "markdown · fuente" else "markdown · vista"
            else -> "texto"
        }
        StatusBar("NORMAL", j.blue, hint, route = state.route)
    }
}

@Composable
private fun FileBody(f: FsFile, source: Boolean) {
    val j = LocalJw.current
    val text = f.text
    val blocks = f.blocks
    when {
        f.kind == FileKind.Binary || text == null -> Column(Modifier.fillMaxWidth().padding(40.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("no es texto", color = j.fg, fontSize = 14.sp)
            Text("${f.path.substringAfterLast('/')} · ${sizeWords(f.size)}\nel teléfono solo muestra texto.", color = j.dim, fontSize = 12.sp, textAlign = TextAlign.Center)
        }
        f.kind == FileKind.Md && !source && blocks != null -> LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (f.truncated) item(key = "cut") { NoteLine("se muestra el primer 1 MiB: el archivo es más grande") }
            itemsIndexed(blocks) { _, b -> MdBlockView(b) }
        }
        else -> {
            val lines = remember(f) { text.split('\n') }
            LazyColumn(Modifier.fillMaxSize()) {
                if (f.truncated) item(key = "cut") { NoteLine("se muestra el primer 1 MiB: el archivo es más grande") }
                itemsIndexed(lines) { i, l ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("${i + 1}", color = j.dim, fontSize = 10.5.sp, textAlign = TextAlign.End, modifier = Modifier.widthIn(min = 30.dp))
                        Text(l.ifEmpty { " " }, color = j.fg, fontSize = 11.5.sp, modifier = Modifier.weight(1f))
                    }
                }
            }
        }
    }
}

@Composable
private fun spansText(spans: List<MdSpan>, base: SpanStyle = SpanStyle()): AnnotatedString {
    val j = LocalJw.current
    return buildAnnotatedString {
        for (sp in spans) {
            val st = SpanStyle(
                fontWeight = if (sp.b) FontWeight.Bold else null,
                fontStyle = if (sp.i) FontStyle.Italic else null,
                textDecoration = when {
                    sp.href != null -> TextDecoration.Underline
                    sp.s -> TextDecoration.LineThrough
                    else -> null
                },
                color = if (sp.href != null) j.blue else androidx.compose.ui.graphics.Color.Unspecified,
                background = if (sp.code) j.sel else androidx.compose.ui.graphics.Color.Unspecified,
            )
            withStyle(base.merge(st)) { append(sp.t) }
        }
    }
}

/** A Markdown block as the PC's parser made it (REQ-81): nothing here parses Markdown. */
@Composable
private fun MdBlockView(b: MdBlock) {
    val j = LocalJw.current
    when (b) {
        is MdBlock.Heading -> Text(
            spansText(b.spans), color = when (b.level) { 1 -> j.fg; 2 -> j.blue; else -> j.cyan },
            fontWeight = FontWeight.Bold, fontSize = when (b.level) { 1 -> 17.sp; 2 -> 15.sp; else -> 13.5.sp },
            modifier = Modifier.padding(top = if (b.level > 1) 8.dp else 0.dp),
        )
        is MdBlock.Paragraph -> Text(spansText(b.spans), color = j.fg, fontSize = 13.sp, lineHeight = 20.sp)
        is MdBlock.Code -> Box(Modifier.fillMaxWidth().background(j.sel, Corner).horizontalScroll(rememberScrollState()).padding(10.dp)) {
            Text(b.text, color = j.fg, fontSize = 11.5.sp, softWrap = false)
        }
        is MdBlock.Ul -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            b.items.forEach { item -> MdItem("•", item) }
        }
        is MdBlock.Ol -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            b.items.forEachIndexed { i, item -> MdItem("${b.start + i}.", item) }
        }
        is MdBlock.Quote -> Row(Modifier.fillMaxWidth().drawBehind { drawLine(j.line, Offset(1.5.dp.toPx(), 0f), Offset(1.5.dp.toPx(), size.height), 3.dp.toPx()) }.padding(start = 12.dp)) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) { b.blocks.forEach { MdBlockView(it) } }
        }
        MdBlock.Rule -> Box(Modifier.fillMaxWidth().heightIn(min = 1.dp).background(j.line))
        is MdBlock.Table -> Column(Modifier.horizontalScroll(rememberScrollState())) {
            (listOf(b.head) + b.rows).filter { it.isNotEmpty() }.forEachIndexed { r, row ->
                Row {
                    row.forEach { cell ->
                        Text(
                            spansText(cell, if (r == 0 && b.head.isNotEmpty()) SpanStyle(fontWeight = FontWeight.Bold) else SpanStyle()), color = j.fg, fontSize = 12.sp,
                            modifier = Modifier.width(110.dp).border(BorderStroke(1.dp, j.line)).padding(horizontal = 8.dp, vertical = 4.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun MdItem(bullet: String, blocks: List<MdBlock>) {
    val j = LocalJw.current
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(bullet, color = j.dim, fontSize = 13.sp, modifier = Modifier.widthIn(min = 16.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) { blocks.forEach { MdBlockView(it) } }
    }
}
