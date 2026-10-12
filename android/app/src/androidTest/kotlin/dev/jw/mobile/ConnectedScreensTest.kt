package dev.jw.mobile

import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.jw.proto.Palettes
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okio.ByteString
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The M4 screens on a real device, against a fake bridge on localhost (no PC, no tunnel).
 * Writes screenshots to the app's cache dir (`shots/`); pull them with `run-as`.
 */
@RunWith(AndroidJUnit4::class)
class ConnectedScreensTest {
    @get:Rule val rule = createComposeRule()
    private val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val server = MockWebServer()
    private val inputs = LinkedBlockingQueue<ByteArray>()
    private lateinit var session: Session
    private val nav = Nav()
    private val device = "ef".repeat(32)

    private val workspaces = """{"t":"workspaces","session":"main","items":[
      {"id":"r1","kind":"root","name":"jw","path":"/home/me/jw","project":"jw","branch":"main","pr":null,"session":"main","parent":null},
      {"id":"w1","kind":"worktree","name":"alpha","path":"/home/me/jw-alpha","project":"jw","branch":"feat/alpha","pr":43,"session":"main","parent":"r1"},
      {"id":"w2","kind":"worktree","name":"beta","path":"/home/me/jw-beta","project":"jw","branch":"feat/beta","pr":null,"session":"main","parent":"r1"},
      {"id":"f1","kind":"folder","name":"Notes","path":"/home/me/notes","project":"","branch":"","pr":null,"session":"main","parent":null}]}"""
    private fun pane(n: Int, s: String, role: String, agent: String?) =
        """{"pane":$n,"stream":"$s","role":"$role","exited":null,"fg":null,"busy":false,"bell":false,"agent":${agent?.let { "\"$it\"" }},"cwd":"/home/me"}"""
    private val panes = """{"t":"Panes","panes":[${pane(1, "r1", "agent", "working")},${pane(2, "r1", "dev:web", null)},${pane(3, "w1", "agent", "waiting")},${pane(4, "w1", "shell", null)},${pane(5, "f1", "shell", null)}]}"""

    @Before fun start() {
        server.enqueue(
            MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                override fun onMessage(webSocket: WebSocket, text: String) {
                    if (text.contains("\"auth\"")) {
                        webSocket.send("""{"t":"ready","bridge":1,"protocol":16,"read_only":false}""")
                        webSocket.send(workspaces)
                        webSocket.send("""{"t":"sessions","items":[{"name":"main","workspaces":4},{"name":"work","workspaces":1}]}""")
                    }
                }
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                    val b = bytes.toByteArray()
                    if (b[0].toInt() == 0) webSocket.send(ByteString.of(0, *panes.toByteArray())) else inputs.add(b)
                }
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {}
            }),
        )
        server.start()
        val url = server.url("/").toString()
        val store = object : TokenStore {
            override fun load() = StoredDevice("x.trycloudflare.com", device)
            override fun save(device: StoredDevice) {}
            override fun clear() {}
        }
        rule.runOnUiThread {
            session = Session(kotlinx.coroutines.MainScope(), store, Bridge(), "Test", { url }, answerIds = { null to null }, pollMs = 500)
            session.start()
        }
        rule.setContent {
            JwTheme(Palettes.FALLBACK) {
                Surface(Modifier.fillMaxSize(), color = LocalJw.current.bg) {
                    Box(Modifier.fillMaxSize().background(LocalJw.current.bg)) {
                        val s by session.state.collectAsState()
                        (s as? UiState.Connected)?.let { ConnectedApp(it, session, nav, AnswerPrefs(ctx), ViewedMarks(PrefsMarkStore(ctx)), ticking = false) }
                    }
                }
            }
        }
    }

    @After fun stop() { session.stop(); server.shutdown() }

    private fun shot(name: String) {
        val dir = File(ctx.cacheDir, "shots").apply { mkdirs() }
        File(dir, "$name.png").outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private fun waitFor(text: String) = rule.waitUntil(15_000) { rule.onAllNodesWithText(text).fetchSemanticsNodes().isNotEmpty() }

    @Test fun walkTheScreens() {
        waitFor("alpha")
        waitFor("y · aprobar")
        shot("m4-1-home")
        // Search filters what the PC already sent, by name, branch or path.
        rule.onNodeWithContentDescription("Buscar").performClick()
        rule.onNodeWithText("buscar workspace", substring = true).performTextInput("beta")
        rule.waitForIdle()
        assertTrue(rule.onAllNodesWithText("alpha").fetchSemanticsNodes().isEmpty())
        assertTrue(rule.onAllNodesWithText("beta").fetchSemanticsNodes().isNotEmpty())
        shot("m4-1b-search")
        rule.onNodeWithContentDescription("Cerrar búsqueda").performClick()
        waitFor("alpha")
        rule.onNodeWithText("main ▾", substring = true).performClick()
        waitFor("sesiones")
        shot("m4-2-sessions")
        rule.onNodeWithText("work").performClick() // picks the other session and closes the sheet
        rule.onNodeWithText("alpha").performClick()
        waitFor("espera tu respuesta")
        shot("m4-3-workspace")
        rule.onNodeWithText("y · sí").performClick()
        val sent = inputs.poll(5, TimeUnit.SECONDS)
        assertTrue("Input frame reached the bridge", sent != null && sent[0].toInt() == 1)
        shot("m4-4-after-yes")
        rule.onNodeWithText("‹").performClick()
        rule.onNodeWithText("avisos").performClick()
        waitFor("abrir")
        shot("m4-5-avisos")
        rule.onNodeWithText("ajustes").performClick()
        waitFor("respuestas rápidas")
        shot("m4-6-settings")
    }
}
