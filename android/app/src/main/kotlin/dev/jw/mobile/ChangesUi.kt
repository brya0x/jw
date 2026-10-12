package dev.jw.mobile

import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.CommitInfo
import dev.jw.proto.CommitList
import dev.jw.proto.Diff
import dev.jw.proto.DiffFile
import dev.jw.proto.DiffHunk
import dev.jw.proto.DiffLine
import dev.jw.proto.DiffScope
import dev.jw.proto.FileStatus
import dev.jw.proto.LineKind

private val Corner = RoundedCornerShape(6.dp)

/** The three views of the TUI's diff pane (REQ-72). */
private enum class View(val label: String) { General("General"), Uncommitted("Sin commitear"), Commits("Commits") }

/** What one view of one workspace shows right now. */
private sealed interface Shown {
    /** Asked and not answered yet; `before` is the last answer for the same view, still on screen. */
    class Loading(val before: Diff?) : Shown
    class Ready(val diff: Diff) : Shown
    class Failed(val why: String) : Shown
}

private fun shownFor(d: DiffState, ws: String, scope: DiffScope): Shown = when (d) {
    is DiffState.Loaded -> if (d.diff.ws == ws && d.diff.scope == scope) Shown.Ready(d.diff) else Shown.Loading(null)
    is DiffState.Loading -> Shown.Loading(d.before?.takeIf { it.ws == ws && it.scope == scope })
    is DiffState.Failed -> if (d.ws == ws && d.scope == scope) Shown.Failed(d.why) else Shown.Loading(null)
    DiffState.Idle -> Shown.Loading(null)
}

