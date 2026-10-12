package dev.jw.term

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeysTest {
    private fun b(s: String) = s.toByteArray(Charsets.ISO_8859_1)
    private val e = "\u001b"

    @Test fun plainKeys() {
        assertArrayEquals(b(e), Keys.key(Key.Esc))
        assertArrayEquals(b("\t"), Keys.key(Key.Tab))
        assertArrayEquals(b("$e[Z"), Keys.key(Key.Tab, shift = true))
        assertArrayEquals(b("\r"), Keys.key(Key.Enter))
        assertArrayEquals(b("\u007f"), Keys.key(Key.Backspace))
        assertArrayEquals(b("$e[3~"), Keys.key(Key.Delete))
        assertArrayEquals(b("$e[5~"), Keys.key(Key.PageUp))
    }

    @Test fun arrowsFollowApplicationCursorMode() {
        assertArrayEquals(b("$e[A"), Keys.key(Key.Up))
        assertArrayEquals(b("${e}OA"), Keys.key(Key.Up, appCursor = true))
        assertArrayEquals(b("$e[1;5C"), Keys.key(Key.Right, ctrl = true))
        assertArrayEquals(b("$e[1;5C"), Keys.key(Key.Right, appCursor = true, ctrl = true))
        assertArrayEquals(b("$e[1;3D"), Keys.key(Key.Left, alt = true))
        assertArrayEquals(b("$e[3;5~"), Keys.key(Key.Delete, ctrl = true))
    }

    @Test fun ctrlChords() {
        assertArrayEquals(byteArrayOf(3), Keys.ctrl('c'))
        assertArrayEquals(byteArrayOf(4), Keys.ctrl('D'))
        assertArrayEquals(byteArrayOf(18), Keys.ctrl('r'))
        assertArrayEquals(byteArrayOf(27), Keys.ctrl('['))
        assertNull(Keys.ctrl('1'))
        assertArrayEquals(byteArrayOf(3), Keys.text("c", ctrl = true))
        assertArrayEquals(b("${e}x"), Keys.text("x", alt = true))
    }

    @Test fun textIsUtf8() {
        assertArrayEquals("é中".toByteArray(), Keys.text("é中"))
    }

    @Test fun pasteIsBracketedWhenAskedAndCannotCloseItself() {
        assertArrayEquals(b("$e[200~a\nb$e[201~"), Keys.paste("a\nb", true))
        assertArrayEquals(b("$e[200~ab$e[201~"), Keys.paste("a$e[201~b", true))
        assertArrayEquals(b("a\rb\rc"), Keys.paste("a\nb\r\nc", false))
    }
}
