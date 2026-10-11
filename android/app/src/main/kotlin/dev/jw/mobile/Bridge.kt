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
import java.net.UnknownHostException
import java.util.concurrent.TimeUnit

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
    fun connect(url: String, hello: Hello, outbox: ReceiveChannel<Outgoing>? = null): Flow<BridgeEvent> = callbackFlow {
        val request = Request.Builder().url(url).build()
        var ended = false
        fun end(f: Failure) {
            if (ended) return
            ended = true
            trySend(BridgeEvent.Ended(f))
            close()
        }
        val ws = client.newWebSocket(
            request,
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
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
        awaitClose { ws.cancel() }
    }
}
