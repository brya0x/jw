package dev.jw.next.app

import android.content.Context
import androidx.compose.runtime.mutableIntStateOf
import dev.jw.next.client.CacheResult
import dev.jw.next.client.NodeCache
import dev.jw.next.client.NodeSource
import dev.jw.next.client.Seen
import dev.jw.next.client.worktreeStarted
import dev.jw.next.contract.Accent
import dev.jw.next.contract.ClientMsg
import dev.jw.next.contract.Cmd
import dev.jw.next.contract.ConfigFrom
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.ConversationList
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.DiffScope
import dev.jw.next.contract.DirListing
import dev.jw.next.contract.DiffState
import dev.jw.next.contract.ErrorBody
import dev.jw.next.contract.ErrorCode
import dev.jw.next.contract.FileListing
import dev.jw.next.contract.Id
import dev.jw.next.contract.Image
import dev.jw.next.contract.MarkdownDoc
import dev.jw.next.contract.OpenKind
import dev.jw.next.contract.Node
import dev.jw.next.contract.Peer
import dev.jw.next.contract.ProjectEdit
import dev.jw.next.contract.ProjectInfo
import dev.jw.next.contract.WorktreeInfo
import dev.jw.next.contract.WorktreeList
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull

/** A source the UI can watch: [tick] changes whenever a node arrives. */
abstract class AppSource : NodeSource {
    val tick = mutableIntStateOf(0)

    /** What the header says about where the data comes from. */
    abstract val status: String

    /** Whether typing reaches a shell: not with the fixtures. */
    open val writable: Boolean = false
}

/** The nodes the shared fixtures hold: what the app draws when no daemon is given. */
class FixtureSource(context: Context) : AppSource() {
    override val status = "ejemplo · fixtures"

    private val nodes: Map<Id, Node>
    private val session: Node.Session

    init {
        val dir = "fixtures/daemon"
        val all = context.assets.list(dir).orEmpty().filter { it.startsWith("snapshot-") }.map { name ->
            val text = context.assets.open("$dir/$name").bufferedReader().use { it.readText() }
            name to (ContractJson.decodeFromString(DaemonMsg.serializer(), text) as DaemonMsg.Snapshot).node
        }
        session = all.first { it.first == "snapshot-session-attention.json" }.second as Node.Session
        // Two session fixtures share one id; the session is the explicit one above.
        nodes = all.map { it.second }.filter { it !is Node.Session }.associateBy { it.id }
    }

    override val epoch: String = session.epoch
    override fun session(): Node.Session = session
    override fun node(id: Id): Node? = nodes[id]

    /** The changes are the fixtures' too: the same diff for every workspace, and marks are not kept. */
    private fun found(node: Node?): Result<Id> =
        node?.let { Result.success(it.id) } ?: Result.failure(Exception("sin datos de ejemplo"))

    override fun openDiff(workspace: Id, scope: DiffScope, commit: String?, reply: (Result<Id>) -> Unit) {
        val whole = nodes.values.filterIsInstance<Node.Diff>().filter { it.state == DiffState.Ok && !it.tooLarge }
        reply(found(whole.firstOrNull { it.scope == scope }))
    }

    override fun openCommits(workspace: Id, reply: (Result<Id>) -> Unit) {
        reply(found(nodes.values.filterIsInstance<Node.Commits>().firstOrNull { !it.more }))
    }
}

/** This phone's seen record, in private preferences; nothing here leaves the phone. */
class SeenStore(context: Context) {
    private val prefs = context.getSharedPreferences("seen", Context.MODE_PRIVATE)

    fun load(): Seen = Seen.fromJson(prefs.getString("record", null))

    fun save(seen: Seen) {
        prefs.edit().putString("record", seen.toJson()).apply()
    }
}

/**
 * The daemon's protocol over any transport that carries its lines: nodes are opened on demand
 * and kept in a [NodeCache], and when the transport comes back every opened node is opened again.
 */
abstract class ProtocolSource : AppSource() {
    private val lock = Any()
    private val cache = NodeCache()
    private val opened = LinkedHashSet<Id>()
    private var rootId: Id? = null
    private val commands = AtomicInteger()

    /** What to do with the reply to a command, by the command's id. */
    private val replies = mutableMapOf<String, (DaemonMsg.Reply) -> Unit>()

    /** Hand one line to the transport without blocking the caller (the UI thread calls this). */
    protected abstract fun sendLine(line: String)

    override val epoch: String get() = synchronized(lock) { cache.epoch.orEmpty() }