/** The diff tab (REQ-25, REQ-72): the three views of `nav.diffWs`, files, hunks, and "viewed". */
@Composable
fun ChangesScreen(model: Model, state: UiState.Connected, session: Session, nav: Nav, viewed: ViewedMarks, now: Long, waiting: Int) {
    val j = LocalJw.current
    val ws = nav.diffWs?.let { id -> model.items.firstOrNull { it.id == id } }
    var viewIx by rememberSaveable(ws?.id) { mutableIntStateOf(0) }
    val view = View.entries[viewIx]
    var commit by remember(ws?.id) { mutableStateOf<CommitInfo?>(null) }
    var openPath by remember(ws?.id, viewIx, commit?.sha) { mutableStateOf<String?>(null) }
    var picker by remember { mutableStateOf(false) }
    val diffState by session.diff.collectAsState()
    val commitsState by session.commits.collectAsState()
    val counts = remember(ws?.id) { mutableStateMapOf<View, String>() }

    val scope: DiffScope? = when (view) {
        View.General -> DiffScope.General
        View.Uncommitted -> DiffScope.Uncommitted
        View.Commits -> commit?.let { DiffScope.Commit(it.sha) }
    }

    // A view loads when it is first shown (REQ-72); leaving the screen lets go of what it held.
    LaunchedEffect(ws?.id, viewIx, commit?.sha) {
        val id = ws?.id ?: return@LaunchedEffect
        if (view == View.Commits && commit == null) session.requestCommits(id) else scope?.let { session.requestDiff(id, it) }
    }
    DisposableEffect(Unit) { onDispose { session.clearDiff() } }

    val shown = if (ws != null && scope != null) shownFor(diffState, ws.id, scope) else Shown.Loading(null)
    val diff = (shown as? Shown.Ready)?.diff ?: (shown as? Shown.Loading)?.before
    val commitList = (commitsState as? CommitsState.Loaded)?.list?.takeIf { it.ws == ws?.id }

    LaunchedEffect(diff, ws?.id) {
        val d = diff ?: return@LaunchedEffect
        if (d.ws != ws?.id) return@LaunchedEffect
        if (d.tooLarge == null) viewed.prune(d.ws, d.scope, d.files)
        val v = when (d.scope) { DiffScope.General -> View.General; DiffScope.Uncommitted -> View.Uncommitted; is DiffScope.Commit -> null }
        if (v != null) counts[v] = (d.tooLarge ?: d.files.size).toString() + if (d.truncated) "+" else ""
    }
    LaunchedEffect(commitList) { commitList?.let { counts[View.Commits] = it.commits.size.toString() + if (it.more) "+" else "" } }

    val files = diff?.files.orEmpty()
    val openFile = openPath?.let { p -> files.firstOrNull { it.path == p } }
    BackHandler(openFile != null || commit != null) {
        if (openFile != null) openPath = null else commit = null
    }

    Column(Modifier.fillMaxSize().background(j.bg)) {
        if (openFile != null && ws != null && diff != null) {
            FileHeader(openFile, viewed.isViewed(diff.ws, diff.scope, openFile), onBack = { openPath = null }) { viewed.toggle(diff.ws, diff.scope, openFile) }
            FileBody(openFile, Modifier.weight(1f))
            StatusBar("DIFF", j.cyan, "archivo ${files.indexOf(openFile) + 1}/${files.size}", route = state.route)
        } else {
            val subtitle = when {
                ws == null -> null
                view == View.General -> diff?.title ?: ws.name
                view == View.Uncommitted -> "${ws.name} · sin commitear"
                commit == null -> (counts[View.Commits] ?: "…") + " commits"
                else -> commit?.let { c -> c.short + commitList?.let { l -> " · ${l.commits.indexOfFirst { it.sha == c.sha } + 1}/${l.commits.size}" }.orEmpty() }
            }
            ChangesHeader(ws?.name, subtitle) { picker = true }
            ViewTabs(view, counts, if (shown is Shown.Failed) view else null) { viewIx = it.ordinal; commit = null; openPath = null }
            commit?.takeIf { view == View.Commits }?.let { c -> CommitBar(c, commitList, onBack = { commit = null }, onGo = { commit = it }) }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when {
                    ws == null -> Centre("elige un workspace", "entra a uno, o toca el nombre arriba,\npara ver sus cambios.")
                    view == View.Commits && commit == null -> CommitsBody(commitsState, ws.id, commitList, now, session, onPick = { commit = it })
                    shown is Shown.Failed -> Failure(shown.why, "las otras vistas siguen disponibles") { scope?.let { session.requestDiff(ws.id, it) } }
                    diff == null -> Loading(if (view == View.Commits) "los commits" else "los cambios")
                    else -> FilesBody(diff, view, viewed, refreshing = shown is Shown.Loading, onOpen = { openPath = it.path })
                }
            }
            val hint = when {
                ws == null -> "sin workspace"
                shown is Shown.Failed -> "el PC respondió con un error"
                view == View.Commits && commit == null -> if (commitList == null) "cargando" else "toca un commit"
                diff == null -> "cargando"
                diff.tooLarge != null -> "demasiado grande"
                else -> "${viewed.count(diff.ws, diff.scope, diff.files)}/${diff.files.size} vistos"
            }
            if (shown is Shown.Failed) StatusBar("ERR", j.red, hint, route = state.route) else StatusBar("DIFF", j.cyan, hint, route = state.route)
            NavBar(nav, waiting)
        }
    }
    if (picker) PickSheet(model, ws, { x -> nav.diffWs = x.id; picker = false }, onClose = { picker = false })
}

@Composable
private fun ChangesHeader(name: String?, subtitle: String?, onPick: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .clickable(role = Role.Button, onClickLabel = "Elegir workspace", onClick = onPick).padding(horizontal = 14.dp, vertical = 8.dp).heightIn(min = 52.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text("cambios" + (name?.let { " · $it" } ?: ""), color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) Text(subtitle, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Text("▾", color = j.dim)
    }
}

@Composable
private fun ViewTabs(current: View, counts: Map<View, String>, failed: View?, onPick: (View) -> Unit) {
    val j = LocalJw.current
    Row(Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }) {
        for (v in View.entries) {
            val on = v == current
            val c = if (on) j.blue else j.dim
            Row(
                Modifier.weight(1f).heightIn(min = 48.dp).clickable(role = Role.Tab) { onPick(v) }
                    .drawBehind { if (on) drawLine(j.blue, Offset(0f, size.height), Offset(size.width, size.height), 2.dp.toPx()) },
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center,
            ) {
                Text(v.label, color = c, fontSize = 12.sp, maxLines = 1)
                Text(" " + (if (failed == v) "!" else counts[v] ?: ""), color = if (failed == v) j.red else c, fontSize = 11.sp)
            }
        }
    }
}

