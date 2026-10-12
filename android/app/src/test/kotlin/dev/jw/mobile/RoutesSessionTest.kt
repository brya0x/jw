package dev.jw.mobile

import dev.jw.proto.Hosts
import dev.jw.proto.LanAddr
import dev.jw.proto.ReconnectPolicy
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.SocketPolicy
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import okio.ByteString
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.MessageDigest

/** Wifi first, the tunnel as the fallback (REQ-59 to REQ-64). The wifi is a second fake bridge. */
class RoutesSessionTest {
    private val tunnel = FakeBridge()
    private val wifi = FakeBridge()
    private val store = FakeStore()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val token = "0123456789abcdef0123456789abcdef"
    private val fp = "c".repeat(64)
    private val lan = LanAddr("192.168.1.23", 48211, fp)
    private val host = "example.trycloudflare.com"

    private fun session(
        wifiUrl: () -> String = { wifi.endpoint().url("x") },
        net: () -> Net? = { null },
        policy: () -> ReconnectPolicy = { ReconnectPolicy(baseMs = 50, maxMs = 100, dnsRetryMs = 50) },
        wifiReturnMs: Long = 60_000,
        probe: ((LanAddr) -> Boolean)? = null,
    ) = Session(
        scope, store, Bridge(), "Pixel", tunnel.endpoint(), policy,
        network = net, lanOpenMs = 600, wifiReturnMs = wifiReturnMs, lanEndpoint = { wifiUrl() },
        probeLan = probe ?: { Bridge().probe(it, 600) },
    )

    private suspend inline fun <reified T : UiState> Session.await(): T =
        withTimeout(20_000) { state.first { it is T } as T }

    @After fun tearDown() { scope.cancel(); tunnel.shutdown(); runCatching { wifi.shutdown() } }

    private fun ready(f: FakeBridge) = f.enqueue { _, ws -> ws.send(FakeBridge.ready()) }

    @Test fun theWifiIsTriedFirstAndTheTunnelIsLeftAlone() = runBlocking<Unit> {
        store.device = StoredDevice(host, FakeBridge.DEVICE, lan)
        ready(wifi)
        val s = session(); s.start()
        val c = s.await<UiState.Connected>()
        assertEquals(Route.Wifi, c.route)
        assertEquals(0, tunnel.server.requestCount)
        assertEquals("192.168.1.23:48211", c.lan)
        assertEquals(host, c.tunnel)
    }

    @Test fun aWifiThatDoesNotAnswerFallsToTheTunnel() = runBlocking<Unit> {
        store.device = StoredDevice(host, FakeBridge.DEVICE, lan)
        wifi.shutdown()
        ready(tunnel)
        val s = session(); s.start()
        assertEquals(Route.Tunnel, s.await<UiState.Connected>().route)
    }

    @Test fun aWifiThatTakesTheConnectionAndSaysNothingIsGivenUpOn() = runBlocking<Unit> {
        store.device = StoredDevice(host, FakeBridge.DEVICE, lan)
        // Accepts the socket and never upgrades it: reads have no timeout, so only the open deadline ends it.
        wifi.server.enqueue(MockResponse().setSocketPolicy(SocketPolicy.NO_RESPONSE))
        ready(tunnel)
        val s = session(); s.start()
        assertEquals(Route.Tunnel, s.await<UiState.Connected>().route)
    }

    @Test fun aPairingTokenAlreadySentOnTheWifiIsNeverSentAgainOnTheTunnel() = runBlocking<Unit> {
        wifi.enqueue { _, ws -> ws.fail("expired") }
        val s = session(); s.start()
        assertTrue(s.onQr("jw://192.168.1.23:48211/$token?fp=$fp&t=$host"))
        val scan = s.await<UiState.Scan>()
        assertEquals(ScanWhy.EXPIRED, scan.why)
        assertEquals("the token was spent on the wifi", 0, tunnel.server.requestCount)
    }

