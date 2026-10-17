package dev.jw.next.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.jw.next.client.ProjectDraft
import dev.jw.next.client.ProjectField
import dev.jw.next.client.ProjectState
import dev.jw.next.client.draft
import dev.jw.next.client.parsePorts
import dev.jw.next.client.portLines
import dev.jw.next.client.repoName
import dev.jw.next.client.savedNote
import dev.jw.next.contract.ConfigFrom
import dev.jw.next.contract.OpenKind

private val Pad = 16.dp

/**
 * The settings of a repository on the phone (prototype `docs/prototypes/project.html`, REQ-520 to REQ-526): where they
 * come from, and the simple ones edited in place. What is changed stays on the screen until it is saved.
 */
@Composable
internal fun ProjectDialog(source: AppSource, repo: String, close: () -> Unit) {
    Dialog(onDismissRequest = close, properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)) {
        Box(Modifier.fillMaxSize().background(Palette.bg)) { ProjectBody(source, repo, close) }
    }
}

@Composable
private fun BoxScope.ProjectBody(source: AppSource, repo: String, close: () -> Unit) {
    var state by remember(repo) { mutableStateOf<ProjectState>(ProjectState.Reading) }
    var note by remember(repo) { mutableStateOf<String?>(null) }
    var saved by remember(repo) { mutableStateOf(false) }
    var busy by remember(repo) { mutableStateOf(false) }
    var editing by remember(repo) { mutableStateOf<ProjectField?>(null) }
    var warned by remember(repo) { mutableStateOf(false) }
    val read = {
        source.projectConfig(repo) { result ->
            state = result.fold({ ProjectState.Ready(it.draft()) }, { ProjectState.Refused(it.message ?: "el PC no pudo leer la configuración") })
        }
    }
    LaunchedEffect(repo) { read() }
    val draft = (state as? ProjectState.Ready)?.draft
    val leave = {
        if (draft?.dirty == true && !warned) {
            warned = true
            saved = false
            note = "hay cambios sin guardar: guarda, o vuelve otra vez para descartarlos"
        } else {
            close()
        }
    }
    BackHandler(onBack = leave)
    val update = { d: ProjectDraft ->
        state = ProjectState.Ready(d)
        warned = false
        note = null
    }
    val save = {
        if (draft != null && !busy && !draft.nothingToSave) {
            busy = true
            note = null
            source.editProjectConfig(repo, draft.to, draft.edit()) { result ->
                busy = false
                saved = result.isSuccess
                note = savedNote(result)
                if (result.isSuccess) read()
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        val sub = when (draft?.info?.from) {
            null -> if (draft == null) null else "sin configuración"
            ConfigFrom.Repo -> "del repositorio"
            ConfigFrom.Personal -> "personal"
        }
        Header("Proyecto · ${repoName(repo)}", sub, onBack = leave)
        when (val s = state) {
            ProjectState.Reading -> Mono("leyendo la configuración…", Palette.dim, 12, modifier = Modifier.padding(Pad))
            is ProjectState.Refused -> Mono(s.why, Palette.red, 12, lines = 8, modifier = Modifier.padding(Pad))
            is ProjectState.Ready -> {
                val d = s.draft
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                    Mono(
                        when (d.info.from) {
                            null -> "ninguna: se abre con los valores de siempre. Guardar crea tu archivo personal; el repositorio no se toca."
                            ConfigFrom.Repo -> "del repositorio (.jw.toml): manda sobre tu archivo personal y lo ve el equipo."
                            ConfigFrom.Personal -> "personal: fuera del repositorio; todos tus worktrees lo comparten."
                        },
                        Palette.fg, 12, lines = 4,
                        modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Pad, vertical = 10.dp),
                    )
                    ProjectField.entries.forEach { f -> ProjectRow(f, d) { editing = f } }
                }
                Column(
                    Modifier.fillMaxWidth().background(Palette.panel).navigationBarsUnlessKeyboard().padding(horizontal = Pad, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.semantics { contentDescription = "Dónde se guarda" }) {
                        listOf(ConfigFrom.Personal to "Personal", ConfigFrom.Repo to "Repositorio").forEach { (to, label) ->
                            Pill(label, d.to == to, Modifier.weight(1f)) { update(d.withTarget(to)) }
                        }
                    }
                    Mono(d.target(repo), Palette.dim, 11, lines = 2)
                    if (d.warning().isNotEmpty()) Mono("⚠ ${d.warning()}", Palette.yellow, 11, lines = 2)
                    if (note != null) Mono(note.orEmpty(), if (saved) Palette.green else Palette.yellow, 12, lines = 4)
                    val enabled = !busy && !d.nothingToSave
                    Box(
                        Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp)
                            .background(if (enabled) Palette.green else Palette.sel, RoundedCornerShape(8.dp))
                            .clickable(enabled = enabled, onClick = save),
                        contentAlignment = Alignment.Center,
                    ) { Mono(if (busy) "guardando…" else "Guardar", if (enabled) Palette.panel else Palette.dim, 13, bold = true) }
                }
            }
        }
    }
    val f = editing
    if (f != null && draft != null) ProjectSheet(f, draft, update) { editing = null }
}

