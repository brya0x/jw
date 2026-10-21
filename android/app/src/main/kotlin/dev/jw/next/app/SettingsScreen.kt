package dev.jw.next.app

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.jw.next.client.ClientState
import dev.jw.next.client.Look
import dev.jw.next.client.LookMode
import dev.jw.next.client.Navigator
import dev.jw.next.client.Theme

/**
 * The mode and the theme (REQ-157, REQ-158), as the `Ajustes` screen of the mobile prototype.
 * Only the themes of the kind in force are listed, and each kind keeps its own choice. It is
 * the phone's own: nothing here reaches the PC.
 */
@Composable
fun SettingsScreen(state: ClientState, nav: Navigator, look: Look, osDark: Boolean, pc: PcEntry?, change: (Look) -> Unit, set: (ClientState) -> Unit) {
    val dark = look.isDark(osDark)
    val now = look.theme(osDark)
    Column(Modifier.fillMaxSize()) {
        Header("ajustes", if (pc != null) "PCs y tema" else "solo el tema", onBack = { set(nav.back(state)) })
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            if (pc != null) {
                Section("pc")
                PcRow(pc)
                Mono("Aquí cambias de PC o emparejas otro. El PC al que estás conectado es el verde.", Palette.dim, 12, lines = 3, modifier = Modifier.padding(16.dp))
                Hairline()
            }
            Section("modo")
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).border(BorderStroke(1.dp, Palette.line), RoundedCornerShape(8.dp))) {
                LookMode.entries.forEach { m ->
                    val on = look.mode == m
                    Column(Modifier.weight(1f).clickable { change(look.copy(mode = m)) }, horizontalAlignment = Alignment.CenterHorizontally) {
                        Box(Modifier.height(44.dp), contentAlignment = Alignment.Center) { Mono(m.label, if (on) Palette.blue else Palette.dim, 13, bold = on) }
                        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) Palette.blue else Color.Transparent))
                    }
                }
            }
            Mono(
                if (look.mode == LookMode.System) "Sigue el modo oscuro o claro de Android: ahora usa el ${if (dark) "oscuro" else "claro"}." else "Fijo en ${look.mode.label}, diga lo que diga Android.",
                Palette.dim, 12, lines = 3, modifier = Modifier.padding(16.dp),
            )
            Hairline()
            Mono(
                "Cada modo recuerda su tema. Lo eliges aquí, no en el PC: pinta la app, los colores del terminal y la salida del agente. El terminal del PC tiene los mismos diez temas (cinco oscuros y cinco claros) y su propio ajuste.",
                Palette.dim, 12, lines = 6, modifier = Modifier.padding(16.dp),
            )
            Section("temas ${if (dark) "oscuros" else "claros"}")
            look.shown(osDark).chunked(2).forEach { pair ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 5.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    pair.forEach { t -> ThemeCard(t, t.id == now.id, Modifier.weight(1f)) { change(look.choose(osDark, t.id)) } }
                    if (pair.size == 1) Box(Modifier.weight(1f))
                }
            }
            Hairline()
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Mono("así se ve el panel con «${now.name}»", Palette.fg, 13, bold = true, lines = 2)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Mono("~/ws/jw", Palette.dim, 12)
                    Mono("✓ 42 tests", Palette.green, 12)
                    Mono("✗ 1 falló", Palette.red, 12)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Mono("⚠ aviso", Palette.yellow, 12)
                    Mono("info", Palette.blue, 12)
                    Mono("claude", Palette.magenta, 12)
                    Mono("cyan", Palette.cyan, 12)
                }
            }
        }
    }
}

/** The PC this phone is connected to, with how many it has paired, and what opens the list of them (REQ-217). */
data class PcEntry(val name: String, val status: String, val paired: Int, val open: () -> Unit)

@Composable
private fun PcRow(pc: PcEntry) {
    Row(Modifier.fillMaxWidth().clickable(onClick = pc.open).padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(8.dp).background(Palette.green, CircleShape))
        Column(Modifier.weight(1f).padding(start = 12.dp)) {
            Mono(pc.name, bold = true, size = 14)
            Mono(pc.status, Palette.dim, 11, lines = 2)
        }
        Mono("mis PCs · ${pc.paired} ›", Palette.cyan, 11)
    }
    Hairline()
}

/** A theme drawn in its own colours, so choosing it is seeing it; the chosen one is outlined in blue. */
@Composable
private fun ThemeCard(t: Theme, on: Boolean, modifier: Modifier, pick: () -> Unit) {
    val ground = Color(t.bg)
    Column(
        modifier.background(ground, RoundedCornerShape(8.dp))
            .border(BorderStroke(if (on) 2.dp else 1.dp, if (on) Palette.blue else Palette.line), RoundedCornerShape(8.dp))
            .clickable(onClick = pick).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(5.dp)) {
            listOf(t.blue, t.green, t.red, t.yellow).forEach { Box(Modifier.size(10.dp).background(Color(it), CircleShape)) }
        }
        Mono(t.name, Color(t.fg), 13, bold = true, lines = 2)
    }
}
