package dev.jw.mobile

import dev.jw.proto.Hello
import dev.jw.proto.LanAddr
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.tls.HeldCertificate
import okhttp3.tls.HandshakeCertificates
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Test
import org.junit.runner.RunWith
import java.security.MessageDigest

/**
 * The wifi pin (REQ-59) against a real TLS server whose certificate is self-signed and has no address in it,
 * as the PC's is. The same assertions run on the phone (`PinnedTlsDeviceTest`), where the TLS stack is not the JDK's.
 */
@RunWith(AndroidJUnit4::class)
class PinnedTlsDeviceTest {
    private val cert = HeldCertificate.Builder().commonName("jw mobile").build()
    private val other = HeldCertificate.Builder().commonName("jw mobile").build()
    private val server = MockWebServer().apply {
        val certs = HandshakeCertificates.Builder().heldCertificate(cert).build()
        useHttps(certs.sslSocketFactory(), false)
        enqueue(MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) { webSocket.send("""{"t":"error","code":"unknown","msg":"m"}"""); webSocket.close(1000, null) }
        }))
        start()
    }

    private fun fp(c: HeldCertificate) =
        MessageDigest.getInstance("SHA-256").digest(c.certificate.encoded).joinToString("") { "%02x".format(it) }

    private fun lan(c: HeldCertificate) = LanAddr("127.0.0.1", server.port, fp(c))

    @After fun stop() = server.shutdown()

    private fun events(pin: String?) = runBlocking {
        withTimeout(10_000) {
            Bridge().connect("wss://127.0.0.1:${server.port}/", Hello.Auth("x"), pin = pin).toList()
        }
    }

    @Test fun theRightPinConnects() {
        assertTrue("the socket opened", events(fp(cert)).any { it is BridgeEvent.Open })
    }

    @Test fun aWrongPinIsRefusedBeforeAnythingIsSent() {
        val ev = events(fp(other))
        assertFalse("nothing opened, so nothing was sent", ev.any { it is BridgeEvent.Open })
        assertTrue(ev.last() is BridgeEvent.Ended)
    }

    @Test fun theOrdinaryChecksRefuseASelfSignedCertificate() {
        // The tunnel's client has no pin and must not trust this: the pin client is not the default one.
        val ev = events(null)
        assertFalse(ev.any { it is BridgeEvent.Open })
    }

    @Test fun theProbeSaysWhetherThePinnedHostAnswers() {
        assertTrue(Bridge().probe(lan(cert), 3_000))
        assertFalse("another certificate", Bridge().probe(lan(other), 3_000))
        assertFalse("nothing there", Bridge().probe(LanAddr("127.0.0.1", 1, fp(cert)), 1_000))
    }

    @Test fun anEmptyChainIsNotTrusted() {
        val t = PinnedTrust("a".repeat(64))
        try { t.checkServerTrusted(emptyArray(), "RSA"); error("accepted") } catch (_: java.security.cert.CertificateException) { }
        try { t.checkServerTrusted(null, "RSA"); error("accepted") } catch (_: java.security.cert.CertificateException) { }
    }
}
