package dev.jw.next.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.ClientState
import dev.jw.next.client.FolderView
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.WorktreePlan
import dev.jw.next.client.folderView
import dev.jw.next.client.freeName
import dev.jw.next.client.planWorktree
import dev.jw.next.client.repoName
import dev.jw.next.client.repos
import dev.jw.next.client.tildePath
import dev.jw.next.client.takenNames
import dev.jw.next.client.worktreesDir
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.Id
import java.util.concurrent.atomic.AtomicInteger

private val Gutter = 16.dp

/** Goes to [id] as soon as the session shows it: the daemon says a workspace exists a moment after it replies. */
@Composable
private fun OpenWhenShown(source: AppSource, id: Id?, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    val version = source.tick.intValue
    LaunchedEffect(version, id) { if (id != null) nav.openWhenShown(state, id)?.let(set) }
}

/** What a long press on the title of Home, or its `+`, offers (REQ-201). */
@Composable
internal fun BoxScope.NewWorkspaceSheet(dismiss: () -> Unit, folders: () -> Unit) {
    Sheet(dismiss) {
        Mono("workspace nuevo", Palette.fg, 14, bold = true)
        SheetAction("o", "abrir una carpeta del PC", click = folders)
        Mono("Un worktree se hace con una pulsación larga sobre un workspace.", Palette.dim, 12, lines = 2)
    }
}

/**
 * The PC's folders, to open one as a workspace (REQ-201 to REQ-205): a row goes into a folder,
 * the dock below opens the one it stands in, or goes to it when it is already open.
 */
@Composable
fun FoldersScreen(source: AppSource, state: ClientState, nav: Navigator, set: (ClientState) -> Unit) {
    var path by remember { mutableStateOf<String?>(null) }
    var hidden by remember { mutableStateOf(false) }
    var listing by remember { mutableStateOf<DirListing?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var refusal by remember { mutableStateOf<String?>(null) }
    var opening by remember { mutableStateOf(false) }
    val asks = remember { AtomicInteger() }
    var waiting by remember { mutableStateOf<Id?>(null) }
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    val back = { set(nav.show(state, Screen.Home)) }
    BackHandler(onBack = back)
    LaunchedEffect(path, hidden) {
        error = null
        refusal = null
        val asked = asks.incrementAndGet()
        source.listDir(path, hidden) { result ->
            // A slower answer to an earlier tap must not replace the folder asked for last.
            if (asked == asks.get()) result.onSuccess { listing = it }.onFailure { error = it.message ?: "el PC no pudo leer la carpeta" }
        }
    }
    OpenWhenShown(source, waiting, state, nav, set)
    val view = listing?.let { folderView(it, session) }
    Column(Modifier.fillMaxSize()) {
        Header("abrir carpeta", view?.let { tildePath(it.path, it.home) } ?: "leyendo…", onBack = back, action = {
            Box(Modifier.defaultMinSize(minWidth = 72.dp, minHeight = 48.dp).clickable { hidden = !hidden }, contentAlignment = Alignment.Center) {
                Mono(".ocultos", if (hidden) Palette.blue else Palette.dim, 12)
            }
        })
        if (view != null) Crumbs(view) { path = it }
        Box(Modifier.weight(1f)) {
            when {
                error != null -> Column(Modifier.fillMaxSize().padding(Gutter), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically), horizontalAlignment = Alignment.CenterHorizontally) {
                    Mono(error.orEmpty(), Palette.red, 13, lines = 4)
                    view?.parent?.let { up -> SheetAction("‹", "subir", Palette.blue) { path = up } }
                }
                view == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Mono("leyendo…", Palette.dim, 13) }
                else -> FolderList(view) { path = it }
            }
        }
        refusal?.let { Mono(it, Palette.red, 12, lines = 4, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(Gutter)) }
        if (view != null && error == null) Dock(view, busy = opening || waiting != null) {
            val here = view.here
            if (here != null) set(nav.openWorkspace(state, here))
            else {
                opening = true
                refusal = null
                source.openFolder(view.path) { result ->
                    result.onSuccess { waiting = it }.onFailure { refusal = it.message ?: "el PC no pudo abrir la carpeta" }
                    opening = false
                }
            }
        }
    }
}

@Composable
private fun Crumbs(view: FolderView, go: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().background(Palette.panel).horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        view.crumbs.forEachIndexed { i, (label, at) ->
            if (i > 0) Mono("›", Palette.dim, 12)
            Box(Modifier.defaultMinSize(minHeight = 44.dp).clickable { go(at) }.padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                Mono(label, if (at == view.path) Palette.fg else Palette.dim, 12)
            }
        }
    }
    Hairline()
}

@Composable
private fun FolderList(view: FolderView, go: (String) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        view.parent?.let { up ->
            item(key = "..") { FolderRowView("‹", "..", tildePath(up, view.home), null, false) { go(up) } }
        }
        items(view.rows, key = { it.path }) { row ->
            FolderRowView("▸", row.name, if (row.git) "repositorio git" else "carpeta", row.open, row.git) { go(row.path) }
        }
        if (view.rows.isEmpty()) item { Mono("sin subcarpetas", Palette.dim, 13, modifier = Modifier.padding(Gutter)) }
        if (view.cut) item { Mono("se listan las primeras ${view.rows.size} carpetas de ${view.total}", Palette.dim, 11, lines = 2, modifier = Modifier.padding(Gutter)) }
    }
}

