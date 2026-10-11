package dev.jw.mobile

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
}

private val Corner = RoundedCornerShape(6.dp)

@Composable
private fun markColor(m: Mark): Color {
    val j = LocalJw.current
    return when (m) { Mark.Working -> j.magenta; Mark.Waiting -> j.yellow; Mark.Dev -> j.green }
}

@Composable
private fun Marks(panes: List<PaneInfo>) {
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        for (m in Workspaces.marks(panes)) Text(m.glyph.toString(), color = markColor(m), fontSize = 14.sp, fontWeight = FontWeight.Bold)
    }
}

/** `.yn`: the y and n buttons. Disabled (dim) unless [enabled]; see [AnswerRule]. */
@Composable
private fun YesNo(enabled: Boolean, yLabel: String, nLabel: String, onY: () -> Unit, onN: () -> Unit, modifier: Modifier = Modifier) {
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
fun ConnectedApp(state: UiState.Connected, session: Session, nav: Nav, answers: AnswerPrefs) {
    val j = LocalJw.current
    val model by session.model.collectAsState()
    val now by produceState(System.currentTimeMillis()) { while (true) { delay(500); value = System.currentTimeMillis() } }
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
            open != null -> WorkspaceScreen(open, model, state, session, nav, now)
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
    val projects = model.items.filter { it.kind != Kind.Folder }
    val folders = model.items.filter { it.kind == Kind.Folder }
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 12.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) { SessionChip(model, nav) }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            Section("workspaces · ${model.items.size}")
            var i = 0
            for (w in projects) WorkspaceRow(w, i++, model, state, session, nav, now)
            if (folders.isNotEmpty()) {
                Section("carpetas")
                for (w in folders) WorkspaceRow(w, i++, model, state, session, nav, now)
            }
            if (model.items.isEmpty()) Body("Esperando la lista del PC…", j.dim, 12.0)
            Row(Modifier.padding(14.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("● abierto", color = j.dim, fontSize = 11.5.sp)
                Text("✻ agente", color = j.dim, fontSize = 11.5.sp)
                Text("? espera", color = j.dim, fontSize = 11.5.sp)
                Text("⚡ dev", color = j.dim, fontSize = 11.5.sp)
            }
        }
        StatusBar("NORMAL", j.blue, "toca una fila para entrar", if (state.saved) null else "sin guardar")
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
        StatusBar("?", j.yellow, "${pending.size} esperando")
        NavBar(nav, waiting)
    }
}

