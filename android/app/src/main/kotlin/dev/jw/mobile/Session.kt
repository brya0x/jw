package dev.jw.mobile

import dev.jw.proto.Decision
import dev.jw.proto.ErrorCode
import dev.jw.proto.Failure
import dev.jw.proto.Hello
import dev.jw.proto.PROTOCOL
import dev.jw.proto.Palettes
import dev.jw.proto.QrPayload
import dev.jw.proto.ReconnectPolicy
import dev.jw.proto.ServerMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.takeWhile
import kotlinx.coroutines.launch
import java.security.SecureRandom

/** What the screens show. None of these carries a token or a pairing code except [Pairing]'s code. */
sealed interface UiState {
    /** No stored device: scan the QR. `notice` says why we are back here, if we are. */
    class Scan(val notice: String? = null) : UiState

    /** Waiting for the person at the PC to compare `code`. */
    class Pairing(val host: String, val code: String) : UiState

    class Connecting(val host: String) : UiState

    class Connected(
        val host: String,
        val protocol: Int,
        val readOnly: Boolean,
        val palettes: Palettes,
    ) : UiState

    /** The PC's jw speaks `pc`, this app speaks `app`: say what to update, do not attach. */
    class Mismatch(val host: String, val app: Int, val pc: Int) : UiState

    /** Down, retrying. `code` is the bridge's error code when there was one (`daemon`, `protocol`, `limited`). */
    class Reconnecting(val host: String, val inMs: Long, val code: String?) : UiState
}

/** Where wss URLs come from; tests point it at MockWebServer. */
fun interface Endpoint {
    fun url(host: String): String
}

/**
 * Pairs, stays connected and decides what happens after each failure. The policy is pure
 * ([ReconnectPolicy]); this class owns the I/O around it.
 */
class Session(
    private val scope: CoroutineScope,
    private val store: TokenStore,
    private val bridge: Bridge,
    private val deviceName: String,
    private val endpoint: Endpoint = Endpoint { "wss://$it/" },
    private val policy: () -> ReconnectPolicy = { ReconnectPolicy() },
    private val clock: () -> Long = System::currentTimeMillis,
    private val random: SecureRandom = SecureRandom(),
) {
    private val _state = MutableStateFlow<UiState>(UiState.Scan())
    val state: StateFlow<UiState> = _state.asStateFlow()
    private var job: Job? = null

    /** App start: connect with the stored device, or ask for a scan. */
    fun start() {
        val dev = store.load()
        if (dev == null) {
            _state.value = UiState.Scan()
        } else {
            _state.value = UiState.Connecting(dev.host)
            run(dev.host, Hello.Auth(dev.device))
        }
    }

    /** A QR was read. `raw` is never echoed anywhere. */
    fun onQr(raw: String): Boolean {
        val qr = QrPayload.parse(raw) ?: return false
        val code = "%06d".format(random.nextInt(1_000_000))
        _state.value = UiState.Pairing(qr.host, code)
        run(qr.host, Hello.Pair(qr.token, deviceName, code))
        return true
    }

    /** Forget this PC and go back to scanning. */
    fun forget() {
        job?.cancel()
        store.clear()
        _state.value = UiState.Scan()
    }

    /** The app is leaving the foreground for good. */
    fun stop() {
        job?.cancel()
    }

    private fun run(host: String, first: Hello) {
        job?.cancel()
        job = scope.launch { loop(host, first) }
    }

    private suspend fun loop(host: String, first: Hello) {
        val p = policy()
        var hello = first
        while (true) {
            var failure: Failure = Failure.Network
            var bridgeFailed = false
            var pairing = hello is Hello.Pair
            var mismatch = false
            bridge.connect(endpoint.url(host), hello).takeWhile { ev ->
                when (ev) {
                    is BridgeEvent.Binary -> Unit
                    is BridgeEvent.Ended -> if (!bridgeFailed) failure = ev.failure
                    is BridgeEvent.Message -> when (val m = ev.msg) {
                        is ServerMessage.Paired -> {
                            // The pairing token is single-use: keep the device before anything else.
                            store.save(StoredDevice(host, m.device))
                            hello = Hello.Auth(m.device)
                            pairing = false
                            _state.value = UiState.Connecting(host)
                        }
                        is ServerMessage.Ready -> {
                            if (m.protocol != PROTOCOL) {
                                _state.value = UiState.Mismatch(host, PROTOCOL, m.protocol)
                                mismatch = true
                            } else {
                                p.onConnected()
                                _state.value = UiState.Connected(host, m.protocol, m.readOnly, m.palettes)
                            }
                        }
                        is ServerMessage.Error -> {
                            // These two leave the connection open; anything else is followed by a close.
                            if (m.code != ErrorCode.READ_ONLY && m.code != ErrorCode.UNSUPPORTED) {
                                failure = Failure.Bridge(m.code)
                                bridgeFailed = true
                            }
                        }
                        is ServerMessage.Other -> Unit
                    }
                }
                !mismatch
            }.collect()
            if (mismatch) return
            if (pairing) {
                // A failed pairing is never retried with the same token: each try counts against REQ-5.
                if (failure is Failure.Dns) {
                    when (val d = p.onFailure(failure, clock())) {
                        is Decision.Retry -> { delay(d.afterMs); continue }
                        else -> Unit
                    }
                }
                _state.value = UiState.Scan(pairNotice(failure))
                return
            }
            when (val d = p.onFailure(failure, clock())) {
                is Decision.Retry -> {
                    _state.value = UiState.Reconnecting(host, d.afterMs, (failure as? Failure.Bridge)?.code)
                    delay(d.afterMs)
                }
                Decision.Rescan -> {
                    store.clear()
                    _state.value = UiState.Scan(
                        if ((failure as? Failure.Bridge)?.code == ErrorCode.REVOKED) {
                            "Este teléfono fue quitado en el PC. Escanea de nuevo."
                        } else {
                            "El PC ya no reconoce este teléfono. Escanea de nuevo."
                        },
                    )
                    return
                }
                Decision.GiveUpDns -> {
                    _state.value = UiState.Scan("No se pudo resolver $host. Revisa la red y escanea de nuevo.")
                    return
                }
            }
        }
    }

    private fun pairNotice(f: Failure): String = when {
        f is Failure.Bridge && f.code == ErrorCode.EXPIRED -> "El QR caducó. Genera otro en el PC."
        f is Failure.Bridge && f.code == ErrorCode.DENIED -> "El PC rechazó el emparejamiento."
        f is Failure.Bridge && f.code == ErrorCode.LIMITED -> "Demasiados intentos. Espera 5 minutos."
        f is Failure.Bridge && f.code == ErrorCode.TIMEOUT -> "El PC no respondió a tiempo."
        f is Failure.Bridge && f.code == ErrorCode.UNKNOWN -> "El QR no es válido o ya se usó. Genera otro."
        f is Failure.Dns -> "No se pudo resolver el túnel. Revisa la red y escanea de nuevo."
        else -> "No se pudo emparejar. Genera otro QR e inténtalo de nuevo."
    }
}
