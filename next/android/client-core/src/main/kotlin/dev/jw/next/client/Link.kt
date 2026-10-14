package dev.jw.next.client

import dev.jw.next.contract.Denied
import java.security.MessageDigest
import java.security.SecureRandom
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** A host and port, as the PC reports its private address. */
@Serializable
data class HostPort(val host: String, val port: Int) {
    override fun toString() = "$host:$port"
}

/** What the pairing link (the QR's text) tells a phone: where the PC is and the one-use token. */
data class PairLink(val token: String, val lan: HostPort?, val fp: String?, val tunnel: String?)

private val hex32 = Regex("^[0-9a-f]{32}$")
private val hex64 = Regex("^[0-9a-f]{64}$")
private val tunnelHost = Regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*\\.trycloudflare\\.com$")

/** The only tunnel hosts a phone accepts (REQ-82). */
fun isTunnelHost(host: String) = tunnelHost.matches(host)

/** 10/8, 172.16/12 and 192.168/16: the only addresses a phone will pin a certificate for (REQ-81). */
fun isPrivateV4(host: String): Boolean {
    val p = host.split('.')
    if (p.size != 4) return false
    val n = p.map { it.toIntOrNull()?.takeIf { v -> v in 0..255 && it == v.toString() } ?: return false }
    return n[0] == 10 || (n[0] == 172 && n[1] in 16..31) || (n[0] == 192 && n[1] == 168)
}

fun parseHostPort(text: String): HostPort? {
    val (host, port) = text.split(':').takeIf { it.size == 2 } ?: return null
    val p = port.toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
    return HostPort(host, p).takeIf { isPrivateV4(host) }
}

/**
 * `jwn://<ipv4>:<port>/<token>?fp=<64 hex>[&t=<tunnel host>]` or `jwn://<tunnel host>/<token>`.
 * Anything else, including a host the phone must not trust, is not a link.
 */
fun parsePairLink(text: String, scheme: String): PairLink? {
    val prefix = "$scheme://"
    val raw = text.trim()
    if (!raw.startsWith(prefix)) return null
    val (path, query) = raw.removePrefix(prefix).let { it.substringBefore('?') to it.substringAfter('?', "") }
    val (authority, token) = path.split('/').takeIf { it.size == 2 } ?: return null
    if (!hex32.matches(token)) return null
    val params = query.split('&').filter { it.isNotEmpty() }.associate { it.substringBefore('=') to it.substringAfter('=', "") }
    val tunnel = params["t"]?.also { if (!isTunnelHost(it)) return null }
    if (isTunnelHost(authority)) return if (query.isEmpty()) PairLink(token, null, null, authority) else null
    val lan = parseHostPort(authority) ?: return null
    val fp = params["fp"]?.takeIf { hex64.matches(it) } ?: return null
    return PairLink(token, lan, fp, tunnel)
}

/** A way to reach the PC, in the order a phone tries them: the network first, then the tunnel. */
data class Candidate(val url: String, val lan: Boolean)

/** What a paired phone keeps: its device, its secret and where the PC was last seen. */
@Serializable
data class Pairing(
    val device: String,
    val secret: String,
    val lan: HostPort? = null,
    val fp: String? = null,
    val tunnel: String? = null,
) {
    fun candidates(): List<Candidate> = listOfNotNull(
        lan?.takeIf { fp != null }?.let { Candidate("wss://$it/", true) },
        tunnel?.let { Candidate("wss://$it/", false) },
    )

    /** The PC reported its addresses over an authenticated connection; a host that is not allowed is ignored. */
    fun withEndpoints(newLan: String?, newTunnel: String?) = copy(
        lan = newLan?.let(::parseHostPort) ?: if (newLan == null) null else lan,
        tunnel = newTunnel?.takeIf(::isTunnelHost) ?: if (newTunnel == null) null else tunnel,
    )

    fun toJson(): String = Json.encodeToString(serializer(), this)

    companion object {
        fun fromJson(text: String?): Pairing? =
            text?.let { runCatching { Json.decodeFromString(serializer(), it) }.getOrNull() }
    }
}

fun fromLink(link: PairLink, device: String, secret: String) =
    Pairing(device, secret, link.lan, link.fp, link.tunnel)

fun sha256Hex(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

/** Whether a certificate is the one the QR named: its SHA-256 must equal [fp], nothing else matters (REQ-81). */
fun certMatches(der: ByteArray, fp: String): Boolean =
    MessageDigest.isEqual(sha256Hex(der).toByteArray(), fp.toByteArray())

/** The digits the phone shows and the person at the PC compares. */
fun pairingCode(random: SecureRandom = SecureRandom()): String = "%06d".format(random.nextInt(1_000_000))

/** Why the link failed, as the screens say it. */
enum class LinkFail { Expired, Declined, Locked, Revoked, Unreachable, Versions }

fun Denied.toFail(): LinkFail = when (this) {
    Denied.BadToken -> LinkFail.Expired
    Denied.Declined, Denied.Timeout -> LinkFail.Declined
    Denied.LockedOut -> LinkFail.Locked
    Denied.BadSecret, Denied.Revoked -> LinkFail.Revoked
}
