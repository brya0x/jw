package dev.jw.mobile

import androidx.compose.material3.minimumInteractiveComponentSize
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.AgentState
import dev.jw.proto.AnswerRule
import dev.jw.proto.Kind
import dev.jw.proto.PaneInfo
import dev.jw.proto.Workspace
import dev.jw.proto.Workspaces
import dev.jw.term.Key
import dev.jw.term.Keys
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private val Corner = RoundedCornerShape(6.dp)

/** What a key of the row sends; the arrows follow the pane's application-cursor mode. */
private class KeyDef(val label: String, val bytes: (appCursor: Boolean) -> ByteArray?, val repeats: Boolean = false)

private val KEYS = listOf(
    KeyDef("esc", { Keys.key(Key.Esc) }),
    KeyDef("tab", { Keys.key(Key.Tab) }),
    KeyDef("⏎", { Keys.key(Key.Enter) }),
    KeyDef("⌫", { Keys.key(Key.Backspace) }, repeats = true),
    KeyDef("↑", { Keys.key(Key.Up, it) }, repeats = true),
    KeyDef("↓", { Keys.key(Key.Down, it) }, repeats = true),
    KeyDef("←", { Keys.key(Key.Left, it) }, repeats = true),
    KeyDef("→", { Keys.key(Key.Right, it) }, repeats = true),
    KeyDef("^C", { Keys.ctrl('c') }),
    KeyDef("^D", { Keys.ctrl('d') }),
    KeyDef("^R", { Keys.ctrl('r') }),
    KeyDef("/", { "/".toByteArray() }),
    KeyDef("|", { "|".toByteArray() }),
    KeyDef("~", { "~".toByteArray() }),
)

/** The sheets over a workspace: the `^␣` menu, its two questions, and the workspace finder. */
private enum class Sheet { None, Leader, Pick, Name, CloseAsk, QuitAsk }

