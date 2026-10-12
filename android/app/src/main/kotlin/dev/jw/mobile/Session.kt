package dev.jw.mobile

import dev.jw.proto.AgentState
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
import dev.jw.proto.PaneEvent
import dev.jw.proto.PaneInfo
import dev.jw.proto.Panes
import dev.jw.proto.RefreshRule
import dev.jw.proto.SessionInfo
import dev.jw.proto.Workspace
import dev.jw.proto.CommitList
import dev.jw.proto.Diff
import dev.jw.proto.DiffScope
import dev.jw.proto.FsFile
import dev.jw.proto.FsListing
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
import java.util.concurrent.ConcurrentHashMap

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
/** How many rows of scrollback a scroll up asks the PC for. */
private const val HISTORY_ROWS = 2_000

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
    /** Panes answered with y or n whose latest `Panes` still says Waiting. */
    val answered: Set<Long> = emptySet(),
)

/** The changes of one workspace and view on screen (M6, REQ-13). A failure's words may name a path on the PC: shown, never logged. */
sealed interface DiffState {
    data object Idle : DiffState

    /** Asked and waiting; `before` is the last answer for the same workspace and view, kept on screen while it refreshes. */
    class Loading(val ws: String, val scope: DiffScope, val before: Diff? = null) : DiffState {
        override fun toString() = "Loading"
    }

    class Loaded(val diff: Diff) : DiffState {
        override fun toString() = "Loaded($diff)"
    }

    class Failed(val ws: String, val scope: DiffScope, val why: String) : DiffState {
        override fun toString() = "Failed"
    }
}

/** The commits of the workspace on screen, for the Commits view. */
sealed interface CommitsState {
    data object Idle : CommitsState

    class Loading(val ws: String) : CommitsState {
        override fun toString() = "Loading"
    }

    class Loaded(val list: CommitList) : CommitsState {
        override fun toString() = "Loaded($list)"
    }

    class Failed(val ws: String, val why: String) : CommitsState {
        override fun toString() = "Failed"
    }
}

/** A directory of the PC on screen (REQ-77): asked, answered, or why not. A reason may name a path; shown, never logged. */
sealed interface FsState {
    data object Idle : FsState

    class Loading(val req: String) : FsState {
        override fun toString() = "Loading"
    }

    class Loaded(val listing: FsListing) : FsState {
        override fun toString() = "Loaded($listing)"
    }

    class Failed(val req: String, val why: String) : FsState {
        override fun toString() = "Failed"
    }
}

/** A file of the PC on screen (REQ-79). */
sealed interface FileState {
    data object Idle : FileState

    class Loading(val req: String) : FileState {
        override fun toString() = "Loading"
    }

    class Loaded(val file: FsFile) : FileState {
        override fun toString() = "Loaded($file)"
    }

    class Failed(val req: String, val why: String) : FileState {
        override fun toString() = "Failed"
    }
}

/** What the person asked the PC to make: a workspace from a folder, a session, a worktree (REQ-85 to 87). One at a time. */
sealed interface MakeState {
    data object Idle : MakeState

    /** Asked and waiting: a worktree can take minutes. */
    class Busy(val what: Make, val connection: Int) : MakeState {
        override fun toString() = "Busy($what)"
    }

    /** Made: the workspace `ws` (of session `session`, which is the new one for a new session). */
    class Done(val what: Make, val ws: String, val session: String?) : MakeState {
        override fun toString() = "Done($what)"
    }

    /** Refused or lost, with the reason, which may name a path or a branch of the PC: shown, never logged. */
    class Failed(val what: Make, val why: String) : MakeState {
        override fun toString() = "Failed($what)"
    }
}

enum class Make { Open, Session, Worktree }

/** What the PC is asked that runs git. It answers one at a time per phone, so the session sends them one at a time. */
private sealed interface GitAsk {
    data class Diff(val ws: String, val scope: DiffScope) : GitAsk
    data class Commits(val ws: String) : GitAsk
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
    private val _diff = MutableStateFlow<DiffState>(DiffState.Idle)
    val diff: StateFlow<DiffState> = _diff.asStateFlow()
    private val _commits = MutableStateFlow<CommitsState>(CommitsState.Idle)
    val commits: StateFlow<CommitsState> = _commits.asStateFlow()
    private val _fs = MutableStateFlow<FsState>(FsState.Idle)
    val fs: StateFlow<FsState> = _fs.asStateFlow()
    private val _file = MutableStateFlow<FileState>(FileState.Idle)
    val file: StateFlow<FileState> = _file.asStateFlow()
    /** Moves with every connection that gets in, so a make can tell whether its answer could still come. */
    @Volatile private var connGen = 0
    private val _make = MutableStateFlow<MakeState>(MakeState.Idle)
    val make: StateFlow<MakeState> = _make.asStateFlow()
    private val asks = Any()
    private var inFlight: GitAsk? = null
    private val queued = ArrayDeque<GitAsk>()
    private val _toasts = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val toasts: SharedFlow<String> = _toasts.asSharedFlow()

