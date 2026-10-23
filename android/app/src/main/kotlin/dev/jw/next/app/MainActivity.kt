package dev.jw.next.app

import android.content.res.Configuration
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.core.view.WindowCompat
import dev.jw.next.client.ClientState
import dev.jw.next.client.LinkFail
import dev.jw.next.client.Look
import dev.jw.next.client.Navigator
import dev.jw.next.client.Pairing
import dev.jw.next.client.Screen
import dev.jw.next.client.parsePairLink
import kotlinx.coroutines.flow.first

/** What the app is showing at the top level: no PC yet, adding one, the list of PCs, or a source of nodes. */
private sealed interface Mode {
    data object Welcome : Mode
    data class Scan(val back: Mode) : Mode
    data class Paste(val back: Mode, val error: Boolean = false) : Mode

    /** The PCs. [current] stays connected until [next], the one being connected to, is ready. */
    data class Pcs(val current: WsLinkSource?, val next: WsLinkSource? = null, val from: Screen = Screen.Home) : Mode

    /** A source of nodes, with the screen it opens on: where the person was before going to the list of PCs. */
    data class Running(val source: AppSource, val start: Screen = Screen.Home) : Mode
}

class MainActivity : ComponentActivity() {
    private val links by lazy { LinkStore(this) }
    private val artifactStore by lazy { ArtifactStore(this) }

    private fun open(linkText: String, back: Mode): Mode {
        val link = parsePairLink(linkText, BuildConfig.LINK_SCHEME) ?: return Mode.Paste(back, error = true)
        return Mode.Running(WsLinkSource(links, null, link))
    }

    /** Whether the phone is in its dark mode: what `system` follows at start. */
    private fun isNight(): Boolean =
        resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES

    /** Where to go when there is no PC connected: the list, or the welcome of a phone with none. */
    private fun idle(): Mode = if (links.load().pcs.isEmpty()) Mode.Welcome else Mode.Pcs(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // `am start -e live 127.0.0.1:7870` draws a daemon on the loopback link; `-e link jwn://…`
        // pairs from that link; otherwise the PC used last, else the welcome screen (REQ-88).
        val live = intent.getStringExtra("live")?.split(":")?.takeIf { it.size == 2 && it[1].toIntOrNull() != null }
        val initial: Mode = when {
            live != null -> Mode.Running(LiveSource(live[0], live[1].toInt()))
            intent.getStringExtra("link") != null -> open(intent.getStringExtra("link").orEmpty(), Mode.Welcome)
            else -> links.load().lastUsed()?.let { Mode.Running(WsLinkSource(links, it, null)) } ?: Mode.Welcome
        }
        val store = SeenStore(this)
        val folds = FoldedStore(this)
        val looks = LookStore(this)
        // Drawn in the chosen theme from the first frame, not after one in the default.
        Palette.use(looks.load().theme(isNight()))
        setContent {
            var mode by remember { mutableStateOf(initial) }
            var look by remember { mutableStateOf(looks.load()) }
            val osDark = isSystemInDarkTheme()
            val setLook = { l: Look ->
                look = l
                looks.save(l)
                Palette.use(l.theme(osDark))
            }
            SideEffect {
                Palette.use(look.theme(osDark))
                val bars = WindowCompat.getInsetsController(window, window.decorView)
                bars.isAppearanceLightStatusBars = !look.isDark(osDark)
                bars.isAppearanceLightNavigationBars = !look.isDark(osDark)
            }
            KeyboardRoot(Modifier.fillMaxSize().background(Palette.bg).statusBarsPadding()) {
                when (val m = mode) {
                    is Mode.Welcome -> WelcomeScreen({ mode = Mode.Scan(Mode.Welcome) }, { mode = Mode.Paste(Mode.Welcome) }, { mode = Mode.Running(FixtureSource(this@MainActivity)) })
                    is Mode.Scan -> {
                        BackHandler { mode = m.back }
                        ScanFlow({ mode = open(it, m.back) }, { mode = Mode.Paste(m.back) }) { mode = m.back }
                    }
                    is Mode.Paste -> {
                        BackHandler { mode = m.back }
                        PasteScreen(m.error, { mode = open(it, m.back) }) { mode = m.back }
                    }
                    is Mode.Pcs -> Pcs(m) { mode = it }
                    is Mode.Running -> Running(m.source, m.start, store, folds, look, osDark, setLook) { mode = it }
                }
            }
        }
    }

