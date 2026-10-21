package dev.jw.next.app

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import dev.jw.next.client.Need
import dev.jw.next.client.accentOf
import dev.jw.next.contract.Accent
import dev.jw.next.client.Theme
import dev.jw.next.client.Themes
import dev.jw.next.client.Tone

/**
 * The colours the screens draw with: the theme in force (REQ-157). Every colour is read from a
 * state, so a screen that draws with one redraws when the theme changes.
 */
object Palette {
    private var theme by mutableStateOf(Themes.get(null, dark = true))

    /** Draw with [t] from now on. */
    fun use(t: Theme) {
        theme = t
    }

    val bg get() = Color(theme.bg)
    val panel get() = Color(theme.panel)
    val fg get() = Color(theme.fg)
    val dim get() = Color(theme.dim)
    val line get() = Color(theme.line)
    val sel get() = Color(theme.sel)
    val blue get() = Color(theme.blue)
    val green get() = Color(theme.green)
    val yellow get() = Color(theme.yellow)
    val red get() = Color(theme.red)
    val magenta get() = Color(theme.magenta)
    val cyan get() = Color(theme.cyan)

    /** Whether the theme in force is a dark one. */
    val dark get() = theme.dark

    /** A conversation's accent: one of the theme's six (REQ-452). */
    fun accent(a: Accent) = Color(theme.accentOf(a))
}

/** JetBrains Mono: one real monospace face, so marks, names and screens line up on every phone. */
val MonoFont = FontFamily(
    Font(R.font.jetbrains_mono_regular, FontWeight.Normal),
    Font(R.font.jetbrains_mono_bold, FontWeight.Bold),
)

fun Need.color(): Color = when (this) {
    Need.Decide -> Palette.yellow
    Need.Failed -> Palette.red
    Need.Review -> Palette.cyan
}

fun Tone.color(): Color = when (this) {
    Tone.Danger -> Palette.red
    Tone.Warn -> Palette.yellow
    Tone.Plan -> Palette.magenta
    Tone.Good -> Palette.green
    Tone.Dim -> Palette.dim
    Tone.Merged -> Palette.cyan
}
