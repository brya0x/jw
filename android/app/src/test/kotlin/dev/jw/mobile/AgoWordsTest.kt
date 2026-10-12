package dev.jw.mobile

import org.junit.Assert.assertEquals
import org.junit.Test

class AgoWordsTest {
    @Test fun ageInWords() {
        assertEquals("ahora", agoWords(5))
        assertEquals("ahora", agoWords(-30))
        assertEquals("hace 5 min", agoWords(300))
        assertEquals("hace 3 h", agoWords(3 * 3600 + 10))
        assertEquals("hace 2 d", agoWords(2 * 86_400))
        assertEquals("hace 2 m", agoWords(65 * 86_400))
    }

    @Test fun whyAFileIsNotShownInWords() {
        assertEquals("enlace simbólico (no se sigue)", unreadableWords("symlink"))
        assertEquals("algo nuevo", unreadableWords("algo nuevo"))
    }
}
