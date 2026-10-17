package dev.jw.next.client

import dev.jw.next.contract.Cursor
import dev.jw.next.contract.Screen
import dev.jw.next.contract.Span
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals

/** REQ-532: the phone draws the colours of a screen on the alternate screen, and the text of any other. */
class ScreenColoursTest {
    private val cursor = Cursor(0, 0, true)

    @Test
    fun a_screen_without_spans_is_its_lines_as_plain_text() {
        val screen = Screen(2, 10, listOf("$ ls", ""), cursor)
        assertEquals(listOf(listOf(Span("$ ls")), listOf(Span(""))), screen.styledRows())
    }

    @Test
    fun a_screen_with_spans_is_drawn_from_them_and_not_from_its_lines() {
        val red = Span("local", fg = JsonPrimitive(1))
        val screen = Screen(2, 10, listOf("local x", ""), cursor, alt = true, spans = listOf(listOf(red, Span(" x")), emptyList()))
        assertEquals(listOf(listOf(red, Span(" x")), emptyList()), screen.styledRows())
    }
}
