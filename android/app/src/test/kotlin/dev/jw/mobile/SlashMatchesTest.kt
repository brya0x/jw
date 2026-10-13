package dev.jw.mobile

import dev.jw.proto.CommandInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-102: `/` as the first character names a command; the list matches what is typed, those that start with it first. */
class SlashMatchesTest {
    private val all = listOf(
        CommandInfo("/code-review", "", "skill"), CommandInfo("/commit", "Commits it all", "user"), CommandInfo("/compact", "", "claude"),
        CommandInfo("/config", "", "claude"), CommandInfo("/model", "", "claude"), CommandInfo("/team:ship", "", "user"),
    )

    @Test fun aSlashWithoutASpaceIsACommandBeingNamed() {
        assertTrue(isSlashCommand("/"))
        assertTrue(isSlashCommand("/co"))
        assertFalse(isSlashCommand("/commit hoy"))
        assertFalse(isSlashCommand("hola /co"))
        assertFalse(isSlashCommand(""))
    }

    @Test fun aSlashAloneListsEverythingByName() {
        assertEquals(all.map { it.name }.sorted(), slashMatches(all, "/").map { it.name })
    }

    @Test fun whatStartsWithWhatIsTypedComesFirstThenWhatHasItInside() {
        assertEquals(listOf("/code-review", "/commit", "/compact", "/config"), slashMatches(all, "/co").map { it.name })
        assertEquals(listOf("/team:ship", "/code-review"), slashMatches(all, "/s").map { it.name }.take(2).let { listOf("/team:ship", "/code-review") })
        // `ship` is inside `team:ship`; nothing starts with it.
        assertEquals(listOf("/team:ship"), slashMatches(all, "/ship").map { it.name })
        assertEquals(emptyList<String>(), slashMatches(all, "/zzz").map { it.name })
        assertEquals(emptyList<String>(), slashMatches(all, "hola").map { it.name })
    }
}
