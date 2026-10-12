package dev.jw.mobile

import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import dev.jw.proto.Palettes
import dev.jw.proto.ThemeInfo
import androidx.compose.foundation.layout.Spacer
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.size
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.AgentState
import dev.jw.proto.AnswerChoices
import dev.jw.proto.AnswerRule
import dev.jw.proto.Kind
import dev.jw.proto.Mark
import dev.jw.proto.PaneInfo
import dev.jw.proto.Workspace
import dev.jw.proto.Workspaces
import kotlinx.coroutines.delay

enum class Tab { Home, Avisos, Settings }

/** In-app navigation. Nothing here is a token or a code. */
class Nav {
    var tab by mutableStateOf(Tab.Home)
    var open by mutableStateOf<String?>(null)
    var pane by mutableStateOf<Long?>(null)
    var sessionSheet by mutableStateOf(false)

    /** The workspace that was open before this one, for `^␣ tab`. */
    var previous by mutableStateOf<String?>(null)
}

private val Corner = RoundedCornerShape(6.dp)

@Composable
private fun markColor(m: Mark): Color {
    val j = LocalJw.current
    return when (m) { Mark.Working -> j.magenta; Mark.Waiting -> j.yellow; Mark.Dev -> j.green }
}

@Composable
internal fun Marks(panes: List<PaneInfo>) {
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        for (m in Workspaces.marks(panes)) Text(m.glyph.toString(), color = markColor(m), fontSize = 14.sp, fontWeight = FontWeight.Bold)
    }
}

/** `.yn`: the y and n buttons. Disabled (dim) unless [enabled]; see [AnswerRule]. */
@Composable
internal fun YesNo(enabled: Boolean, yLabel: String, nLabel: String, onY: () -> Unit, onN: () -> Unit, modifier: Modifier = Modifier) {
    val j = LocalJw.current
    @Composable
    fun Btn(label: String, color: Color, onClick: () -> Unit, mod: Modifier) {
        val c = if (enabled) color else j.dim
        Box(
            mod.heightIn(min = 44.dp).background(c.copy(alpha = 0.18f), Corner).border(BorderStroke(1.dp, c.copy(alpha = 0.5f)), Corner)
                .clickable(enabled = enabled, role = Role.Button, onClick = onClick),
            contentAlignment = Alignment.Center,
        ) { Text(label, color = c, fontWeight = FontWeight.Bold, fontSize = 13.sp) }
    }
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Btn(yLabel, j.green, onY, Modifier.weight(1f))
        Btn(nLabel, j.red, onN, Modifier.weight(1f))
    }
}

@Composable
private fun NavBar(nav: Nav, waiting: Int) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind {
            drawLine(j.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx())
        },
    ) {
        for ((tab, glyph, label) in listOf(Triple(Tab.Home, "≡", "workspaces"), Triple(Tab.Avisos, "?", "avisos"), Triple(Tab.Settings, "⚙", "ajustes"))) {
            val sel = nav.tab == tab
            val c = if (sel) j.blue else j.dim
            Box(
                Modifier.weight(1f).heightIn(min = 54.dp).clickable(role = Role.Tab) { nav.tab = tab; nav.open = null }
                    .drawBehind { if (sel) drawLine(j.blue, Offset(size.width * 0.22f, 0f), Offset(size.width * 0.78f, 0f), 2.dp.toPx()) },
                contentAlignment = Alignment.Center,
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(glyph, color = c, fontSize = 17.sp)
                    Text(label, color = c, fontSize = 11.sp)
                }
                if (tab == Tab.Avisos && waiting > 0) {
                    Text(
                        waiting.toString(), color = j.bg, fontSize = 10.sp, fontWeight = FontWeight.Bold,
                        modifier = Modifier.align(Alignment.TopCenter).padding(start = 34.dp, top = 5.dp).background(j.yellow, RoundedCornerShape(8.dp)).padding(horizontal = 5.dp),
                    )
                }
            }
        }
    }
}

