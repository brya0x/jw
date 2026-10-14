package dev.jw.next.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import dev.jw.next.client.ClientState
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
    data class Pcs(val current: WsLinkSource?, val next: WsLinkSource? = null) : Mode
    data class Running(val source: AppSource) : Mode
}

class MainActivity : ComponentActivity() {
    private val links by lazy { LinkStore(this) }

    private fun open(linkText: String, back: Mode): Mode {
        val link = parsePairLink(linkText, BuildConfig.LINK_SCHEME) ?: return Mode.Paste(back, error = true)
        return Mode.Running(WsLinkSource(links, null, link))
    }

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
        setContent {
            var mode by remember { mutableStateOf(initial) }
            Box(Modifier.fillMaxSize().background(Palette.bg).statusBarsPadding()) {
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
                    is Mode.Running -> Running(m.source, store) { mode = it }
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
                    go(Mode.Pcs(m.current))
                }
            } finally {
                // Leaving the list while it connects (Back) must not leave a connection nobody holds.
                if (!handedOver) next.close()
            }
        }
        m.current?.let { current -> BackHandler { go(Mode.Running(current)) } }
        PcsScreen(
            store = links,
            current = m.current?.device,
            busy = m.next?.device,
            connect = { p: Pairing -> go(Mode.Pcs(m.current, WsLinkSource(links, p, null))) },
            open = { m.current?.let { go(Mode.Running(it)) } },
            forget = { p ->
                links.update { it.remove(p.device) }
                val dropped = m.current?.device == p.device
                if (dropped) m.current?.close()
                go(if (links.load().pcs.isEmpty()) Mode.Welcome else Mode.Pcs(if (dropped) null else m.current))
            },
            add = {
                m.current?.close()
                go(Mode.Scan(Mode.Pcs(null)))
            },
            back = m.current?.let { current -> { go(Mode.Running(current)) } },
        )
    }

    @Composable
    private fun Running(source: AppSource, store: SeenStore, go: (Mode) -> Unit) {
        val link = source as? WsLinkSource
        val ui = link?.ui?.value
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
        val pcs = link?.let { { go(Mode.Pcs(it)) } }
        if (link?.offline?.value == true) return OfflineScreen(link.name, link::retryNow) { pcs?.invoke() }
        val nav = remember(source) { Navigator(source) }
        // A record from another epoch is not trusted: it is emptied before first use.
        var state by remember(source) { mutableStateOf(ClientState(seen = store.load())) }
        val epoch = source.epoch
        LaunchedEffect(epoch) { if (epoch.isNotEmpty()) state = state.copy(seen = state.seen.forEpoch(epoch)) }
        LaunchedEffect(state.seen) { store.save(state.seen) }
        BackHandler(enabled = state.screen is Screen.Workspace) { state = nav.back(state) }
        when (val screen = state.screen) {
            is Screen.Workspace -> WorkspaceScreen(source, state, screen, nav) { state = it }
            else -> HomeScreen(source, state, nav, pcs) { state = it }
        }
    }
}
