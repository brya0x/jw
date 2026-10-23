package dev.jw.next.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.ChangesModel
import dev.jw.next.client.ChangesView
import dev.jw.next.client.Content
import dev.jw.next.client.HunkRow
import dev.jw.next.client.LineKind
import dev.jw.next.client.Live
import dev.jw.next.client.TreeRow
import dev.jw.next.client.ago
import dev.jw.next.client.hunkRows
import dev.jw.next.client.treeRows
import dev.jw.next.client.whyUnread
import dev.jw.next.contract.DiffFile
import dev.jw.next.contract.Id
import dev.jw.next.contract.FileStatus
import dev.jw.next.contract.Seen
import kotlinx.coroutines.delay

private val Gutter = 16.dp
private val AddBg = Color(0xFF2A3B2C)
private val DelBg = Color(0xFF40292D)

/**
 * The changes of one workspace, as `docs/prototypes/mobile.html` draws them (REQ-113 to REQ-121), in the body of the diff's tab
 * of the workspace screen (REQ-730). [leave] is Back when there is nothing to go back to inside it; [openFile] asks the PC for a
 * viewer tab of a file (REQ-731).
 */
@Composable
fun ColumnScope.DiffPanel(source: AppSource, workspace: Id, leave: () -> Unit, openFile: (String) -> Unit) {
    val model = remember(source, workspace) { ChangesModel(source, workspace) }
    val bump = remember(model) { mutableIntStateOf(0) }
    DisposableEffect(model) {
        model.onChange = { bump.intValue++ }
        onDispose {
            model.onChange = {}
            model.close()
        }
    }
    BackHandler { if (!model.back()) leave() }
    // Either the PC sent a node or the person did something: read the model again.
    val version = source.tick.intValue + bump.intValue
    val content = remember(version) {
        model.onNodes()
        model.content()
    }
    val counts = remember(version) { ChangesView.entries.associateWith(model::count) }
    val updatedFor = model.updatedFor()
    LaunchedEffect(version) {
        if (updatedFor > 0) {
            delay(updatedFor + 50)
            bump.intValue++
        }
    }
    val open = (content as? Content.Files)?.diff?.let(model::openFileIn)
    if (open != null) {
        FileScreen(open, model, openFile)
    } else {
        ChangesHeader(model, content)
        ViewTabs(model, counts)
        Box(Modifier.weight(1f)) { ChangesBody(model, content, version) }
    }
}

@Composable
private fun ChangesHeader(model: ChangesModel, content: Content) {
    val sub = when (model.view) {
        ChangesView.Uncommitted -> "sin commitear" + if (content is Content.Files) liveText(model) + seenText(content) else ""
        ChangesView.Commits -> model.commit?.let { "commit ${it.take(7)}" } ?: "commits"
        ChangesView.General -> (content as? Content.Files)?.diff?.base ?: "contra la rama por defecto"
    }
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(horizontal = Gutter)) {
                Mono("cambios", bold = true, size = 15)
                Mono(sub, if (model.live() == Live.Updated && model.view == ChangesView.Uncommitted) Palette.yellow else Palette.dim, 11)
            }
        }
        Hairline()
    }
}

private fun liveText(model: ChangesModel): String =
    if (model.live() == Live.Updated) " · ● actualizado" else " · ● en vivo"

private fun seenText(content: Content.Files): String {
    val files = content.diff.files
    return if (content.diff.tooLarge || files.isEmpty()) "" else " · ${files.count { it.seen == Seen.Yes }}/${files.size} vistos"
}

@Composable
private fun ViewTabs(model: ChangesModel, counts: Map<ChangesView, String?>) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Row(Modifier.fillMaxWidth().height(44.dp)) {
            listOf(ChangesView.Uncommitted to "Sin commitear", ChangesView.Commits to "Commits", ChangesView.General to "General").forEach { (view, label) ->
                val on = model.view == view
                Column(Modifier.weight(1f).clickable { model.select(view) }, horizontalAlignment = Alignment.CenterHorizontally) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Mono(label, if (on) Palette.blue else Palette.dim, 12, bold = on)
                        counts[view]?.let { Mono(it, if (on) Palette.blue else Palette.dim, 11) }
                    }
                    Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
                }
            }
        }
        Hairline()
    }
}

@Composable
private fun ChangesBody(model: ChangesModel, content: Content, version: Int) {
    when (content) {
        Content.Reading -> Centre("◌ leyendo ${if (model.view == ChangesView.Commits && model.commit == null) "los commits" else "los cambios"}", "git corre en el PC; puede tardar en repos grandes", Palette.fg)
        is Content.Failed -> Centre("no se pudo leer", content.why, Palette.red) {
            Mono("las otras vistas siguen disponibles", Palette.dim, 11)
            Spacer(Modifier.height(6.dp))
            Box(Modifier.defaultMinSize(minHeight = 44.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).clickable { model.retry() }.padding(horizontal = 18.dp), contentAlignment = Alignment.Center) {
                Mono("reintentar", Palette.fg, 13)
            }
        }
        Content.NoBase -> Centre("origin no tiene rama por defecto: no hay contra qué comparar", "mira «Sin commitear» o «Commits»", Palette.fg)
        is Content.CommitList -> CommitList(model, content)
        is Content.Files -> FilesList(model, content, version)
    }
}

