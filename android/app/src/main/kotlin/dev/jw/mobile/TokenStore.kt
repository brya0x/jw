package dev.jw.mobile

/** The paired PC: tunnel host and the device token. */
class StoredDevice(val host: String, val device: String) {
    override fun toString() = "StoredDevice(host=$host)"
}

interface TokenStore {
    /** Null when nothing is stored, or what is stored cannot be read any more. */
    fun load(): StoredDevice?
    fun save(device: StoredDevice)
    fun clear()
}
