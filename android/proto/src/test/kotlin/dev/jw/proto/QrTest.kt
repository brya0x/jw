package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class QrTest {
    private val token = "0123456789abcdef0123456789abcdef"
    private val fp = "b".repeat(64)

    @Test fun tunnelFormStillParses() {
        val q = QrPayload.parse("jw://virtual-archive.trycloudflare.com/$token")!!
        assertEquals("virtual-archive.trycloudflare.com", q.tunnel)
        assertNull(q.lan)
        assertEquals(token, q.token)
    }

    @Test fun wifiFormParsesWithAndWithoutTheTunnel() {
        val a = QrPayload.parse("jw://192.168.1.23:48211/$token?fp=$fp")!!
        assertEquals("192.168.1.23:48211", a.lan!!.hostPort)
        assertEquals(fp, a.lan!!.fp)
        assertNull(a.tunnel)
        val b = QrPayload.parse("  jw://192.168.1.23:48211/$token?fp=$fp&t=a-b.trycloudflare.com\n")!!
        assertEquals("a-b.trycloudflare.com", b.tunnel)
        assertEquals(token, b.token)
    }

    @Test fun theTokenOnlyGoesToHostsItMay() {
        // A public address, a name, a wrong tunnel suffix, a bad port, a short fingerprint: none is parsed.
        for (bad in listOf(
            "jw://8.8.8.8:1000/$token?fp=$fp",
            "jw://172.32.0.1:1000/$token?fp=$fp",
            "jw://evil.example.com:1000/$token?fp=$fp",
            "jw://192.168.1.23:0/$token?fp=$fp",
            "jw://192.168.1.23:70000/$token?fp=$fp",
            "jw://192.168.1.23:1000/$token?fp=${fp.take(10)}",
            "jw://192.168.1.23:1000/$token?fp=$fp&t=evil.example.com",
            "jw://evil.example.com/$token",
            "jw://192.168.1.23:1000/${token.take(8)}?fp=$fp",
            "https://192.168.1.23:1000/$token?fp=$fp",
        )) assertNull(bad, QrPayload.parse(bad))
    }

    @Test fun privateRanges() {
        for (ok in listOf("10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.9.9", "192.168.0.0")) assertTrue(ok, Hosts.isPrivateV4(ok))
        for (no in listOf("172.15.0.1", "172.32.0.1", "192.167.1.1", "169.254.1.1", "127.0.0.1", "1.2.3.4", "300.1.1.1", "10.0.0", "a.b.c.d")) assertFalse(no, Hosts.isPrivateV4(no))
    }

    @Test fun toStringNeverHoldsTheToken() {
        assertFalse(QrPayload.parse("jw://192.168.1.23:1/$token?fp=$fp")!!.toString().contains(token))
        assertNotNull(Hosts.lan("10.0.0.2", 1, fp))
    }
}
