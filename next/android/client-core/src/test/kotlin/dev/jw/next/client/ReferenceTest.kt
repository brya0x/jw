package dev.jw.next.client

import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-631: the same table as the terminal client's `a_path_is_written_from_the_folder_the_engine_runs_in`. */
class ReferenceTest {
    private val w = "/w/app"

    @Test
    fun a_path_is_written_from_the_folder_the_engine_runs_in() {
        // the workspace itself: as it is
        assertEquals("src/a.rs", fromFolder(w, "/w/app", "src/a.rs"))
        // below the folder, beside it, above it, and the folder itself
        assertEquals("x.rs", fromFolder(w, "/w/app/crates", "crates/x.rs"))
        assertEquals("../x.rs", fromFolder(w, "/w/app/crates/d", "crates/x.rs"))
        assertEquals("../docs/a.md", fromFolder(w, "/w/app/crates", "docs/a.md"))
        assertEquals(".", fromFolder(w, "/w/app/crates", "crates"))
        // outside the workspace, with a trailing slash and a `.`
        assertEquals("../../w/app/docs/a.md", fromFolder(w, "/home/me/", "docs/a.md"))
        assertEquals("x.rs", fromFolder(w, "/w/app/./crates/", "crates/x.rs"))
    }

    @Test
    fun the_list_starts_in_the_folder_when_it_is_inside_the_workspace_and_at_its_root_when_not() {
        assertEquals("crates/d", folderInWorkspace(w, "/w/app/crates/d"))
        assertEquals("crates", folderInWorkspace("/w/app/", "/w/app/crates/"))
        assertEquals("", folderInWorkspace(w, "/w/app"))
        assertEquals("", folderInWorkspace(w, "/home/me"))
        assertEquals("", folderInWorkspace(w, "/w/application"))
        assertEquals("", folderInWorkspace(w, null))
        assertEquals("", folderInWorkspace(null, "/w/app/crates"))
    }
}
