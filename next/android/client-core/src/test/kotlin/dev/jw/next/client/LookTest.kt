package dev.jw.next.client

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/** REQ-156 to REQ-158: the phone's look. */
class LookTest {
    @Test
    fun theTableIsTheSharedFile() {
        val file = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "themes.json")
        val themes = Json.parseToJsonElement(file.readText()).jsonObject["themes"]!!.jsonArray.map { it.jsonObject }
        assertEquals(themes.map { it["id"]!!.jsonPrimitive.content }, Themes.all.map { it.id }, "the same themes in the same order")
        fun colour(o: JsonObject, key: String) = 0xFF000000L or o[key]!!.jsonPrimitive.content.removePrefix("#").toLong(16)
        for ((o, t) in themes.zip(Themes.all)) {
            assertEquals(o["name"]!!.jsonPrimitive.content, t.name, t.id)
            assertEquals(o["kind"]!!.jsonPrimitive.content == "dark", t.dark, t.id)
            val ours = listOf(t.bg, t.panel, t.fg, t.dim, t.line, t.sel, t.blue, t.green, t.yellow, t.red, t.magenta, t.cyan)
            val theirs = listOf("bg", "panel", "fg", "dim", "line", "sel", "blue", "green", "yellow", "red", "magenta", "cyan").map { colour(o, it) }
            assertEquals(theirs, ours, t.id)
        }
    }

    @Test
    fun aSelectorShowsOnlyTheKindInForce() {
        val look = Look(LookMode.Dark)
        assertEquals(listOf("one-dark", "catppuccin-mocha", "tokyo-night", "gruvbox-dark", "solarized-dark"), look.shown(osDark = false).map { it.id })
        assertTrue(Look(LookMode.Light).shown(osDark = true).none { it.dark })
        // `system` shows what the phone is in now.
        assertTrue(Look().shown(osDark = true).all { it.dark })
        assertTrue(Look().shown(osDark = false).none { it.dark })
    }

    @Test
    fun eachKindKeepsItsOwnChoice() {
        val look = Look(LookMode.Dark).choose(osDark = false, "gruvbox-dark").step(forward = true)
        assertEquals(LookMode.Light, look.mode)
        assertEquals("one-light", look.theme(osDark = true).id, "the light choice was not touched")
        val picked = look.choose(osDark = true, "tokyo-day")
        assertEquals("tokyo-day", picked.theme(osDark = false).id)
        assertEquals("gruvbox-dark", picked.copy(mode = LookMode.Dark).theme(osDark = false).id, "back to the dark one")
    }

    @Test
    fun aThemeOfTheOtherKindIsNotAChoice() {
        val look = Look(LookMode.Dark)
        assertEquals(look, look.choose(osDark = false, "one-light"))
        assertEquals(look, look.choose(osDark = false, "nope"))
    }

    @Test
    fun systemFollowsThePhoneAndTheOthersDoNot() {
        assertTrue(Look().isDark(osDark = true))
        assertFalse(Look().isDark(osDark = false))
        assertFalse(Look(LookMode.Light).isDark(osDark = true))
        assertTrue(Look(LookMode.Dark).isDark(osDark = false))
        assertEquals(LookMode.Light, LookMode.System.step(forward = false))
        assertEquals(LookMode.System, LookMode.Light.step(forward = true))
    }

    @Test
    fun whatIsStoredIsReadBackAndEachValueFallsBackAlone() {
        assertEquals(Look(), Look.of(null, null, null))
        assertEquals(Look(LookMode.Light, "one-dark", "gruvbox-light"), Look.of("light", "nope", "gruvbox-light"))
        assertEquals(Look(LookMode.System, "tokyo-night", "one-light"), Look.of("sepia", "tokyo-night", "tokyo-night"), "a dark theme is not a light choice")
    }
}
