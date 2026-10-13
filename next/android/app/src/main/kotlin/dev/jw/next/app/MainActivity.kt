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
import dev.jw.next.client.Navigator
import dev.jw.next.client.Screen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // `am start -e live 127.0.0.1:7870` draws a running daemon; without it, the fixtures.
        val live = intent.getStringExtra("live")?.split(":")?.takeIf { it.size == 2 && it[1].toIntOrNull() != null }
        val source: AppSource = if (live != null) LiveSource(live[0], live[1].toInt()) else FixtureSource(this)
        val store = SeenStore(this)
        val nav = Navigator(source)
        setContent {
            // A record from another epoch is not trusted: it is emptied before first use.
            var state by remember { mutableStateOf(ClientState(seen = store.load())) }
            val epoch = source.epoch
            LaunchedEffect(epoch) { if (epoch.isNotEmpty()) state = state.copy(seen = state.seen.forEpoch(epoch)) }
            LaunchedEffect(state.seen) { store.save(state.seen) }
            BackHandler(enabled = state.screen is Screen.Workspace) { state = nav.back(state) }
            Box(Modifier.fillMaxSize().background(Palette.bg).statusBarsPadding()) {
                when (val screen = state.screen) {
                    is Screen.Workspace -> WorkspaceScreen(source, state, screen, nav) { state = it }
                    else -> HomeScreen(source, state, nav) { state = it }
                }
            }
        }
    }
}
