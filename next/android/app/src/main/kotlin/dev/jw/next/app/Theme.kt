package dev.jw.next.app

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import dev.jw.next.client.Tone

/** The One Dark palette of the mobile prototype (`docs/prototypes/mobile.html`). */
object Palette {
    val bg = Color(0xFF282C34)
    val panel = Color(0xFF21252B)
    val fg = Color(0xFFABB2BF)
    val dim = Color(0xFF5C6370)
    val line = Color(0xFF3E4451)
    val sel = Color(0xFF2C313A)
    val blue = Color(0xFF61AFEF)
    val green = Color(0xFF98C379)
    val yellow = Color(0xFFE5C07B)
    val red = Color(0xFFE06C75)
    val magenta = Color(0xFFC678DD)
    val cyan = Color(0xFF56B6C2)
}

/** JetBrains Mono: one real monospace face, so marks, names and screens line up on every phone. */
val MonoFont = FontFamily(
    Font(R.font.jetbrains_mono_regular, FontWeight.Normal),
    Font(R.font.jetbrains_mono_bold, FontWeight.Bold),
)

fun Tone.color(): Color = when (this) {
    Tone.Danger -> Palette.red
    Tone.Warn -> Palette.yellow
    Tone.Plan -> Palette.magenta
    Tone.Good -> Palette.green
    Tone.Dim -> Palette.dim
    Tone.Merged -> Palette.cyan
}
