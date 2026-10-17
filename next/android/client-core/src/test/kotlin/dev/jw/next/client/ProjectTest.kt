package dev.jw.next.client

import dev.jw.next.contract.ConfigFrom
import dev.jw.next.contract.OpenKind
import dev.jw.next.contract.ProjectEdit
import dev.jw.next.contract.ProjectInfo
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** REQ-520 to REQ-526: the project screen's model. */
class ProjectTest {
    private fun info(from: ConfigFrom?, rootText: String? = null) = ProjectInfo(
        source = from?.let { "/p/.jw.toml" },
        root = "/code/app-wt",
        rootText = rootText,
        setup = listOf("make deps"),
        open = OpenKind.Shell,
        from = from,
        personal = "/c/projects/app.toml",
        ports = mapOf("web" to 0),
    )

    @Test
    fun theDraftShowsWhatThePcSaysAndStartsOnTheFileItCameFrom() {
        val d = info(ConfigFrom.Repo, "../app-trees").draft()
        assertEquals("../app-trees", d.cur.root, "the root as the file wrote it")
        assertTrue(d.cur.rootSet)
        assertEquals(ConfigFrom.Repo, d.to)
        assertEquals(ConfigFrom.Personal, info(null).draft().to, "a project with no file saves to the personal one")
        assertFalse(info(null).draft().cur.rootSet)
        assertEquals("/code/app-wt", info(null).draft().cur.root, "the default root, as the PC says it")
        assertTrue(d.nothingToSave)
    }

    @Test
    fun onlyWhatChangedIsSentToTheFileTheSettingsCameFrom() {
        val d = info(ConfigFrom.Personal).draft().withOpen(OpenKind.Claude).withList(ProjectField.Setup, listOf("make deps", "  ", "make seed"))
        assertEquals(ProjectEdit(setup = listOf("make deps", "make seed"), open = OpenKind.Claude), d.edit(), "the root, the copies and the ports did not change (REQ-524)")
        assertTrue(d.dirty)
        assertFalse(d.nothingToSave)
    }

    @Test
    fun savingInAnotherFileSendsEverythingTheScreenShows() {
        val d = info(ConfigFrom.Repo).draft().withTarget(ConfigFrom.Personal)
        assertEquals(
            ProjectEdit(setup = listOf("make deps"), copy = emptyList(), ports = mapOf("web" to 0), open = OpenKind.Shell),
            d.edit(),
            "a root nobody wrote is not written",
        )
        assertFalse(d.nothingToSave, "a new file is made even with nothing changed")
    }

    @Test
    fun aRootIsTypedAndAnEmptyOneIsTheDefaultAgain() {
        val d = info(ConfigFrom.Personal).draft().withRoot(" ../x ")
        assertEquals("../x", d.edit().root)
        assertEquals(null, d.withRoot("").edit().root, "no root was written and none is now")
        val written = info(ConfigFrom.Personal, "../old").draft().withRoot("")
        assertEquals("", written.edit().root, "an empty root takes the key out of the file")
        assertEquals("/code/app-wt", written.cur.root)
        assertFalse(written.cur.rootSet)
    }

    @Test
    fun portsAreCheckedWhenTypedAndTheFirstBadLineIsNamed() {
        assertEquals(listOf("web" to 0, "api" to 1), parsePorts(listOf("web=0", " api = 1 ", "")).getOrThrow())
        for (bad in listOf("=5", "base=1", "Api=3", "api=100", "api", "api=-1", "a b=1")) {
            val why = parsePorts(listOf("web=0", bad)).exceptionOrNull()?.message
            assertTrue(why != null && why.contains(bad), "$bad: $why")
        }
        assertEquals("el puerto web ya está", parsePorts(listOf("web=0", "web=1")).exceptionOrNull()?.message)
        assertEquals(listOf("web=0"), portLines(listOf("web" to 0)))
    }

    @Test
    fun whereItSavesIsToldWithItsWarning() {
        val repo = info(ConfigFrom.Repo).draft()
        assertEquals("cambia el .jw.toml que ve el equipo", repo.warning())
        assertEquals("quedaría tapada por el .jw.toml del repositorio", repo.withTarget(ConfigFrom.Personal).warning())
        assertEquals("crea un .jw.toml en el repositorio", info(ConfigFrom.Personal).draft().withTarget(ConfigFrom.Repo).warning())
        assertEquals("", info(null).draft().warning())
        assertEquals("/code/app/.jw.toml (lo ve el equipo)", repo.target("/code/app/"))
        assertEquals("/c/projects/app.toml", repo.withTarget(ConfigFrom.Personal).target("/code/app"))
    }

    @Test
    fun whatSavingSaidIsToldInWords() {
        assertEquals("guardado en /p/.jw.toml", savedNote(Result.success("/p/.jw.toml" to false)))
        assertTrue(savedNote(Result.success("/c/a.toml" to true)).contains("tapado"))
        assertEquals("no se pudo guardar: disco lleno", savedNote(Result.failure(Exception("disco lleno"))))
    }
}
