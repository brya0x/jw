package dev.jw.mobile

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.material3.Typography
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import dev.jw.proto.Palette
import dev.jw.proto.Palettes

/** JetBrains Mono, the prototype's face (OFL, bundled): the system "monospace" is not mono on every ROM. */
val JwMono = FontFamily(Font(R.font.jetbrains_mono_regular, FontWeight.Normal), Font(R.font.jetbrains_mono_bold, FontWeight.Bold))

/** jw's palette roles, as the prototype names them. */
class JwColors(
    val bg: Color, val panel: Color, val fg: Color, val dim: Color, val line: Color, val sel: Color,
    val blue: Color, val green: Color, val yellow: Color, val red: Color, val magenta: Color, val cyan: Color,
    /** The backgrounds of an added and a deleted line (REQ-25). */
    val addBg: Color, val delBg: Color,
)

val LocalJw = staticCompositionLocalOf { toColors(Palettes.FALLBACK.dark, true) }

private fun toColors(p: Palette, dark: Boolean): JwColors {
    val fb = if (dark) Palettes.FALLBACK.dark else Palettes.FALLBACK.light
    fun c(r: String) = Color(android.graphics.Color.parseColor(p[r] ?: fb[r]!!))
    return JwColors(
        c("bg"), c("panel"), c("fg"), c("dim"), c("line"), c("sel"),
        c("blue"), c("green"), c("yellow"), c("red"), c("magenta"), c("cyan"),
        c("add_bg"), c("del_bg"),
    )
}

/**
 * The theme [chosen] on the phone (it paints the app and the panes' colours, and the PC and its agents keep
 * theirs); with none, dark or light follows the system. The colours come from `ready.palettes` or the
 * One Dark/Light fallback.
 */
@Composable
fun JwTheme(palettes: Palettes, chosen: String?, content: @Composable () -> Unit) {
    val (palette, dark) = palettes.resolve(chosen, isSystemInDarkTheme())
    val j = toColors(palette, dark)
    val scheme = (if (dark) darkColorScheme() else lightColorScheme()).copy(
        background = j.bg, surface = j.panel, onBackground = j.fg, onSurface = j.fg,
        primary = j.blue, onPrimary = j.bg, error = j.red, outline = j.line,
        surfaceVariant = j.sel, onSurfaceVariant = j.dim,
    )
    val mono = TextStyle(fontFamily = JwMono)
    CompositionLocalProvider(LocalJw provides j) {
        MaterialTheme(
            colorScheme = scheme,
            typography = Typography(
                bodyLarge = mono, bodyMedium = mono, titleLarge = mono, titleMedium = mono,
                labelLarge = mono, headlineLarge = mono, headlineMedium = mono,
            ),
            content = content,
        )
    }
}
