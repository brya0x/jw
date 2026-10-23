package dev.jw.next.app

import android.annotation.SuppressLint
import android.graphics.Color as AndroidColor
import android.webkit.CookieManager
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import dev.jw.next.client.ArtifactBar
import dev.jw.next.client.ArtifactCopies
import dev.jw.next.client.ArtifactStatus
import dev.jw.next.client.ClientState
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.artifactBar
import dev.jw.next.client.artifactList
import dev.jw.next.client.artifactShell
import dev.jw.next.client.artifactSub
import dev.jw.next.client.canAskChanges
import dev.jw.next.client.label
import dev.jw.next.client.latest
import dev.jw.next.client.pendingArtifacts
import dev.jw.next.client.shortHash
import dev.jw.next.client.status
import dev.jw.next.client.verdictRefusal
import dev.jw.next.client.version
import dev.jw.next.contract.ArtifactDoc
import dev.jw.next.contract.ArtifactFormat
import dev.jw.next.contract.ArtifactItem
import dev.jw.next.contract.Delivered
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.Verdict
import java.io.ByteArrayInputStream

private val Gutter = 16.dp

/**
 * The `artifacts` node of [workspace], asked for and kept open while the screen that calls this shows: null until the PC
 * has sent it. A workspace's `⋮` badge, its list and an artifact's screen all read it (REQ-780 to REQ-782).
 */
@Composable
internal fun rememberArtifacts(source: AppSource, workspace: Id): Node.Artifacts? {
    val version = source.tick.intValue
    var node by remember(workspace) { mutableStateOf<Id?>(null) }
    LaunchedEffect(workspace) {
        source.openArtifacts(workspace) { r -> r.onSuccess { id -> node = id; source.open(id) } }
    }
    return remember(version, node) { node?.let { source.node(it) as? Node.Artifacts } }
}

private fun nowSecs() = System.currentTimeMillis() / 1000

internal fun workspaceName(source: AppSource, workspace: Id): String =
    source.session().workspaces.firstOrNull { it.id == workspace }?.name ?: "workspace"

/** The tag that says an artifact's status, in the colour the list uses for it. */
@Composable
private fun StatusTag(status: ArtifactStatus) {
    val color = when (status) {
        ArtifactStatus.Pending -> Palette.yellow
        ArtifactStatus.Approved -> Palette.green
        ArtifactStatus.Changes -> Palette.magenta
    }
    Mono(status.label(), color, 10, modifier = Modifier.border(1.dp, Palette.line, RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 1.dp))
}

@Composable
private fun ArtifactRow(item: ArtifactItem, click: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp).clickable(onClick = click).padding(horizontal = Gutter, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Mono("▣", Palette.cyan, 14, modifier = Modifier.padding(end = 2.dp))
        Column(Modifier.weight(1f)) {
            Mono(item.title, Palette.fg, 13, bold = true)
            Mono(artifactSub(item, nowSecs()), Palette.dim, 11)
        }
        StatusTag(item.status())
    }
    Hairline()
}

/** The list of artifacts of [items]: what waits first, the decided ones folded under «ya decididos» (REQ-781). */
@Composable
private fun ArtifactListView(items: List<ArtifactItem>, open: (ArtifactItem) -> Unit) {
    val list = remember(items) { artifactList(items) }
    var decided by remember { mutableStateOf(false) }
    if (items.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(Gutter), contentAlignment = Alignment.Center) {
            Mono("Nada que revisar. Cuando el agente de este workspace publique un artefacto, aparece aquí.", Palette.dim, 13, lines = 4)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        items(list.pending, key = { it.id }) { ArtifactRow(it) { open(it) } }
        if (list.decided.isNotEmpty()) {
            item {
                Mono(
                    (if (decided) "▾ " else "▸ ") + "ya decididos · ${list.decided.size}", Palette.dim, 11,
                    modifier = Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable { decided = !decided }.padding(horizontal = Gutter, vertical = 14.dp),
                )
            }
            if (decided) items(list.decided, key = { it.id }) { ArtifactRow(it) { open(it) } }
        }
    }
}