    @Volatile private var outbox: Channel<Outgoing>? = null
    @Volatile private var visible = false
    @Volatile private var foreground = true
    private val refresh = RefreshRule(pollMs)
    private var poller: Job? = null
    @Volatile private var routes = Routes(null, null)
    private var lastWifiLook = 0L

    // The panes of the workspace on screen, as terminals (M5). Fed on the connection's thread, read by the surface.
    private val screens = ConcurrentHashMap<Long, PaneText>()
    private val exits = ConcurrentHashMap<Long, Int>()
    private val chats = ConcurrentHashMap<Long, ChatState>()
    @Volatile private var chatPane: Long? = null
    private val shells = ConcurrentHashMap<Long, ShellState>()
    @Volatile private var shellPane: Long? = null
    @Volatile private var watching: String? = null
    private val _termRev = MutableStateFlow(0L)

    /** Moves with every byte, size change and exit of a watched pane; the surface redraws when it does. */
    val termRev: StateFlow<Long> = _termRev.asStateFlow()

    fun terminal(pane: Long): PaneText? = screens[pane]

    /** The rows above the screen of `pane`, once the person scrolls up (answered as [PaneEvent.History]). */
    fun requestHistory(pane: Long) = send(Outgoing.Binary(Panes.textHistoryFrame(pane, HISTORY_ROWS)))

    /** The conversation of the agent in `pane`, once the PC has answered (REQ-90); null until then. */
    fun chat(pane: Long): ChatState? = chats[pane]

    /** Follows the conversation of the agent in `pane` (null: none); only one at a time, the one on screen. */
    fun watchChat(pane: Long?) {
        if (pane == chatPane) return
        chatPane?.let { send(Outgoing.Binary(Panes.chatUnwatchFrame(it))); chats.remove(it) }
        chatPane = pane
        pane?.let { send(Outgoing.Binary(Panes.chatWatchFrame(it))) }
        _termRev.update { it + 1 }
    }

    /** The commands of the shell in `pane` and its line being typed, once the PC has answered (REQ-92); null until then. */
    fun shell(pane: Long): ShellState? = shells[pane]

    /** Follows the commands of the shell in `pane` (null: none); only one at a time, the one on screen. */
    fun watchShell(pane: Long?) {
        if (pane == shellPane) return
        shellPane?.let { send(Outgoing.Binary(Panes.shellUnwatchFrame(it))); shells.remove(it) }
        shellPane = pane
        pane?.let { send(Outgoing.Binary(Panes.shellWatchFrame(it))) }
        _termRev.update { it + 1 }
    }

    /** The exit status of a watched pane whose process ended, else null. */
    fun exitStatus(pane: Long): Int? = exits[pane]

    /**
     * Watches the panes of workspace `stream` (null: none). Every pane arrives as a `Snapshot` and then as
     * output; a different workspace first lets go of the old one (REQ-21). The sizes the app changed go back.
     */
    fun watch(stream: String?) {
        if (stream == watching) return
        if (watching != null) { send(Outgoing.Binary(Panes.detachFrame())) }
        screens.clear()
        exits.clear()
        watching = stream
        stream?.let { send(Outgoing.Binary(Panes.textViewFrame())); send(Outgoing.Binary(Panes.attachFrame(it))) }
        _termRev.update { it + 1 }
    }

    /** What a key or a paste sends to `pane`. The bridge drops it in read-only and says so. */
    fun input(pane: Long, bytes: ByteArray) = send(Outgoing.Binary(Workspaces.inputFrame(pane, bytes)))

    /** `^␣ t`: a shell beside `pane`, in `cwd`. */
    fun newPane(pane: Long, cwd: String) {
        val t = screens[pane] ?: return
        send(Outgoing.Binary(Panes.splitFrame(pane, cwd, t.cols, t.rows)))
    }

    /** `^␣ x`: ends the pane's process and forgets the pane. */
    fun closePane(pane: Long) = send(Outgoing.Binary(Panes.killFrame(pane)))

