package dev.jw.next.app

import android.content.Context
import dev.jw.next.client.NodeSource
import dev.jw.next.client.Seen
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node

/** The nodes the shared fixtures hold: what the app draws until it talks to a daemon. */
class FixtureSource(context: Context) : NodeSource {
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