    override fun session(): Node.Session = synchronized(lock) {
        rootId?.let { cache.node(it) as? Node.Session }
    } ?: Node.Session("", 1, "", 0, emptyList(), "sin conexión", emptyList())

    override fun node(id: Id): Node? = synchronized(lock) { cache.node(id) }

    override fun open(id: Id) {
        synchronized(lock) { opened.add(id) }
        send(ClientMsg.Open(id, epoch = cachedEpoch(id), since = cachedRev(id)))
    }

    override val writable: Boolean = true

    override fun write(pane: Id, data: String) {
        command(Cmd.Write(pane, data))
    }

    override fun markSeen(id: Id, rev: Long) {
        command(Cmd.MarkSeen(id, rev))
    }

    override fun rev(id: Id): Long? = cachedRev(id)

    override fun openDiff(workspace: Id, scope: DiffScope, commit: String?, reply: (Result<Id>) -> Unit) {
        command(Cmd.OpenDiff(workspace, scope, commit)) { reply(it.nodeId()) }
    }

    override fun openCommits(workspace: Id, reply: (Result<Id>) -> Unit) {
        command(Cmd.OpenCommits(workspace)) { reply(it.nodeId()) }
    }

    override fun refreshDiff(node: Id) {
        command(Cmd.RefreshDiff(node))
    }

    override fun setViewed(node: Id, path: String, viewed: Boolean) {
        command(Cmd.SetViewed(node, path, viewed))
    }

