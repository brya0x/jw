package dev.jw.next.app

import android.Manifest
import android.content.ClipData
import android.content.pm.PackageManager
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import dev.jw.next.client.IMAGES_MAX
import dev.jw.next.client.OLDER_PAGE
import dev.jw.next.client.OlderHeld
import dev.jw.next.client.foldRuns
import dev.jw.next.client.RowAct
import dev.jw.next.client.PanelView
import dev.jw.next.client.Submit
import dev.jw.next.client.artifactsPanel
import dev.jw.next.client.filesView
import dev.jw.next.client.followedWork
import dev.jw.next.client.helpPanel
import dev.jw.next.client.itemText
import dev.jw.next.client.label
import dev.jw.next.client.savedPanel
import dev.jw.next.client.skillsPanel
import dev.jw.next.client.slashMenu
import dev.jw.next.client.statusPanel
import dev.jw.next.client.submitOf
import dev.jw.next.client.suggestionShown
import dev.jw.next.client.tag
import dev.jw.next.client.tasksPanel
import dev.jw.next.client.turnText
import dev.jw.next.client.word
import dev.jw.next.client.workLine
import dev.jw.next.client.working
import dev.jw.next.contract.CommandKind
import dev.jw.next.contract.ConvCommand
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ConvStatus
import dev.jw.next.contract.Id
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.Node
import dev.jw.next.contract.TaskState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

private val Gutter = 16.dp

/** How many lines the line grows to before it scrolls. */
private const val LINE_ROWS = 5

/** A word for the status line over the field: yellow when something failed, green when something worked. */
private data class Notice(val text: String, val error: Boolean)

/** What is open over the conversation. */
private sealed interface Open {
    data object Settings : Open
    data object Plus : Open
    data object Tasks : Open
    data object Files : Open

    /** One of jwn's lists; null view while it is read. */
    data class Panel(val view: PanelView?) : Open
}

/**
 * A pane that runs `claude`, drawn from its `conversation` node in place of the pane's blocks (REQ-440 to REQ-452;
 * prototype `docs/prototypes/conversation.html`): the items, the sheet that asks, the line with `+` above it, the lists.
 * [settingsAsked] counts the taps on the pane's tab, each of which opens the settings.
 */
