package dev.jw.next.app

import android.content.Context
import androidx.compose.runtime.mutableIntStateOf
import dev.jw.next.client.CacheResult
import dev.jw.next.client.NodeCache
import dev.jw.next.client.NodeSource
import dev.jw.next.client.Seen
import dev.jw.next.contract.ClientMsg
import dev.jw.next.contract.Cmd
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.Peer
import java.util.concurrent.Executors
import kotlinx.serialization.SerializationException

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
 * The live daemon, over the same JSON lines the terminal client uses. Development link: the
 * phone reaches the daemon's loopback listener through `adb reverse`. Nodes are opened on
 * demand and kept in a [NodeCache]; a dropped connection is retried and reopens them.
 */
class LiveSource(private val host: String, private val port: Int) : AppSource() {
    private val lock = Any()
    private val cache = NodeCache()
    private val opened = LinkedHashSet<Id>()
    private var rootId: Id? = null
    private var out: java.io.OutputStream? = null

    @Volatile
    private var state = "conectando…"
    override val status: String get() = "en vivo · $host:$port · $state"

    @Volatile
    private var closed = false

    init {
        Thread({ run() }, "live-source").apply { isDaemon = true }.start()
    }

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

    private var commands = 0

    override fun write(pane: Id, data: String) {
        send(ClientMsg.Command("w${++commands}", Cmd.Write(pane, data)))
    }

    override fun markSeen(id: Id, rev: Long) {
        send(ClientMsg.Command("m${++commands}", Cmd.MarkSeen(id, rev)))
    }

    fun close() {
        closed = true
        writer.shutdown()
    }

    private fun cachedRev(id: Id) = synchronized(lock) { cache.rev(id) }
    private fun cachedEpoch(id: Id) = synchronized(lock) { cache.epoch }.takeIf { cachedRev(id) != null }

    /** Writes happen on one thread of their own: a socket write on the UI thread is refused. */
    private val writer = Executors.newSingleThreadExecutor { r -> Thread(r, "live-writer").apply { isDaemon = true } }

    private fun send(msg: ClientMsg) {
        val line = ContractJson.encodeToString(ClientMsg.serializer(), msg) + "\n"
        writer.execute {
            try {
                synchronized(lock) { out }?.let {
                    it.write(line.toByteArray())
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
                    send(ClientMsg.Hello(Peer("jwn-android", "0.0.1"), mapOf("session" to listOf(1), "workspace" to listOf(1), "pane" to listOf(1), "blocks" to listOf(1))))
                    socket.getInputStream().bufferedReader().forEachLine { line -> receive(line) }
                }
            } catch (_: java.io.IOException) {
                // Fall through to the retry below.
            }
            synchronized(lock) { out = null }
            state = "sin conexión, reintentando"
            tick.intValue++
            Thread.sleep(2000)
        }
    }

    private fun receive(line: String) {
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
        if (reopen != null) send(ClientMsg.Open(reopen))
        tick.intValue++
    }
}
