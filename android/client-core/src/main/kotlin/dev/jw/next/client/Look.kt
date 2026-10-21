package dev.jw.next.client

/** How the phone chooses what to draw with: a mode, and a theme for each kind (REQ-157, REQ-158). */
enum class LookMode(val id: String, val label: String) {
    System("system", "sistema"),
    Dark("dark", "oscuro"),
    Light("light", "claro"),
    ;

    /** The next mode, or the one before it. */
    fun step(forward: Boolean): LookMode = entries[(ordinal + if (forward) 1 else entries.size - 1) % entries.size]
}

/** One palette, as `0xAARRGGBB`: the twelve colours of `contract/themes.json`. */
data class Theme(
    val id: String,
    val name: String,
    /** A dark theme is offered while the mode is dark, a light one while it is light. */
    val dark: Boolean,
    val bg: Long,
    val panel: Long,
    val fg: Long,
    val dim: Long,
    val line: Long,
    val sel: Long,
    val blue: Long,
    val green: Long,
    val yellow: Long,
    val red: Long,
    val magenta: Long,
    val cyan: Long,
)

/** The ten palettes. `contract/themes.json` is home; a test keeps this table equal to it (REQ-156). */
object Themes {
    const val DEFAULT_DARK = "one-dark"
    const val DEFAULT_LIGHT = "one-light"

    val all: List<Theme> = listOf(
        Theme("one-dark", "One Dark", true, 0xFF282C34L, 0xFF21252BL, 0xFFABB2BFL, 0xFF5C6370L, 0xFF3E4451L, 0xFF2C313AL, 0xFF61AFEFL, 0xFF98C379L, 0xFFE5C07BL, 0xFFE06C75L, 0xFFC678DDL, 0xFF56B6C2L),
        Theme("one-light", "One Light", false, 0xFFFAFAFAL, 0xFFF0F0F1L, 0xFF383A42L, 0xFFA0A1A7L, 0xFFDBDBDCL, 0xFFE5E5E6L, 0xFF4078F2L, 0xFF50A14FL, 0xFFC18401L, 0xFFE45649L, 0xFFA626A4L, 0xFF0184BCL),
        Theme("catppuccin-mocha", "Catppuccin Mocha", true, 0xFF1E1E2EL, 0xFF181825L, 0xFFCDD6F4L, 0xFF6C7086L, 0xFF313244L, 0xFF45475AL, 0xFF89B4FAL, 0xFFA6E3A1L, 0xFFF9E2AFL, 0xFFF38BA8L, 0xFFCBA6F7L, 0xFF94E2D5L),
        Theme("catppuccin-latte", "Catppuccin Latte", false, 0xFFEFF1F5L, 0xFFE6E9EFL, 0xFF4C4F69L, 0xFF9CA0B0L, 0xFFCCD0DAL, 0xFFDCE0E8L, 0xFF1E66F5L, 0xFF40A02BL, 0xFFDF8E1DL, 0xFFD20F39L, 0xFF8839EFL, 0xFF179299L),
        Theme("tokyo-night", "Tokyo Night", true, 0xFF1A1B26L, 0xFF16161EL, 0xFFC0CAF5L, 0xFF565F89L, 0xFF292E42L, 0xFF283457L, 0xFF7AA2F7L, 0xFF9ECE6AL, 0xFFE0AF68L, 0xFFF7768EL, 0xFFBB9AF7L, 0xFF7DCFFFL),
        Theme("tokyo-day", "Tokyo Day", false, 0xFFE1E2E7L, 0xFFD5D6DBL, 0xFF3760BFL, 0xFF848CB5L, 0xFFC4C8DAL, 0xFFD0D5E3L, 0xFF2E7DE9L, 0xFF587539L, 0xFF8C6C3EL, 0xFFF52A65L, 0xFF9854F1L, 0xFF007197L),
        Theme("gruvbox-dark", "Gruvbox Dark", true, 0xFF282828L, 0xFF1D2021L, 0xFFEBDBB2L, 0xFF928374L, 0xFF3C3836L, 0xFF32302FL, 0xFF83A598L, 0xFFB8BB26L, 0xFFFABD2FL, 0xFFFB4934L, 0xFFD3869BL, 0xFF8EC07CL),
        Theme("gruvbox-light", "Gruvbox Light", false, 0xFFFBF1C7L, 0xFFF2E5BCL, 0xFF3C3836L, 0xFF7C6F64L, 0xFFD5C4A1L, 0xFFEBDBB2L, 0xFF076678L, 0xFF79740EL, 0xFFB57614L, 0xFF9D0006L, 0xFF8F3F71L, 0xFF427B58L),
        Theme("solarized-dark", "Solarized Dark", true, 0xFF002B36L, 0xFF00212BL, 0xFF93A1A1L, 0xFF586E75L, 0xFF0F4350L, 0xFF073642L, 0xFF268BD2L, 0xFF859900L, 0xFFB58900L, 0xFFDC322FL, 0xFFD33682L, 0xFF2AA198L),
        Theme("solarized-light", "Solarized Light", false, 0xFFFDF6E3L, 0xFFEEE8D5L, 0xFF586E75L, 0xFF93A1A1L, 0xFFE0DAC3L, 0xFFE6DFC8L, 0xFF268BD2L, 0xFF859900L, 0xFFB58900L, 0xFFDC322FL, 0xFFD33682L, 0xFF2AA198L),
    )

    fun ofKind(dark: Boolean): List<Theme> = all.filter { it.dark == dark }

    /** [id] when it is a theme of this kind, else the default one of the kind. */
    fun get(id: String?, dark: Boolean): Theme {
        val default = if (dark) DEFAULT_DARK else DEFAULT_LIGHT
        return ofKind(dark).firstOrNull { it.id == id } ?: ofKind(dark).first { it.id == default }
    }
}

/**
 * What the person chose: the mode and the theme each kind uses, so that changing the mode
 * brings back the theme last picked for it. Kept on the phone, never sent to the PC.
 */
data class Look(
    val mode: LookMode = LookMode.System,
    val dark: String = Themes.DEFAULT_DARK,
    val light: String = Themes.DEFAULT_LIGHT,
) {
    /** Dark or light now: `system` follows the phone ([osDark]). */
    fun isDark(osDark: Boolean): Boolean = when (mode) {
        LookMode.System -> osDark
        LookMode.Dark -> true
        LookMode.Light -> false
    }

    /** The theme in force. */
    fun theme(osDark: Boolean): Theme {
        val dark = isDark(osDark)
        return Themes.get(if (dark) this.dark else light, dark)
    }

    /** The themes a selector shows: only the kind in force. */
    fun shown(osDark: Boolean): List<Theme> = Themes.ofKind(isDark(osDark))

    /** Use [id] for the kind in force; an id of the other kind changes nothing. */
    fun choose(osDark: Boolean, id: String): Look {
        val dark = isDark(osDark)
        if (Themes.ofKind(dark).none { it.id == id }) return this
        return if (dark) copy(dark = id) else copy(light = id)
    }

    fun step(forward: Boolean): Look = copy(mode = mode.step(forward))

    companion object {
        /** From what was stored: each value that is missing or not understood is the default for it. */
        fun of(mode: String?, dark: String?, light: String?): Look = Look(
            LookMode.entries.firstOrNull { it.id == mode } ?: LookMode.System,
            Themes.get(dark, true).id,
            Themes.get(light, false).id,
        )
    }
}