@Composable
internal fun ColumnScope.ConversationPane(source: AppSource, node: Node.Conversation, pane: Node.Pane, workspace: Id, branch: String?, settingsAsked: Int) {
    val nodeId = node.id
    val accent = Palette.accent(node.accent)
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboard.current
    val uri = LocalUriHandler.current

    var text by remember(nodeId) { mutableStateOf(TextFieldValue("")) }
    var retaking by remember(nodeId) { mutableStateOf(false) }
    var picked by remember(nodeId) { mutableStateOf<Int?>(null) }
    var opened by remember(nodeId) { mutableStateOf(setOf<Int>()) }
    // The runs of tools that are open, by the number of their first item (REQ-558).
    var unfolded by remember(nodeId) { mutableStateOf(setOf<Int>()) }
    var open by remember(nodeId) { mutableStateOf<Open?>(null) }
    var notice by remember(nodeId) { mutableStateOf<Notice?>(null) }
    var answering by remember(nodeId) { mutableStateOf(false) }
    var askClosed by remember(nodeId) { mutableStateOf<String?>(null) }
    var plan by remember(nodeId) { mutableStateOf<Pair<String, MarkdownDoc>?>(null) }
    var todosOpen by remember(nodeId) { mutableStateOf(false) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    val pictures = remember(nodeId) { mutableStateListOf<Picture>() }
    var pasted by remember(nodeId) { mutableIntStateOf(0) }
    var filesPath by remember(nodeId) { mutableStateOf("") }

    LaunchedEffect(node.status) { if (node.status != ConvStatus.Dead && node.status != ConvStatus.Resuming) retaking = false }
    LaunchedEffect(settingsAsked) { if (settingsAsked > 0) open = Open.Settings }
    val working = node.working(retaking)
    val followed = node.followedWork(retaking)
    LaunchedEffect(working, followed, open) { while (working || followed || open is Open.Panel) { now = System.currentTimeMillis(); delay(1000) } }
    LaunchedEffect(notice) { if (notice != null) { delay(3500); notice = null } }
    LaunchedEffect(node.ask?.id) { answering = false }

    fun say(message: String, error: Boolean = false) { notice = Notice(message, error) }
    fun copyText(what: String) {
        scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("jwn", what))) }
        val lines = what.lines().size
        say("copiado · $lines ${if (lines == 1) "línea" else "líneas"} · ${what.length} caracteres")
    }
    fun fail(result: Result<Unit>) { result.onFailure { say(it.message ?: "el PC no pudo hacerlo", error = true) } }
    // The items that came before the node's window are held beside it, and the number of an item stays when more are loaded (REQ-569).
    val held = remember(nodeId) { OlderHeld() }
    var olderVersion by remember(nodeId) { mutableIntStateOf(0) }
    held.sync(node)
    val items = remember(node, olderVersion) { held.all(node) }
    val firstItem = held.first(node)

    // ---- what a send does ----
    fun openPanel(name: String, arg: String) {
        when (name) {
            "help" -> open = Open.Panel(helpPanel(node))
            "status" -> open = Open.Panel(statusPanel(node, pane.cwd))
            "skills" -> open = Open.Panel(skillsPanel(node))
            "tasks" -> open = Open.Panel(tasksPanel(node))
            "artifacts" -> open = Open.Panel(artifactsPanel(node.artifacts))
            "resume", "continue" -> {
                if (name == "resume" && arg.isNotEmpty()) {
                    source.resumeConversation(pane.id, arg, ::fail)
                } else {
                    open = Open.Panel(null)
                    source.listConversations(workspace) { r ->
                        // The sheet may have been closed while the PC answered: an answer for a list nobody waits for is dropped.
                        r.onSuccess { list ->
                            if (name == "continue") {
                                open = null
                                list.conversations.firstOrNull()?.let { source.resumeConversation(pane.id, it.id, ::fail) } ?: say("no hay conversaciones guardadas de esta carpeta")
                            } else if (open is Open.Panel) {
                                open = Open.Panel(savedPanel(list.conversations))
                            }
                        }.onFailure { open = null; say(it.message ?: "el PC no pudo listar las conversaciones", error = true) }
                    }
                }
            }
            else -> say("/$name se ve en el terminal del PC")
        }
    }

    fun stopTask(task: String) {
        source.conversationStopTask(nodeId, task, ::fail)
        say("deteniendo la tarea…")
    }

    fun send(line: String) {
        when (val s = submitOf(line, pictures.map { it.n to it.image })) {
            Submit.Nothing -> {}
            is Submit.Say -> {
                if (node.status == ConvStatus.Dead || node.status == ConvStatus.Resuming) retaking = true
                source.conversationSend(nodeId, s.text, s.images, ::fail)
                text = TextFieldValue("")
                pictures.clear()
                picked = null
            }
            is Submit.Shell -> { source.conversationShell(nodeId, s.command, ::fail); text = TextFieldValue("") }
            is Submit.Panel -> { text = TextFieldValue(""); openPanel(s.name, s.arg) }
            is Submit.Setting -> { text = TextFieldValue(""); open = Open.Settings }
        }
    }

    fun insert(piece: String) {
        val at = text.selection.end.coerceIn(0, text.text.length)
        val next = text.text.substring(0, at) + piece + text.text.substring(at)
        text = TextFieldValue(next, TextRange(at + piece.length))
    }

    // [temporary] is the photo the camera wrote: it goes once it has been read, so no photo is left in the cache.
    fun addPicture(from: Uri, temporary: File? = null) {
        if (pictures.size >= IMAGES_MAX) { temporary?.delete(); say("como mucho $IMAGES_MAX imágenes en un mensaje", error = true); return }
        val n = pasted + 1
        scope.launch {
            val made = withContext(Dispatchers.IO) { readPicture(context, from, n).also { temporary?.delete() } }
            made.onSuccess { p -> pasted = n; pictures += p; insert("[Imagen#$n] ") }.onFailure { say(it.message ?: "no se pudo leer la imagen", error = true) }
        }
    }

    val gallery = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { it?.let(::addPicture) }
    var shot by remember { mutableStateOf<File?>(null) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val file = shot
        if (ok && file != null) addPicture(FileProvider.getUriForFile(context, "${context.packageName}.files", file), temporary = file) else file?.delete()
    }
    fun takePhoto() {
        val file = File(File(context.cacheDir, "images").apply { mkdirs() }, "foto-${System.currentTimeMillis()}.jpg")
        shot = file
        camera.launch(FileProvider.getUriForFile(context, "${context.packageName}.files", file))
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> if (granted) takePhoto() else say("sin permiso de la cámara", error = true) }

    // Typing `@` first opens the files, as `+` does (REQ-445).
    LaunchedEffect(text.text) {
        if (text.text == "@") { text = TextFieldValue(""); open = Open.Files; filesPath = "" }
    }

    // ---- the tasks and the checklist (REQ-448) ----
    val running = node.tasks.count { it.state == TaskState.Run }
    if (node.todos.isNotEmpty() || running > 0) {
        Column(Modifier.fillMaxWidth().background(Palette.panel)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = Pad), verticalAlignment = Alignment.CenterVertically) {
                if (node.todos.isNotEmpty()) {
                    val done = node.todos.count { it.done }
                    Mono(
                        "${if (done == node.todos.size) "☑" else "☐"} $done/${node.todos.size} tareas", Palette.fg, 12,
                        modifier = Modifier.weight(1f).defaultMinSize(minHeight = 36.dp).clickable { todosOpen = !todosOpen }.padding(vertical = 8.dp),
                    )
                } else Spacer(Modifier.weight(1f))
                if (running > 0) Box(
                    Modifier.defaultMinSize(minHeight = 34.dp).background(Palette.sel, CircleShape).clickable { open = Open.Tasks }.padding(horizontal = 12.dp),
                    contentAlignment = Alignment.Center,
                ) { Mono("⚙ $running", Palette.yellow, 12, bold = true) }
            }
            if (todosOpen) node.todos.forEach { Mono((if (it.done) "☑ " else "☐ ") + it.text, if (it.done) Palette.dim else Palette.fg, 12, lines = 2, modifier = Modifier.padding(start = 30.dp, end = Pad, bottom = 4.dp)) }
            Hairline()
        }
    }

    // ---- the items ----
    // The runs that are open, from the start of the window; one that is not a run any more (its tools were trimmed) is forgotten.
    val relativeUnfolded = remember(unfolded, firstItem) { unfolded.map { it - firstItem }.toSet() }
    // A run that older items made longer starts earlier: what was open stays open, under its new first (REQ-558, REQ-569).
    LaunchedEffect(items, firstItem) {
        val runs = foldRuns(items)
        val kept = unfolded.mapNotNull { k -> runs.firstOrNull { k in (it.first + firstItem)..(it.last + firstItem) }?.let { it.first + firstItem } }.toSet()
        if (kept != unfolded) unfolded = kept
    }
    // In a list that grows upward the oldest end is where it cannot scroll forward: older items are asked for there, one request at a time.
    val listState = rememberLazyListState()
    // Not before the list is laid out: until then it cannot say which end it is at.
    val atEnd by remember { derivedStateOf { listState.layoutInfo.totalItemsCount > 0 && !listState.canScrollForward } }
    LaunchedEffect(atEnd, olderVersion, node.older, node.first) {
        val ask = held.ask(node, atEnd) ?: return@LaunchedEffect
        olderVersion++
        // The answer comes on the reader thread; the list is changed from the main one.
        source.readOlder(nodeId, ask.before, OLDER_PAGE) { result -> scope.launch { if (held.replied(ask, result)) olderVersion++ } }
    }
    ConversationItems(
        items = items,
        accent = accent,
        picked = picked?.minus(firstItem),
        opened = opened.map { it - firstItem }.toSet(),
        unfolded = relativeUnfolded,
        began = node.running,
        now = now,
        state = listState,
        firstItem = firstItem,
        oldest = held.row(node),
        retryOlder = { held.retry(); olderVersion++ },
        modifier = Modifier.weight(1f).fillMaxWidth().padding(vertical = 8.dp),
        toggleFold = { first ->
            val n = firstItem + first
            unfolded = if (n in unfolded) unfolded - n else unfolded + n
            // What was picked inside a run that closes has no row to be picked at.
            val run = foldRuns(items).firstOrNull { it.first == first }
            if (run != null && n !in unfolded && picked?.minus(firstItem) in run.first..run.last) picked = null
        },
    ) { i ->
        val abs = firstItem + i
        picked = abs
        if (items[i].let { it is dev.jw.next.contract.ConvItem.Tool || it is dev.jw.next.contract.ConvItem.Agent || it is dev.jw.next.contract.ConvItem.Shell }) {
            opened = if (abs in opened) opened - abs else opened + abs
        }
    }

    // What waits for the turn that runs is shown as messages not sent yet, the first few: the chip says how many there are (REQ-549).
    node.queue.take(QueueShown).forEach { Mono("⧖ $it", Palette.dim, 12, lines = 2, modifier = Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 2.dp)) }
    if (node.queue.size > QueueShown) Mono("⧖ y ${node.queue.size - QueueShown} más", Palette.dim, 12, modifier = Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 2.dp))

    // ---- the line over the field, the notice and the copy bar ----
    node.workLine(retaking, now)?.let { line ->
        Mono("${spinner(true)} $line", Palette.dim, 12, lines = 1, modifier = Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 4.dp))
    }
    notice?.let { Mono(it.text, if (it.error) Palette.yellow else Palette.green, 12, lines = 2, modifier = Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 4.dp)) }
    picked?.takeIf { it - firstItem in items.indices }?.let { abs ->
        val i = abs - firstItem
        Row(Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = 10.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf<Pair<String, () -> Unit>>(
                "Copiar" to { copyText(itemText(items[i])) },
                "Copiar el turno" to { copyText(turnText(items, i)) },
                "Soltar" to { picked = null },
            ).forEach { (label, act) ->
                Box(
                    Modifier.weight(1f).defaultMinSize(minHeight = 44.dp).background(Palette.bg, RoundedCornerShape(8.dp)).clickable(onClick = act),
                    contentAlignment = Alignment.Center,
                ) { Mono(label, Palette.fg, 12) }
            }
        }
    }

    // ---- what asks ----
    val ask = node.ask
    if (ask != null && askClosed != ask.id) {
        val viewPlan = {
            source.readPlan(nodeId, ask.id) { r ->
                r.onSuccess { plan = ask.id to it; askClosed = ask.id }.onFailure { say(it.message ?: "el PC no pudo leer el plan", error = true) }
            }
        }
        AskSheet(ask, answering, dismiss = { askClosed = ask.id }, viewPlan = viewPlan) { key ->
            answering = true
            source.conversationAnswer(nodeId, ask.id, key) { r -> answering = false; fail(r) }
        }
    } else if (ask != null) {
        Mono(
            "! ${ask.kind.tag()} · toca para responder", Palette.yellow, 12, bold = true,
            modifier = Modifier.fillMaxWidth().background(Palette.panel).clickable { askClosed = null }.padding(horizontal = Pad, vertical = 10.dp),
        )
    }

    // ---- the plan that was asked for whole, over everything (REQ-480) ----
    plan?.let { (askId, doc) ->
        val waiting = ask?.takeIf { it.id == askId }
        val answers = PlanAnswers(waiting?.options.orEmpty(), waiting != null) { key ->
            plan = null
            answering = true
            source.conversationAnswer(nodeId, askId, key) { r -> answering = false; fail(r) }
        }
        ViewerDialog(doc, answers) { plan = null }
    }

    // ---- the `/` list above the field (REQ-446) ----
    val menu = slashMenu(node.commands, text.text)
    if (menu.isNotEmpty()) CommandList(menu, accent) { c -> send(c.name) }

    // ---- the field ----
    // What Claude proposes next: a tap puts it in the field, the ✕ puts it aside; writing puts it aside too (REQ-572).
    var asideSuggestion by remember(nodeId) { mutableStateOf<String?>(null) }
    val proposed = node.suggestionShown(text.text, asideSuggestion, retaking)
    LaunchedEffect(text.text, node.suggestion) { if (text.text.isNotBlank() && node.suggestion != null) asideSuggestion = node.suggestion }
    Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsUnlessKeyboard().keepsKeyboard().padding(top = 8.dp)) {
        proposed?.let { p ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(
                    Modifier.weight(1f).defaultMinSize(minHeight = 48.dp).background(Palette.sel, RoundedCornerShape(8.dp))
                        .clickable { text = TextFieldValue(p, TextRange(p.length)); asideSuggestion = p }
                        .semantics { role = Role.Button; contentDescription = "Usar la sugerencia: $p" }.padding(horizontal = 10.dp, vertical = 6.dp),
                    contentAlignment = Alignment.CenterStart,
                ) { Mono("↳ $p", Palette.dim, 12, lines = 3) }
                Box(
                    Modifier.defaultMinSize(minWidth = 48.dp, minHeight = 48.dp).clickable { asideSuggestion = p }.semantics { role = Role.Button; contentDescription = "Descartar la sugerencia" },
                    contentAlignment = Alignment.Center,
                ) { Mono("✕", Palette.dim, 14) }
            }
        }
        if (pictures.isNotEmpty()) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            pictures.toList().forEach { p ->
                Row(Modifier.background(Palette.sel, RoundedCornerShape(8.dp)).padding(start = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Mono(p.label, Palette.cyan, 11)
                    Box(Modifier.size(32.dp).clickable { pictures.remove(p); text = TextFieldValue(text.text.replace("[Imagen#${p.n}] ", "").replace("[Imagen#${p.n}]", ""), text.selection) }.semantics { contentDescription = "Quitar la imagen" }, contentAlignment = Alignment.Center) { Mono("✕", Palette.dim, 12) }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.defaultMinSize(minHeight = 26.dp).background(Palette.sel, CircleShape).clickable { open = Open.Settings }.padding(horizontal = 9.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Mono("●", accent, 11)
                    Mono(node.model, Palette.fg, 11)
                    Mono("· ${node.mode.label()}", node.mode.color(), 11)
                }
                branch?.let { Chip("⎇ $it", Palette.magenta) }
                if (node.queue.isNotEmpty()) Chip("${node.queue.size} en cola", Palette.yellow)
                if (text.text.startsWith("!")) Chip("! shell", Palette.yellow)
            }
            Box(Modifier.defaultMinSize(minWidth = 40.dp, minHeight = 36.dp).clickable { open = Open.Plus }.semantics { contentDescription = "Adjuntar" }, contentAlignment = Alignment.Center) { Mono("+", Palette.blue, 20, bold = true) }
        }
        val shell = text.text.startsWith("!")
        Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Mono(if (shell) "!" else "❯", if (shell) Palette.yellow else Palette.green, 14, bold = true)
            Spacer(Modifier.width(10.dp))
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                maxLines = LINE_ROWS,
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 13.sp),
                cursorBrush = SolidColor(Palette.blue),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Default),
                modifier = Modifier.weight(1f).heightIn(min = 48.dp).wrapContentHeight(Alignment.CenterVertically),
                decorationBox = { inner ->
                    if (text.text.isEmpty()) Mono(
                        when {
                            !source.writable -> "solo lectura: datos de ejemplo"
                            node.ask != null -> "responde arriba"
                            working -> "escribe: se envía al terminar"
                            else -> "escribe a claude"
                        },
                        Palette.dim, 13,
                    )
                    inner()
                },
            )
            // While Claude works the button is the red stop; once something is typed it queues the message instead (REQ-443).
            if (working && text.text.isBlank()) {
                Box(Modifier.size(48.dp).clickable { source.conversationInterrupt(nodeId) }.semantics { contentDescription = "Interrumpir" }, contentAlignment = Alignment.Center) { Mono("■", Palette.red, 18, bold = true) }
            } else {
                Box(Modifier.size(48.dp).clickable(enabled = source.writable) { send(text.text) }.semantics { contentDescription = "Enviar" }, contentAlignment = Alignment.Center) {
                    GlyphIcon(Glyph.Send, if (source.writable) Palette.blue else Palette.dim)
                }
            }
        }
    }

    // ---- the sheets ----
    when (val o = open) {
        null -> {}
        Open.Settings -> SettingsSheet(node, dismiss = { open = null }) { name, a, model, effort, mode -> source.conversationSet(nodeId, name, a, model, effort, mode, ::fail) }
        Open.Plus -> PlusSheet(dismiss = { open = null }) { which ->
            open = null
            when (which) {
                "file" -> { open = Open.Files; filesPath = "" }
                "photo" -> if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) takePhoto() else permission.launch(Manifest.permission.CAMERA)
                "gallery" -> gallery.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                "commands" -> text = TextFieldValue("/", TextRange(1))
                "shell" -> text = TextFieldValue("!", TextRange(1))
            }
        }
        Open.Tasks -> PanelSheet(tasksPanel(node), now, dismiss = { open = null }) { act ->
            if (act is RowAct.Stop) { open = null; stopTask(act.task) }
        }
        Open.Files -> FilesSheet(source, workspace, filesPath, setPath = { filesPath = it }, dismiss = { open = null }) { path -> open = null; insert("@$path ") }
        is Open.Panel -> PanelSheet(o.view, now, dismiss = { open = null }) { act ->
            when (act) {
                RowAct.Nothing -> {}
                is RowAct.Insert -> { open = null; text = TextFieldValue(act.text, TextRange(act.text.length)) }
                is RowAct.Resume -> { open = null; source.resumeConversation(pane.id, act.id, ::fail) }
                is RowAct.Copy -> { open = null; copyText(act.text) }
                is RowAct.Open -> uri.openUri(act.url)
                is RowAct.Stop -> { open = null; stopTask(act.task) }
            }
        }
    }
}

