package dev.jw.next.client

import dev.jw.next.contract.ArtifactDoc
import dev.jw.next.contract.ArtifactItem
import dev.jw.next.contract.ArtifactKind
import dev.jw.next.contract.ArtifactVersion
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.Delivered
import dev.jw.next.contract.Id
import dev.jw.next.contract.Verdict
import kotlinx.serialization.Serializable

/**
 * What the phone knows of artifacts (N171): the list a workspace has, the bar that decides one, the menu
 * that leads to them, and the page that draws an `html` one. All of it is pure: the screens only draw it.
 */

/** The policy of the page that holds an artifact: nothing leaves it and nothing comes in (REQ-776). */
const val ARTIFACT_CSP =
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; " +
        "media-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'"

/**
 * The page the WebView loads for an `html` artifact: the app's own policy first, then the artifact as the
 * `srcdoc` of an iframe in a sandbox, written as an attribute value so that nothing in its text can come
 * before the policy or change it (REQ-776). Only `&` and `"` are written as entities, which is all an
 * attribute value in double quotes needs and what the prototype did.
 */
fun artifactShell(html: String): String {
    val attr = html.replace("&", "&amp;").replace("\"", "&quot;")
    return "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"$ARTIFACT_CSP\">" +
        "<style>html,body{margin:0;height:100%;background:#fff}iframe{border:0;width:100%;height:100%;display:block}</style>" +
        "<iframe sandbox=\"allow-scripts\" srcdoc=\"$attr\"></iframe>"
}

enum class ArtifactStatus { Pending, Approved, Changes }

fun ArtifactItem.latest(): ArtifactVersion = versions.last()

/** A version's status is its verdict (none: it waits). */
fun ArtifactVersion.status(): ArtifactStatus = when (verdict?.verdict) {
    null -> ArtifactStatus.Pending
    Verdict.Approved -> ArtifactStatus.Approved
    Verdict.Changes -> ArtifactStatus.Changes
}

/** An artifact's status is its latest version's. */
fun ArtifactItem.status(): ArtifactStatus = latest().status()

/** What a workspace's list shows: the ones that wait first, the decided ones apart and folded (REQ-781). */
data class ArtifactList(val pending: List<ArtifactItem>, val decided: List<ArtifactItem>)

fun artifactList(items: List<ArtifactItem>): ArtifactList {
    val (pending, decided) = items.partition { it.status() == ArtifactStatus.Pending }
    val newest = compareByDescending<ArtifactItem> { it.latest().at }
    return ArtifactList(pending.sortedWith(newest), decided.sortedWith(newest))
}

fun pendingArtifacts(items: List<ArtifactItem>): Int = items.count { it.status() == ArtifactStatus.Pending }

/** The rows of the menu in a workspace's header (REQ-780), in order; «artefactos» carries what waits. */
enum class WorkspaceMenuRow { Cambiar, Archivos, Cambios, Artefactos }

fun WorkspaceMenuRow.label(): String = when (this) {
    WorkspaceMenuRow.Cambiar -> "cambiar workspace"
    WorkspaceMenuRow.Archivos -> "archivos"
    WorkspaceMenuRow.Cambios -> "cambios"
    WorkspaceMenuRow.Artefactos -> "artefactos"
}

/** What stands in place of the bar that decides (REQ-778, REQ-779). */
sealed interface ArtifactBar {
    /** The version shown is not the latest: only a way to see it. */
    data class Newer(val n: Int) : ArtifactBar

    /** The version has its verdict, and whether the agent was told. */
    data class Decided(val verdict: Verdict, val text: String?, val delivered: Delivered?) : ArtifactBar

    /** No link to the PC: the buttons are there and do nothing (REQ-783). */
    data object Offline : ArtifactBar

    /** Aprobar and Pedir cambios. */
    data object Ready : ArtifactBar
}

fun artifactBar(item: ArtifactItem, viewing: Int, linkUp: Boolean): ArtifactBar {
    val last = item.latest()
    if (viewing != last.n) return ArtifactBar.Newer(last.n)
    last.verdict?.let { return ArtifactBar.Decided(it.verdict, it.text, it.delivered) }
    return if (linkUp) ArtifactBar.Ready else ArtifactBar.Offline
}

