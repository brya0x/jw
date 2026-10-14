package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.LinkFail

private val Gutter = 16.dp

@Composable
internal fun Button(text: String, fill: Boolean, click: () -> Unit) {
    Box(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 52.dp)
            .background(if (fill) Palette.blue else Palette.sel, RoundedCornerShape(12.dp))
            .clickable(onClick = click),
        contentAlignment = Alignment.Center,
    ) { Mono(text, if (fill) Palette.bg else Palette.fg, 14, bold = true) }
}

@Composable
internal fun Hero(title: String, body: String, content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = Gutter + 8.dp, vertical = 32.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Mono(title, Palette.fg, 20, bold = true, lines = 3)
        Mono(body, Palette.dim, 13, lines = 12)
        Spacer(Modifier.height(6.dp))
        content()
    }
}

/** First screen of a phone with no PC: pair, or look at the example data. */
@Composable
fun WelcomeScreen(scan: () -> Unit, paste: () -> Unit, example: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        Header("jwn", "sin PC emparejado")
        Box(Modifier.weight(1f)) {
            Hero("Empareja este teléfono", "En tu PC ejecuta jwn pair. Enseña un código QR y un enlace: escanea el código o pega el enlace.") {
                Button("escanear código", true, scan)
                Button("pegar enlace", false, paste)
                Button("ver datos de ejemplo", false, example)
            }
        }
    }
}

/** Where the pairing link is pasted. */
@Composable
fun PasteScreen(error: Boolean, connect: (String) -> Unit, back: () -> Unit) {
    var text by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize().navigationBarsPadding()) {
        Header("pegar enlace", "el que muestra jwn pair", onBack = back)
        Column(Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 12.sp),
                cursorBrush = SolidColor(Palette.blue),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth().height(140.dp).background(Palette.panel, RoundedCornerShape(12.dp))
                    .border(1.dp, if (error) Palette.red else Palette.line, RoundedCornerShape(12.dp)).padding(12.dp),
                decorationBox = { inner ->
                    if (text.isEmpty()) Mono("jwn://…", Palette.dim, 12)
                    inner()
                },
            )
            if (error) Mono("Ese enlace no es de jwn, o apunta a una dirección en la que este teléfono no confía.", Palette.red, 12, lines = 4)
            Button("conectar", text.isNotBlank()) { if (text.isNotBlank()) connect(text) }
        }
    }
}

/** The code the person compares with the one `jwn pair` shows; the PC answers or the time runs out. */
@Composable
fun PairingScreen(ui: LinkUi.Pairing, cancel: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        Header("emparejando", ui.host, onBack = cancel)
        Hero("Compara este código", "Tu PC te pregunta si confía en este teléfono. Compara el código con el que ves en jwn pair y responde y.") {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                Mono(ui.code.chunked(3).joinToString(" "), Palette.yellow, 34, bold = true)
            }
            Mono("esperando tu respuesta en el PC (60 s)", Palette.dim, 12)
            Button("cancelar", false, cancel)
        }
    }
}

/** Why the link did not come up, and what to do. */
@Composable
fun LinkFailScreen(fail: LinkFail, scan: () -> Unit, paste: () -> Unit, back: () -> Unit) {
    val (title, body, label) = when (fail) {
        LinkFail.Expired -> Triple("Ese código ya no sirve", "Cada código vale una vez y dos minutos. En tu PC ejecuta jwn pair otra vez para uno nuevo.", "escanear otro código")
        LinkFail.Declined -> Triple("Tu PC rechazó este teléfono", "Si no fuiste tú quien lo pidió, otra persona pudo ver el código. Ejecuta jwn pair otra vez para uno nuevo.", "escanear otro código")
        LinkFail.Locked -> Triple("Demasiados intentos", "Tu PC bloqueó los intentos desde aquí. Espera un minuto y vuelve a intentarlo.", "volver")
        LinkFail.Revoked -> Triple("Este teléfono ya no está emparejado", "Desde tu PC se quitó este teléfono. Empareja de nuevo con jwn pair.", "empezar de nuevo")
        LinkFail.Unreachable -> Triple("No encuentro tu PC", "Cada vez que se reinicia el enlace, la dirección del túnel cambia. Comprueba que el PC esté encendido y vuelve a pegar un enlace nuevo de jwn pair.", "escanear otro código")
    }
    Column(Modifier.fillMaxSize()) {
        Header("sin conexión", "jwn pair", onBack = back)
        Hero(title, body) {
            val retry = fail != LinkFail.Locked
            Button(label, true, if (retry) scan else back)
            if (retry) Button("pegar enlace", false, paste)
        }
    }
}

/** The last PC does not answer: say so, keep trying, and let the person pick another (REQ-90). */
@Composable
fun OfflineScreen(name: String?, retry: () -> Unit, pcs: () -> Unit) {
    val pc = name ?: "tu PC"
    Column(Modifier.fillMaxSize()) {
        Header("jwn", "sin respuesta de $pc", action = { PcButton(pcs) })
        Box(Modifier.weight(1f)) {
            Hero("No encuentro «$pc»", "Está apagado o fuera de tu red. Sigo intentando cada pocos segundos; en cuanto conteste, entras solo.") {
                Button("reintentar ahora", true, retry)
                Button("ver mis PCs", false, pcs)
            }
        }
    }
}