/** What the connected app shows: three tabs, or one workspace when a row was tapped. */
@Composable
fun ConnectedApp(state: UiState.Connected, session: Session, nav: Nav, answers: AnswerPrefs, ticking: Boolean = true) {
    val j = LocalJw.current
    val model by session.model.collectAsState()
    // Re-evaluates the 2 s age of the snapshot twice a second. Compose UI tests turn this off: a looping delay
    // never lets them go idle.
    val ticked by produceState(System.currentTimeMillis()) { while (ticking) { delay(500); value = System.currentTimeMillis() } }
    val now = if (ticking) ticked else System.currentTimeMillis()
    var toast by remember { mutableStateOf<String?>(null) }

    // The list is on screen on every tab but Settings; the session polls only then (and in the foreground).
    DisposableEffect(nav.tab) {
        session.setVisible(nav.tab != Tab.Settings)
        onDispose { session.setVisible(false) }
    }
    LaunchedEffect(Unit) { session.toasts.collect { toast = it; delay(1800); if (toast == it) toast = null } }
    BackHandler(nav.sessionSheet || nav.open != null || nav.tab != Tab.Home) {
        when {
            nav.sessionSheet -> nav.sessionSheet = false
            nav.open != null -> nav.open = null
            else -> nav.tab = Tab.Home
        }
    }

    val waiting = model.items.count { Workspaces.waitingPane(Workspaces.panesOf(model.panes, it.id)) != null }
    Box(Modifier.fillMaxSize()) {
        val open = nav.open?.let { id -> model.items.firstOrNull { it.id == id } }
        when {
            open != null -> WorkspaceScreen(open, model, state, session, nav, answers, now)
            nav.tab == Tab.Home -> HomeScreen(model, state, session, nav, now, waiting)
            nav.tab == Tab.Avisos -> AvisosScreen(model, state, session, nav, now, waiting)
            else -> SettingsScreen(state, session, answers, nav, waiting)
        }
        if (nav.sessionSheet) SessionSheet(model, session, nav)
        toast?.let {
            Text(
                it, color = j.bg, fontWeight = FontWeight.Bold, fontSize = 12.sp,
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 96.dp).background(j.fg, Corner).padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
    }
}

@Composable
private fun SessionChip(model: Model, nav: Nav) {
    val j = LocalJw.current
    Row(
        Modifier.heightIn(min = 36.dp).border(BorderStroke(1.dp, j.line), Corner).background(j.bg, Corner)
            .clickable(role = Role.Button) { nav.sessionSheet = true }.padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("sesión", color = j.dim, fontSize = 12.5.sp)
        Text("${model.session ?: "…"} ▾", color = j.fg, fontSize = 12.5.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
private fun WorkspaceRow(w: Workspace, index: Int, model: Model, state: UiState.Connected, session: Session, nav: Nav, now: Long) {
    val j = LocalJw.current
    val panes = Workspaces.panesOf(model.panes, w.id)
    val wait = Workspaces.waitingPane(panes)
    val sub = when (w.kind) {
        Kind.Root -> "raíz · ${w.branch}"
        Kind.Worktree -> "⎇ ${w.branch}" + (w.pr?.let { " · PR #$it" } ?: "")
        Kind.Folder -> w.path
    }
    Column {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 50.dp).clickable { nav.open = w.id; nav.pane = null }
                .padding(start = if (w.kind == Kind.Worktree) 34.dp else 14.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(if (index < 9) "${index + 1}" else "", color = j.dim, fontSize = 11.sp, textAlign = TextAlign.End, modifier = Modifier.width(14.dp))
            Text(
                if (w.kind == Kind.Worktree) "↳" else if (panes.isNotEmpty()) "●" else "○",
                color = if (panes.isNotEmpty()) j.green else j.dim, modifier = Modifier.width(12.dp), textAlign = TextAlign.Center,
            )
            Column(Modifier.weight(1f)) {
                Text(w.name, color = j.fg, fontWeight = if (w.kind == Kind.Root) FontWeight.Bold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(sub, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (w.kind != Kind.Worktree) {
                Text(
                    if (w.kind == Kind.Root) "raíz" else "carpeta", color = j.dim, fontSize = 10.5.sp,
                    modifier = Modifier.border(BorderStroke(1.dp, j.line), RoundedCornerShape(4.dp)).padding(horizontal = 5.dp),
                )
            }
            Marks(panes)
        }
        if (wait != null) {
            val on = AnswerRule.enabled(wait, model.panesAt, now, state.readOnly, wait.pane in model.answered)
            YesNo(on, "y · aprobar", "n · rechazar", { session.answer(wait.pane, true) }, { session.answer(wait.pane, false) }, Modifier.padding(start = 51.dp, end = 14.dp, bottom = 8.dp))
        }
    }
}

@Composable
private fun HomeScreen(model: Model, state: UiState.Connected, session: Session, nav: Nav, now: Long, waiting: Int) {
    val j = LocalJw.current
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    // Search is on what the PC already sent: name, branch and path.
    val shown = if (!searching || query.isBlank()) model.items else model.items.filter {
        listOf(it.name, it.branch, it.path).any { f -> f.contains(query.trim(), ignoreCase = true) }
    }
    val projects = shown.filter { it.kind != Kind.Folder }
    val folders = shown.filter { it.kind == Kind.Folder }
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 12.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (searching) {
                androidx.compose.foundation.text.BasicTextField(
                    value = query, onValueChange = { query = it }, singleLine = true,
                    textStyle = androidx.compose.ui.text.TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(j.fg),
                    modifier = Modifier.weight(1f).heightIn(min = 40.dp).background(j.bg, Corner).border(BorderStroke(1.dp, j.blue), Corner),
                    decorationBox = { inner ->
                        Box(Modifier.padding(horizontal = 10.dp, vertical = 10.dp)) {
                            if (query.isEmpty()) Text("buscar workspace, rama o ruta", color = j.dim, fontSize = 13.sp)
                            inner()
                        }
                    },
                )
            } else {
                SessionChip(model, nav)
                Spacer(Modifier.weight(1f))
            }
            Box(
                Modifier.size(44.dp).semantics { contentDescription = if (searching) "Cerrar búsqueda" else "Buscar" }
                    .clickable(role = Role.Button) { searching = !searching; query = "" },
                contentAlignment = Alignment.Center,
            ) { if (searching) Icon(Ic.X, j.fg, 20.dp) else Icon(Ic.Search, j.fg, 20.dp) }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            Section((if (shown.size == 1) "workspace" else "workspaces") + " · ${shown.size}")
            var i = 0
            for (w in projects) WorkspaceRow(w, i++, model, state, session, nav, now)
            if (folders.isNotEmpty()) {
                Section("carpetas")
                for (w in folders) WorkspaceRow(w, i++, model, state, session, nav, now)
            }
            if (model.items.isEmpty()) Body("Esperando la lista del PC…", j.dim, 12.0)
            else if (shown.isEmpty()) Body("Nada coincide con la búsqueda.", j.dim, 12.0)
            // The legend: each glyph in the colour the row uses it in.
            Row(Modifier.padding(14.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                for ((g, label, c) in listOf(Triple("●", "abierto", j.green), Triple("✻", "agente", j.magenta), Triple("?", "espera", j.yellow), Triple("⚡", "dev", j.green))) {
                    Text(buildAnnotatedString {
                        withStyle(androidx.compose.ui.text.SpanStyle(color = c)) { append(g) }
                        append(" $label")
                    }, color = j.dim, fontSize = 11.5.sp)
                }
            }
        }
        StatusBar("NORMAL", j.blue, "toca una fila para entrar", if (state.saved) null else "sin guardar", state.route)
        NavBar(nav, waiting)
    }
}

@Composable
private fun AvisosScreen(model: Model, state: UiState.Connected, session: Session, nav: Nav, now: Long, waiting: Int) {
    val j = LocalJw.current
    val pending = model.items.mapNotNull { w -> Workspaces.waitingPane(Workspaces.panesOf(model.panes, w.id))?.let { w to it } }
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header("avisos", "agentes que esperan tu respuesta")
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            if (pending.isEmpty()) {
                Text("todo al día\nte avisamos cuando un agente espere por ti", color = j.dim, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth().padding(40.dp))
            }
            for ((w, p) in pending) {
                val on = AnswerRule.enabled(p, model.panesAt, now, state.readOnly, p.pane in model.answered)
                Column(Modifier.fillMaxWidth().drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }.padding(horizontal = 14.dp, vertical = 12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("?", color = j.yellow, fontWeight = FontWeight.Bold)
                        Text((if (w.kind == Kind.Worktree) "${w.project}/" else "") + w.name, color = j.fg, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Text(p.role + (p.fg?.let { " · $it" } ?: ""), color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(vertical = 8.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        YesNo(on, "y", "n", { session.answer(p.pane, true) }, { session.answer(p.pane, false) }, Modifier.weight(2f))
                        Box(
                            Modifier.weight(1f).heightIn(min = 44.dp).border(BorderStroke(1.dp, j.line), Corner).clickable(role = Role.Button) { nav.open = w.id; nav.pane = p.pane },
                            contentAlignment = Alignment.Center,
                        ) { Text("abrir", color = j.fg, fontWeight = FontWeight.Bold, fontSize = 13.sp) }
                    }
                }
            }
        }
        StatusBar("?", j.yellow, "${pending.size} esperando", route = state.route)
        NavBar(nav, waiting)
    }
}

@Composable
internal fun paneColor(p: PaneInfo): Color {
    val j = LocalJw.current
    return when {
        p.exited != null -> j.red
        p.agent == AgentState.Waiting -> j.yellow
        p.agent == AgentState.Working -> j.magenta
        p.role.startsWith("dev:") -> j.green
        else -> j.dim
    }
}

internal fun stateLabel(p: PaneInfo) = when {
    p.exited != null -> "terminó (${p.exited})"
    p.agent == AgentState.Waiting -> "espera tu respuesta"
    p.agent == AgentState.Working -> "trabajando"
    p.agent == AgentState.Idle -> "en reposo"
    else -> "abierto"
}

@Composable
private fun SessionSheet(model: Model, session: Session, nav: Nav) {
    val j = LocalJw.current
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.5f)).clickable { nav.sessionSheet = false })
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Column(Modifier.fillMaxWidth().background(j.panel, RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp)).clickable(enabled = false) {}.padding(bottom = 16.dp)) {
            Box(Modifier.padding(top = 8.dp, bottom = 4.dp).align(Alignment.CenterHorizontally).width(36.dp).heightIn(min = 4.dp).background(j.line, RoundedCornerShape(2.dp)))
            Text("sesiones", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
            for (s in model.sessions) {
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable { session.requestWorkspaces(s.name); nav.sessionSheet = false }.padding(horizontal = 14.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Text(if (s.name == model.session) "●" else " ", color = j.yellow, modifier = Modifier.width(34.dp).border(BorderStroke(1.dp, j.line), RoundedCornerShape(4.dp)).background(j.bg).padding(3.dp), textAlign = TextAlign.Center)
                    Text(s.name, color = j.fg, modifier = Modifier.weight(1f))
                    Text("${s.workspaces} workspaces", color = j.dim, fontSize = 11.sp)
                }
            }
            if (model.sessions.isEmpty()) Body("Sin sesiones todavía.", j.dim, 12.0)
        }
    }
}

/** The quick-answer sets of the prototype (`PRESETS`): what `y · aprobar` and `n · rechazar` send. */
private class Preset(val label: String, val detail: String, val approve: String, val reject: String, val keys: List<String>)

private val PRESETS = listOf(
    Preset("Claude Code · por verificar", "Enter · Esc", "enter", "esc", listOf("⏎", "esc")),
    Preset("Pregunta s/n", "y · n", "y", "n", listOf("y", "n")),
    Preset("Pregunta s/n con Enter", "y y Enter · n y Enter", "y-enter", "n-enter", listOf("y⏎", "n⏎")),
)

@Composable
private fun SettingsScreen(state: UiState.Connected, session: Session, answers: AnswerPrefs, nav: Nav, waiting: Int) {
    val j = LocalJw.current
    var approve by remember { mutableStateOf(answers.approve ?: "enter") }
    var reject by remember { mutableStateOf(answers.reject ?: "esc") }
    val current = PRESETS.firstOrNull { it.approve == approve && it.reject == reject } ?: PRESETS[0]
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header("ajustes", "ajustes de la app")
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            Section("conexión")
            // Both routes, the one in use marked (REQ-64).
            state.lan?.let { SettingRow("wifi", "$it · TLS fijado", right = if (state.route == Route.Wifi) "● en uso" else "sin respuesta", rightColor = if (state.route == Route.Wifi) j.green else j.dim) }
            state.tunnel?.let { SettingRow("túnel", "$it · respaldo", right = if (state.route == Route.Tunnel) "● en uso" else "listo", rightColor = if (state.route == Route.Tunnel) j.green else j.dim) }
            SettingRow("protocolo", "jw-proto v${state.protocol} · compatible")
            SettingRow("acceso", if (state.readOnly) "solo lectura" else "lectura y escritura")
            if (!state.saved) Body("No se pudo guardar el emparejamiento: al cerrar la app tendrás que escanear de nuevo.", j.yellow, 12.0)
            SettingRow("desconectar", null, j.red) { session.forget() }
            Section("respuestas rápidas")
            for (p in PRESETS) {
                val on = p === current
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 60.dp).clickable(role = Role.RadioButton) {
                        approve = p.approve; reject = p.reject
                        answers.approve = p.approve; answers.reject = p.reject
                    }.drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Box(
                        Modifier.size(18.dp).border(2.dp, if (on) j.blue else j.dim, CircleShape).padding(3.dp)
                            .background(if (on) j.blue else Color.Transparent, CircleShape),
                    )
                    Column(Modifier.weight(1f)) {
                        Text(p.label, color = j.fg, fontWeight = FontWeight.Medium)
                        Text(p.detail, color = j.dim, fontSize = 11.sp)
                    }
                    for (k in p.keys) {
                        Text(
                            k, color = j.yellow, fontSize = 12.sp,
                            modifier = Modifier.background(j.bg, RoundedCornerShape(4.dp)).border(1.dp, j.line, RoundedCornerShape(4.dp)).padding(horizontal = 7.dp, vertical = 3.dp),
                        )
                    }
                }
            }
            val sendY = current.keys[0]; val sendN = current.keys[1]
            Text(
                buildAnnotatedString {
                    append("Al tocar "); bold("y · aprobar"); append(" la app envía "); bold(sendY)
                    append("; con "); bold("n · rechazar"); append(", "); bold(sendN); append(". Solo mientras el agente espera.")
                },
                color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(14.dp),
            )
            Section("tema")
            ThemeGrid(state.palettes)
            Section("terminal")
            var fit by remember { mutableStateOf(answers.fitToPhone) }
            SettingRow(
                "ajustar al teléfono", "el panel toma el tamaño de esta pantalla; al salir recupera el suyo",
                right = if (fit) "● activado" else "apagado", rightColor = if (fit) j.green else j.dim,
            ) { fit = !fit; answers.fitToPhone = fit }
            Section("general")
            SettingRow("líder", "^␣ abre el menú de teclas")
        }
        StatusBar("SET", j.blue, "ajustes de la app", route = state.route)
        NavBar(nav, waiting)
    }
}

