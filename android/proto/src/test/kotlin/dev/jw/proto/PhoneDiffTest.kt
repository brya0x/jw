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
        assertEquals(DiffScope.General, d.scope)
        assertEquals("demo vs origin/main", d.title)
        assertTrue(d.truncated)
        assertNull(d.tooLarge)
        assertFalse(d.noBase)
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
        assertTrue(d.files[4].untracked)
        assertFalse(d.files[0].untracked)
        assertTrue(d.files[5].truncated)
        assertEquals(5000, d.files[5].added)
        assertNull(d.files[0].unreadable)
    }

    @Test fun theRequestIsTheDocumentedOne() {
        assertEquals("""{"t":"diff","ws":"w1","scope":"general"}""", Control.diff("w1"))
        assertEquals("""{"t":"diff","ws":"w1","scope":"uncommitted"}""", Control.diff("w1", DiffScope.Uncommitted))
        assertEquals("""{"t":"diff","ws":"w1","scope":"commit:abc1234"}""", Control.diff("w1", DiffScope.Commit("abc1234")))
        assertEquals("""{"t":"commits","ws":"w1"}""", Control.commits("w1"))
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
            """{"t":"ready","bridge":1,"protocol":14,"palettes":[{"name":"One Dark","dark":true,"colors":{"add_bg":"#112233","del_bg":"nope"}}]}""",
        ) as ServerMessage.Ready
        assertEquals("#112233", r.palettes.dark["add_bg"])
        assertEquals(Palettes.FALLBACK.dark["del_bg"], r.palettes.dark["del_bg"])
    }

    @Test fun aViewPastTheServersCapListsNamesOnly() {
        val raw = checkNotNull(javaClass.getResource("/phone_diff_large.json")).readText()
        val d = (Control.parse(raw) as ServerMessage.DiffAnswer).diff
        assertEquals(DiffScope.Uncommitted, d.scope)
        assertEquals(1234, d.tooLarge)
        assertEquals(listOf("a/b.rs", "c.txt"), d.files.map { it.path })
        assertTrue(d.files.all { it.hunks.isEmpty() })
    }

    @Test fun theCommitListAndItsScopeWords() {
        val raw = checkNotNull(javaClass.getResource("/phone_commits.json")).readText()
        val c = (Control.parse(raw) as ServerMessage.CommitsAnswer).list
        assertEquals("w1", c.ws)
        assertTrue(c.more)
        assertFalse(c.noBase)
        assertEquals(listOf("0123456", "89abcde"), c.commits.map { it.short })
        assertEquals("fix the thing", c.commits[0].subject)
        assertEquals(1_760_000_000L, c.commits[0].whenSecs)
        assertEquals("0123456789abcdef0123456789abcdef01234567", c.commits[0].sha)
        assertFalse(c.toString().contains("fix the thing"))
        assertEquals(DiffScope.Commit("abc1234"), DiffScope.parse("commit:abc1234"))
        assertEquals("commit:abc1234", DiffScope.Commit("abc1234").word)
        assertNull(DiffScope.parse("branch:main"))
    }

    @Test fun anErrorSaysWhichAskItAnswers() {
        val e = Control.parse("""{"t":"error","code":"unsupported","msg":"busy: wait","ask":"diff","ws":"w1","scope":"uncommitted"}""") as ServerMessage.Error
        assertEquals(Triple("diff", "w1", "uncommitted"), Triple(e.ask, e.ws, e.scope))
        val old = Control.parse("""{"t":"error","code":"read_only","msg":"m"}""") as ServerMessage.Error
        assertNull(old.ask)
    }
}
