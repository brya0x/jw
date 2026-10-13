package dev.jw.mobile

import dev.jw.proto.Control
import dev.jw.proto.DiffFile
import dev.jw.proto.ServerMessage
import org.junit.Assert.assertEquals
import org.junit.Test

/** REQ-89, REQ-90: the files of a view as a folder tree. */
class DiffTreeTest {
    private fun files(vararg paths: String): List<DiffFile> {
        val items = paths.joinToString(",") { """{"path":"$it","status":"modified","added":2,"deleted":1,"hunks":[]}""" }
        return ((Control.parse("""{"t":"diff","ws":"w","scope":"general","files":[$items]}""") as ServerMessage.DiffAnswer).diff).files
    }

    private fun show(rows: List<TreeRow>) = rows.map {
        when (it) {
            is TreeRow.Dir -> " ".repeat(it.depth) + it.label + "/"
            is TreeRow.File -> " ".repeat(it.depth) + it.file.path.substringAfterLast('/')
        }
    }

    @Test fun foldersComeFirstAndASingleChildChainIsOneRow() {
        val sorted = treeSorted(files("README.md", "crates/tui/src/b.rs", "crates/tui/src/a.rs", "crates/core/x.rs", "docs/specs/s.md"))
        assertEquals(listOf("crates/core/x.rs", "crates/tui/src/a.rs", "crates/tui/src/b.rs", "docs/specs/s.md", "README.md"), sorted.map { it.path })
        assertEquals(
            listOf("crates/", " core/", "  x.rs", " tui/src/", "  a.rs", "  b.rs", "docs/specs/", " s.md", "README.md"),
            show(treeRows(sorted, emptySet())),
        )
    }

    @Test fun aFolderCarriesItsCountsAndAClosedOneHidesOnlyItsRows() {
        val sorted = treeSorted(files("a/x.rs", "a/b/y.rs", "a/b/z.rs", "top.rs"))
        val dir = treeRows(sorted, emptySet()).first() as TreeRow.Dir
        assertEquals(Triple(3, 6, 3), Triple(dir.files, dir.added, dir.deleted))
        assertEquals(listOf("a/", "top.rs"), show(treeRows(sorted, setOf("a/"))))
        assertEquals(listOf("a/", " b/", " x.rs", "top.rs"), show(treeRows(sorted, setOf("a/b/"))))
        assertEquals(4, sorted.size)
    }
}
