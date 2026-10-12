package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-77 to 81. The `phone_*.json` files are what the daemon's own serialiser makes (a Rust test holds them to that). */
class PcFilesTest {
    private fun res(n: String) = checkNotNull(javaClass.getResource("/$n")).readText()

    @Test fun aListingParsesInFull() {
        val l = (Control.parse(res("phone_ls.json")) as ServerMessage.LsAnswer).listing
        assertEquals("~/ws", l.req)
        assertEquals("/Users/me/ws", l.path)
        assertEquals("/Users/me", l.parent)
        assertEquals(listOf("jw", "to-jw", "README.md"), l.entries.map { it.name })
        assertEquals(listOf(EntryKind.Dir, EntryKind.Link, EntryKind.File), l.entries.map { it.kind })
        assertTrue(l.entries[0].isFolder && l.entries[1].isFolder && !l.entries[2].isFolder)
        assertEquals("dir", l.entries[1].target)
        assertEquals(1234L, l.entries[2].size)
        assertTrue(l.entries[2].md)
        assertEquals(3, l.total)
        assertFalse(l.truncated)
        assertFalse(l.toString().contains("README"))
    }

    @Test fun aMarkdownFileComesAsBlocks() {
        val f = (Control.parse(res("phone_read_md.json")) as ServerMessage.ReadAnswer).file
        assertEquals(FileKind.Md, f.kind)
        assertTrue(f.text!!.startsWith("# Title"))
        val b = f.blocks!!
        val h = b[0] as MdBlock.Heading
        assertEquals(1, h.level)
        assertEquals("Title", h.spans.single().t)
        val p = b[1] as MdBlock.Paragraph
        assertTrue(p.spans.any { it.b && it.t == "bold" } && p.spans.any { it.code && it.t == "code" })
        assertEquals("http://x", p.spans.first { it.href != null }.href)
        val ul = b[2] as MdBlock.Ul
        assertEquals(2, ul.items.size)
        assertEquals("one", ((ul.items[0][0] as MdBlock.Paragraph).spans.single().t))
        val code = b[3] as MdBlock.Code
        assertEquals("sh" to "jw mobile", code.lang to code.text)
        val t = b[4] as MdBlock.Table
        assertEquals(2, t.head.size)
        assertEquals("2", t.rows[0][1].single().t)
        assertFalse(f.toString().contains("Title"))
    }

    @Test fun aBinaryFileHasNoText() {
        val f = (Control.parse(res("phone_read_binary.json")) as ServerMessage.ReadAnswer).file
        assertEquals(FileKind.Binary, f.kind)
        assertNull(f.text)
        assertNull(f.blocks)
        assertEquals(18_000L, f.size)
    }

    @Test fun theRequestsAndTheErrorsOfAnAsk() {
        assertEquals("""{"t":"ls","path":"~/ws"}""", Control.ls("~/ws"))
        assertEquals("""{"t":"read","path":"/a/b.md"}""", Control.read("/a/b.md"))
        val e = Control.parse("""{"t":"error","code":"unsupported","msg":"permission denied","ask":"ls","req":"/root"}""") as ServerMessage.Error
        assertEquals(Triple("ls", "/root", "permission denied"), Triple(e.ask, e.req, e.msg))
    }

    @Test fun malformedAnswersAreNotFiles() {
        assertTrue(Control.parse("""{"t":"ls","entries":[]}""") is ServerMessage.Other)
        assertTrue(Control.parse("""{"t":"read","path":"/x","kind":"weird"}""") is ServerMessage.Other)
        // A hostile nesting is cut, not followed to the bottom of the stack.
        val deep = "{\"k\":\"quote\",\"blocks\":[".repeat(500) + "]}".repeat(500)
        val f = (Control.parse("""{"t":"read","path":"/x","kind":"md","size":1,"truncated":false,"text":"x","blocks":[$deep]}""") as ServerMessage.ReadAnswer).file
        assertTrue(f.blocks!!.isNotEmpty())
    }
}