/** One field of the settings: what it is called and its value as it is now. */
@Composable
private fun ProjectRow(field: ProjectField, d: ProjectDraft, click: () -> Unit) {
    val shown = when (field) {
        ProjectField.Root -> listOf(d.cur.root)
        ProjectField.Setup -> d.cur.setup
        ProjectField.Copy -> d.cur.copy
        ProjectField.Ports -> d.cur.ports.map { (n, o) -> "$n +$o" }
        ProjectField.Open -> listOf(if (d.cur.open == OpenKind.Claude) "claude" else "shell")
    }
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp).clickable(onClick = click)
            .semantics { contentDescription = "${field.label}: ${shown.joinToString(", ").ifEmpty { "vacío" }}" }
            .padding(horizontal = Pad, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Mono(field.label, Palette.dim, 11, modifier = Modifier.width(92.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (shown.isEmpty()) Mono("—", Palette.dim, 13)
            shown.forEach { Mono(it, Palette.fg, 13, lines = 3) }
            if (d.isDefault(field)) Mono("por defecto", Palette.dim, 11)
        }
    }
    Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))
}

/** The editor of one field: a line of text, the lines of a list, or the two things a workspace can open with. */
@Composable
private fun BoxScope.ProjectSheet(field: ProjectField, d: ProjectDraft, update: (ProjectDraft) -> Unit, dismiss: () -> Unit) {
    var error by remember(field) { mutableStateOf<String?>(null) }
    var text by remember(field) { mutableStateOf(d.cur.root) }
    val lines = remember(field) {
        mutableStateListOf<String>().also {
            it.addAll(
                when (field) {
                    ProjectField.Setup -> d.cur.setup
                    ProjectField.Copy -> d.cur.copy
                    ProjectField.Ports -> portLines(d.cur.ports)
                    else -> emptyList()
                },
            )
        }
    }
    val done = {
        when (field) {
            ProjectField.Root -> { update(d.withRoot(text)); dismiss() }
            ProjectField.Setup, ProjectField.Copy -> { update(d.withList(field, lines.toList())); dismiss() }
            ProjectField.Ports -> parsePorts(lines.toList()).fold({ update(d.withPorts(it)); dismiss() }, { error = it.message })
            ProjectField.Open -> dismiss()
        }
    }
    Sheet(dismiss) {
        Mono(field.label, Palette.fg, 14, bold = true)
        when (field) {
            ProjectField.Root -> {
                SheetField(text, field.label, { text = it })
                Mono("Un camino relativo empieza en el repositorio. Vacío vuelve a la carpeta de siempre (<repo>-wt).", Palette.dim, 11, lines = 3)
            }
            ProjectField.Open -> {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.semantics { contentDescription = "Al abrir" }) {
                    listOf(OpenKind.Shell to "Shell", OpenKind.Claude to "Claude").forEach { (kind, label) ->
                        Pill(label, d.cur.open == kind, Modifier.weight(1f)) { update(d.withOpen(kind)) }
                    }
                }
                Mono("Un solo panel. Si hay setup, corre primero ahí; si falla, queda la shell con el error. Al crear un worktree se puede elegir otra cosa solo para ese.", Palette.dim, 11, lines = 4)
            }
            else -> {
                lines.forEachIndexed { i, line ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        SheetField(line, "${field.label} ${i + 1}", { lines[i] = it }, Modifier.weight(1f))
                        Box(
                            Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { lines.removeAt(i) }
                                .semantics { contentDescription = "Quitar ${field.label} ${i + 1}" },
                            contentAlignment = Alignment.Center,
                        ) { Mono("✕", Palette.fg, 14) }
                    }
                }
                SheetAction("+", "añadir") { lines.add("") }
                if (field == ProjectField.Ports) Mono("nombre=desplazamiento (0 a 99); el nombre en minúsculas, y no «base»", Palette.dim, 11, lines = 2)
            }
        }
        if (error != null) Mono(error.orEmpty(), Palette.red, 12, lines = 4)
        SheetAction("↵", "Listo", Palette.blue, click = done)
    }
}

/** A line of text of a sheet, on one line, with no capitals and no suggestions. */
@Composable
private fun SheetField(value: String, label: String, onChange: (String) -> Unit, modifier: Modifier = Modifier) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        singleLine = true,
        textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 14.sp),
        cursorBrush = SolidColor(Palette.blue),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Done),
        modifier = modifier.fillMaxWidth().keepsKeyboard().semantics { contentDescription = label },
        decorationBox = { inner ->
            Box(
                Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).border(1.dp, Palette.line, RoundedCornerShape(6.dp)).padding(horizontal = 12.dp),
                contentAlignment = Alignment.CenterStart,
            ) { inner() }
        },
    )
}
