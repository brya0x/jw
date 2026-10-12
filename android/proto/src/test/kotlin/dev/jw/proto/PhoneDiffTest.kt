package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-13 and REQ-25. `phone_diff.json` is what the daemon's own serialiser makes (a Rust test holds it to that). */
class PhoneDiffTest {
    private val answer = checkNotNull(javaClass.getResource("/phone_diff.json")).readText()

    private fun parsed() = (Control.parse(answer) as ServerMessage.DiffAnswer).diff

    @Test fun theServersAnswerParsesInFull() {
        val d = parsed()
        assertEquals("w1", d.ws)
        assertEquals("demo vs origin/main", d.title)
        assertTrue(d.truncated)
        assertEquals(listOf("src/a.ts", "new.md", "gone.txt", "img.png", "link", "big.rs"), d.files.map { it.path })

        val a = d.files[0]
        assertEquals(FileStatus.Modified, a.status)
        assertEquals(2 to 1, a.added to a.deleted)
        val h = a.hunks.single()
        assertEquals("export function track()", h.context)
        assertEquals(listOf(LineKind.Ctx, LineKind.Del, LineKind.Add, LineKind.Add), h.lines.map { it.kind })
        assertEquals(listOf(1 to 1, 2 to null, null to 2, null to 3), h.lines.map { it.old to it.new })
        assertEquals("const limit = 20", h.lines[2].text)

        assertEquals(FileStatus.Renamed to "old.md", d.files[1].status to d.files[1].oldPath)
        assertEquals(FileStatus.Deleted, d.files[2].status)
        assertTrue(d.files[3].binary && d.files[3].hunks.isEmpty())
        assertEquals("symlink", d.files[4].unreadable)
        assertTrue(d.files[5].truncated)
        assertEquals(5000, d.files[5].added)
        assertNull(d.files[0].unreadable)
    }

    @Test fun theRequestIsTheDocumentedOne() {
        assertEquals("""{"t":"diff","ws":"w1"}""", Control.diff("w1"))
    }

    @Test fun aFileKeyChangesWithItsContentNotJustItsCounts() {
        val a = parsed().files[0]
        val edited = Diff.parse(
            kotlinx.serialization.json.Json.parseToJsonElement(answer.replace("const limit = 20", "const limit = 99")) as kotlinx.serialization.json.JsonObject,
        )!!.files[0]
        assertEquals(a.added, edited.added)
        assertNotEquals(a.key, edited.key)
        assertEquals(a.key, parsed().files[0].key)
    }

    @Test fun badShapesAreNotDiffsAndUnknownWordsDegrade() {
        assertTrue(Control.parse("""{"t":"diff","files":[]}""") is ServerMessage.Other)
        assertTrue(Control.parse("""{"t":"diff","ws":"w"}""") is ServerMessage.Other)
        val d = (Control.parse(
            """{"t":"diff","ws":"w","files":[{"path":"a","status":"copied","hunks":[{"lines":[{"k":"???","text":"x"}]}]},{"nope":1}]}""",
        ) as ServerMessage.DiffAnswer).diff
        assertEquals(1, d.files.size)
        assertEquals(FileStatus.Modified, d.files[0].status)
        assertEquals(LineKind.Ctx, d.files[0].hunks[0].lines[0].kind)
        assertEquals("", d.title)
        assertFalse(d.truncated)
    }

    @Test fun toStringKeepsTheCodeOut() {
        val d = parsed()
        assertFalse(d.toString().contains("limit"))
        assertFalse(d.files[0].toString().contains("src/a.ts"))
    }

    @Test fun theFourDiffColoursAreRoles() {
        for (r in listOf("add_bg", "del_bg", "add_word", "del_word")) {
            assertTrue(r in Palettes.ROLES)
            assertTrue(Palettes.FALLBACK.dark[r] != null && Palettes.FALLBACK.light[r] != null)
        }
    }

    @Test fun readyPalettesBringTheDiffColours() {
        val r = Control.parse(
            """{"t":"ready","bridge":1,"protocol":13,"palettes":[{"name":"One Dark","dark":true,"colors":{"add_bg":"#112233","del_bg":"nope"}}]}""",
        ) as ServerMessage.Ready
        assertEquals("#112233", r.palettes.dark["add_bg"])
        assertEquals(Palettes.FALLBACK.dark["del_bg"], r.palettes.dark["del_bg"])
    }
}