@Composable
private fun Centre(title: String, detail: String, tone: Color, extra: (@Composable () -> Unit)? = null) {
    Column(Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 44.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Mono(title, tone, 14, lines = 3, modifier = Modifier.fillMaxWidth())
        Mono(detail, Palette.dim, 12, lines = 5, modifier = Modifier.fillMaxWidth())
        extra?.invoke()
    }
}

@Composable
private fun CommitList(model: ChangesModel, content: Content.CommitList) {
    val now = System.currentTimeMillis() / 1000
    if (content.list.commits.isEmpty()) return Centre("sin commits en la rama", "", Palette.dim)
    LazyColumn(Modifier.fillMaxSize()) {
        items(content.list.commits, key = { it.sha }) { c ->
            Column(Modifier.fillMaxWidth().clickable { model.openCommit(c.sha) }.padding(horizontal = Gutter, vertical = 10.dp)) {
                Mono(c.subject, bold = true, size = 13)
                Mono("${c.short} · ${ago(now, c.time)}", Palette.dim, 11, modifier = Modifier.padding(top = 2.dp))
            }
            Hairline()
        }
        if (content.list.more) item { Note("se listan los últimos 200 commits; hay más antiguos") }
    }
}

@Composable
private fun Note(text: String) {
    Box(Modifier.fillMaxWidth().background(Palette.sel).padding(horizontal = Gutter, vertical = 10.dp)) { Mono(text, Palette.yellow, 11, lines = 4) }
    Hairline()
}

@Composable
private fun FilesList(model: ChangesModel, content: Content.Files, version: Int) {
    val diff = content.diff
    val rows = remember(version, model.view, model.commit) { treeRows(diff.files) { model.isShut(it) } }
    Column(Modifier.fillMaxSize()) {
        if (model.view == ChangesView.Commits && model.commit != null) CommitBar(model)
        if (diff.files.isEmpty()) {
            val text = when (model.view) {
                ChangesView.General -> "nada commiteado contra el origen"
                ChangesView.Uncommitted -> "sin cambios sin commitear"
                ChangesView.Commits -> "este commit no cambió archivos"
            }
            Centre(text, "", Palette.dim)
            return@Column
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            if (diff.tooLarge) {
                item {
                    val left = diff.omitted?.let { " y faltan $it" } ?: ""
                    Note("este diff pasó de 2 MiB: se listan ${diff.files.size}$left, sin líneas. Míralo entero en el PC.")
                }
            }
            items(rows, key = { row -> when (row) { is TreeRow.Dir -> "d${row.key}"; is TreeRow.File -> "f${row.file.path}" } }) { row ->
                when (row) {
                    is TreeRow.Dir -> DirRow(row, diff.tooLarge) { model.toggleFolder(row.key) }
                    is TreeRow.File -> FileRow(row, diff.tooLarge, { model.openFile(row.file.path) }, { model.toggleViewed(row.file.path) })
                }
            }
        }
    }
}

/** `‹ commits`, the commit's subject, and the arrows to its neighbours. */
@Composable
private fun CommitBar(model: ChangesModel) {
    Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.defaultMinSize(minHeight = 44.dp).clickable { model.back() }.padding(horizontal = 8.dp), contentAlignment = Alignment.Center) { Mono("‹ commits", Palette.blue, 12, bold = true) }
        Mono(model.commit.orEmpty().take(7), Palette.dim, 11, modifier = Modifier.weight(1f))
        for ((delta, glyph) in listOf(-1 to "◂", 1 to "▸")) {
            val ok = model.hasCommit(delta)
            Box(Modifier.size(44.dp).clickable(enabled = ok) { model.stepCommit(delta) }, contentAlignment = Alignment.Center) { Mono(glyph, if (ok) Palette.fg else Palette.line, 16) }
        }
    }
    Hairline()
}

@Composable
private fun DirRow(row: TreeRow.Dir, big: Boolean, toggle: () -> Unit) {
    Row(Modifier.fillMaxWidth().background(Palette.panel).clickable(onClick = toggle).defaultMinSize(minHeight = 40.dp).padding(start = Gutter - 2.dp + 16.dp * row.depth, end = Gutter), verticalAlignment = Alignment.CenterVertically) {
        Mono(if (row.shut) "▸" else "▾", Palette.dim, 12, modifier = Modifier.width(18.dp))
        Mono("${row.label}/", bold = true, size = 13, modifier = Modifier.weight(1f))
        Mono("${row.files}", Palette.dim, 11)
        if (!big) Counts(row.added, row.deleted, Modifier.padding(start = 10.dp))
    }
    Hairline()
}

@Composable
private fun Counts(added: Int, deleted: Int, modifier: Modifier = Modifier) {
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Mono("+$added", Palette.green, 12)
        Mono("-$deleted", Palette.red, 12)
    }
}

