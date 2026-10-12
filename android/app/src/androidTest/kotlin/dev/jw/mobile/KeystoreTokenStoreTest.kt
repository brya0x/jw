package dev.jw.mobile

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** The real Android Keystore on a real device. Assertions never print the values. */
@RunWith(AndroidJUnit4::class)
class KeystoreTokenStoreTest {
    private val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val dev = StoredDevice("test.trycloudflare.com", "ab".repeat(32))

    @Before @After fun clean() = KeystoreTokenStore(ctx).clear()

    private fun same(a: StoredDevice?) = a != null && a.host == dev.host && a.device == dev.device

    @Test fun savedDeviceComesBackFromANewInstance() {
        KeystoreTokenStore(ctx).save(dev)
        assertEquals(true, same(KeystoreTokenStore(ctx).load()))
    }

    @Test fun fileIsNotPlainText() {
        KeystoreTokenStore(ctx).save(dev)
        val raw = File(ctx.filesDir, "device.bin").readBytes()
        assertFalse(String(raw, Charsets.ISO_8859_1).contains(dev.device))
    }

    @Test fun aCorruptedFileIsClearedAndLoadsNull() {
        val store = KeystoreTokenStore(ctx)
        store.save(dev)
        val f = File(ctx.filesDir, "device.bin")
        val b = f.readBytes()
        b[b.size - 1] = (b[b.size - 1].toInt() xor 1).toByte()
        f.writeBytes(b)
        assertNull(KeystoreTokenStore(ctx).load())
        assertFalse(f.exists())
    }

    @Test fun savesAgainAfterAClear() {
        val store = KeystoreTokenStore(ctx)
        store.save(dev)
        store.clear()
        assertNull(store.load())
        store.save(dev)
        assertNotNull(KeystoreTokenStore(ctx).load())
    }
}
