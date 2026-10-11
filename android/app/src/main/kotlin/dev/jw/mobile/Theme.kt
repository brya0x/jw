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
import androidx.compose.ui.text.font.FontFamily
import dev.jw.proto.Palette
import dev.jw.proto.Palettes

/** jw's palette roles, as the prototype names them. */
class JwColors(
    val bg: Color, val panel: Color, val fg: Color, val dim: Color, val line: Color, val sel: Color,
    val blue: Color, val green: Color, val yellow: Color, val red: Color, val magenta: Color, val cyan: Color,
)

val LocalJw = staticCompositionLocalOf { toColors(Palettes.FALLBACK.dark, true) }

private fun toColors(p: Palette, dark: Boolean): JwColors {
    val fb = if (dark) Palettes.FALLBACK.dark else Palettes.FALLBACK.light
    fun c(r: String) = Color(android.graphics.Color.parseColor(p[r] ?: fb[r]!!))
    return JwColors(
        c("bg"), c("panel"), c("fg"), c("dim"), c("line"), c("sel"),
        c("blue"), c("green"), c("yellow"), c("red"), c("magenta"), c("cyan"),
    )
}

/** Dark or light follows the system; the colours come from `ready.palettes` or the One Dark/Light fallback. */
@Composable
fun JwTheme(palettes: Palettes, content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val j = toColors(if (dark) palettes.dark else palettes.light, dark)
    val scheme = (if (dark) darkColorScheme() else lightColorScheme()).copy(
        background = j.bg, surface = j.panel, onBackground = j.fg, onSurface = j.fg,
        primary = j.blue, onPrimary = j.bg, error = j.red, outline = j.line,
        surfaceVariant = j.sel, onSurfaceVariant = j.dim,
    )
    val mono = TextStyle(fontFamily = FontFamily.Monospace)
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
