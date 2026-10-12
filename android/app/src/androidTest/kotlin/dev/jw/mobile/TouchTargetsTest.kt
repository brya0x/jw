package dev.jw.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
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

/**
 * REQ-26: every control a finger can press is at least 48 dp each way. The test walks the connected screens against a
 * fake bridge on localhost (no PC, no tunnel), with stores in memory: it never touches the paired device or the
 * preferences of the installed app. Run it with `adb shell am instrument -e class dev.jw.mobile.TouchTargetsTest`, never
 * through `connectedDebugAndroidTest`, which uninstalls the app (and the pairing) afterwards.
 */
@RunWith(AndroidJUnit4::class)
class TouchTargetsTest {
    @get:Rule val rule = createAndroidComposeRule<androidx.activity.ComponentActivity>()
    private val server = MockWebServer()
    private lateinit var session: Session
    private val nav = Nav()
    private val device = "ef".repeat(32)
    private val bad = mutableListOf<String>()

    private val workspaces = """{"t":"workspaces","session":"main","items":[
      {"id":"r1","kind":"root","name":"jw","path":"/home/me/jw","project":"jw","branch":"main","pr":null,"session":"main","parent":null},
      {"id":"w1","kind":"worktree","name":"alpha","path":"/home/me/jw-alpha","project":"jw","branch":"feat/alpha","pr":43,"session":"main","parent":"r1"}]}"""
    private fun pane(n: Int, s: String, role: String, agent: String?) =
        """{"pane":$n,"stream":"$s","role":"$role","exited":null,"fg":null,"busy":false,"bell":false,"agent":${agent?.let { "\"$it\"" }},"cwd":"/home/me"}"""
    private val panes = """{"t":"Panes","panes":[${pane(3, "w1", "agent", "waiting")},${pane(4, "w1", "shell", null)}]}"""
    private val diff = """{"t":"diff","ws":"w1","scope":"general","title":"alpha vs main","truncated":false,"too_large":null,"no_base":false,"files":[
      {"path":"a.rs","status":"modified","added":1,"deleted":1,"untracked":false,"hunks":[{"context":"","old_start":1,"new_start":1,"lines":[{"k":"del","text":"x","old":1,"new":null},{"k":"add","text":"y","old":null,"new":1}]}]},
      {"path":"b.rs","status":"added","added":1,"deleted":0,"untracked":false,"hunks":[]}]}"""
    private val commits = """{"t":"commits","ws":"w1","commits":[{"sha":"0123456789abcdef","short":"0123456","subject":"a commit","when":1}],"more":false,"no_base":false}"""

    private val ls = """{"t":"ls","req":"/home/me/jw-alpha","path":"/home/me/jw-alpha","parent":"/home/me","entries":[{"name":"docs","kind":"dir","size":null,"mtime":1,"target":null,"md":false},{"name":"README.md","kind":"file","size":10,"mtime":2,"target":null,"md":true}],"total":2,"truncated":false}"""
    private val readMd = """{"t":"read","req":"/home/me/jw-alpha/README.md","path":"/home/me/jw-alpha/README.md","kind":"md","size":8,"truncated":false,"text":"# Hi\n","blocks":[{"k":"h","level":1,"spans":[{"t":"Hi"}]}]}"""

    private fun raw(head: String, bytes: ByteArray): ByteString {
        val h = head.toByteArray()
        return ByteString.of(1, (h.size ushr 24).toByte(), (h.size ushr 16).toByte(), (h.size ushr 8).toByte(), h.size.toByte(), *h, *bytes)
    }

