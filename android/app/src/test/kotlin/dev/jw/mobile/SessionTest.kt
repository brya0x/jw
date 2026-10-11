package dev.jw.mobile

import dev.jw.proto.Failure
import dev.jw.proto.Hello
import dev.jw.proto.ReconnectPolicy
import dev.jw.proto.ServerMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class SessionTest {
    private val fake = FakeBridge()
    private val store = FakeStore()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val token = "0123456789abcdef0123456789abcdef"
    private val qr = "jw://example.trycloudflare.com/$token"

    private fun session(policy: () -> ReconnectPolicy = { ReconnectPolicy(baseMs = 50, maxMs = 100, dnsRetryMs = 50) }) =
        Session(scope, store, Bridge(), "Pixel", fake.endpoint(), policy)

    private suspend inline fun <reified T : UiState> Session.await(): T =
        withTimeout(20_000) { state.first { it is T } as T }

    @After fun tearDown() { scope.cancel(); fake.shutdown() }

    @Test fun pairsStoresDeviceAndConnects() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready(ro = true)) }
        val s = session()
        assertTrue(s.onQr(qr))
        val pairing = s.state.value as UiState.Pairing
        assertEquals(6, pairing.code.length)
        val c = s.await<UiState.Connected>()
        assertTrue(c.readOnly)
        assertEquals(10, c.protocol)
        assertEquals(FakeBridge.DEVICE, store.device!!.device)
        val hello = fake.received.take()
        assertTrue(hello.contains(""""t":"pair"""") && hello.contains(token) && hello.contains(pairing.code))
        assertNull("no Origin header", fake.server.takeRequest().getHeader("Origin"))
    }

    @Test fun failedSaveKeepsConnectingAndWarns() = runBlocking<Unit> {
        store.failSave = true
        fake.enqueue { _, ws -> ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready()) }
        val s = session()
        s.onQr(qr)
        assertTrue(!s.await<UiState.Connected>().saved)
    }

    @Test fun deviceIsStoredEvenIfReadyNeverComes() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.PAIRED); ws.fail("daemon") }
        fake.enqueue { _, _ -> }
        val s = session()
        s.onQr(qr)
        val r = s.await<UiState.Reconnecting>()
        assertEquals("daemon", r.code)
        assertEquals(FakeBridge.DEVICE, store.device!!.device)
    }

    @Test fun slowPcAnswerIsWaitedFor() = runBlocking<Unit> {
        fake.enqueue { _, ws ->
            Thread.sleep(15_000)
            ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready())
        }
        val s = session()
        s.onQr(qr)
        s.await<UiState.Connected>()
    }

    @Test fun pairingErrorsGoBackToScanWithoutRetry() = runBlocking<Unit> {
        for (code in listOf("expired", "denied", "unknown")) {
            fake.enqueue { _, ws -> ws.fail(code) }
            val s = session()
            s.onQr(qr)
            val scan = s.await<UiState.Scan>()
            assertTrue(code, scan.notice != null)
            assertNull(store.device)
        }
        assertEquals(3, fake.server.requestCount)
    }

    @Test fun unknownAndRevokedAreFinalWhenAuthing() = runBlocking<Unit> {
        for (code in listOf("unknown", "revoked")) {
            store.device = StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE)
            fake.enqueue { _, ws -> ws.fail(code) }
            val s = session()
            s.start()
            s.await<UiState.Scan>()
            assertNull("token dropped", store.device)
        }
        assertEquals(2, fake.server.requestCount)
    }

    @Test fun reconnectsAfterSilentClose() = runBlocking<Unit> {
        store.device = StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE)
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.close(1001, "bye") }
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready(ro = true)) }
        val s = session()
        s.start()
        s.await<UiState.Reconnecting>()
        assertTrue(s.await<UiState.Connected>().readOnly)
        assertEquals(2, fake.server.requestCount)
    }

    @Test fun protocolMismatchDoesNotAttachOrRetry() = runBlocking<Unit> {
        store.device = StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE)
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready(protocol = 11)) }
        val s = session()
        s.start()
        val m = s.await<UiState.Mismatch>()
        assertEquals(11, m.pc)
        kotlinx.coroutines.delay(300)
        assertEquals(1, fake.server.requestCount)
        assertTrue(s.state.value is UiState.Mismatch)
    }

    @Test fun readOnlyAndUnsupportedKeepTheConnection() = runBlocking<Unit> {
        store.device = StoredDevice("example.trycloudflare.com", FakeBridge.DEVICE)
        fake.enqueue { _, ws -> ws.send(FakeBridge.ready()); ws.send(FakeBridge.error("read_only")); ws.send(FakeBridge.error("unsupported")) }
        val s = session()
        s.start()
        s.await<UiState.Connected>()
        kotlinx.coroutines.delay(300)
        assertTrue(s.state.value is UiState.Connected)
    }

    @Test fun badQrIsRejectedAndNotEchoed() {
        val s = session()
        assertTrue(!s.onQr("https://evil.example/x"))
        assertTrue(s.state.value is UiState.Scan)
    }

    @Test fun dnsFailureBeforeFirstConnectIsRetriedThenGivesUp() = runBlocking<Unit> {
        store.device = StoredDevice("no-such-host.invalid", FakeBridge.DEVICE)
        val s = Session(scope, store, Bridge(), "Pixel", { "ws://no-such-host.invalid/" }, { ReconnectPolicy(dnsGraceMs = 400, dnsRetryMs = 100) })
        s.start()
        val scan = s.await<UiState.Scan>()
        assertTrue(scan.notice!!.contains("no-such-host.invalid"))
    }

    @Test fun bridgeFlowEndsWithFailure() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.fail("limited") }
        val events = Bridge().connect(fake.endpoint().url("x"), Hello.Auth(FakeBridge.DEVICE)).toList()
        assertTrue((events.first() as BridgeEvent.Message).msg is ServerMessage.Error)
        assertTrue((events.last() as BridgeEvent.Ended).failure is Failure.Network)
    }

    @Test fun secondQrWhilePairingIsIgnored() = runBlocking<Unit> {
        fake.enqueue { _, ws -> ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready()) }
        val s = session()
        assertTrue(s.onQr(qr))
        assertTrue(s.onQr(qr))
        s.await<UiState.Connected>()
        assertTrue(s.onQr(qr))
        assertEquals(1, fake.server.requestCount)
    }

    @Test fun pairingCodeIsAsciiDigitsInAnyLocale() {
        val old = java.util.Locale.getDefault()
        try {
            java.util.Locale.setDefault(java.util.Locale.forLanguageTag("ar-EG-u-nu-arab"))
            repeat(50) {
                val s = session()
                s.onQr(qr)
                val code = (s.state.value as UiState.Pairing).code
                assertTrue(code, code.length == 6 && code.all { it in '0'..'9' })
                s.stop()
            }
        } finally {
            java.util.Locale.setDefault(old)
        }
    }
}
