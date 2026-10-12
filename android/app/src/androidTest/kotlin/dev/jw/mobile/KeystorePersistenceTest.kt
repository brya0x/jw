package dev.jw.mobile

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Run `save`, then `am force-stop dev.jw.mobile`, then `load`, as separate instrumentation runs (see android/AGENTS.md).
 * `load` clears the store at the end. Run alone, `load` fails on purpose: nothing was saved.
 */
@RunWith(AndroidJUnit4::class)
class KeystorePersistenceTest {
    private val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val host = "persist.trycloudflare.com"
    private val token = "cd".repeat(32)

    @Test fun save() = KeystoreTokenStore(ctx).save(StoredDevice(host, token))

    @Test fun load() {
        val got = KeystoreTokenStore(ctx).load()
        KeystoreTokenStore(ctx).clear()
        assertEquals(true, got != null && got.host == host && got.device == token)
    }
}