    override fun renameWorkspace(workspace: Id, name: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.RenameWorkspace(workspace, name)) { reply(it.done("el PC no pudo renombrar el workspace")) }
    }

    override fun renamePane(pane: Id, name: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.RenamePane(pane, name)) { reply(it.done("el PC no pudo nombrar el panel")) }
    }

    override fun listFiles(workspace: Id, path: String?, reply: (Result<FileListing>) -> Unit) {
        command(Cmd.ListFiles(workspace, path)) { r -> reply(r.read(FileListing.serializer())) }
    }

    override fun listDir(path: String?, hidden: Boolean, reply: (Result<DirListing>) -> Unit) {
        command(Cmd.ListDir(path, hidden)) { r -> reply(r.read(DirListing.serializer())) }
    }

    override fun openFolder(path: String, reply: (Result<Id>) -> Unit) {
        command(Cmd.OpenFolder(path)) { r -> withShell(r.workspaceId("el PC no pudo abrir la carpeta"), reply) }
    }

    override fun projectConfig(repo: String, reply: (Result<ProjectInfo>) -> Unit) {
        command(Cmd.ProjectConfig(repo)) { r -> reply(r.read(ProjectInfo.serializer())) }
    }

    override fun editProjectConfig(repo: String, to: ConfigFrom, set: ProjectEdit, reply: (Result<Pair<String, Boolean>>) -> Unit) {
        command(Cmd.EditProjectConfig(repo, to, set)) { r ->
            val path = (r.result?.get("path") as? JsonPrimitive)?.contentOrNull
            val shadowed = (r.result?.get("shadowed") as? JsonPrimitive)?.booleanOrNull == true
            reply(
                when {
                    !r.ok -> Result.failure(Exception(r.error?.message ?: "el PC no pudo guardar la configuración"))
                    path == null -> Result.failure(Exception("el PC no dijo dónde la guardó"))
                    else -> Result.success(path to shadowed)
                },
            )
        }
    }

    override fun listWorktrees(repo: String, reply: (Result<List<WorktreeInfo>>) -> Unit) {
        command(Cmd.ListWorktrees(repo)) { r -> reply(r.read(WorktreeList.serializer(), "los worktrees").map { it.worktrees }) }
    }

    override fun openWorktree(repo: String, path: String, reply: (Result<Id>) -> Unit) {
        command(Cmd.OpenWorktree(repo, path)) { r -> reply(r.workspaceId("el PC no pudo abrir el worktree")) }
    }

    override fun startWorkspace(workspace: Id, open: OpenKind?, reply: (Result<Unit>) -> Unit) {
        command(Cmd.StartWorkspace(workspace, open)) { reply(it.done("el PC no pudo abrir el panel")) }
    }

    override fun createWorktree(repo: String, branch: String, base: String?, folder: String, open: OpenKind?, reply: (Result<Id>) -> Unit) {
        command(Cmd.CreateWorktree(repo, branch, base, folder)) { r ->
            worktreeStarted(r.workspaceId("el PC no pudo crear el worktree"), { ws, done -> startWorkspace(ws, open, done) }, ::newPane, reply)
        }
    }

    /** A folder just opened gets a shell, as the terminal client does (REQ-208); [reply] gets its id, or what failed. */
    /** The panes a folder's settings say (REQ-503), or, if the PC cannot start them, one shell. */
    private fun withShell(made: Result<Id>, reply: (Result<Id>) -> Unit) {
        val workspace = made.getOrElse { return reply(made) }
        command(Cmd.StartWorkspace(workspace)) { started ->
            if (started.ok) return@command reply(made)
            command(Cmd.NewPane(workspace)) { p ->
                reply(if (p.ok) made else Result.failure(Exception("se abrió, pero sin shell: ${p.error?.message ?: "el PC no pudo abrir el panel"}")))
            }
        }
    }

    override fun newPane(workspace: Id, reply: (Result<Id>) -> Unit) {
        command(Cmd.NewPane(workspace)) { reply(it.made("pane", "el panel", "el PC no pudo abrir el panel")) }
    }

    override fun removeWorkspace(workspace: Id, removeWorktree: Boolean, reply: (Result<Unit>) -> Unit) {
        command(Cmd.RemoveWorkspace(workspace, removeWorktree)) { reply(it.done("el PC no pudo quitar el workspace")) }
    }

    override fun closePane(pane: Id, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ClosePane(pane)) { reply(it.done("el PC no pudo cerrar el panel")) }
    }

    override fun conversationSend(node: Id, text: String, images: List<Image>, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ConversationSend(node, text, images)) { reply(it.done("el PC no pudo enviarlo")) }
    }

    override fun conversationAnswer(node: Id, ask: String, key: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ConversationAnswer(node, ask, key, by = "el móvil")) { reply(it.done("el PC no pudo registrar la respuesta")) }
    }

    override fun conversationInterrupt(node: Id) {
        command(Cmd.ConversationInterrupt(node))
    }

    override fun conversationStopTask(node: Id, task: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ConversationStopTask(node, task)) { reply(it.done("el PC no pudo detener la tarea")) }
    }

    override fun conversationSet(node: Id, name: String?, accent: Accent?, model: String?, effort: String?, mode: ConvMode?, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ConversationSet(node, name, accent, model, effort, mode)) { reply(it.done("el PC no pudo cambiarlo")) }
    }

    override fun conversationShell(node: Id, command: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.ConversationShell(node, command)) { reply(it.done("el PC no pudo ejecutar la línea")) }
    }

    override fun listConversations(workspace: Id, reply: (Result<ConversationList>) -> Unit) {
        command(Cmd.ListConversations(workspace)) { r -> reply(r.read(ConversationList.serializer())) }
    }

    override fun readMarkdown(workspace: Id, path: String, reply: (Result<MarkdownDoc>) -> Unit) {
        command(Cmd.ReadMarkdown(workspace, path)) { r -> reply(r.read(MarkdownDoc.serializer(), "el documento")) }
    }

    override fun readPlan(node: Id, ask: String, reply: (Result<MarkdownDoc>) -> Unit) {
        command(Cmd.ReadPlan(node, ask)) { r -> reply(r.read(MarkdownDoc.serializer(), "el plan")) }
    }

    override fun resumeConversation(pane: Id, id: String, reply: (Result<Unit>) -> Unit) {
        command(Cmd.OpenConversation(pane, resume = id)) { reply(it.done("el PC no pudo retomar la conversación")) }
    }

    override fun close(id: Id) {
        synchronized(lock) { opened.remove(id) }
        send(ClientMsg.Close(id))
    }

    /** Send a command; [reply] runs on the thread that reads the answer, or with a failure if the link drops first. */
    private fun command(cmd: Cmd, reply: ((DaemonMsg.Reply) -> Unit)? = null) {
        val id = "c${commands.incrementAndGet()}"
        if (reply != null) synchronized(lock) { replies[id] = reply }
        send(ClientMsg.Command(id, cmd))
    }

    /** What a command read, as a [T]: the PC's message when it refused, else what is wrong with its answer. */
    private fun <T> DaemonMsg.Reply.read(serializer: DeserializationStrategy<T>, what: String = "la carpeta"): Result<T> {
        if (!ok) return Result.failure(Exception(error?.message ?: "el PC no pudo leer $what"))
        val body = result ?: return Result.failure(Exception("el PC no dio $what"))
        return try {
            Result.success(ContractJson.decodeFromJsonElement(serializer, body))
        } catch (e: SerializationException) {
            Result.failure(Exception("el PC dio $what de una forma que no se entiende"))
        }
    }

    /** Nothing, or the PC's message ([fallback] when it gave none). */
    private fun DaemonMsg.Reply.done(fallback: String): Result<Unit> =
        if (ok) Result.success(Unit) else Result.failure(Exception(error?.message ?: fallback))

    /** What a command made, as the id under [key] of its result, or the PC's message ([fallback] when it gave none). */
    private fun DaemonMsg.Reply.made(key: String, what: String, fallback: String): Result<Id> {
        val id = (result?.get(key) as? JsonPrimitive)?.contentOrNull
        return when {
            ok && id != null -> Result.success(id)
            ok -> Result.failure(Exception("el PC no dio $what"))
            else -> Result.failure(Exception(error?.message ?: fallback))
        }
    }

    private fun DaemonMsg.Reply.workspaceId(fallback: String) = made("workspace", "el workspace", fallback)

    private fun DaemonMsg.Reply.nodeId() = made("node", "el nodo", "el PC no pudo hacerlo")

    private fun cachedRev(id: Id) = synchronized(lock) { cache.rev(id) }
    private fun cachedEpoch(id: Id) = synchronized(lock) { cache.epoch }.takeIf { cachedRev(id) != null }

    protected fun send(msg: ClientMsg) = sendLine(ContractJson.encodeToString(ClientMsg.serializer(), msg))

    /** The transport is up: say hello; the daemon answers with its root, and everything opened is opened again. */
    protected fun greet() {
        send(ClientMsg.Hello(Peer("jwn-android", "0.0.1"), mapOf("session" to listOf(1), "workspace" to listOf(1), "pane" to listOf(1), "blocks" to listOf(1), "diff" to listOf(1), "commits" to listOf(1), "conversation" to listOf(1))))
    }

    /** The transport is gone. */
    protected fun dropped() {
        // An answer that was still to come never will: say so, so nothing waits for it for ever.
        val lost = synchronized(lock) { replies.values.toList().also { replies.clear() } }
        val gone = DaemonMsg.Reply("", false, null, ErrorBody(ErrorCode.Io, "se perdió la conexión con el PC"))
        lost.forEach { it(gone) }
        tick.intValue++
    }

    /** One line from the daemon. */
    protected fun receive(line: String) {
        val msg = try {
            ContractJson.decodeFromString(DaemonMsg.serializer(), line)
        } catch (e: SerializationException) {
            android.util.Log.w("jwn", "cannot read a line: ${e.message?.take(300)}")
            return
        }
        val reopen = synchronized(lock) {
            val result = cache.apply(msg)
            if (msg is DaemonMsg.Hello) {
                rootId = msg.root
                opened.add(msg.root)
            }
            if (result is CacheResult.Reopen) result.node else null
        }
        if (msg is DaemonMsg.Hello) synchronized(lock) { opened.toList() }.forEach(::open)
        if (msg is DaemonMsg.Reply) synchronized(lock) { replies.remove(msg.id) }?.invoke(msg)
        if (reopen != null) send(ClientMsg.Open(reopen))
        tick.intValue++
    }
}

