package dev.jw.term

enum class Key { Esc, Tab, Enter, Backspace, Up, Down, Left, Right, Home, End, PageUp, PageDown, Delete, Insert }

/** What a key, a typed text or a paste sends to the program in a pane (xterm's encoding, as `tui/src/tui/keys.rs` does for the TUI). */
object Keys {
    private const val ESC = 0x1b

    fun key(k: Key, appCursor: Boolean = false, ctrl: Boolean = false, alt: Boolean = false, shift: Boolean = false): ByteArray {
        val mod = 1 + (if (shift) 1 else 0) + (if (alt) 2 else 0) + (if (ctrl) 4 else 0)
        fun csi(final: Char) = if (mod == 1) esc("[$final") else esc("[1;$mod$final")
        fun ss3(final: Char) = if (mod == 1) esc("O$final") else esc("[1;$mod$final")
        fun tilde(n: Int) = if (mod == 1) esc("[$n~") else esc("[$n;$mod~")
        return when (k) {
            Key.Esc -> byteArrayOf(ESC.toByte())
            Key.Tab -> if (shift) esc("[Z") else if (alt) byteArrayOf(ESC.toByte(), 9) else byteArrayOf(9)
            Key.Enter -> if (alt) byteArrayOf(ESC.toByte(), 13) else byteArrayOf(13)
            Key.Backspace -> {
                val b = (if (ctrl) 0x08 else 0x7f).toByte()
                if (alt) byteArrayOf(ESC.toByte(), b) else byteArrayOf(b)
            }
            Key.Up -> if (appCursor) ss3('A') else csi('A')
            Key.Down -> if (appCursor) ss3('B') else csi('B')
            Key.Right -> if (appCursor) ss3('C') else csi('C')
            Key.Left -> if (appCursor) ss3('D') else csi('D')
            Key.Home -> if (appCursor) ss3('H') else csi('H')
            Key.End -> if (appCursor) ss3('F') else csi('F')
            Key.Insert -> tilde(2)
            Key.Delete -> tilde(3)
            Key.PageUp -> tilde(5)
            Key.PageDown -> tilde(6)
        }
    }

    /** `^C` and its kin: the byte a Ctrl-chord of `c` sends, or null when it has none. */
    fun ctrl(c: Char): ByteArray? = when (c) {
        in 'a'..'z' -> byteArrayOf((c - 'a' + 1).toByte())
        in 'A'..'Z' -> byteArrayOf((c - 'A' + 1).toByte())
        '@', ' ' -> byteArrayOf(0)
        '[' -> byteArrayOf(27)
        '\\' -> byteArrayOf(28)
        ']' -> byteArrayOf(29)
        '^' -> byteArrayOf(30)
        '_' -> byteArrayOf(31)
        '?' -> byteArrayOf(127)
        else -> null
    }

    /** Typed text, as UTF-8; with Alt each character goes after an Esc, with Ctrl a letter becomes its control byte. */
    fun text(s: String, ctrl: Boolean = false, alt: Boolean = false): ByteArray {
        if (!ctrl && !alt) return s.toByteArray(Charsets.UTF_8)
        val out = java.io.ByteArrayOutputStream()
        var i = 0
        while (i < s.length) {
            val cp = s.codePointAt(i)
            i += Character.charCount(cp)
            if (alt) out.write(ESC)
            val c = ctrl(cp.toChar()).takeIf { ctrl && cp < 0x80 }
            out.write(c ?: String(Character.toChars(cp)).toByteArray(Charsets.UTF_8))
        }
        return out.toByteArray()
    }

    /** A paste: inside the bracketed-paste markers when the program asked for them (without any end marker the text itself carries), else with newlines as Enter. */
    fun paste(s: String, bracketed: Boolean): ByteArray {
        if (!bracketed) return s.replace("\r\n", "\r").replace('\n', '\r').toByteArray(Charsets.UTF_8)
        val body = s.replace("\u001b[201~", "").replace("\u001b[200~", "")
        return esc("[200~").plus(body.toByteArray(Charsets.UTF_8)).plus(esc("[201~"))
    }

    private fun esc(rest: String) = byteArrayOf(ESC.toByte()) + rest.toByteArray(Charsets.US_ASCII)
}
