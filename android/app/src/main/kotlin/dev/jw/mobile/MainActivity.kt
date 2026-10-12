package dev.jw.mobile

import android.Manifest
import android.app.Application
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.jw.proto.PROTOCOL
import dev.jw.proto.Palettes

class SessionViewModel(app: Application) : AndroidViewModel(app) {
    val answers = AnswerPrefs(app)
    val nav = Nav()
    val session = Session(
        scope = viewModelScope,
        store = KeystoreTokenStore(app),
        bridge = Bridge(),
        deviceName = Build.MODEL ?: "phone",
        answerIds = { answers.approve to answers.reject },
        network = { phoneNetwork(app) },
    ).also { it.start() }

    /** The last palettes the PC sent, so the theme survives a reconnect. */
    var palettes: Palettes = Palettes.FALLBACK

    override fun onCleared() = session.stop()
}

/** The phone's connectivity from the system, or null when it cannot be read. */
private fun phoneNetwork(app: Application): Net? = try {
    val cm = app.getSystemService(android.net.ConnectivityManager::class.java)
    val caps = cm.getNetworkCapabilities(cm.activeNetwork)
    if (caps == null) {
        Net(internet = false, mobile = false, wifi = false)
    } else {
        Net(
            internet = caps.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET),
            mobile = caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_CELLULAR),
            wifi = caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_WIFI),
        )
    }
} catch (_: Exception) {
    null
}

class MainActivity : ComponentActivity() {
    private val vm: SessionViewModel by viewModels()

    override fun onResume() {
        super.onResume()
        vm.session.setForeground(true)
    }

    override fun onPause() {
        vm.session.setForeground(false)
        super.onPause()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val state by vm.session.state.collectAsState()
            (state as? UiState.Connected)?.let { vm.palettes = it.palettes }
            JwTheme(vm.palettes) { App(state, vm.session, vm.nav, vm.answers) }
        }
    }
}

@Composable
private fun App(state: UiState, session: Session, nav: Nav, answers: AnswerPrefs) {
    val j = LocalJw.current
    // Material's Text takes its style from here, so everything is mono like the prototype.
    androidx.compose.material3.Surface(color = j.bg, contentColor = j.fg, modifier = Modifier.fillMaxSize()) {
      androidx.compose.material3.ProvideTextStyle(androidx.compose.ui.text.TextStyle(fontFamily = JwMono)) {
        Box(Modifier.fillMaxSize().background(j.bg).systemBarsPadding()) {
            when (state) {
                is UiState.Scan -> ScanFlow(state, session)
                is UiState.Pairing -> Pairing(state, session)
                is UiState.Connecting -> Waiting("conectando", state.host, "…", session)
                is UiState.Connected -> ConnectedApp(state, session, nav, answers)
                is UiState.Mismatch -> Mismatch(state, session)
                is UiState.Reconnecting -> Reconnecting(state, session)
            }
        }
      }
    }
}

/** Where the scan flow is: the connect screen, the camera permission, or the camera. */
private enum class Step { CONNECT, PERMISSION, CAMERA }

/**
 * Everything before a PC is paired (REQ-51 to REQ-54): the connect screen, camera permission, the scan
 * screen with its paste sheet, and the failure screen a failed pairing ends on. The step is the screen's
 * own: a new `state` (a failure, a forget) starts it over.
 */
@Composable
private fun ScanFlow(state: UiState.Scan, session: Session) {
    val ctx = LocalContext.current
    var step by remember(state) { mutableStateOf(Step.CONNECT) }
    var paste by remember(state) { mutableStateOf(false) }
    var linkError by remember(state) { mutableStateOf<String?>(null) }
    var failShown by remember(state) { mutableStateOf(state.why != null) }
    var asked by remember { mutableStateOf(false) }
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        granted = it
        asked = true
        if (it) step = Step.CAMERA
    }
    val activity = ctx as? android.app.Activity
    // Denied for good: asked once, refused, and Android will not show the dialog again.
    val blocked = asked && !granted && activity != null &&
        !androidx.core.app.ActivityCompat.shouldShowRequestPermissionRationale(activity, Manifest.permission.CAMERA)
    val openSettings = {
        ctx.startActivity(
            android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, android.net.Uri.fromParts("package", ctx.packageName, null))
                .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
    val tryLink = { text: String ->
        if (session.onQr(text)) linkError = null else linkError = LINK_REFUSED
    }
    val why = state.why
    if (why != null && failShown) {
        val again = { failShown = false }
        when (why) {
            ScanWhy.UNREACHABLE -> UnreachableScreen(state.host ?: "", state.reach, onScanAnother = again, onRetry = null, onBack = again)
            else -> FailScreen(why, onScanAnother = again, onBack = again)
        }
        return
    }
    Box(Modifier.fillMaxSize()) {
        when (step) {
            Step.CONNECT -> ConnectScreen(
                onScan = { step = if (granted) Step.CAMERA else Step.PERMISSION },
                onPaste = { paste = true },
            )
            Step.PERMISSION -> PermissionScreen(
                blocked = blocked,
                onAllow = { ask.launch(Manifest.permission.CAMERA) },
                onSettings = openSettings,
                onPaste = { paste = true },
                onBack = { step = Step.CONNECT },
            )
            Step.CAMERA -> {
                var torchAvailable by remember { mutableStateOf(false) }
                var torchOn by remember { mutableStateOf(false) }
                var hit by remember { mutableStateOf(false) }
                ScanScreenUi(
                    hit = hit, torchAvailable = torchAvailable, torchOn = torchOn,
                    onTorch = { torchOn = !torchOn }, onPaste = { paste = true }, onBack = { step = Step.CONNECT },
                ) {
                    QrScanner(
                        Modifier.matchParentSize(), torch = torchOn,
                        onTorchAvailable = { torchAvailable = it },
                    ) { text -> if (session.onQr(text)) hit = true }
                }
            }
        }
        if (paste) {
            PasteSheet(
                error = linkError,
                onConnect = tryLink,
                onClose = { paste = false; linkError = null },
            )
        }
    }
}

@Composable
private fun Pairing(state: UiState.Pairing, session: Session) {
    PairScreen(state.host, state.code, state.reached, state.via, onCancel = { session.scanAnother() })
}

@Composable
private fun Waiting(title: String, host: String, hint: String, session: Session) {
    val j = LocalJw.current
    Screen(title, host, "WAIT", j.yellow, hint) {
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
}

@Composable
private fun Mismatch(state: UiState.Mismatch, session: Session) {
    ProtocolScreen(state.app, state.pc, onBack = { session.scanAnother() })
}

@Composable
private fun Reconnecting(state: UiState.Reconnecting, session: Session) {
    val j = LocalJw.current
    val why = when (state.code) {
        "daemon" -> "El PC no tiene un daemon de jw activo."
        "protocol" -> "El puente y el daemon del PC no coinciden; reinicia jw en el PC."
        "limited" -> "Demasiados intentos: espero 5 minutos."
        else -> "Sin conexión con el PC."
    }
    if (state.unreachable) {
        UnreachableScreen(state.host, state.reach, onScanAnother = { session.scanAnother() }, onRetry = null, onBack = { session.scanAnother() })
        return
    }
    Screen("reconectando", state.host, "WAIT", j.yellow, "reintento en ${state.inMs / 1000} s") {
        Body(why, j.yellow)
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
}