/** A workspace: its panes as tabs, the pane's terminal, a key row and a line to type (REQ-19, 21 to 24). */
@Composable
fun WorkspaceScreen(w: Workspace, model: Model, state: UiState.Connected, session: Session, nav: Nav, answers: AnswerPrefs, now: Long) {
    val j = LocalJw.current
    val panes = Workspaces.panesOf(model.panes, w.id)
    val root = if (w.kind == Kind.Worktree) model.items.firstOrNull { it.id == w.parent } else null
    val sel = panes.firstOrNull { it.pane == nav.pane } ?: panes.firstOrNull()
    val rev by session.termRev.collectAsState()
    val term = sel?.let { session.terminal(it.pane) }
    val exit = sel?.let { session.exitStatus(it.pane) }
    var sheet by remember { mutableStateOf(Sheet.None) }
    var selection by remember(sel?.pane) { mutableStateOf<Selection?>(null) }
    val fit = remember { answers.fitToPhone }
    val ctx = LocalContext.current

    // Watch this workspace's panes while it is on screen; the size a pane had comes back when it is let go (REQ-24).
    val watching = panes.isNotEmpty()
    DisposableEffect(w.id, watching) {
        session.watch(if (watching) w.id else null)
        onDispose { session.restoreSizes(); session.watch(null) }
    }
    DisposableEffect(w.id) { onDispose { nav.previous = w.id } }
    DisposableEffect(sel?.pane) { onDispose { session.restoreSizes() } }
    BackHandlerFor(sheet != Sheet.None) { sheet = Sheet.None }

    Column(Modifier.fillMaxSize().background(j.bg).imePadding()) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 4.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(48.dp).heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = "Volver") { nav.open = null }, contentAlignment = Alignment.Center) { Icon(Ic.Back, j.fg, 22.dp) }
            Column(Modifier.weight(1f)) {
                Text((root?.let { "${it.name} › " } ?: "") + w.name, color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    (if (w.kind == Kind.Worktree) "⎇ ${w.branch}" + (w.pr?.let { " · PR #$it" } ?: "") + " · " else "") + w.path,
                    color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            Box(Modifier.width(48.dp).heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = "Cambiar workspace") { sheet = Sheet.Pick }, contentAlignment = Alignment.Center) { Icon(Ic.Search, j.fg, 22.dp) }
            Box(Modifier.width(48.dp).heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = "Cambios") { nav.showChanges(w.id) }, contentAlignment = Alignment.Center) { Text("±", color = j.fg, fontSize = 18.sp) }
        }
        // The panes of this workspace.
        Row(Modifier.fillMaxWidth().background(j.bg).horizontalScroll(rememberScrollState()).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }) {
            for (p in panes) {
                val on = p.pane == sel?.pane
                val c = paneColor(p)
                Row(
                    Modifier.clickable { nav.pane = p.pane }.minimumInteractiveComponentSize()
                        .heightIn(min = 40.dp).background(if (on) j.sel else j.bg)
                        .drawBehind { if (on) drawLine(c, Offset(0f, size.height), Offset(size.width, size.height), 2.dp.toPx()) }.padding(horizontal = 14.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Box(Modifier.width(7.dp).heightIn(min = 7.dp).background(c, RoundedCornerShape(4.dp)))
                    Text(p.role, color = if (on) j.fg else j.dim, fontSize = 12.5.sp)
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (sel == null) {
                Text("Este workspace no tiene paneles abiertos.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(14.dp))
            } else {
                TerminalView(
                    term = term, rev = rev, fit = fit, selection = selection, onSelection = { selection = it },
                    onSize = { c, r -> session.fit(sel.pane, c, r) }, onTap = {},
                    modifier = Modifier.fillMaxSize().padding(horizontal = 4.dp),
                )
                if (exit != null) {
                    Text(
                        "proceso terminado · estado $exit", color = j.dim, fontSize = 11.sp,
                        modifier = Modifier.align(Alignment.TopEnd).padding(6.dp).background(j.panel, Corner).padding(horizontal = 8.dp, vertical = 3.dp),
                    )
                }
            }
        }
        if (sel != null && sel.agent == AgentState.Waiting) {
            val on = AnswerRule.enabled(sel, model.panesAt, now, state.readOnly, sel.pane in model.answered)
            Column(
                Modifier.fillMaxWidth().background(j.yellow.copy(alpha = 0.12f).compositeOver(j.bg)).padding(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text("? ${sel.role} espera tu respuesta", color = j.fg, fontSize = 12.5.sp)
                YesNo(on, "y · sí", "n · no", { session.answer(sel.pane, true) }, { session.answer(sel.pane, false) })
            }
        }
        val picked = selection
        if (picked != null && term != null) {
            Row(
                Modifier.fillMaxWidth().background(j.sel).padding(horizontal = 10.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val text = remember(picked) { picked.text(term) }
                val lines = text.count { it == '\n' } + 1
                Text(if (lines == 1) "1 línea seleccionada" else "$lines líneas seleccionadas", color = j.dim, fontSize = 12.sp, modifier = Modifier.weight(1f))
                KeyButton("copiar") {
                    val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.setPrimaryClip(ClipData.newPlainText("jw", text))
                    selection = null
                }
                KeyButton("cancelar") { selection = null }
            }
        }
        KeyRow(sel, term, session, onLeader = { sheet = Sheet.Leader })
        Composer(sel, state, session)
        StatusBar("TERM", j.green, sel?.let { "${it.role} · ${term?.let { t -> "${t.cols}×${t.rows}" } ?: "…"} · ^␣ menú" } ?: "sin paneles", route = state.route)
    }

    when (sheet) {
        Sheet.None -> {}
        Sheet.Leader -> LeaderSheet(
            sel = sel, w = w, nav = nav, model = model, onClose = { sheet = Sheet.None },
            onPick = { sheet = Sheet.Pick }, onSessions = { sheet = Sheet.None; nav.sessionSheet = true },
            onNew = { sel?.let { p -> session.newPane(p.pane, p.cwd ?: w.path) }; sheet = Sheet.None },
            onClosePane = { sheet = Sheet.CloseAsk }, onName = { sheet = Sheet.Name },
            onSettings = { sheet = Sheet.None; nav.open = null; nav.tab = Tab.Settings },
            onQuit = { sheet = Sheet.QuitAsk }, onPrevious = { nav.previous?.let { nav.open = it; nav.pane = null }; sheet = Sheet.None },
            onChanges = { sheet = Sheet.None; nav.showChanges(w.id) },
        )
        Sheet.Pick -> PickSheet(model, w, { x -> nav.open = x.id; nav.pane = null }, onClose = { sheet = Sheet.None })
        Sheet.Name -> NameSheet(sel, onDone = { name -> sel?.let { session.namePane(it.pane, name) }; sheet = Sheet.None }, onClose = { sheet = Sheet.None })
        Sheet.CloseAsk -> AskSheet(
            "¿cerrar el panel ${sel?.role ?: ""}?", sel?.fg?.let { "corre $it: se termina" } ?: "se termina su proceso", "cerrar",
            onYes = { sel?.let { session.closePane(it.pane) }; sheet = Sheet.None }, onNo = { sheet = Sheet.None },
        )
        Sheet.QuitAsk -> AskSheet(
            "¿desconectar este teléfono?", "tendrás que escanear el código otra vez", "desconectar",
            onYes = { sheet = Sheet.None; session.forget() }, onNo = { sheet = Sheet.None },
        )
    }
}

@Composable
private fun BackHandlerFor(enabled: Boolean, onBack: () -> Unit) = androidx.activity.compose.BackHandler(enabled, onBack)

@Composable
private fun KeyButton(label: String, accent: Boolean = false, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val j = LocalJw.current
    Box(
        modifier.clickable(role = Role.Button, onClick = onClick).minimumInteractiveComponentSize().widthIn(min = 44.dp).heightIn(min = 38.dp)
            .background(j.bg, RoundedCornerShape(5.dp)).border(BorderStroke(1.dp, if (accent) j.yellow.copy(alpha = 0.5f) else j.line), RoundedCornerShape(5.dp))
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label, color = if (accent) j.yellow else j.fg, fontSize = 12.sp) }
}

/** `.keys`: `^␣` first, then the keys a phone keyboard lacks; the arrows and ⌫ repeat while held. */
@Composable
private fun KeyRow(sel: PaneInfo?, term: dev.jw.term.Terminal?, session: Session, onLeader: () -> Unit) {
    val j = LocalJw.current
    val scope = rememberCoroutineScope()
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) }
            .horizontalScroll(rememberScrollState()).padding(start = 10.dp, end = 10.dp, top = 3.dp),
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        KeyButton("^␣", accent = true, onClick = onLeader)
        for (k in KEYS) {
            val send = {
                val b = k.bytes(term?.appCursor ?: false)
                if (sel != null && b != null) session.input(sel.pane, b)
            }
            Box(
                Modifier
                    .pointerInput(sel?.pane, k) {
                        detectTapGestures(onPress = {
                            send()
                            if (k.repeats) {
                                val job = scope.launch { delay(350); while (true) { send(); delay(70) } }
                                tryAwaitRelease()
                                job.cancel()
                            }
                        })
                    }
                    // The press is a raw gesture (it repeats while held), so say to accessibility that this is a button.
                    .semantics(mergeDescendants = true) { role = Role.Button; onClick(k.label) { send(); true } }
                    .minimumInteractiveComponentSize().widthIn(min = 44.dp).heightIn(min = 38.dp)
                    .background(j.bg, RoundedCornerShape(5.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(5.dp))
                    .padding(horizontal = 10.dp),
                contentAlignment = Alignment.Center,
            ) { Text(k.label, color = j.fg, fontSize = 12.sp) }
        }
    }
}

/** `.comp`: a line typed here goes to the pane, then Enter. */
@Composable
private fun Composer(sel: PaneInfo?, state: UiState.Connected, session: Session) {
    val j = LocalJw.current
    var text by remember(sel?.pane) { mutableStateOf("") }
    val send = {
        if (sel != null) {
            if (text.isNotEmpty()) session.input(sel.pane, Keys.text(text))
            session.input(sel.pane, Keys.key(Key.Enter))
            text = ""
        }
    }
    Row(
        Modifier.fillMaxWidth().background(j.panel).padding(start = 10.dp, end = 10.dp, top = 4.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(if (sel?.role == "agent") "›" else "❯", color = j.green, fontWeight = FontWeight.Bold)
        BasicTextField(
            value = text, onValueChange = { text = it }, enabled = sel != null && !state.readOnly,
            textStyle = TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono), cursorBrush = SolidColor(j.fg),
            singleLine = true, keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), keyboardActions = KeyboardActions(onSend = { send() }),
            modifier = Modifier.weight(1f),
            decorationBox = { inner ->
                FieldFrame(j.line) {
                    Box(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) {
                        if (text.isEmpty()) Text(if (state.readOnly) "solo lectura" else if (sel?.role == "agent") "escribe a ${sel.role}" else "escribe un comando", color = j.dim, fontSize = 13.5.sp)
                        inner()
                    }
                }
            },
        )
        Box(
            Modifier.clickable(enabled = !state.readOnly, role = Role.Button, onClickLabel = "Enviar", onClick = send).minimumInteractiveComponentSize()
                .width(44.dp).heightIn(min = 44.dp).background(if (state.readOnly) j.dim else j.blue, Corner),
            contentAlignment = Alignment.Center,
        ) { Text("↵", color = j.bg, fontWeight = FontWeight.Bold, fontSize = 16.sp) }
    }
}