@Composable
private fun CommitBar(c: CommitInfo, list: CommitList?, onBack: () -> Unit, onGo: (CommitInfo) -> Unit) {
    val j = LocalJw.current
    val at = list?.commits?.indexOfFirst { it.sha == c.sha } ?: -1
    Row(
        Modifier.fillMaxWidth().drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }.padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = "Volver a los commits", onClick = onBack).padding(horizontal = 10.dp), contentAlignment = Alignment.Center) {
            Text("‹ commits", color = j.fg, fontSize = 11.5.sp)
        }
        Text(c.subject, color = j.dim, fontSize = 11.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        for ((glyph, step, label) in listOf(Triple("◂", -1, "Commit anterior"), Triple("▸", 1, "Commit siguiente"))) {
            val target = list?.commits?.getOrNull(at + step)
            Box(
                Modifier.size(48.dp).clickable(enabled = target != null, role = Role.Button, onClickLabel = label) { target?.let(onGo) },
                contentAlignment = Alignment.Center,
            ) { Text(glyph, color = if (target != null) j.fg else j.dim.copy(alpha = 0.4f)) }
        }
    }
}

@Composable
private fun Centre(big: String, small: String, color: Color = LocalJw.current.fg, content: @Composable () -> Unit = {}) {
    val j = LocalJw.current
    Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 44.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(big, color = color, fontSize = 14.sp, textAlign = TextAlign.Center)
        Text(small, color = j.dim, fontSize = 12.sp, textAlign = TextAlign.Center)
        content()
    }
}

@Composable
private fun Loading(what: String) {
    val j = LocalJw.current
    Centre("leyendo $what", "git corre en el PC; puede tardar en repos grandes") {
        CircularProgressIndicator(color = j.blue, strokeWidth = 2.dp, modifier = Modifier.size(22.dp).semantics { contentDescription = "Cargando" })
    }
}

@Composable
private fun Failure(why: String, note: String, onRetry: () -> Unit) {
    val j = LocalJw.current
    Centre("no se pudo leer", why, color = j.red) {
        Box(Modifier.clickable(role = Role.Button, onClick = onRetry).minimumInteractiveComponentSize().heightIn(min = 44.dp).border(BorderStroke(1.dp, j.line), Corner).padding(horizontal = 18.dp), contentAlignment = Alignment.Center) {
            Text("reintentar", color = j.fg, fontSize = 13.sp)
        }
        Text(note, color = j.dim, fontSize = 11.sp)
    }
}

@Composable
private fun Note(text: String) {
    val j = LocalJw.current
    Text(
        text, color = j.yellow, fontSize = 11.5.sp,
        modifier = Modifier.fillMaxWidth().background(j.sel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }.padding(horizontal = 14.dp, vertical = 10.dp),
    )
}

/** The list of a view: a commit's files, General or Uncommitted (REQ-72, REQ-73). */
@Composable
private fun FilesBody(diff: Diff, view: View, viewed: ViewedMarks, refreshing: Boolean, onOpen: (DiffFile) -> Unit) {
    val j = LocalJw.current
    if (diff.noBase) {
        Centre("origin no tiene rama por defecto", "no hay contra qué comparar.\nMira «Sin commitear» o «Commits».")
        return
    }
    if (diff.files.isEmpty() && diff.tooLarge == null) {
        Centre(
            when {
                view == View.Commits -> "este commit no cambia archivos"
                diff.scope == DiffScope.Uncommitted -> "sin cambios sin commitear"
                else -> "nada commiteado contra la base"
            },
            "",
        )
        return
    }
    val tooLarge = diff.tooLarge != null
    LazyColumn(Modifier.fillMaxSize()) {
        if (tooLarge) item(key = "note-large") { Note("este diff pasó de 2 MiB: se listan ${diff.files.size} de ${diff.tooLarge} archivos, sin líneas. Míralo entero en el PC.") }
        if (diff.truncated && !tooLarge) item(key = "note-cut") { Note("hay más de lo que cabe en el teléfono: se muestra lo primero. Míralo entero en el PC.") }
        items(diff.files, key = { it.path }) { f ->
            FileRow(f, tooLarge, viewed.isViewed(diff.ws, diff.scope, f), onOpen = { onOpen(f) }) { viewed.toggle(diff.ws, diff.scope, f) }
        }
        if (refreshing) item(key = "refreshing") { Text("actualizando…", color = j.dim, fontSize = 11.sp, modifier = Modifier.padding(14.dp)) }
    }
}

