package dev.jw.mobile

import dev.jw.proto.Control
import dev.jw.proto.Failure
import dev.jw.proto.Frame
import dev.jw.proto.FrameCodec
import dev.jw.proto.Hello
import dev.jw.proto.ServerMessage
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.launch
import okio.ByteString.Companion.toByteString
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import dev.jw.proto.LanAddr
import java.net.InetSocketAddress
import java.net.Socket
import java.net.UnknownHostException
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509TrustManager

/** What one connection to the bridge produces, in order. Never carries a token in `toString`. */
sealed interface BridgeEvent {
    class Message(val msg: ServerMessage) : BridgeEvent

    /** The WebSocket opened: something answered at the host. */
    data object Open : BridgeEvent

    /** A binary jw-proto frame after `ready`. M3 does not use them. */
    class Binary(val frame: Frame) : BridgeEvent

    /** The connection ended; `failure` says how. The flow completes right after. */
    class Ended(val failure: Failure) : BridgeEvent
}

/** What the app sends after `ready`: a control message (text) or a jw-proto frame (binary). */
sealed interface Outgoing {
    class Text(val text: String) : Outgoing
    class Binary(val frame: Frame) : Outgoing
}

/**
 * Trusts exactly one certificate: the one whose SHA-256 is `fp` (REQ-59). The PC's wifi certificate is self-signed
 * and carries no address, so the pin is its whole identity; the chain, the name and the dates are not looked at.
 * Used only for a wifi address, never for a tunnel host, which keeps the system's checks.
 */
class PinnedTrust(private val fp: String) : X509TrustManager {
    override fun checkClientTrusted(chain: Array<X509Certificate>?, authType: String?) =
        throw CertificateException("this app does not authenticate clients")

    override fun checkServerTrusted(chain: Array<X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("no certificate")
        val got = MessageDigest.getInstance("SHA-256").digest(leaf.encoded).joinToString("") { "%02x".format(it) }
        if (!MessageDigest.isEqual(got.toByteArray(), fp.toByteArray())) throw CertificateException("not the pinned certificate")
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    fun context(): SSLContext = SSLContext.getInstance("TLS").apply { init(null, arrayOf(this@PinnedTrust), null) }
}

/** The WebSocket client. No Android classes, so the unit tests can run it against MockWebServer. */
class Bridge(
    // The PC may take about a minute to answer a pairing: no read timeout. The bridge pings.
    private val client: OkHttpClient = OkHttpClient.Builder()
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .connectTimeout(20, TimeUnit.SECONDS)
        .build(),
) {
    /**
     * Opens `url` (`wss://host/`), sends [hello] as the first text message, and emits what comes
     * back. Cancelling the collector closes the socket. No `Origin` header is ever sent: OkHttp
     * adds none, and the bridge refuses a handshake that carries one.
     */
    fun connect(
        url: String,
        hello: Hello,
        outbox: ReceiveChannel<Outgoing>? = null,
        /** The fingerprint a wifi address must present (REQ-59); null for a tunnel host. */
        pin: String? = null,
        /** The attempt ends as a network failure when nothing has opened by then (a host that takes the
         * connection and says nothing would hold it for ever, since reads have no timeout). */
        openWithinMs: Long? = null,
    ): Flow<BridgeEvent> = callbackFlow {
        val request = Request.Builder().url(url).build()
        var ended = false
        var opened = false
        fun end(f: Failure) {
            if (ended) return
            ended = true
            trySend(BridgeEvent.Ended(f))
            close()
        }
        // The wifi client is its own: it trusts one certificate and does not look at the name. The
        // tunnel never goes through it.
        val http = if (pin == null) client else pinnedClient(pin)
        val ws = http.newWebSocket(
            request,
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    opened = true
                    trySend(BridgeEvent.Open)
                    webSocket.send(Control.encode(hello))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    val msg = Control.parse(text) ?: return
                    trySend(BridgeEvent.Message(msg))
                }

                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                    val frame = try {
                        FrameCodec.decode(bytes.toByteArray())
                    } catch (_: Exception) {
                        return
                    }
                    trySend(BridgeEvent.Binary(frame))
                }

                override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                    webSocket.close(1000, null)
                    end(Failure.Network)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    end(Failure.Network)
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    end(
                        when {
                            t is UnknownHostException -> Failure.Dns
                            response != null -> Failure.Http(response.code)
                            else -> Failure.Network
                        },
                    )
                }
            },
        )
        if (outbox != null) {
            launch {
                for (o in outbox) {
                    when (o) {
                        is Outgoing.Text -> ws.send(o.text)
                        is Outgoing.Binary -> ws.send(FrameCodec.encode(o.frame).toByteString())
                    }
                }
            }
        }
        if (openWithinMs != null) {
            launch {
                kotlinx.coroutines.delay(openWithinMs)
                if (!opened) {
                    end(Failure.Network)
                    ws.cancel()
                }
            }
        }
        awaitClose { ws.cancel() }
    }

    private fun pinnedClient(fp: String): OkHttpClient {
        val trust = PinnedTrust(fp)
        return client.newBuilder()
            .sslSocketFactory(trust.context().socketFactory, trust)
            // The certificate has no address in it: the pin is what says it is the PC.
            .hostnameVerifier { _, _ -> true }
            .build()
    }

    /**
     * Whether a wifi address answers with the pinned certificate: a TCP connection and a TLS handshake, nothing
     * sent after it, so no token and no seat is used for more than a moment (REQ-63).
     */
    fun probe(lan: LanAddr, timeoutMs: Long): Boolean = try {
        val trust = PinnedTrust(lan.fp)
        Socket().use { raw ->
            raw.connect(InetSocketAddress(lan.host, lan.port), timeoutMs.toInt())
            raw.soTimeout = timeoutMs.toInt()
            (trust.context().socketFactory.createSocket(raw, lan.host, lan.port, true) as SSLSocket).use { it.startHandshake() }
        }
        true
    } catch (_: Exception) {
        false
    }
}