private fun FileStatus.letter(): Pair<String, Color> = when (this) {
    FileStatus.Modified -> "M" to Palette.yellow
    FileStatus.Added -> "A" to Palette.green
    FileStatus.Deleted -> "D" to Palette.red
    FileStatus.Renamed -> "R" to Palette.cyan
    FileStatus.Untracked -> "?" to Palette.magenta
}

@Composable
private fun FileRow(row: TreeRow.File, big: Boolean, open: () -> Unit, mark: () -> Unit) {
    val f = row.file
    val (letter, tone) = f.status.letter()
    val seen = f.seen == Seen.Yes
    Row(
        Modifier.fillMaxWidth().clickable(onClick = open).defaultMinSize(minHeight = 52.dp).padding(start = Gutter - 2.dp + 16.dp * row.depth, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.size(18.dp).background(tone, RoundedCornerShape(4.dp)), contentAlignment = Alignment.Center) { Mono(letter, Palette.bg, 10, bold = true) }
        Column(Modifier.weight(1f)) {
            Mono(f.path.substringAfterLast('/'), if (seen) Palette.dim else Palette.fg, 13)
            val why = f.unreadable?.let(::whyUnread) ?: if (f.binary) "binario" else f.oldPath?.let { "← $it" }
            why?.let { Mono(it, if (f.unreadable != null) Palette.yellow else Palette.cyan, 11) }
        }
        if (!big) Counts(f.added, f.deleted)
        MarkBox(f.seen, mark)
    }
    Hairline()
}

/** The box at the right of a file: a green tick when seen, a yellow ring when it changed since. */
@Composable
private fun MarkBox(seen: Seen?, click: () -> Unit) {
    Box(Modifier.size(44.dp).clickable(onClick = click), contentAlignment = Alignment.Center) {
        val (fill, line, glyph) = when (seen) {
            Seen.Yes -> Triple(Palette.green, Palette.green, "✓")
            Seen.Changed -> Triple(Color.Transparent, Palette.yellow, "◌")
            null -> Triple(Color.Transparent, Palette.dim, "")
        }
        Box(Modifier.size(24.dp).background(fill, RoundedCornerShape(5.dp)).border(1.5.dp, line, RoundedCornerShape(5.dp)), contentAlignment = Alignment.Center) {
            if (glyph.isNotEmpty()) Mono(glyph, if (seen == Seen.Yes) Palette.bg else Palette.yellow, if (seen == Seen.Changed) 17 else 13, bold = true)
        }
    }
}

@Composable
private fun ColumnScope.FileScreen(file: DiffFile, model: ChangesModel, openFile: (String) -> Unit) {
    Header(file.path.substringAfterLast('/'), (file.oldPath?.let { "$it → " } ?: "") + file.path, onBack = { model.back() }) {
        Row {
            Box(
                Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { openFile(file.path) }.semantics { contentDescription = "Abrir el archivo" },
                contentAlignment = Alignment.Center,
            ) { Mono("▤", Palette.blue, 18, bold = true) }
            MarkBox(file.seen) { model.toggleViewed(file.path) }
        }
    }
    Column(Modifier.weight(1f)) {
        if (file.seen == Seen.Changed) Note("este archivo cambió desde que lo marcaste como visto")
        val reason = when {
            file.unreadable != null -> "${file.unreadable?.let(::whyUnread)}: el contenido no se muestra"
            file.binary -> "archivo binario: no hay líneas que mostrar"
            file.hunks.isEmpty() && file.status == FileStatus.Renamed -> "renombrado, sin cambios en el contenido"
            file.hunks.isEmpty() -> "sin cambios en el contenido"
            else -> null
        }
        if (reason != null) {
            Centre(reason, "", Palette.dim)
            return@Column
        }
        val rows = remember(file.hash) { hunkRows(file) }
        LazyColumn(Modifier.fillMaxSize()) {
            items(rows.size) { i ->
                when (val r = rows[i]) {
                    is HunkRow.Head -> Box(Modifier.fillMaxWidth().background(Palette.sel).padding(horizontal = 12.dp, vertical = 6.dp)) { Mono(r.text, Palette.cyan, 11, lines = 2) }
                    is HunkRow.Line -> Row(
                        Modifier.fillMaxWidth().background(when (r.kind) { LineKind.Add -> AddBg; LineKind.Del -> DelBg; LineKind.Ctx -> Color.Transparent }).padding(horizontal = 12.dp, vertical = 1.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Num(r.old)
                        Num(r.new)
                        val (sign, colour) = when (r.kind) { LineKind.Add -> "+ " to Palette.green; LineKind.Del -> "- " to Palette.red; LineKind.Ctx -> "  " to Palette.fg }
                        Text(sign + r.text, color = if (r.kind == LineKind.Ctx) Palette.fg else colour, fontFamily = MonoFont, fontSize = 11.5.sp, fontWeight = FontWeight.Normal, textAlign = TextAlign.Start, modifier = Modifier.weight(1f))
                    }
                }
            }
        }
    }
}

@Composable
private fun Num(n: Int?) = Text(n?.toString().orEmpty(), color = Palette.dim, fontFamily = MonoFont, fontSize = 10.5.sp, textAlign = TextAlign.End, modifier = Modifier.width(34.dp))
