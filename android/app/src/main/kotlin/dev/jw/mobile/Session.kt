package dev.jw.mobile

import dev.jw.proto.AnswerChoices
import dev.jw.proto.AnswerRule
import dev.jw.proto.Control
import dev.jw.proto.Decision
import dev.jw.proto.ErrorCode
import dev.jw.proto.Failure
import dev.jw.proto.Hello
import dev.jw.proto.PROTOCOL
import dev.jw.proto.PaneInfo
import dev.jw.proto.RefreshRule
import dev.jw.proto.SessionInfo
import dev.jw.proto.Workspace
import dev.jw.proto.Workspaces
import dev.jw.proto.Palettes
import dev.jw.proto.QrPayload
import dev.jw.proto.ReconnectPolicy
import dev.jw.proto.ServerMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.update
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
        /** False when the device token could not be stored: pairing will not survive a restart. */
        val saved: Boolean = true,
    ) : UiState

    /** The PC's jw speaks `pc`, this app speaks `app`: say what to update, do not attach. */
    class Mismatch(val host: String, val app: Int, val pc: Int) : UiState

    /** Down, retrying. `code` is the bridge's error code when there was one (`daemon`, `protocol`, `limited`). */
    class Reconnecting(
        val host: String,
        val inMs: Long,
        val code: String?,
        /** Several network failures in a row: the PC is probably gone, offer to scan a new code. */
        val unreachable: Boolean = false,
    ) : UiState
}