/** The artifacts of a workspace, reached from the `⋮` of its header or from the attention that says one waits (REQ-781, REQ-782). */
@Composable
fun ArtifactsScreen(source: AppSource, state: ClientState, screen: Screen.Artifacts, nav: Navigator, set: (ClientState) -> Unit) {
    val node = rememberArtifacts(source, screen.workspace)
    val name = remember(source.tick.intValue, screen.workspace) { workspaceName(source, screen.workspace) }
    val waiting = node?.let { pendingArtifacts(it.items) } ?: 0
    BackHandler { set(nav.leaveArtifacts(state)) }
    Column(Modifier.fillMaxSize()) {
        Header("artefactos", "$name · " + if (waiting > 0) "$waiting por revisar" else "nada por revisar", onBack = { set(nav.leaveArtifacts(state)) })
        if (node == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Mono("leyendo…", Palette.dim, 13) }
        } else {
            ArtifactListView(node.items) { set(nav.artifact(state, screen.workspace, it.id)) }
        }
    }
}

/** One artifact, read from the PC (or from the copy this phone kept of it), with the bar that decides it (REQ-775 to REQ-779). */
@Composable
fun ArtifactScreen(source: AppSource, state: ClientState, screen: Screen.Artifact, nav: Navigator, copies: ArtifactCopyBox, set: (ClientState) -> Unit) {
    val node = rememberArtifacts(source, screen.workspace)
    val item = node?.items?.firstOrNull { it.id == screen.id }
    val name = remember(source.tick.intValue, screen.workspace) { workspaceName(source, screen.workspace) }
    // The version first shown stays shown: a newer one that arrives is offered by the bar, not put under the person's thumb (REQ-779).
    var viewing by remember(screen.id) { mutableStateOf<Int?>(null) }
    LaunchedEffect(item?.id) { if (viewing == null && item != null) viewing = item.latest().n }
    var doc by remember(screen.id, viewing) { mutableStateOf<ArtifactDoc?>(null) }
    var error by remember(screen.id, viewing) { mutableStateOf<String?>(null) }
    var note by remember(screen.id) { mutableStateOf<String?>(null) }
    BackHandler { set(nav.leaveArtifacts(state)) }
    LaunchedEffect(screen.id, viewing) {
        val n = viewing ?: return@LaunchedEffect
        copies.copies.docOf(screen.id, n)?.let { doc = it }
        source.readArtifact(screen.id, n) { r ->
            r.onSuccess { read ->
                doc = read
                item?.let { copies.keep(screen.workspace, name, it, read) }
            }.onFailure { if (doc == null) error = it.message ?: "el PC no pudo leer el artefacto" }
        }
    }
    if (item == null) {
        Column(Modifier.fillMaxSize()) {
            Header("artefacto", name, onBack = { set(nav.leaveArtifacts(state)) })
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Mono("leyendo…", Palette.dim, 13) }
        }
        return
    }
    ArtifactPage(
        item = item, workspace = name, viewing = viewing ?: item.latest().n, setViewing = { viewing = it }, doc = doc, error = error,
        linkUp = source.writable, note = note, back = { set(nav.leaveArtifacts(state)) },
        decide = { verdict, text ->
            val v = item.version(viewing)
            source.decideArtifact(item.id, v.n, v.hash, verdict, text) { r -> r.onFailure { note = verdictRefusal(it.message ?: "el PC no guardó el veredicto") } }
        },
    )
}

/**
 * The page of an artifact, whether it comes from the PC or from a copy: an `html` one in the isolated WebView, a `md` one
 * with the Markdown viewer, and at the foot the bar the app draws outside both (REQ-775 to REQ-779, REQ-783). [decide] is
 * null where nothing can be decided (a copy shown with no link).
 */
