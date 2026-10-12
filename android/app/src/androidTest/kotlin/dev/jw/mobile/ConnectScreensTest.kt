package dev.jw.mobile

import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.jw.proto.Palettes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * The connection screens (REQ-51 to REQ-54), drawn from fixed state: no camera, no bridge. The camera
 * permission being blocked cannot be reached by hand on every ROM (`pm revoke` is refused to adb), so
 * its screen is drawn from a flag here. Screenshots go to the app's cache dir (`shots/`, private).
 * The theme is the system's: run once with `cmd uimode night yes` and once with `no`.
 */
@RunWith(AndroidJUnit4::class)
class ConnectScreensTest {
    @get:Rule val rule = createComposeRule()
    private val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val reach = Reach(internet = true, mobileData = true, cloudflare = true)
    private val host = "virtual-archive-bunny-economies.trycloudflare.com"

    private fun show(content: @Composable () -> Unit) {
        rule.setContent {
            JwTheme(Palettes.FALLBACK) {
                Surface(Modifier.fillMaxSize(), color = LocalJw.current.bg) {
                    androidx.compose.material3.ProvideTextStyle(androidx.compose.ui.text.TextStyle(fontFamily = JwMono)) { content() }
                }
            }
        }
    }

    private fun shot(name: String) {
        val night = (ctx.resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES
        val dir = File(ctx.cacheDir, "shots").apply { mkdirs() }
        File(dir, "$name-${if (night) "dark" else "light"}.png").outputStream().use {
            rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it)
        }
    }

    @Test fun connect() {
        show { ConnectScreen({}, {}) }
        rule.onNodeWithText("escanear").assertExists()
        rule.onNodeWithText("pegar enlace").assertExists()
        shot("connect")
    }

    @Test fun permissionAskedAndBlocked() {
        var blocked by mutableStateOf(false)
        show { PermissionScreen(blocked, {}, {}, {}, {}) }
        rule.onNodeWithText("necesito la cámara").assertExists()
        rule.onNodeWithText("permitir cámara").assertExists()
        shot("perm")
        rule.runOnUiThread { blocked = true }
        rule.onNodeWithText("la cámara está bloqueada").assertExists()
        rule.onNodeWithText("abrir ajustes del sistema").assertExists()
        shot("permdenied")
    }

    @Test fun scanWithAndWithoutTorchAndPasteSheet() {
        var torch by mutableStateOf(true)
        var sheet by mutableStateOf(false)
        var error by mutableStateOf<String?>(null)
        var connected = ""
        show {
            Box(Modifier.fillMaxSize()) {
                ScanScreenUi(hit = false, torchAvailable = torch, torchOn = false, onTorch = {}, onPaste = { sheet = true }, onBack = {}) {
                    // Stands in for the camera: a dark feed with a code in it.
                    Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Color(0xFF10141B), Color(0xFF06080C)))))
                    DecorativeQr(Modifier.fillMaxWidth(.6f).aspectRatio(1f).padding(8.dp))
                }
                if (sheet) PasteSheet(error, { connected = it; error = LINK_REFUSED }, { sheet = false })
            }
        }
        rule.onNodeWithText("$ jw scan", substring = true).assertExists()
        shot("scan")
        rule.runOnUiThread { torch = false }
        rule.onNodeWithText("pegar enlace").assertExists()
        shot("scan-notorch")
        rule.onNodeWithText("pegar enlace").performClick()
        rule.onNodeWithText("conectar").performClick()
        rule.onNodeWithText(LINK_REFUSED).assertExists()
        shot("paste")
        assertEquals("", connected)
    }

    @Test fun pairing() {
        show { PairScreen(host, "482913", reached = true, via = Route.Wifi, onCancel = {}) }
        rule.onNodeWithText("482 913").assertExists()
        rule.onNodeWithText("PC alcanzado · por wifi").assertExists()
        shot("pair")
    }

    @Test fun failures() {
        var why by mutableStateOf(ScanWhy.DENIED)
        show { FailScreen(why, {}, {}) }
        for ((w, name) in listOf(ScanWhy.DENIED to "denied", ScanWhy.EXPIRED to "expired", ScanWhy.LIMITED to "limited")) {
            rule.runOnUiThread { why = w }
            rule.waitForIdle()
            shot(name)
        }
        rule.onNodeWithText("volver").assertExists()
    }

    @Test fun unreachableShowsOnlyWhatWasMeasured() {
        var r by mutableStateOf(reach)
        show { UnreachableScreen(host, r, {}, {}, {}) }
        rule.onNodeWithText("Cloudflare responde", substring = true).assertExists()
        shot("unreach")
        rule.runOnUiThread { r = Reach() }
        rule.waitForIdle()
        assertTrue("an unmeasured row is not drawn", rule.onAllNodesWithTextCount("Cloudflare responde") == 0)
        assertTrue(rule.onAllNodesWithTextCount("internet") == 0)
        rule.onNodeWithText("sin respuesta", substring = true).assertExists()
        shot("unreach-unmeasured")
    }

    private fun androidx.compose.ui.test.junit4.ComposeContentTestRule.onAllNodesWithTextCount(t: String) =
        onAllNodes(androidx.compose.ui.test.hasText(t, substring = true)).fetchSemanticsNodes().size

    @Test fun protocolNamesTheSideBehind() {
        var pc by mutableStateOf(14)
        show { ProtocolScreen(app = 14, pc = pc, onBack = {}) }
        rule.onNodeWithText("Tu app está atrasada", substring = true).assertExists()
        shot("proto")
        rule.runOnUiThread { pc = 12 }
        rule.waitForIdle()
        rule.onNodeWithText("Tu jw está atrasado", substring = true).assertExists()
        shot("protojw")
    }
}
