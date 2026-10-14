package dev.jw.next.app

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.compose.runtime.mutableStateOf
import dev.jw.next.client.Candidate
import dev.jw.next.client.LinkFail
import dev.jw.next.client.PairLink
import dev.jw.next.client.Pairing
import dev.jw.next.client.PcList
import dev.jw.next.client.certMatches
import dev.jw.next.client.cleanHost
import dev.jw.next.client.fromLink
import dev.jw.next.client.pairingCode
import dev.jw.next.client.toFail
import dev.jw.next.contract.BridgeMsg
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.PhoneMsg
import java.security.KeyStore
import java.security.cert.X509Certificate
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/** What the link screens show while a phone pairs or reconnects. */
sealed interface LinkUi {
    data object Connecting : LinkUi
    data class Pairing(val code: String, val host: String) : LinkUi
    data class Failed(val fail: LinkFail) : LinkUi
    data object Connected : LinkUi
}

/**
 * The PCs this phone is paired with (device secrets and where each PC was last seen),
 * encrypted with a key that lives in the Android Keystore and never leaves it (REQ-83). A blob
 * that cannot be read is no pairing.
 */
class LinkStore(context: Context) {
    private val prefs = context.getSharedPreferences("link", Context.MODE_PRIVATE)

    @Synchronized
    fun load(): PcList {
        read("pcs")?.let { PcList.fromJson(it) }?.let { return it }
        // A phone that paired before there could be several holds one pairing.
        return read("pairing")?.let { Pairing.fromJson(it) }?.let(PcList::ofOne) ?: PcList()
    }

    /** Change the list under one lock, so a pairing and a reconnecting phone never overwrite each other. */
    @Synchronized
    fun update(change: (PcList) -> PcList): PcList {
        val next = change(load())
        val sealed = Cipher.getInstance(TRANSFORM).run {
            init(Cipher.ENCRYPT_MODE, key())
            iv + doFinal(next.toJson().toByteArray())
        }
        prefs.edit().putString("pcs", Base64.encodeToString(sealed, Base64.NO_WRAP)).remove("pairing").apply()
        return next
    }

    private fun read(name: String): String? {
        val blob = prefs.getString(name, null) ?: return null
        return try {
            val raw = Base64.decode(blob, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw.copyOfRange(0, IV)))
            String(cipher.doFinal(raw, IV, raw.size - IV))
        } catch (_: java.security.GeneralSecurityException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    private fun key(): SecretKey {
        val keys = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keys.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val ALIAS = "jwn-link"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV = 12
    }
}

/** An HTTP client for one way of reaching a PC: the network address is trusted only by its pinned fingerprint. */
internal fun linkClient(candidate: Candidate, fp: String?): OkHttpClient {
    val builder = OkHttpClient.Builder().connectTimeout(5, TimeUnit.SECONDS).pingInterval(20, TimeUnit.SECONDS)
    if (candidate.lan && fp != null) {
        // Only the fingerprint the QR carried makes the certificate trusted: names and chains do not matter.
        val pin = object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) = throw java.security.cert.CertificateException("no client certificates")
            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                if (chain.isEmpty() || !certMatches(chain[0].encoded, fp)) throw java.security.cert.CertificateException("not the pinned certificate")
            }
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
        }
        val ssl = SSLContext.getInstance("TLS").apply { init(null, arrayOf(pin), null) }
        builder.sslSocketFactory(ssl.socketFactory, pin).hostnameVerifier { _, _ -> true }
    }
    return builder.build()
}

/** The text of a bridge message's `t`, or null when it is not JSON. */
internal fun messageType(text: String): String? = try {
    ContractJson.parseToJsonElement(text).jsonObject["t"]?.jsonPrimitive?.content
} catch (_: SerializationException) {
    null
} catch (_: IllegalArgumentException) {
    null
}

internal val BRIDGE_TYPES = setOf("paired", "denied", "ready", "endpoints")

private enum class Attempt { Unreachable, Ended, Denied }

/**
 * The daemon through the phone link: a WebSocket to the PC (its network address, pinned by
 * fingerprint, then the tunnel), `auth` with the device secret or `pair` with the link's token,
 * and from `ready` the daemon's own protocol. A first pairing is tried once (the token is
 * spent); a paired phone retries for as long as it is open.
 */