@Composable
internal fun ArtifactPage(
    item: ArtifactItem,
    workspace: String,
    viewing: Int,
    setViewing: (Int) -> Unit,
    doc: ArtifactDoc?,
    error: String?,
    linkUp: Boolean,
    note: String?,
    back: () -> Unit,
    decide: ((Verdict, String?) -> Unit)?,
) {
    var asking by remember(item.id) { mutableStateOf(false) }
    var draft by remember(item.id) { mutableStateOf("") }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            val md = doc?.takeIf { it.format == ArtifactFormat.Md }?.doc
            if (md != null) {
                Viewer(md, null, back)
            } else {
                Header(item.title, "$workspace · ${item.kind.label()}", onBack = back)
                Mono("aislado · sin red · sin acceso a jw", Palette.green, 11, modifier = Modifier.fillMaxWidth().background(Palette.sel).padding(horizontal = Gutter, vertical = 8.dp))
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    val html = doc?.html
                    when {
                        html != null -> key(html) { ArtifactWebView(artifactShell(html)) }
                        error != null -> Box(Modifier.fillMaxSize().padding(Gutter), contentAlignment = Alignment.Center) { Mono(error, Palette.red, 13, lines = 4) }
                        else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Mono("leyendo…", Palette.dim, 13) }
                    }
                }
            }
            note?.let { Mono(it, Palette.yellow, 12, lines = 3, modifier = Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp)) }
            ArtifactDock(item, viewing, setViewing, artifactBar(item, viewing, linkUp && decide != null), onApprove = { decide?.invoke(Verdict.Approved, null) }, onAsk = { asking = true })
        }
        if (asking && decide != null) Sheet(dismiss = { asking = false }) {
            Mono("pedir cambios · v$viewing · va al agente", Palette.dim, 12)
            BasicTextField(
                value = draft,
                onValueChange = { draft = it },
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 14.sp),
                cursorBrush = SolidColor(Palette.blue),
                modifier = Modifier.fillMaxWidth().defaultMinSize(minHeight = 96.dp).background(Palette.bg, RoundedCornerShape(6.dp)).border(1.dp, Palette.blue, RoundedCornerShape(6.dp)).padding(12.dp),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ActionPill("cancelar", Palette.dim, true, Modifier.weight(1f)) { asking = false }
                ActionPill("enviar", Palette.yellow, canAskChanges(draft), Modifier.weight(1f)) {
                    decide(Verdict.Changes, draft.trim())
                    asking = false
                    draft = ""
                }
            }
        }
    }
}

@Composable
private fun ActionPill(text: String, color: Color, enabled: Boolean, modifier: Modifier = Modifier, click: () -> Unit) {
    val shape = RoundedCornerShape(6.dp)
    Box(
        modifier.height(48.dp).background(color.copy(alpha = if (enabled) 0.18f else 0.06f), shape).border(1.dp, color.copy(alpha = if (enabled) 0.5f else 0.2f), shape)
            .clickable(enabled = enabled, onClick = click),
        contentAlignment = Alignment.Center,
    ) { Mono(text, if (enabled) color else Palette.dim, 13, bold = true) }
}

/** The bar at the foot of an artifact: the version, and what the person can do with it (REQ-778, REQ-779, REQ-783). */
@Composable
private fun ArtifactDock(item: ArtifactItem, viewing: Int, setViewing: (Int) -> Unit, bar: ArtifactBar, onApprove: () -> Unit, onAsk: () -> Unit) {
    val v = item.version(viewing)
    var confirming by remember(item.id, viewing) { mutableStateOf(false) }
    var versions by remember(item.id) { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().background(Palette.panel).padding(horizontal = Gutter, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box {
                Mono("v${v.n} ▾", Palette.blue, 12, bold = true, modifier = Modifier.defaultMinSize(minHeight = 40.dp).clickable { versions = true }.padding(end = 10.dp).padding(vertical = 10.dp))
                DropdownMenu(expanded = versions, onDismissRequest = { versions = false }, containerColor = Palette.panel) {
                    item.versions.reversed().forEach { x ->
                        DropdownMenuItem(
                            text = { Mono("v${x.n} · ${shortHash(x.hash)} · ${x.status().label()}", Palette.fg, 12) },
                            onClick = { versions = false; setViewing(x.n) },
                        )
                    }
                }
            }
            Mono(shortHash(v.hash), Palette.dim, 11, modifier = Modifier.weight(1f))
            if (bar is ArtifactBar.Ready) Mono("→ claude", Palette.dim, 11)
        }
        when (bar) {
            is ArtifactBar.Newer -> ActionPill("hay una versión nueva (v${bar.n}) · verla", Palette.green, true, Modifier.fillMaxWidth()) { setViewing(bar.n) }
            is ArtifactBar.Decided -> {
                ActionPill(if (bar.verdict == Verdict.Approved) "✓ aprobado" else "↺ cambios pedidos", if (bar.verdict == Verdict.Approved) Palette.green else Palette.magenta, false, Modifier.fillMaxWidth()) {}
                bar.text?.let { Mono("«$it»", Palette.dim, 11, lines = 4) }
                Mono(if (bar.delivered == Delivered.Agent) "enviado al agente" else "guardado: el agente no lo recibió", Palette.dim, 11)
            }
            ArtifactBar.Offline -> {
                Mono("sin enlace con el PC: solo lectura", Palette.yellow, 11)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionPill("pedir cambios", Palette.yellow, false, Modifier.weight(1f)) {}
                    ActionPill("aprobar", Palette.green, false, Modifier.weight(1f)) {}
                }
            }
            ArtifactBar.Ready -> if (confirming) {
                Mono("¿aprobar v${v.n} (${shortHash(v.hash)})? El agente sigue.", Palette.fg, 12, lines = 2)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionPill("cancelar", Palette.dim, true, Modifier.weight(1f)) { confirming = false }
                    ActionPill("sí, aprobar", Palette.green, true, Modifier.weight(1f)) { confirming = false; onApprove() }
                }
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionPill("pedir cambios", Palette.yellow, true, Modifier.weight(1f), onAsk)
                    ActionPill("aprobar", Palette.green, true, Modifier.weight(1f)) { confirming = true }
                }
            }
        }
    }
}