/** «Pedir cambios» sends only with something to say (REQ-778). */
fun canAskChanges(text: String): Boolean = text.isNotBlank()

/** The version [n] of [item], or the latest when there is no such one. */
fun ArtifactItem.version(n: Int?): ArtifactVersion = versions.firstOrNull { it.n == n } ?: latest()

/** What a daemon's refusal of a verdict says to the person, from its message (REQ-779). */
fun verdictRefusal(message: String): String = when {
    message.startsWith("stale") -> "Hay una versión nueva: ábrela y decide sobre ella."
    message.startsWith("decided") -> "Esa versión ya tiene veredicto."
    else -> message
}

/** The versions of an artifact whose text the phone keeps: the newest few, so what it keeps does not grow without end. */
private const val KEPT_VERSIONS = 3

/** What the phone keeps of one artifact it opened, to show it read only when there is no link to the PC (REQ-783). */
@Serializable
data class ArtifactCopy(val workspace: Id, val workspaceName: String, val item: ArtifactItem, val docs: Map<Int, ArtifactDoc> = emptyMap())

/** Every copy of one PC, as a file of the app's own storage that is deleted when the PC is revoked or forgotten (REQ-784). */
@Serializable
data class ArtifactCopies(val copies: List<ArtifactCopy> = emptyList()) {
    /** [doc] was read of [item]: the copy has it, and the item as the PC listed it, with the newest few versions' texts. */
    fun keep(workspace: Id, workspaceName: String, item: ArtifactItem, doc: ArtifactDoc): ArtifactCopies {
        val old = copies.firstOrNull { it.item.id == item.id }
        val docs = ((old?.docs ?: emptyMap()) + (doc.n to doc)).toSortedMap().entries.toList().takeLast(KEPT_VERSIONS).associate { it.key to it.value }
        val copy = ArtifactCopy(workspace, workspaceName, item, docs)
        return ArtifactCopies(copies.filter { it.item.id != item.id } + copy)
    }

    /** The copies of [workspace] as the items a list draws; the text of one is [docOf]. */
    fun items(workspace: Id): List<ArtifactItem> = copies.filter { it.workspace == workspace }.map { it.item }

    /** The workspaces that have a copy, with their names, in order of name. */
    fun workspaces(): List<Pair<Id, String>> = copies.map { it.workspace to it.workspaceName }.distinct().sortedBy { it.second }

    /** The kept text of version [n] of [id], or null when this phone never opened it. */
    fun docOf(id: Id, n: Int): ArtifactDoc? = copies.firstOrNull { it.item.id == id }?.docs?.get(n)

    fun isEmpty(): Boolean = copies.isEmpty()

    fun toJson(): String = ContractJson.encodeToString(serializer(), this)

    companion object {
        /** A damaged or missing file is no copies: nothing breaks and nothing is shown that is not there. */
        fun fromJson(text: String?): ArtifactCopies =
            text?.let { runCatching { ContractJson.decodeFromString(serializer(), it) }.getOrNull() } ?: ArtifactCopies()
    }
}

fun ArtifactKind.label(): String = when (this) {
    ArtifactKind.Prototype -> "prototipo"
    ArtifactKind.Document -> "documento"
    ArtifactKind.Report -> "informe"
    ArtifactKind.Spec -> "spec"
}

fun ArtifactStatus.label(): String = when (this) {
    ArtifactStatus.Pending -> "por revisar"
    ArtifactStatus.Approved -> "aprobado"
    ArtifactStatus.Changes -> "cambios pedidos"
}

/** A size as the list says it: KiB, or MiB from one. */
fun artifactSize(bytes: Long): String =
    if (bytes >= 1_048_576) "%.1f MiB".format(java.util.Locale.ROOT, bytes / 1_048_576.0) else "${maxOf(1L, (bytes + 512) / 1024)} KiB"

/** The line under an artifact's title in the list: its version, kind, size and age (REQ-781). */
fun artifactSub(item: ArtifactItem, nowSecs: Long): String {
    val v = item.latest()
    return "v${v.n} · ${item.kind.label()} · ${artifactSize(v.size)} · ${ago(nowSecs, v.at)}"
}

/** The first six characters of a hash, which is what the bar and the agent's message show. */
fun shortHash(hash: String): String = hash.take(6)