/**
 * The live daemon over its loopback TCP listener. Development link: the phone reaches it
 * through `adb reverse`. A dropped connection is retried.
 */
class LiveSource(private val host: String, private val port: Int) : ProtocolSource() {
    private var out: java.io.OutputStream? = null
    private val lock = Any()

    @Volatile
    private var state = "conectando…"
    override val status: String get() = "en vivo · $host:$port · $state"

    @Volatile
    private var closed = false

    init {
        Thread({ run() }, "live-source").apply { isDaemon = true }.start()
    }

    fun close() {
        closed = true
        writer.shutdown()
    }

    /** Writes happen on one thread of their own: a socket write on the UI thread is refused. */
    private val writer = Executors.newSingleThreadExecutor { r -> Thread(r, "live-writer").apply { isDaemon = true } }

    override fun sendLine(line: String) {
        writer.execute {
            try {
                synchronized(lock) { out }?.let {
                    it.write((line + "\n").toByteArray())
                    it.flush()
                }
            } catch (_: java.io.IOException) {
                // The reader notices the dead socket and reconnects.
            }
        }
    }

    private fun run() {
        while (!closed) {
            try {
                java.net.Socket().use { socket ->
                    socket.connect(java.net.InetSocketAddress(host, port), 3000)
                    synchronized(lock) { out = socket.getOutputStream() }
                    state = "conectado"
                    greet()
                    socket.getInputStream().bufferedReader().forEachLine { line -> receive(line) }
                }
            } catch (_: java.io.IOException) {
                // Fall through to the retry below.
            }
            synchronized(lock) { out = null }
            state = "sin conexión, reintentando"
            dropped()
            Thread.sleep(2000)
        }
    }
}