    /** `^␣ n`: names the pane; blank goes back to its automatic title. */
    fun namePane(pane: Long, name: String) = send(Outgoing.Binary(Panes.nameFrame(pane, name)))

    private fun onPane(e: PaneEvent) {
        when (e) {
            is PaneEvent.Text -> screens.getOrPut(e.pane) { PaneText(e.cols, e.rows) }.setScreen(e)
            is PaneEvent.History -> screens[e.pane]?.setHistory(e)
            is PaneEvent.Chat -> chats.compute(e.pane) { _, old -> ChatState.merge(old, e) }
            is PaneEvent.Shell -> shells.compute(e.pane) { _, old -> ShellState.merge(old, e) }
            // The PC sends this client text, never a terminal's bytes (`TextView`).
            is PaneEvent.Snapshot, is PaneEvent.Output, is PaneEvent.Resized -> {}
            is PaneEvent.Exited -> exits[e.pane] = e.status
        }
        _termRev.update { it + 1 }
    }

    /** The workspaces, avisos or workspace screen is on screen (the list is what is being watched). */
    fun setVisible(v: Boolean) { visible = v }

    /** The activity is resumed. */
    fun setForeground(f: Boolean) { foreground = f }

    private fun send(o: Outgoing) { outbox?.trySend(o) }

    /** Ask for the workspaces of `session` (null: the PC's default, `main`). */
    fun requestWorkspaces(session: String? = null) = send(Outgoing.Text(Control.workspaces(session)))

    fun requestSessions() = send(Outgoing.Text(Control.sessions()))

    /**
     * Asks for the changes of workspace `ws` in one view (REQ-13). The answer lands in [diff]; one for another
     * workspace or view (an old ask) is dropped. Only the latest ask waits its turn; a request the connection lost is
     * asked again when it is back.
     */
    fun requestDiff(ws: String, scope: DiffScope = DiffScope.General) {
        val before = when (val d = _diff.value) {
            is DiffState.Loaded -> d.diff.takeIf { it.ws == ws && it.scope == scope }
            is DiffState.Loading -> d.before?.takeIf { it.ws == ws && it.scope == scope }
            else -> null
        }
        _diff.value = DiffState.Loading(ws, scope, before)
        enqueue(GitAsk.Diff(ws, scope))
    }

    /** Asks for the commits of `ws`'s branch (the Commits view). */
    fun requestCommits(ws: String) {
        _commits.value = CommitsState.Loading(ws)
        enqueue(GitAsk.Commits(ws))
    }

    /** Asks for the entries of the directory `path` of the PC (`~` is its home). The answer lands in [fs]. */
    fun requestLs(path: String) {
        _fs.value = FsState.Loading(path)
        send(Outgoing.Text(Control.ls(path)))
    }

    /** Asks for the text of the file `path` of the PC. The answer lands in [file]. */
    fun requestRead(path: String) {
        _file.value = FileState.Loading(path)
        send(Outgoing.Text(Control.read(path)))
    }

    /** Makes the folder `path` of the PC a workspace of `session` (REQ-85). */
    fun requestOpen(path: String, session: String?) = ask(Make.Open, Control.open(path, session))

    /** Makes session `name`, starting in the folder `dir` (REQ-86). */
    fun requestNewSession(name: String, dir: String) = ask(Make.Session, Control.sessionNew(name, dir))

    /** Makes worktree `name` of `project` in `session` and gives `task` to its agent (REQ-87). */
    fun requestWorktree(project: String, name: String, branch: String?, task: String?, session: String?) =
        ask(Make.Worktree, Control.worktree(project, name, branch, task, session))

    private fun ask(what: Make, text: String) {
        if (_make.value is MakeState.Busy) return
        _make.value = MakeState.Busy(what, connGen)
        send(Outgoing.Text(text))
    }

    /** The make is over (or its screen left): nothing is shown of it any more. */
    fun clearMake() { if (_make.value !is MakeState.Busy) _make.value = MakeState.Idle }

    private fun made(what: Make, ws: String, session: String?) {
        if ((_make.value as? MakeState.Busy)?.what != what) return
        _make.value = MakeState.Done(what, ws, session)
        // What was made is in the PC's list now: ask for it, in the session it is in.
        requestWorkspaces(session ?: _model.value.session)
        if (what == Make.Session) requestSessions()
    }

    /** The files screen is left: let go of what it held. */
    fun clearFiles() {
        _fs.value = FsState.Idle
        _file.value = FileState.Idle
    }

