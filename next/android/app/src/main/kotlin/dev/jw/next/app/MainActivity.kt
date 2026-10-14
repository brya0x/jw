package dev.jw.next.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.jw.next.client.ClientState
import dev.jw.next.client.LinkFail
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen
import dev.jw.next.client.parsePairLink

/** What the app is showing at the top level: no PC yet, or a source of nodes. */
private sealed interface Mode {
    data object Welcome : Mode
    data class Paste(val error: Boolean = false) : Mode
    data class Running(val source: AppSource) : Mode
}

class MainActivity : ComponentActivity() {
    private val links by lazy { LinkStore(this) }

    private fun open(linkText: String): Mode {
        val link = parsePairLink(linkText, BuildConfig.LINK_SCHEME) ?: return Mode.Paste(error = true)
        return Mode.Running(WsLinkSource(links, null, link))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // `am start -e live 127.0.0.1:7870` draws a daemon on the loopback link; `-e link jwn://…`
        // pairs from that link; otherwise the paired PC, else the welcome screen.
        val live = intent.getStringExtra("live")?.split(":")?.takeIf { it.size == 2 && it[1].toIntOrNull() != null }
        val initial: Mode = when {
            live != null -> Mode.Running(LiveSource(live[0], live[1].toInt()))
            intent.getStringExtra("link") != null -> open(intent.getStringExtra("link").orEmpty())
            else -> links.load()?.let { Mode.Running(WsLinkSource(links, it, null)) } ?: Mode.Welcome
        }
        val store = SeenStore(this)
        setContent {
            var mode by remember { mutableStateOf(initial) }
            Box(Modifier.fillMaxSize().background(Palette.bg).statusBarsPadding()) {
                when (val m = mode) {
                    is Mode.Welcome -> WelcomeScreen({ mode = Mode.Paste() }, { mode = Mode.Running(FixtureSource(this@MainActivity)) })
                    is Mode.Paste -> {
                        BackHandler { mode = Mode.Welcome }
                        PasteScreen(m.error, { mode = open(it) }) { mode = Mode.Welcome }
                    }
                    is Mode.Running -> Running(m.source, store, links) { mode = it }
                }
            }
        }
    }

    @androidx.compose.runtime.Composable
    private fun Running(source: AppSource, store: SeenStore, links: LinkStore, go: (Mode) -> Unit) {
        val link = source as? WsLinkSource
        val ui = link?.ui?.value
        val leave = { link?.close(); go(Mode.Welcome) }
        when (ui) {
            is LinkUi.Pairing -> return PairingScreen(ui, leave)
            is LinkUi.Failed -> return LinkFailScreen(ui.fail, { link.close(); go(Mode.Paste()) }, {
                link.close()
                go(Mode.Welcome)
            })
            else -> {}
        }
        val nav = remember(source) { Navigator(source) }
        // A record from another epoch is not trusted: it is emptied before first use.
        var state by remember(source) { mutableStateOf(ClientState(seen = store.load())) }
        val epoch = source.epoch
        LaunchedEffect(epoch) { if (epoch.isNotEmpty()) state = state.copy(seen = state.seen.forEpoch(epoch)) }
        LaunchedEffect(state.seen) { store.save(state.seen) }
        BackHandler(enabled = state.screen is Screen.Workspace) { state = nav.back(state) }
        val forget = link?.let { { it.close(); links.save(null); go(Mode.Welcome) } }
        when (val screen = state.screen) {
            is Screen.Workspace -> WorkspaceScreen(source, state, screen, nav) { state = it }
            else -> HomeScreen(source, state, nav, forget) { state = it }
        }
    }
}
