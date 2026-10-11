package dev.jw.proto

/** What the QR holds: `jw://<tunnel host>/<pairing token>`. */
class QrPayload(val host: String, val token: String) {
    override fun toString() = "QrPayload(host=$host)"

    companion object {
        private val RE = Regex("^jw://([A-Za-z0-9.-]{1,253})/([0-9a-f]{32})$")

        /** Null when the text is not a jw QR. Never puts the text in a message. */
        fun parse(raw: String): QrPayload? {
            val m = RE.matchEntire(raw.trim()) ?: return null
            return QrPayload(m.groupValues[1].lowercase(), m.groupValues[2])
        }
    }
}
