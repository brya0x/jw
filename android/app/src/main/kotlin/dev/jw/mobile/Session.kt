package dev.jw.mobile

import dev.jw.proto.AnswerChoices
import dev.jw.proto.AnswerRule
import dev.jw.proto.Control
import dev.jw.proto.Decision
import dev.jw.proto.ErrorCode
import dev.jw.proto.Failure
import dev.jw.proto.Hello
import dev.jw.proto.PROTOCOL
import dev.jw.proto.Hosts
import dev.jw.proto.LanAddr
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
    /**
     * No stored device (or a pairing that did not work): scan the QR. `notice` says why we are back
     * here, if we are; `why` is the same as a kind the screens switch on, and `host` the one that
     * failed, if there was one.
     */
    class Scan(
        val notice: String? = null,
        val why: ScanWhy? = null,
        val host: String? = null,
        val reach: Reach = Reach(),
    ) : UiState

    /** Waiting for the person at the PC to compare `code`. */
    class Pairing(
        val host: String,
        val code: String,
        val reached: Boolean = false,
        /** The route the code was sent on, once something answered. */
        val via: Route? = null,
    ) : UiState

    class Connecting(val host: String) : UiState

    class Connected(
        val host: String,
        val protocol: Int,
        val readOnly: Boolean,
        val palettes: Palettes,
        /** False when the device token could not be stored: pairing will not survive a restart. */
        val saved: Boolean = true,
        /** The route this connection is on, and the addresses the app holds for both (REQ-64). */
        val route: Route = Route.Tunnel,
        val lan: String? = null,
        val tunnel: String? = null,
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
        val reach: Reach = Reach(),
    ) : UiState
}

/**
 * Why a scan screen is followed by a failure screen (REQ-54). Only five failure screens are
 * approved, so every cause is mapped to the nearest one (`android/SPEC-NOTES.md`).
 */
enum class ScanWhy { DENIED, EXPIRED, LIMITED, UNREACHABLE }

/**
 * What the app measured about a host it could not reach; `null` is "not measured", and the screen
 * draws no row for it (REQ-54).
 */
data class Reach(
    /** The phone has a network (ConnectivityManager), and whether it is mobile data. */
    val internet: Boolean? = null,
    val mobileData: Boolean? = null,
    /** An HTTP answer came from the host's side: Cloudflare is there, the tunnel behind it is not. */
    val cloudflare: Boolean? = null,
    /** The wifi address was tried and did not answer; null when the app holds none (REQ-64). */
    val wifi: Boolean? = null,
)

/** Which way the app reaches the PC: its wifi address, or the tunnel (REQ-63). */
enum class Route { Wifi, Tunnel }

/** The phone's own connectivity, from the system. */
data class Net(val internet: Boolean, val mobile: Boolean, val wifi: Boolean)

/** Where the PC can be reached, as far as this app knows (REQ-62). */
private class Routes(var lan: LanAddr?, var tunnel: String?) {
    /** What the screens call the PC when they must name it. */
    val label get() = tunnel ?: lan?.hostPort ?: ""
}

/** How long an attempt on the wifi gets to open a connection before the tunnel is tried (REQ-63). */
private const val LAN_OPEN_MS = 3_000L

