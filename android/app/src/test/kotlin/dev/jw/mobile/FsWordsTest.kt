package dev.jw.mobile

import org.junit.Assert.assertEquals
import org.junit.Test

class FsWordsTest {
    @Test fun theReasonsOfThePcInTheAppsWords() {
        assertEquals("sin permiso", fsReasonWords("permission denied"))
        assertEquals("no existe", fsReasonWords("no such file or folder"))
        assertEquals("el PC tardó demasiado", fsReasonWords("took longer than 5 s"))
        assertEquals("el PC está ocupado: intenta de nuevo", fsReasonWords("busy: wait for the answer before asking again"))
        assertEquals("algo que no conozco", fsReasonWords("algo que no conozco"))
    }

    @Test fun pathsAndSizes() {
        assertEquals("/a/b", joinPath("/a", "b"))
        assertEquals("/b", joinPath("/", "b"))
        assertEquals("812 B", sizeWords(812))
        assertEquals("4 KB", sizeWords(4096))
        assertEquals("1.7 MB", sizeWords(1_800_000))
    }
}
