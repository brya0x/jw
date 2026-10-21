package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.jw.next.client.Pairing
import dev.jw.next.client.PcState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** How often the PCs are looked at while the list is open (REQ-87). */
private const val LOOK_EVERY_MS = 10_000L

private fun Pairing.via(): String = lan?.let { "wifi · $it" } ?: tunnel?.let { "túnel · $it" } ?: "sin dirección"

/**
 * The PCs this phone is paired with: the ones that answer are green and can be connected to.
 * [current] is the PC the app is connected to now (it is not looked at again); [busy] is the one
 * being connected to.
 */
@Composable
fun PcsScreen(
    store: LinkStore,
    current: String?,
    busy: String?,
    connect: (Pairing) -> Unit,
    open: () -> Unit,
    forget: (Pairing) -> Unit,
    add: () -> Unit,
    back: (() -> Unit)?,
) {
    var pcs by remember { mutableStateOf(store.load().pcs) }
    val states = remember { mutableStateMapOf<String, PcState>() }
    var forgetting by remember { mutableStateOf<Pairing?>(null) }
    val scope = rememberCoroutineScope()

    fun look(p: Pairing) = scope.launch {
        val seen = withContext(Dispatchers.IO) { lookAt(p) }
        states[p.device] = seen.state
        seen.host?.takeIf { it != p.name }?.let { host -> store.update { it.update(p.device) { old -> old.copy(name = host) } } }
    }

    LaunchedEffect(Unit) {
        while (true) {
            pcs = store.load().pcs
            pcs.filter { it.device != current }.forEach { p ->
                states.putIfAbsent(p.device, PcState.Checking)
                look(p)
            }
            delay(LOOK_EVERY_MS)
        }
    }

    val online = pcs.count { it.device == current || states[it.device] == PcState.Online }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            Header("mis PCs", if (online > 0) "$online ${if (online == 1) "encendido" else "encendidos"} · ${pcs.size} emparejados" else "ninguno encendido", onBack = back)
            LazyColumn(Modifier.weight(1f)) {
                items(pcs, key = { it.device }) { p ->
                    val state = if (p.device == current) null else states[p.device] ?: PcState.Checking
                    PcRow(p, state, p.device == current, busy, connect = { connect(p) }, open = open, recheck = {
                        states[p.device] = PcState.Checking
                        look(p)
                    }, more = { forgetting = p }, forget = { forget(p) })
                }
            }
            Column(Modifier.fillMaxWidth().background(Palette.panel).navigationBarsPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(
                    Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).background(Palette.sel, RoundedCornerShape(8.dp)).clickable(onClick = add),
                    contentAlignment = Alignment.Center,
                ) { Mono("+ emparejar otro PC", Palette.fg, 13, bold = true) }
                Mono("Se comprueban cada 10 s mientras esta pantalla está abierta.", Palette.dim, 11, lines = 2)
            }
        }
        forgetting?.let { p ->
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.55f)).clickable { forgetting = null })
            Column(
                Modifier.align(Alignment.BottomCenter).fillMaxWidth().background(Palette.panel, RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp))
                    .navigationBarsPadding().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Mono("olvidar «${p.name ?: p.via()}»", Palette.fg, 14, bold = true, lines = 2)
                Mono("Este teléfono dejará de poder entrar a ese PC. Para volver tendrás que emparejar de nuevo con jwn pair.", Palette.dim, 12, lines = 4)
                Mono(
                    "olvidar este PC", Palette.red, 14, bold = true,
                    modifier = Modifier.fillMaxWidth().clickable { forgetting = null; forget(p) }.padding(vertical = 12.dp),
                )
                Mono("cancelar", Palette.fg, 14, modifier = Modifier.fillMaxWidth().clickable { forgetting = null }.padding(vertical = 12.dp))
            }
        }
    }
}

@Composable
private fun PcRow(
    p: Pairing,
    state: PcState?,
    isCurrent: Boolean,
    busy: String?,
    connect: () -> Unit,
    open: () -> Unit,
    recheck: () -> Unit,
    more: () -> Unit,
    forget: () -> Unit,
) {
    val connecting = busy == p.device
    val (dot, text, label) = when {
        connecting -> Triple(Palette.yellow, "conectando…" to Palette.yellow, "conectando…")
        isCurrent -> Triple(Palette.green, "conectado · ahora" to Palette.green, "abrir")
        state == PcState.Online -> Triple(Palette.green, "en línea" to Palette.green, "conectar")
        state == PcState.Checking || state == null -> Triple(Palette.yellow, "comprobando…" to Palette.yellow, "comprobando…")
        state == PcState.Revoked -> Triple(Palette.red, "ya no está emparejado" to Palette.red, "olvidar")
        else -> Triple(Palette.dim, "apagado" to Palette.dim, "reintentar")
    }
    val primary = !connecting && !isCurrent && state == PcState.Online
    val enabled = busy == null && !connecting && (isCurrent || (state != null && state != PcState.Checking))
    Column {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, top = 12.dp, bottom = 12.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(10.dp).background(dot, CircleShape))
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Mono(p.name ?: "PC sin nombre", Palette.fg, 14, bold = true)
                Mono(p.via(), Palette.dim, 11)
                Mono(text.first, text.second, 11, modifier = Modifier.padding(top = 2.dp))
            }
            Box(
                Modifier.defaultMinSize(minWidth = 92.dp, minHeight = 40.dp)
                    .background(if (primary) Palette.blue else Palette.sel, RoundedCornerShape(6.dp))
                    .clickable(enabled = enabled) { if (isCurrent) open() else if (state == PcState.Revoked) forget() else if (state == PcState.Offline) recheck() else connect() },
                contentAlignment = Alignment.Center,
            ) { Mono(label, if (primary) Palette.bg else if (enabled) Palette.fg else Palette.dim, 12, bold = true, modifier = Modifier.padding(horizontal = 12.dp)) }
            Box(Modifier.size(44.dp).clickable(onClick = more), contentAlignment = Alignment.Center) { Mono("⋯", Palette.dim, 18) }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Palette.line))
    }
}