/** How often the app looks again for the wifi while it is on the tunnel (REQ-63). */
private const val WIFI_RETURN_MS = 30_000L

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
    /** The phone's own connectivity; null when unknown. */
    private val network: () -> Net? = { null },
    private val lanOpenMs: Long = LAN_OPEN_MS,
    private val wifiReturnMs: Long = WIFI_RETURN_MS,
    /** Where a wifi address is connected to; tests point it at MockWebServer. */
    private val lanEndpoint: (LanAddr) -> String = { "wss://${it.host}:${it.port}/" },
    /** Whether a wifi address answers with its pin, without sending anything (REQ-63); tests point it at a fake. */
    private val probeLan: (LanAddr) -> Boolean = { bridge.probe(it, lanOpenMs) },
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
    private var routes = Routes(null, null)
    private var lastWifiLook = 0L

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
                lookForWifi()
            }
        }
    }

    /**
     * While on the tunnel with a wifi address held and the phone on wifi, looks every [wifiReturnMs] for the PC
     * there: a handshake against the pin, no token, no failure counted. When it answers the connection moves
     * (REQ-63), and the screens do not go through "reconnecting".
     */
    private suspend fun lookForWifi() {
        val c = _state.value as? UiState.Connected ?: return
        val lan = routes.lan ?: return
        if (c.route != Route.Tunnel || network()?.wifi != true) return
        val now = clock()
        if (now - lastWifiLook < wifiReturnMs) return
        lastWifiLook = now
        val here = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { probeLan(lan) }
        if (here && (_state.value as? UiState.Connected)?.route == Route.Tunnel) {
            val dev = store.load()?.device ?: return
            run(routes, Hello.Auth(dev), fresh = false)
        }
    }

    /** App start: connect with the stored device, or ask for a scan. */
    fun start() {
        val dev = store.load()
        if (dev == null) {
            _state.value = UiState.Scan()
        } else {
            routes = Routes(dev.lan, dev.host.ifEmpty { null })
            _state.value = UiState.Connecting(routes.label)
            run(routes, Hello.Auth(dev.device))
        }
    }

    /** A QR was read. `raw` is never echoed anywhere. */
    fun onQr(raw: String): Boolean {
        val qr = QrPayload.parse(raw) ?: return false
        // The camera keeps decoding the same QR: a pairing in flight must not be restarted (the token is single-use).
        if (_state.value !is UiState.Scan) return true
        val code = random.nextInt(1_000_000).toString().padStart(6, '0')
        val r = Routes(qr.lan, qr.tunnel)
        routes = r
        _state.value = UiState.Pairing(r.label, code)
        run(r, Hello.Pair(qr.token, deviceName, code))
        return true
    }

    /** Forget this PC and go back to scanning. */
    fun forget() {
        job?.cancel()
        outbox = null
        _model.value = Model()
        store.clear()
        routes = Routes(null, null)
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

    private fun run(r: Routes, first: Hello, fresh: Boolean = true) {
        job?.cancel()
        startPoller()
        job = scope.launch { loop(r, first, fresh) }
    }

    private fun reach(f: Failure, wifiFailed: Boolean): Reach {
        val n = network()
        return Reach(n?.internet, n?.mobile, if (f is Failure.Http) true else null, if (wifiFailed) false else null)
    }

    /** What one attempt on one route came to. */
    private class Attempt(
        var failure: Failure = Failure.Network,
        var bridgeFailed: Boolean = false,
        var mismatch: Boolean = false,
        /** The socket opened: whatever is on the other end answered, so a pairing token has been sent. */
        var opened: Boolean = false,
        /** `ready` came on a matching protocol. */
        var ready: Boolean = false,
    )

    private suspend fun loop(r: Routes, first: Hello, fresh: Boolean) {
        val p = policy()
        var hello = first
        var saved = true
        var firstRound = fresh
        while (true) {
            val order = buildList {
                if (r.lan != null) add(Route.Wifi)
                if (r.tunnel != null) add(Route.Tunnel)
            }
            var attempt = Attempt()
            var wifiFailed = false
            for ((i, route) in order.withIndex()) {
                attempt = Attempt()
                val before = hello
                hello = connectOn(route, r, hello, attempt, p, saved).also { saved = it.second }.first
                if (route == Route.Wifi && !attempt.ready && !attempt.opened) wifiFailed = true
                // The next route is tried only when nothing was said on this one: a pairing token that has been
                // sent is spent, and an attempt that reached `ready` was not a failure to reach the PC.
                val tryNext = if (before is Hello.Pair) !attempt.opened else !attempt.ready && !attempt.mismatch && !attempt.bridgeFailed
                if (!tryNext || i == order.lastIndex) break
            }
            outbox = null
            if (attempt.mismatch) return
            val failure = attempt.failure
            // Paired during this round: from here it is an ordinary connection that failed.
            if (hello is Hello.Pair) {
                // A failed pairing is never retried with the same token: each try counts against REQ-5.
                if (failure is Failure.Dns && order.size == 1) {
                    when (val d = p.onFailure(failure, clock())) {
                        is Decision.Retry -> { delay(d.afterMs); continue }
                        else -> Unit
                    }
                }
                _state.value = UiState.Scan(pairNotice(failure), pairWhy(failure), r.label, reach(failure, wifiFailed))
                return
            }
            firstRound = false
            when (val d = p.onFailure(failure, clock())) {
                is Decision.Retry -> {
                    _state.value = UiState.Reconnecting(r.label, d.afterMs, (failure as? Failure.Bridge)?.code, p.unreachable, reach(failure, wifiFailed))
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
                        ScanWhy.DENIED,
                        r.label,
                    )
                    return
                }
                Decision.GiveUpDns -> {
                    _state.value = UiState.Scan("No se pudo resolver ${r.label}. Revisa la red y escanea de nuevo.", ScanWhy.UNREACHABLE, r.label, reach(failure, wifiFailed))
                    return
                }
            }
        }
    }

    /**
     * One connection on `route` until it ends. Returns the hello to use next (`auth` once paired) and whether the
     * device could be stored.
     */
    private suspend fun connectOn(
        route: Route,
        r: Routes,
        hello0: Hello,
        a: Attempt,
        p: ReconnectPolicy,
        saved0: Boolean,
    ): Pair<Hello, Boolean> {
        var hello = hello0
        var saved = saved0
        val lan = r.lan
        val tunnelHost = r.tunnel
        val url = if (route == Route.Wifi) lanEndpoint(lan!!) else endpoint.url(tunnelHost!!)
        val out = Channel<Outgoing>(Channel.UNLIMITED)
        bridge.connect(
            url, hello, out,
            pin = if (route == Route.Wifi) lan!!.fp else null,
            openWithinMs = if (route == Route.Wifi) lanOpenMs else null,
        ).takeWhile { ev ->
            when (ev) {
                BridgeEvent.Open -> {
                    a.opened = true
                    (_state.value as? UiState.Pairing)?.let { _state.value = UiState.Pairing(it.host, it.code, true, route) }
                }
                is BridgeEvent.Binary -> Workspaces.parsePanes(ev.frame)?.let { ps ->
                    _model.update { it.copy(panes = ps, panesAt = clock(), answered = emptySet()) }
                }
                is BridgeEvent.Ended -> if (!a.bridgeFailed) a.failure = ev.failure
                is BridgeEvent.Message -> when (val m = ev.msg) {
                    is ServerMessage.Paired -> {
                        // The pairing token is single-use: keep the device before anything else.
                        // If saving fails the device stays in memory: keep connecting, but warn it will not survive a restart.
                        saved = try { store.save(StoredDevice(r.tunnel.orEmpty(), m.device, r.lan)); true } catch (_: Exception) { false }
                        hello = Hello.Auth(m.device)
                        _state.value = UiState.Connecting(r.label)
                    }
                    is ServerMessage.Ready -> {
                        if (m.protocol != PROTOCOL) {
                            _state.value = UiState.Mismatch(r.label, PROTOCOL, m.protocol)
                            a.mismatch = true
                        } else {
                            a.ready = true
                            p.onConnected()
                            _state.value = connected(m, route, r, saved)
                            // A new connection: whatever the PC says now is the truth (REQ-15).
                            outbox = out
                            out.trySend(Outgoing.Text(Control.sessions()))
                            out.trySend(Outgoing.Text(Control.workspaces(_model.value.session)))
                            out.trySend(Outgoing.Binary(Workspaces.listFrame()))
                        }
                    }
                    is ServerMessage.Endpoints -> {
                        // Only on a connection that is in (it follows `ready`) and only what is valid (REQ-62).
                        if (a.ready) learn(m, r, hello)
                    }
                    is ServerMessage.Error -> {
                        // These two leave the connection open; anything else is followed by a close.
                        if (m.code == ErrorCode.READ_ONLY) _toasts.tryEmit("solo lectura")
                        if (m.code != ErrorCode.READ_ONLY && m.code != ErrorCode.UNSUPPORTED) {
                            a.failure = Failure.Bridge(m.code)
                            a.bridgeFailed = true
                        }
                    }
                    is ServerMessage.Workspaces -> _model.update { it.copy(session = m.session, items = m.items) }
                    is ServerMessage.Sessions -> _model.update { it.copy(sessions = m.items) }
                    is ServerMessage.Other -> Unit
                }
            }
            !a.mismatch
        }.collect()
        out.close()
        return hello to saved
    }

    private fun connected(m: ServerMessage.Ready, route: Route, r: Routes, saved: Boolean) =
        UiState.Connected(r.label, m.protocol, m.readOnly, m.palettes, saved, route, r.lan?.hostPort, r.tunnel)

    /**
     * Takes what `endpoints` says (REQ-62): the tunnel host is kept (and the device token will go there from now
     * on, which is why it had to come on a connection that is in), and so is the wifi address with its pin.
     * Persisted only when something changed.
     */
    private fun learn(m: ServerMessage.Endpoints, r: Routes, hello: Hello) {
        val tunnel = m.tunnel
        val lan = m.lan
        var changed = false
        if (tunnel != r.tunnel && (tunnel == null || Hosts.isTunnel(tunnel))) { r.tunnel = tunnel; changed = true }
        if (lan != r.lan) { r.lan = lan; changed = true }
        if (!changed) return
        (hello as? Hello.Auth)?.let { a ->
            try { store.save(StoredDevice(r.tunnel.orEmpty(), a.device, r.lan)) } catch (_: Exception) { }
        }
        (_state.value as? UiState.Connected)?.let {
            _state.value = UiState.Connected(r.label, it.protocol, it.readOnly, it.palettes, it.saved, it.route, r.lan?.hostPort, r.tunnel)
        }
    }

    /** Every pairing failure drawn as one of the approved failure screens. */
    private fun pairWhy(f: Failure): ScanWhy = when {
        f is Failure.Bridge && f.code == ErrorCode.DENIED -> ScanWhy.DENIED
        f is Failure.Bridge && f.code == ErrorCode.LIMITED -> ScanWhy.LIMITED
        f is Failure.Bridge -> ScanWhy.EXPIRED // expired, unknown, timeout, anything else the bridge says
        else -> ScanWhy.UNREACHABLE // DNS, a dead tunnel, a network error
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
