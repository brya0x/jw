package dev.jw.mobile

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

// The connection screens of docs/specs/mobile-prototype.html (REQ-51 to REQ-54): connect, camera
// permission, the scan screen, paste link, pairing and the five failure screens. Each takes its
// state and callbacks, so a test can draw it without a camera or a bridge.

/** `.hero`: centred column, 16 dp gap, 24 dp sides. */
@Composable
private fun Hero(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().fillMaxHeight(),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) { Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(16.dp), horizontalAlignment = Alignment.CenterHorizontally) { content() } }
}

@Composable
private fun H2(text: String) = Text(text, color = LocalJw.current.fg, fontSize = 20.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)

/** A `.hero p`: dim, centred, with `code` spans in a chip. */
@Composable
private fun HeroP(text: String) {
    val j = LocalJw.current
    Text(
        code(text, j), color = j.dim, fontSize = 12.5.sp, lineHeight = 18.sp, textAlign = TextAlign.Center,
        modifier = Modifier.widthIn(max = 300.dp),
    )
}

/** Text with `backticked` spans drawn as the prototype's `code`: green on the selection colour. */
private fun code(text: String, j: JwColors): AnnotatedString = buildAnnotatedString {
    text.split('`').forEachIndexed { i, part ->
        if (i % 2 == 1) withStyle(SpanStyle(color = j.green, background = j.sel)) { append(" $part ") } else append(part)
    }
}

@Composable
private fun Col(content: @Composable () -> Unit) = Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }

/** A screen with a back header, a hero and a status bar. */
@Composable
private fun Flow(title: String, sub: String, mode: String, chip: Color, hint: String, onBack: () -> Unit, content: @Composable () -> Unit) {
    val j = LocalJw.current
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header(title, sub, onBack)
        Box(Modifier.weight(1f).fillMaxWidth()) { Hero(content) }
        StatusBar(mode, chip, hint)
    }
}

/** `qrSvg()`: the decorative QR of the connect screen, the same 25 by 25 pattern. */
@Composable
fun DecorativeQr(modifier: Modifier = Modifier) {
    Canvas(modifier.background(Color.White, RoundedCornerShape(10.dp)).padding(14.dp)) {
        val n = 25
        val c = size.width / n
        var seed = 7L
        fun rnd(): Double { seed = seed * 16807 % 2147483647; return seed / 2147483647.0 }
        fun finder(x: Int, y: Int) = (x < 7 && y < 7) || (x >= n - 7 && y < 7) || (x < 7 && y >= n - 7)
        val ink = Color(0xFF111111)
        for (y in 0 until n) for (x in 0 until n) {
            if (finder(x, y)) continue
            if (rnd() > .52) drawRect(ink, Offset(x * c, y * c), Size(c, c))
        }
        fun f(x: Int, y: Int) {
            drawRect(ink, Offset(x * c, y * c), Size(7 * c, 7 * c))
            drawRect(Color.White, Offset((x + 1) * c, (y + 1) * c), Size(5 * c, 5 * c))
            drawRect(ink, Offset((x + 2) * c, (y + 2) * c), Size(3 * c, 3 * c))
        }
        f(0, 0); f(n - 7, 0); f(0, n - 7)
    }
}

/** `vConnect`: the first screen with no stored device. */
@Composable
fun ConnectScreen(onScan: () -> Unit, onPaste: () -> Unit) {
    val j = LocalJw.current
    Box(Modifier.fillMaxSize().background(j.bg)) {
        Hero {
            DecorativeQr(Modifier.size(200.dp))
            H2("conecta tu PC")
            HeroP("En tu PC ejecuta `jw mobile` y escanea el código. El túnel dura mientras el puente esté abierto.")
            JwButton("escanear", fill = true, icon = Ic.Scan, onClick = onScan)
            JwButton("pegar enlace", icon = Ic.Link, onClick = onPaste)
        }
    }
}

@Composable
private fun BigIcon(ic: Ic, bad: Boolean) {
    val j = LocalJw.current
    Box(Modifier.size(84.dp).background(j.sel, RoundedCornerShape(22.dp)), contentAlignment = Alignment.Center) {
        Icon(ic, if (bad) j.red else j.blue, 38.dp)
    }
}

