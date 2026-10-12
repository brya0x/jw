package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.width
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.Kind

private val Corner = RoundedCornerShape(6.dp)

/** What the home screen can have open over it (REQ-88). */
enum class HomeSheet { Plus, Worktree }

/** What the Files screen is choosing a folder for. */
enum class PickFor { OpenFolder, SessionDir }

/** A worktree's or a session's name as the PC takes it: lowercase letters, digits and dashes, starting with a letter or digit. */
internal fun validName(name: String, max: Int = 64) =
    name.isNotEmpty() && name.length <= max && name[0].let { it in 'a'..'z' || it in '0'..'9' } &&
        name.all { it in 'a'..'z' || it in '0'..'9' || it == '-' }

/** `+`: open a folder or make a new worktree. A new session is made from the sessions sheet. */
@Composable
fun PlusSheet(nav: Nav, onWorktree: () -> Unit) {
    SheetFrame(onClose = { nav.sheet = null }) {
        Text("abrir", color = LocalJw.current.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
        SheetRow("o", "abrir carpeta", "del PC") { nav.sheet = null; nav.startPick(PickFor.OpenFolder) }
        SheetRow("w", "nuevo worktree", "de un proyecto", onClick = onWorktree)
    }
}

@Composable
private fun TextField(
    value: String,
    onValue: (String) -> Unit,
    placeholder: String,
    enabled: Boolean,
    label: String,
    lines: Int = 1,
    capitalize: Boolean = false,
) {
    val j = LocalJw.current
    Column(Modifier.padding(horizontal = 12.dp)) {
        Text(label, color = j.dim, fontSize = 11.sp, modifier = Modifier.padding(start = 2.dp, bottom = 2.dp))
        BasicTextField(
            value = value, onValueChange = onValue, enabled = enabled, singleLine = lines == 1, minLines = lines,
            textStyle = TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono), cursorBrush = SolidColor(j.fg),
            keyboardOptions = KeyboardOptions(capitalization = if (capitalize) KeyboardCapitalization.Sentences else KeyboardCapitalization.None, autoCorrectEnabled = false),
            modifier = Modifier.fillMaxWidth(),
            decorationBox = { inner ->
                FieldFrame(j.line, drawn = if (lines > 1) (lines * 24 + 24).dp else 44.dp) {
                    Box(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) {
                        if (value.isEmpty()) Text(placeholder, color = j.dim, fontSize = 13.5.sp)
                        inner()
                    }
                }
            },
        )
    }
}

@Composable
private fun PickRow(label: String, value: String, enabled: Boolean, onClick: () -> Unit) {
    val j = LocalJw.current
    Column(Modifier.padding(horizontal = 12.dp)) {
        Text(label, color = j.dim, fontSize = 11.sp, modifier = Modifier.padding(start = 2.dp, bottom = 2.dp))
        Row(
            Modifier.fillMaxWidth().heightIn(min = 48.dp).background(j.bg, Corner).border(BorderStroke(1.dp, j.line), Corner)
                .clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(value, color = j.fg, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("▾", color = j.dim)
        }
    }
}

@Composable
private fun GoButton(label: String, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val j = LocalJw.current
    Box(
        Modifier.padding(horizontal = 12.dp, vertical = 8.dp).fillMaxWidth().heightIn(min = 48.dp)
            .background(if (enabled && !busy) j.blue else j.line, Corner).clickable(enabled = enabled && !busy, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (busy) CircularProgressIndicator(color = j.fg, strokeWidth = 2.dp, modifier = Modifier.size(16.dp).semantics { contentDescription = "Creando" })
            Text(label, color = if (enabled && !busy) j.bg else j.dim, fontWeight = FontWeight.Bold, fontSize = 13.5.sp)
        }
    }
}

@Composable
private fun SheetNote(text: String, color: androidx.compose.ui.graphics.Color) =
    Text(text, color = color, fontSize = 11.5.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp))