@Composable
private fun FolderRowView(glyph: String, name: String, detail: String, open: Id?, git: Boolean, click: () -> Unit) {
    Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(onClick = click).padding(horizontal = Gutter, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Mono(glyph, Palette.blue, 13, modifier = Modifier.padding(end = 12.dp))
        Column(Modifier.weight(1f)) {
            Mono(name, bold = true, size = 14)
            Mono(detail, Palette.dim, 11)
        }
        if (open != null) Tag("● abierta", Palette.green)
        if (git) Tag("git", Palette.cyan)
    }
    Hairline()
}

@Composable
internal fun Tag(text: String, color: Color) {
    Box(Modifier.padding(start = 6.dp).border(1.dp, Palette.line, RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 1.dp)) { Mono(text, color, 10) }
}

@Composable
private fun Dock(view: FolderView, busy: Boolean, click: () -> Unit) {
    Hairline()
    Row(Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Mono(tildePath(view.path, view.home), Palette.dim, 11, modifier = Modifier.weight(1f))
        val here = view.here != null
        Box(
            Modifier.defaultMinSize(minHeight = 48.dp).then(if (here) Modifier.border(1.dp, Palette.green, RoundedCornerShape(6.dp)) else Modifier.background(Palette.blue, RoundedCornerShape(6.dp)))
                .clickable(enabled = !busy, onClick = click).padding(horizontal = 16.dp),
            contentAlignment = Alignment.Center,
        ) { Mono(if (busy) "abriendo…" else if (here) "ya abierta · ir" else "abrir esta carpeta", if (here) Palette.green else Palette.bg, 13, bold = true) }
    }
}

/**
 * A worktree of a repository (REQ-206 to REQ-209), as `w` in the terminal client: the name,
 * what it will make and why it cannot, and what the PC says when git refuses.
 */
@Composable
internal fun BoxScope.WorktreeSheet(source: AppSource, repoAtStart: String, state: ClientState, nav: Navigator, set: (ClientState) -> Unit, dismiss: () -> Unit) {
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    var repo by remember(repoAtStart) { mutableStateOf(repoAtStart) }
    var typed by remember(repoAtStart) { mutableStateOf<String?>(null) }
    var there by remember(repoAtStart) { mutableStateOf<DirListing?>(null) }
    var choosing by remember(repoAtStart) { mutableStateOf(false) }
    var error by remember(repoAtStart) { mutableStateOf<String?>(null) }
    var busy by remember(repoAtStart) { mutableStateOf(false) }
    var waiting by remember(repoAtStart) { mutableStateOf<Id?>(null) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(repoAtStart) { focus.requestFocus() }
    LaunchedEffect(repo) {
        there = null
        source.listDir(worktreesDir(repo), false) { result -> there = result.getOrNull() }
    }
    OpenWhenShown(source, waiting, state, nav, set)
    val taken = takenNames(repo, there, session)
    val name = typed ?: freeName(taken)
    val plan = planWorktree(repo, name, taken)
    val make = {
        if (!busy && plan is WorktreePlan.Ok) {
            busy = true
            error = null
            source.createWorktree(repo, plan.branch, null, plan.folder) { result ->
                result.onSuccess { waiting = it }.onFailure { busy = false; error = it.message ?: "el PC no pudo crear el worktree" }
            }
        }
    }
    Sheet(dismiss) {
        Mono("worktree nuevo", Palette.fg, 14, bold = true)
        val all = repos(session)
        Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).clickable(enabled = !busy && all.size > 1) { choosing = !choosing }.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Mono("proyecto ", Palette.dim, 11)
            Mono(repoName(repo), Palette.fg, 14, bold = true, modifier = Modifier.weight(1f))
            if (all.size > 1) Mono("▾", Palette.dim, 11)
        }
        if (choosing) all.forEach { r ->
            SheetAction(if (r == repo) "●" else " ", repoName(r)) { repo = r; typed = null; choosing = false }
        }
        else {
            Mono("nombre", Palette.dim, 11)
            BasicTextField(
                value = name,
                onValueChange = { typed = it.trim() },
                enabled = !busy,
                singleLine = true,
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 14.sp),
                cursorBrush = SolidColor(Palette.blue),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { make() }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus),
                decorationBox = { inner ->
                    Box(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).border(1.dp, Palette.blue, RoundedCornerShape(6.dp)).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) { inner() }
                },
            )
            when (plan) {
                is WorktreePlan.Wrong -> Mono(plan.why, Palette.red, 12, lines = 3)
                is WorktreePlan.Ok -> Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Row { Mono("rama    ", Palette.dim, 12); Mono(plan.branch, Palette.fg, 12) }
                    Row { Mono("desde   ", Palette.dim, 12); Mono("la rama actual", Palette.fg, 12) }
                    Row { Mono("carpeta ", Palette.dim, 12); Mono("${repoName(repo)}-wt/${plan.folder}", Palette.fg, 12, lines = 2) }
                }
            }
            if (error != null) Mono(error.orEmpty(), Palette.red, 12, lines = 5)
            SheetAction("↵", if (busy) "creando…" else "crear y abrir", Palette.blue, enabled = !busy && plan is WorktreePlan.Ok, click = make)
            Mono("Crea el worktree y le abre un shell, como w en el PC.", Palette.dim, 12, lines = 2)
        }
    }
}