@Composable
internal fun SheetFrame(onClose: () -> Unit, content: @Composable () -> Unit) {
    val j = LocalJw.current
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.5f)).clickable(onClick = onClose))
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Column(Modifier.fillMaxWidth().heightIn(max = 620.dp).background(j.panel, RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp)).clickable(enabled = false) {}.padding(bottom = 16.dp)) {
            Box(Modifier.padding(top = 8.dp, bottom = 4.dp).align(Alignment.CenterHorizontally).width(36.dp).heightIn(min = 4.dp).background(j.line, RoundedCornerShape(2.dp)))
            content()
        }
    }
}

@Composable
private fun SheetRow(key: String, label: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(
            key, color = if (enabled) j.yellow else j.dim, fontSize = 12.sp, textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(min = 34.dp).background(j.bg, RoundedCornerShape(4.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 3.dp),
        )
        Text(label, color = if (enabled) j.fg else j.dim, modifier = Modifier.weight(1f))
        Text(tag, color = j.dim, fontSize = 11.sp)
    }
}

/** `^␣`: the actions of the TUI that make sense here, each sending what the TUI sends; the ones that only the PC can do say so. */
@Composable
private fun LeaderSheet(
    sel: PaneInfo?, w: Workspace, nav: Nav, model: Model, onClose: () -> Unit,
    onPick: () -> Unit, onSessions: () -> Unit, onNew: () -> Unit, onClosePane: () -> Unit, onName: () -> Unit,
    onSettings: () -> Unit, onQuit: () -> Unit, onPrevious: () -> Unit, onChanges: () -> Unit,
) {
    val j = LocalJw.current
    SheetFrame(onClose) {
        Text("^␣ · teclas", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
        Column(Modifier.verticalScroll(rememberScrollState())) {
            SheetRow("␣", "buscar workspace", "ir", onClick = onPick)
            SheetRow("tab", "workspace anterior", "ir", enabled = nav.previous != null, onClick = onPrevious)
            SheetRow("a", "sesiones", "ir", onClick = onSessions)
            SheetRow("t", "panel nuevo", "paneles", enabled = sel != null, onClick = onNew)
            SheetRow("x", "cerrar panel", "paneles", enabled = sel != null, onClick = onClosePane)
            SheetRow("n", "nombrar panel", "paneles", enabled = sel != null, onClick = onName)
            SheetRow("d", "cambios (diff)", "diff", onClick = onChanges)
            SheetRow(",", "ajustes", "ir", onClick = onSettings)
            SheetRow("q", "desconectar", "", onClick = onQuit)
            for ((k, l) in listOf("o" to "abrir carpeta", "w" to "nuevo worktree", "r" to "renombrar", "s" to "sincronizar", "X" to "quitar worktree", "R" to "revisar un PR")) {
                SheetRow(k, l, "en el PC", enabled = false, onClick = {})
            }
        }
    }
}

@Composable
internal fun PickSheet(model: Model, current: Workspace?, onPick: (Workspace) -> Unit, onClose: () -> Unit) {
    val j = LocalJw.current
    var q by remember { mutableStateOf("") }
    val found = model.items.filter { it.id != current?.id && (q.isBlank() || (it.name + " " + it.path).contains(q, ignoreCase = true)) }
    SheetFrame(onClose) {
        Text("workspaces", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
        BasicTextField(
            value = q, onValueChange = { q = it }, singleLine = true,
            textStyle = TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono), cursorBrush = SolidColor(j.fg),
            modifier = Modifier.padding(horizontal = 12.dp).fillMaxWidth(),
            decorationBox = { inner ->
                FieldFrame(j.blue) {
                    Box(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) {
                        if (q.isEmpty()) Text("buscar…", color = j.dim, fontSize = 13.5.sp)
                        inner()
                    }
                }
            },
        )
        Column(Modifier.verticalScroll(rememberScrollState()).padding(top = 6.dp)) {
            for (x in found) {
                val panes = Workspaces.panesOf(model.panes, x.id)
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 50.dp).clickable { onPick(x) }.padding(horizontal = 14.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(if (panes.isNotEmpty()) "●" else "○", color = if (panes.isNotEmpty()) j.green else j.dim)
                    Column(Modifier.weight(1f)) {
                        Text((if (x.kind == Kind.Worktree) "${x.project}/" else "") + x.name, color = j.fg, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(x.path, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Text(when (x.kind) { Kind.Root -> "raíz"; Kind.Worktree -> "worktree"; Kind.Folder -> "carpeta" }, color = j.dim, fontSize = 10.5.sp)
                    Marks(panes)
                }
            }
            if (found.isEmpty()) Text("sin resultados", color = j.dim, modifier = Modifier.padding(20.dp))
        }
    }
}

@Composable
private fun NameSheet(sel: PaneInfo?, onDone: (String) -> Unit, onClose: () -> Unit) {
    val j = LocalJw.current
    var name by remember { mutableStateOf("") }
    SheetFrame(onClose) {
        Text("nombre del panel ${sel?.role ?: ""} · vacío vuelve al automático", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
        BasicTextField(
            value = name, onValueChange = { name = it }, singleLine = true,
            textStyle = TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono), cursorBrush = SolidColor(j.fg),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { onDone(name) }),
            modifier = Modifier.padding(horizontal = 12.dp).fillMaxWidth(),
            decorationBox = { inner -> FieldFrame(j.blue) { Box(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) { inner() } } },
        )
        SheetRow("↵", "guardar", "", onClick = { onDone(name) })
    }
}

@Composable
private fun AskSheet(title: String, detail: String, yes: String, onYes: () -> Unit, onNo: () -> Unit) {
    val j = LocalJw.current
    SheetFrame(onNo) {
        Text(title, color = j.fg, fontWeight = FontWeight.Bold, modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp))
        Text(detail, color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp).padding(bottom = 10.dp))
        Row(Modifier.padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.weight(1f)) { JwButton("cancelar", onClick = onNo) }
            Box(Modifier.weight(1f)) { JwButton(yes, fill = true, onClick = onYes) }
        }
    }
}