@Composable
private fun WorkspaceScreen(w: Workspace, model: Model, state: UiState.Connected, session: Session, nav: Nav, now: Long) {
    val j = LocalJw.current
    val panes = Workspaces.panesOf(model.panes, w.id)
    val root = if (w.kind == Kind.Worktree) model.items.firstOrNull { it.id == w.parent } else null
    val sibs = when (w.kind) {
        Kind.Folder -> listOf(w)
        Kind.Root -> listOf(w) + model.items.filter { it.parent == w.id }
        Kind.Worktree -> listOfNotNull(root) + model.items.filter { it.parent == w.parent }
    }
    val sel = panes.firstOrNull { it.pane == nav.pane } ?: panes.firstOrNull()
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.panel).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }
                .padding(horizontal = 4.dp, vertical = 8.dp).heightIn(min = 52.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(44.dp).heightIn(min = 44.dp).clickable(role = Role.Button) { nav.open = null }, contentAlignment = Alignment.Center) { Text("‹", color = j.fg, fontSize = 22.sp) }
            Column(Modifier.weight(1f)) {
                Text((root?.let { "${it.name} › " } ?: "") + w.name, color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(w.path, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        // rail: the root and its worktrees
        Row(Modifier.fillMaxWidth().background(j.panel).horizontalScroll(rememberScrollState())) {
            for (x in sibs) {
                val on = x.id == w.id
                Row(
                    Modifier.heightIn(min = 42.dp).background(if (on) j.bg else j.panel).clickable { nav.open = x.id; nav.pane = null }
                        .drawBehind { if (on) drawLine(j.blue, Offset(0f, size.height), Offset(size.width, size.height), 2.dp.toPx()) }.padding(horizontal = 14.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp),
                ) {
                    if (x.kind == Kind.Worktree) Text("↳", color = j.dim)
                    Text(x.name, color = if (on) j.fg else j.dim, fontSize = 12.5.sp, maxLines = 1)
                    Marks(Workspaces.panesOf(model.panes, x.id))
                }
            }
        }
        // panes of this workspace
        Row(Modifier.fillMaxWidth().background(j.bg).horizontalScroll(rememberScrollState()).drawBehind { drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }) {
            for (p in panes) {
                val on = p.pane == sel?.pane
                val c = paneColor(p)
                Row(
                    Modifier.heightIn(min = 40.dp).background(if (on) j.sel else j.bg).clickable { nav.pane = p.pane }
                        .drawBehind { if (on) drawLine(c, Offset(0f, size.height), Offset(size.width, size.height), 2.dp.toPx()) }.padding(horizontal = 14.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Box(Modifier.width(7.dp).heightIn(min = 7.dp).background(c, RoundedCornerShape(4.dp)))
                    Text(p.role, color = if (on) j.fg else j.dim, fontSize = 12.5.sp)
                }
            }
        }
        // M5 draws the terminal here.
        Column(Modifier.weight(1f).fillMaxWidth().padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (sel == null) {
                Text("Este workspace no tiene paneles abiertos.", color = j.dim, fontSize = 12.sp)
            } else {
                Text("${sel.role}${sel.fg?.let { " · $it" } ?: ""}", color = j.fg, fontWeight = FontWeight.Bold)
                Text(stateLabel(sel), color = paneColor(sel), fontSize = 12.sp)
                sel.cwd?.let { Text(it, color = j.dim, fontSize = 11.sp) }
                Text("La terminal llega en M5.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(top = 12.dp))
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
        StatusBar("VIEW", j.cyan, sel?.let { stateLabel(it) } ?: "sin paneles")
    }
}

@Composable
private fun paneColor(p: PaneInfo): Color {
    val j = LocalJw.current
    return when {
        p.exited != null -> j.red
        p.agent == AgentState.Waiting -> j.yellow
        p.agent == AgentState.Working -> j.magenta
        p.role.startsWith("dev:") -> j.green
        else -> j.dim
    }
}

private fun stateLabel(p: PaneInfo) = when {
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

@Composable
private fun SettingsScreen(state: UiState.Connected, session: Session, answers: AnswerPrefs, nav: Nav, waiting: Int) {
    val j = LocalJw.current
    var approve by remember { mutableStateOf(answers.approve) }
    var reject by remember { mutableStateOf(answers.reject) }
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header("ajustes", "respuestas y conexión")
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            Section("conexión")
            KvRow("host", state.host)
            KvRow("protocolo", "jw-proto v${state.protocol}", j.green)
            KvRow("acceso", if (state.readOnly) "solo lectura" else "lectura y escritura", if (state.readOnly) j.yellow else j.green)
            if (!state.saved) Body("No se pudo guardar el emparejamiento: al cerrar la app tendrás que escanear de nuevo.", j.yellow, 12.0)
            Section("y · aprobar envía")
            for (c in AnswerChoices.APPROVE) ChoiceRow(c.label, c.id == approve) { approve = c.id; answers.approve = c.id }
            Section("n · rechazar envía")
            for (c in AnswerChoices.REJECT) ChoiceRow(c.label, c.id == reject) { reject = c.id; answers.reject = c.id }
            Body("Enter y Esc son lo habitual; si tu agente pregunta distinto, cámbialo aquí.", j.dim, 11.5)
            Row(
                Modifier.fillMaxWidth().heightIn(min = 54.dp).clickable(role = Role.Button) { session.forget() }.padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) { Text("desconectar y olvidar este PC", color = j.red, fontWeight = FontWeight.Medium) }
        }
        StatusBar("SET", j.blue, "ajustes de la app")
        NavBar(nav, waiting)
    }
}

@Composable
private fun ChoiceRow(label: String, on: Boolean, onClick: () -> Unit) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 54.dp).clickable(role = Role.RadioButton, onClick = onClick).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(if (on) "●" else "○", color = if (on) j.green else j.dim)
        Text(label, color = j.fg)
    }
}