    /** The changes screen is left: let go of the answers, which can be megabytes. */
    fun clearDiff() {
        synchronized(asks) { queued.clear() }
        _diff.value = DiffState.Idle
        _commits.value = CommitsState.Idle
    }

    private fun enqueue(ask: GitAsk) {
        synchronized(asks) {
            // Only the latest of each kind is still wanted.
            queued.removeAll { it::class == ask::class }
            if (inFlight != ask) queued.addLast(ask)
        }
        pump()
    }

    /** Sends the next ask if none is out. */
    private fun pump() {
        val next = synchronized(asks) {
            if (inFlight != null) return
            queued.removeFirstOrNull()?.also { inFlight = it }
        } ?: return
        send(Outgoing.Text(when (next) {
            is GitAsk.Diff -> Control.diff(next.ws, next.scope)
            is GitAsk.Commits -> Control.commits(next.ws)
        }))
    }

    /** `ask` is answered (or refused): let the next one go. */
    private fun done(ask: GitAsk) {
        synchronized(asks) { if (inFlight == ask) inFlight = null }
        pump()
    }

    /** A new connection has a free seat: whatever was waiting is asked again. */
    private fun reask() {
        synchronized(asks) {
            inFlight = null
            queued.clear()
            (_diff.value as? DiffState.Loading)?.let { queued.addLast(GitAsk.Diff(it.ws, it.scope)) }
            (_commits.value as? CommitsState.Loading)?.let { queued.addLast(GitAsk.Commits(it.ws)) }
        }
        pump()
        // A make cannot be asked again (it would be made twice): the PC goes on with it, and the list will show it.
        (_make.value as? MakeState.Busy)?.takeIf { it.connection != connGen }?.let {
            _make.value = MakeState.Failed(it.what, "se perdió la conexión mientras el PC lo hacía: mira la lista de workspaces en un momento")
            requestWorkspaces(_model.value.session)
        }
        // The files asks have seats of their own and no queue: what was waiting is asked again.
        (_fs.value as? FsState.Loading)?.let { send(Outgoing.Text(Control.ls(it.req))) }
        (_file.value as? FileState.Loading)?.let { send(Outgoing.Text(Control.read(it.req))) }
    }