    @Composable
    private fun Pcs(m: Mode.Pcs, go: (Mode) -> Unit) {
        // The PC being connected to must say ready before the one in use is let go.
        LaunchedEffect(m.next) {
            val next = m.next ?: return@LaunchedEffect
            var handedOver = false
            try {
                val end = snapshotFlow { next.ui.value }.first { it is LinkUi.Connected || it is LinkUi.Failed }
                if (end is LinkUi.Connected) {
                    m.current?.close()
                    handedOver = true
                    go(Mode.Running(next))
                } else {
                    go(Mode.Pcs(m.current, from = m.from))
                }
            } finally {
                // Leaving the list while it connects (Back) must not leave a connection nobody holds.
                if (!handedOver) next.close()
            }
        }
        m.current?.let { current -> BackHandler { go(Mode.Running(current, m.from)) } }
        PcsScreen(
            store = links,
            current = m.current?.device,
            busy = m.next?.device,
            connect = { p: Pairing -> go(Mode.Pcs(m.current, WsLinkSource(links, p, null), m.from)) },
            open = { m.current?.let { go(Mode.Running(it, m.from)) } },
            forget = { p ->
                links.update { it.remove(p.device) }
                // A PC that is forgotten leaves nothing of its artifacts on this phone (REQ-784).
                artifactStore.wipe(p.device)
                val dropped = m.current?.device == p.device
                if (dropped) m.current?.close()
                go(if (links.load().pcs.isEmpty()) Mode.Welcome else Mode.Pcs(if (dropped) null else m.current, from = m.from))
            },
            add = {
                m.current?.close()
                go(Mode.Scan(Mode.Pcs(null)))
            },
            back = m.current?.let { current -> { go(Mode.Running(current, m.from)) } },
        )
    }

    @Composable
    private fun Running(source: AppSource, start: Screen, store: SeenStore, folds: FoldedStore, look: Look, osDark: Boolean, setLook: (Look) -> Unit, go: (Mode) -> Unit) {
        val link = source as? WsLinkSource
        val ui = link?.ui?.value
        // A phone the PC revoked keeps nothing of its artifacts (REQ-784); forgetting a PC does the same.
        LaunchedEffect(ui) { if (link != null && ui is LinkUi.Failed && ui.fail == LinkFail.Revoked) link.device?.let(artifactStore::wipe) }
        when (ui) {
            is LinkUi.Pairing -> return PairingScreen(ui) {
                link.close()
                go(idle())
            }
            is LinkUi.Failed -> return LinkFailScreen(
                ui.fail,
                { link.close(); go(Mode.Scan(idle())) },
                { link.close(); go(Mode.Paste(idle())) },
                { link.close(); go(idle()) },
            )
            else -> {}
        }
        // Back at the offline screen the next time there is no link, not at the list it was left on.
        var showSaved by remember(source, link?.offline?.value) { mutableStateOf(false) }
        val copies = remember(source) { ArtifactCopyBox(artifactStore, link?.device ?: "fixture") }
        if (link?.offline?.value == true) {
            // With no link the artifacts it opened before are still there, read only (REQ-783).
            if (showSaved) return SavedArtifactsScreen(copies.copies) { showSaved = false }
            return OfflineScreen(link.name, link::retryNow, copies.copies.copies.size, { showSaved = true }) { go(Mode.Pcs(link)) }
        }
        val nav = remember(source) { Navigator(source) }
        // A record from another epoch is not trusted: it is emptied before first use.
        // What is folded is kept for the PC it is folded on, by its device.
        val pc = link?.device ?: "fixture"
        var state by remember(source) { mutableStateOf(ClientState(screen = start, seen = store.load(), folded = folds.load(pc))) }
        val epoch = source.epoch
        LaunchedEffect(epoch) { if (epoch.isNotEmpty()) state = state.copy(seen = state.seen.forEpoch(epoch)) }
        LaunchedEffect(state.seen) { store.save(state.seen) }
        LaunchedEffect(state.folded) { folds.save(pc, state.folded) }
        BackHandler(enabled = state.screen is Screen.Workspace || state.screen is Screen.Settings || state.screen is Screen.Avisos || state.screen is Screen.Archived) { state = nav.back(state) }
        when (val screen = state.screen) {
            is Screen.Workspace -> WorkspaceScreen(source, state, screen, nav) { state = it }
            is Screen.Archived -> ArchivedScreen(source, state, nav) { state = it }
            is Screen.Folders -> FoldersScreen(source, state, nav) { state = it }
            is Screen.Files -> FilesScreen(source, state, screen, nav) { state = it }
            is Screen.Artifacts -> ArtifactsScreen(source, state, screen, nav) { state = it }
            is Screen.Artifact -> ArtifactScreen(source, state, screen, nav, copies) { state = it }
            is Screen.Settings -> {
                val pc = link?.let { PcEntry(it.name ?: "este PC", source.status, links.load().pcs.size) { go(Mode.Pcs(it, from = Screen.Settings)) } }
                SettingsScreen(state, nav, look, osDark, pc, setLook) { state = it }
            }
            else -> HomeScreen(source, state, nav) { state = it }
        }
    }
}