@Composable
private fun StatusBadge(f: DiffFile) {
    val j = LocalJw.current
    val (glyph, color, label) = when {
        f.untracked -> Triple("?", j.magenta, "sin seguimiento")
        f.status == FileStatus.Added -> Triple("A", j.green, "nuevo")
        f.status == FileStatus.Deleted -> Triple("D", j.red, "borrado")
        f.status == FileStatus.Renamed -> Triple("R", j.cyan, "renombrado")
        else -> Triple("M", j.yellow, "modificado")
    }
    Text(
        glyph, color = j.bg, fontSize = 10.5.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center,
        modifier = Modifier.size(18.dp).background(color, RoundedCornerShape(4.dp)).semantics { contentDescription = label },
    )
}

internal fun unreadableWords(why: String) = when (why) {
    "symlink" -> "enlace simbólico (no se sigue)"
    "not a regular file" -> "no es un archivo normal"
    "too large" -> "demasiado grande"
    "permission denied" -> "sin permiso"
    "it went away" -> "ya no está"
    "it could not be read" -> "no se pudo leer"
    "the name is not UTF-8" -> "nombre que no es UTF-8"
    "not a path inside the workspace" -> "fuera del workspace"
    else -> why
}

@Composable
private fun FileRow(f: DiffFile, listOnly: Boolean, isViewed: Boolean, onOpen: () -> Unit, onMark: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .clickable(enabled = !listOnly, role = Role.Button, onClick = onOpen).padding(start = 14.dp, end = 12.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        StatusBadge(f)
        Column(Modifier.weight(1f)) {
            Text(f.path.substringAfterLast('/'), color = j.fg, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val dir = f.path.substringBeforeLast('/', "")
            Text(
                when {
                    f.unreadable != null -> unreadableWords(f.unreadable.orEmpty())
                    f.oldPath != null -> "← ${f.oldPath}"
                    f.binary -> "binario" + if (dir.isNotEmpty()) " · $dir" else ""
                    else -> dir
                },
                color = if (f.unreadable != null) j.yellow else if (f.oldPath != null || f.binary) j.cyan else j.dim,
                fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        }
        if (!listOnly) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("+${f.added}", color = j.green, fontSize = 12.sp)
                Text("-${f.deleted}", color = j.red, fontSize = 12.sp)
            }
            ViewedBox(isViewed, onMark)
        }
    }
}

@Composable
private fun ViewedBox(on: Boolean, onClick: () -> Unit) {
    val j = LocalJw.current
    Box(Modifier.size(48.dp).clickable(role = Role.Checkbox, onClickLabel = "Marcar visto", onClick = onClick), contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(24.dp).let { if (on) it.background(j.green, RoundedCornerShape(5.dp)) else it.border(BorderStroke(1.5.dp, j.dim), RoundedCornerShape(5.dp)) },
            contentAlignment = Alignment.Center,
        ) { if (on) Text("✓", color = j.bg, fontSize = 13.sp) }
    }
}

@Composable
private fun CommitsBody(state: CommitsState, ws: String, list: CommitList?, now: Long, session: Session, onPick: (CommitInfo) -> Unit) {
    val j = LocalJw.current
    when {
        state is CommitsState.Failed && state.ws == ws -> Failure(state.why, "las otras vistas siguen disponibles") { session.requestCommits(ws) }
        list == null -> Loading("los commits")
        list.noBase -> Column {
            Note("origin no tiene rama por defecto: estos son los últimos commits de HEAD.")
            CommitRows(list, now, onPick)
        }
        list.commits.isEmpty() -> Centre("sin commits en la rama", "")
        else -> CommitRows(list, now, onPick)
    }
}

@Composable
private fun CommitRows(list: CommitList, now: Long, onPick: (CommitInfo) -> Unit) {
    val j = LocalJw.current
    LazyColumn(Modifier.fillMaxWidth()) {
        items(list.commits, key = { it.sha }) { c ->
            Column(
                Modifier.fillMaxWidth().heightIn(min = 52.dp).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                    .clickable(role = Role.Button) { onPick(c) }.padding(horizontal = 14.dp, vertical = 8.dp),
            ) {
                Text(c.subject.ifBlank { "(sin asunto)" }, color = j.fg, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(c.short, color = j.cyan, fontSize = 11.sp)
                    Text("· " + agoWords(now / 1000 - c.whenSecs), color = j.dim, fontSize = 11.sp)
                }
            }
        }
        if (list.more) item(key = "more") { Note("se listan los últimos ${list.commits.size} commits; hay más antiguos.") }
    }
}