/** What the PC last told us (M4): sessions, the current session's workspaces and the panes of `List`. */
data class Model(
    val sessions: List<SessionInfo> = emptyList(),
    val session: String? = null,
    val items: List<Workspace> = emptyList(),
    val panes: List<PaneInfo> = emptyList(),
    /** When `panes` arrived, by the session's clock; null before the first answer. */
    val panesAt: Long? = null,
    /** Panes already answered with y or n since the latest `Panes`. */
    val answered: Set<Long> = emptySet(),
)

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
    /** Ids of the configured answer bytes (Settings); the defaults are Enter and Esc. */
    private val answerIds: () -> Pair<String?, String?> = { null to null },
    private val pollMs: Long = 2_000,
    private val tickMs: Long = 250,
) {
    private val _state = MutableStateFlow<UiState>(UiState.Scan())
    val state: StateFlow<UiState> = _state.asStateFlow()
    private var job: Job? = null

    private val _model = MutableStateFlow(Model())
    val model: StateFlow<Model> = _model.asStateFlow()
    private val _toasts = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val toasts: SharedFlow<String> = _toasts.asSharedFlow()

    @Volatile private var outbox: Channel<Outgoing>? = null
    @Volatile private var visible = false
    @Volatile private var foreground = true
    private val refresh = RefreshRule(pollMs)
    private var poller: Job? = null

    /** The workspaces, avisos or workspace screen is on screen (the list is what is being watched). */
    fun setVisible(v: Boolean) { visible = v }

    /** The activity is resumed. */
    fun setForeground(f: Boolean) { foreground = f }

    private fun send(o: Outgoing) { outbox?.trySend(o) }

    /** Ask for the workspaces of `session` (null: the PC's default, `main`). */
    fun requestWorkspaces(session: String? = null) = send(Outgoing.Text(Control.workspaces(session)))

    fun requestSessions() = send(Outgoing.Text(Control.sessions()))

    /**
     * y or n for `pane`. Sends once, and only while the latest `Panes` (at most 2 s old) says Waiting
     * for that pane (REQ-20); otherwise tells the person it is no longer waiting.
     */
    fun answer(pane: Long, approve: Boolean) {
        val c = state.value as? UiState.Connected ?: return
        val m = _model.value
        val p = m.panes.firstOrNull { it.pane == pane }
        if (!AnswerRule.enabled(p, m.panesAt, clock(), c.readOnly, pane in m.answered)) {
            _toasts.tryEmit(if (c.readOnly) "solo lectura" else "ya no espera")
            return
        }
        val (a, r) = answerIds()
        val bytes = (if (approve) AnswerChoices.approve(a) else AnswerChoices.reject(r)).bytes
        _model.update { it.copy(answered = it.answered + pane) }
        send(Outgoing.Binary(Workspaces.inputFrame(pane, bytes)))
        _toasts.tryEmit(if (approve) "aprobado" else "rechazado")
    }

    private fun startPoller() {
        if (poller?.isActive == true) return
        poller = scope.launch {
            while (true) {
                delay(tickMs)
                if (refresh.due(clock(), visible && foreground && outbox != null)) send(Outgoing.Binary(Workspaces.listFrame()))
            }
        }
    }

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
        // The camera keeps decoding the same QR: a pairing in flight must not be restarted (the token is single-use).
        if (_state.value !is UiState.Scan) return true
        val code = random.nextInt(1_000_000).toString().padStart(6, '0')
        _state.value = UiState.Pairing(qr.host, code)
        run(qr.host, Hello.Pair(qr.token, deviceName, code))
        return true
    }

    /** Forget this PC and go back to scanning. */
    fun forget() {
        job?.cancel()
        outbox = null
        _model.value = Model()
        store.clear()
        _state.value = UiState.Scan()
    }

    /**
     * Scan a different code. The stored device is kept until a new pairing succeeds, so a PC that comes back
     * at the same address still works. The long-term token never goes to a host learned from a QR.
     */
    fun scanAnother() {
        job?.cancel()
        _state.value = UiState.Scan()
    }

    /** The app is leaving the foreground for good. */
    fun stop() {
        job?.cancel()
        poller?.cancel()
    }

    private fun run(host: String, first: Hello) {
        job?.cancel()
        startPoller()
        job = scope.launch { loop(host, first) }
    }

    private suspend fun loop(host: String, first: Hello) {
        val p = policy()
        var hello = first
        var saved = true
        while (true) {
            var failure: Failure = Failure.Network
            var bridgeFailed = false
            var pairing = hello is Hello.Pair
            var mismatch = false
            val out = Channel<Outgoing>(Channel.UNLIMITED)
            bridge.connect(endpoint.url(host), hello, out).takeWhile { ev ->
                when (ev) {
                    is BridgeEvent.Binary -> Workspaces.parsePanes(ev.frame)?.let { ps ->
                        _model.update { it.copy(panes = ps, panesAt = clock(), answered = emptySet()) }
                    }
                    is BridgeEvent.Ended -> if (!bridgeFailed) failure = ev.failure
                    is BridgeEvent.Message -> when (val m = ev.msg) {
                        is ServerMessage.Paired -> {
                            // The pairing token is single-use: keep the device before anything else.
                            // If saving fails the device stays in memory: keep connecting, but warn it will not survive a restart.
                            saved = try { store.save(StoredDevice(host, m.device)); true } catch (_: Exception) { false }
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
                                _state.value = UiState.Connected(host, m.protocol, m.readOnly, m.palettes, saved)
                                // A new connection: whatever the PC says now is the truth (REQ-15).
                                outbox = out
                                out.trySend(Outgoing.Text(Control.sessions()))
                                out.trySend(Outgoing.Text(Control.workspaces(_model.value.session)))
                                out.trySend(Outgoing.Binary(Workspaces.listFrame()))
                            }
                        }
                        is ServerMessage.Error -> {
                            // These two leave the connection open; anything else is followed by a close.
                            if (m.code == ErrorCode.READ_ONLY) _toasts.tryEmit("solo lectura")
                            if (m.code != ErrorCode.READ_ONLY && m.code != ErrorCode.UNSUPPORTED) {
                                failure = Failure.Bridge(m.code)
                                bridgeFailed = true
                            }
                        }
                        is ServerMessage.Workspaces -> _model.update { it.copy(session = m.session, items = m.items) }
                        is ServerMessage.Sessions -> _model.update { it.copy(sessions = m.items) }
                        is ServerMessage.Other -> Unit
                    }
                }
                !mismatch
            }.collect()
            outbox = null
            out.close()
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
                    _state.value = UiState.Reconnecting(host, d.afterMs, (failure as? Failure.Bridge)?.code, p.unreachable)
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