    /**
     * y or n for `pane`. Sends once, and only while the latest `Panes` (at most 6 s old) says Waiting
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
                if (refresh.due(clock(), visible && foreground && outbox != null)) {
                    send(Outgoing.Binary(Workspaces.listFrame()))
                    // The list of workspaces too: one added or closed on the PC shows up without reopening the app.
                    send(Outgoing.Text(Control.workspaces(_model.value.session)))
                }
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
            run(routes, Hello.Auth(dev))
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
        clearDiff()
        clearFiles()
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

    private fun run(r: Routes, first: Hello) {
        job?.cancel()
        startPoller()
        job = scope.launch { loop(r, first) }
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

    private suspend fun loop(r: Routes, first: Hello) {
        val p = policy()
        var hello = first
        var saved = true
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
                is BridgeEvent.Binary -> {
                    val pane = Panes.parse(ev.frame)
                    if (pane != null) onPane(pane)
                    else Workspaces.parsePanes(ev.frame)?.let { ps ->
                        _model.update { m ->
                            // An answered prompt stays answered while the pane still says Waiting, so the
                            // buttons do not go green again between the tap and the agent moving on.
                            val waiting = ps.filter { it.agent == AgentState.Waiting }.map { it.pane }.toSet()
                            m.copy(panes = ps, panesAt = clock(), answered = m.answered intersect waiting)
                        }
                    }
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
                            connGen++
                            _state.value = connected(m, route, r, saved)
                            // A new connection: whatever the PC says now is the truth (REQ-15).
                            outbox = out
                            out.trySend(Outgoing.Text(Control.sessions()))
                            out.trySend(Outgoing.Text(Control.workspaces(_model.value.session)))
                            out.trySend(Outgoing.Binary(Workspaces.listFrame()))
                            reask()
                            // The screens come again as snapshots: the new one is the truth (REQ-15).
                            screens.clear()
                            exits.clear()
                            chats.clear()
                            chatPane?.let { out.trySend(Outgoing.Binary(Panes.chatWatchFrame(it))) }
                            shells.clear()
                            shellPane?.let { out.trySend(Outgoing.Binary(Panes.shellWatchFrame(it))) }
                            watching?.let {
                                out.trySend(Outgoing.Binary(Panes.textViewFrame()))
                                out.trySend(Outgoing.Binary(Panes.attachFrame(it)))
                            }
                            _termRev.update { it + 1 }
                        }
                    }
                    is ServerMessage.Endpoints -> {
                        // Only on a connection that is in (it follows `ready`) and only what is valid (REQ-62).
                        if (a.ready) learn(m, r, hello)
                    }
                    is ServerMessage.Error -> {
                        // These two leave the connection open; anything else is followed by a close.
                        if (m.code == ErrorCode.READ_ONLY) _toasts.tryEmit("solo lectura")
                        // An ask's own failure says which ask it is (`ask`, `ws`, `scope`); `busy` is the PC saying one
                        // is still running, which the session's own queue prevents, so it fails nothing.
                        val files = m.ask == "ls" || m.ask == "read"
                        if (m.code == ErrorCode.UNSUPPORTED && (files || !m.msg.startsWith("busy"))) askFailed(m)
                        // A make that a read-only bridge refused says so in its own sheet.
                        if (m.code == ErrorCode.READ_ONLY && m.ask != null) askFailed(m)
                        if (m.code != ErrorCode.READ_ONLY && m.code != ErrorCode.UNSUPPORTED) {
                            a.failure = Failure.Bridge(m.code)
                            a.bridgeFailed = true
                        }
                    }
                    is ServerMessage.Workspaces -> _model.update { it.copy(session = m.session, items = m.items) }
                    is ServerMessage.Sessions -> _model.update { it.copy(sessions = m.items) }
                    is ServerMessage.DiffAnswer -> {
                        val d = m.diff
                        (_diff.value as? DiffState.Loading)?.takeIf { it.ws == d.ws && it.scope == d.scope }?.let { _diff.value = DiffState.Loaded(d) }
                        done(GitAsk.Diff(d.ws, d.scope))
                    }
                    is ServerMessage.LsAnswer -> (_fs.value as? FsState.Loading)?.takeIf { it.req == m.listing.req }?.let { _fs.value = FsState.Loaded(m.listing) }
                    is ServerMessage.ReadAnswer -> (_file.value as? FileState.Loading)?.takeIf { it.req == m.file.req }?.let { _file.value = FileState.Loaded(m.file) }
                    is ServerMessage.Opened -> made(Make.Open, m.ws, null)
                    is ServerMessage.SessionCreated -> made(Make.Session, m.ws, m.name)
                    is ServerMessage.WorktreeMade -> made(Make.Worktree, m.ws, null)
                    is ServerMessage.CommitsAnswer -> {
                        (_commits.value as? CommitsState.Loading)?.takeIf { it.ws == m.list.ws }?.let { _commits.value = CommitsState.Loaded(m.list) }
                        done(GitAsk.Commits(m.list.ws))
                    }
                    is ServerMessage.Other -> Unit
                }
            }
            !a.mismatch
        }.collect()
        out.close()
        return hello to saved
    }

    private fun askFailed(m: ServerMessage.Error) {
        val make = when (m.ask) { "opened" -> Make.Open; "session_created" -> Make.Session; "worktree" -> Make.Worktree; else -> null }
        if (make != null) {
            if ((_make.value as? MakeState.Busy)?.what == make) _make.value = MakeState.Failed(make, if (m.code == ErrorCode.READ_ONLY) "solo lectura: el puente no deja hacer esto" else m.msg)
            return
        }
        when (m.ask) {
            "ls" -> m.req?.let { req -> (_fs.value as? FsState.Loading)?.takeIf { it.req == req }?.let { _fs.value = FsState.Failed(req, m.msg) } }
            "read" -> m.req?.let { req -> (_file.value as? FileState.Loading)?.takeIf { it.req == req }?.let { _file.value = FileState.Failed(req, m.msg) } }
        }
        val ws = m.ws ?: return
        when (m.ask) {
            "diff" -> {
                val scope = DiffScope.parse(m.scope) ?: DiffScope.General
                (_diff.value as? DiffState.Loading)?.takeIf { it.ws == ws && it.scope == scope }?.let { _diff.value = DiffState.Failed(ws, scope, m.msg) }
                done(GitAsk.Diff(ws, scope))
            }
            "commits" -> {
                (_commits.value as? CommitsState.Loading)?.takeIf { it.ws == ws }?.let { _commits.value = CommitsState.Failed(ws, m.msg) }
                done(GitAsk.Commits(ws))
            }
        }
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