    @Test fun aPairingThatCouldNotOpenTheWifiGoesToTheTunnelWithTheSameToken() = runBlocking<Unit> {
        wifi.shutdown()
        tunnel.enqueue { hello, ws -> ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready()) }
        val s = session(); s.start()
        assertTrue(s.onQr("jw://192.168.1.23:48211/$token?fp=$fp&t=$host"))
        val c = s.await<UiState.Connected>()
        assertEquals(Route.Tunnel, c.route)
        assertTrue(tunnel.received.take().contains(token))
        // Both addresses are kept for the next time.
        assertEquals(host, store.device!!.host)
        assertEquals(lan, store.device!!.lan)
    }

    @Test fun aWifiOnlyQrPairsWithNoTunnelYet() = runBlocking<Unit> {
        wifi.enqueue { _, ws -> ws.send(FakeBridge.PAIRED); ws.send(FakeBridge.ready()) }
        val s = session(); s.start()
        assertTrue(s.onQr("jw://192.168.1.23:48211/$token?fp=$fp"))
        val c = s.await<UiState.Connected>()
        assertEquals(Route.Wifi, c.route)
        assertEquals("", store.device!!.host)
        assertEquals(lan, store.device!!.lan)
        assertEquals(0, tunnel.server.requestCount)
    }

    @Test fun endpointsAfterReadyAreKeptWhenValid() = runBlocking<Unit> {
        store.device = StoredDevice("", FakeBridge.DEVICE, lan)
        val newLan = """{"host":"192.168.1.40","port":50000,"fp":"${"d".repeat(64)}"}"""
        wifi.enqueue { _, ws ->
            ws.send(FakeBridge.ready())
            ws.send("""{"t":"endpoints","lan":$newLan,"tunnel":"new-one.trycloudflare.com"}""")
        }
        val s = session(); s.start()
        s.await<UiState.Connected>()
        withTimeout(10_000) { while (store.device!!.host != "new-one.trycloudflare.com") delay(20) }
        assertEquals("192.168.1.40:50000", store.device!!.lan!!.hostPort)
        assertEquals(FakeBridge.DEVICE, store.device!!.device)
        val c = withTimeout(10_000) { s.state.first { it is UiState.Connected && it.tunnel != null } as UiState.Connected }
        assertEquals("new-one.trycloudflare.com", c.tunnel)
    }

    @Test fun endpointsThatAreNotValidAreNotTaken() = runBlocking<Unit> {
        store.device = StoredDevice("", FakeBridge.DEVICE, lan)
        wifi.enqueue { _, ws ->
            ws.send(FakeBridge.ready())
            // A host the token must never go to, and a public address: both dropped.
            ws.send("""{"t":"endpoints","lan":{"host":"8.8.8.8","port":1,"fp":"${"d".repeat(64)}"},"tunnel":"evil.example.com"}""")
        }
        val s = session(); s.start()
        s.await<UiState.Connected>()
        delay(500)
        assertEquals("", store.device!!.host)
        // `lan` is null in what arrived, so the stored address is replaced by nothing only when the PC says so:
        // an invalid entry is dropped as if the PC had named none.
        assertNull(store.device!!.lan?.takeIf { it.host == "8.8.8.8" })
    }

    @Test fun endpointsBeforeReadyAreIgnored() = runBlocking<Unit> {
        store.device = StoredDevice("", FakeBridge.DEVICE, lan)
        wifi.enqueue { _, ws ->
            ws.send("""{"t":"endpoints","lan":null,"tunnel":"early.trycloudflare.com"}""")
            ws.send(FakeBridge.ready())
        }
        val s = session(); s.start()
        s.await<UiState.Connected>()
        delay(300)
        assertEquals("", store.device!!.host)
    }

    @Test fun theWifiRowIsOnlyClaimedWhenTheWifiWasTried() = runBlocking<Unit> {
        // Tried and silent: the row is false. Never held: no row.
        store.device = StoredDevice(host, FakeBridge.DEVICE, lan)
        wifi.shutdown()
        val dead = Session(
            scope, store, Bridge(), "Pixel", Endpoint { "ws://127.0.0.1:1/" },
            { ReconnectPolicy(baseMs = 20, maxMs = 20) }, lanOpenMs = 300, lanEndpoint = { "ws://127.0.0.1:1/" },
            network = { Net(true, true, false) },
        )
        dead.start()
        val r = withTimeout(20_000) { dead.state.first { it is UiState.Reconnecting && it.unreachable } as UiState.Reconnecting }
        assertEquals(false, r.reach.wifi)
        dead.stop()
        store.device = StoredDevice(host, FakeBridge.DEVICE, null)
        val noWifi = Session(
            scope, store, Bridge(), "Pixel", Endpoint { "ws://127.0.0.1:1/" },
            { ReconnectPolicy(baseMs = 20, maxMs = 20) },
        )
        noWifi.start()
        val r2 = withTimeout(20_000) { noWifi.state.first { it is UiState.Reconnecting && it.unreachable } as UiState.Reconnecting }
        assertNull(r2.reach.wifi)
        noWifi.stop()
    }

    @Test fun backOnTheWifiTheConnectionMovesWithoutGoingThroughReconnecting() = runBlocking<Unit> {
        // The wifi is a TLS server whose certificate is the pin. The stored pin is wrong at first (as when the
        // PC changed), so the app lands on the tunnel; the tunnel then says the right one (REQ-61), and the
        // app, which is on wifi, finds the PC there and moves.
        val cert = HeldCertificate.Builder().commonName("jw mobile").build()
        val good = MessageDigest.getInstance("SHA-256").digest(cert.certificate.encoded).joinToString("") { "%02x".format(it) }
        val tls = MockWebServer().apply {
            useHttps(HandshakeCertificates.Builder().heldCertificate(cert).build().sslSocketFactory(), false)
            enqueue(MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                override fun onMessage(webSocket: WebSocket, text: String) { webSocket.send(FakeBridge.ready()) }
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {}
            }))
            start()
        }
        try {
            // The address is a private one, as a real PC's is; the test points the connection and the probe at the TLS fake.
            val addr = { f: String -> LanAddr("192.168.1.23", 48211, f) }
            store.device = StoredDevice(host, FakeBridge.DEVICE, addr("e".repeat(64)))
            tunnel.enqueue { _, ws ->
                ws.send(FakeBridge.ready())
                ws.send("""{"t":"endpoints","lan":{"host":"192.168.1.23","port":48211,"fp":"$good"},"tunnel":"$host"}""")
            }
            val s = session(
                wifiUrl = { "wss://127.0.0.1:${tls.port}/" }, net = { Net(true, false, true) }, wifiReturnMs = 300,
                probe = { Bridge().probe(LanAddr("127.0.0.1", tls.port, it.fp), 1_000) },
            )
            s.start()
            assertEquals(Route.Tunnel, s.await<UiState.Connected>().route)
            val seen = mutableListOf<UiState>()
            val collect = scope.launch { s.state.collect { seen += it } }
            val c = withTimeout(20_000) { s.state.first { it is UiState.Connected && it.route == Route.Wifi } as UiState.Connected }
            assertEquals(Route.Wifi, c.route)
            collect.cancel()
            assertTrue("it moved without a reconnecting screen", seen.none { it is UiState.Reconnecting })
        } finally {
            tls.shutdown()
        }
    }
}
