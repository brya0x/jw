package dev.jw.proto

/** The PC's address on the local network and the fingerprint of its certificate, which is the pin. */
class LanAddr(val host: String, val port: Int, val fp: String) {
    override fun toString() = "LanAddr($host:$port)"
    val hostPort get() = "$host:$port"
    override fun equals(other: Any?) = other is LanAddr && other.host == host && other.port == port && other.fp == fp
    override fun hashCode() = hostPort.hashCode() * 31 + fp.hashCode()
}

/** What a host may be: the two shapes the app ever connects to. */
object Hosts {
    private val TUNNEL = Regex("^[a-z0-9]+(?:-[a-z0-9]+)*(?:\\.[a-z0-9]+(?:-[a-z0-9]+)*)*\\.trycloudflare\\.com$")
    private val V4 = Regex("^(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})$")
    private val FP = Regex("^[0-9a-f]{64}$")

    /** A quick-tunnel host (`*.trycloudflare.com`); a named tunnel (OPEN-2) will need this widened. */
    fun isTunnel(host: String) = TUNNEL.matches(host)

    /** A private IPv4 address (10/8, 172.16/12, 192.168/16): the pairing token never goes to any other. */
    fun isPrivateV4(host: String): Boolean {
        val m = V4.matchEntire(host) ?: return false
        val o = m.groupValues.drop(1).map { it.toIntOrNull() ?: return false }
        if (o.any { it !in 0..255 }) return false
        return o[0] == 10 || (o[0] == 172 && o[1] in 16..31) || (o[0] == 192 && o[1] == 168)
    }

    /** A wifi address as the QR or `endpoints` gives it, or null when any part is not valid. */
    fun lan(host: String, port: Int, fp: String): LanAddr? =
        if (isPrivateV4(host) && port in 1..65535 && FP.matches(fp)) LanAddr(host, port, fp) else null
}

/**
 * What the QR holds (REQ-58): the pairing token and where to reach the PC, the wifi address with its
 * fingerprint, the tunnel host, or both (`jw://<ipv4>:<port>/<token>?fp=<sha256>[&t=<tunnel host>]` and
 * `jw://<tunnel host>/<token>`). At least one is set.
 */
class QrPayload(val token: String, val lan: LanAddr?, val tunnel: String?) {
    override fun toString() = "QrPayload(lan=$lan, tunnel=$tunnel)"

    companion object {
        private val TOKEN = "[0-9a-f]{32}"
        private val TUNNEL = Regex("^jw://([^/:?&]+)/($TOKEN)$")
        private val WIFI = Regex("^jw://([0-9.]+):(\\d{1,5})/($TOKEN)\\?fp=([0-9a-f]{64})(?:&t=([^&]+))?$")

        /** Null when the text is not a jw QR, or a host is not what it may be. Never puts the text in a message. */
        fun parse(raw: String): QrPayload? {
            val text = raw.trim()
            TUNNEL.matchEntire(text)?.let { m ->
                val host = m.groupValues[1]
                return if (Hosts.isTunnel(host)) QrPayload(m.groupValues[2], null, host) else null
            }
            val m = WIFI.matchEntire(text) ?: return null
            val lan = Hosts.lan(m.groupValues[1], m.groupValues[2].toIntOrNull() ?: return null, m.groupValues[4]) ?: return null
            val t = m.groupValues[5].ifEmpty { null }
            if (t != null && !Hosts.isTunnel(t)) return null
            return QrPayload(m.groupValues[3], lan, t)
        }
    }
}
