package dev.jw.mobile

import dev.jw.proto.LanAddr
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class StoredDeviceFormatTest {
    private val fp = "f".repeat(64)
    private val device = "ab".repeat(32)

    @Test fun theFirstFormStillReads() {
        val d = KeystoreTokenStore.decode("old-one.trycloudflare.com\n$device")
        assertEquals("old-one.trycloudflare.com", d.host)
        assertEquals(device, d.device)
        assertNull(d.lan)
    }

    @Test fun v2RoundTripsWithBothAddresses() {
        val d = KeystoreTokenStore.decode(KeystoreTokenStore.encode(StoredDevice("a-b.trycloudflare.com", device, LanAddr("192.168.1.23", 48211, fp))))
        assertEquals("a-b.trycloudflare.com", d.host)
        assertEquals(device, d.device)
        assertEquals(LanAddr("192.168.1.23", 48211, fp), d.lan)
    }

    @Test fun v2RoundTripsWithOnlyTheWifiOrOnlyTheTunnel() {
        val wifi = KeystoreTokenStore.decode(KeystoreTokenStore.encode(StoredDevice("", device, LanAddr("10.0.0.9", 1000, fp))))
        assertEquals("", wifi.host)
        assertEquals("10.0.0.9:1000", wifi.lan!!.hostPort)
        val tunnel = KeystoreTokenStore.decode(KeystoreTokenStore.encode(StoredDevice("a-b.trycloudflare.com", device)))
        assertNull(tunnel.lan)
    }

    @Test fun whatIsReadBackIsCheckedAgain() {
        // A file that names a host the app never connects to is not trusted: it counts as unreadable.
        assertThrows(Exception::class.java) { KeystoreTokenStore.decode("v2\n$device\nevil.example.com\n\n") }
        // A wifi address that is not private or has no fingerprint reads as none.
        assertNull(KeystoreTokenStore.decode("v2\n$device\n\n8.8.8.8:1\n$fp").lan)
        assertNull(KeystoreTokenStore.decode("v2\n$device\n\n192.168.1.2:1\nshort").lan)
    }
}