/** New worktree (REQ-87, 88): a project, a name, a branch if it is not the name's, and a task for the agent. */
@Composable
fun WorktreeSheet(model: Model, state: UiState.Connected, session: Session, nav: Nav) {
    val j = LocalJw.current
    val make by session.make.collectAsState()
    val projects = remember(model.items) { model.items.filter { it.kind == Kind.Root }.map { it.project }.distinct() }
    var project by remember { mutableStateOf(nav.worktreeProject?.takeIf { it in projects } ?: projects.firstOrNull().orEmpty()) }
    var picking by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var branch by remember { mutableStateOf("") }
    var task by remember { mutableStateOf("") }
    DisposableEffect(Unit) { session.clearMake(); onDispose { session.clearMake() } }

    val busy = (make as? MakeState.Busy)?.what == Make.Worktree
    val failed = (make as? MakeState.Failed)?.takeIf { it.what == Make.Worktree }
    val nameOk = validName(name)
    SheetFrame(onClose = { if (!busy) nav.sheet = null }) {
        Text("nuevo worktree", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (picking) {
                for (p in projects) {
                    SheetRow(if (p == project) "●" else " ", p, "") { project = p; picking = false }
                }
            } else {
                PickRow("proyecto", project.ifEmpty { "sin proyectos" }, !busy && projects.size > 1) { picking = true }
                TextField(name, { name = it.lowercase().replace(' ', '-') }, "ajustes-rapidos", !busy && !state.readOnly, "nombre")
                if (name.isNotEmpty() && !nameOk) SheetNote("minúsculas, números y guiones", j.red)
                TextField(branch, { branch = it }, "la rama sale del nombre", !busy && !state.readOnly, "rama · si la dejas vacía, sale del nombre")
                TextField(task, { task = it }, "Qué debe hacer el agente al abrir", !busy && !state.readOnly, "tarea para el agente · opcional", lines = 3, capitalize = true)
                SheetNote("sale de origin/<base> y corre el setup del proyecto, como ^␣ w en el PC", j.dim)
                failed?.let { SheetNote(it.why, j.red) }
                if (state.readOnly) SheetNote("solo lectura: este puente no deja crear worktrees", j.yellow)
                GoButton(if (busy) "creando…" else "crear y abrir", busy, nameOk && project.isNotEmpty() && !state.readOnly) {
                    session.requestWorktree(project, name, branch.trim().ifEmpty { null }, task.trim().ifEmpty { null }, model.session ?: "main")
                }
                if (busy) SheetNote("puede tardar: corre el setup. Si sales, sigue en el PC.", j.dim)
            }
        }
    }
}

/** New session (REQ-86, 88): a name and the folder it starts in, inside the sessions sheet. */
@Composable
fun NewSessionForm(state: UiState.Connected, session: Session, nav: Nav) {
    val j = LocalJw.current
    val make by session.make.collectAsState()
    var name by remember { mutableStateOf("") }
    DisposableEffect(Unit) { session.clearMake(); onDispose { session.clearMake() } }
    val busy = (make as? MakeState.Busy)?.what == Make.Session
    val failed = (make as? MakeState.Failed)?.takeIf { it.what == Make.Session }
    val ok = validName(name, 24)
    Row(Modifier.padding(horizontal = 6.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            "‹", color = j.fg, fontSize = 20.sp, textAlign = TextAlign.Center,
            modifier = Modifier.clickable(enabled = !busy, role = Role.Button, onClickLabel = "volver a las sesiones") { nav.sessionForm = false }
                .minimumInteractiveComponentSize().width(44.dp),
        )
        Text("nueva sesión", color = j.dim, fontSize = 12.sp)
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        TextField(name, { name = it.lowercase().replace(' ', '-') }, "trabajo", !busy && !state.readOnly, "nombre")
        if (name.isNotEmpty() && !ok) SheetNote("minúsculas, números y guiones; hasta 24", j.red)
        PickRow("carpeta con la que empieza", nav.sessionDir, !busy && !state.readOnly) { nav.sessionSheet = false; nav.startPick(PickFor.SessionDir) }
        failed?.let { SheetNote(it.why, j.red) }
        if (state.readOnly) SheetNote("solo lectura: este puente no deja crear sesiones", j.yellow)
        GoButton(if (busy) "creando…" else "crear y abrir", busy, ok && !state.readOnly) { session.requestNewSession(name, nav.sessionDir) }
    }
}

/** Whatever was made opens, once the PC's list has it (REQ-88). */
@Composable
fun MakeDone(model: Model, session: Session, nav: Nav) {
    val make by session.make.collectAsState()
    val done = make as? MakeState.Done ?: return
    val there = model.items.any { it.id == done.ws }
    LaunchedEffect(done, there) {
        if (!there) kotlinx.coroutines.delay(8_000)
        if (there) { nav.open = done.ws; nav.pane = null }
        nav.sheet = null; nav.closeSessions(); nav.pick = null; nav.filesPath = null; nav.viewerPath = null
        session.clearMake()
    }
}
