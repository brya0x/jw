package dev.jw.mobile

import dev.jw.proto.LanAddr

/**
 * The paired PC: the device token and where to reach it. `host` is the tunnel's (empty until the PC has told the
 * phone, or when it never will), `lan` the wifi address with the pin. Either can be missing, not both.
 */
class StoredDevice(val host: String, val device: String, val lan: LanAddr? = null) {
    override fun toString() = "StoredDevice(host=$host, lan=$lan)"
}

interface TokenStore {
    /** Null when nothing is stored, or what is stored cannot be read any more. */
    fun load(): StoredDevice?
    fun save(device: StoredDevice)
    fun clear()
}
