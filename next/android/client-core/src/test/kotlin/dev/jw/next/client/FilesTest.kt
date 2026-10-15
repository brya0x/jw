package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.FileEntry
import dev.jw.next.contract.FileKind
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.GitMark
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-323 to REQ-328 against the shared fixture of a `list_files` reply. */
class FilesTest {
    private val listing = (ContractJson.decodeFromString(
        DaemonMsg.serializer(),
        File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon/reply-ok-list-files.json").readText(),
    ) as DaemonMsg.Reply).let { ContractJson.decodeFromJsonElement(FileListing.serializer(), it.result!!) }

    @Test
    fun theFixtureReadsAsAListingWithEachKindMarkAndSize() {
        assertEquals("src", listing.path)
        assertEquals(listOf(FileKind.Dir, FileKind.Dir, FileKind.File, FileKind.Link), listing.entries.map { it.kind })
        assertEquals(GitMark.Modified, listing.entries[0].git)
        assertTrue(listing.entries[1].ignored)
        assertEquals(1536L, listing.entries[2].size)
    }

    @Test
    fun rowsKeepTheOrderTheyCameInAndHaveTheirPathFromTheWorkspace() {
        val v = filesView("jw", listing, hidden = false)
        assertEquals(listOf("auth", "target", "main.rs", "link-a-etc"), v.rows.map { it.name })
        assertEquals(listOf("src/auth", "src/target", "src/main.rs", "src/link-a-etc"), v.rows.map { it.path })
        assertEquals("src/x", filesView("jw", FileListing("src", listOf(FileEntry("x", FileKind.File)), 1), false).rows.single().path)
        assertEquals("x", filesView("jw", FileListing("", listOf(FileEntry("x", FileKind.File)), 1), false).rows.single().path, "no leading slash at the top")
    }

    @Test
    fun theHiddenOnesAreLeftOutUnlessAsked() {
        val l = FileListing("", listOf(FileEntry(".git", FileKind.Dir), FileEntry("src", FileKind.Dir), FileEntry(".gitignore", FileKind.File)), 3)
        assertEquals(listOf("src"), filesView("jw", l, false).rows.map { it.name })
        assertEquals(listOf(".git", "src", ".gitignore"), filesView("jw", l, true).rows.map { it.name })
        assertFalse(filesView("jw", l, false).cut, "what the screen hides is not what the PC cut")
    }

    @Test
    fun theWalkGoesNoHigherThanTheWorkspacesFolder() {
        val top = filesView("jw", FileListing("", emptyList(), 0), false)
        assertNull(top.parent)
        assertEquals("/", top.where)
        assertEquals(listOf("jw" to ""), top.crumbs)
        val deep = filesView("jw", FileListing("src/auth", emptyList(), 0), false)
        assertEquals("src", deep.parent)
        assertEquals("/src/auth", deep.where)
        assertEquals(listOf("jw" to "", "src" to "src", "auth" to "src/auth"), deep.crumbs)
        assertEquals("", filesView("jw", FileListing("src", emptyList(), 0), false).parent, "up from a first folder is the workspace's own")
    }

    @Test
    fun aListingTheDaemonCutSaysSo() {
        val cut = filesView("jw", FileListing("", listOf(FileEntry("a", FileKind.File)), 2005), false)
        assertTrue(cut.cut)
        assertEquals(2005, cut.total)
    }

    @Test
    fun whatGitSaysIsOneMarkAndIgnoredIsTheLastResort() {
        fun mark(git: GitMark?, ignored: Boolean = false) = FileRow("x", "x", FileKind.File, null, git, ignored).mark
        assertEquals(FileMark("M", MarkTone.Changed), mark(GitMark.Modified))
        assertEquals(FileMark("?", MarkTone.New), mark(GitMark.Untracked))
        assertEquals(FileMark("A", MarkTone.New), mark(GitMark.Added))
        assertEquals(FileMark("D", MarkTone.Bad), mark(GitMark.Deleted))
        assertEquals(FileMark("!", MarkTone.Bad), mark(GitMark.Conflict))
        assertEquals(FileMark("◌", MarkTone.Ignored), mark(null, ignored = true))
        assertNull(mark(null))
    }

    @Test
    fun theSecondLineIsTheSizeOfAFileAndNothingForAFolder() {
        fun detail(kind: FileKind, size: Long? = null) = FileRow("x", "x", kind, size, null, false).detail
        assertEquals("936 B", detail(FileKind.File, 936))
        assertEquals("0 B", detail(FileKind.File, 0))
        assertEquals("1 KB", detail(FileKind.File, 1024))
        assertEquals("2 KB", detail(FileKind.File, 1536), "rounded to the nearest")
        assertEquals("1023 KB", detail(FileKind.File, 1023L * 1024))
        assertEquals("3.5 MB", detail(FileKind.File, 3_670_016))
        assertEquals("enlace", detail(FileKind.Link))
        assertNull(detail(FileKind.Dir))
    }
}
