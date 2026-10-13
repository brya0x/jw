package dev.jw.mobile

import org.junit.Assert.assertEquals
import org.junit.Test

/** REQ-91: a message's `code` and **bold** are drawn; a mark that is not closed is text. */
class MarkedPiecesTest {
    @Test fun boldAndCodeAreCutOutOfTheText() {
        assertEquals(
            listOf(Mark.Plain to "Las líneas con ", Mark.Bold to "something", Mark.Plain to " y ", Mark.Code to "code", Mark.Plain to "."),
            markedPieces("Las líneas con **something** y `code`."),
        )
    }

    @Test fun nothingInsideCodeIsRead() {
        assertEquals(listOf(Mark.Code to "**x**"), markedPieces("`**x**`"))
    }

    @Test fun aMarkThatIsNeverClosedOrIsEmptyShowsAsWritten() {
        assertEquals(listOf(Mark.Plain to "a ** b y `c"), markedPieces("a ** b y `c"))
        assertEquals(listOf(Mark.Plain to "**** y ``"), markedPieces("**** y ``"))
        assertEquals(listOf(Mark.Plain to "2 * 3 * 4"), markedPieces("2 * 3 * 4"))
    }

    @Test fun severalBoldsInOneLine() {
        assertEquals(
            listOf(Mark.Bold to "uno", Mark.Plain to ", ", Mark.Bold to "dos"),
            markedPieces("**uno**, **dos**"),
        )
    }

    @Test fun aRunOfBackticksIsClosedByAsManyAndDoesNotShiftTheBoldAfterIt() {
        assertEquals(
            listOf(Mark.Plain to "tal cual ", Mark.Code to "`**x**`", Mark.Plain to " y ", Mark.Bold to "negrita"),
            markedPieces("tal cual `` `**x**` `` y **negrita**"),
        )
        assertEquals(listOf(Mark.Code to "**"), markedPieces("`**`"))
    }
}
