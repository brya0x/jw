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
import dev.jw.next.client.certMatches
import dev.jw.next.client.fromLink
import dev.jw.next.client.pairingCode
import dev.jw.next.client.toFail
import dev.jw.next.contract.BridgeMsg
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.Denied
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
 * The pairing (device secret and where the PC is), encrypted with a key that lives in the
 * Android Keystore and never leaves it (REQ-83). A blob that cannot be read is no pairing.
 */
class LinkStore(context: Context) {
    private val prefs = context.getSharedPreferences("link", Context.MODE_PRIVATE)

    fun load(): Pairing? {
        val blob = prefs.getString("pairing", null) ?: return null
        return try {
            val raw = Base64.decode(blob, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw.copyOfRange(0, IV)))
            Pairing.fromJson(String(cipher.doFinal(raw, IV, raw.size - IV)))
        } catch (_: java.security.GeneralSecurityException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    fun save(pairing: Pairing?) {
        if (pairing == null) {
            prefs.edit().remove("pairing").apply()
            return
        }
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(pairing.toJson().toByteArray())
        prefs.edit().putString("pairing", Base64.encodeToString(sealed, Base64.NO_WRAP)).apply()
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

private enum class Attempt { Unreachable, Ended, Denied }

/**
 * The daemon through the phone link: a WebSocket to the PC (its network address, pinned by
 * fingerprint, then the tunnel), `auth` with the device secret or `pair` with the link's token,
 * and from `ready` the daemon's own protocol. A first pairing is tried once (the token is
 * spent); a paired phone retries for as long as it is open.
 */
class WsLinkSource(
    private val store: LinkStore,
    private var pairing: Pairing?,
    private val link: PairLink?,
    private val deviceName: String = Build.MODEL,
) : ProtocolSource() {
    val ui = mutableStateOf<LinkUi>(LinkUi.Connecting)
    private val code = pairingCode()

    @Volatile
    private var socket: WebSocket? = null

    @Volatile
    private var closed = false
    private val outcome = LinkedBlockingQueue<Attempt>()

    override val status: String
        get() = when (ui.value) {
            is LinkUi.Connected -> "en vivo · por el enlace"
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

    private fun client(c: Candidate): OkHttpClient {
        val builder = OkHttpClient.Builder().connectTimeout(5, TimeUnit.SECONDS).pingInterval(20, TimeUnit.SECONDS)
        val fp = fingerprint()
        if (c.lan && fp != null) {
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

    private fun attempt(c: Candidate): Attempt {
        outcome.clear()
        if (pairing == null) ui.value = LinkUi.Pairing(code, c.url.removePrefix("wss://").trimEnd('/'))
        var opened = false
        val ws = client(c).newWebSocket(
            Request.Builder().url(c.url).build(),
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    opened = true
                    val p = pairing
                    // The contract's own type writes it: nothing a person typed is spliced into JSON by hand.
                    val first = if (p != null) PhoneMsg.Auth(p.device, p.secret) else PhoneMsg.Pair(link!!.token, deviceName, code)
                    webSocket.send(ContractJson.encodeToString(PhoneMsg.serializer(), first))
                }

                override fun onMessage(webSocket: WebSocket, text: String) = message(webSocket, text)

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

    private fun message(ws: WebSocket, text: String) {
        val type = try {
            ContractJson.parseToJsonElement(text).jsonObject["t"]?.jsonPrimitive?.content
        } catch (_: SerializationException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }
        if (type !in BRIDGE) {
            receive(text)
            return
        }
        when (val msg = ContractJson.decodeFromString(BridgeMsg.serializer(), text)) {
            is BridgeMsg.Paired -> {
                val made = fromLink(link!!, msg.device, msg.secret)
                pairing = made
                store.save(made)
            }
            is BridgeMsg.DeniedMsg -> {
                ui.value = LinkUi.Failed(msg.reason.toFail())
                if (msg.reason == Denied.BadSecret || msg.reason == Denied.Revoked) {
                    pairing = null
                    store.save(null)
                }
                outcome.offer(Attempt.Denied)
            }
            is BridgeMsg.Ready -> {
                ui.value = LinkUi.Connected
                greet()
            }
            is BridgeMsg.Endpoints -> pairing?.let {
                val moved = it.withEndpoints(msg.lan, msg.tunnel)
                pairing = moved
                store.save(moved)
            }
        }
    }

    private companion object {
        val BRIDGE = setOf("paired", "denied", "ready", "endpoints")
    }
}
