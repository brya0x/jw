package dev.jw.mobile

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** REQ-103: every activity is locked to portrait. */
class ManifestOrientationTest {
    @Test fun everyActivityIsPortrait() {
        val manifest = File("src/main/AndroidManifest.xml").readText()
        val activities = Regex("<activity\\b[^>]*>").findAll(manifest).map { it.value }.toList()
        assertTrue(activities.isNotEmpty())
        for (a in activities) assertTrue(a, a.contains("android:screenOrientation=\"portrait\""))
        assertEquals(false, manifest.contains("configChanges"))
    }
}