class WsLinkSource(
    private val store: LinkStore,
    initial: Pairing?,
    private val link: PairLink?,
    private val deviceName: String = Build.MODEL,
) : ProtocolSource() {
    val ui = mutableStateOf<LinkUi>(LinkUi.Connecting)
    private val code = pairingCode()

    /** Set from the connection's thread and read from the UI's. */
    @Volatile
    private var pairing: Pairing? = initial

    /** The paired PC this source talks to, once there is one. */
    val device: String? get() = pairing?.device

    @Volatile
    private var socket: WebSocket? = null

    @Volatile
    private var closed = false
    private val outcome = LinkedBlockingQueue<Attempt>()

    override val status: String
        get() = when (ui.value) {
            is LinkUi.Connected -> "en vivo · ${pairing?.name ?: "por el enlace"}"
            is LinkUi.Connecting -> "conectando…"
            is LinkUi.Pairing -> "emparejando…"
            is LinkUi.Failed -> "sin conexión"
        }

    init {
        Thread({ run() }, "link-source").apply { isDaemon = true }.start()
    }

    fun close() {
        closed = true
        socket?.cancel()
    }

    override fun sendLine(line: String) {
        socket?.send(line)
    }

    private fun candidates(): List<Candidate> = pairing?.candidates() ?: link?.candidates() ?: emptyList()

    private fun fingerprint(): String? = pairing?.fp ?: link?.fp

    private fun run() {
        while (!closed) {
            var reached = false
            for (c in candidates()) {
                if (closed) return
                when (attempt(c)) {
                    Attempt.Denied -> return
                    Attempt.Ended -> {
                        reached = true
                        break
                    }
                    Attempt.Unreachable -> {}
                }
            }
            if (pairing == null) {
                // The token is spent by the first PC that answers; nothing more to try.
                if (!reached) ui.value = LinkUi.Failed(LinkFail.Unreachable)
                return
            }
            ui.value = LinkUi.Connecting
            dropped()
            Thread.sleep(2000)
        }
    }

    private fun attempt(c: Candidate): Attempt {
        outcome.clear()
        if (pairing == null) ui.value = LinkUi.Pairing(code, c.url.removePrefix("wss://").trimEnd('/'))
        var opened = false
        val ws = linkClient(c, fingerprint()).newWebSocket(
            Request.Builder().url(c.url).build(),
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    opened = true
                    val p = pairing
                    // The contract's own type writes it: nothing a person typed is spliced into JSON by hand.
                    val first = if (p != null) PhoneMsg.Auth(p.device, p.secret) else PhoneMsg.Pair(link!!.token, deviceName, code)
                    webSocket.send(ContractJson.encodeToString(PhoneMsg.serializer(), first))
                }

                override fun onMessage(webSocket: WebSocket, text: String) = message(text)

                override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                    // OkHttp reports the end only once this side has answered the close.
                    webSocket.close(1000, null)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    outcome.offer(if (opened) Attempt.Ended else Attempt.Unreachable)
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    outcome.offer(if (opened) Attempt.Ended else Attempt.Unreachable)
                }
            },
        )
        socket = ws
        val result = outcome.take()
        socket = null
        ws.cancel()
        return result
    }

    private fun message(text: String) {
        if (messageType(text) !in BRIDGE_TYPES) {
            receive(text)
            return
        }
        when (val msg = ContractJson.decodeFromString(BridgeMsg.serializer(), text)) {
            is BridgeMsg.Paired -> {
                val made = fromLink(link!!, msg.device, msg.secret)
                pairing = made
                store.update { it.add(made) }
            }
            is BridgeMsg.DeniedMsg -> {
                // The entry stays in the list whatever the PC says: only the person forgets a PC (REQ-87).
                ui.value = LinkUi.Failed(msg.reason.toFail())
                outcome.offer(Attempt.Denied)
            }
            is BridgeMsg.Ready -> {
                ui.value = LinkUi.Connected
                pairing?.let { p ->
                    val named = p.copy(name = cleanHost(msg.host) ?: p.name)
                    pairing = named
                    store.update { it.update(p.device) { _ -> named }.used(p.device) }
                }
                greet()
            }
            is BridgeMsg.Endpoints -> pairing?.let { p ->
                val moved = p.withEndpoints(msg.lan, msg.tunnel)
                pairing = moved
                store.update { it.update(p.device) { _ -> moved } }
            }
        }
    }
}