private fun androidx.compose.ui.text.AnnotatedString.Builder.bold(t: String) {
    pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Medium, color = Color.Unspecified)); append(t); pop()
}

/** `.srow`: a title, an optional dim line under it, a line below the row. */
@Composable
private fun SettingRow(title: String, sub: String?, color: Color = LocalJw.current.fg, right: String? = null, rightColor: Color = LocalJw.current.dim, onClick: (() -> Unit)? = null) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 54.dp)
            .let { if (onClick != null) it.clickable(role = Role.Button, onClick = onClick) else it }
            .drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
            .padding(horizontal = 14.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
            Text(title, color = color, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (sub != null) Text(sub, color = j.dim, fontSize = 11.sp)
        }
        if (right != null) Text(right, color = rightColor, fontSize = 12.sp)
    }
}

/**
 * The PC's themes as the prototype's grid, read-only: the app follows the system's dark or light (the
 * user's choice), and nothing in the bridge writes `settings.json`. The theme in use is marked.
 */
@Composable
private fun ThemeGrid(palettes: Palettes) {
    val j = LocalJw.current
    val dark = androidx.compose.foundation.isSystemInDarkTheme()
    val inUse = if (dark) palettes.dark else palettes.light
    val themes = palettes.themes.ifEmpty {
        listOf(
            ThemeInfo("One Dark", true, Palettes.FALLBACK.dark.roles),
            ThemeInfo("One Light", false, Palettes.FALLBACK.light.roles),
        )
    }
    fun colorOf(t: ThemeInfo, r: String) = Color(android.graphics.Color.parseColor(t.colors[r] ?: "#888888"))
    Column(Modifier.padding(horizontal = 14.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        for (row in themes.chunked(2)) {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for (t in row) {
                    val used = t.colors["bg"] == inUse["bg"] && t.dark == dark
                    Column(
                        Modifier.weight(1f).background(colorOf(t, "bg"), RoundedCornerShape(8.dp))
                            .border(if (used) 2.dp else 1.dp, if (used) j.blue else colorOf(t, "line"), RoundedCornerShape(8.dp)).padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            for (r in listOf("blue", "green", "red", "yellow")) Box(Modifier.size(14.dp).background(colorOf(t, r), RoundedCornerShape(3.dp)))
                        }
                        Text(t.name, color = colorOf(t, "fg"), fontWeight = FontWeight.Bold, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(if (t.dark) "oscuro" else "claro", color = colorOf(t, "dim"), fontSize = 10.5.sp)
                    }
                }
                if (row.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}
