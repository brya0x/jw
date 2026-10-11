package dev.jw.mobile

import android.Manifest
import android.app.Application
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
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
    Box(Modifier.fillMaxSize().background(j.bg).systemBarsPadding()) {
        when (state) {
            is UiState.Scan -> ScanScreen(state, session)
            is UiState.Pairing -> Pairing(state, session)
            is UiState.Connecting -> Message("conectando…", state.host, session)
            is UiState.Connected -> Connected(state, session)
            is UiState.Mismatch -> Mismatch(state, session)
            is UiState.Reconnecting -> Reconnecting(state, session)
        }
    }
}

@Composable
private fun ScanScreen(state: UiState.Scan, session: Session) {
    val ctx = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    var wrong by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Title("conecta tu PC")
        Dim("En el PC ejecuta `jw mobile` y escanea el QR.")
        state.notice?.let { Warn(it) }
        if (wrong) Warn("Ese QR no es de jw mobile.")
        if (granted) {
            QrScanner(Modifier.weight(1f).fillMaxWidth()) { text -> wrong = !session.onQr(text) }
        } else {
            Dim("Hace falta la cámara para leer el QR.")
            Button(onClick = { ask.launch(Manifest.permission.CAMERA) }, modifier = Modifier.heightIn(min = 48.dp)) {
                Text("permitir cámara")
            }
        }
    }
}

@Composable
private fun Pairing(state: UiState.Pairing, session: Session) {
    val j = LocalJw.current
    Column(
        Modifier.fillMaxSize().padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Title("emparejando")
        Dim("Compara este código con el que muestra el PC y acepta allí.")
        Text(state.code.chunked(3).joinToString(" "), color = j.cyan, fontSize = 48.sp, fontWeight = FontWeight.Bold)
        Dim(state.host)
        OutlinedButton(onClick = session::forget, modifier = Modifier.heightIn(min = 48.dp)) { Text("cancelar") }
    }
}

@Composable
private fun Message(title: String, detail: String, session: Session) {
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically)) {
        Title(title)
        Dim(detail)
        OutlinedButton(onClick = session::forget, modifier = Modifier.heightIn(min = 48.dp)) { Text("olvidar este PC") }
    }
}

@Composable
private fun Connected(state: UiState.Connected, session: Session) {
    val j = LocalJw.current
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Title("conectado")
        Column(Modifier.fillMaxWidth().background(j.panel).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row { Text("host      ", color = j.dim); Text(state.host, color = j.fg) }
            Row { Text("protocolo ", color = j.dim); Text("jw-proto v${state.protocol}", color = j.green) }
            Row {
                Text("acceso    ", color = j.dim)
                Text(if (state.readOnly) "solo lectura" else "lectura y escritura", color = if (state.readOnly) j.yellow else j.green)
            }
        }
        Dim("Los espacios de trabajo llegan con la siguiente parte.")
        OutlinedButton(onClick = session::forget, modifier = Modifier.heightIn(min = 48.dp)) { Text("olvidar este PC") }
    }
}

@Composable
private fun Mismatch(state: UiState.Mismatch, session: Session) {
    val j = LocalJw.current
    val newer = state.pc > state.app
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically)) {
        Title("versión incompatible")
        Text("Esta app habla jw-proto v${state.app}; el PC habla v${state.pc}.", color = j.fg)
        Text(
            if (newer) "Actualiza la app jw en este teléfono." else "Actualiza jw en el PC y reinicia su daemon.",
            color = j.yellow,
        )
        Dim("No me conecto hasta que coincidan.")
        OutlinedButton(onClick = session::forget, modifier = Modifier.heightIn(min = 48.dp)) { Text("olvidar este PC") }
    }
}

@Composable
private fun Reconnecting(state: UiState.Reconnecting, session: Session) {
    val why = when (state.code) {
        "daemon" -> "El PC no tiene daemon de jw activo."
        "protocol" -> "El puente y el daemon del PC no coinciden; reinicia jw en el PC."
        "limited" -> "Demasiados intentos: espero 5 minutos."
        else -> "Sin conexión."
    }
    Message("reconectando…", "$why (${state.host}) Reintento en ${state.inMs / 1000} s.", session)
}

@Composable
private fun Title(t: String) = Text(t, color = LocalJw.current.blue, fontSize = 22.sp, fontWeight = FontWeight.Bold)

@Composable
private fun Dim(t: String) = Text(t, color = LocalJw.current.dim, textAlign = TextAlign.Start)

@Composable
private fun Warn(t: String) = Text(t, color = LocalJw.current.yellow)
