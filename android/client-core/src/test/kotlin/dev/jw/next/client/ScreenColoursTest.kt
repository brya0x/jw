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

    @Test
    fun the_cursor_is_on_its_row_and_needs_spaces_only_when_the_text_ends_before_it() {
        val spans = listOf(listOf(Span("local"), Span(" x")), emptyList())
        fun at(row: Int, col: Int, visible: Boolean = true) =
            Screen(2, 10, listOf("local x", ""), Cursor(row, col, visible), alt = true, spans = spans).cursorCell()
        assertEquals(CursorCell(0, 3, 0), at(0, 3), "on a cell of the text")
        assertEquals(CursorCell(0, 6, 0), at(0, 6), "on the last character")
        assertEquals(CursorCell(0, 7, 1), at(0, 7), "just after the text")
        assertEquals(CursorCell(1, 4, 5), at(1, 4), "on an empty row")
        assertEquals(null, at(0, 3, visible = false), "hidden")
        assertEquals(null, at(2, 0), "below the rows")
    }

    @Test
    fun the_cursor_of_a_screen_without_spans_is_read_on_its_lines() {
        val screen = Screen(2, 10, listOf("$ ls", ""), Cursor(0, 4, true))
        assertEquals(CursorCell(0, 4, 1), screen.cursorCell())
    }
}