/** `vPerm`: asks for the camera, or says it is blocked. */
@Composable
fun PermissionScreen(blocked: Boolean, onAllow: () -> Unit, onSettings: () -> Unit, onPaste: () -> Unit, onBack: () -> Unit) {
    val j = LocalJw.current
    Flow("escanear código", "jw mobile", "SCAN", j.blue, if (blocked) "cámara bloqueada" else "la cámara pide permiso", onBack) {
        BigIcon(if (blocked) Ic.X else Ic.Cam, blocked)
        H2(if (blocked) "la cámara está bloqueada" else "necesito la cámara")
        HeroP(
            if (blocked) "Actívala en los ajustes del sistema, o pega el enlace que muestra jw."
            else "Solo para leer el QR que muestra jw en tu PC. No guarda fotos ni vídeo.",
        )
        if (blocked) JwButton("abrir ajustes del sistema", fill = true, onClick = onSettings)
        else JwButton("permitir cámara", fill = true, onClick = onAllow)
        JwButton("pegar enlace", icon = Ic.Link, onClick = onPaste)
    }
}

/** The `.ret` reticle: four corners, 3 dp, blue until a code is read, then green. */
@Composable
private fun Reticle(hit: Boolean) {
    val j = LocalJw.current
    val colour = if (hit) j.green else j.blue
    Canvas(Modifier.fillMaxWidth(.66f).widthIn(max = 240.dp).aspectRatio(1f)) {
        val l = 30.dp.toPx()
        val w = 3.dp.toPx()
        val o = 4.dp.toPx()
        val s = Stroke(w)
        fun corner(x: Float, y: Float, dx: Float, dy: Float) {
            drawLine(colour, Offset(x, y), Offset(x + dx * l, y), w)
            drawLine(colour, Offset(x, y), Offset(x, y + dy * l), w)
        }
        corner(-o, -o, 1f, 1f); corner(size.width + o, -o, -1f, 1f)
        corner(-o, size.height + o, 1f, -1f); corner(size.width + o, size.height + o, -1f, -1f)
        if (!hit) {
            drawLine(
                Brush.horizontalGradient(listOf(Color.Transparent, colour, Color.Transparent)),
                Offset(0f, size.height / 2), Offset(size.width, size.height / 2), 2.dp.toPx(),
            )
        }
    }
}

/**
 * `vScan` (camera A): the terminal log over the feed, the reticle, and a bar with `pegar enlace`
 * and a torch (only when the phone has one). `feed` is the camera preview.
 */
@Composable
fun ScanScreenUi(
    hit: Boolean,
    torchAvailable: Boolean,
    torchOn: Boolean,
    onTorch: () -> Unit,
    onPaste: () -> Unit,
    onBack: () -> Unit,
    feed: @Composable BoxScope.() -> Unit,
) {
    val j = LocalJw.current
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header("escanear código", "jw scan", onBack)
        Box(Modifier.weight(1f).fillMaxWidth().background(Color(0xFF0B0C10)), contentAlignment = Alignment.Center) {
            feed()
            val shadow = Shadow(Color.Black, Offset(0f, 2f), 3f)
            Column(Modifier.align(Alignment.TopStart).padding(start = 14.dp, end = 14.dp, top = 12.dp)) {
                val t = TextStyle(fontFamily = JwMono, fontSize = 11.5.sp, lineHeight = 19.5.sp, shadow = shadow)
                Text(buildAnnotatedString {
                    withStyle(SpanStyle(color = j.green)) { append("$") }
                    append(" jw scan")
                }, color = Color(0xFF9AA4B2), style = t)
                Text("· cámara lista", color = Color(0xFF7F8A99), style = t)
                Text("· el QR tarda ~1 min en aparecer en jw", color = Color(0xFF7F8A99), style = t)
                if (hit) Text("✓ QR leído · conectando", color = j.green, style = t)
                else Text("· buscando un QR de jw▮", color = Color(0xFF7F8A99), style = t)
            }
            Reticle(hit)
        }
        Row(
            Modifier.fillMaxWidth().background(j.panel).padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(Modifier.weight(1f)) { JwButton("pegar enlace", icon = Ic.Link, onClick = onPaste) }
            if (torchAvailable) {
                Box(
                    Modifier.size(52.dp, 48.dp).border(BorderStroke(1.dp, if (torchOn) j.yellow else j.line), RoundedCornerShape(6.dp))
                        .clickable(role = Role.Button, onClickLabel = "Linterna", onClick = onTorch),
                    contentAlignment = Alignment.Center,
                ) { Icon(Ic.Zap, if (torchOn) j.yellow else j.fg, 18.dp) }
            }
        }
        StatusBar("SCAN", j.blue, if (hit) "QR leído" else "buscando un código…")
    }
}

/** The text refused when a pasted link is not a jw quick tunnel link. */
const val LINK_REFUSED = "ese enlace no es de un túnel de jw"

/**
 * `sheet` of `paste`: a text field, never the clipboard read by itself. `error` is the red line.
 * What is typed or pasted is not logged or kept anywhere but this field.
 */