/** "hace 5 min", from an age in seconds. */
internal fun agoWords(secs: Long): String {
    val s = secs.coerceAtLeast(0)
    return when {
        s < 60 -> "ahora"
        s < 3600 -> "hace ${s / 60} min"
        s < 86_400 -> "hace ${s / 3600} h"
        s < 86_400 * 30 -> "hace ${s / 86_400} d"
        else -> "hace ${s / (86_400 * 30)} m"
    }
}

@Composable
private fun FileHeader(f: DiffFile, isViewed: Boolean, onBack: () -> Unit, onMark: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .padding(horizontal = 4.dp, vertical = 8.dp).heightIn(min = 52.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp).clickable(role = Role.Button, onClickLabel = "Volver", onClick = onBack), contentAlignment = Alignment.Center) { Icon(Ic.Back, j.fg, 22.dp) }
        Column(Modifier.weight(1f)) {
            Text(f.path.substringAfterLast('/'), color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text((f.oldPath?.let { "$it → " } ?: "") + f.path, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        ViewedBox(isViewed, onMark)
    }
}

/** A hunk's header or one of its lines: what a file's list is made of, flat, so only what is on screen is composed. */
private sealed interface Row2 {
    class Head(val text: String) : Row2
    class Line(val line: DiffLine) : Row2
}

private fun rowsOf(hunks: List<DiffHunk>): List<Row2> = buildList {
    for (h in hunks) {
        add(Row2.Head("@@ -${h.oldStart} +${h.newStart} @@" + if (h.context.isNotBlank()) " ${h.context}" else ""))
        for (l in h.lines) add(Row2.Line(l))
    }
}

@Composable
private fun FileBody(f: DiffFile, modifier: Modifier) {
    val j = LocalJw.current
    when {
        f.unreadable != null -> Box(modifier) { Centre(unreadableWords(f.unreadable.orEmpty()), "el contenido no se muestra: el PC no lo leyó") }
        f.binary -> Box(modifier) { Centre("archivo binario", "no hay líneas que mostrar") }
        f.hunks.isEmpty() -> Box(modifier) {
            if (f.status == FileStatus.Renamed) Centre("renombrado", "${f.oldPath} → ${f.path}\nsin cambios en el contenido")
            else Centre("sin líneas", "")
        }
        else -> {
            val rows = remember(f) { rowsOf(f.hunks) }
            LazyColumn(modifier) {
                if (f.truncated) item(key = "cut") { Note("se muestran las primeras ${f.hunks.sumOf { it.lines.size }} líneas de +${f.added} -${f.deleted}") }
                items(rows.size) { i ->
                    when (val r = rows[i]) {
                        is Row2.Head -> Text(
                            r.text, color = j.cyan, fontSize = 11.5.sp,
                            modifier = Modifier.fillMaxWidth().background(j.sel).padding(horizontal = 12.dp, vertical = 6.dp),
                        )
                        is Row2.Line -> DiffLineRow(r.line)
                    }
                }
            }
        }
    }
}

@Composable
private fun DiffLineRow(l: DiffLine) {
    val j = LocalJw.current
    val bg = when (l.kind) { LineKind.Add -> j.addBg; LineKind.Del -> j.delBg; LineKind.Ctx -> Color.Transparent }
    Row(Modifier.fillMaxWidth().background(bg).padding(horizontal = 6.dp, vertical = 1.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(l.old?.toString() ?: "", color = j.dim, fontSize = 10.5.sp, textAlign = TextAlign.End, modifier = Modifier.widthIn(min = 30.dp))
        Text(l.new?.toString() ?: "", color = j.dim, fontSize = 10.5.sp, textAlign = TextAlign.End, modifier = Modifier.widthIn(min = 30.dp))
        Text(
            when (l.kind) { LineKind.Add -> "+ "; LineKind.Del -> "- "; LineKind.Ctx -> "  " } + l.text,
            color = when (l.kind) { LineKind.Add -> j.green; LineKind.Del -> j.red; LineKind.Ctx -> j.fg },
            fontSize = 11.5.sp, modifier = Modifier.weight(1f),
        )
    }
}