private val Pad = 12.dp

/** How many of the messages that wait are drawn under the list. */
private const val QueueShown = 3

/** The mode in the colour the terminal client gives it: dim for manual, magenta for accept edits, cyan for plan, yellow for auto. */
@Composable
private fun ConvMode.color() = when (this) {
    ConvMode.Manual -> Palette.dim
    ConvMode.AcceptEdits -> Palette.magenta
    ConvMode.Plan -> Palette.cyan
    ConvMode.Auto -> Palette.yellow
}

/** The commands whose names hold what is typed, above the keyboard; a tap runs one (REQ-446). A command that needs Claude's screen is marked `↗`. */
@Composable
private fun CommandList(menu: List<ConvCommand>, accent: androidx.compose.ui.graphics.Color, run: (ConvCommand) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.panel)) {
        Hairline()
        LazyColumn(Modifier.heightIn(max = 280.dp)) {
            items(menu, key = { it.name }) { c ->
                Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).clickable { run(c) }.padding(horizontal = Pad, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Column(Modifier.weight(1f)) {
                        Mono((if (c.kind == CommandKind.Screen) "↗ " else "") + c.name, if (c.kind == CommandKind.Screen) Palette.dim else accent, 13, bold = true)
                        c.description?.let { Mono(it, Palette.dim, 12, lines = 2) }
                    }
                    Mono(c.source.word(), Palette.dim, 11)
                }
            }
        }
    }
}