@Composable
fun PasteSheet(error: String?, onConnect: (String) -> Unit, onClose: () -> Unit) {
    val j = LocalJw.current
    var text by remember { mutableStateOf("") }
    Box(Modifier.fillMaxSize()) {
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = .45f)).clickable(onClick = onClose))
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().background(j.panel, RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp))
                .drawLineTop(j.line).padding(bottom = 8.dp),
        ) {
            Box(Modifier.padding(top = 8.dp, bottom = 4.dp).size(36.dp, 4.dp).background(j.line, RoundedCornerShape(2.dp)).align(Alignment.CenterHorizontally))
            Text("pegar enlace", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 8.dp))
            BasicTextField(
                value = text, onValueChange = { text = it }, singleLine = true,
                textStyle = TextStyle(color = j.fg, fontSize = 13.5.sp, fontFamily = JwMono),
                cursorBrush = SolidColor(j.fg),
                modifier = Modifier.padding(horizontal = 12.dp).fillMaxWidth().heightIn(min = 44.dp).background(j.bg, RoundedCornerShape(6.dp))
                    .border(BorderStroke(1.dp, j.blue), RoundedCornerShape(6.dp)),
                decorationBox = { inner ->
                    Box(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) {
                        if (text.isEmpty()) Text("jw://….trycloudflare.com/…", color = j.dim, fontSize = 13.5.sp)
                        inner()
                    }
                },
            )
            Text(error ?: "", color = j.red, fontSize = 12.sp, modifier = Modifier.heightIn(min = 26.dp).padding(start = 14.dp, end = 14.dp, top = 6.dp))
            Row(
                Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) { onConnect(text) }.padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    "↵", color = j.yellow, fontSize = 12.sp, textAlign = TextAlign.Center,
                    modifier = Modifier.widthIn(min = 34.dp).background(j.bg, RoundedCornerShape(4.dp)).border(BorderStroke(1.dp, j.line), RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 3.dp),
                )
                Text("conectar", color = j.fg, modifier = Modifier.weight(1f))
                Text("solo túneles de jw", color = j.dim, fontSize = 11.sp)
            }
        }
    }
}

private fun Modifier.drawLineTop(c: Color): Modifier = this.then(
    Modifier.border(BorderStroke(1.dp, c), RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp)),
)

/** `vPair`: the code to compare, what has happened so far, and cancel. */
@Composable
fun PairScreen(host: String, code: String, reached: Boolean, via: Route?, onCancel: () -> Unit) {
    val j = LocalJw.current
    var frame by remember { mutableIntStateOf(0) }
    LaunchedEffect(Unit) { while (true) { delay(110); frame = (frame + 1) % SPIN.length } }
    Flow("emparejando", host, "PAIR", j.yellow, "compara el código en jw", onCancel) {
        HeroP("Tu PC te pregunta si confía en este teléfono. Compara este código con el que ves en jw y responde **y**.".replace("**y**", "y"))
        Text(
            code.chunked(3).joinToString(" "), color = j.yellow, fontSize = 46.sp, fontWeight = FontWeight.Bold,
            letterSpacing = 4.sp, modifier = Modifier.padding(vertical = 2.dp),
        )
        Column(Modifier.widthIn(max = 300.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Step("●", j.green, "QR leído")
            if (reached) Step("●", j.green, "PC alcanzado" + when (via) { Route.Wifi -> " · por wifi"; Route.Tunnel -> " · por el túnel"; null -> "" })
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(SPIN[frame].toString(), color = j.cyan, fontSize = 13.sp)
                Text("esperando tu respuesta", color = j.fg, fontSize = 13.sp)
            }
        }
        JwButton("cancelar", onClick = onCancel)
    }
}

private const val SPIN = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

@Composable
private fun Step(mark: String, colour: Color, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(mark, color = colour, fontSize = 13.sp)
        Text(text, color = LocalJw.current.fg, fontSize = 13.sp)
    }
}

