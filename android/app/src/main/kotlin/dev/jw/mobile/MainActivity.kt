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
    val session = Session(
        scope = viewModelScope,
        store = KeystoreTokenStore(app),
        bridge = Bridge(),
        deviceName = Build.MODEL ?: "phone",
    ).also { it.start() }

    /** The last palettes the PC sent, so the theme survives a reconnect. */
    var palettes: Palettes = Palettes.FALLBACK

    override fun onCleared() = session.stop()
}

class MainActivity : ComponentActivity() {
    private val vm: SessionViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val state by vm.session.state.collectAsState()
            (state as? UiState.Connected)?.let { vm.palettes = it.palettes }
            JwTheme(vm.palettes) { App(state, vm.session) }
        }
    }
}

@Composable
private fun App(state: UiState, session: Session) {
    val j = LocalJw.current
    // Material's Text takes its style from here, so everything is mono like the prototype.
    androidx.compose.material3.Surface(color = j.bg, contentColor = j.fg, modifier = Modifier.fillMaxSize()) {
      androidx.compose.material3.ProvideTextStyle(androidx.compose.ui.text.TextStyle(fontFamily = JwMono)) {
        Box(Modifier.fillMaxSize().background(j.bg).systemBarsPadding()) {
            when (state) {
                is UiState.Scan -> ScanScreen(state, session)
                is UiState.Pairing -> Pairing(state, session)
                is UiState.Connecting -> Waiting("conectando", state.host, "…", session)
                is UiState.Connected -> Connected(state, session)
                is UiState.Mismatch -> Mismatch(state, session)
                is UiState.Reconnecting -> Reconnecting(state, session)
            }
        }
      }
    }
}

@Composable
private fun ScanScreen(state: UiState.Scan, session: Session) {
    val j = LocalJw.current
    val ctx = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    var wrong by remember { mutableStateOf(false) }
    Screen("conecta tu PC", "jw mobile", "SCAN", j.blue, if (granted) "apunta al QR" else "falta la cámara") {
        Body("En tu PC ejecuta `jw mobile` y escanea el código.", j.dim)
        state.notice?.let { Body(it, j.yellow) }
        if (wrong) Body("Ese QR no es de jw mobile.", j.yellow)
        if (granted) {
            QrScanner(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 14.dp)) { text -> wrong = !session.onQr(text) }
        } else {
            Body("Hace falta la cámara para leer el QR.", j.dim)
            Box(Modifier.padding(horizontal = 14.dp)) { JwButton("permitir cámara", fill = true) { ask.launch(Manifest.permission.CAMERA) } }
        }
    }
}

@Composable
private fun Pairing(state: UiState.Pairing, session: Session) {
    val j = LocalJw.current
    Screen("emparejando", state.host, "PAIR", j.yellow, "compara el código en el PC") {
        Body("Compara este código con el del PC y acepta allí.", j.dim)
        Text(
            state.code.chunked(3).joinToString(" "), color = j.cyan, fontSize = 44.sp, fontWeight = FontWeight.Bold,
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
        )
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("cancelar") { session.forget() } }
    }
}

@Composable
private fun Waiting(title: String, host: String, hint: String, session: Session) {
    val j = LocalJw.current
    Screen(title, host, "WAIT", j.yellow, hint) {
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
}

@Composable
private fun Connected(state: UiState.Connected, session: Session) {
    val j = LocalJw.current
    Screen("conectado", state.host, "CONN", j.green, if (state.readOnly) "solo lectura" else "listo") {
        Section("conexión")
        KvRow("host", state.host)
        KvRow("protocolo", "jw-proto v${state.protocol}", j.green)
        KvRow("acceso", if (state.readOnly) "solo lectura" else "lectura y escritura", if (state.readOnly) j.yellow else j.green)
        if (!state.saved) Body("No se pudo guardar el emparejamiento: al cerrar la app tendrás que escanear de nuevo.", j.yellow, 12.0)
        Body("Los espacios de trabajo llegan con la siguiente parte.", j.dim, 12.0)
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
}

@Composable
private fun Mismatch(state: UiState.Mismatch, session: Session) {
    val j = LocalJw.current
    val newer = state.pc > state.app
    Screen("versión incompatible", state.host, "ERR", j.red, "no me conecto") {
        Section("protocolo")
        KvRow("esta app", "jw-proto v${state.app}")
        KvRow("el PC", "jw-proto v${state.pc}", j.red)
        Body(if (newer) "Actualiza la app jw en este teléfono." else "Actualiza jw en el PC y reinicia su daemon.", j.yellow)
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
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
    Screen("reconectando", state.host, "WAIT", j.yellow, "reintento en ${state.inMs / 1000} s") {
        if (state.unreachable) {
            Body("No encuentro tu PC. Si reiniciaste jw mobile, escanea el código nuevo.", j.yellow)
            Box(Modifier.padding(horizontal = 14.dp)) { JwButton("Escanear otro código", fill = true) { session.scanAnother() } }
        } else {
            Body(why, j.yellow)
        }
        Box(Modifier.padding(horizontal = 14.dp)) { JwButton("olvidar este PC") { session.forget() } }
    }
}
