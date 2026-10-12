package dev.jw.mobile

import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okio.ByteString

/** What the real bridge does after an error: say it, then close. */
fun WebSocket.fail(code: String) { send(FakeBridge.error(code)); close(1000, null) }

class FakeStore(var device: StoredDevice? = null) : TokenStore {
    var saves = 0
    var failSave = false
    override fun load() = device
    override fun save(device: StoredDevice) { if (failSave) throw IllegalStateException("keystore"); this.device = device; saves++ }
    override fun clear() { device = null }
}

/** A bridge that speaks the documented protocol: [script] decides the answer to the first text message. */
class FakeBridge {
    val server = MockWebServer().apply { start() }
    val received = java.util.concurrent.LinkedBlockingQueue<String>()

    fun endpoint() = Endpoint { server.url("/").toString() }

    /** Binary jw-proto bodies (kind byte onward) the phone sent. */
    val frames = java.util.concurrent.LinkedBlockingQueue<ByteArray>()

    /** Answers to the phone's binary frames (kind byte onward). */
    @Volatile var onFrame: (frame: ByteArray, ws: WebSocket) -> Unit = { _, _ -> }

    /** Answers to messages after the first one. */
    @Volatile var onLater: (text: String, ws: WebSocket) -> Unit = { _, _ -> }

    fun enqueue(script: (hello: String, ws: WebSocket) -> Unit) {
        var first = true
        server.enqueue(
            MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                override fun onMessage(webSocket: WebSocket, text: String) {
                    received.add(text)
                    if (first) { first = false; script(text, webSocket) } else onLater(text, webSocket)
                }
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) { frames.add(bytes.toByteArray()); onFrame(bytes.toByteArray(), webSocket) }
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {}
            }),
        )
    }

    fun shutdown() = server.shutdown()

    companion object {
        const val DEVICE = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0"
        fun ready(protocol: Int = 21, ro: Boolean = false) =
            """{"t":"ready","bridge":1,"protocol":$protocol,"read_only":$ro}"""
        fun error(code: String) = """{"t":"error","code":"$code","msg":"m"}"""
        /** A kind-0 frame, as the relay carries `Panes`. */
        fun frame(json: String): ByteString = ByteString.of(0, *json.toByteArray())
        /** A kind-1 frame: a JSON head and the raw bytes, as `Snapshot` and `Output` come. */
        fun raw(head: String, bytes: ByteArray): ByteString {
            val h = head.toByteArray()
            return ByteString.of(1, (h.size ushr 24).toByte(), (h.size ushr 16).toByte(), (h.size ushr 8).toByte(), h.size.toByte(), *h, *bytes)
        }
        const val WORKSPACES = """{"t":"workspaces","session":"main","items":[{"id":"r1","kind":"root","name":"jw","path":"/p","project":"jw","branch":"main","pr":null,"session":"main","parent":null}]}"""
        const val SESSIONS = """{"t":"sessions","items":[{"name":"main","workspaces":1}]}"""
        fun panes(agent: String) = """{"t":"Panes","panes":[{"pane":7,"stream":"r1","role":"agent","exited":null,"fg":null,"busy":false,"bell":false,"agent":"$agent","cwd":null}]}"""
        val PAIRED = """{"t":"paired","device":"$DEVICE","id":"abcd1234"}"""
    }
}