/** The five failure screens. Their causes map to these in `Session` (see `SPEC-NOTES.md`). */
@Composable
fun FailScreen(why: ScanWhy, onScanAnother: () -> Unit, onBack: () -> Unit) {
    val j = LocalJw.current
    val (title, text, button) = when (why) {
        ScanWhy.DENIED -> Triple(
            "Tu PC rechazó este teléfono",
            "Si no fuiste tú quien escaneó, otra persona pudo ver el código del QR. En `jw mobile` pulsa Enter para generar uno nuevo.",
            "escanear otro código",
        )
        ScanWhy.EXPIRED -> Triple(
            "Ese código ya no sirve",
            "Cada código vale una vez y dos minutos. En `jw mobile` pulsa Enter para uno nuevo.",
            "escanear otro código",
        )
        ScanWhy.LIMITED -> Triple(
            "Demasiados intentos",
            "Tu PC bloqueó los intentos desde aquí. Espera 5 minutos y vuelve a escanear.",
            "volver",
        )
        ScanWhy.UNREACHABLE -> error("UnreachableScreen draws this one")
    }
    Flow("sin conexión", "jw mobile", "ERR", j.red, "no conectado", onBack) {
        BigIcon(Ic.X, true)
        H2(title)
        HeroP(text)
        JwButton(button, fill = true, onClick = if (why == ScanWhy.LIMITED) onBack else onScanAnother)
    }
}

/**
 * "No encuentro tu PC": a log of what was measured (REQ-54). A row the app did not measure is not drawn;
 * the PC's row is the one that failed.
 */
@Composable
fun UnreachableScreen(host: String, reach: Reach, onScanAnother: () -> Unit, onRetry: (() -> Unit)?, onBack: () -> Unit) {
    val j = LocalJw.current
    Flow("sin conexión", "jw mobile", "ERR", j.red, "no conectado", onBack) {
        H2("No encuentro tu PC")
        Column(
            Modifier.fillMaxWidth().widthIn(max = 330.dp).background(j.panel, RoundedCornerShape(12.dp))
                .border(BorderStroke(1.dp, j.line), RoundedCornerShape(12.dp)).padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            val t = TextStyle(fontFamily = JwMono, fontSize = 12.5.sp, lineHeight = 23.sp)
            // The wifi row is there only when the app held an address and tried it (REQ-64).
            if (reach.wifi == false) {
                Text(buildAnnotatedString {
                    withStyle(SpanStyle(color = j.red)) { append("✗") }
                    append(" wifi")
                    withStyle(SpanStyle(color = j.dim)) { append(" · sin respuesta") }
                }, color = j.fg, style = t)
            }
            reach.internet?.let { net ->
                Text(buildAnnotatedString {
                    withStyle(SpanStyle(color = if (net) j.green else j.red)) { append(if (net) "✓" else "✗") }
                    append(if (net) " internet" else " sin internet")
                    if (net && reach.mobileData != null) withStyle(SpanStyle(color = j.dim)) { append(if (reach.mobileData) " · datos móviles" else " · wifi") }
                }, color = j.fg, style = t)
            }
            reach.cloudflare?.let {
                Text(buildAnnotatedString {
                    withStyle(SpanStyle(color = j.green)) { append("✓") }
                    append(" Cloudflare responde")
                }, color = j.fg, style = t)
            }
            Text(buildAnnotatedString {
                withStyle(SpanStyle(color = j.red)) { append("✗") }
                append(" tu PC")
                withStyle(SpanStyle(color = j.dim)) { append(" · sin respuesta") }
            }, color = j.fg, style = t)
            Text(host, color = j.dim, fontSize = 11.sp, lineHeight = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 18.dp, top = 2.dp))
        }
        HeroP("Cada vez que se reinicia `jw mobile` la dirección de tu PC cambia. Escanea el código nuevo.")
        JwButton("escanear otro código", fill = true, onClick = onScanAnother)
        if (onRetry != null) JwButton("reintentar", onClick = onRetry)
    }
}

/** "La app y jw no hablan igual": says which side is behind, from the two numbers. */
@Composable
fun ProtocolScreen(app: Int, pc: Int, onBack: () -> Unit) {
    val j = LocalJw.current
    val jwBehind = pc < app
    Flow("versiones distintas", "jw mobile", "ERR", j.red, "no conectado", onBack) {
        H2("La app y jw no hablan igual")
        Column(
            Modifier.fillMaxWidth().widthIn(max = 330.dp).background(j.panel, RoundedCornerShape(12.dp))
                .border(BorderStroke(1.dp, j.line), RoundedCornerShape(12.dp)).padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            val t = TextStyle(fontFamily = JwMono, fontSize = 12.5.sp, lineHeight = 22.sp)
            val lo = if (jwBehind) "jw " else "app"
            val hi = if (jwBehind) "app" else "jw "
            Text("-  $lo protocolo ${minOf(app, pc)}", color = j.red, style = t)
            Text("+  $hi protocolo ${maxOf(app, pc)}", color = j.green, style = t)
        }
        HeroP(
            if (jwBehind) "Tu jw está atrasado. Actualízalo con `cargo install --path crates/jw` y reinicia el daemon."
            else "Tu app está atrasada. Instala la última versión.",
        )
        JwButton("entendido", fill = true, onClick = onBack)
    }
}