/**
 * An `html` artifact, drawn by a WebView that has no way out and none in: no bridge to the app is ever added, no file
 * or content is read, the network is blocked and every request is answered with nothing, a link goes nowhere, and a
 * page that kills its renderer does not take the app with it (REQ-775). What the artifact says is inside the page
 * [shell] makes (REQ-776), which puts the app's policy before it and the artifact in a sandbox.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun ArtifactWebView(shell: String) {
    AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { context ->
            WebView(context).apply {
                // An artifact is a page with scripts (a prototype is interactive); the page, the policy and everything below keep it in.
                settings.javaScriptEnabled = true
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.blockNetworkLoads = true
                settings.domStorageEnabled = false
                settings.javaScriptCanOpenWindowsAutomatically = false
                settings.setSupportMultipleWindows(false)
                settings.cacheMode = WebSettings.LOAD_NO_CACHE
                settings.mediaPlaybackRequiresUserGesture = true
                settings.setGeolocationEnabled(false)
                CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
                webViewClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse =
                        WebResourceResponse("text/plain", "utf-8", ByteArrayInputStream(ByteArray(0)))
                    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail) = true
                }
                webChromeClient = object : WebChromeClient() {
                    override fun onPermissionRequest(request: PermissionRequest) = request.deny()
                }
                setBackgroundColor(AndroidColor.WHITE)
                loadDataWithBaseURL(null, shell, "text/html", "utf-8", null)
            }
        },
        onRelease = { it.destroy() },
    )
}

/**
 * What the phone kept, shown read only while there is no link to the PC (REQ-783): the workspaces that have a copy, the
 * artifacts of one, and one with its bar that decides nothing.
 */
@Composable
fun SavedArtifactsScreen(copies: ArtifactCopies, back: () -> Unit) {
    var workspace by remember { mutableStateOf<Id?>(null) }
    var open by remember { mutableStateOf<Id?>(null) }
    var viewing by remember(open) { mutableStateOf<Int?>(null) }
    val inWorkspace = workspace
    val item = open?.let { id -> copies.copies.firstOrNull { it.item.id == id } }
    BackHandler {
        when {
            open != null -> open = null
            workspace != null -> workspace = null
            else -> back()
        }
    }
    when {
        item != null -> {
            val n = viewing ?: item.item.latest().n
            ArtifactPage(
                item = item.item, workspace = item.workspaceName, viewing = n, setViewing = { viewing = it },
                doc = copies.docOf(item.item.id, n), error = "esta versión no se abrió con el enlace encendido: no hay copia",
                linkUp = false, note = null, back = { open = null }, decide = null,
            )
        }
        inWorkspace != null -> Column(Modifier.fillMaxSize()) {
            Header("artefactos guardados", copies.workspaces().firstOrNull { it.first == inWorkspace }?.second, onBack = { workspace = null })
            ArtifactListView(copies.items(inWorkspace)) { open = it.id }
        }
        else -> Column(Modifier.fillMaxSize()) {
            Header("artefactos guardados", "sin enlace con el PC · solo lectura", onBack = back)
            LazyColumn(Modifier.fillMaxSize()) {
                items(copies.workspaces(), key = { it.first }) { (id, name) ->
                    Mono(
                        "$name · ${copies.items(id).size}", Palette.fg, 14, bold = true,
                        modifier = Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp).clickable { workspace = id }.padding(horizontal = Gutter, vertical = 16.dp),
                    )
                    Hairline()
                }
            }
        }
    }
}