/** The files of the workspace to put in the line as `@path` (REQ-445): folders open, a file is chosen. */
@Composable
private fun FilesSheet(source: AppSource, workspace: Id, path: String, setPath: (String) -> Unit, dismiss: () -> Unit, choose: (String) -> Unit) {
    var view by remember(path) { mutableStateOf<dev.jw.next.client.FilesView?>(null) }
    var error by remember(path) { mutableStateOf<String?>(null) }
    LaunchedEffect(path) { source.listFiles(workspace, path.ifEmpty { null }) { r -> r.onSuccess { view = filesView("·", it, hidden = false) }.onFailure { error = it.message } } }
    ConvSheet(dismiss) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Mono("/$path", Palette.fg, 14, bold = true, modifier = Modifier.weight(1f))
            Mono("Cerrar", Palette.dim, 13, modifier = Modifier.clickable(onClick = dismiss).defaultMinSize(minHeight = 32.dp))
        }
        error?.let { Mono(it, Palette.red, 12, lines = 3) }
        val v = view
        if (v == null && error == null) Mono("cargando…", Palette.dim, 12)
        v?.let { fv ->
            Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                fv.parent?.let { up -> FileRowView("..", "carpeta de arriba", Palette.dim) { setPath(up) } }
                fv.rows.forEach { r ->
                    val dir = r.kind == dev.jw.next.contract.FileKind.Dir
                    FileRowView(if (dir) "${r.name}/" else r.name, r.detail.orEmpty(), if (dir) Palette.blue else Palette.fg) { if (dir) setPath(r.path) else choose(r.path) }
                }
            }
        }
    }
}

@Composable
private fun FileRowView(name: String, detail: String, color: androidx.compose.ui.graphics.Color, click: () -> Unit) {
    Row(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).clickable(onClick = click).padding(horizontal = 4.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Mono(name, color, 13, modifier = Modifier.weight(1f))
        if (detail.isNotBlank()) Mono(detail, Palette.dim, 11)
    }
}