    @Before fun start() {
        server.enqueue(
            MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                override fun onMessage(webSocket: WebSocket, text: String) {
                    when {
                        text.contains("\"auth\"") -> {
                            webSocket.send("""{"t":"ready","bridge":1,"protocol":16,"read_only":false}""")
                            webSocket.send(workspaces)
                            webSocket.send("""{"t":"sessions","items":[{"name":"main","workspaces":1}]}""")
                        }
                        text.contains("\"diff\"") -> webSocket.send(diff)
                        text.contains("\"commits\"") -> webSocket.send(commits)
                        text.contains("\"t\":\"ls\"") -> webSocket.send(ls)
                        text.contains("\"t\":\"read\"") -> webSocket.send(readMd)
                    }
                }
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                    val b = bytes.toByteArray()
                    if (b[0].toInt() != 0) return
                    val json = String(b, 1, b.size - 1)
                    if (json.contains("Attach")) {
                        webSocket.send(raw("""{"t":"Snapshot","pane":3,"role":"agent","cols":40,"rows":10,"bytes":[]}""", "hello".toByteArray()))
                    } else {
                        webSocket.send(ByteString.of(0, *panes.toByteArray()))
                    }
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
        val answers = AnswerPrefs(androidx.test.core.app.ApplicationProvider.getApplicationContext())
        val viewed = ViewedMarks(object : MarkStore {
            override fun load() = emptySet<String>()
            override fun save(marks: Set<String>) {}
        })
        rule.runOnUiThread {
            session = Session(kotlinx.coroutines.MainScope(), store, Bridge(), "Test", { url }, answerIds = { null to null }, pollMs = 500)
            session.start()
        }
        rule.setContent {
            JwTheme(Palettes.FALLBACK) {
                Surface(Modifier.fillMaxSize(), color = LocalJw.current.bg) {
                    Box(Modifier.fillMaxSize().background(LocalJw.current.bg)) {
                        val s by session.state.collectAsState()
                        (s as? UiState.Connected)?.let { ConnectedApp(it, session, nav, answers, viewed, ticking = false) }
                    }
                }
            }
        }
    }

    @After fun stop() { session.stop(); server.shutdown() }

    private fun back() {
        rule.activityRule.scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
        rule.waitForIdle()
    }

    private fun waitFor(text: String, substring: Boolean = false) =
        rule.waitUntil(15_000) { rule.onAllNodesWithText(text, substring = substring).fetchSemanticsNodes().isNotEmpty() }

    /** Every node that can be pressed and is smaller than 48 dp either way, by what it says. */
    private fun measure(where: String) {
        rule.waitForIdle()
        val d = rule.density
        for (n in rule.onAllNodes(androidx.compose.ui.test.SemanticsMatcher("can be pressed") {
            it.config.getOrNull(SemanticsActions.OnClick) != null || it.config.getOrNull(SemanticsActions.SetText) != null
        }).fetchSemanticsNodes()) {
            val w = with(d) { n.size.width.toDp() }
            val h = with(d) { n.size.height.toDp() }
            if (w < 47.9.dp || h < 47.9.dp) {
                val said = n.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }
                    ?: n.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString()
                    ?: n.config.getOrNull(SemanticsActions.OnClick)?.label ?: "?"
                bad += "$where: \"$said\" ${w.value.toInt()}x${h.value.toInt()}"
            }
        }
    }

    @Test fun everyControlIsAtLeast48dp() {
        waitFor("alpha")
        measure("home")
        rule.onNodeWithContentDescription("Buscar").performClick()
        measure("home, search")
        rule.onNodeWithContentDescription("Cerrar búsqueda").performClick()
        rule.onNodeWithContentDescription("Abrir o crear").performClick()
        waitFor("nuevo worktree")
        measure("plus sheet")
        rule.onNodeWithText("nuevo worktree").performClick()
        waitFor("tarea para el agente", substring = true)
        measure("new worktree sheet")
        back()
        rule.onNodeWithContentDescription("Abrir o crear").performClick()
        waitFor("nueva sesión")
        rule.onNodeWithText("nueva sesión").performClick()
        waitFor("carpeta con la que empieza")
        measure("new session sheet")
        rule.onNodeWithText("~").performClick()
        waitFor("usar esta carpeta")
        measure("files, choosing a folder")
        back() // the picker: the session sheet comes back
        waitFor("carpeta con la que empieza")
        back()
        rule.onNodeWithText("main ▾", substring = true).performClick()
        waitFor("sesiones")
        measure("sessions sheet")
        back() // the sessions sheet
        rule.onNodeWithText("avisos").performClick()
        measure("avisos")
        rule.onNodeWithText("ajustes").performClick()
        waitFor("respuestas rápidas")
        measure("ajustes")
        rule.onNodeWithText("workspaces").performClick()
        waitFor("alpha")
        rule.onNodeWithText("alpha").performClick()
        waitFor("^␣")
        measure("workspace")
        rule.onNodeWithText("^␣").performClick()
        waitFor("buscar workspace")
        measure("leader sheet")
        rule.onNodeWithText("buscar workspace").performClick()
        measure("workspace picker")
        back() // the picker
        rule.onNode(androidx.compose.ui.test.SemanticsMatcher("the files button") { it.config.getOrNull(SemanticsActions.OnClick)?.label == "Archivos" }).performClick()
        waitFor("README.md")
        measure("files")
        rule.onNodeWithText("README.md").performClick()
        waitFor("fuente")
        measure("file viewer")
        back(); back()
        // Over to the workspace's changes.
        rule.onNodeWithText("±").performClick()
        waitFor("a.rs")
        measure("changes, general")
        rule.onNodeWithText("a.rs").performClick()
        waitFor("@@ -1 +1 @@")
        measure("changes, a file")
        back() // the file
        rule.onNodeWithText("Commits", substring = true).performClick()
        waitFor("a commit")
        measure("changes, commits")
        rule.onNodeWithText("a commit").performClick()
        measure("changes, a commit")
        assertTrue("controls under 48 dp:\n" + bad.joinToString("\n"), bad.isEmpty())
    }
}
