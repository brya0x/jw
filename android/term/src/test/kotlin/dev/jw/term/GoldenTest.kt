package dev.jw.term

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Every `golden/<name>.<rows>x<cols>.in` fed to the emulator must leave the screen vt100 left in `.out`. */
class GoldenTest {
    private val dir = File(requireNotNull(javaClass.getResource("/golden")) { "no golden dir" }.toURI())

    private fun color(c: Int) = when (Color.kind(c)) {
        0 -> "D"
        1 -> "I${Color.value(c)}"
        else -> "R${Color.value(c) shr 16 and 0xff},${Color.value(c) shr 8 and 0xff},${Color.value(c) and 0xff}"
    }

    private fun Boolean.int() = if (this) 1 else 0

    /** The dump `crates/jw/examples/cells.rs` prints. */
    private fun dump(t: Terminal): String = t.read {
        val sb = StringBuilder()
        sb.append("size ${t.rows} ${t.cols}\n")
        sb.append("cursor ${t.cursorRow} ${t.cursorCol + t.wrapPending.int()} hidden=${(!t.cursorVisible).int()}\n")
        val mode = when (t.mouseMode) {
            MouseMode.None -> "none"
            MouseMode.Press -> "press"
            MouseMode.PressRelease -> "press-release"
            MouseMode.ButtonMotion -> "button-motion"
            MouseMode.AnyMotion -> "any-motion"
        }
        val enc = when (t.mouseEncoding) {
            MouseEncoding.Default -> "default"
            MouseEncoding.Utf8 -> "utf8"
            MouseEncoding.Sgr -> "sgr"
        }
        sb.append("modes alt=${t.altScreen.int()} appcursor=${t.appCursor.int()} appkeypad=${t.appKeypad.int()} paste=${t.bracketedPaste.int()} mouse=$mode/$enc\n")
        val attrs = StringBuilder()
        for (r in 0 until t.rows) {
            val l = t.line(r)
            sb.append("row $r |${l.text()}|\n")
            for (c in 0 until t.cols) {
                val fl = l.flags[c]
                if (fl and Attr.CONT != 0) continue
                val f = buildString {
                    if (fl and Attr.BOLD != 0) append('b')
                    if (fl and Attr.DIM != 0) append('d')
                    if (fl and Attr.ITALIC != 0) append('i')
                    if (fl and Attr.UNDERLINE != 0) append('u')
                    if (fl and Attr.INVERSE != 0) append('v')
                }
                val fg = color(l.fg[c])
                val bg = color(l.bg[c])
                if (f.isEmpty() && fg == "D" && bg == "D") continue
                attrs.append("cell $r $c fg=$fg bg=$bg f=${f.ifEmpty { "-" }}\n")
            }
        }
        sb.append(attrs).toString()
    }

    private fun cases() = dir.listFiles { f -> f.name.endsWith(".in") }!!.sortedBy { it.name }

    private fun size(f: File): Pair<Int, Int> {
        val s = f.name.removeSuffix(".in").substringAfterLast('.')
        return s.substringBefore('x').toInt() to s.substringAfter('x').toInt()
    }

    @Test fun streamsLeaveTheScreenVt100Leaves() {
        val files = cases()
        assertTrue("no golden streams found", files.size >= 10)
        val bad = ArrayList<String>()
        for (f in files) {
            val (rows, cols) = size(f)
            val t = Terminal(cols, rows)
            t.feed(f.readBytes())
            val want = File(f.path.removeSuffix(".in") + ".out").readText()
            val got = dump(t)
            if (got != want) bad += "${f.name}\n${firstDifference(want, got)}"
        }
        assertEquals(bad.joinToString("\n\n"), 0, bad.size)
    }

    /** The same stream a byte at a time leaves the same screen: a sequence or a character may be cut anywhere. */
    @Test fun chunkingDoesNotChangeTheScreen() {
        for (f in cases()) {
            val (rows, cols) = size(f)
            val bytes = f.readBytes()
            val whole = Terminal(cols, rows).also { it.feed(bytes) }
            val one = Terminal(cols, rows).also { t -> for (b in bytes) t.feed(byteArrayOf(b)) }
            assertEquals(f.name, dump(whole), dump(one))
        }
    }

    private fun firstDifference(want: String, got: String): String {
        val a = want.lines()
        val b = got.lines()
        for (i in 0 until maxOf(a.size, b.size)) {
            if (a.getOrNull(i) != b.getOrNull(i)) return "line ${i + 1}: want ${a.getOrNull(i)}\n         got  ${b.getOrNull(i)}"
        }
        return "equal"
    }
}
